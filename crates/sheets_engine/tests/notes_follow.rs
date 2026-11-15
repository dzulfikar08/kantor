//! Notes follow their cells as in Excel: Sort moves them with their rows,
//! Paste brings the copied cells' notes (Cut moves them), and the fill
//! handle copies them along. Each edit undoes as one step.

use sheets_engine::{CellRange, Fill, FillDirection, SortKey, Workbook};

fn note(wb: &Workbook, sheet: u32, row: u32, col: u32) -> Option<String> {
    wb.note_at(sheet, row, col).map(|n| n.text)
}

fn cells(wb: &Workbook, sheet: u32) -> Vec<(u32, u32, String)> {
    let mut v: Vec<(u32, u32, String)> = wb
        .notes_now(sheet)
        .values()
        .map(|n| (n.row, n.col, n.text.clone()))
        .collect();
    v.sort();
    v
}

#[test]
fn sorting_moves_notes_with_their_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::new_empty().unwrap();
    for (r, v) in ["Name", "Cherry", "Apple", "Banana"].iter().enumerate() {
        wb.set_input(0, r as u32, 0, v).unwrap();
        wb.set_input(0, r as u32, 1, &format!("{}", r * 10))
            .unwrap();
    }
    wb.set_note(0, 1, 1, "cherry's count", "Rina").unwrap();
    wb.set_note(0, 2, 0, "apple", "Rina").unwrap();
    // Outside the sorted columns: stays.
    wb.set_note(0, 1, 3, "not sorted", "Rina").unwrap();
    let range = CellRange::from_corners((0, 0), (3, 1));
    wb.sort_range(
        0,
        range,
        &[SortKey {
            col: 0,
            descending: false,
        }],
        true,
    )
    .unwrap();
    assert_eq!(wb.display_value(0, 1, 0), "Apple");
    assert_eq!(note(&wb, 0, 1, 0).as_deref(), Some("apple"));
    assert_eq!(note(&wb, 0, 3, 1).as_deref(), Some("cherry's count"));
    assert_eq!(note(&wb, 0, 1, 3).as_deref(), Some("not sorted"));
    assert_eq!(wb.notes_now(0).len(), 3);
    // Saved where they are now.
    let path = dir.path().join("sorted.xlsx");
    wb.save_as(&path).unwrap();
    let back = Workbook::open(&path).unwrap();
    assert_eq!(note(&back, 0, 3, 1).as_deref(), Some("cherry's count"));
    assert_eq!(note(&back, 0, 1, 0).as_deref(), Some("apple"));
    // One undo puts values and notes back.
    wb.undo().unwrap();
    assert_eq!(wb.display_value(0, 1, 0), "Cherry");
    assert_eq!(note(&wb, 0, 1, 1).as_deref(), Some("cherry's count"));
    assert_eq!(note(&wb, 0, 2, 0).as_deref(), Some("apple"));
    wb.redo().unwrap();
    assert_eq!(note(&wb, 0, 3, 1).as_deref(), Some("cherry's count"));
}

#[test]
fn paste_brings_copied_notes_and_cut_moves_them() {
    let mut wb = Workbook::new_empty().unwrap();
    wb.insert_sheet(1).unwrap();
    wb.set_input(0, 0, 0, "a").unwrap();
    wb.set_input(0, 1, 0, "b").unwrap();
    wb.set_note(0, 0, 0, "on a", "Bayu").unwrap();
    wb.set_note(0, 5, 3, "will be pasted over", "Bayu").unwrap();
    wb.set_note(0, 6, 3, "kept below", "Bayu").unwrap();

    // Copy A1:A2, paste at D5: D5 gets A1's note, D6 loses its own (A2 had
    // none), A1 keeps its note.
    let range = CellRange::from_corners((0, 0), (1, 0));
    let text = wb.range_tsv(0, range);
    let clip = wb.note_clip(0, range, false);
    assert!(!clip.is_empty());
    let pasted = wb
        .paste_tsv_with_notes(0, 4, 3, &text, &clip)
        .unwrap()
        .unwrap();
    assert_eq!(pasted, CellRange::from_corners((4, 3), (5, 3)));
    assert_eq!(
        cells(&wb, 0),
        [
            (0, 0, "on a".into()),
            (4, 3, "on a".into()),
            (6, 3, "kept below".into())
        ]
    );
    // One undo: the values and the notes as before.
    wb.undo().unwrap();
    assert_eq!(wb.display_value(0, 4, 3), "");
    assert_eq!(note(&wb, 0, 5, 3).as_deref(), Some("will be pasted over"));
    assert_eq!(note(&wb, 0, 4, 3), None);

    // Cut A1:A2 and paste on the other sheet: the note moves.
    let clip = wb.note_clip(0, range, true);
    wb.clear_range(0, range).unwrap();
    wb.paste_tsv_with_notes(1, 2, 2, &text, &clip).unwrap();
    assert_eq!(note(&wb, 0, 0, 0), None);
    assert_eq!(note(&wb, 1, 2, 2).as_deref(), Some("on a"));
    assert_eq!(wb.display_value(1, 3, 2), "b");
    wb.undo().unwrap();
    assert_eq!(note(&wb, 0, 0, 0).as_deref(), Some("on a"));
    assert_eq!(note(&wb, 1, 2, 2), None);

    // Cut and paste on the same sheet, overlapping: A1:A2 down one row.
    let clip = wb.note_clip(0, range, true);
    wb.paste_tsv_with_notes(0, 1, 0, &text, &clip).unwrap();
    assert_eq!(note(&wb, 0, 0, 0), None);
    assert_eq!(note(&wb, 0, 1, 0).as_deref(), Some("on a"));

    // A plain paste (text from elsewhere) leaves notes alone.
    wb.paste_tsv(0, 6, 3, "x").unwrap();
    assert_eq!(note(&wb, 0, 6, 3).as_deref(), Some("kept below"));
}

#[test]
fn the_fill_handle_copies_notes() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "1").unwrap();
    wb.set_input(0, 1, 0, "2").unwrap();
    wb.set_note(0, 0, 0, "first", "Rina").unwrap();
    wb.set_note(0, 3, 0, "filled over", "Rina").unwrap();
    // A1:A2 down to A6: the pattern repeats (A3, A5 get A1's note; A4's
    // own goes, as A2 has none).
    let fill = Fill {
        source: CellRange::from_corners((0, 0), (1, 0)),
        direction: FillDirection::Down,
        to: 5,
    };
    wb.fill(0, &fill).unwrap();
    assert_eq!(wb.display_value(0, 5, 0), "6");
    assert_eq!(
        cells(&wb, 0),
        [
            (0, 0, "first".into()),
            (2, 0, "first".into()),
            (4, 0, "first".into())
        ]
    );
    let path = dir.path().join("filled.xlsx");
    wb.save_as(&path).unwrap();
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    assert_eq!(Workbook::open(&path).unwrap().notes_now(0).len(), 3);
    wb.undo().unwrap();
    assert_eq!(wb.display_value(0, 5, 0), "");
    assert_eq!(
        cells(&wb, 0),
        [(0, 0, "first".into()), (3, 0, "filled over".into())]
    );

    // Right, from B1 (no note) over C1's note: gone.
    wb.set_note(0, 0, 2, "C1", "Rina").unwrap();
    let fill = Fill {
        source: CellRange::from_corners((0, 1), (0, 1)),
        direction: FillDirection::Right,
        to: 3,
    };
    wb.fill(0, &fill).unwrap();
    assert_eq!(note(&wb, 0, 0, 2), None);
}
