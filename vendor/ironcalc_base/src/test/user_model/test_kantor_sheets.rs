// KANTOR PATCH (sheet tabs): Excel's Delete and Move or Copy.
#![allow(clippy::unwrap_used)]

use crate::test::user_model::util::new_empty_user_model;

fn names(model: &crate::UserModel) -> Vec<String> {
    model
        .get_defined_name_list()
        .into_iter()
        .map(|(n, s, f)| format!("{n}@{s:?}={f}"))
        .collect()
}

fn sheets(model: &crate::UserModel) -> Vec<String> {
    model
        .get_worksheets_properties()
        .into_iter()
        .map(|p| p.name)
        .collect()
}

#[test]
fn deleting_a_sheet_turns_its_references_into_ref_errors() {
    let mut model = new_empty_user_model();
    model.new_sheet().unwrap(); // Sheet2
    model.set_user_input(1, 1, 1, "5").unwrap();
    model.set_user_input(0, 1, 1, "=Sheet2!A1*2").unwrap();
    model.set_user_input(0, 2, 1, "=SUM(Sheet2!A1:B3)+1").unwrap();
    model.new_defined_name("Rate", None, "Sheet2!$A$1").unwrap();
    model.new_defined_name("Own", Some(1), "Sheet2!$B$1").unwrap();
    model.set_user_input(1, 3, 3, "note me").unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "10");

    model.kantor_delete_sheet(1).unwrap();
    assert_eq!(sheets(&model), ["Sheet1"]);
    assert_eq!(model.get_cell_content(0, 1, 1).unwrap(), "=#REF!*2");
    assert_eq!(model.get_cell_content(0, 2, 1).unwrap(), "=SUM(#REF!)+1");
    assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "#REF!");
    // The sheet's own name is gone, the global one points nowhere but
    // keeps its cell part, as Excel writes it (§34).
    assert_eq!(names(&model), ["Rate@None=#REF!$A$1"]);
    // A new sheet with the old name doesn't pick the references up.
    model.kantor_insert_sheet("Sheet2", 1).unwrap();
    model.set_user_input(1, 1, 1, "7").unwrap();
    assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "#REF!");
    model.undo().unwrap();
    model.undo().unwrap();

    // Undo puts the sheet, the formulas and the names back.
    model.undo().unwrap();
    assert_eq!(sheets(&model), ["Sheet1", "Sheet2"]);
    assert_eq!(model.get_cell_content(0, 1, 1).unwrap(), "=Sheet2!A1*2");
    assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "10");
    assert_eq!(model.get_formatted_cell_value(1, 3, 3).unwrap(), "note me");
    assert_eq!(names(&model).len(), 2);
    model.redo().unwrap();
    assert_eq!(sheets(&model), ["Sheet1"]);
    assert_eq!(model.get_cell_content(0, 1, 1).unwrap(), "=#REF!*2");
}

#[test]
fn a_copy_has_its_name_and_place() {
    let mut model = new_empty_user_model();
    model.new_sheet().unwrap(); // Sheet2
    model.set_user_input(0, 1, 1, "3").unwrap();
    model.set_user_input(0, 1, 2, "=A1+Sheet1!A1").unwrap();
    model.new_defined_name("Local", Some(0), "Sheet1!$A$1").unwrap();

    // Before Sheet2 is index 1.
    model.kantor_copy_sheet(0, "Copy", 1).unwrap();
    assert_eq!(sheets(&model), ["Sheet1", "Copy", "Sheet2"]);
    assert_eq!(model.get_selected_sheet(), 1);
    assert_eq!(model.get_cell_content(1, 1, 2).unwrap(), "=A1+Copy!A1");
    assert_eq!(model.get_formatted_cell_value(1, 1, 2).unwrap(), "6");
    assert!(names(&model).contains(&"Local@Some(1)=Copy!$A$1".to_string()));
    // Taken names are refused.
    assert!(model.kantor_copy_sheet(0, "copy", 0).is_err());

    model.undo().unwrap();
    assert_eq!(sheets(&model), ["Sheet1", "Sheet2"]);
    assert_eq!(names(&model), ["Local@Some(0)=Sheet1!$A$1"]);
    model.redo().unwrap();
    assert_eq!(sheets(&model), ["Sheet1", "Copy", "Sheet2"]);
    assert_eq!(model.get_cell_content(1, 1, 2).unwrap(), "=A1+Copy!A1");

    // At the end, and at the front (before its source).
    model.kantor_copy_sheet(2, "Last", 3).unwrap();
    assert_eq!(sheets(&model), ["Sheet1", "Copy", "Sheet2", "Last"]);
    model.kantor_copy_sheet(0, "First", 0).unwrap();
    assert_eq!(sheets(&model), ["First", "Sheet1", "Copy", "Sheet2", "Last"]);
    model.undo().unwrap();
    assert_eq!(model.get_selected_sheet(), 0);
    assert_eq!(sheets(&model), ["Sheet1", "Copy", "Sheet2", "Last"]);
}
