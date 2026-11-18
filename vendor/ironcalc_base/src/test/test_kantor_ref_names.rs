#![allow(clippy::unwrap_used)]
//! KANTOR PATCH (§33): defined names that point at a deleted sheet.

use crate::model::is_ref_error_name_formula;
use crate::UserModel;

fn value(m: &UserModel, sheet: u32, row: i32, column: i32) -> String {
    m.get_formatted_cell_value(sheet, row, column).unwrap()
}

#[test]
fn ref_error_formulas() {
    for f in [
        "#REF!",
        "=#REF!",
        "#REF!$A$1",
        "#REF!$A$1:$C$1",
        "#ref!A1",
        "#REF!$2:$3",
    ] {
        assert!(is_ref_error_name_formula(f), "{f}");
    }
    for f in [
        "#REF",
        "#REF!+1",
        "SUM(#REF!)",
        "#REF!$A$1:$B$2:$C$3",
        "#REF!$",
        "Sheet1!#REF!",
        "#NAME?",
        "",
    ] {
        assert!(!is_ref_error_name_formula(f), "{f}");
    }
}

#[test]
fn name_to_a_deleted_sheet_gives_ref() {
    let mut m = UserModel::new_empty("t", "en", "UTC", "en").unwrap();
    m.new_defined_name("_On2", None, "#REF!$A$1:$C$1").unwrap();
    m.new_defined_name("Gone", None, "#REF!").unwrap();
    m.set_user_input(0, 1, 1, "=SUM(_On2)").unwrap();
    m.set_user_input(0, 2, 1, "=Gone").unwrap();
    m.set_user_input(0, 3, 1, "=ISREF(Gone)").unwrap();
    assert_eq!(value(&m, 0, 1, 1), "#REF!");
    assert_eq!(value(&m, 0, 2, 1), "#REF!");
    assert_eq!(m.get_cell_content(0, 1, 1).unwrap(), "=SUM(_On2)");
    // The formula text is kept as Excel writes it.
    let names = m.get_defined_name_list();
    assert!(names.contains(&("_On2".to_owned(), None, "#REF!$A$1:$C$1".to_owned())));
    // Inserting rows and deleting another sheet leave it as it is.
    m.insert_rows(0, 1, 1).unwrap();
    m.kantor_insert_sheet("Other", 1).unwrap();
    m.kantor_delete_sheet(1).unwrap();
    let names = m.get_defined_name_list();
    assert!(names.contains(&("_On2".to_owned(), None, "#REF!$A$1:$C$1".to_owned())));
    assert_eq!(value(&m, 0, 2, 1), "#REF!");
    assert_eq!(m.get_cell_content(0, 2, 1).unwrap(), "=SUM(_On2)");
    m.delete_rows(0, 1, 1).unwrap();
    // Editing the name to a real range makes the formula work.
    m.set_user_input(0, 5, 2, "7").unwrap();
    m.update_defined_name("_On2", None, "_On2", None, "Sheet1!$B$5")
        .unwrap();
    assert_eq!(value(&m, 0, 1, 1), "7");
}

#[test]
fn deleting_the_sheet_a_name_points_at_gives_ref() {
    let mut m = UserModel::new_empty("t", "en", "UTC", "en").unwrap();
    m.kantor_insert_sheet("Data", 1).unwrap();
    m.set_user_input(1, 1, 1, "5").unwrap();
    m.new_defined_name("Total", None, "Data!$A$1").unwrap();
    m.set_user_input(0, 1, 1, "=Total*2").unwrap();
    assert_eq!(value(&m, 0, 1, 1), "10");
    m.kantor_delete_sheet(1).unwrap();
    assert_eq!(value(&m, 0, 1, 1), "#REF!");
    let names = m.get_defined_name_list();
    assert_eq!(names[0].2, "#REF!");
}
