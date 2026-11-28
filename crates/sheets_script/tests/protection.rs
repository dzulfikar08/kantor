//! Cell protection flags (`Range.Locked`, `FormulaHidden`), `Protect`
//! accepted without effect, and `Range.RemoveDuplicates` keeping each kept
//! row's formats.

mod support;

use sheets_engine::{CellRange, Workbook};
use support::*;

const LOCK: &str = r#"
Sub LockForm()
    Cells.Locked = False
    Range("A1:C1").Locked = True
    Range("C2:C5").FormulaHidden = True
    ActiveSheet.Protect Password:="pw", DrawingObjects:=True, Contents:=True, UserInterfaceOnly:=True
    Debug.Print Range("A1").Locked & "|" & Range("A2").Locked & "|" & Range("C3").FormulaHidden & "|" & IsNull(Range("A1:A2").Locked)
    ActiveSheet.Unprotect "pw"
    Range("B3").Value = "still editable"
End Sub
"#;

fn check_lock(book: &Workbook) {
    assert_eq!(book.cell_protection(0, 0, 0), (true, false));
    assert_eq!(book.cell_protection(0, 0, 2), (true, false));
    assert_eq!(book.cell_protection(0, 1, 0), (false, false));
    assert_eq!(book.cell_protection(0, 2, 2), (false, true));
    assert_eq!(book.cell_protection(0, 4, 1), (false, false));
}

#[test]
fn locked_and_formula_hidden_are_kept_and_saved() {
    let (book, outcome, printed) = run_printed(sales(), LOCK, "LockForm");
    ok(&outcome);
    assert_eq!(printed, ["True|False|True|True"]);
    check_lock(&book);
    assert_eq!(cell(&book, 0, "B3"), "still editable");
    // Other formatting stays.
    assert_eq!(cell(&book, 0, "C2"), "300");

    let (reopened, dir) = reopen(book, "locked.xlsx");
    check_lock(&reopened);
    let styles = package_text(&dir.path().join("locked.xlsx"), "xl/styles.xml").unwrap();
    assert!(styles.contains("locked=\"0\""), "{styles}");
    assert!(styles.contains("hidden=\"1\""), "{styles}");
}

#[test]
fn undo_takes_locked_back() {
    let (mut book, outcome) = run(sales(), LOCK, "LockForm");
    ok(&outcome);
    assert!(book.undo().unwrap());
    assert_eq!(book.cell_protection(0, 1, 0), (true, false));
    assert_eq!(book.cell_protection(0, 2, 2), (true, false));
    assert_eq!(cell(&book, 0, "B3"), "West");
}

#[test]
fn protection_is_accepted_and_reads_unprotected() {
    let code = r#"
Sub Go()
    ThisWorkbook.Protect Password:="pw", Structure:=True
    ActiveSheet.Protect "pw"
    Debug.Print ActiveSheet.ProtectContents & "|" & Range("A1").Locked
    ThisWorkbook.Unprotect "pw"
End Sub
"#;
    let (_, outcome, printed) = run_printed(sales(), code, "Go");
    ok(&outcome);
    assert_eq!(printed, ["False|True"]);
}

/// Name and Region rows with duplicates; the rows of Dewi and Citra bold.
fn duplicates() -> Workbook {
    let mut b = Workbook::new_empty().unwrap();
    let rows = [
        ["Name", "Region", "Amount"],
        ["Dewi", "East", "300"],
        ["Budi", "West", "150"],
        ["dewi", "East", "999"],
        ["Citra", "North", "50"],
        ["Budi", "West", "150"],
    ];
    for (i, row) in rows.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            b.set_input(0, i as u32, j as u32, v).unwrap();
        }
    }
    for r in [1, 4] {
        b.set_range_style_value(0, CellRange::from_corners((r, 0), (r, 2)), "font.b", "true")
            .unwrap();
    }
    b.set_input(0, 7, 0, "below").unwrap();
    b
}

const DEDUPE: &str = r#"
Sub Dedupe()
    Range("A1:C6").RemoveDuplicates Columns:=Array(1, 2), Header:=xlYes
End Sub
"#;

#[test]
fn remove_duplicates_keeps_each_rows_formats() {
    let (book, outcome) = run(duplicates(), DEDUPE, "Dedupe");
    ok(&outcome);
    let names: Vec<String> = (1..=6).map(|r| cell(&book, 0, &format!("A{r}"))).collect();
    assert_eq!(names, ["Name", "Dewi", "Budi", "Citra", "", ""]);
    assert_eq!(cell(&book, 0, "C4"), "50");
    let bold = |r: u32, c: u32| book.cell_style(0, r, c).bold;
    assert!(bold(1, 0) && bold(1, 2), "Dewi's row stays bold");
    assert!(!bold(2, 0), "Budi's row stays plain");
    assert!(
        bold(3, 0) && bold(3, 1),
        "Citra's row moved up with its bold"
    );
    assert!(!bold(4, 0) && !bold(5, 0), "the freed rows are cleared");
    // Outside the range nothing moves.
    assert_eq!(cell(&book, 0, "A8"), "below");
}

#[test]
fn remove_duplicates_is_one_undo_step() {
    let (mut book, outcome) = run(duplicates(), DEDUPE, "Dedupe");
    ok(&outcome);
    assert!(book.undo().unwrap());
    assert_eq!(cell(&book, 0, "A4"), "dewi");
    assert_eq!(cell(&book, 0, "A6"), "Budi");
    assert!(book.cell_style(0, 4, 0).bold);
    assert!(!book.cell_style(0, 3, 0).bold);
}

#[test]
fn remove_duplicates_with_a_column_outside_the_range_fails() {
    let code =
        "Sub Go()\n    Range(\"A1:B6\").RemoveDuplicates Columns:=3, Header:=xlYes\nEnd Sub\n";
    let (_, outcome) = run(duplicates(), code, "Go");
    assert_eq!(err(&outcome).number, 1004);
}
