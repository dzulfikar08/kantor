//! Tracked deletions in headers are struck through like the body's (on
//! screen and in the PDF).

use std::path::PathBuf;

use docs_engine::{Document, HfKind, HfType, Position, Selection};
use docs_layout::Engine;

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/docx")
        .join(name)
}

#[test]
fn header_deletions_are_struck_through() {
    let mut doc = Document::open(&corpus("report.docx")).unwrap();
    doc.set_author("Ani Wijaya");
    doc.set_track_changes(true, Selection::default());
    let (edit, mut h) = doc.edit_hf(0, HfKind::Header, HfType::Default).unwrap();
    let len = h.paragraphs()[0].text.len();
    assert!(len > 3);
    h.delete_selection(Selection {
        anchor: Position::new(0, 0),
        head: Position::new(0, 3),
    });
    assert_eq!(h.paragraphs()[0].text.len(), len, "deleted text stays");
    doc.finish_part(edit, h, Selection::default());
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let struck: usize = laid
        .furniture
        .iter()
        .flatten()
        .filter(|p| p.kind == HfKind::Header)
        .map(|p| p.layout.strike_rules().len())
        .sum();
    assert!(struck > 0, "header deletions drawn struck through");
}
