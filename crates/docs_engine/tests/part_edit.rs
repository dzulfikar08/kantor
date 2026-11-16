//! Headers, footers and notes edited in place with the full editor
//! (`Document::edit_hf` / `edit_note` / `finish_part`): formatting, tables
//! and pictures saved into the right part, new parts created, different
//! first and even pages, Link to Previous, header/footer distances.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _};
use std::path::PathBuf;

use docs_engine::{
    Document, HfKind, HfType, NoteKind, PageField, Position, Selection, Toggle, page_field,
};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/docx")
        .join(name)
}

fn parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        out.insert(name, v);
    }
    out
}

fn text(p: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(p[name].clone()).unwrap_or_else(|_| panic!("{name}"))
}

/// A 2×1 PNG.
fn tiny_png() -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend(2u32.to_be_bytes());
    png.extend(1u32.to_be_bytes());
    png.extend([8, 6, 0, 0, 0, 0, 0, 0, 0]);
    png
}

fn sel(a: (usize, usize), b: (usize, usize)) -> Selection {
    Selection {
        anchor: Position::new(a.0, a.1),
        head: Position::new(b.0, b.1),
    }
}

#[test]
fn header_edited_with_formatting_table_and_picture_saves_only_its_part() {
    let path = corpus("report.docx");
    let orig = parts(&std::fs::read(&path).unwrap());
    let mut doc = Document::open(&path).unwrap();
    let story = doc
        .story_index_for(&doc.sections(), 0, HfKind::Header, HfType::Default)
        .unwrap();
    let part_name = doc.stories()[story].part.clone();
    let (edit, mut h) = doc.edit_hf(0, HfKind::Header, HfType::Default).unwrap();
    assert!(!edit.created());
    let old_text = h.paragraphs()[0].text.clone();
    // Bold text at the start, a table and a picture after the text.
    let c = h.insert_text(Selection::default(), "Draft ", None);
    h.toggle(sel((0, 0), (0, 5)), Toggle::Bold);
    let end = h.end();
    let c2 = h.newline(Selection::caret(end), None);
    let c2 = h
        .insert_image(c2, tiny_png(), "logo.png")
        .expect("picture in the header");
    h.insert_table(c2, 1, 2).expect("a table in the header");
    assert!(doc.part_changed(&edit, &h));
    assert!(doc.finish_part(edit, h, c));
    assert!(doc.is_dirty());
    let st = &doc.stories()[story];
    assert!(st.doc.paragraphs()[0].text.starts_with("Draft "));
    // One undo step for the whole edit.
    doc.undo();
    assert_eq!(doc.stories()[story].doc.paragraphs()[0].text, old_text);
    doc.redo();

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.docx");
    doc.save_as(&out).unwrap();
    let saved = parts(&std::fs::read(&out).unwrap());
    // Only that header changed; the other headers and footers are as they were.
    for (name, bytes) in &orig {
        if (name.contains("header") || name.contains("footer")) && *name != part_name {
            assert_eq!(&saved[name], bytes, "{name} unchanged");
        }
    }
    let header = text(&saved, &part_name);
    assert!(header.contains("<w:b/>"), "{header}");
    assert!(header.contains("Draft"));
    assert!(header.contains("<w:tbl>"), "{header}");
    assert!(header.contains("r:embed=\"rIdKantorheader"), "{header}");
    let rels_name = {
        let (dir, file) = part_name.rsplit_once('/').unwrap();
        format!("{dir}/_rels/{file}.rels")
    };
    let rels = text(&saved, &rels_name);
    assert!(rels.contains("relationships/image"), "{rels}");
    let media: Vec<&String> = saved
        .keys()
        .filter(|k| k.starts_with("word/media/kantor_header"))
        .collect();
    assert_eq!(media.len(), 1, "{:?}", saved.keys());
    assert!(text(&saved, "[Content_Types].xml").contains("Extension=\"png\""));

    // Reopened: the header has the picture and the table.
    let back = Document::open(&out).unwrap();
    let h = &back.stories()[story].doc;
    assert!(h.paragraphs()[0].text.starts_with("Draft "));
    assert!(h.paragraphs().iter().any(|p| p.cell.is_some()));
    assert!(h.paragraphs().iter().flat_map(|p| p.objects()).any(
        |(_, o)| matches!(o, docs_engine::Object::Image(i) if h.image_bytes(&i.rel_id).is_some())
    ));
    // Saving again doesn't add the picture twice.
    let mut back = back;
    let (edit, mut h) = back.edit_hf(0, HfKind::Header, HfType::Default).unwrap();
    h.insert_text(Selection::default(), "x", None);
    back.finish_part(edit, h, Selection::default());
    let out2 = dir.path().join("out2.docx");
    back.save_as(&out2).unwrap();
    let again = parts(&std::fs::read(&out2).unwrap());
    assert_eq!(
        again
            .keys()
            .filter(|k| k.starts_with("word/media/kantor_header"))
            .count(),
        1
    );
}

#[test]
fn new_footer_for_a_later_section_creates_part_and_relationship() {
    let mut doc = Document::new();
    let c = doc.insert_text(Selection::default(), "One\nTwo", None);
    let c = doc.insert_section_break(c).unwrap();
    doc.insert_text(c, "Three", None);
    assert_eq!(doc.sections().len(), 2);
    let saved_before = doc.revision();

    // Opening an empty new footer and leaving changes nothing.
    let (edit, f) = doc.edit_hf(1, HfKind::Footer, HfType::Default).unwrap();
    assert!(edit.created());
    assert!(!doc.finish_part(edit, f, c));
    assert!(doc.stories().is_empty());
    assert_eq!(doc.revision(), saved_before);
    assert!(
        doc.story_index_for(&doc.sections(), 1, HfKind::Footer, HfType::Default)
            .is_none()
    );

    let (edit, mut f) = doc.edit_hf(1, HfKind::Footer, HfType::Default).unwrap();
    let c2 = f.insert_text(Selection::default(), "Page ", None);
    f.insert_page_field(c2, PageField::Page);
    assert!(doc.finish_part(edit, f, c));
    // Only the second section has it (the first doesn't link forward).
    let sections = doc.sections();
    assert!(
        doc.story_index_for(&sections, 0, HfKind::Footer, HfType::Default)
            .is_none()
    );
    let i = doc
        .story_index_for(&sections, 1, HfKind::Footer, HfType::Default)
        .unwrap();
    assert_eq!(
        docs_engine::Document::page_fields_in(doc.stories()[i].doc.paragraphs()),
        1
    );

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("f.docx");
    doc.save_as(&out).unwrap();
    let p = parts(&std::fs::read(&out).unwrap());
    let footer = p
        .keys()
        .find(|k| k.starts_with("word/footer"))
        .cloned()
        .expect("footer part");
    assert!(text(&p, &footer).contains("PAGE"));
    let rels = text(&p, "word/_rels/document.xml.rels");
    assert!(rels.contains("relationships/footer"));
    assert!(text(&p, "[Content_Types].xml").contains("footer+xml"));
    let body = text(&p, "word/document.xml");
    // The reference is in the body's (last) section only.
    let last = &body[body.rfind("<w:sectPr").unwrap()..];
    assert!(last.contains("w:footerReference"), "{body}");
    assert_eq!(body.matches("w:footerReference").count(), 1);
    // Undo removes it again.
    doc.undo();
    assert!(
        doc.story_index_for(&doc.sections(), 1, HfKind::Footer, HfType::Default)
            .is_none()
    );
}

#[test]
fn first_and_even_page_headers() {
    let mut doc = Document::new();
    doc.insert_text(Selection::default(), "Body", None);
    let s = Selection::default();
    doc.set_title_page(0, true, s);
    doc.set_even_odd_headers(true, s);
    for (ty, words) in [
        (HfType::First, "Cover"),
        (HfType::Default, "Odd"),
        (HfType::Even, "Even"),
    ] {
        let (edit, mut h) = doc.edit_hf(0, HfKind::Header, ty).unwrap();
        h.insert_text(Selection::default(), words, None);
        assert!(doc.finish_part(edit, h, s));
    }
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("h.docx");
    doc.save_as(&out).unwrap();
    let p = parts(&std::fs::read(&out).unwrap());
    let body = text(&p, "word/document.xml");
    for ty in ["first", "default", "even"] {
        assert!(
            body.contains(&format!("w:headerReference w:type=\"{ty}\"")),
            "{body}"
        );
    }
    assert!(body.contains("<w:titlePg/>"));
    assert!(
        text(&p, "word/settings.xml").contains("<w:evenAndOddHeaders/>")
            || !p.contains_key("word/settings.xml")
    );
    let back = Document::open(&out).unwrap();
    for (ty, words) in [
        (HfType::First, "Cover"),
        (HfType::Default, "Odd"),
        (HfType::Even, "Even"),
    ] {
        assert_eq!(back.hf_text(0, HfKind::Header, ty).0, words);
    }
}

#[test]
fn link_to_previous_copies_and_relinks() {
    let mut doc = Document::new();
    let c = doc.insert_text(Selection::default(), "One", None);
    let c = doc.insert_section_break(c).unwrap();
    doc.insert_text(c, "Two", None);
    let s = Selection::default();
    let (edit, mut h) = doc.edit_hf(0, HfKind::Header, HfType::Default).unwrap();
    h.insert_text(Selection::default(), "Shared", None);
    doc.finish_part(edit, h, s);
    assert!(
        !doc.hf_linked(0, HfKind::Header, HfType::Default),
        "first section never links"
    );
    assert!(doc.hf_linked(1, HfKind::Header, HfType::Default));
    assert_eq!(doc.hf_text(1, HfKind::Header, HfType::Default).0, "Shared");

    // Unlink: section 2 gets its own copy, then a change there stays there.
    doc.set_hf_linked(1, HfKind::Header, HfType::Default, false, s);
    assert!(!doc.hf_linked(1, HfKind::Header, HfType::Default));
    assert_eq!(doc.hf_text(1, HfKind::Header, HfType::Default).0, "Shared");
    let (edit, mut h) = doc.edit_hf(1, HfKind::Header, HfType::Default).unwrap();
    let end = h.end();
    h.insert_text(Selection::caret(end), " (part 2)", None);
    doc.finish_part(edit, h, s);
    assert_eq!(doc.hf_text(0, HfKind::Header, HfType::Default).0, "Shared");
    assert_eq!(
        doc.hf_text(1, HfKind::Header, HfType::Default).0,
        "Shared (part 2)"
    );
    // Link again: section 2 shows section 1's header.
    doc.set_hf_linked(1, HfKind::Header, HfType::Default, true, s);
    assert!(doc.hf_linked(1, HfKind::Header, HfType::Default));
    assert_eq!(doc.hf_text(1, HfKind::Header, HfType::Default).0, "Shared");
    // Undo the relink.
    doc.undo();
    assert_eq!(
        doc.hf_text(1, HfKind::Header, HfType::Default).0,
        "Shared (part 2)"
    );
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("l.docx");
    doc.save_as(&out).unwrap();
    let body = text(&parts(&std::fs::read(&out).unwrap()), "word/document.xml");
    assert_eq!(body.matches("w:headerReference").count(), 2, "{body}");
}

#[test]
fn header_and_footer_distances() {
    let path = corpus("report.docx");
    let mut doc = Document::open(&path).unwrap();
    let s = Selection::default();
    let last = doc.sections().len() - 1;
    let was = doc.sections()[last].props.header_dist;
    doc.set_hf_distances(0, 500, 600, s);
    doc.set_hf_distances(last, 400, 300, s);
    let sections = doc.sections();
    assert_eq!(
        (sections[0].props.header_dist, sections[0].props.footer_dist),
        (500, 600)
    );
    assert_eq!(
        (
            sections[last].props.header_dist,
            sections[last].props.footer_dist
        ),
        (400, 300)
    );
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("d.docx");
    doc.save_as(&out).unwrap();
    let body = text(&parts(&std::fs::read(&out).unwrap()), "word/document.xml");
    assert!(body.contains("w:header=\"500\" w:footer=\"600\""), "{body}");
    assert!(body.contains("w:header=\"400\" w:footer=\"300\""), "{body}");
    doc.undo();
    assert_eq!(doc.sections()[last].props.header_dist, was);
}

#[test]
fn add_page_numbers_keeps_the_footer() {
    let mut doc = Document::open(&corpus("letter.docx")).unwrap();
    let before = doc.hf_text(0, HfKind::Footer, HfType::Default).0;
    assert!(doc.add_page_numbers(0, "Page {PAGE} of {PAGES}", Selection::default()));
    let i = doc
        .story_index_for(&doc.sections(), 0, HfKind::Footer, HfType::Default)
        .unwrap();
    let paras = doc.stories()[i].doc.paragraphs();
    assert_eq!(paras[0].text, before);
    let fields: Vec<PageField> = paras
        .last()
        .unwrap()
        .objects()
        .filter_map(|(_, o)| page_field(o))
        .collect();
    assert_eq!(fields, [PageField::Page, PageField::NumPages]);
    // A second time: already there.
    assert!(!doc.add_page_numbers(0, "Page {PAGE} of {PAGES}", Selection::default()));
}

#[test]
fn note_edited_with_formatting_and_picture() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.docx");
    let mut doc = Document::new();
    let c = doc.insert_text(Selection::default(), "Claim", None);
    let (c, id) = doc.insert_note(NoteKind::Footnote, c);
    let (edit, mut n) = doc.edit_note(NoteKind::Footnote, &id).unwrap();
    // The caret goes after the number and its space.
    let end = n.end();
    let a = n.insert_text(Selection::caret(end), "Source: survey", None);
    n.toggle(sel((0, end.offset), (0, a.head.offset)), Toggle::Italic);
    let a = n.newline(a, None);
    n.insert_image(a, tiny_png(), "chart.png").unwrap();
    assert!(doc.finish_part(edit, n, c));
    doc.save_as(&path).unwrap();
    let p = parts(&std::fs::read(&path).unwrap());
    let f = text(&p, "word/footnotes.xml");
    assert!(f.contains("<w:i/>"), "{f}");
    assert!(f.contains("Source: survey"));
    assert!(f.contains("<w:footnoteRef/>"));
    assert!(f.contains("r:embed=\"rIdKantorfootnote1_Image1\""), "{f}");
    assert!(text(&p, "word/_rels/footnotes.xml.rels").contains("relationships/image"));
    assert!(p.contains_key("word/media/kantor_footnote1_image1.png"));
    let back = Document::open(&path).unwrap();
    assert!(
        back.note_text(NoteKind::Footnote, &id)
            .starts_with("Source: survey")
    );
    let note = back.note(NoteKind::Footnote, &id).unwrap();
    assert!(
        note.doc.paragraphs()[0]
            .runs
            .iter()
            .any(|r| r.props.italic == Some(true))
    );
    // Undo in the main document takes the whole note edit back.
    doc.undo();
    assert_eq!(doc.note_text(NoteKind::Footnote, &id), "");
}

#[test]
fn edits_in_headers_and_notes_are_tracked() {
    let mut doc = Document::open(&corpus("notes.docx")).unwrap();
    doc.set_author("Ani Wijaya");
    doc.set_track_changes(true, Selection::default());
    // A footnote: typing is an insertion by the user.
    let id = doc
        .notes()
        .iter()
        .find(|n| n.kind == NoteKind::Footnote && !n.separator)
        .unwrap()
        .id
        .clone();
    let (edit, mut n) = doc.edit_note(NoteKind::Footnote, &id).unwrap();
    assert!(n.track_changes());
    let end = n.end();
    n.insert_text(Selection::caret(end), " Checked.", None);
    let ch = n.changes();
    assert_eq!(ch.len(), 1);
    assert_eq!(ch[0].kind, docs_engine::RevKind::Insert);
    assert_eq!(ch[0].author, "Ani Wijaya");
    doc.finish_part(edit, n, Selection::default());
    assert_eq!(doc.part_changes(), 1);
    // A new header: typing tracked too.
    let (edit, mut h) = doc.edit_hf(0, HfKind::Header, HfType::Default).unwrap();
    h.insert_text(Selection::default(), "Draft", None);
    doc.finish_part(edit, h, Selection::default());
    assert_eq!(doc.part_changes(), 2);
    // Saved as w:ins in the parts and read back.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.docx");
    doc.save_as(&path).unwrap();
    let saved = parts(&std::fs::read(&path).unwrap());
    let notes = text(&saved, "word/footnotes.xml");
    assert!(notes.contains("w:author=\"Ani Wijaya\""), "{notes}");
    let back = Document::open(&path).unwrap();
    assert_eq!(back.part_changes(), 2);
    // Reject All covers them: one undo step.
    let mut rejected = back.clone();
    rejected.resolve_all(false, Selection::default());
    assert_eq!(rejected.part_changes(), 0);
    let note = |d: &Document| {
        d.notes()
            .iter()
            .find(|n| n.id == id && n.kind == NoteKind::Footnote)
            .unwrap()
            .doc
            .paragraphs()
            .iter()
            .map(|p| p.text.clone())
            .collect::<String>()
    };
    assert!(!note(&rejected).contains("Checked."));
    rejected.undo();
    assert_eq!(rejected.part_changes(), 2);
    // Accept All keeps the text, unmarked.
    let mut accepted = back;
    accepted.resolve_all(true, Selection::default());
    assert_eq!(accepted.part_changes(), 0);
    assert!(note(&accepted).contains("Checked."));
}

/// A list started in a header saves its definition in numbering.xml.
#[test]
fn list_started_in_a_header_is_saved_with_its_definition() {
    let mut doc = Document::new();
    doc.insert_text(Selection::default(), "Body", None);
    let (edit, mut part) = doc.edit_hf(0, HfKind::Header, HfType::Default).unwrap();
    let c = part.insert_text(Selection::default(), "Item", None);
    part.toggle_list(c, docs_engine::ListKind::Bullet);
    let num = part.paragraph(0).props.num.expect("a list item").id;
    assert!(doc.finish_part(edit, part, Selection::default()));
    let p = parts(&doc.to_docx_bytes().unwrap());
    let numbering = text(&p, "word/numbering.xml");
    assert!(
        numbering.contains(&format!("w:numId=\"{num}\"")),
        "{numbering}"
    );
}
