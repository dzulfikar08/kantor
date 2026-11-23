//! Importing OpenDocument text: the corpus in tests/corpus/odt (made by
//! tests/corpus/odt/make_fixtures.py: flat ODF written by hand, saved as
//! .odt by LibreOffice), saving the import as `.docx`, and damaged files.

use std::path::{Path, PathBuf};

use docs_engine::{Align, Document, HfKind, Object, VMerge, page_field};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/odt")
        .join(name)
}

fn open(name: &str) -> Document {
    Document::open(&corpus(name)).unwrap_or_else(|e| panic!("{name}: {e:#}"))
}

fn texts(doc: &Document) -> Vec<String> {
    doc.paragraphs()
        .iter()
        .map(|p| p.text.replace('\u{FFFC}', ""))
        .collect()
}

#[test]
fn text_formatting_styles_and_headings() {
    for name in ["text.odt", "text.fodt"] {
        let doc = open(name);
        assert!(doc.is_imported(), "{name}");
        assert!(doc.path().is_none(), "Save must not write over the .odt");
        assert!(doc.needs_save_as());
        assert_eq!(doc.title(), name);
        assert_eq!(doc.source_path(), Some(corpus(name).as_path()));
        assert!(doc.import_notes().is_empty(), "{:?}", doc.import_notes());

        let t = texts(&doc);
        assert_eq!(&t[..3], ["Laporan Kantor", "Ringkasan", "Detail"]);
        for (i, h) in ["Heading1", "Heading2", "Heading3"].iter().enumerate() {
            assert_eq!(doc.paragraphs()[i].props.style.as_deref(), Some(*h));
        }
        let p = &doc.paragraphs()[3];
        let run = |s: &str| {
            let at = p
                .text
                .find(s)
                .unwrap_or_else(|| panic!("{s} in {:?}", p.text));
            p.props_at(at + 1)
        };
        assert_eq!(run("bold").bold, Some(true));
        assert_eq!(run("italic").italic, Some(true));
        assert_eq!(run("underlined").underline, Some(true));
        assert!(format!("{:?}", run("double").extras).contains("double"));
        assert!(format!("{:?}", run("struck").extras).contains("w:strike"));
        assert_eq!(run("big red").size, Some(36));
        assert_eq!(
            run("big red").color.map(|c| c.hex()).as_deref(),
            Some("C00000")
        );
        assert_eq!(
            run("Courier New").fonts.ascii.as_deref(),
            Some("Courier New")
        );
        assert!(format!("{:?}", run("highlighted").extras).contains("yellow"));
        assert!(format!("{:?}", run("shaded").extras).contains("CCFFCC"));
        assert!(format!("{:?}", run("small caps").extras).contains("smallCaps"));
        assert_eq!(run("code style").style.as_deref(), Some("Code"));
        assert_eq!(run("and italic").style.as_deref(), Some("Strong"));
        assert_eq!(run("and italic").italic, Some(true));
        assert_eq!(run("Plain").bold, None);

        assert_eq!(doc.paragraphs()[4].props.align, Some(Align::Center));
        assert_eq!(doc.paragraphs()[5].props.align, Some(Align::Right));
        let j = &doc.paragraphs()[6].props;
        assert_eq!(j.align, Some(Align::Justify));
        assert_eq!((j.before, j.after), (Some(240), Some(120)));
        assert_eq!(j.line.map(|l| l.line), Some(360));
        assert_eq!(j.indent.left, Some(1134));
        assert_eq!(j.indent.right, Some(567));
        assert_eq!(j.indent.first, Some(283));

        // Tabs counted from the indent (1 cm) as LibreOffice does.
        let tabs = doc.tab_stops(&doc.paragraphs()[7]);
        let pos: Vec<i32> = tabs.iter().map(|t| t.pos).collect();
        assert_eq!(pos, [3402, 6236], "{tabs:?}");
        assert_eq!(
            t[7],
            format!(
                "Name:\tKantor\t12{}Second line with   spaces.",
                docs_engine::LINE_BREAK
            )
        );
        assert_eq!(doc.paragraphs()[8].props.style.as_deref(), Some("BodyText"));
        assert_eq!(doc.paragraphs()[9].props.style.as_deref(), Some("NoteBox"));
        assert_eq!(doc.paragraphs()[10].props_at(1).size, Some(18));
        assert!(t.contains(&"Bahasa Indonesia: Selamat pagi, apa kabar? Rp 1.250.000,00".into()));
        assert!(t.contains(&"CJK: 你好世界 こんにちは 안녕하세요".into()));
        assert!(t.contains(&"Emoji: 😀 👍🏽 ✓".into()));
        assert!(t.contains(&"Visit the Kantor site or go to the target.".into()));
        let last = doc.paragraphs().last().unwrap();
        assert_eq!(last.props.page_break_before, Some(true));
        assert_eq!(t.last().unwrap(), "After a page break.");
    }
}

#[test]
fn styles_keep_their_parents() {
    let doc = open("text.odt");
    let bytes = doc.to_docx_bytes().unwrap();
    let styles = part(&bytes, "word/styles.xml");
    // Note box is based on Body Text, which is based on Normal.
    let note = &styles[styles.find("w:styleId=\"NoteBox\"").unwrap()..];
    let note = &note[..note.find("</w:style>").unwrap()];
    assert!(note.contains("<w:basedOn w:val=\"BodyText\"/>"), "{note}");
    assert!(note.contains("<w:i/>"), "{note}");
    assert!(note.contains("w:fill=\"EEEEEE\""), "{note}");
    // Heading 1: 130 % of Heading's 14 pt, bold.
    let h1 = &styles[styles.find("w:styleId=\"Heading1\"").unwrap()..];
    let h1 = &h1[..h1.find("</w:style>").unwrap()];
    assert!(h1.contains("<w:sz w:val=\"36\"/>"), "{h1}");
    assert!(h1.contains("<w:outlineLvl w:val=\"0\"/>"), "{h1}");
}

fn part(docx: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(docx)).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name(name).unwrap(), &mut xml).unwrap();
    xml
}

#[test]
fn bullets_numbers_and_chapter_numbering() {
    let doc = open("lists.odt");
    assert!(doc.import_notes().is_empty(), "{:?}", doc.import_notes());
    let t = texts(&doc);
    let m = doc.list_markers();
    let m: Vec<Option<&str>> = m.iter().map(Option::as_deref).collect();
    let got: Vec<(&str, Option<&str>)> = t.iter().map(String::as_str).zip(m).collect();
    assert_eq!(
        got,
        [
            ("Chapter one", Some("1.")),
            ("Shopping:", None),
            ("Rice", Some("•")),
            ("Fruit", Some("•")),
            ("Mango", Some("◦")),
            ("Papaya", Some("◦")),
            ("Coffee", Some("•")),
            ("Steps", Some("1.1")),
            ("First step", Some("1.")),
            ("Second step", Some("2.")),
            ("Sub step", Some("(a)")),
            ("Another sub step", Some("(b)")),
            ("Third step", Some("3.")),
            ("More about the third step.", None),
            ("An interruption.", None),
            ("Fourth step", Some("4.")),
            ("A list header", None),
            ("Fifth, restarted", Some("5.")),
            ("Sixth", Some("6.")),
            ("Roman one", Some("I)")),
            ("Roman two", Some("II)")),
            ("Chapter two", Some("2.")),
        ]
    );
    // An item's later paragraph lines up with its text.
    let more = &doc.paragraphs()[13].props;
    assert_eq!(more.indent.left, Some(720));
    assert!(more.num.is_none());
}

#[test]
fn tables_with_spans_and_header_rows() {
    let doc = open("tables.odt");
    let t = texts(&doc);
    assert_eq!(t[0], "Before the table.");
    assert_eq!(t.last().unwrap(), "After the table.");
    let cell = doc.paragraphs()[1].cell.expect("a table");
    let table = doc.table(cell.table).unwrap();
    assert_eq!(table.rows.len(), 4);
    // 3, 5 and 7 cm.
    let grid: Vec<i64> = table.grid.iter().map(|g| *g as i64).collect();
    for (g, want) in grid.iter().zip([1701, 2835, 3969]) {
        assert!((g - want).abs() < 15, "{grid:?}");
    }
    assert_eq!(table.rows[0].cells.len(), 2);
    assert_eq!(table.rows[0].cells[1].span, 2);
    assert_eq!(table.rows[2].cells[0].vmerge, VMerge::Restart);
    assert_eq!(table.rows[3].cells[0].vmerge, VMerge::Continue);
    assert!(table.rows[0].cells[0].borders.top.is_some_and(|b| b.on));
    let in_cells: Vec<&str> = doc
        .paragraphs()
        .iter()
        .filter(|p| p.cell.is_some_and(|c| c.table == cell.table))
        .map(|p| p.text.as_str())
        .collect();
    assert!(in_cells.starts_with(&["Item", "Details", "#", "Qty", "Note", "Merged down"]));
    assert_eq!(doc.paragraphs()[1].props_at(1).bold, Some(true));
    let xml = part(&doc.to_docx_bytes().unwrap(), "word/document.xml");
    assert_eq!(xml.matches("<w:tblHeader/>").count(), 2);
    assert!(xml.contains("w:fill=\"DDDDDD\""));
    assert!(xml.contains("<w:vAlign w:val=\"center\"/>"));
    assert!(
        xml.contains("<w:jc w:val=\"center\"/></w:tblPr>")
            || xml.contains("<w:jc w:val=\"center\"/><w:tblLayout")
    );
    // The inner table stays inside its cell.
    let outer = xml.find("<w:tbl>").unwrap();
    let inner = xml[outer + 7..]
        .find("<w:tbl>")
        .map(|i| i + outer + 7)
        .unwrap();
    assert!(xml[outer..inner].contains("<w:tc>"));
    assert!(xml[inner..].contains(">b<"));
}

#[test]
fn page_styles_headers_footers_and_columns() {
    let doc = open("page.odt");
    let s = doc.sections();
    assert_eq!(
        s.len(),
        4,
        "{:?}",
        s.iter().map(|s| s.paras.clone()).collect::<Vec<_>>()
    );
    let p = s[0].props.page;
    // US Letter landscape, 1.5 / 1.5 / 2.5 / 2 cm; the body starts below
    // the 0.5 cm header and its 0.5 cm gap.
    assert_eq!((p.width, p.height), (15840, 12240));
    assert_eq!((p.margin_left, p.margin_right), (1417, 1134));
    assert_eq!(s[0].props.header_dist, 850);
    assert_eq!(p.margin_top, 850 + 283 + 283);
    assert!(s[0].props.title_page);
    let a4 = s[1].props.page;
    assert_eq!((a4.width, a4.height), (11906, 16838));
    assert_eq!(s[2].props.columns.num, 2);
    assert!(s[2].props.columns.sep);
    assert!(s[2].props.columns.space.abs_diff(567) < 5);
    assert_eq!(s[3].props.columns.num, 1);
    let story = |kind: HfKind, text: &str| {
        doc.stories()
            .iter()
            .find(|st| st.kind == kind && st.doc.paragraphs()[0].text.contains(text))
            .unwrap_or_else(|| panic!("{text}"))
    };
    let header = story(HfKind::Header, "Kantor report, page");
    assert!(
        header.doc.paragraphs()[0]
            .objects()
            .any(|(_, o)| page_field(o).is_some()),
        "PAGE field"
    );
    story(HfKind::Header, "Cover page header");
    story(HfKind::Footer, "Pages:");
    story(HfKind::Footer, "Portrait footer");
    // The portrait page has no header of its own: an empty one, not the
    // landscape page's.
    let first_header = &s[0].props.refs;
    let portrait = &s[1].props.refs;
    let id = |refs: &[(HfKind, docs_engine::HfType, String)]| {
        refs.iter()
            .find(|r| r.0 == HfKind::Header && r.1 == docs_engine::HfType::Default)
            .map(|r| r.2.clone())
    };
    assert_ne!(id(first_header), id(portrait));
}

#[test]
fn notes_comments_changes_and_fields() {
    let doc = open("notes.odt");
    assert!(doc.import_notes().is_empty(), "{:?}", doc.import_notes());
    let notes: Vec<String> = doc
        .notes()
        .iter()
        .filter(|n| !n.separator)
        .map(|n| n.doc.paragraphs()[0].text.trim().to_owned())
        .collect();
    assert!(
        notes.iter().any(|n| n.ends_with("The footnote text.")),
        "{notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.ends_with("The endnote text.")),
        "{notes:?}"
    );
    let c = doc.comments();
    assert_eq!(c.len(), 2, "{c:?}");
    assert_eq!(
        (c[0].author.as_str(), c[0].initials.as_str()),
        ("Budi Santoso", "BS")
    );
    assert_eq!(c[0].text, "Check this span.");
    assert_eq!(c[0].date, "2026-11-13T09:00:00Z");
    assert_eq!(c[1].text, "A point comment.\nSecond line.");
    let anchors = doc.comment_anchors();
    let a = anchors[0].1;
    let covered =
        doc.paragraphs()[a.start.para].text[a.start.offset..a.end.offset].replace('\u{FFFC}', "");
    assert_eq!(covered, "span of text");

    let changes = doc.changes();
    let summary: Vec<(String, String, String)> = changes
        .iter()
        .map(|c| (format!("{:?}", c.kind), c.author.clone(), c.text.clone()))
        .collect();
    assert!(
        summary
            .iter()
            .any(|(k, a, t)| k.contains("Ins") && a == "Budi" && t == "inserted words"),
        "{summary:?}"
    );
    assert!(
        summary
            .iter()
            .any(|(k, a, t)| k.contains("Del") && a == "Sari" && t == "old words "),
        "{summary:?}"
    );
    let t = texts(&doc);
    // NUMPAGES shows the laid-out count, not the file's.
    assert_eq!(t[0].replace("  ", " 1 "), "Title: Catatan rapat, 1 page.");
    let xml = part(&doc.to_docx_bytes().unwrap(), "word/document.xml");
    assert!(xml.contains("w:instr=\" NUMPAGES \""));
    assert!(xml.contains("<w:bookmarkStart w:id=\"1\" w:name=\"span\"/>"));
    // Document properties.
    let core = part(&doc.to_docx_bytes().unwrap(), "docProps/core.xml");
    assert!(
        core.contains("<dc:title>Catatan rapat</dc:title>"),
        "{core}"
    );
}

/// Every floating object of the document.
fn floats(doc: &Document) -> Vec<docs_engine::Float> {
    doc.paragraphs()
        .iter()
        .flat_map(|p| p.objects())
        .filter_map(|(_, o)| match o {
            Object::Float(f) => Some((**f).clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn pictures_frames_and_left_out_shapes() {
    use docs_engine::float::{Place, RelH, RelV};
    use docs_engine::{FloatKind, Wrap};
    let doc = open("pictures.odt");
    assert_eq!(doc.import_notes(), ["drawing shapes were left out"]);
    let inline = doc.paragraphs()[0]
        .objects()
        .find_map(|(_, o)| match o {
            Object::Image(i) => Some(i.clone()),
            _ => None,
        })
        .expect("an inline picture");
    // 2 x 1 cm.
    assert!((inline.cx - 720_000).abs() < 1_000, "{}", inline.cx);
    assert!((inline.cy - 360_000).abs() < 1_000, "{}", inline.cy);
    assert!(
        doc.image_bytes(&inline.rel_id)
            .unwrap()
            .starts_with(b"\x89PNG")
    );

    let f = floats(&doc);
    assert_eq!(f.len(), 3, "{f:?}");
    let page = &f[0];
    assert_eq!(page.at.h.0, RelH::Page);
    assert_eq!(page.at.v.0, RelV::Page);
    assert_eq!(page.at.wrap, Wrap::None);
    let right = &f[1];
    assert!(matches!(right.kind, FloatKind::Picture { .. }));
    assert_eq!(right.at.wrap, Wrap::Square);
    assert_eq!(right.at.h.0, RelH::Column);
    assert!(!matches!(right.at.h.1, Place::Offset(_)), "aligned right");
    let frame = &f[2];
    assert_eq!(frame.kind, FloatKind::Shape);
    let lines: Vec<String> = frame
        .text
        .as_ref()
        .map(|t| t.doc.paragraphs().iter().map(|p| p.text.clone()).collect())
        .unwrap_or_default();
    assert_eq!(lines, ["Text in a frame.", "Second frame line."]);
    let (fill, _) = frame.shape_xml();
    assert!(fill.unwrap_or_default().contains("FFFFCC"));
    // The SVG came with LibreOffice's PNG fallback.
    let svg_para = &doc.paragraphs()[4];
    assert!(
        svg_para
            .objects()
            .any(|(_, o)| matches!(o, Object::Image(_)))
    );
}

#[test]
fn template_and_save_as_docx() {
    let dir = tempfile::tempdir().unwrap();
    let ott = dir.path().join("Memo.ott");
    std::fs::copy(corpus("text.odt"), &ott).unwrap();
    assert!(docs_engine::is_import_path(&ott));
    let mut doc = Document::open(&ott).unwrap();
    assert_eq!(doc.title(), "Memo.ott");
    assert_eq!(doc.suggested_file_name(), "Memo.docx");
    assert!(doc.save().is_err(), "Save needs a new .docx name");
    let out = dir.path().join("Memo.docx");
    doc.save_as(&out).unwrap();
    assert!(!doc.is_imported());
    let again = Document::open(&out).unwrap();
    assert!(!again.is_imported());
    assert_eq!(texts(&again), texts(&doc));
    assert!(
        !again.has_unsupported_content(),
        "{:?}",
        again.unsupported_content()
    );

    // An .odt under a .docx name is told apart by its content.
    let odd = dir.path().join("renamed.docx");
    std::fs::copy(corpus("lists.odt"), &odd).unwrap();
    assert!(docs_engine::is_import_path(&odd));
    let doc = Document::open(&odd).unwrap();
    assert!(doc.is_imported());
    assert_eq!(texts(&doc)[0], "Chapter one");
}

#[test]
fn every_fixture_saves_without_unsupported_content() {
    for name in [
        "text.odt",
        "lists.odt",
        "tables.odt",
        "page.odt",
        "notes.odt",
        "pictures.odt",
    ] {
        let doc = open(name);
        assert!(
            !doc.has_unsupported_content(),
            "{name}: {:?}",
            doc.unsupported_content()
        );
        let bytes = doc.to_docx_bytes().unwrap();
        let back = Document::open_recovered(bytes, Some(corpus(name)), Vec::new()).unwrap();
        assert_eq!(texts(&back), texts(&doc), "{name}");
    }
}

fn flat(body: &str) -> Vec<u8> {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
 xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
 xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
 xmlns:svg="urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0"
 xmlns:form="urn:oasis:names:tc:opendocument:xmlns:form:1.0"
 xmlns:xlink="http://www.w3.org/1999/xlink"
 office:mimetype="application/vnd.oasis.opendocument.text">
<office:body><office:text>{body}</office:text></office:body></office:document>"#
    )
    .into_bytes()
}

fn import(bytes: &[u8]) -> anyhow::Result<Document> {
    Document::import_odt(bytes, Path::new("x.odt"))
}

#[test]
fn flat_xml_without_styles_and_left_out_pictures() {
    let svg = "PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciLz4=";
    let doc = import(&flat(&format!(
        "<text:p>A <text:s text:c=\"2\"/>b</text:p>\
         <text:p><draw:frame text:anchor-type=\"as-char\" svg:width=\"1cm\" svg:height=\"1cm\">\
         <draw:image><office:binary-data>{svg}</office:binary-data></draw:image></draw:frame></text:p>\
         <text:p>  leading   spaces  collapse </text:p>\
         <text:p>Starred<text:note text:note-class=\"footnote\"><text:note-citation text:label=\"*\">*</text:note-citation>\
         <text:note-body><text:p>A custom mark.</text:p></text:note-body></text:note></text:p>"
    )))
    .unwrap();
    let t = texts(&doc);
    assert_eq!(t[0], "A   b");
    assert_eq!(t[2], "leading spaces collapse ");
    // A custom note mark is typed after the reference, as in Word.
    assert_eq!(t[3], "Starred*");
    let xml = part(&doc.to_docx_bytes().unwrap(), "word/document.xml");
    assert!(xml.contains("w:customMarkFollows=\"1\""), "{xml}");
    assert_eq!(
        doc.import_notes(),
        ["pictures in formats Kantor can't show (SVG, TIFF) were left out"]
    );
}

#[test]
fn refuses_other_files_and_errors_cleanly() {
    let e = format!("{:#}", import(b"{\\rtf1 hello}").err().unwrap());
    assert!(e.contains("not an OpenDocument text file"), "{e}");
    // A spreadsheet isn't a text document.
    let ods = std::fs::read(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xls/formatted.ods"),
    )
    .unwrap();
    assert!(import(&ods).is_err());
    assert!(import(&flat("<text:p>unclosed")).is_err());
    let good = std::fs::read(corpus("text.odt")).unwrap();
    for len in [0, 8, 100, 1000, good.len() / 2, good.len() - 10] {
        assert!(import(&good[..len]).is_err(), "cut at {len}");
    }
}

/// Damaged files never panic: bytes overwritten all over every fixture
/// (`ODT_FUZZ=N` runs N damaged copies of each instead of 60).
#[test]
fn corrupt_bytes_never_panic() {
    let rounds: usize = std::env::var("ODT_FUZZ")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for name in [
        "text.odt",
        "lists.odt",
        "tables.odt",
        "pictures.odt",
        "text.fodt",
    ] {
        let good = std::fs::read(corpus(name)).unwrap();
        for _ in 0..rounds {
            let mut b = good.clone();
            for _ in 0..1 + next() % 8 {
                let at = (next() as usize) % b.len();
                b[at] = match next() % 3 {
                    0 => b'<',
                    1 => b'"',
                    _ => next() as u8,
                };
            }
            let _ = import(&b);
        }
        for k in 1..20 {
            let _ = import(&good[..good.len() * k / 20]);
        }
    }
}
