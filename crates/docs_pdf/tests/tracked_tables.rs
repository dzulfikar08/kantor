//! Tracked table rows and cells are shaded as Word shades them (inserted
//! light blue, deleted pink), on screen and in the PDF.

use std::path::PathBuf;

use docs_engine::{Document, RevKind};
use docs_layout::Engine;

#[test]
fn tracked_cells_and_rows_are_shaded() {
    for name in ["tracked_cells.docx", "tracked.docx"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/docx")
            .join(name);
        let doc = Document::open(&path).unwrap();
        let mut engine = Engine::new();
        let laid = docs_pdf::lay_out(&doc, &mut engine);
        let colors: Vec<_> = laid.paged.pages[0].rules.iter().map(|r| r.color).collect();
        for kind in [RevKind::Insert, RevKind::Delete] {
            let want = docs_pdf::build::tracked_fill(kind);
            // One shaded rectangle per tracked cell (two in each file).
            assert_eq!(
                colors.iter().filter(|c| **c == want).count(),
                2,
                "{name} {kind:?}"
            );
        }
        let scene = docs_pdf::scene(&doc, &laid);
        let want = docs_pdf::build::tracked_fill(RevKind::Insert);
        assert!(scene.pages[0].rules.iter().any(|r| r.1 == want));
    }
}

/// A tracked merge across columns (`w:hMerge`) is drawn as one cell,
/// shaded as Word shades merges; accepted, it's one cell spanning both.
#[test]
fn tracked_merge_across_columns_is_drawn_merged() {
    use docs_engine::{Paragraph, Position, Selection};
    let mut d = Document::from_paragraphs(vec![Paragraph::new("x")]);
    let caret = |p, o| Selection::caret(Position::new(p, o));
    let mut c = d.insert_table(caret(0, 1), 1, 3).unwrap();
    for (k, t) in ["a", "b", "c"].iter().enumerate() {
        c = d.insert_text(c, t, None);
        if k < 2 {
            c = d.next_cell(c, true).unwrap();
        }
    }
    d.set_track_changes(true, Selection::default());
    let at = |d: &Document, t: &str| d.paragraphs().iter().position(|p| p.text == t).unwrap();
    let sel = Selection {
        anchor: Position::new(at(&d, "a"), 0),
        head: Position::new(at(&d, "b"), 0),
    };
    d.merge_cells(sel).unwrap();
    let (_, _, blocks) = docs_pdf::build::build(&d);
    let t = blocks
        .iter()
        .find_map(|b| match b {
            docs_layout::Block::Table(t) => Some(t),
            _ => None,
        })
        .unwrap();
    let row = &t.rows[0];
    assert_eq!((row.cells[0].col, row.cells[0].span), (0, 2));
    assert_eq!(row.cells[0].fill, Some(docs_pdf::build::MERGED_FILL));
    assert_eq!(row.cells[1].merge, docs_layout::Merge::Continue);
    assert_eq!(row.cells[1].borders, [None; 4]);
    assert_eq!((row.cells[2].col, row.cells[2].span), (2, 1));
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&d, &mut engine);
    assert!(
        laid.paged.pages[0]
            .rules
            .iter()
            .any(|r| r.color == docs_pdf::build::MERGED_FILL)
    );
}
