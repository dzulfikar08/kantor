#![allow(clippy::unwrap_used)]
//! KANTOR PATCH (pivot): GETPIVOTDATA over the value cells the caller
//! describes in `Model::kantor_pivots`.

use crate::functions::kantor_pivot::{PivotGrid, PivotValueCell};
use crate::test::util::new_empty_model;

fn cell(data: &str, items: &[(&str, &str)], row: i32, column: i32) -> PivotValueCell {
    let mut items: Vec<(String, String)> = items
        .iter()
        .map(|(f, i)| (f.to_string(), i.to_string()))
        .collect();
    items.sort();
    PivotValueCell {
        data: vec![format!("sum of {data}"), data.to_string()],
        items,
        row,
        column,
    }
}

#[test]
fn getpivotdata_finds_cells_by_field_and_item() {
    let mut model = new_empty_model();
    // A1:B4: Row Labels | Sum of Units; East 3; West 4; Grand Total 7.
    model._set("A1", "Row Labels");
    model._set("B1", "Sum of Units");
    model._set("A2", "East");
    model._set("B2", "3");
    model._set("A3", "West");
    model._set("B3", "4");
    model._set("A4", "Grand Total");
    model._set("B4", "7");
    model.kantor_pivots = vec![PivotGrid {
        sheet: 0,
        top: 1,
        left: 1,
        bottom: 4,
        right: 2,
        cells: vec![
            cell("units", &[("region", "east")], 2, 2),
            cell("units", &[("region", "west")], 3, 2),
            cell("units", &[], 4, 2),
        ],
    }];
    model._set("D1", r#"=GETPIVOTDATA("Sum of Units",A1,"Region","West")"#);
    model._set("D2", r#"=GETPIVOTDATA("units",$B$4)"#);
    model._set("D3", r#"=GETPIVOTDATA("Units",A1,"Region","North")"#);
    model._set("D4", r#"=GETPIVOTDATA("Units",F9)"#);
    model._set("D5", r#"=GETPIVOTDATA("Units")"#);
    model._set("D6", r#"=GETPIVOTDATA("Units",A1,"Region")"#);
    model.evaluate();
    assert_eq!(model._get_text("D1"), "4");
    assert_eq!(model._get_text("D2"), "7");
    assert_eq!(model._get_text("D3"), "#REF!");
    assert_eq!(model._get_text("D4"), "#REF!");
    assert_eq!(model._get_text("D5"), "#ERROR!");
    assert_eq!(model._get_text("D6"), "#ERROR!");
    // Saved without a prefix.
    assert_eq!(model._get_formula("D2"), r#"=GETPIVOTDATA("units",$B$4)"#);
}
