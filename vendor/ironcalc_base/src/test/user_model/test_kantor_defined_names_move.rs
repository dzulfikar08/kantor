// KANTOR PATCH (fidelity): defined names follow rows and columns.
#![allow(clippy::unwrap_used)]

use crate::test::user_model::util::new_empty_user_model;

fn names(model: &crate::UserModel) -> Vec<String> {
    model
        .get_defined_name_list()
        .into_iter()
        .map(|(n, _, f)| format!("{n}={f}"))
        .collect()
}

#[test]
fn names_follow_inserted_and_deleted_rows() {
    let mut model = new_empty_user_model();
    model.set_user_input(0, 6, 1, "7").unwrap();
    model
        .new_defined_name("Rng", None, "Sheet1!$A$5:$A$10")
        .unwrap();
    model.new_defined_name("Top", Some(0), "Sheet1!$B$1").unwrap();
    model.set_user_input(0, 1, 3, "=SUM(Rng)").unwrap();

    model.insert_rows(0, 3, 2).unwrap();
    assert_eq!(names(&model), ["Rng=Sheet1!$A$7:$A$12", "Top=Sheet1!$B$1"]);
    assert_eq!(model.get_formatted_cell_value(0, 1, 3).unwrap(), "7");
    model.undo().unwrap();
    assert_eq!(names(&model), ["Rng=Sheet1!$A$5:$A$10", "Top=Sheet1!$B$1"]);
    model.redo().unwrap();
    assert_eq!(names(&model), ["Rng=Sheet1!$A$7:$A$12", "Top=Sheet1!$B$1"]);

    model.insert_columns(0, 1, 1).unwrap();
    assert_eq!(names(&model), ["Rng=Sheet1!$B$7:$B$12", "Top=Sheet1!$C$1"]);
}

#[test]
fn deleted_cells_make_ref_errors_that_undo_restores() {
    let mut model = new_empty_user_model();
    model.new_defined_name("One", None, "Sheet1!$A$3").unwrap();
    model.new_defined_name("Below", None, "Sheet1!$A$9").unwrap();
    model.delete_rows(0, 2, 2).unwrap();
    assert_eq!(names(&model), ["One=#REF!", "Below=Sheet1!$A$7"]);
    model.undo().unwrap();
    assert_eq!(names(&model), ["One=Sheet1!$A$3", "Below=Sheet1!$A$9"]);
    model.redo().unwrap();
    assert_eq!(names(&model), ["One=#REF!", "Below=Sheet1!$A$7"]);
    model.undo().unwrap();

    model.delete_columns(0, 1, 1).unwrap();
    assert_eq!(names(&model), ["One=#REF!", "Below=#REF!"]);
    model.undo().unwrap();
    assert_eq!(names(&model), ["One=Sheet1!$A$3", "Below=Sheet1!$A$9"]);
}
