//! Notes (cell comments) of an .xlsx: openpyxl's absolute comments target,
//! where the notes are after row edits, and that a save keeps them.

use std::path::{Path, PathBuf};

use sheets_engine::Workbook;

/// openpyxl output (vendor/ironcalc/tests/gen_openpyxl_comments.py): the
/// sheet's comments relationship target is `/xl/comments/comment1.xml`.
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../vendor/ironcalc/tests/openpyxl_comments.xlsx")
}

fn note(wb: &Workbook, row: u32, col: u32) -> Option<(String, String)> {
    wb.note_at(0, row, col).map(|n| (n.author, n.text))
}

#[test]
fn openpyxl_notes_open_and_follow_their_cells() {
    let mut wb = Workbook::open(fixture()).expect("opens");
    assert_eq!(
        note(&wb, 1, 1),
        Some(("Rina".into(), "Includes the pop-up stall.".into()))
    );
    assert_eq!(
        note(&wb, 2, 0),
        Some(("Bayu".into(), "New shop opened in March.".into()))
    );
    assert_eq!(wb.notes_now(0).len(), 2);
    // A row inserted above moves both notes down; deleting B3's row drops it.
    wb.insert_rows(0, 1, 1).unwrap();
    assert_eq!(note(&wb, 1, 1), None);
    assert_eq!(note(&wb, 2, 1).unwrap().0, "Rina");
    assert_eq!(note(&wb, 3, 0).unwrap().0, "Bayu");
    wb.delete_rows(0, 2, 2).unwrap();
    assert_eq!(wb.notes_now(0).len(), 1);
    assert_eq!(note(&wb, 2, 0).unwrap().0, "Bayu");
    wb.undo().unwrap();
    wb.undo().unwrap();
    assert_eq!(note(&wb, 1, 1).unwrap().0, "Rina");
}

#[test]
fn a_save_keeps_the_notes() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("saved.xlsx");
    let mut wb = Workbook::open(fixture()).unwrap();
    wb.set_input(0, 0, 3, "Solo").unwrap();
    wb.save_as(&out).unwrap();
    let wb = Workbook::open(&out).unwrap();
    assert_eq!(
        note(&wb, 1, 1),
        Some(("Rina".into(), "Includes the pop-up stall.".into()))
    );
    assert_eq!(wb.notes_now(0).len(), 2);
}
