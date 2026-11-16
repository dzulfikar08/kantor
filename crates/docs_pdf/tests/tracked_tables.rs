//! Tracked table rows and cells are shaded as Word shades them (inserted
//! light blue, deleted pink), on screen and in the PDF.

use std::path::PathBuf;

use docs_engine::{Document, RevKind};
use docs_layout::Engine;

#[test]
fn tracked_cells_and_rows_are_shaded() {
    for name in ["tracked_cells.docx", "tracked.docx"] {
        let path =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx").join(name);
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
