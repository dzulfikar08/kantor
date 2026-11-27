//! Saving away from the UI thread (`Document::begin_save`, `SaveJob::run`,
//! `Document::finish_save`): the file gets the document as it was when the
//! save began; edits made meanwhile stay unsaved and go into the next save.

use docs_engine::{Document, Position, Selection};

#[test]
fn edits_during_a_background_save_stay_unsaved() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.docx");
    let mut doc = Document::new();
    doc.insert_text(Selection::default(), "first\nsecond", None);
    doc.save_as(&path).unwrap();

    doc.insert_text(Selection::caret(Position::new(0, 5)), " edited", None);
    let job = doc.begin_save().unwrap();
    // Typing goes on while it's written.
    doc.insert_text(Selection::caret(Position::new(1, 6)), " later", None);
    let done = job.run();
    doc.finish_save(done).unwrap();
    assert!(doc.is_dirty(), "the later edit isn't saved");
    let saved = Document::open(&path).unwrap();
    assert_eq!(saved.paragraph(0).text, "first edited");
    assert_eq!(saved.paragraph(1).text, "second");

    doc.save().unwrap();
    assert!(!doc.is_dirty());
    let saved = Document::open(&path).unwrap();
    assert_eq!(saved.paragraph(1).text, "second later");
}

#[test]
fn a_background_save_with_nothing_edited_meanwhile_is_a_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.docx");
    let mut doc = Document::new();
    doc.insert_text(Selection::default(), "one", None);
    doc.save_as(&path).unwrap();
    doc.insert_text(Selection::caret(Position::new(0, 3)), " two", None);
    let job = doc.begin_save().unwrap();
    doc.finish_save(job.run()).unwrap();
    assert!(!doc.is_dirty());
    assert_eq!(Document::open(&path).unwrap().paragraph(0).text, "one two");
    // And it saves again from there.
    doc.insert_text(Selection::caret(Position::new(0, 7)), " three", None);
    doc.save().unwrap();
    assert_eq!(
        Document::open(&path).unwrap().paragraph(0).text,
        "one two three"
    );
}
