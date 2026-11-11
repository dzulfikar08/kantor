#![allow(clippy::unwrap_used)]
//! KANTOR PATCH (tables): structured references, tables following inserted
//! and deleted rows, sorted rows and filtered rows.

use crate::expressions::parser::stringify::to_excel_string;
use crate::expressions::types::CellReferenceRC;
use crate::types::{Table, TableColumn, TableStyleInfo};
use crate::UserModel;

fn table(name: &str, reference: &str, columns: &[&str], totals: u32) -> Table {
    Table {
        name: name.to_string(),
        display_name: name.to_string(),
        sheet_name: "Sheet1".to_string(),
        reference: reference.to_string(),
        totals_row_count: totals,
        header_row_count: 1,
        header_row_dxf_id: None,
        data_dxf_id: None,
        totals_row_dxf_id: None,
        columns: columns
            .iter()
            .enumerate()
            .map(|(i, n)| TableColumn {
                id: i as u32 + 1,
                name: n.to_string(),
                ..Default::default()
            })
            .collect(),
        style_info: TableStyleInfo::default(),
        has_filters: true,
        kantor_auto_filter: None,
    }
}

/// Sales (A1:C5): Region, Units, Unit Price; a total row in row 5.
fn sales() -> UserModel<'static> {
    let mut m = UserModel::new_empty("t", "en", "UTC", "en").unwrap();
    let rows = [
        ["Region", "Units", "Unit Price"],
        ["North", "10", "2"],
        ["South", "20", "3"],
        ["East", "5", "4"],
    ];
    for (r, row) in rows.iter().enumerate() {
        for (c, v) in row.iter().enumerate() {
            m.set_user_input(0, r as i32 + 1, c as i32 + 1, v).unwrap();
        }
    }
    m.kantor_set_table(
        "Sales",
        Some(table("Sales", "A1:C5", &["Region", "Units", "Unit Price"], 1)),
    )
    .unwrap();
    m.set_user_input(0, 5, 2, "=SUBTOTAL(109,Sales[Units])")
        .unwrap();
    m
}

fn text(m: &UserModel, cell: (i32, i32)) -> String {
    m.get_formatted_cell_value(0, cell.0, cell.1).unwrap()
}

#[test]
fn structured_references_evaluate_and_keep_their_text() {
    let mut m = sales();
    for r in 2..=4 {
        m.set_user_input(0, r, 4, "=Sales[@Units]*Sales[@[Unit Price]]")
            .unwrap();
    }
    m.set_user_input(0, 7, 1, "=SUM(Sales[Units])").unwrap();
    m.set_user_input(0, 8, 1, "=Sales[[#Totals],[Units]]").unwrap();
    m.set_user_input(0, 9, 1, "=ROWS(Sales[#All])").unwrap();
    m.set_user_input(0, 10, 1, "=COUNTA(Sales[#Headers])").unwrap();
    m.set_user_input(0, 11, 1, "=ROWS(sales[[#headers],[#data]])").unwrap();
    m.set_user_input(0, 12, 1, "=SUM(Sales[[Units]:[Unit Price]])").unwrap();
    m.set_user_input(0, 13, 1, "=Sales[@Units]").unwrap();
    m.set_user_input(0, 14, 1, "=SUM(Sales[[#This Row],[Units]])").unwrap();
    assert_eq!(text(&m, (2, 4)), "20");
    assert_eq!(text(&m, (3, 4)), "60");
    assert_eq!(text(&m, (4, 4)), "20");
    assert_eq!(text(&m, (5, 2)), "35");
    assert_eq!(text(&m, (7, 1)), "35");
    assert_eq!(text(&m, (8, 1)), "35");
    assert_eq!(text(&m, (9, 1)), "5");
    assert_eq!(text(&m, (10, 1)), "3");
    assert_eq!(text(&m, (11, 1)), "4");
    assert_eq!(text(&m, (12, 1)), "44");
    // Outside the table's rows, `@` is #VALUE! as in Excel.
    assert_eq!(text(&m, (13, 1)), "#VALUE!");
    assert_eq!(
        m.get_cell_content(0, 2, 4).unwrap(),
        "=Sales[@Units]*Sales[@[Unit Price]]"
    );
    assert_eq!(m.get_cell_content(0, 14, 1).unwrap(), "=SUM(Sales[@Units])");
    assert_eq!(m.get_cell_content(0, 12, 1).unwrap(), "=SUM(Sales[[Units]:[Unit Price]])");
    // One shared formula for the three rows.
    let model = m.get_model();
    assert_eq!(
        model.workbook.worksheets[0]
            .shared_formulas
            .iter()
            .filter(|f| f.contains("[@Units]*"))
            .count(),
        1
    );
    // Files get the `[#This Row]` form.
    let node = &model.parsed_formulas[0][0].0;
    let context = CellReferenceRC {
        sheet: "Sheet1".to_string(),
        row: 5,
        column: 2,
    };
    assert_eq!(to_excel_string(node, &context), "SUBTOTAL(109,Sales[Units])");
    let parsed = {
        let mut p = model.parser.clone();
        p.parse("Sales[@[Unit Price]]*Sales[[#Totals],[Units]]", &context)
    };
    assert_eq!(
        to_excel_string(&parsed, &context),
        "Sales[[#This Row],[Unit Price]]*Sales[[#Totals],[Units]]"
    );
}

#[test]
fn references_follow_the_table_as_it_grows() {
    let mut m = sales();
    m.set_user_input(0, 8, 1, "=SUM(Sales[Units])").unwrap();
    // Insert a row inside the table: it grows.
    m.insert_rows(0, 3, 1).unwrap();
    assert_eq!(m.get_model().workbook.tables["Sales"].reference, "A1:C6");
    m.set_user_input(0, 3, 2, "100").unwrap();
    assert_eq!(text(&m, (9, 1)), "135");
    // The total row moved down with the table.
    assert_eq!(text(&m, (6, 2)), "135");
    m.undo().unwrap(); // the 100
    m.undo().unwrap(); // the inserted row
    assert_eq!(m.get_model().workbook.tables["Sales"].reference, "A1:C5");
    // Deleting the last data row shrinks it; undo restores it exactly.
    m.delete_rows(0, 4, 1).unwrap();
    assert_eq!(m.get_model().workbook.tables["Sales"].reference, "A1:C4");
    assert_eq!(text(&m, (7, 1)), "30");
    m.undo().unwrap();
    assert_eq!(m.get_model().workbook.tables["Sales"].reference, "A1:C5");
    assert_eq!(text(&m, (8, 1)), "35");
    // Rows inserted above move it; columns inserted inside add columns.
    m.insert_rows(0, 1, 2).unwrap();
    assert_eq!(m.get_model().workbook.tables["Sales"].reference, "A3:C7");
    m.insert_columns(0, 2, 1).unwrap();
    let t = &m.get_model().workbook.tables["Sales"];
    assert_eq!(t.reference, "A3:D7");
    let names: Vec<&str> = t.columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Region", "Column1", "Units", "Unit Price"]);
    // Deleting the header row removes the table: its references are #REF!.
    m.delete_rows(0, 3, 1).unwrap();
    assert!(!m.get_model().workbook.tables.contains_key("Sales"));
}

#[test]
fn permuted_rows_take_their_formulas_along_and_undo() {
    let mut m = UserModel::new_empty("t", "en", "UTC", "en").unwrap();
    for (r, (a, b)) in [("c", 3), ("a", 1), ("b", 2)].iter().enumerate() {
        let r = r as i32 + 1;
        m.set_user_input(0, r, 1, a).unwrap();
        m.set_user_input(0, r, 2, &b.to_string()).unwrap();
        m.set_user_input(0, r, 3, &format!("=B{r}*10")).unwrap();
        m.set_user_input(0, r, 4, "=$B$1").unwrap();
    }
    m.set_user_input(0, 1, 5, "outside").unwrap();
    m.kantor_permute_rows(0, 1, 1, 4, &[2, 3, 1]).unwrap();
    let col = |m: &UserModel, c: i32| (1..=3).map(|r| text(m, (r, c))).collect::<Vec<_>>();
    assert_eq!(col(&m, 1), ["a", "b", "c"]);
    assert_eq!(col(&m, 3), ["10", "20", "30"]);
    // Absolute references stay where they pointed.
    assert_eq!(col(&m, 4), ["1", "1", "1"]);
    assert_eq!(m.get_cell_content(0, 1, 3).unwrap(), "=B1*10");
    assert_eq!(text(&m, (1, 5)), "outside");
    m.undo().unwrap();
    assert_eq!(col(&m, 1), ["c", "a", "b"]);
    assert_eq!(col(&m, 3), ["30", "10", "20"]);
    m.redo().unwrap();
    assert_eq!(col(&m, 1), ["a", "b", "c"]);
    assert!(m.kantor_permute_rows(0, 1, 1, 2, &[1, 1, 3]).is_err());
}

#[test]
fn filtered_rows_are_skipped_by_subtotal() {
    let mut m = sales();
    m.set_user_input(0, 7, 1, "=SUBTOTAL(9,Sales[Units])").unwrap();
    m.set_user_input(0, 8, 1, "=SUBTOTAL(109,Sales[Units])").unwrap();
    m.set_user_input(0, 9, 1, "=AGGREGATE(9,5,Sales[Units])").unwrap();
    m.set_user_input(0, 10, 1, "=SUM(B2:B4)").unwrap();
    // Not under a filter: hidden by hand, SUBTOTAL(9) still counts it.
    let mut t = m.get_model().workbook.tables["Sales"].clone();
    t.has_filters = false;
    m.kantor_set_table("Sales", Some(t)).unwrap();
    assert!(m.kantor_set_rows_hidden(0, &[(3, true)]).unwrap());
    assert_eq!(text(&m, (7, 1)), "35");
    assert_eq!(text(&m, (8, 1)), "15");
    // Under an AutoFilter it is filtered out for both.
    m.kantor_set_sheet_filter(
        0,
        Some(r#"<autoFilter ref="A1:C4"><filterColumn colId="0"/></autoFilter>"#.to_string()),
    )
    .unwrap();
    assert_eq!(text(&m, (7, 1)), "15");
    assert_eq!(text(&m, (8, 1)), "15");
    assert_eq!(text(&m, (9, 1)), "15");
    assert_eq!(text(&m, (10, 1)), "35");
    // Undo shows the row again, as it was.
    m.undo().unwrap();
    m.undo().unwrap();
    assert!(!m.get_model().workbook.worksheets[0].rows.iter().any(|r| r.hidden));
    assert!(!m.kantor_set_rows_hidden(0, &[(3, false)]).unwrap());
}

#[test]
fn the_sheet_filter_follows_rows_and_undoes() {
    let mut m = UserModel::new_empty("t", "en", "UTC", "en").unwrap();
    let xml = r#"<autoFilter ref="A1:C10"><filterColumn colId="1"><filters><filter val="x"/></filters></filterColumn></autoFilter>"#;
    m.kantor_set_sheet_filter(0, Some(xml.to_string())).unwrap();
    let filter = |m: &UserModel| m.get_model().workbook.worksheets[0].kantor_auto_filter.clone();
    m.insert_rows(0, 5, 2).unwrap();
    assert!(filter(&m).unwrap().starts_with(r#"<autoFilter ref="A1:C12">"#));
    m.delete_rows(0, 11, 2).unwrap();
    assert!(filter(&m).unwrap().starts_with(r#"<autoFilter ref="A1:C10">"#));
    m.undo().unwrap();
    assert!(filter(&m).unwrap().starts_with(r#"<autoFilter ref="A1:C12">"#));
    m.undo().unwrap();
    m.undo().unwrap();
    assert_eq!(filter(&m), None);
}

#[test]
fn unqualified_references_inside_a_table() {
    // As XlsxWriter writes them: `[[#This Row],Units]`, `[Units]`.
    let mut m = sales();
    let mut t = m.get_model().workbook.tables["Sales"].clone();
    t.reference = "A1:D5".to_string();
    t.columns.push(TableColumn {
        id: 4,
        name: "Amount".to_string(),
        ..Default::default()
    });
    m.kantor_set_table("Sales", Some(t)).unwrap();
    m.set_user_input(0, 2, 4, "=[@Units]+[[#This Row],Unit Price]").unwrap();
    m.set_user_input(0, 5, 4, "=SUBTOTAL(109,[Units])").unwrap();
    assert_eq!(text(&m, (2, 4)), "12");
    assert_eq!(text(&m, (5, 4)), "35");
    assert_eq!(
        m.get_cell_content(0, 2, 4).unwrap(),
        "=Sales[@Units]+Sales[@[Unit Price]]"
    );
    // Outside a table it is no reference.
    assert!(m.set_user_input(0, 9, 9, "=[@Units]").is_ok());
    assert_eq!(text(&m, (9, 9)), "#ERROR!");
}
