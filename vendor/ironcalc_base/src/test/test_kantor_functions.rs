#![allow(clippy::unwrap_used)]
//! KANTOR PATCH (functions): AGGREGATE, GROUPBY, PIVOTBY, BAHTTEXT, USDOLLAR,
//! DBCS/JIS, PHONETIC, SUBTOTAL fixes and functions passed by name.
//! Expected values are Excel's (documentation examples where there are some).

use crate::test::util::new_empty_model;
use crate::Model;

fn set_column(model: &mut Model, column: &str, first_row: i32, values: &[&str]) {
    for (i, v) in values.iter().enumerate() {
        model._set(&format!("{column}{}", first_row + i as i32), v);
    }
}

/// The texts of `row`'s cells from column `from` to `to` (letters).
fn row_texts(model: &Model, row: i32, from: char, to: char) -> Vec<String> {
    (from..=to)
        .map(|c| model._get_text(&format!("{c}{row}")))
        .collect()
}

// ── AGGREGATE ────────────────────────────────────────────────────────────────

/// The data of Microsoft's AGGREGATE example.
fn aggregate_example() -> Model<'static> {
    let mut model = new_empty_model();
    set_column(
        &mut model,
        "A",
        1,
        &[
            "=1/0",
            "72",
            "30",
            "=SQRT(-1)",
            "31",
            "96",
            "32",
            "81",
            "33",
            "53",
            "34",
        ],
    );
    set_column(
        &mut model,
        "B",
        1,
        &[
            "82", "65", "95", "63", "53", "71", "55", "83", "100", "91", "89",
        ],
    );
    model
}

#[test]
fn aggregate_documentation_example() {
    let mut model = aggregate_example();
    model._set("D1", "=AGGREGATE(4, 6, A1:A11)");
    model._set("D2", "=AGGREGATE(14, 6, A1:A11, 3)");
    model._set("D3", "=AGGREGATE(15, 6, A1:A11)");
    model._set("D4", "=AGGREGATE(12, 6, A1:A11, B1:B11)");
    model._set("D5", "=MAX(A1:A2)");
    model._set("D6", "=AGGREGATE(4, 4, A1:A11)");
    model.evaluate();
    assert_eq!(model._get_text("D1"), "96");
    assert_eq!(model._get_text("D2"), "72");
    assert_eq!(model._get_text("D3"), "#VALUE!");
    assert_eq!(model._get_text("D4"), "68");
    assert_eq!(model._get_text("D5"), "#DIV/0!");
    assert_eq!(model._get_text("D6"), "#DIV/0!");
}

#[test]
fn aggregate_functions_1_to_13() {
    let mut model = new_empty_model();
    // A6 stays empty.
    set_column(&mut model, "A", 1, &["1", "2", "2", "text", "TRUE"]);
    model._set("A7", "5");
    let expected = [
        (1, "2.5"),
        (2, "4"),
        (3, "6"),
        (4, "5"),
        (5, "1"),
        (6, "20"),
        (7, "1.732050808"),
        (8, "1.5"),
        (9, "10"),
        (10, "3"),
        (11, "2.25"),
        (12, "2"),
        (13, "2"),
    ];
    for (i, (function, _)) in expected.iter().enumerate() {
        model._set(
            &format!("C{}", i + 1),
            &format!("=AGGREGATE({function},,A1:A7)"),
        );
    }
    model.evaluate();
    for (i, (function, value)) in expected.iter().enumerate() {
        assert_eq!(
            model._get_text(&format!("C{}", i + 1)),
            *value,
            "function {function}"
        );
    }
}

#[test]
fn aggregate_functions_14_to_19() {
    let mut model = new_empty_model();
    set_column(&mut model, "A", 1, &["4", "1", "3", "2"]);
    model._set("C1", "=AGGREGATE(14,0,A1:A4,2)");
    model._set("C2", "=AGGREGATE(15,0,A1:A4,2)");
    model._set("C3", "=AGGREGATE(16,0,A1:A4,0.3)");
    model._set("C4", "=AGGREGATE(17,0,A1:A4,1)");
    model._set("C5", "=AGGREGATE(18,0,A1:A4,0.3)");
    model._set("C6", "=AGGREGATE(19,0,A1:A4,1)");
    model._set("C7", "=AGGREGATE(14,0,A1:A4,5)");
    model._set("C8", "=AGGREGATE(18,0,A1:A4,0.1)");
    model._set("C9", "=AGGREGATE(19,0,A1:A4,4)");
    model._set("C10", "=AGGREGATE(17,0,A1:A4,4)");
    model.evaluate();
    assert_eq!(model._get_text("C1"), "3");
    assert_eq!(model._get_text("C2"), "2");
    assert_eq!(model._get_text("C3"), "1.9");
    assert_eq!(model._get_text("C4"), "1.75");
    assert_eq!(model._get_text("C5"), "1.5");
    assert_eq!(model._get_text("C6"), "1.25");
    assert_eq!(model._get_text("C7"), "#NUM!");
    assert_eq!(model._get_text("C8"), "#NUM!");
    assert_eq!(model._get_text("C9"), "#NUM!");
    assert_eq!(model._get_text("C10"), "4");
}

#[test]
fn aggregate_array_form_ignores_errors() {
    let mut model = new_empty_model();
    set_column(&mut model, "A", 1, &["10", "20", "30"]);
    set_column(&mut model, "B", 1, &["1", "0", "1"]);
    // The classic "largest where" formula: dividing by FALSE gives #DIV/0!.
    model._set("C1", "=AGGREGATE(14,6,A1:A3/(B1:B3=1),1)");
    model._set("C2", "=AGGREGATE(15,6,A1:A3/(B1:B3=1),1)");
    model._set("C3", "=AGGREGATE(14,4,A1:A3/(B1:B3=1),1)");
    // Functions 1–13 want references.
    model._set("C4", "=AGGREGATE(9,0,{1,2})");
    model.evaluate();
    assert_eq!(model._get_text("C1"), "30");
    assert_eq!(model._get_text("C2"), "10");
    assert_eq!(model._get_text("C3"), "#DIV/0!");
    assert_eq!(model._get_text("C4"), "#VALUE!");
}

#[test]
fn aggregate_hidden_rows_and_nested_subtotals() {
    let mut model = new_empty_model();
    set_column(
        &mut model,
        "A",
        1,
        &[
            "1",
            "2",
            "4",
            "=SUBTOTAL(9,A1:A3)",
            "=AGGREGATE(9,0,A1:A2)*10",
        ],
    );
    model._set("C1", "=AGGREGATE(9,0,A1:A5)");
    model._set("C2", "=AGGREGATE(9,4,A1:A5)");
    model._set("C3", "=AGGREGATE(9,5,A1:A5)");
    model._set("C4", "=AGGREGATE(9,1,A1:A5)");
    model._set("C5", "=SUBTOTAL(9,A1:A5)");
    model.set_row_hidden(0, 2, true).unwrap();
    model.evaluate();
    assert_eq!(model._get_text("A4"), "7");
    assert_eq!(model._get_text("A5"), "30");
    // Nested SUBTOTAL/AGGREGATE cells skipped, hidden row counted.
    assert_eq!(model._get_text("C1"), "7");
    // Nothing ignored.
    assert_eq!(model._get_text("C2"), "44");
    // Hidden row ignored, nested counted: 1 + 4 + 7 + 30.
    assert_eq!(model._get_text("C3"), "42");
    assert_eq!(model._get_text("C4"), "5");
    // SUBTOTAL skips a cell with AGGREGATE inside an expression too.
    assert_eq!(model._get_text("C5"), "7");
}

#[test]
fn aggregate_wrong_arguments() {
    let mut model = new_empty_model();
    model._set("A1", "5");
    model._set("C1", "=AGGREGATE(20,0,A1)");
    model._set("C2", "=AGGREGATE(0,0,A1)");
    model._set("C3", "=AGGREGATE(9,8,A1)");
    model._set("C4", "=AGGREGATE(9,0)");
    model._set("C5", "=AGGREGATE(\"x\",0,A1)");
    model._set("C6", "=AGGREGATE(9,0,A:A)");
    model._set("C7", "=AGGREGATE(1,0,B1:B3)");
    model.evaluate();
    assert_eq!(model._get_text("C1"), "#VALUE!");
    assert_eq!(model._get_text("C2"), "#VALUE!");
    assert_eq!(model._get_text("C3"), "#VALUE!");
    assert_eq!(model._get_text("C4"), "#ERROR!");
    assert_eq!(model._get_text("C5"), "#VALUE!");
    assert_eq!(model._get_text("C6"), "5");
    assert_eq!(model._get_text("C7"), "#DIV/0!");
}

// ── SUBTOTAL fixes ───────────────────────────────────────────────────────────

#[test]
fn subtotal_110_and_111_skip_hidden_rows() {
    let mut model = new_empty_model();
    set_column(&mut model, "A", 1, &["1", "2", "3", "100"]);
    model._set("C1", "=SUBTOTAL(110,A1:A4)");
    model._set("C2", "=SUBTOTAL(111,A1:A4)");
    model._set("C3", "=SUBTOTAL(10,A1:A4)");
    model.set_row_hidden(0, 4, true).unwrap();
    model.evaluate();
    assert_eq!(model._get_text("C1"), "1");
    assert_eq!(model._get_text("C2"), "0.666666667");
    assert_eq!(model._get_text("C3"), "2401.666666667");
}

// ── BAHTTEXT, USDOLLAR, DBCS/JIS, PHONETIC ───────────────────────────────────

#[test]
fn bahttext() {
    let cases = [
        ("1234", "หนึ่งพันสองร้อยสามสิบสี่บาทถ้วน"),
        ("0", "ศูนย์บาทถ้วน"),
        ("1", "หนึ่งบาทถ้วน"),
        ("11", "สิบเอ็ดบาทถ้วน"),
        ("21", "ยี่สิบเอ็ดบาทถ้วน"),
        ("101", "หนึ่งร้อยหนึ่งบาทถ้วน"),
        ("0.5", "ห้าสิบสตางค์"),
        ("1.25", "หนึ่งบาทยี่สิบห้าสตางค์"),
        ("-10", "ลบสิบบาทถ้วน"),
        ("1000000", "หนึ่งล้านบาทถ้วน"),
        ("2500001", "สองล้านห้าแสนหนึ่งบาทถ้วน"),
        ("21000000", "ยี่สิบเอ็ดล้านบาทถ้วน"),
        ("0.005", "หนึ่งสตางค์"),
        ("\"12\"", "สิบสองบาทถ้วน"),
        ("\"x\"", "#VALUE!"),
    ];
    let mut model = new_empty_model();
    for (i, (input, _)) in cases.iter().enumerate() {
        model._set(&format!("A{}", i + 1), &format!("=BAHTTEXT({input})"));
    }
    model._set("B1", "=BAHTTEXT(C1)");
    model.evaluate();
    for (i, (input, expected)) in cases.iter().enumerate() {
        assert_eq!(
            model._get_text(&format!("A{}", i + 1)),
            *expected,
            "{input}"
        );
    }
    // An empty cell is 0.
    assert_eq!(model._get_text("B1"), "ศูนย์บาทถ้วน");
}

#[test]
fn usdollar_dbcs_jis_phonetic() {
    let mut model = new_empty_model();
    model._set("A1", "1234.567");
    model._set("A2", "ab 12");
    model._set("B1", "=USDOLLAR(A1,2)");
    model._set("B2", "=USDOLLAR(-0.5)");
    model._set("B3", "=DBCS(A2)");
    model._set("B4", "=JIS(\"A!\")");
    model._set("B5", "=ASC(DBCS(A2))");
    model._set("B6", "=PHONETIC(A2)");
    model._set("B7", "=PHONETIC(A1)");
    model._set("B8", "=PHONETIC(A2:A3)");
    model.evaluate();
    assert_eq!(model._get_text("B1"), "$1,234.57");
    assert_eq!(model._get_text("B2"), "($0.50)");
    assert_eq!(model._get_text("B3"), "ａｂ\u{3000}１２");
    assert_eq!(model._get_text("B4"), "Ａ！");
    assert_eq!(model._get_text("B5"), "ab 12");
    assert_eq!(model._get_text("B6"), "ab 12");
    assert_eq!(model._get_text("B7"), "");
    assert_eq!(model._get_text("B8"), "ab 12");
}

// ── Functions passed by name ─────────────────────────────────────────────────

#[test]
fn functions_passed_by_name() {
    let mut model = new_empty_model();
    set_column(&mut model, "A", 1, &["1", "-2"]);
    set_column(&mut model, "B", 1, &["3", "4"]);
    model._set("D1", "=BYROW(A1:B2,SUM)");
    model._set("E1", "=BYCOL(A1:B2,MAX)");
    model._set("E3", "=MAP(A1:B2,ABS)");
    model._set("E6", "=REDUCE(0,A1:B2,SUM)");
    model._set("E7", "=SUM");
    model._set("E8", "=COUNT({1,\"a\",2})");
    model.evaluate();
    assert_eq!(model._get_text("D1"), "4");
    assert_eq!(model._get_text("D2"), "2");
    assert_eq!(model._get_text("E1"), "1");
    assert_eq!(model._get_text("F1"), "4");
    assert_eq!(row_texts(&model, 4, 'E', 'F'), ["2", "4"]);
    assert_eq!(model._get_text("E6"), "6");
    // On its own the name is not a value.
    assert_eq!(model._get_text("E7"), "#NAME?");
    // COUNT counts the numbers of an array (upstream counted none).
    assert_eq!(model._get_text("E8"), "2");
}

// ── GROUPBY ──────────────────────────────────────────────────────────────────

/// Region and Amount with headers in A1:B7.
fn sales() -> Model<'static> {
    let mut model = new_empty_model();
    set_column(
        &mut model,
        "A",
        1,
        &["Region", "East", "West", "east", "North", "West", "East"],
    );
    set_column(
        &mut model,
        "B",
        1,
        &["Amount", "10", "20", "5", "7", "3", "1"],
    );
    model
}

#[test]
fn groupby_sum_with_detected_headers() {
    let mut model = sales();
    model._set("D1", "=GROUPBY(A1:A7,B1:B7,SUM)");
    model.evaluate();
    // Headers detected (text over a number) but not shown: one level only.
    assert_eq!(row_texts(&model, 1, 'D', 'E'), ["East", "16"]);
    assert_eq!(row_texts(&model, 2, 'D', 'E'), ["North", "7"]);
    assert_eq!(row_texts(&model, 3, 'D', 'E'), ["West", "23"]);
    assert_eq!(row_texts(&model, 4, 'D', 'E'), ["Total", "46"]);
    assert_eq!(row_texts(&model, 5, 'D', 'E'), ["", ""]);
}

#[test]
fn groupby_options() {
    let mut model = sales();
    // Headers shown, sorted by the amount, descending.
    model._set("D1", "=GROUPBY(A1:A7,B1:B7,SUM,3,1,-2)");
    // No totals, AVERAGE.
    model._set("G1", "=GROUPBY(A2:A7,B2:B7,AVERAGE,0,0)");
    // Totals at the top, COUNT.
    model._set("J1", "=GROUPBY(A2:A7,B2:B7,COUNT,0,-1)");
    model.evaluate();
    assert_eq!(row_texts(&model, 1, 'D', 'E'), ["Region", "Amount"]);
    assert_eq!(row_texts(&model, 2, 'D', 'E'), ["West", "23"]);
    assert_eq!(row_texts(&model, 3, 'D', 'E'), ["East", "16"]);
    assert_eq!(row_texts(&model, 4, 'D', 'E'), ["North", "7"]);
    assert_eq!(row_texts(&model, 5, 'D', 'E'), ["Total", "46"]);

    assert_eq!(row_texts(&model, 1, 'G', 'H'), ["East", "5.333333333"]);
    assert_eq!(row_texts(&model, 3, 'G', 'H'), ["West", "11.5"]);
    assert_eq!(row_texts(&model, 4, 'G', 'H'), ["", ""]);

    assert_eq!(row_texts(&model, 1, 'J', 'K'), ["Total", "6"]);
    assert_eq!(row_texts(&model, 2, 'J', 'K'), ["East", "3"]);
}

#[test]
fn groupby_lambda_percentof_and_filter() {
    let mut model = sales();
    model._set("D1", "=GROUPBY(A2:A7,B2:B7,LAMBDA(x,MAX(x)-MIN(x)),0,0)");
    model._set("G1", "=GROUPBY(A2:A7,B2:B7,PERCENTOF,0,1)");
    model._set("J1", "=GROUPBY(A2:A7,B2:B7,SUM,0,1,1,B2:B7>4)");
    model._set(
        "M1",
        "=GROUPBY(A2:A7,B2:B7,LAMBDA(part,all,SUM(part)/SUM(all)),0,0)",
    );
    model.evaluate();
    assert_eq!(row_texts(&model, 1, 'D', 'E'), ["East", "9"]);
    assert_eq!(row_texts(&model, 2, 'D', 'E'), ["North", "0"]);
    assert_eq!(row_texts(&model, 1, 'G', 'H'), ["East", "0.347826087"]);
    assert_eq!(row_texts(&model, 4, 'G', 'H'), ["Total", "1"]);
    assert_eq!(row_texts(&model, 1, 'J', 'K'), ["East", "15"]);
    assert_eq!(row_texts(&model, 2, 'J', 'K'), ["North", "7"]);
    assert_eq!(row_texts(&model, 3, 'J', 'K'), ["West", "20"]);
    assert_eq!(row_texts(&model, 4, 'J', 'K'), ["Total", "42"]);
    assert_eq!(row_texts(&model, 3, 'M', 'N'), ["West", "0.5"]);
}

#[test]
fn groupby_two_levels_with_subtotals() {
    let mut model = new_empty_model();
    set_column(
        &mut model,
        "A",
        1,
        &["Region", "East", "West", "East", "West", "East"],
    );
    set_column(
        &mut model,
        "B",
        1,
        &["Item", "Tea", "Tea", "Rice", "Rice", "Tea"],
    );
    set_column(&mut model, "C", 1, &["Qty", "1", "2", "4", "8", "16"]);
    model._set("E1", "=GROUPBY(A1:B6,C1:C6,SUM)");
    model.evaluate();
    // Two levels: headers shown, subtotals after each region.
    let expected = [
        ["Region", "Item", "Qty"],
        ["East", "Rice", "4"],
        ["East", "Tea", "17"],
        ["East", "", "21"],
        ["West", "Rice", "8"],
        ["West", "Tea", "2"],
        ["West", "", "10"],
        ["Grand Total", "", "31"],
    ];
    for (i, row) in expected.iter().enumerate() {
        assert_eq!(
            row_texts(&model, i as i32 + 1, 'E', 'G'),
            row,
            "row {}",
            i + 1
        );
    }
}

#[test]
fn groupby_errors() {
    let mut model = sales();
    model._set("D1", "=GROUPBY(A2:A7,B2:B6,SUM)");
    model._set("D2", "=GROUPBY(A2:A7,B2:B7,BOGUS)");
    model._set("D3", "=GROUPBY(A2:A7,B2:B7,SUM,5)");
    model._set("D4", "=GROUPBY(A2:A7,B2:B7,SUM,0,2)");
    model._set("D5", "=GROUPBY(A2:A7,B2:B7,SUM,0,1,3)");
    model._set("D6", "=GROUPBY(A2:A7,B2:B7,SUM,0,1,1,B2:B7>100)");
    model._set("D7", "=GROUPBY(A2:A7,B2:B7,5)");
    model.evaluate();
    assert_eq!(model._get_text("D1"), "#VALUE!");
    assert_eq!(model._get_text("D2"), "#NAME?");
    assert_eq!(model._get_text("D3"), "#VALUE!");
    assert_eq!(model._get_text("D4"), "#VALUE!");
    assert_eq!(model._get_text("D5"), "#VALUE!");
    // Everything filtered out: just the total of nothing.
    assert_eq!(model._get_text("D6"), "Total");
    assert_eq!(model._get_text("D7"), "#VALUE!");
}

#[test]
fn groupby_whole_columns() {
    let mut model = sales();
    model._set("D1", "=GROUPBY(A:A,B:B,SUM,1,0)");
    model.evaluate();
    // The empty rows below the data are one blank group, sorted last.
    assert_eq!(row_texts(&model, 1, 'D', 'E'), ["East", "16"]);
    assert_eq!(row_texts(&model, 3, 'D', 'E'), ["West", "23"]);
    assert_eq!(row_texts(&model, 4, 'D', 'E'), ["", "0"]);
    assert_eq!(row_texts(&model, 5, 'D', 'E'), ["", ""]);
}

// ── PIVOTBY ──────────────────────────────────────────────────────────────────

fn pivot_data() -> Model<'static> {
    let mut model = new_empty_model();
    set_column(
        &mut model,
        "A",
        1,
        &["East", "East", "West", "West", "East", "North"],
    );
    set_column(
        &mut model,
        "B",
        1,
        &["2023", "2024", "2023", "2024", "2023", "2024"],
    );
    set_column(&mut model, "C", 1, &["10", "5", "20", "3", "1", "7"]);
    model
}

#[test]
fn pivotby_sum() {
    let mut model = pivot_data();
    model._set("E1", "=PIVOTBY(A1:A6,B1:B6,C1:C6,SUM)");
    model.evaluate();
    let expected = [
        ["", "2023", "2024", "Total"],
        ["East", "11", "5", "16"],
        ["North", "", "7", "7"],
        ["West", "20", "3", "23"],
        ["Total", "31", "15", "46"],
    ];
    for (i, row) in expected.iter().enumerate() {
        assert_eq!(
            row_texts(&model, i as i32 + 1, 'E', 'H'),
            row,
            "row {}",
            i + 1
        );
    }
}

#[test]
fn pivotby_percentof_relative_to() {
    let mut model = pivot_data();
    // Share of each row's total, no column totals, rows sorted descending.
    model._set("E1", "=PIVOTBY(A1:A6,B1:B6,C1:C6,PERCENTOF,0,1,-1,0,1,,1)");
    // Share of each column's total (the default).
    model._set("E10", "=PIVOTBY(A1:A6,B1:B6,C1:C6,PERCENTOF,0,0,1,0)");
    model.evaluate();
    assert_eq!(row_texts(&model, 1, 'E', 'G'), ["", "2023", "2024"]);
    assert_eq!(
        row_texts(&model, 2, 'E', 'G'),
        ["West", "0.869565217", "0.130434783"]
    );
    assert_eq!(row_texts(&model, 4, 'E', 'G'), ["East", "0.6875", "0.3125"]);
    assert_eq!(
        row_texts(&model, 5, 'E', 'G'),
        ["Total", "0.673913043", "0.326086957"]
    );
    assert_eq!(
        row_texts(&model, 11, 'E', 'G'),
        ["East", "0.35483871", "0.333333333"]
    );
}

#[test]
fn pivotby_several_values_and_headers() {
    let mut model = new_empty_model();
    set_column(&mut model, "A", 1, &["Region", "East", "West", "East"]);
    set_column(&mut model, "B", 1, &["Year", "2023", "2023", "2024"]);
    set_column(&mut model, "C", 1, &["Qty", "1", "2", "3"]);
    set_column(&mut model, "D", 1, &["Price", "10", "20", "30"]);
    model._set("F1", "=PIVOTBY(A1:A4,B1:B4,C1:D4,SUM,3,0,,0)");
    model.evaluate();
    assert_eq!(row_texts(&model, 1, 'F', 'J'), ["", "2023", "", "2024", ""]);
    assert_eq!(
        row_texts(&model, 2, 'F', 'J'),
        ["Region", "Qty", "Price", "Qty", "Price"]
    );
    assert_eq!(
        row_texts(&model, 3, 'F', 'J'),
        ["East", "1", "10", "3", "30"]
    );
    assert_eq!(row_texts(&model, 4, 'F', 'J'), ["West", "2", "20", "", ""]);
    assert_eq!(model._get_text("F5"), "");
}
