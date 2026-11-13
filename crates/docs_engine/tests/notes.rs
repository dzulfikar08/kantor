//! Footnotes and endnotes: read, numbered, edited, inserted, saved.
//! The fixtures are made here (synthetic `.docx` packages).

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _, Write as _};

use docs_engine::{Document, NoteKind, NoteRestart, Position, Selection};

const W_NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
const R_NS: &str =
    "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

fn footnote_ref(id: u32) -> String {
    format!(
        "<w:r><w:rPr><w:rStyle w:val=\"FootnoteReference\"/></w:rPr><w:footnoteReference w:id=\"{id}\"/></w:r>"
    )
}

fn note(kind: &str, id: u32, text: &str) -> String {
    let style = if kind == "footnote" {
        "FootnoteText"
    } else {
        "EndnoteText"
    };
    let reference = if kind == "footnote" {
        "FootnoteReference"
    } else {
        "EndnoteReference"
    };
    format!(
        "<w:{kind} w:id=\"{id}\"><w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr><w:r><w:rPr><w:rStyle w:val=\"{reference}\"/></w:rPr><w:{kind}Ref/></w:r><w:r><w:t xml:space=\"preserve\"> {text}</w:t></w:r></w:p></w:{kind}>"
    )
}

fn separators(kind: &str) -> String {
    format!(
        "<w:{kind} w:type=\"separator\" w:id=\"-1\"><w:p><w:r><w:separator/></w:r></w:p></w:{kind}><w:{kind} w:type=\"continuationSeparator\" w:id=\"0\"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:{kind}>"
    )
}

/// A package with two footnotes (one in each of two sections), an
/// endnote, and footnote numbering restarting each section.
fn package() -> Vec<u8> {
    let body = format!(
        "<w:p><w:r><w:t>First claim.</w:t></w:r>{}<w:r><w:t xml:space=\"preserve\"> More.</w:t></w:r></w:p>\
         <w:p><w:r><w:t>Second</w:t></w:r>{}<w:r><w:t>.</w:t></w:r><w:r><w:endnoteReference w:id=\"1\"/></w:r></w:p>\
         <w:p><w:pPr><w:sectPr><w:footnotePr><w:numFmt w:val=\"lowerLetter\"/></w:footnotePr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:pPr><w:r><w:t>End of one.</w:t></w:r></w:p>\
         <w:p><w:r><w:t>Next section</w:t></w:r>{}</w:p>",
        footnote_ref(1),
        footnote_ref(2),
        footnote_ref(3)
    );
    let footnotes = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:footnotes {W_NS} {R_NS}>{}{}{}{}</w:footnotes>",
        separators("footnote"),
        note("footnote", 1, "Source: survey."),
        note("footnote", 2, "See annex."),
        note("footnote", 3, "Restarted.")
    );
    let endnotes = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:endnotes {W_NS} {R_NS}>{}{}</w:endnotes>",
        separators("endnote"),
        note("endnote", 1, "An endnote.")
    );
    let settings = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:settings {W_NS}><w:zoom w:percent=\"100\"/><w:defaultTabStop w:val=\"720\"/><w:footnotePr><w:numRestart w:val=\"eachSect\"/><w:footnote w:id=\"-1\"/><w:footnote w:id=\"0\"/></w:footnotePr><w:endnotePr><w:endnote w:id=\"-1\"/><w:endnote w:id=\"0\"/></w:endnotePr><w:compat/></w:settings>"
    );
    let styles = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:styles {W_NS}><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style><w:style w:type=\"paragraph\" w:styleId=\"FootnoteText\"><w:name w:val=\"footnote text\"/><w:basedOn w:val=\"Normal\"/><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr><w:rPr><w:sz w:val=\"20\"/></w:rPr></w:style><w:style w:type=\"character\" w:styleId=\"FootnoteReference\"><w:name w:val=\"footnote reference\"/><w:rPr><w:vertAlign w:val=\"superscript\"/></w:rPr></w:style></w:styles>"
    );
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:document {W_NS} {R_NS}><w:body>{body}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr></w:body></w:document>"
    );
    let rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>";
    let doc_rels = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/><Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/settings\" Target=\"settings.xml\"/><Relationship Id=\"rId3\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/footnotes\" Target=\"footnotes.xml\"/><Relationship Id=\"rId4\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/endnotes\" Target=\"endnotes.xml\"/></Relationships>";
    let types = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/><Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/><Override PartName=\"/word/settings.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml\"/><Override PartName=\"/word/footnotes.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.footnotes+xml\"/><Override PartName=\"/word/endnotes.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.endnotes+xml\"/></Types>";
    let parts = [
        ("[Content_Types].xml", types.to_owned()),
        ("_rels/.rels", rels.to_owned()),
        ("word/_rels/document.xml.rels", doc_rels.to_owned()),
        ("word/document.xml", document),
        ("word/styles.xml", styles),
        ("word/settings.xml", settings),
        ("word/footnotes.xml", footnotes),
        ("word/endnotes.xml", endnotes),
    ];
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in parts {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn parts(bytes: &[u8]) -> BTreeMap<String, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut s = String::new();
        if f.read_to_string(&mut s).is_ok() {
            out.insert(name, s);
        }
    }
    out
}

fn open(dir: &std::path::Path) -> (Document, std::path::PathBuf) {
    let path = dir.join("notes.docx");
    std::fs::write(&path, package()).unwrap();
    (Document::open(&path).unwrap(), path)
}

#[test]
fn notes_are_read_and_numbered() {
    let dir = tempfile::tempdir().unwrap();
    let (doc, _) = open(dir.path());
    assert!(
        !doc.has_unsupported_content(),
        "{:?}",
        doc.unsupported_content()
    );
    assert!(
        !doc.preserved_content()
            .iter()
            .any(|p| p.contains("footnote")),
        "{:?}",
        doc.preserved_content()
    );
    let refs = doc.note_refs();
    let labels: Vec<(NoteKind, &str, &str)> = refs
        .iter()
        .map(|r| (r.kind, r.id.as_str(), r.label.as_str()))
        .collect();
    // Section 1 numbers its footnotes a, b (its own format); section 2
    // restarts at 1 (settings: each section). The endnote is i.
    assert_eq!(
        labels,
        [
            (NoteKind::Footnote, "1", "a"),
            (NoteKind::Footnote, "2", "b"),
            (NoteKind::Endnote, "1", "i"),
            (NoteKind::Footnote, "3", "1"),
        ]
    );
    assert_eq!(
        doc.note_props(NoteKind::Footnote, 1).restart,
        NoteRestart::EachSection
    );
    assert_eq!(doc.note_text(NoteKind::Footnote, "2"), "See annex.");
    assert_eq!(doc.note_text(NoteKind::Endnote, "1"), "An endnote.");
    assert_eq!(doc.notes().iter().filter(|n| n.separator).count(), 4);
    assert_eq!(
        doc.note_at(Position::new(0, "First claim.".len() + 3)),
        Some((NoteKind::Footnote, "1".to_owned()))
    );
}

#[test]
fn untouched_notes_save_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let (mut doc, path) = open(dir.path());
    let before = parts(&std::fs::read(&path).unwrap());
    doc.insert_text(Selection::default(), "Edited ", None);
    doc.save().unwrap();
    let after = parts(&std::fs::read(&path).unwrap());
    assert_eq!(after["word/footnotes.xml"], before["word/footnotes.xml"]);
    assert_eq!(after["word/endnotes.xml"], before["word/endnotes.xml"]);
    assert!(after["word/document.xml"].contains("<w:footnoteReference w:id=\"2\"/>"));
}

#[test]
fn edit_insert_and_undo_notes() {
    let dir = tempfile::tempdir().unwrap();
    let (mut doc, path) = open(dir.path());
    let sel = Selection::default();
    assert!(doc.set_note_text(NoteKind::Footnote, "2", "See annex B.\nSecond line.", sel));
    assert_eq!(
        doc.note_text(NoteKind::Footnote, "2"),
        "See annex B.\nSecond line."
    );
    // A new footnote after "Second": numbered among the others.
    let at = Selection::caret(Position::new(1, "Second".len()));
    let (caret, id) = doc.insert_note(NoteKind::Footnote, at);
    assert_eq!(id, "4");
    assert_eq!(caret.head.offset, at.head.offset + 3);
    doc.set_note_text(NoteKind::Footnote, &id, "Inserted in Kantor.", caret);
    let labels: Vec<String> = doc.note_refs().into_iter().map(|r| r.label).collect();
    assert_eq!(labels, ["a", "b", "c", "i", "1"]);
    doc.save().unwrap();
    let saved = parts(&std::fs::read(&path).unwrap());
    let f = &saved["word/footnotes.xml"];
    assert!(
        f.contains("<w:footnote w:type=\"separator\" w:id=\"-1\">"),
        "separators kept"
    );
    assert!(f.contains(" Source: survey."), "untouched note kept");
    assert!(f.contains("See annex B."), "{f}");
    assert!(f.contains("<w:footnote w:id=\"4\">"), "{f}");
    assert!(f.contains("<w:footnoteRef/>"));
    assert!(saved["word/document.xml"].contains("<w:footnoteReference w:id=\"4\"/>"));
    // Reopened: the same notes.
    let back = Document::open(&path).unwrap();
    assert_eq!(
        back.note_text(NoteKind::Footnote, "4"),
        "Inserted in Kantor."
    );
    assert_eq!(
        back.note_text(NoteKind::Footnote, "2"),
        "See annex B.\nSecond line."
    );
    // Undo takes the note text, the new note and its reference back.
    doc.undo();
    assert_eq!(doc.note_text(NoteKind::Footnote, "4"), "");
    doc.undo();
    assert!(doc.note(NoteKind::Footnote, "4").is_none());
    assert_eq!(doc.note_refs().len(), 4);
    doc.undo();
    assert_eq!(doc.note_text(NoteKind::Footnote, "2"), "See annex.");
    // Saving after the undo writes the notes as they are now.
    doc.save().unwrap();
    let again = Document::open(&path).unwrap();
    assert_eq!(again.note_text(NoteKind::Footnote, "2"), "See annex.");
    assert!(again.note(NoteKind::Footnote, "4").is_none());
}

#[test]
fn first_footnote_creates_the_part() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.docx");
    let mut doc = Document::new();
    let c = doc.insert_text(Selection::default(), "Hello world", None);
    let (c, id) = doc.insert_note(NoteKind::Footnote, c);
    doc.set_note_text(NoteKind::Footnote, &id, "A note.", c);
    let (_, eid) = doc.insert_note(NoteKind::Endnote, c);
    doc.set_note_text(NoteKind::Endnote, &eid, "At the end.", c);
    doc.save_as(&path).unwrap();
    let saved = parts(&std::fs::read(&path).unwrap());
    assert!(saved["word/footnotes.xml"].contains("w:type=\"separator\""));
    assert!(saved["word/_rels/document.xml.rels"].contains("relationships/footnotes"));
    assert!(saved["word/_rels/document.xml.rels"].contains("relationships/endnotes"));
    assert!(saved["[Content_Types].xml"].contains("footnotes+xml"));
    assert!(saved["word/document.xml"].contains("<w:vertAlign w:val=\"superscript\"/>"));
    let back = Document::open(&path).unwrap();
    assert_eq!(back.note_text(NoteKind::Footnote, "1"), "A note.");
    assert_eq!(back.note_text(NoteKind::Endnote, "1"), "At the end.");
    let labels: Vec<String> = back.note_refs().into_iter().map(|r| r.label).collect();
    assert_eq!(labels, ["1", "i"]);
    // Deleting a note removes its reference too.
    let mut back = back;
    assert!(back.delete_note(NoteKind::Footnote, "1", Selection::default()));
    assert_eq!(back.note_refs().len(), 1);
    assert_eq!(back.paragraph(0).text, "Hello world\u{FFFC}");
}
