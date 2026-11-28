//! Formulas written by macros over ranges.

mod support;
use support::*;

#[test]
fn one_formula_over_a_range_moves_relative_references() {
    let mut book = sales();
    set(&mut book, 0, "D2", "1");
    set(&mut book, 0, "D3", "2");
    let code = r#"
Sub Totals()
    Range("C6:D6").Formula = "=SUM(C2:C5)"
    Range("E2:E3").Value = "=C2*$D$2"
    Range("F2").Formula = "=SUM(C2:C5)"
End Sub
"#;
    let (book, outcome) = run(book, code, "Totals");
    ok(&outcome);
    assert_eq!(input(&book, 0, "C6"), "=SUM(C2:C5)");
    assert_eq!(input(&book, 0, "D6"), "=SUM(D2:D5)");
    assert_eq!(cell(&book, 0, "D6"), "3");
    assert_eq!(input(&book, 0, "E2"), "=C2*$D$2");
    assert_eq!(input(&book, 0, "E3"), "=C3*$D$2");
    assert_eq!(input(&book, 0, "F2"), "=SUM(C2:C5)");
}
