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

// ------------------------------------------------ note boxes and formatting

#[test]
fn note_boxes_and_formatted_text_save_in_place() {
    use sheets_engine::notes::{NoteBox, NoteRun};
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx/12_comments_links_props.xlsx");
    let mut wb = Workbook::open(&src).unwrap();
    let a3 = wb.note_at(0, 2, 0).unwrap();
    assert_eq!(a3.place, None);
    assert_eq!(a3.runs, None);
    // B1's box moved and made bigger.
    let place = NoteBox {
        left: (2, 5),
        top: (1, 0),
        right: (5, 10),
        bottom: (8, 3),
    };
    wb.set_note_place(0, 0, 1, place).unwrap();
    // The same place again: no undo step.
    let rev = wb.revision();
    wb.set_note_place(0, 0, 1, place).unwrap();
    assert_eq!(wb.revision(), rev);
    let out = scratch("boxes.xlsx");
    wb.save_as(&out).unwrap();
    assert!(problems(&out).is_empty(), "{:?}", problems(&out));
    let before = parts(&src);
    let after = parts(&out);
    // Only B1's anchor went into the VML; the comments part is as it was.
    assert_eq!(before["xl/comments1.xml"], after["xl/comments1.xml"]);
    let vml = &after["xl/drawings/vmlDrawing1.vml"];
    assert!(
        vml.contains("<x:Anchor>3, 5, 1, 0, 6, 10, 8, 3</x:Anchor><x:AutoFill>"),
        "{vml}"
    );
    assert_eq!(vml.matches("<x:Anchor>").count(), 1);
    let back = Workbook::open(&out).unwrap();
    assert_eq!(back.note_at(0, 0, 1).unwrap().place, Some(place));
    // A box follows its cell down when rows go in above.
    let mut moved = Workbook::open(&out).unwrap();
    moved.insert_rows(0, 0, 2).unwrap();
    let n = moved.note_at(0, 2, 1).unwrap();
    assert_eq!(n.place_or_default().anchor(n.row, n.col)[2], 3);

    // Formatted text: a bold red word in A3's note.
    let mut wb = Workbook::open(&out).unwrap();
    let runs = vec![
        NoteRun {
            text: "Komentar ".into(),
            ..NoteRun::default()
        },
        NoteRun {
            text: "penting".into(),
            bold: true,
            color: Some(0xC00000),
            size: Some(1100),
            ..NoteRun::default()
        },
        NoteRun {
            text: " sekali.".into(),
            italic: true,
            ..NoteRun::default()
        },
    ];
    wb.set_note_runs(0, 2, 0, runs.clone(), "Kantor").unwrap();
    assert_eq!(
        wb.note_at(0, 2, 0).unwrap().text,
        "Komentar penting sekali."
    );
    let out2 = scratch("formatted.xlsx");
    wb.save_as(&out2).unwrap();
    assert!(problems(&out2).is_empty(), "{:?}", problems(&out2));
    let c = &parts(&out2)["xl/comments1.xml"];
    // B1's comment stays as it was; A3's is written with its runs.
    assert!(c.contains("<comment ref=\"B1\" authorId=\"0\"><text><r><t>Budget figure from the planning meeting.</t></r></text></comment>"));
    assert!(c.contains("<r><rPr><b/><sz val=\"11\"/><color rgb=\"FFC00000\"/><rFont val=\"Tahoma\"/></rPr><t xml:space=\"preserve\">penting</t></r>"), "{c}");
    let back = Workbook::open(&out2).unwrap();
    let n = back.note_at(0, 2, 0).unwrap();
    let got = n.runs.as_deref().unwrap();
    assert_eq!(got.len(), 3);
    assert!(got[1].bold && got[1].color == Some(0xC00000));
    assert!(got[2].italic);
    // B1's box kept its place through the second save.
    assert_eq!(back.note_at(0, 0, 1).unwrap().place, Some(place));
    // Plain text with Excel's default formatting reads as plain.
    assert_eq!(back.note_at(0, 0, 1).unwrap().runs, None);
}
