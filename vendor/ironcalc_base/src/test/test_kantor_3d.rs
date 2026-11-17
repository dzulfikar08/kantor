#![allow(clippy::unwrap_used)]
//! KANTOR PATCH (3-D references): `SUM(Jan:Mar!B2)` and friends.

use crate::UserModel;

/// Jan, Feb, Mar with 1, 2, 4 in B2 and 10, 20, 40 in C3; Summary first.
fn quarter() -> UserModel<'static> {
    let mut m = UserModel::new_empty("t", "en", "UTC", "en").unwrap();
    m.rename_sheet(0, "Summary").unwrap();
    for (i, (name, b2, c3)) in [("Jan", 1, 10), ("Feb", 2, 20), ("Mar", 4, 40)]
        .iter()
        .enumerate()
    {
        let index = i as u32 + 1;
        m.kantor_insert_sheet(name, index).unwrap();
        m.set_user_input(index, 2, 2, &b2.to_string()).unwrap();
        m.set_user_input(index, 3, 3, &c3.to_string()).unwrap();
    }
    m
}

fn value(m: &UserModel, sheet: u32, row: i32, column: i32) -> String {
    m.get_formatted_cell_value(sheet, row, column).unwrap()
}

fn formula(m: &UserModel, sheet: u32, row: i32, column: i32) -> String {
    m.get_cell_content(sheet, row, column).unwrap()
}

#[test]
fn sums_over_sheets() {
    let mut m = quarter();
    m.set_user_input(0, 1, 1, "=SUM(Jan:Mar!B2)").unwrap();
    m.set_user_input(0, 2, 1, "=SUM(Jan:Mar!B2:C3)").unwrap();
    m.set_user_input(0, 3, 1, "=AVERAGE(Jan:Feb!B2)").unwrap();
    m.set_user_input(0, 4, 1, "=COUNT(Jan:Mar!A1:C3)").unwrap();
    m.set_user_input(0, 5, 1, "=MAX(Jan:Mar!C3)+MIN(Jan:Mar!C3)").unwrap();
    m.set_user_input(0, 6, 1, "=SUM('Jan:Mar'!$B$2, 100)").unwrap();
    m.set_user_input(0, 7, 1, "=PRODUCT(Jan:Mar!B2)").unwrap();
    assert_eq!(value(&m, 0, 1, 1), "7");
    assert_eq!(value(&m, 0, 2, 1), "77");
    assert_eq!(value(&m, 0, 3, 1), "1.5");
    assert_eq!(value(&m, 0, 4, 1), "6");
    assert_eq!(value(&m, 0, 5, 1), "50");
    assert_eq!(value(&m, 0, 6, 1), "107");
    assert_eq!(value(&m, 0, 7, 1), "8");
    assert_eq!(formula(&m, 0, 1, 1), "=SUM(Jan:Mar!B2)");
    assert_eq!(formula(&m, 0, 2, 1), "=SUM(Jan:Mar!B2:C3)");
    assert_eq!(formula(&m, 0, 6, 1), "=SUM(Jan:Mar!$B$2,100)");

    // Edits on any sheet in between are picked up.
    m.set_user_input(2, 2, 2, "30").unwrap();
    assert_eq!(value(&m, 0, 1, 1), "35");
}

#[test]
fn quoted_names_and_elsewhere() {
    let mut m = quarter();
    m.rename_sheet(1, "Jan 26").unwrap();
    m.set_user_input(0, 1, 1, "=SUM('Jan 26:Mar'!B2)").unwrap();
    assert_eq!(value(&m, 0, 1, 1), "7");
    assert_eq!(formula(&m, 0, 1, 1), "=SUM('Jan 26:Mar'!B2)");
    // Not a function that reads them: #VALUE!, as Excel.
    m.set_user_input(0, 2, 1, "=Feb:Mar!B2").unwrap();
    assert_eq!(value(&m, 0, 2, 1), "#VALUE!");
    m.set_user_input(0, 3, 1, "=SUMIF(Feb:Mar!B2,\">0\")").unwrap();
    assert_eq!(value(&m, 0, 3, 1), "#VALUE!");
    // A missing sheet is #REF!.
    m.set_user_input(0, 4, 1, "=SUM(Jan 26:Apr!B2)").unwrap();
    m.set_user_input(0, 4, 1, "=SUM(Feb:Apr!B2)").unwrap();
    assert_eq!(value(&m, 0, 4, 1), "#REF!");
}

#[test]
fn follows_sheet_changes() {
    let mut m = quarter();
    m.set_user_input(0, 1, 1, "=SUM(Jan:Mar!B2)").unwrap();

    // Renaming an end renames it in the formula.
    m.rename_sheet(3, "March").unwrap();
    assert_eq!(formula(&m, 0, 1, 1), "=SUM(Jan:March!B2)");
    assert_eq!(value(&m, 0, 1, 1), "7");

    // A sheet inserted in between is counted; one outside isn't.
    m.kantor_insert_sheet("Extra", 2).unwrap();
    m.set_user_input(2, 2, 2, "100").unwrap();
    m.kantor_insert_sheet("After", 5).unwrap();
    m.set_user_input(5, 2, 2, "1000").unwrap();
    assert_eq!(value(&m, 0, 1, 1), "107");

    // Moving a sheet out of the span drops it.
    m.move_sheet(2, 5).unwrap(); // Extra after After
    assert_eq!(value(&m, 0, 1, 1), "7");

    // Deleting a middle sheet drops its value; an end moves inwards.
    // Sheets: Summary, Jan, Feb, March, After, Extra
    m.kantor_delete_sheet(2).unwrap(); // Feb
    assert_eq!(value(&m, 0, 1, 1), "5");
    m.kantor_delete_sheet(1).unwrap(); // Jan, an end
    assert_eq!(formula(&m, 0, 1, 1), "=SUM(March:March!B2)");
    assert_eq!(value(&m, 0, 1, 1), "4");
    m.undo().unwrap();
    assert_eq!(formula(&m, 0, 1, 1), "=SUM(Jan:March!B2)");
    m.redo().unwrap();
    m.kantor_delete_sheet(1).unwrap(); // March, the last one
    assert_eq!(formula(&m, 0, 1, 1), "=SUM(#REF!)");
}

#[test]
fn rows_inserted_leave_it_alone_and_fill_moves_it() {
    let mut m = quarter();
    m.set_user_input(0, 1, 1, "=SUM(Jan:Mar!B2)").unwrap();
    m.insert_rows(1, 1, 1).unwrap();
    assert_eq!(formula(&m, 0, 1, 1), "=SUM(Jan:Mar!B2)");
    // Copying the formula one row down moves the relative reference.
    m.set_user_input(0, 1, 2, "=SUM(Jan:Mar!B$2:B2)").unwrap();
    m.auto_fill_rows(
        &crate::expressions::types::Area {
            sheet: 0,
            row: 1,
            column: 2,
            width: 1,
            height: 1,
        },
        2,
    )
    .unwrap();
    assert_eq!(formula(&m, 0, 2, 2), "=SUM(Jan:Mar!B$2:B3)");
    assert_eq!(value(&m, 0, 2, 2), "7");
}
