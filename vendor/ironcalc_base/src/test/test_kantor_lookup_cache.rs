#![allow(clippy::unwrap_used)]

//! KANTOR PATCH: the lookup cache (`functions/lookup_cache.rs`) gives the
//! results of the uncached lookups, which read and scan the vector on every
//! call. Each workbook here is evaluated both ways and every cell compared;
//! a few results are also checked by hand.

use crate::model::Model;
use crate::test::util::new_empty_model;

/// Evaluates the workbook built by `build` with and without the lookup cache,
/// checks that every cell reads the same, and returns the cached model.
fn same_with_and_without_cache<'a>(build: impl Fn(&mut Model<'a>)) -> Model<'a> {
    let mut cached = new_empty_model();
    build(&mut cached);
    cached.evaluate();
    let mut uncached = new_empty_model();
    build(&mut uncached);
    uncached.evaluation.lookup_cache_enabled = false;
    uncached.evaluate();
    let cells = cached.get_all_cells();
    assert_eq!(cells.len(), uncached.get_all_cells().len());
    for cell in cells {
        assert_eq!(
            cached._get_text_at(cell.index, cell.row, cell.column),
            uncached._get_text_at(cell.index, cell.row, cell.column),
            "sheet {} row {} column {}",
            cell.index,
            cell.row,
            cell.column
        );
    }
    cached
}

/// Column A: every kind of value a lookup has to tell apart. Column B: the row.
const COLUMN: &[&str] = &[
    "apple", "APPLE", "Apple", "1", "'1", "=1+2^-52", "0", "=\"\"", "", "TRUE", "FALSE", "=NA()",
    "=1/0", "a*b", "a~b", "a?c", "=0.1+0.2", "0.3", "1E-20", "Straße", "STRASSE", "'2", "2",
    "apple", "-7.5", "=A1&\"\"",
];

/// Lookup values for column D: hits, near misses, wildcards, blanks, errors.
const LOOKUPS: &[&str] = &[
    "apple", "aPPle", "'1", "1", "=1+2^-52", "0", "=\"\"", "", "TRUE", "FALSE", "a*", "a~*b",
    "a~b", "a~~b", "a?c", "a~?c", "*", "?pple", "0.3", "=0.1+0.2", "1E-20", "2E-20", "strasse",
    "'2", "2", "-7.5", "zzz", "=NA()", "=1/0",
];

fn lookup_workbook(model: &mut Model) {
    for (i, value) in COLUMN.iter().enumerate() {
        let row = i + 1;
        if !value.is_empty() {
            model._set(&format!("A{row}"), value);
        }
        model._set(&format!("B{row}"), &row.to_string());
    }
    let n = COLUMN.len();
    for (i, value) in LOOKUPS.iter().enumerate() {
        let r = i + 1;
        if !value.is_empty() {
            model._set(&format!("D{r}"), value);
        }
        let formulas = [
            format!("=VLOOKUP(D{r},A:B,2,FALSE)"),
            format!("=VLOOKUP(D{r},$A$1:$B${},2,0)", n + 10),
            format!("=MATCH(D{r},A:A,0)"),
            format!("=MATCH(D{r},$A$1:$A${n},0)"),
            format!("=XLOOKUP(D{r},$A$1:$A${n},$B$1:$B${n},\"none\")"),
            format!("=XLOOKUP(D{r},$A$1:$A${n},$B$1:$B${n},\"none\",0,-1)"),
            format!("=XMATCH(D{r},A:A)"),
            format!("=XMATCH(D{r},A:A,0,-1)"),
            format!("=XLOOKUP(D{r},A:A,B:B,\"none\",2)"),
            format!("=VLOOKUP(D{r},A:B,2,TRUE)"),
            format!("=MATCH(D{r},A:A,1)"),
            format!("=MATCH(D{r},A:A,-1)"),
            format!("=LOOKUP(D{r},A:A,B:B)"),
            format!("=COUNTIF(A:A,D{r})"),
        ];
        for (k, formula) in formulas.iter().enumerate() {
            // Columns F onwards.
            model
                .set_user_input(0, r as i32, 6 + k as i32, formula.clone())
                .unwrap();
        }
    }
}

#[test]
fn exact_and_approximate_lookups_match_the_uncached_ones() {
    let model = same_with_and_without_cache(lookup_workbook);
    let row_of = |lookup: &str| LOOKUPS.iter().position(|l| *l == lookup).unwrap() + 1;
    // Duplicates: the first one wins; text is compared without case.
    assert_eq!(model._get_text(&format!("F{}", row_of("aPPle"))), "1");
    // From the last item: the last duplicate (A26 is `=A1&""`).
    assert_eq!(model._get_text(&format!("K{}", row_of("apple"))), "26");
    // The number 1 and the text "1" are different values.
    assert_eq!(model._get_text(&format!("F{}", row_of("1"))), "4");
    assert_eq!(model._get_text(&format!("F{}", row_of("'1"))), "5");
    assert_eq!(model._get_text(&format!("F{}", row_of("'2"))), "22");
    assert_eq!(model._get_text(&format!("F{}", row_of("2"))), "23");
    // Wildcards still work, and `~` escapes them.
    assert_eq!(model._get_text(&format!("F{}", row_of("a*"))), "1");
    assert_eq!(model._get_text(&format!("F{}", row_of("a~*b"))), "14");
    assert_eq!(model._get_text(&format!("F{}", row_of("?pple"))), "1");
    assert_eq!(model._get_text(&format!("F{}", row_of("a~?c"))), "16");
    // Not there.
    assert_eq!(model._get_text(&format!("F{}", row_of("zzz"))), "#N/A");
    assert_eq!(model._get_text(&format!("J{}", row_of("zzz"))), "none");
    // An error as the lookup value is the result.
    assert_eq!(model._get_text(&format!("F{}", row_of("=1/0"))), "#DIV/0!");
}

/// The same lookups across sheets, as in the workbook that motivated the cache:
/// a lookup per row over another sheet's whole columns.
#[test]
fn lookups_across_sheets() {
    let model = same_with_and_without_cache(|model| {
        model.add_sheet("Data").unwrap();
        for (i, value) in COLUMN.iter().enumerate() {
            if !value.is_empty() {
                let v = value.replace("A1", "Data!A1");
                model._set(&format!("Data!A{}", i + 1), &v);
            }
            model._set(&format!("Data!B{}", i + 1), &format!("{}", 100 + i));
        }
        for (i, value) in LOOKUPS.iter().enumerate() {
            let r = i + 1;
            if !value.is_empty() {
                model._set(&format!("A{r}"), value);
            }
            model._set(
                &format!("B{r}"),
                &format!("=VLOOKUP(CLEAN($A{r}),Data!$A:$B,2,0)"),
            );
            model._set(
                &format!("C{r}"),
                &format!(
                    "=IFERROR(VLOOKUP($A{r},Data!$B:$B,1,FALSE),VLOOKUP($A{r},Data!$A:$B,2,FALSE))"
                ),
            );
            model._set(
                &format!("D{r}"),
                &format!("=HLOOKUP($A{r},Data!$1:$2,2,FALSE)"),
            );
        }
    });
    assert_eq!(model._get_text("B1"), "100");
}

/// HLOOKUP searches the first row through the same cache.
#[test]
fn hlookup_by_row() {
    same_with_and_without_cache(|model| {
        for (i, value) in COLUMN.iter().enumerate() {
            let column = i as i32 + 1;
            if !value.is_empty() {
                model
                    .set_user_input(0, 1, column, value.to_string())
                    .unwrap();
            }
            model
                .set_user_input(0, 2, column, format!("{column}"))
                .unwrap();
        }
        for (i, value) in LOOKUPS.iter().enumerate() {
            let r = i as i32 + 5;
            if !value.is_empty() {
                model.set_user_input(0, r, 1, value.to_string()).unwrap();
            }
            model
                .set_user_input(0, r, 2, format!("=HLOOKUP(A{r},$1:$2,2,FALSE)"))
                .unwrap();
            model
                .set_user_input(0, r, 3, format!("=HLOOKUP(A{r},$1:$2,2,TRUE)"))
                .unwrap();
            model
                .set_user_input(0, r, 4, format!("=MATCH(A{r},$1:$1,0)"))
                .unwrap();
        }
    });
}

/// Approximate match over a sorted column, and the empty rows below it that a
/// whole-column range includes.
#[test]
fn approximate_match_ignores_trailing_blanks() {
    let model = same_with_and_without_cache(|model| {
        for i in 1..=50 {
            model._set(&format!("A{i}"), &format!("{}", i * 10));
            model._set(&format!("B{i}"), &format!("row {i}"));
        }
        for (r, value) in ["-5", "0", "10", "15", "255", "500", "9999", "\"x\"", "TRUE"]
            .iter()
            .enumerate()
        {
            let r = r + 1;
            model._set(&format!("D{r}"), &format!("={value}"));
            model._set(&format!("E{r}"), &format!("=VLOOKUP(D{r},A:B,2)"));
            model._set(&format!("F{r}"), &format!("=VLOOKUP(D{r},A:B,2,TRUE)"));
            model._set(&format!("G{r}"), &format!("=MATCH(D{r},A:A)"));
            model._set(&format!("H{r}"), &format!("=MATCH(D{r},A1:A100,1)"));
            model._set(&format!("I{r}"), &format!("=LOOKUP(D{r},A:B)"));
            model._set(
                &format!("J{r}"),
                &format!("=XLOOKUP(D{r},A:A,B:B,\"no\",-1,2)"),
            );
            model._set(
                &format!("K{r}"),
                &format!("=VLOOKUP(D{r},$A$1:$B$50,2,TRUE)"),
            );
        }
    });
    assert_eq!(model._get_text("K4"), "row 1");
    assert_eq!(model._get_text("K5"), "row 25");
    // The empty rows below the data are not searched, as in Excel (upstream
    // read them as 0 and could land on a blank row: "0").
    for column in ["E", "F", "I", "K"] {
        assert_eq!(model._get_text(&format!("{column}4")), "row 1");
        assert_eq!(model._get_text(&format!("{column}6")), "row 50");
    }
    assert_eq!(model._get_text("G7"), "50");
    assert_eq!(model._get_text("H7"), "50");
    assert_eq!(model._get_text("E1"), "#N/A");
}

/// A lookup whose range holds the formula being evaluated reads it as a
/// cycle; a cached vector must not hide that.
#[test]
fn lookup_ranges_holding_the_lookup_itself() {
    same_with_and_without_cache(|model| {
        for i in 1..=10 {
            model._set(&format!("A{i}"), &format!("k{i}"));
            model._set(&format!("B{i}"), &format!("=VLOOKUP(\"k{i}\",A:C,3,FALSE)"));
            model._set(&format!("C{i}"), &format!("{i}"));
            // Searches column B, which holds lookups, including itself.
            model._set(&format!("D{i}"), &format!("=MATCH({i},B:B,0)"));
            model._set(&format!("E{i}"), "=MATCH(\"x\",E:E,0)");
        }
    });
}

/// A lookup column filled by formulas that themselves look things up, read
/// in an order that needs them evaluated on demand.
#[test]
fn lookups_over_formula_columns() {
    same_with_and_without_cache(|model| {
        for i in 1..=40 {
            model._set(&format!("Z{i}"), &format!("{}", (i * 7) % 13));
            model._set(&format!("A{i}"), &format!("=\"id\"&Z{i}"));
            model._set(
                &format!("B{i}"),
                &format!("=VLOOKUP(\"id\"&MOD({i},13),A:Z,26,FALSE)"),
            );
            model._set(&format!("C{i}"), &format!("=MATCH(B{i},B:B,0)"));
        }
    });
}

/// A dynamic array spilling into a column that lookups read earlier in the
/// pass: the pass restarts, and the results are those of the uncached run.
#[test]
fn dynamic_spill_into_a_lookup_range() {
    let model = same_with_and_without_cache(|model| {
        model._set("C1", "=VLOOKUP(3,E:F,2,FALSE)");
        model._set("C2", "=MATCH(4,E:E,0)");
        model._set("C3", "=XLOOKUP(5,E:E,F:F,\"none\")");
        model._set("E1", "=SEQUENCE(6)");
        model._set("F1", "=SEQUENCE(6,1,10)");
        model._set("C4", "=VLOOKUP(3,E:F,2,FALSE)");
    });
    assert_eq!(model._get_text("C1"), "12");
    assert_eq!(model._get_text("C2"), "4");
    assert_eq!(model._get_text("C3"), "14");
}

/// Nothing is kept from one evaluation to the next.
#[test]
fn edits_between_evaluations() {
    let mut model = new_empty_model();
    for i in 1..=5 {
        model._set(&format!("A{i}"), &format!("k{i}"));
        model._set(&format!("B{i}"), &format!("{i}"));
        model._set(&format!("D{i}"), "=VLOOKUP(\"k3\",A:B,2,FALSE)");
    }
    model.evaluate();
    assert_eq!(model._get_text("D5"), "3");
    model._set("B3", "33");
    model._set("A1", "K3");
    model.evaluate();
    assert_eq!(model._get_text("D5"), "1");
    model._set("A1", "x");
    model.evaluate();
    assert_eq!(model._get_text("D5"), "33");
}

/// SUMIFS and friends read their first criteria range through the cache, and
/// the others when that evaluates nothing (never with a dynamic array in the
/// workbook).
fn conditional_sums_workbook(model: &mut Model, with_dynamic_array: bool) {
    model.add_sheet("Data").unwrap();
    if with_dynamic_array {
        model._set("Data!Z1", "=SEQUENCE(3)");
    }
    for i in 1..=60 {
        model._set(&format!("Data!A{i}"), &format!("k{}", i % 7));
        model._set(&format!("Data!B{i}"), &format!("{}", i % 3));
        model._set(&format!("Data!C{i}"), &format!("{i}"));
        model._set(&format!("Data!D{i}"), &format!("=C{i}*2"));
    }
    for i in 1..=20 {
        let formulas = [
            format!("=SUMIFS(Data!$C:$C,Data!$A:$A,$A{i})"),
            format!("=SUMIFS(Data!$D:$D,Data!$A:$A,$A{i},Data!$B:$B,\">0\")"),
            format!("=COUNTIFS(Data!$A:$A,\"k*\",Data!$B:$B,{})", i % 3),
            format!(
                "=AVERAGEIFS(Data!$C:$C,Data!$B:$B,{},Data!$A:$A,$A{i})",
                i % 3
            ),
            format!("=SUMIF($A:$A,$A{i},$B:$B)"),
            // The second criteria range holds these formulas themselves.
            "=SUMIFS($B:$B,$A:$A,\"k1\",$G:$G,\">0\")".to_string(),
            format!("=COUNTIF($A:$A,$A{i})"),
            format!("=COUNTIFS(Data!$A:$A,$A{i},Data!$D:$D,\">10\",Data!$Z:$Z,\"<>x\")"),
            "=COUNTIFS($A:$A,\"k1\",$J:$J,\">0\")".to_string(),
        ];
        model._set(&format!("A{i}"), &format!("k{}", i % 9));
        for (k, formula) in formulas.iter().enumerate() {
            // Columns B to J.
            model
                .set_user_input(0, i, 2 + k as i32, formula.clone())
                .unwrap();
        }
    }
}

#[test]
fn conditional_sums_per_row() {
    for with_dynamic_array in [false, true] {
        let model = same_with_and_without_cache(|model| {
            conditional_sums_workbook(model, with_dynamic_array)
        });
        // k1 is in rows 1, 8, ..., 57.
        assert_eq!(model._get_text("B1"), "261");
        // k1 is in A1, A10 and A19.
        assert_eq!(model._get_text("H1"), "3");
    }
}
