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

// ------------------------------------------------------------ editing notes

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("notes");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn parts(path: &Path) -> std::collections::BTreeMap<String, String> {
    use std::io::Read as _;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = std::collections::BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut s = String::new();
        if e.read_to_string(&mut s).is_ok() {
            out.insert(e.name().unwrap().into_owned(), s);
        }
    }
    out
}

fn problems(path: &Path) -> Vec<String> {
    sheets_engine::package_problems(&std::fs::read(path).unwrap())
}

fn text(wb: &Workbook, row: u32, col: u32) -> Option<String> {
    wb.note_at(0, row, col).map(|n| n.text)
}

#[test]
fn new_edit_delete_undo_and_move_with_rows() {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_note(0, 1, 1, "Dewi:\nCheck the total", "Dewi")
        .unwrap();
    assert_eq!(text(&wb, 1, 1).as_deref(), Some("Dewi:\nCheck the total"));
    assert_eq!(wb.note_at(0, 1, 1).unwrap().author, "Dewi");
    // Editing keeps the author; the same text again is no undo step.
    wb.set_note(0, 1, 1, "Dewi:\nChecked", "Someone else")
        .unwrap();
    assert_eq!(wb.note_at(0, 1, 1).unwrap().author, "Dewi");
    wb.set_note(0, 1, 1, "Dewi:\nChecked", "Dewi").unwrap();
    wb.undo().unwrap();
    assert_eq!(text(&wb, 1, 1).as_deref(), Some("Dewi:\nCheck the total"));
    wb.redo().unwrap();
    assert_eq!(text(&wb, 1, 1).as_deref(), Some("Dewi:\nChecked"));
    // Notes move with inserted and deleted rows and columns.
    wb.set_note(0, 4, 0, "second", "Dewi").unwrap();
    wb.insert_rows(0, 0, 2).unwrap();
    wb.insert_columns(0, 0, 1).unwrap();
    assert_eq!(text(&wb, 1, 1), None);
    assert_eq!(text(&wb, 3, 2).as_deref(), Some("Dewi:\nChecked"));
    assert_eq!(text(&wb, 6, 1).as_deref(), Some("second"));
    assert_eq!(wb.note_cells(0), vec![(3, 2), (6, 1)]);
    assert_eq!(wb.next_note(0, (0, 0), false), Some((3, 2)));
    assert_eq!(wb.next_note(0, (3, 2), false), Some((6, 1)));
    assert_eq!(wb.next_note(0, (6, 1), false), Some((3, 2)));
    assert_eq!(wb.next_note(0, (3, 2), true), Some((6, 1)));
    // A note added after the move moves with later edits too.
    wb.set_note(0, 0, 0, "top", "Dewi").unwrap();
    wb.delete_rows(0, 3, 3).unwrap();
    assert_eq!(wb.note_cells(0), vec![(0, 0), (5, 1)]);
    wb.undo().unwrap();
    assert_eq!(wb.note_cells(0), vec![(0, 0), (3, 2), (6, 1)]);
    // Delete Note on a range; Show/Hide Note.
    let all = sheets_engine::CellRange::from_corners((0, 0), (9, 9));
    assert!(wb.show_notes(0, all, true).unwrap());
    assert!(wb.note_at(0, 3, 2).unwrap().shown);
    assert!(!wb.show_notes(0, all, true).unwrap());
    let one = sheets_engine::CellRange::from_corners((3, 2), (3, 2));
    assert!(wb.delete_notes(0, one).unwrap());
    assert!(!wb.delete_notes(0, one).unwrap());
    assert_eq!(wb.note_cells(0), vec![(0, 0), (6, 1)]);
    wb.undo().unwrap();
    assert_eq!(text(&wb, 3, 2).as_deref(), Some("Dewi:\nChecked"));
}

#[test]
fn notes_made_in_kantor_are_saved_for_excel() {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "Total").unwrap();
    wb.set_note(0, 0, 0, "Dewi:\nAll shops & stalls", "Dewi")
        .unwrap();
    wb.set_note(0, 2, 3, "Plain <note>", "Bayu").unwrap();
    let all = sheets_engine::CellRange::from_corners((2, 3), (2, 3));
    wb.show_notes(0, all, true).unwrap();
    let out = scratch("new.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert_eq!(problems(&out), Vec::<String>::new());
    let p = parts(&out);
    let comments = &p["xl/comments1.xml"];
    assert!(comments.contains("<author>Dewi</author><author>Bayu</author>"));
    assert!(comments.contains("<b/>"), "bold author line: {comments}");
    assert!(comments.contains("Plain &lt;note&gt;"));
    let vml = &p["xl/drawings/vmlDrawing1.vml"];
    assert_eq!(vml.matches("<v:shape ").count(), 2);
    assert_eq!(vml.matches("<x:Visible/>").count(), 1);
    assert!(p["xl/worksheets/sheet1.xml"].contains("<legacyDrawing r:id="));
    assert!(p["[Content_Types].xml"].contains("/xl/comments1.xml"));
    let back = Workbook::open(&out).unwrap();
    assert_eq!(
        text(&back, 0, 0).as_deref(),
        Some("Dewi:\nAll shops & stalls")
    );
    assert_eq!(back.note_at(0, 2, 3).unwrap().author, "Bayu");
    assert!(back.note_at(0, 2, 3).unwrap().shown);
    assert!(!back.note_at(0, 0, 0).unwrap().shown);
}

#[test]
fn editing_a_files_notes_keeps_the_rest_and_deleting_all_removes_the_parts() {
    let out = scratch("edited.xlsx");
    let mut wb = Workbook::open(fixture()).expect("opens");
    // B2: edited; A3: kept; D5: new.
    wb.set_note(
        0,
        1,
        1,
        "Includes the pop-up stall and the kiosk.",
        "Kantor",
    )
    .unwrap();
    wb.set_note(0, 4, 3, "New one", "Dewi").unwrap();
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert_eq!(problems(&out), Vec::<String>::new());
    let back = Workbook::open(&out).unwrap();
    assert_eq!(
        note(&back, 1, 1),
        Some((
            "Rina".into(),
            "Includes the pop-up stall and the kiosk.".into()
        ))
    );
    assert_eq!(
        note(&back, 2, 0),
        Some(("Bayu".into(), "New shop opened in March.".into()))
    );
    assert_eq!(note(&back, 4, 3), Some(("Dewi".into(), "New one".into())));
    let p = parts(&out);
    let vml: Vec<&String> = p
        .iter()
        .filter(|(k, _)| k.ends_with(".vml"))
        .map(|(_, v)| v)
        .collect();
    assert_eq!(vml.len(), 1);
    assert_eq!(vml[0].matches("ObjectType=\"Note\"").count(), 3);

    // Delete every note: no comments or VML part is left behind.
    let mut wb = Workbook::open(&out).unwrap();
    let all = sheets_engine::CellRange::from_corners((0, 0), (99, 25));
    assert!(wb.delete_notes(0, all).unwrap());
    let none = scratch("none.xlsx");
    wb.save_as(&none).unwrap();
    assert_eq!(problems(&none), Vec::<String>::new());
    let p = parts(&none);
    assert!(
        !p.keys()
            .any(|k| k.contains("comment") || k.ends_with(".vml")),
        "{:?}",
        p.keys().collect::<Vec<_>>()
    );
    assert!(!p["xl/worksheets/sheet1.xml"].contains("legacyDrawing"));
    assert!(!p["[Content_Types].xml"].contains("comments+xml"));
    assert!(Workbook::open(&none).unwrap().notes_now(0).is_empty());
}

#[test]
fn save_edit_save_and_save_undo_save() {
    let a = scratch("session-a.xlsx");
    let b = scratch("session-b.xlsx");
    let c = scratch("session-c.xlsx");
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_note(0, 0, 0, "first", "Dewi").unwrap();
    wb.save_as(&a).unwrap();
    // Saved again unchanged: every part the same (but the save time).
    wb.save_as(&b).unwrap();
    let (pa, pb) = (parts(&a), parts(&b));
    for (name, text) in &pa {
        if name == "docProps/core.xml" {
            continue;
        }
        assert_eq!(Some(text), pb.get(name), "{name} changed");
    }
    // Edit, add, save: one comments part, one drawing, both notes.
    wb.set_note(0, 0, 0, "first, edited", "Dewi").unwrap();
    wb.set_note(0, 3, 1, "second", "Dewi").unwrap();
    wb.save_as(&b).unwrap();
    assert_eq!(problems(&b), Vec::<String>::new());
    let p = parts(&b);
    assert_eq!(p.keys().filter(|k| k.contains("comments")).count(), 1);
    assert_eq!(p.keys().filter(|k| k.ends_with(".vml")).count(), 1);
    let back = Workbook::open(&b).unwrap();
    assert_eq!(text(&back, 0, 0).as_deref(), Some("first, edited"));
    assert_eq!(text(&back, 3, 1).as_deref(), Some("second"));
    // Undo both and save: the file is back to the first note alone.
    wb.undo().unwrap();
    wb.undo().unwrap();
    wb.save_as(&c).unwrap();
    assert_eq!(problems(&c), Vec::<String>::new());
    let back = Workbook::open(&c).unwrap();
    assert_eq!(text(&back, 0, 0).as_deref(), Some("first"));
    assert_eq!(text(&back, 3, 1), None);
    assert_eq!(back.notes_now(0).len(), 1);
    assert_eq!(
        parts(&c)
            .values()
            .map(|x| x.matches("ObjectType=\"Note\"").count())
            .sum::<usize>(),
        1
    );
    // Undo the first note too: saved without notes.
    wb.undo().unwrap();
    wb.save_as(&c).unwrap();
    assert_eq!(problems(&c), Vec::<String>::new());
    assert!(Workbook::open(&c).unwrap().notes_now(0).is_empty());
}

#[test]
fn notes_follow_rows_inserted_after_a_save() {
    let a = scratch("moved-a.xlsx");
    let mut wb = Workbook::open(fixture()).unwrap();
    wb.save_as(&a).unwrap();
    wb.insert_rows(0, 0, 1).unwrap();
    assert_eq!(note(&wb, 2, 1).unwrap().0, "Rina");
    wb.set_note(0, 0, 0, "top", "Dewi").unwrap();
    wb.save_as(&a).unwrap();
    assert_eq!(problems(&a), Vec::<String>::new());
    let back = Workbook::open(&a).unwrap();
    assert_eq!(note(&back, 2, 1).unwrap().0, "Rina");
    assert_eq!(note(&back, 3, 0).unwrap().0, "Bayu");
    assert_eq!(text(&back, 0, 0).as_deref(), Some("top"));
}
