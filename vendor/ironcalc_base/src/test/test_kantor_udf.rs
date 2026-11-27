#![allow(clippy::unwrap_used)]
//! KANTOR PATCH (user functions, §37): host functions in formulas, and
//! formulas evaluated without a cell.

use std::sync::Arc;

use crate::kantor_udf::{KantorUserFunctions, KantorValue};
use crate::UserModel;

struct Funcs;

impl KantorUserFunctions for Funcs {
    fn call(&self, name: &str, args: Vec<KantorValue>) -> Option<KantorValue> {
        match name.to_ascii_lowercase().as_str() {
            // Ten percent of the argument.
            "mytax" => Some(match args.first() {
                Some(KantorValue::Number(n)) => KantorValue::Number(n * 0.1),
                Some(KantorValue::Empty) => KantorValue::Number(0.0),
                _ => KantorValue::Error("#VALUE!".to_string()),
            }),
            // The sum of a range's numbers and how many cells it had.
            "cells" => Some(match args.first() {
                Some(KantorValue::Array(rows)) => KantorValue::Text(format!(
                    "{}x{}",
                    rows.len(),
                    rows.first().map_or(0, Vec::len)
                )),
                _ => KantorValue::Text("scalar".to_string()),
            }),
            "fails" => Some(KantorValue::Error("#N/A".to_string())),
            _ => None,
        }
    }
}

fn text(m: &UserModel, cell: (i32, i32)) -> String {
    m.get_formatted_cell_value(0, cell.0, cell.1).unwrap()
}

#[test]
fn user_functions_evaluate_and_follow_their_arguments() {
    let mut m = UserModel::new_empty("t", "en", "UTC", "en").unwrap();
    m.set_user_input(0, 1, 1, "200").unwrap();
    m.set_user_input(0, 1, 2, "=MyTax(A1)").unwrap();
    m.set_user_input(0, 2, 2, "=Cells(A1:C4)").unwrap();
    m.set_user_input(0, 3, 2, "=Fails()").unwrap();
    m.set_user_input(0, 4, 2, "=NoSuch(1)").unwrap();
    assert_eq!(text(&m, (1, 2)), "#NAME?");
    m.kantor_set_user_functions(Some(Arc::new(Funcs)));
    m.evaluate();
    assert_eq!(text(&m, (1, 2)), "20");
    assert_eq!(text(&m, (2, 2)), "4x3");
    assert_eq!(text(&m, (3, 2)), "#N/A");
    assert_eq!(text(&m, (4, 2)), "#NAME?");
    // The formula text stays as written.
    assert_eq!(m.get_cell_content(0, 1, 2).unwrap(), "=MyTax(A1)");
    m.set_user_input(0, 1, 1, "50").unwrap();
    m.evaluate();
    assert_eq!(text(&m, (1, 2)), "5");
    m.kantor_set_user_functions(None);
    m.evaluate();
    assert_eq!(text(&m, (1, 2)), "#NAME?");
}

#[test]
fn formulas_evaluate_without_a_cell() {
    let mut m = UserModel::new_empty("t", "en", "UTC", "en").unwrap();
    m.set_user_input(0, 1, 1, "3").unwrap();
    m.set_user_input(0, 2, 1, "4").unwrap();
    m.set_user_input(0, 3, 1, "x").unwrap();
    assert_eq!(
        m.kantor_evaluate_formula(0, 1, 1, "=SUM(A1:A3)").unwrap(),
        KantorValue::Number(7.0)
    );
    assert_eq!(
        m.kantor_evaluate_formula(0, 1, 1, "A1:A2").unwrap(),
        KantorValue::Array(vec![
            vec![KantorValue::Number(3.0)],
            vec![KantorValue::Number(4.0)]
        ])
    );
    assert_eq!(
        m.kantor_evaluate_formula(0, 1, 1, "1/0").unwrap(),
        KantorValue::Error("#DIV/0!".to_string())
    );
    assert!(m.kantor_evaluate_formula(0, 1, 1, "SUM(").is_err());
    assert_eq!(
        m.kantor_cell_value(0, 3, 1),
        KantorValue::Text("x".to_string())
    );
    assert_eq!(m.kantor_cell_value(0, 9, 9), KantorValue::Empty);
}
