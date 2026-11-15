//! Word fields in `fields.docx` (scripts/corpus/gen_docx.py): their results
//! shown as text, updated like Word's F9 and Update Table, and saved back
//! in place.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _};
use std::path::PathBuf;

use chrono::{FixedOffset, NaiveDate};
use docs_engine::fields::{NO_BOOKMARK, NO_TOC_ENTRIES};
use docs_engine::{
    Document, FieldContext, LinkTarget, OBJECT, PAGE_BREAK, PageLookup, Position, Selection,
};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/docx")
        .join(name)
}

fn parts(bytes: &[u8]) -> BTreeMap<String, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        out.insert(name, String::from_utf8_lossy(&v).into_owned());
    }
    out
}

/// A paragraph's text as shown (field marks and bookmarks left out).
fn shown(doc: &Document, i: usize) -> String {
    doc.paragraph(i).text.replace(OBJECT, "")
}

fn find(doc: &Document, text: &str) -> usize {
    (0..doc.len())
        .find(|&i| shown(doc, i).contains(text))
        .unwrap_or_else(|| panic!("no paragraph with {text:?}"))
}

fn cx() -> FieldContext {
    FieldContext {
        now: NaiveDate::from_ymd_opt(2026, 11, 15)
            .unwrap()
            .and_hms_opt(9, 30, 0)
            .unwrap(),
        offset: FixedOffset::east_opt(7 * 3600).unwrap(),
    }
}

/// Pages from page breaks: what a layout would say for this file, without
/// fonts.
struct Breaks(Vec<usize>);

impl Breaks {
    fn of(doc: &Document) -> Self {
        let mut page = 0;
        Self(
            doc.paragraphs()
                .iter()
                .map(|p| {
                    let here = page;
                    page += p.text.matches(PAGE_BREAK).count();
                    here
                })
                .collect(),
        )
    }
}

impl PageLookup for Breaks {
    fn page_of(&self, pos: Position) -> Option<usize> {
        self.0.get(pos.para).copied()
    }
    fn page_number(&self, page: usize) -> u32 {
        page as u32 + 1
    }
    fn page_label(&self, page: usize) -> String {
        (page + 1).to_string()
    }
}

fn lay_out(doc: &Document) -> Box<dyn PageLookup> {
    Box::new(Breaks::of(doc))
}

fn all(doc: &Document) -> Selection {
    Selection {
        anchor: Position::new(0, 0),
        head: doc.end(),
    }
}

#[test]
fn fields_show_their_results_and_save_in_place() {
    let path = corpus("fields.docx");
    let doc = Document::open(&path).unwrap();
    assert!(
        doc.unsupported_content().is_empty(),
        "{:?}",
        doc.unsupported_content()
    );
    assert!(!doc.needs_save_as(), "a table of contents saves in place");
    let kinds: Vec<String> = doc.fields().iter().map(|f| f.kind()).collect();
    assert_eq!(
        kinds,
        [
            "TOC",
            "PAGEREF",
            "PAGEREF",
            "DATE",
            "TITLE",
            "MERGEFIELD",
            "SEQ",
            "SEQ",
            "REF",
            "PAGEREF",
            "HYPERLINK"
        ]
    );
    // The table of contents is ordinary lines inside its content control.
    assert_eq!(shown(&doc, 0), "Contents");
    assert_eq!(shown(&doc, 1), "Introduction\t4");
    assert_eq!(shown(&doc, 2), "Scope\t9");
    assert_eq!(doc.paragraph(1).props.style.as_deref(), Some("TOC1"));
    let p = find(&doc, "Prepared on");
    assert_eq!(
        shown(&doc, p),
        "Prepared on 1 January 2020 for Old title. Dear ,"
    );
    assert_eq!(
        shown(&doc, find(&doc, "See ")),
        "See Table 7 on page 3, the Kantor site and the online copy."
    );

    // Saving writes every field back: instructions, results, the control.
    let dir = tempfile::tempdir().unwrap();
    let copy = dir.path().join("fields.docx");
    std::fs::copy(&path, &copy).unwrap();
    let mut doc = Document::open(&copy).unwrap();
    let end = doc.end();
    let c = doc.insert_text(Selection::caret(end), "!", None);
    doc.backspace(c);
    doc.save().unwrap();
    let first = std::fs::read(&copy).unwrap();
    let xml = &parts(&first)["word/document.xml"];
    for want in [
        r#"<w:docPartGallery w:val="Table of Contents"/>"#,
        r#"<w:instrText xml:space="preserve"> TOC \o "1-3" \h \z \u </w:instrText>"#,
        r#"<w:hyperlink w:anchor="_Toc100000001" w:history="1">"#,
        r#"<w:instrText xml:space="preserve"> PAGEREF _Toc100000002 \h </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t xml:space="preserve">9</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:hyperlink>"#,
        r#"<w:p><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p></w:sdtContent></w:sdt>"#,
        r#"<w:fldSimple w:instr=" TITLE \* MERGEFORMAT "><w:r><w:t xml:space="preserve">Old title</w:t></w:r></w:fldSimple>"#,
        r#"<w:instrText xml:space="preserve"> MERGEFIELD FirstName </w:instrText></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>"#,
        r#"<w:bookmarkStart w:id="3" w:name="_Ref1"/>"#,
    ] {
        assert!(xml.contains(want), "{want} missing in\n{xml}");
    }
    let back = Document::open(&copy).unwrap();
    assert_eq!(back.paragraphs(), doc.paragraphs(), "same model after a save");
    assert!(back.unsupported_content().is_empty());

    // An edit elsewhere leaves the other paragraphs and parts as they were.
    let mut back = back;
    let end = back.end();
    back.insert_text(Selection::caret(end), " Done", None);
    back.save().unwrap();
    let second = parts(&std::fs::read(&copy).unwrap());
    let first = parts(&first);
    for (name, text) in &first {
        if name == "docProps/core.xml" {
            continue;
        }
        if name == "word/document.xml" {
            let cut = text.rfind("<w:p>").unwrap();
            assert_eq!(&text[..cut], &second[name][..cut], "paragraphs before the edit");
            continue;
        }
        assert_eq!(Some(text), second.get(name), "{name} changed");
    }
}

#[test]
fn update_all_fields_like_word() {
    let mut doc = Document::open(&corpus("fields.docx")).unwrap();
    let before = doc.paragraphs().to_vec();
    doc.update_fields(all(&doc), &cx(), &mut lay_out);

    // The table of contents lists all three headings, on their pages,
    // in the TOC styles; the content control's edges stay put.
    assert_eq!(shown(&doc, 0), "Contents");
    assert_eq!(shown(&doc, 1), "Introduction\t1");
    assert_eq!(shown(&doc, 2), "Scope\t1");
    assert_eq!(shown(&doc, 3), "Results\t2");
    assert_eq!(shown(&doc, 4), "");
    let styles: Vec<_> = (1..4)
        .map(|i| doc.paragraph(i).props.style.clone().unwrap_or_default())
        .collect();
    assert_eq!(styles, ["TOC1", "TOC2", "TOC1"]);
    // Each line links to a `_Toc` bookmark; the new heading got one.
    assert_eq!(
        doc.link_at(Position::new(3, 4)),
        Some(LinkTarget::Bookmark("_Toc100000003".into()))
    );
    let results = find(&doc, "Results");
    let results = (results + 1..doc.len())
        .find(|&i| shown(&doc, i) == "Results")
        .unwrap();
    assert_eq!(
        doc.bookmark_position("_Toc100000003"),
        Some(Position::new(results, 0))
    );

    let p = find(&doc, "Prepared on");
    assert_eq!(
        shown(&doc, p),
        "Prepared on 15 November 2026 for Laporan Kuartal. Dear «FirstName»,"
    );
    assert_eq!(
        shown(&doc, find(&doc, "Sales by city")),
        "Table 1: Sales by city"
    );
    assert_eq!(shown(&doc, find(&doc, ": Costs")), "Table 2: Costs");
    assert_eq!(
        shown(&doc, find(&doc, "See ")),
        "See Table 1 on page 2, the Kantor site and the online copy."
    );

    // One undo step brings every old result back.
    doc.undo();
    assert_eq!(doc.paragraphs(), before);
    doc.redo();
    assert_eq!(shown(&doc, 3), "Results\t2");

    // Updating again changes nothing.
    let now = doc.paragraphs().to_vec();
    doc.update_fields(all(&doc), &cx(), &mut lay_out);
    for (i, (a, b)) in doc.paragraphs().iter().zip(&now).enumerate() {
        assert_eq!(a, b, "paragraph {i} changed on a second update");
    }
    assert_eq!(doc.paragraphs().len(), now.len());

    // Saved in place and read back: the same fields, the new bookmark.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("updated.docx");
    doc.save_as(&path).unwrap();
    let xml = parts(&std::fs::read(&path).unwrap())["word/document.xml"].clone();
    for want in [
        r#"<w:bookmarkStart w:id="4" w:name="_Toc100000003"/>"#,
        r#"<w:hyperlink w:anchor="_Toc100000003" w:history="1">"#,
        r#"<w:instrText xml:space="preserve"> PAGEREF _Toc100000003 \h </w:instrText>"#,
        r#"<w:t xml:space="preserve">«FirstName»</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/>"#,
    ] {
        assert!(xml.contains(want), "{want} missing in\n{xml}");
    }
    let back = Document::open(&path).unwrap();
    assert!(back.unsupported_content().is_empty());
    for (i, (a, b)) in back.paragraphs().iter().zip(doc.paragraphs()).enumerate() {
        assert_eq!(a, b, "paragraph {i} read back differently");
    }
    assert_eq!(back.paragraphs().len(), doc.paragraphs().len());
    assert_eq!(back.fields().len(), doc.fields().len());
}

#[test]
fn f9_updates_the_field_at_the_caret_and_update_table_the_contents() {
    let mut doc = Document::open(&corpus("fields.docx")).unwrap();
    // Caret in the DATE result: only that field changes.
    let p = find(&doc, "Prepared on");
    let at = doc.paragraph(p).text.find("January").unwrap();
    doc.update_fields(
        Selection::caret(Position::new(p, at)),
        &cx(),
        &mut lay_out,
    );
    assert_eq!(
        shown(&doc, p),
        "Prepared on 15 November 2026 for Old title. Dear ,"
    );
    assert_eq!(shown(&doc, 1), "Introduction\t4", "TOC untouched");
    // Caret outside any field: nothing happens.
    let rev = doc.revision();
    doc.update_fields(Selection::caret(Position::new(0, 0)), &cx(), &mut lay_out);
    assert_eq!(doc.revision(), rev);

    // Update Table from anywhere rebuilds the table of contents only.
    doc.update_tables_of_contents(
        Selection::caret(Position::new(p, 0)),
        &cx(),
        &mut lay_out,
    );
    assert_eq!(shown(&doc, 3), "Results\t2");
    assert_eq!(
        shown(&doc, find(&doc, "Sales by city")),
        "Table 5: Sales by city",
        "other fields keep their results"
    );
    assert!(doc.has_table_of_contents());
}

#[test]
fn toc_switches_levels_and_missing_bookmarks() {
    let mut doc = Document::open(&corpus("fields.docx")).unwrap();
    // `\o "1-1"`: level 1 only; `\n`: no page numbers.
    let span = doc.fields()[0].clone();
    let (b, _) = (span.begin, span.end);
    let text = doc.paragraph(b.para).text.clone();
    assert!(text.starts_with(OBJECT));
    let instr = r#" TOC \o "1-1" \n \h "#;
    let mut doc2 = with_instr(&doc, 0, instr);
    doc2.update_tables_of_contents(Selection::default(), &cx(), &mut lay_out);
    assert_eq!(shown(&doc2, 1), "Introduction");
    assert_eq!(shown(&doc2, 2), "Results");
    assert_eq!(shown(&doc2, 3), "");

    // No headings at all: Word's message.
    let mut doc3 = with_instr(&doc, 0, r#" TOC \o "7-9" "#);
    doc3.update_tables_of_contents(Selection::default(), &cx(), &mut lay_out);
    assert_eq!(shown(&doc3, 1), NO_TOC_ENTRIES);

    // A REF to a bookmark that's gone.
    let i = doc
        .fields()
        .iter()
        .position(|f| f.kind() == "REF")
        .unwrap();
    let mut doc4 = with_instr(&doc, i, " REF _Gone \\h ");
    doc4.update_fields(all(&doc4), &cx(), &mut lay_out);
    assert!(shown(&doc4, find(&doc4, "See ")).contains(NO_BOOKMARK));
    let _ = &mut doc;
}

/// `doc` with field `i`'s instruction replaced (rebuilt through a save).
fn with_instr(doc: &Document, i: usize, instr: &str) -> Document {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.docx");
    let mut d = doc.clone();
    d.save_as(&path).unwrap();
    let mut p = parts(&std::fs::read(&path).unwrap());
    let old = doc.fields()[i].instr.clone();
    let xml = p.get_mut("word/document.xml").unwrap();
    let from = format!("<w:instrText xml:space=\"preserve\">{}</w:instrText>", escape(&old));
    let to = format!("<w:instrText xml:space=\"preserve\">{}</w:instrText>", escape(instr));
    assert!(xml.contains(&from), "{from}");
    *xml = xml.replacen(&from, &to, 1);
    let mut out = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(Cursor::new(&mut out));
        for (name, text) in &p {
            zip.start_file(name.as_str(), zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut zip, text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    std::fs::write(&path, out).unwrap();
    Document::open(&path).unwrap()
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[test]
fn links_go_to_bookmarks_and_addresses() {
    let doc = Document::open(&corpus("fields.docx")).unwrap();
    assert_eq!(
        doc.link_at(Position::new(1, 6)),
        Some(LinkTarget::Bookmark("_Toc100000001".into()))
    );
    let intro = (2..doc.len())
        .find(|&i| shown(&doc, i) == "Introduction")
        .unwrap();
    assert_eq!(
        doc.bookmark_position("_Toc100000001"),
        Some(Position::new(intro, 0))
    );
    let see = find(&doc, "See ");
    let text = &doc.paragraph(see).text;
    let site = text.find("Kantor site").unwrap();
    assert_eq!(
        doc.link_at(Position::new(see, site + 2)),
        Some(LinkTarget::Url("https://kantor.paparanai.id".into()))
    );
    let copy = text.find("online copy").unwrap();
    assert_eq!(
        doc.link_at(Position::new(see, copy + 2)),
        Some(LinkTarget::Url("https://example.com/report".into()))
    );
    assert_eq!(doc.link_at(Position::new(see, 1)), None);
}

#[test]
fn broken_fields_are_saved_as_valid_xml() {
    let dir = tempfile::tempdir().unwrap();
    // Deleting a field's start leaves its result as plain text.
    let mut doc = Document::open(&corpus("fields.docx")).unwrap();
    let p = find(&doc, "Prepared on");
    let begin = doc.paragraph(p).text.find(OBJECT).unwrap();
    doc.delete_selection(Selection {
        anchor: Position::new(p, begin),
        head: Position::new(p, begin + OBJECT.len_utf8() + 1),
    });
    let path = dir.path().join("a.docx");
    doc.save_as(&path).unwrap();
    let xml = parts(&std::fs::read(&path).unwrap())["word/document.xml"].clone();
    assert!(!xml.contains("DATE"), "the field is gone");
    let back = Document::open(&path).unwrap();
    assert!(back.unsupported_content().is_empty());
    assert!(shown(&back, p).starts_with("Prepared on  January 2020 for "));

    // Enter inside a simple field's result: written as a complex field.
    let mut doc = Document::open(&corpus("fields.docx")).unwrap();
    let at = doc.paragraph(p).text.find("title").unwrap();
    doc.newline(Selection::caret(Position::new(p, at)), None);
    let path = dir.path().join("b.docx");
    doc.save_as(&path).unwrap();
    let xml = parts(&std::fs::read(&path).unwrap())["word/document.xml"].clone();
    assert!(!xml.contains("<w:fldSimple"), "{xml}");
    assert!(xml.contains(r#"<w:instrText xml:space="preserve"> TITLE \* MERGEFORMAT </w:instrText>"#));
    let back = Document::open(&path).unwrap();
    assert!(back.unsupported_content().is_empty());
    let title = back.fields().into_iter().find(|f| f.kind() == "TITLE").unwrap();
    assert_eq!(title.end.para, title.begin.para + 1);
}
