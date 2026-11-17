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
    assert_eq!(
        back.paragraphs(),
        doc.paragraphs(),
        "same model after a save"
    );
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
            assert_eq!(
                &text[..cut],
                &second[name][..cut],
                "paragraphs before the edit"
            );
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
    doc.update_fields(Selection::caret(Position::new(p, at)), &cx(), &mut lay_out);
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
    doc.update_tables_of_contents(Selection::caret(Position::new(p, 0)), &cx(), &mut lay_out);
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
    let doc = Document::open(&corpus("fields.docx")).unwrap();
    // `\o "1-1"`: level 1 only; `\n`: no page numbers.
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
    let i = doc.fields().iter().position(|f| f.kind() == "REF").unwrap();
    let mut doc4 = with_instr(&doc, i, " REF _Gone \\h ");
    doc4.update_fields(all(&doc4), &cx(), &mut lay_out);
    assert!(shown(&doc4, find(&doc4, "See ")).contains(NO_BOOKMARK));
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
    let from = format!(
        "<w:instrText xml:space=\"preserve\">{}</w:instrText>",
        escape(&old)
    );
    let to = format!(
        "<w:instrText xml:space=\"preserve\">{}</w:instrText>",
        escape(instr)
    );
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
    assert!(
        xml.contains(r#"<w:instrText xml:space="preserve"> TITLE \* MERGEFORMAT </w:instrText>"#)
    );
    let back = Document::open(&path).unwrap();
    assert!(back.unsupported_content().is_empty());
    let title = back
        .fields()
        .into_iter()
        .find(|f| f.kind() == "TITLE")
        .unwrap();
    assert_eq!(title.end.para, title.begin.para + 1);
}

// ── field codes, Insert Field, bookmarks, cross-references ─────────────────

#[test]
fn field_codes_view_and_editing_a_code() {
    let mut doc = Document::open(&corpus("fields.docx")).unwrap();
    // The view setting: per field ids and Alt+F9 for all.
    let first = doc.fields()[0].clone();
    let mut view = docs_engine::CodeView::default();
    assert!(!view.shows(first.mark_id()));
    view.all = true;
    assert!(view.shows(first.mark_id()));
    view.ids.insert(first.mark_id());
    assert!(!view.shows(first.mark_id()), "toggled back on its own");
    doc.set_field_codes(Some(view));
    assert!(doc.field_codes().is_some());
    doc.set_field_codes(None);
    assert!(doc.field_codes().is_none());

    // Edit a DATE field's code: its result follows, one undo step.
    let date = doc
        .fields()
        .into_iter()
        .find(|f| f.kind() == "DATE")
        .expect("a DATE field");
    let para = date.begin.para;
    let before = shown(&doc, para);
    let sel = Selection::caret(date.begin);
    assert!(doc.set_field_code(
        date.begin,
        "DATE \\@ \"yyyy-MM-dd\"",
        sel,
        &cx(),
        &mut lay_out
    ));
    assert!(
        shown(&doc, para).contains("2026-11-15"),
        "{}",
        shown(&doc, para)
    );
    let now = doc.field_at(date.begin).unwrap();
    assert_eq!(now.instr.trim(), "DATE \\@ \"yyyy-MM-dd\"");
    doc.undo();
    assert_eq!(shown(&doc, para), before);
    doc.redo();
    // Saved as a complex field with the new code.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f.docx");
    doc.save_as(&out).unwrap();
    let body = &parts(&std::fs::read(&out).unwrap())["word/document.xml"];
    assert!(
        body.contains("DATE \\@ &quot;yyyy-MM-dd&quot;")
            || body.contains("DATE \\@ \"yyyy-MM-dd\""),
        "{body}"
    );
    let back = Document::open(&out).unwrap();
    assert!(shown(&back, para).contains("2026-11-15"));
}

#[test]
fn insert_field_bookmark_and_cross_reference() {
    let mut doc = Document::new();
    let c = doc.insert_text(Selection::default(), "Introduction\nSome text.\nSee ", None);
    doc.set_style(Selection::caret(Position::new(0, 0)), Some("Heading1"));
    // Insert ▸ Field: DATE with a picture, updated at once (one step).
    let c = doc.insert_field(c, "DATE \\@ \"d MMMM yyyy\"", &cx(), &mut lay_out);
    assert_eq!(shown(&doc, 2), "See 15 November 2026");
    assert_eq!(doc.fields().len(), 1);
    doc.undo();
    assert_eq!(shown(&doc, 2), "See ");
    doc.redo();
    // A page number field becomes the object headers use.
    doc.insert_field(c, "PAGE \\* roman", &cx(), &mut lay_out);
    let p = doc.paragraph(2);
    assert!(
        p.objects()
            .any(|(_, o)| docs_engine::page_field(o).is_some())
    );

    // Insert ▸ Bookmark on "Some".
    let sel = Selection {
        anchor: Position::new(1, 0),
        head: Position::new(1, 4),
    };
    doc.add_bookmark(sel, "Some_text").unwrap();
    assert_eq!(doc.bookmark_names(false), ["Some_text"]);
    assert!(doc.add_bookmark(sel, "9bad name").is_err());
    // Moving it: the same name again elsewhere.
    let whole = Selection {
        anchor: Position::new(1, 0),
        head: Position::new(1, doc.paragraph(1).text.len()),
    };
    doc.add_bookmark(whole, "Some_text").unwrap();
    assert_eq!(doc.bookmark_names(false).len(), 1);
    assert_eq!(
        doc.bookmark_position("Some_text"),
        Some(Position::new(1, 0))
    );

    // Cross-reference to the heading (text) and to the bookmark (page).
    let targets = doc.xref_targets(&docs_engine::XrefKind::Heading);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].label, "Introduction");
    let end = doc.end();
    let c = doc.insert_text(Selection::caret(end), " and ", None);
    let c = doc.insert_cross_reference(
        c,
        &docs_engine::XrefKind::Heading,
        &targets[0],
        docs_engine::XrefWhat::Text,
        true,
        &cx(),
        &mut lay_out,
    );
    assert!(
        shown(&doc, 2).ends_with(" and Introduction"),
        "{}",
        shown(&doc, 2)
    );
    // Word's hidden `_Ref` bookmark on the heading.
    let hidden = doc.bookmark_names(true);
    assert!(hidden.iter().any(|n| n.starts_with("_Ref")), "{hidden:?}");
    let f = doc
        .fields()
        .into_iter()
        .find(|f| f.kind() == "REF")
        .unwrap();
    assert!(f.instr.contains("\\h"));
    // One undo step takes the field and the bookmark away.
    doc.undo();
    assert!(
        !doc.bookmark_names(true)
            .iter()
            .any(|n| n.starts_with("_Ref"))
    );
    doc.redo();
    let bm = doc.xref_targets(&docs_engine::XrefKind::Bookmark);
    assert_eq!(bm[0].label, "Some_text");
    doc.insert_cross_reference(
        c,
        &docs_engine::XrefKind::Bookmark,
        &bm[0],
        docs_engine::XrefWhat::PageNumber,
        false,
        &cx(),
        &mut lay_out,
    );
    assert!(
        doc.fields()
            .iter()
            .any(|f| f.instr.trim() == "PAGEREF Some_text")
    );

    // Saved and read back: the fields and bookmarks are there.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("x.docx");
    doc.save_as(&out).unwrap();
    let back = Document::open(&out).unwrap();
    assert_eq!(back.bookmark_names(false), ["Some_text"]);
    assert!(back.fields().iter().any(|f| f.kind() == "REF"));
    assert!(shown(&back, 2).contains("Introduction"));
    // Deleting the bookmark.
    let mut back = back;
    assert!(back.delete_bookmark("Some_text", Selection::default()));
    assert!(back.bookmark_names(false).is_empty());
}

#[test]
fn f9_updates_fields_in_headers_and_notes() {
    let mut doc = Document::new();
    let c = doc.insert_text(Selection::default(), "Body", None);
    let (c, id) = doc.insert_note(docs_engine::NoteKind::Footnote, c);
    // A DATE field in the footnote and in the header, inserted while
    // editing them (as the view does).
    let (e, mut note) = doc.edit_note(docs_engine::NoteKind::Footnote, &id).unwrap();
    let end = note.end();
    note.insert_field(
        Selection::caret(end),
        "DATE \\@ \"yyyy\"",
        &cx(),
        &mut lay_out,
    );
    assert!(note.paragraph(0).text.replace(OBJECT, "").ends_with("2026"));
    doc.finish_part(e, note, c);
    let (e, mut h) = doc
        .edit_hf(0, docs_engine::HfKind::Header, docs_engine::HfType::Default)
        .unwrap();
    h.insert_field(
        Selection::default(),
        "DATE \\@ \"yyyy\"",
        &cx(),
        &mut lay_out,
    );
    doc.finish_part(e, h, c);
    // A year later, F9 over the whole document updates them too.
    let later = FieldContext {
        now: NaiveDate::from_ymd_opt(2027, 1, 2)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap(),
        offset: cx().offset,
    };
    assert!(doc.update_part_fields(all(&doc), &later));
    assert!(
        doc.note_text(docs_engine::NoteKind::Footnote, &id)
            .ends_with("2027")
    );
    assert_eq!(
        doc.hf_text(0, docs_engine::HfKind::Header, docs_engine::HfType::Default)
            .0,
        "2027"
    );
    // One undo step.
    doc.undo();
    assert!(
        doc.note_text(docs_engine::NoteKind::Footnote, &id)
            .ends_with("2026")
    );
}

/// The document's text as shown, paragraph by paragraph.
fn texts(doc: &Document) -> Vec<String> {
    (0..doc.len()).map(|i| shown(doc, i)).collect()
}

#[test]
fn tracked_updates_are_deletions_and_insertions() {
    let path = corpus("fields.docx");
    let original = Document::open(&path).unwrap();
    // What the update gives without tracking.
    let mut plain = original.clone();
    plain.update_fields(all(&plain), &cx(), &mut lay_out);
    let mut doc = original.clone();
    doc.set_author("Ani Wijaya");
    doc.set_track_changes(true, Selection::default());
    doc.update_fields(all(&doc), &cx(), &mut lay_out);
    let ch = doc.changes();
    assert!(
        ch.iter().any(|c| c.kind == docs_engine::RevKind::Insert),
        "{ch:?}"
    );
    assert!(ch.iter().all(|c| c.author == "Ani Wijaya"), "{ch:?}");
    // The fields are still fields (struck marks don't count).
    let kinds = |d: &Document| d.fields().iter().map(|f| f.kind()).collect::<Vec<_>>();
    assert_eq!(kinds(&doc), kinds(&plain));
    // Accept All gives the plain update; Reject All the original.
    let mut accepted = doc.clone();
    accepted.resolve_all(true, Selection::default());
    assert_eq!(texts(&accepted), texts(&plain));
    let mut rejected = doc.clone();
    rejected.resolve_all(false, Selection::default());
    assert_eq!(texts(&rejected), texts(&original));
    // A rejected new table of contents line takes its fields with it (no
    // empty PAGEREF left behind).
    assert_eq!(kinds(&rejected), kinds(&original));
    // Saved and read back: the same changes, the same fields.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("t.docx");
    doc.save_as(&out).unwrap();
    let back = Document::open(&out).unwrap();
    assert!(
        back.unsupported_content().is_empty(),
        "{:?}",
        back.unsupported_content()
    );
    assert_eq!(back.changes().len(), doc.changes().len());
    assert_eq!(kinds(&back), kinds(&plain));
    let mut back_acc = back.clone();
    back_acc.resolve_all(true, Selection::default());
    assert_eq!(texts(&back_acc), texts(&plain));
}

#[test]
fn tracked_table_of_contents_update() {
    let path = corpus("fields.docx");
    let original = Document::open(&path).unwrap();
    let mut plain = original.clone();
    plain.update_tables_of_contents(Selection::caret(Position::new(1, 0)), &cx(), &mut lay_out);
    let mut doc = original.clone();
    doc.set_track_changes(true, Selection::default());
    doc.update_tables_of_contents(Selection::caret(Position::new(1, 0)), &cx(), &mut lay_out);
    assert!(doc.has_changes());
    let mut accepted = doc.clone();
    accepted.resolve_all(true, Selection::default());
    assert_eq!(texts(&accepted), texts(&plain));
    let mut rejected = doc.clone();
    rejected.resolve_all(false, Selection::default());
    assert_eq!(texts(&rejected), texts(&original));
    // Its fields too: no empty PAGEREF from a rejected new line.
    let kinds = |d: &Document| d.fields().iter().map(|f| f.kind()).collect::<Vec<_>>();
    assert_eq!(kinds(&rejected), kinds(&original));
    assert_eq!(kinds(&accepted), kinds(&plain));
    doc.undo();
    assert!(!doc.has_changes() || doc.changes() == original.changes());
}

/// A field's code shows the codes of the fields nested in it (in braces,
/// as Word's field codes view); typing braces in the code box nests
/// fields, saved and read back as fields.
#[test]
fn nested_field_codes_are_shown_and_typed() {
    let mut doc = Document::open(&corpus("fields.docx")).unwrap();
    let date = doc
        .fields()
        .into_iter()
        .find(|f| f.kind() == "DATE")
        .unwrap();
    let code = "IF { DATE \\@ \"yyyy\" } = \"2026\" \"this year\" \"later\"";
    let sel = Selection::caret(date.begin);
    assert!(doc.set_field_code(date.begin, code, sel, &cx(), &mut lay_out));
    let f = doc.field_at(date.begin).unwrap();
    assert_eq!(f.kind(), "IF");
    assert_eq!(f.shown_code(), code);
    // The same code again changes nothing.
    assert!(doc.set_field_code(date.begin, code, sel, &cx(), &mut lay_out));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("n.docx");
    doc.save_as(&out).unwrap();
    let body = &parts(&std::fs::read(&out).unwrap())["word/document.xml"];
    // Two field starts for the IF and the nested DATE.
    let at = body
        .find("> IF </w:instrText>")
        .or_else(|| body.find(">IF "))
        .unwrap_or(0);
    assert!(
        body[at.saturating_sub(400)..]
            .matches("fldCharType=\"begin\"")
            .count()
            >= 2
    );
    let back = Document::open(&out).unwrap();
    assert!(
        back.unsupported_content().is_empty(),
        "{:?}",
        back.unsupported_content()
    );
    let f = back.field_at(date.begin).unwrap();
    assert_eq!(f.shown_code(), code);
    assert_eq!(f.kind(), "IF");
}

#[test]
fn tracked_field_code_edits_are_deleted_and_inserted_code() {
    let original = Document::open(&corpus("fields.docx")).unwrap();
    let mut doc = original.clone();
    doc.set_author("Ani Wijaya");
    doc.set_track_changes(true, Selection::default());
    let date = doc
        .fields()
        .into_iter()
        .find(|f| f.kind() == "DATE")
        .expect("a DATE field");
    let old_code = date.shown_code();
    let sel = Selection::caret(date.begin);
    assert!(doc.set_field_instr(date.begin, "DATE \\@ \"yyyy\"", sel));
    let now = doc.field_at(date.begin).unwrap();
    assert_eq!(now.instr.trim(), "DATE \\@ \"yyyy\"");
    assert_eq!(now.shown_code(), "DATE \\@ \"yyyy\"");
    let ch = doc.changes();
    let code_changes: Vec<_> = ch.iter().filter(|c| c.start == date.begin).collect();
    assert!(
        code_changes
            .iter()
            .any(|c| c.kind == docs_engine::RevKind::Delete && c.text == old_code),
        "{ch:?}"
    );
    assert!(
        code_changes
            .iter()
            .any(|c| c.kind == docs_engine::RevKind::Insert
                && c.text == "DATE \\@ \"yyyy\""
                && c.author == "Ani Wijaya"),
        "{ch:?}"
    );
    // Saved as Word writes it: the old code's runs deleted.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("c.docx");
    doc.save_as(&out).unwrap();
    let body = &parts(&std::fs::read(&out).unwrap())["word/document.xml"];
    assert!(body.contains("<w:delInstrText"), "{body}");
    let back = Document::open(&out).unwrap();
    assert!(
        back.unsupported_content().is_empty(),
        "{:?}",
        back.unsupported_content()
    );
    let f = back
        .fields()
        .into_iter()
        .find(|f| f.begin == date.begin)
        .unwrap();
    assert_eq!(f.shown_code(), "DATE \\@ \"yyyy\"");
    assert_eq!(back.changes().len(), doc.changes().len());
    // Rejected (the change at the field): the old code back; accepted: the
    // new one, untracked.
    let mut rej = back.clone();
    rej.reject_change(Selection::caret(date.begin)).unwrap();
    let f = rej
        .fields()
        .into_iter()
        .find(|f| f.begin == date.begin)
        .unwrap();
    assert_eq!(f.shown_code(), old_code);
    assert!(rej.changes().iter().all(|c| c.start != date.begin));
    let mut acc = back.clone();
    acc.resolve_all(true, Selection::default());
    let f = acc
        .fields()
        .into_iter()
        .find(|f| f.begin == date.begin)
        .unwrap();
    assert_eq!(f.shown_code(), "DATE \\@ \"yyyy\"");
    assert!(acc.changes().is_empty());
    let out2 = dir.path().join("a.docx");
    acc.save_as(&out2).unwrap();
    let body = &parts(&std::fs::read(&out2).unwrap())["word/document.xml"];
    assert!(!body.contains("<w:delInstrText"), "{body}");
}

/// formulas.docx (scripts/corpus/gen_docx_more.py): fields Word computes,
/// saved with wrong results on purpose, worked out by F9 on everything.
#[test]
fn formulas_conditions_and_more_fields_are_worked_out() {
    let mut doc = Document::open(&corpus("formulas.docx")).unwrap();
    let sel = all(&doc);
    doc.update_fields(sel, &cx(), &mut lay_out);
    let line = |doc: &Document, start: &str| shown(doc, find(doc, start));
    assert_eq!(
        line(&doc, "Price "),
        "Price 250, count 4. Total: 1,000. Half: 125. Rounded: 3.33. Check: 1. Max: 8. 14"
    );
    assert_eq!(line(&doc, "If: "), "If: many / different / compare: 1 / 1");
    assert_eq!(line(&doc, "Rate is"), "Rate is 0.11 / tax 11");
    // STYLEREF: the nearest heading above; below when there's none above.
    assert_eq!(line(&doc, "Style ref: "), "Style ref: Part one / Part one");
    assert_eq!(
        line(&doc, "Style ref here"),
        "Style ref here: Outside the bookmark"
    );
    // Table formulas: cells by name, a column of formulas totalled.
    assert_eq!(shown(&doc, find(&doc, "Pens") + 3), "18.00");
    assert_eq!(shown(&doc, find(&doc, "Paper") + 3), "12.75");
    assert_eq!(shown(&doc, find(&doc, "Ink") + 3), "20");
    let total = (0..doc.len()).find(|&i| shown(&doc, i) == "Total").unwrap();
    assert_eq!(shown(&doc, total + 1), "17", "SUM(ABOVE) of the Qty column");
    assert_eq!(shown(&doc, total + 2), "17", "AVERAGE(LEFT): 17");
    assert_eq!(shown(&doc, total + 3), "50.75");
    // SEQ switches: \r restarts, \c repeats, \h hides.
    assert_eq!(
        line(&doc, "Seq "),
        "Seq 1, 2, restart 10, same 10, hidden, next 12"
    );
    // LISTNUM: one level below the paragraph's, `\l` and `\s` honoured.
    assert_eq!(line(&doc, "First "), "First a) and b)");
    assert_eq!(line(&doc, "Second "), "Second 5)");
    // INDEX: XE entries with their pages, alphabetically.
    assert_eq!(
        line(&doc, "Index: "),
        format!("Index: Apples, 1{}Bananas, 1", docs_engine::LINE_BREAK)
    );
    // TOC \b: only the headings inside the bookmark; \f: TC entries.
    let contents = find(&doc, "Contents of part two");
    assert_eq!(shown(&doc, contents + 1), "Inside the bookmark\t1");
    assert_eq!(shown(&doc, contents + 2), "Figures");
    assert_eq!(shown(&doc, contents + 3), "Figure entry A\t1");
    assert_eq!(shown(&doc, contents + 4), "Figure entry B\t1");
    assert_eq!(
        doc.paragraph(contents + 4).props.style.as_deref(),
        Some("TOC2")
    );
    // Saved and read back: the new results stay.
    let saved = doc.to_docx_bytes().unwrap();
    let xml = &parts(&saved)["word/document.xml"];
    assert!(xml.contains(">1,000<") && xml.contains(">50.75<"), "{xml}");
    // Fields nested in the code are worked out first.
    let p = find(&doc, "Rate is");
    let end = Selection::caret(Position::new(p, doc.paragraph(p).text.len()));
    doc.insert_field(
        end,
        "IF { REF Count } > 3 \"many\" \"few\"",
        &cx(),
        &mut lay_out,
    );
    doc.insert_field(
        Selection::caret(Position::new(p, doc.paragraph(p).text.len())),
        "= { Price } / { REF Count } \\# \"0.0\"",
        &cx(),
        &mut lay_out,
    );
    assert_eq!(shown(&doc, p), "Rate is 0.11 / tax 11many62.5");
}
