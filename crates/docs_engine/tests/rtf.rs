//! Importing `.rtf` files (tests/corpus/rtf, see scripts/corpus/gen_rtf.py).

use std::io::Read as _;
use std::path::{Path, PathBuf};

use docs_engine::{
    Align, Document, Object, Paragraph, ResolvedLine, Rgb, RunProps, VMerge, page_field, rtf,
};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/rtf")
        .join(name)
}

fn open(name: &str) -> Document {
    Document::open(&corpus(name)).unwrap_or_else(|e| panic!("{name}: {e:#}"))
}

/// A document from RTF text.
fn doc(rtf: &str) -> Document {
    Document::import_rtf(rtf.as_bytes(), Path::new("t.rtf")).expect("imports")
}

/// The generated `word/document.xml` (for formatting the model keeps
/// verbatim, like strike or highlight).
fn xml(rtf: &str) -> String {
    let import = rtf::to_docx(rtf.as_bytes()).expect("converts");
    part(&import.docx, "word/document.xml")
}

fn part(docx: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(docx)).unwrap();
    let mut s = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

fn texts(d: &Document) -> Vec<&str> {
    d.paragraphs().iter().map(|p| p.text.as_str()).collect()
}

fn para<'a>(d: &'a Document, starts: &str) -> (usize, &'a Paragraph) {
    d.paragraphs()
        .iter()
        .enumerate()
        .find(|(_, p)| p.text.starts_with(starts))
        .unwrap_or_else(|| panic!("no paragraph starting {starts:?}: {:?}", texts(d)))
}

/// The run formatting of the first occurrence of `what` in paragraph `p`.
fn run_at<'a>(p: &'a Paragraph, what: &str) -> &'a RunProps {
    let at = p
        .text
        .find(what)
        .unwrap_or_else(|| panic!("{what:?} not in {:?}", p.text));
    p.run_ranges()
        .find(|(r, _)| r.contains(&at))
        .map(|(_, run)| &run.props)
        .expect("a run")
}

// ── the Word-style sample ─────────────────────────────────────────────────

#[test]
fn imports_as_an_unsaved_copy_named_after_the_source() {
    let d = open("word_sample.rtf");
    assert_eq!(d.path(), None, "Save must not write the .rtf");
    assert!(d.is_imported());
    assert!(d.needs_save_as());
    assert_eq!(d.title(), "word_sample.rtf");
    assert_eq!(d.suggested_file_name(), "word_sample.docx");
    assert_eq!(d.source_path(), Some(corpus("word_sample.rtf").as_path()));
    assert!(!d.is_dirty());
    assert!(d.unsupported_content().is_empty());
    assert!(d.import_notes().is_empty(), "{:?}", d.import_notes());
}

#[test]
fn styles_and_headings() {
    let d = open("word_sample.rtf");
    let (_, title) = para(&d, "Quarterly Report");
    assert_eq!(title.props.style.as_deref(), Some("Title"));
    let r = d.resolve_para(title);
    assert_eq!(r.align, Align::Center);
    let (_, h1) = para(&d, "Introduction");
    assert_eq!(h1.props.style.as_deref(), Some("Heading1"));
    assert_eq!(d.styles.display_name("Heading1"), "heading 1");
    let run = d.resolve_run(h1, &h1.runs[0].props);
    assert_eq!(run.size_pt, 16.0);
    assert_eq!(run.font, "Calibri Light");
    assert_eq!(run.color, Rgb([47, 84, 150]));
    let (_, h2) = para(&d, "Lists");
    assert_eq!(h2.props.style.as_deref(), Some("Heading2"));
    // Body text: the document defaults (Calibri 11, 8 pt after, 1.08 lines).
    let (_, body) = para(&d, "Plain, bold");
    let r = d.resolve_para(body);
    assert_eq!(r.space_after_pt, 8.0);
    assert!(matches!(r.line, ResolvedLine::Multiple(m) if (m - 259.0 / 240.0).abs() < 0.01));
    let run = d.resolve_run(body, &body.runs[0].props);
    assert_eq!((run.font.as_str(), run.size_pt), ("Calibri", 11.0));
}

#[test]
fn character_formatting() {
    let d = open("word_sample.rtf");
    let (_, p) = para(&d, "Plain, bold");
    let r = |what| d.resolve_run(p, run_at(p, what));
    assert!(r("bold").bold && !r("bold").italic);
    assert!(r("italic").italic);
    assert!(r("underline").underline);
    assert_eq!(r("red").color, Rgb([255, 0, 0]));
    assert_eq!(r("16 pt").size_pt, 16.0);
    assert_eq!(r("Courier New").font, "Courier New");
    // A character style with its own formatting.
    let strong = run_at(p, "strong red");
    assert_eq!(strong.style.as_deref(), Some("StrongRed"));
    assert!(r("strong red").bold);
    // Hidden text is left out.
    assert!(!p.text.contains("hidden"));
    // What the model keeps verbatim.
    let x = |what| run_at(p, what).extras.iter().collect::<Vec<_>>().join("");
    assert!(x("strike").contains("<w:strike/>"));
    assert!(x("caps").contains("<w:caps/>"));
    assert!(x("small caps").contains("<w:smallCaps/>"));
    assert!(x("highlighted").contains("<w:highlight w:val=\"yellow\"/>"));
    assert!(x("shaded").contains("w:fill=\"DEEAF6\""));
    assert!(x("2,").contains("superscript"), "{}", x("2,"));
    assert!(x("2O").contains("subscript"));
    assert!(x("double").contains("<w:u w:val=\"double\"/>"));
}

#[test]
fn hyperlinks_keep_their_target() {
    let d = open("word_sample.rtf");
    let (_, p) = para(&d, "Plain, bold");
    let at = p.text.find("external link").unwrap();
    let run = p.run_ranges().find(|(r, _)| r.contains(&at)).unwrap().1;
    let link = run.link.as_deref().expect("a hyperlink");
    let id = link.split("r:id=\"").nth(1).unwrap().split('"').next().unwrap();
    let import = rtf::to_docx(&std::fs::read(corpus("word_sample.rtf")).unwrap()).unwrap();
    let rels = part(&import.docx, "word/_rels/document.xml.rels");
    assert!(
        rels.contains(&format!(
            "Id=\"{id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink\" Target=\"https://kantor.paparanai.id/\" TargetMode=\"External\""
        )),
        "{rels}"
    );
    // The link text keeps the field result's formatting.
    assert!(d.resolve_run(p, &run.props).underline);
}

#[test]
fn paragraph_formatting() {
    let d = open("word_sample.rtf");
    let (_, p) = para(&d, "Justified");
    let r = d.resolve_para(p);
    assert_eq!(r.align, Align::Justify);
    assert_eq!(r.space_before_pt, 6.0);
    assert_eq!(r.space_after_pt, 12.0);
    assert!(matches!(r.line, ResolvedLine::Multiple(m) if (m - 1.5).abs() < 0.01));
    assert_eq!(r.indent_left_pt, 36.0);
    assert_eq!(r.indent_right_pt, 18.0);
    assert_eq!(r.first_line_pt, 18.0);
    let (_, c) = para(&d, "Centred line");
    assert_eq!(c.text, "Centred line\u{2028}second line in the same paragraph");
    assert_eq!(d.resolve_para(c).align, Align::Center);
    let (_, t) = para(&d, "Right\t");
    assert_eq!(d.resolve_para(t).align, Align::Right);
    let tabs = t.props.extras.iter().collect::<Vec<_>>().join("");
    assert!(
        tabs.contains("<w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"8640\"/>"),
        "{tabs}"
    );
    let (_, b) = para(&d, "Before the page break");
    assert_eq!(b.text, "Before the page break\u{c}After the page break");
}

#[test]
fn lists_from_the_list_table() {
    let d = open("word_sample.rtf");
    let markers = d.list_markers();
    let marker = |s| markers[para(&d, s).0].clone();
    assert_eq!(marker("First bullet").as_deref(), Some("\u{2022}"));
    assert_eq!(marker("Second bullet").as_deref(), Some("\u{2022}"));
    assert!(marker("Nested bullet").is_some());
    assert_eq!(marker("First step").as_deref(), Some("1."));
    assert_eq!(marker("Second step").as_deref(), Some("2."));
    assert_eq!(marker("Sub-step").as_deref(), Some("2.a"));
    assert_eq!(marker("Introduction"), None);
    let (_, p) = para(&d, "First bullet");
    let r = d.resolve_para(p);
    assert_eq!((r.indent_left_pt, r.first_line_pt), (36.0, -18.0));
}

#[test]
fn tables_with_merges_borders_and_shading() {
    let d = open("word_sample.rtf");
    let (_, item) = para(&d, "Item");
    let id = item.cell.expect("in a table").table;
    let t = d.table(id).expect("table");
    assert_eq!(t.rows.len(), 4);
    assert_eq!(t.grid, vec![3113, 3005, 3006]);
    assert!(t.rows[0].header, "repeated header row");
    assert_eq!(t.rows[0].cells.len(), 3);
    // \clmgf + \clmrg: one cell over two columns.
    assert_eq!(t.rows[1].cells.len(), 2);
    assert_eq!(t.rows[1].cells[0].span, 2);
    // \clvmgf / \clvmrg.
    assert_eq!(t.rows[2].cells[2].vmerge, VMerge::Restart);
    assert_eq!(t.rows[3].cells[2].vmerge, VMerge::Continue);
    assert!(t.rows[0].cells[0].borders.top.is_some());
    let tc = t.rows[0].cells[0].tc_pr.as_deref().unwrap_or_default();
    assert!(tc.contains("w:fill=\"DEEAF6\""), "{tc}");
    let texts: Vec<&str> = d
        .paragraphs()
        .iter()
        .filter(|p| p.cell.is_some_and(|c| c.table == id))
        .map(|p| p.text.as_str())
        .collect();
    assert_eq!(
        texts,
        [
            "Item",
            "Owner",
            "Status",
            "Merged across two columns",
            "Done",
            "Kick-off",
            "Rina",
            "Spans two rows",
            "Review",
            "Budi",
            ""
        ]
    );
    let (_, head) = para(&d, "Item");
    assert!(d.resolve_run(head, &head.runs[0].props).bold);
}

#[test]
fn png_and_jpeg_pictures() {
    let d = open("word_sample.rtf");
    let (_, p) = para(&d, "\u{fffc} and");
    let images: Vec<_> = p
        .objects()
        .filter_map(|(_, o)| match o {
            Object::Image(i) => Some(i.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 2, "the WMF fallback is skipped");
    // \picwgoal600 = 600 twips = 381000 EMU.
    assert_eq!((images[0].cx, images[0].cy), (381_000, 190_500));
    assert_eq!((images[1].cx, images[1].cy), (285_750, 285_750));
    let png = d.image_bytes(&images[0].rel_id).expect("PNG bytes");
    assert!(png.starts_with(b"\x89PNG"));
    let jpg = d.image_bytes(&images[1].rel_id).expect("JPEG bytes");
    assert!(jpg.starts_with(b"\xFF\xD8"));
}

#[test]
fn sections_headers_and_footers() {
    let d = open("word_sample.rtf");
    let sections = d.sections();
    assert_eq!(sections.len(), 2);
    let first = &sections[0].props;
    assert_eq!((first.page.width, first.page.height), (11906, 16838));
    assert_eq!(first.page.margin_left, 1440);
    assert!(first.title_page);
    assert_eq!(first.header_dist, 708);
    let last = &sections[1].props;
    assert_eq!((last.page.width, last.page.height), (16838, 11906));
    assert_eq!(last.page.margin_left, 1134);
    let stories: Vec<String> = d
        .stories()
        .iter()
        .map(|s| {
            s.doc
                .paragraphs()
                .iter()
                .map(|p| p.text.clone())
                .collect::<String>()
        })
        .collect();
    assert!(stories.contains(&"Kantor RTF sample".to_owned()), "{stories:?}");
    assert!(stories.contains(&"First page header".to_owned()));
    let footer = d
        .stories()
        .iter()
        .find(|s| s.doc.paragraphs()[0].text.starts_with("Page "))
        .expect("footer");
    let p = &footer.doc.paragraphs()[0];
    assert_eq!(p.text, "Page \u{fffc}");
    let fields: Vec<_> = p.objects().filter_map(|(_, o)| page_field(o)).collect();
    assert_eq!(fields.len(), 1, "PAGE stays a page number field");
    assert_eq!(first.refs.len(), 3);
}

#[test]
fn unicode_text() {
    let d = open("word_sample.rtf");
    para(&d, "Chinese 中文, Japanese 日本語, Korean 한국어, emoji 😀👍🏽 and a flag 🇮🇩.");
    para(&d, "Justified, indented, 1.5 lines, 6 pt before and 12 pt after: Selamat datang di Kantor. Dokumen ini berbahasa Indonesia: «ä ö ü» — dan seterusnya.");
    // GBK bytes in a \fcharset134 font.
    para(&d, "中文 (SimSun, GBK bytes)");
}

#[test]
fn save_writes_a_docx_that_reopens_the_same() {
    let mut d = open("word_sample.rtf");
    assert!(d.save().is_err(), "no file of its own");
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("word_sample.docx");
    d.save_as(&out).unwrap();
    assert_eq!(d.path(), Some(out.as_path()));
    assert!(!d.is_imported());
    assert_eq!(d.title(), "word_sample.docx");
    let again = Document::open(&out).unwrap();
    assert_eq!(texts(&again), texts(&d));
    assert_eq!(again.sections().len(), 2);
    assert_eq!(again.stories().len(), d.stories().len());
    assert_eq!(again.list_markers(), d.list_markers());
    // The .rtf is untouched.
    assert!(std::fs::read(corpus("word_sample.rtf")).unwrap().starts_with(b"{\\rtf1"));
}

#[test]
fn recovered_imports_still_have_no_file() {
    let d = open("word_sample.rtf");
    let bytes = d.to_docx_bytes().unwrap();
    let r = Document::open_recovered(bytes, Some(corpus("word_sample.rtf")), Vec::new()).unwrap();
    assert_eq!(r.path(), None);
    assert_eq!(r.title(), "word_sample.rtf");
    assert!(r.is_dirty() && r.needs_save_as());
}

// ── other writers ─────────────────────────────────────────────────────────

#[test]
fn code_pages_and_charsets() {
    let d = open("charsets.rtf");
    assert_eq!(
        texts(&d),
        [
            "Привет, мир!",
            "こんにちは",
            "Αθξί",
            "Euro € sign and – dash"
        ]
    );
}

#[test]
fn word_95_pn_lists() {
    let d = open("pn_lists.rtf");
    assert_eq!(texts(&d), ["Apples", "Pears", "One", "Two", "Not a list"]);
    let m = d.list_markers();
    assert_eq!(
        m,
        [
            Some("\u{2022}".to_owned()),
            Some("\u{2022}".to_owned()),
            Some("1.".to_owned()),
            Some("2.".to_owned()),
            None
        ]
    );
}

#[test]
fn libreoffice_files() {
    let f = open("lo_features.rtf");
    para(&f, "Plain, bold, italic, red 14 pt. Indonesian: Rapat dimulai pukul sembilan. 中文 日本語 😀");
    let (_, red) = para(&f, "Plain, bold");
    let r = f.resolve_run(red, run_at(red, "red 14"));
    assert_eq!((r.size_pt, r.color), (14.0, Rgb([0xC0, 0, 0])));
    let m = f.list_markers();
    assert_eq!(m[para(&f, "Alpha").0].as_deref(), Some("\u{2022}"));
    assert_eq!(m[para(&f, "Two").0].as_deref(), Some("2."));
    let (_, merged) = para(&f, "Merged");
    let t = f.table(merged.cell.unwrap().table).unwrap();
    assert_eq!(t.rows.len(), 3);
    assert_eq!(t.rows[0].cells[0].span, 2);
    assert_eq!(t.rows[1].cells[2].vmerge, VMerge::Restart);
    assert_eq!(f.sections().len(), 2);
    assert!(
        f.paragraphs()
            .iter()
            .any(|p| p.objects().any(|(_, o)| matches!(o, Object::Image(_))))
    );

    let r = open("lo_report.rtf");
    assert_eq!(r.sections().len(), 3);
    let (_, h) = para(&r, "1. Section heading 1");
    assert_eq!(h.props.style.as_deref(), Some("Heading1"));
    let (_, wide) = para(&r, "R0C0");
    assert_eq!(r.table(wide.cell.unwrap().table).unwrap().grid.len(), 8);
    let footers = r
        .stories()
        .iter()
        .filter(|s| {
            s.doc
                .paragraphs()
                .iter()
                .any(|p| p.objects().any(|(_, o)| page_field(o).is_some()))
        })
        .count();
    assert!(footers > 0, "PAGE / NUMPAGES fields survive");

    let l = open("lo_letter.rtf");
    para(&l, "Kepada Yth.\u{2028}Bapak Direktur\u{2028}PT Mitra Abadi");
    let inv = open("lo_invoice.rtf");
    para(&inv, "Bill to:\tPT Mitra Abadi\u{2028}Date:\t12/11/2026");
    let (_, total) = para(&inv, "Total:");
    assert_eq!(total.text, "Total:\tRp 18.000.000");
}

// ── the parser on small inputs ────────────────────────────────────────────

#[test]
fn unicode_escapes_and_fallbacks() {
    // \uN with the default one-character fallback, a surrogate pair, \uc0,
    // \uc2 skipping two \'hh, and a negative N.
    let d = doc(concat!(
        r"{\rtf1\ansi Kopi \u9749? dan teh\par ",
        r"\u-10179?\u-8704? senyum\par ",
        r"{\uc0 \u20320\u22909}\par ",
        r"{\uc2 \u8364\'80\'80 }x\par ",
        r"\u-4064?}"
    ));
    assert_eq!(
        texts(&d),
        ["Kopi \u{2615} dan teh", "\u{1F600} senyum", "\u{4F60}\u{597D}", "\u{20AC} x", "\u{F020}"]
    );
}

#[test]
fn escapes_and_special_characters() {
    let d = doc(
        r"{\rtf1\ansi a\{b\}c\\d\~e\_f\-g\emdash\endash\lquote\rquote\ldblquote\rdblquote\bullet\tab h\'e9}",
    );
    assert_eq!(
        texts(&d),
        ["a{b}c\\d\u{a0}e\u{2011}f\u{ad}g—–‘’“”•\th\u{e9}"]
    );
}

#[test]
fn mac_and_default_code_pages() {
    assert_eq!(texts(&doc(r"{\rtf1\mac caf\'8e}")), ["café"]);
    assert_eq!(texts(&doc(r"{\rtf1\ansi caf\'e9}")), ["café"]);
    assert_eq!(texts(&doc(r"{\rtf1\ansi\ansicpg1250 \'9a}")), ["š"]);
}

#[test]
fn unknown_destinations_are_skipped() {
    let d = doc(
        r"{\rtf1{\*\unknownthing secret}{\*\generator Gen;}{\info{\title T}}visible{\*\bkmkstart b}{\*\bkmkend b}\par}",
    );
    assert_eq!(texts(&d), ["visible"]);
}

#[test]
fn footnotes_and_comments_are_noted() {
    let d = doc(r"{\rtf1 text{\super\chftn}{\footnote\pard\plain note}\par}");
    assert_eq!(texts(&d), ["text"]);
    assert_eq!(d.import_notes(), ["footnotes were left out"]);
    let d = doc(r"{\rtf1 {\pict\wmetafile8 0100}x}");
    assert_eq!(
        d.import_notes(),
        ["pictures in formats other than PNG and JPEG were left out"]
    );
}

#[test]
fn line_spacing_kinds() {
    let d = doc(r"{\rtf1\pard\sl-360 exact\par\pard\sl300\slmult0 least\par\pard\sl480\slmult1 double\par}");
    let line = |i: usize| d.resolve_para(&d.paragraphs()[i]).line;
    assert_eq!(line(0), ResolvedLine::Exact(18.0));
    assert_eq!(line(1), ResolvedLine::AtLeast(15.0));
    assert_eq!(line(2), ResolvedLine::Multiple(2.0));
}

#[test]
fn keep_options_and_page_break_before() {
    let x = xml(r"{\rtf1\pard\keepn\keep\pagebb x\par}");
    assert!(x.contains("<w:keepNext/><w:keepLines/><w:pageBreakBefore/>"), "{x}");
    let d = doc(r"{\rtf1\pard\pagebb x\par}");
    assert!(d.resolve_para(&d.paragraphs()[0]).page_break_before);
}

#[test]
fn outline_level_without_a_heading_style() {
    let x = xml(r"{\rtf1\pard\outlinelevel2 x\par}");
    assert!(x.contains("<w:outlineLvl w:val=\"2\"/>"), "{x}");
}

#[test]
fn tables_without_row_definitions_or_final_row() {
    // Cells with no \cellx, and a table the file never closes.
    let d = doc(r"{\rtf1\pard\intbl a\cell b\cell\row\pard\intbl c\cell d}");
    let t = d.table(d.paragraphs()[0].cell.unwrap().table).unwrap();
    assert_eq!(t.rows.len(), 2);
    assert_eq!(texts(&d), ["a", "b", "c", "d", ""]);
}

#[test]
fn nested_tables_are_flattened() {
    let d = doc(
        r"{\rtf1\trowd\cellx3000\pard\intbl outer\par\pard\intbl\itap2 inner1\nestcell inner2\nestcell{\*\nesttableprops\trowd\cellx1000\cellx2000\nestrow}\pard\intbl\itap1 \cell\row}",
    );
    assert_eq!(&texts(&d)[..4], ["outer", "inner1", "inner2", ""]);
    assert!(
        d.import_notes()
            .contains(&"tables inside tables were flattened into paragraphs".to_owned())
    );
}

#[test]
fn hyperlink_to_a_bookmark() {
    let x = xml(r#"{\rtf1{\field{\*\fldinst HYPERLINK \\l "intro"}{\fldrslt go}}\par}"#);
    assert!(x.contains("<w:hyperlink w:anchor=\"intro\">"), "{x}");
}

#[test]
fn other_fields_show_their_result() {
    let d = doc(r#"{\rtf1 Date: {\field{\*\fldinst DATE \\@ "d MMMM yyyy"}{\fldrslt 13 November 2026}}\par}"#);
    assert_eq!(texts(&d), ["Date: 13 November 2026"]);
}

#[test]
fn page_size_margins_and_landscape() {
    let d = doc(r"{\rtf1\paperw16838\paperh11906\landscape\margl720\margr720\margt360\margb360 x}");
    let s = &d.sections()[0].props;
    assert_eq!((s.page.width, s.page.height), (16838, 11906));
    assert_eq!(
        (s.page.margin_left, s.page.margin_top),
        (720, 360)
    );
    let x = xml(r"{\rtf1\paperw16838\paperh11906\landscape x}");
    assert!(x.contains("w:orient=\"landscape\""));
}

#[test]
fn even_and_first_page_headers() {
    let d = doc(
        r"{\rtf1\facingp\titlepg{\headerl even}{\headerr odd}{\headerf first}{\footer foot}body}",
    );
    assert_eq!(d.stories().len(), 4);
    let s = &d.sections()[0].props;
    assert!(s.title_page);
    let types: Vec<String> = s.refs.iter().map(|(_, t, _)| format!("{t:?}")).collect();
    assert!(types.contains(&"Even".to_owned()), "{types:?}");
    assert!(types.contains(&"First".to_owned()));
}

// ── malformed input ───────────────────────────────────────────────────────

#[test]
fn not_rtf_is_an_error() {
    assert!(rtf::to_docx(b"hello").is_err());
    assert!(rtf::to_docx(b"").is_err());
    assert!(rtf::is_rtf(b"\xEF\xBB\xBF  {\\rtf1 x}"));
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("fake.rtf");
    std::fs::write(&p, b"PK\x03\x04 not rtf").unwrap();
    let err = Document::open(&p).err().expect("fails").to_string();
    assert!(err.contains("not an RTF file"), "{err}");
}

#[test]
fn truncated_file_keeps_what_was_read() {
    let d = open("truncated.rtf");
    para(&d, "Quarterly Report");
    para(&d, "Kick-off");
    para(&d, "Rina");
}

#[test]
fn hostile_input_never_panics() {
    let mut cases: Vec<Vec<u8>> = vec![
        b"{\\rtf1".to_vec(),
        b"{\\rtf1 }}}}}} after".to_vec(),
        b"{\\rtf1 \\'".to_vec(),
        b"{\\rtf1 \\'zz \\u \\u-99999999999 \\uc99999 \\fs-5 \\li-2147483648x}".to_vec(),
        b"{\\rtf1 \\bin999999999 ".to_vec(),
        b"{\\rtf1{\\pict\\pngblip\\picwgoal-5\\pichgoal99999999 abc}}".to_vec(),
        b"{\\rtf1{\\pict\\jpegblip\\bin4 \xff\xd8\xff\xd9}}".to_vec(),
        b"{\\rtf1\\cellx-5\\cellx-10\\cell\\cell\\row\\row\\clmrg\\cellx5\\cell\\row}".to_vec(),
        b"{\\rtf1\\clvmrg\\clmrg\\cellx100\\intbl x\\cell\\row}".to_vec(),
        b"{\\rtf1{\\field{\\fldrslt{\\field{\\fldrslt x}}}}{\\field}{\\*\\fldinst HYPERLINK}}".to_vec(),
        b"{\\rtf1 \\ls999\\ilvl99 x\\par{\\*\\listoverridetable{\\listoverride\\listid5\\ls1}}\\ls1 y}".to_vec(),
        b"{\\rtf1{\\*\\listtable{\\list{\\listlevel{\\leveltext\\'ff;}}\\listid1}}{\\*\\listoverridetable{\\listoverride\\listid1\\ls1}}\\ls1\\ilvl8 x}".to_vec(),
        b"{\\rtf1{\\header{\\header nested}}{\\footer\\sect}\\sect\\sect}".to_vec(),
        b"{\\rtf1{\\colortbl;\\red999\\green-1;}\\cf1\\cf99\\highlight1\\cb7 x}".to_vec(),
        b"{\\rtf1{\\fonttbl{\\f0\\fcharset128 \x82;}}\\f0 \\'82}".to_vec(),
        b"{\\rtf1 \\u55357 \\par \\u56832 x}".to_vec(),
        b"{\\rtf1{\\stylesheet{\\s1 heading 1}{\\s1 heading 1;}{\\cs2;}{;}}\\s1 x\\par\\s77 y}".to_vec(),
    ];
    // Deeply nested groups.
    let mut deep = b"{\\rtf1 ".to_vec();
    deep.extend(std::iter::repeat_n(b'{', 100_000));
    deep.extend(b"x");
    cases.push(deep);
    // Every prefix of the sample (truncation at any point) and
    // deterministic byte mutations of it.
    let sample = std::fs::read(corpus("word_sample.rtf")).unwrap();
    for cut in (0..sample.len()).step_by(97) {
        cases.push(sample[..cut].to_vec());
    }
    let mut seed: u64 = 0x5eed;
    for _ in 0..60 {
        let mut m = sample.clone();
        for _ in 0..40 {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let at = (seed >> 33) as usize % m.len();
            let b = [b'{', b'}', b'\\', b'0', b'\'', b'-', b'x', 0xff][(seed >> 20) as usize % 8];
            m[at] = b;
        }
        cases.push(m);
    }
    for (i, case) in cases.iter().enumerate() {
        let mut c = case.clone();
        if !rtf::is_rtf(&c) {
            c.splice(0..0, b"{\\rtf1 ".iter().copied());
        }
        let d = Document::import_rtf(&c, Path::new("x.rtf"))
            .unwrap_or_else(|e| panic!("case {i}: {e:#}"));
        assert!(d.len() >= 1);
    }
}
