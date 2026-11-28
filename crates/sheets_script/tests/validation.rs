//! Data validation from macros: `Range.Validation` Add / Delete / Modify
//! and the rule's properties.

mod support;
use support::*;

use sheets_engine::{ErrorStyle, ValidationOperator, ValidationType};

/// Region lists on B2:B5 (typed) and D2:D5 (from E1:E3), amounts 1 to
/// 100 with messages on C2:C5, as a recorded macro sets them.
const RULES: &str = r##"
Sub AddRules()
    Range("E1").Value = "East"
    Range("E2").Value = "West"
    Range("E3").Value = "North"
    With Range("B2:B5").Validation
        .Delete
        .Add Type:=xlValidateList, AlertStyle:=xlValidAlertStop, _
            Operator:=xlBetween, Formula1:="East,West,North"
        .IgnoreBlank = True
        .InCellDropdown = True
    End With
    Range("D2:D5").Validation.Add xlValidateList, , , "=$E$1:$E$3"
    With Range("C2:C5").Validation
        .Delete
        .Add Type:=xlValidateWholeNumber, AlertStyle:=xlValidAlertWarning, _
            Operator:=xlBetween, Formula1:="1", Formula2:="100"
        .IgnoreBlank = True
        .InputTitle = "Amount"
        .InputMessage = "A whole number from 1 to 100"
        .ErrorTitle = "Out of range"
        .ErrorMessage = "Type 1 to 100"
        .ShowInput = True
        .ShowError = True
    End With
End Sub
"##;

#[test]
fn list_and_whole_number_rules() {
    let code = format!(
        "{RULES}{}",
        r##"
Sub ReadBack()
    With Range("B3").Validation
        Debug.Print CStr(.Type) & "|" & .Formula1 & "|" & .InCellDropdown
    End With
    With Range("D4").Validation
        Debug.Print CStr(.Type) & "|" & .Formula1
    End With
    With Range("C2:C5").Validation
        Debug.Print CStr(.Type) & "|" & CStr(.Operator) & "|" & .Formula1 & "|" & .Formula2
        Debug.Print CStr(.AlertStyle) & "|" & .ErrorTitle & "|" & .ErrorMessage & "|" & .ShowError
        Debug.Print .InputTitle & "|" & .InputMessage & "|" & .ShowInput & "|" & .IgnoreBlank
    End With
    Debug.Print Range("C5").Validation.Value & "|" & Range("C2").Validation.Value
End Sub
"##
    );
    let (mut book, outcome) = run(sales(), &code, "AddRules");
    ok(&outcome);
    let b = book.validation(0, 2, 1).expect("a rule on B3");
    assert_eq!(b.kind, ValidationType::List);
    assert_eq!(b.formula1.as_deref(), Some("\"East,West,North\""));
    assert!(b.dropdown && b.allow_blank);
    let items: Vec<String> = book
        .validation_list(0, 4, 1)
        .unwrap()
        .into_iter()
        .map(|i| i.text)
        .collect();
    assert_eq!(items, ["East", "West", "North"]);
    let items: Vec<String> = book
        .validation_list(0, 1, 3)
        .unwrap()
        .into_iter()
        .map(|i| i.text)
        .collect();
    assert_eq!(items, ["East", "West", "North"]);
    let c = book.validation(0, 3, 2).expect("a rule on C4");
    assert_eq!(c.kind, ValidationType::Whole);
    assert_eq!(c.operator, ValidationOperator::Between);
    assert_eq!(c.formula1.as_deref(), Some("1"));
    assert_eq!(c.formula2.as_deref(), Some("100"));
    let (style, msg) = c.error.clone().unwrap();
    assert_eq!(style, ErrorStyle::Warning);
    assert_eq!(msg.title, "Out of range");
    assert_eq!(msg.text, "Type 1 to 100");
    let prompt = c.prompt.clone().unwrap();
    assert_eq!(prompt.title, "Amount");
    assert_eq!(prompt.text, "A whole number from 1 to 100");
    assert!(book.validation(0, 0, 2).is_none());

    let (_, outcome, printed) = run_printed(book, &code, "ReadBack");
    ok(&outcome);
    assert_eq!(
        printed,
        [
            "3|East,West,North|True",
            "3|=$E$1:$E$3",
            "1|1|1|100",
            "2|Out of range|Type 1 to 100|True",
            "Amount|A whole number from 1 to 100|True|True",
            // C5 holds 50, C2 holds 300.
            "True|False",
        ]
    );
}

#[test]
fn delete_then_add_again_and_modify() {
    let code = r##"
Sub Redo()
    With Range("C2:C5").Validation
        .Delete
        .Add Type:=xlValidateDecimal, AlertStyle:=xlValidAlertInformation, _
            Operator:=xlGreater, Formula1:="0"
        Debug.Print CStr(.Type) & "|" & CStr(.Operator) & "|" & .Formula1 & "|" & CStr(.AlertStyle)
        .Modify Operator:=xlLessEqual, Formula1:="500"
        Debug.Print CStr(.Operator) & "|" & .Formula1
        .Formula1 = "=MAX($C$2:$C$5)"
        Debug.Print .Formula1
    End With
    Range("B2:B5").Validation.Delete
End Sub
"##;
    let (book, outcome) = run(sales(), RULES, "AddRules");
    ok(&outcome);
    let (mut book, outcome, printed) = run_printed(book, code, "Redo");
    ok(&outcome);
    assert_eq!(printed, ["2|5|0|3", "8|500", "=MAX($C$2:$C$5)"]);
    let c = book.validation(0, 1, 2).unwrap();
    assert_eq!(c.kind, ValidationType::Decimal);
    assert_eq!(c.operator, ValidationOperator::LessThanOrEqual);
    assert_eq!(c.formula1.as_deref(), Some("MAX($C$2:$C$5)"));
    assert_eq!(c.error.as_ref().unwrap().0, ErrorStyle::Information);
    assert!(book.validation(0, 1, 1).is_none());
    // D2:D5 keeps its list.
    assert!(book.validation(0, 1, 3).is_some());
}

#[test]
fn adding_over_a_rule_fails_as_in_excel() {
    let code = r##"
Sub Twice()
    Range("C3").Validation.Add xlValidateWholeNumber, , xlGreater, "0"
End Sub

Sub Trapped()
    On Error Resume Next
    Range("B1:B2").Validation.Add xlValidateList, , , "a,b"
    Debug.Print CStr(Err.Number)
    Err.Clear
    Debug.Print CStr(Range("A1").Validation.Type)
    Debug.Print CStr(Err.Number)
    Err.Clear
    Range("A2:A3").Validation.Add xlValidateList
    Debug.Print CStr(Err.Number)
End Sub
"##;
    let (book, outcome) = run(sales(), RULES, "AddRules");
    ok(&outcome);
    let (book, outcome) = run(book, code, "Twice");
    assert_eq!(err(&outcome).number, 1004);
    let (mut book, outcome, printed) = run_printed(book, code, "Trapped");
    ok(&outcome);
    assert_eq!(printed, ["1004", "1004", "1004"]);
    // Nothing changed: B1 has no rule, C3 keeps its own.
    assert!(book.validation(0, 0, 1).is_none());
    assert_eq!(
        book.validation(0, 2, 2).unwrap().kind,
        ValidationType::Whole
    );
    assert!(book.validation(0, 1, 0).is_none());
}

#[test]
fn validation_survives_save_and_reopen() {
    let (book, outcome) = run(sales(), RULES, "AddRules");
    ok(&outcome);
    let (mut book, _dir) = reopen(book, "rules.xlsx");
    let b = book.validation(0, 1, 1).unwrap();
    assert_eq!(b.kind, ValidationType::List);
    assert_eq!(b.formula1.as_deref(), Some("\"East,West,North\""));
    let d = book.validation(0, 4, 3).unwrap();
    assert_eq!(d.kind, ValidationType::List);
    assert_eq!(d.formula1.as_deref(), Some("$E$1:$E$3"));
    let c = book.validation(0, 4, 2).unwrap();
    assert_eq!(c.kind, ValidationType::Whole);
    assert_eq!(c.formula1.as_deref(), Some("1"));
    assert_eq!(c.formula2.as_deref(), Some("100"));
    let (style, msg) = c.error.clone().unwrap();
    assert_eq!(style, ErrorStyle::Warning);
    assert_eq!(msg.text, "Type 1 to 100");
    assert_eq!(c.prompt.clone().unwrap().title, "Amount");
    // A macro reads the reopened rules too.
    let code = "Sub R()\n    Debug.Print Range(\"C2\").Validation.Formula2\nEnd Sub\n";
    let (_, outcome, printed) = run_printed(book, code, "R");
    ok(&outcome);
    assert_eq!(printed, ["100"]);
}

#[test]
fn undo_takes_the_rules_back_in_one_step() {
    let (mut book, outcome) = run(sales(), RULES, "AddRules");
    ok(&outcome);
    assert_eq!(book.sheet_validations(0).len(), 3);
    assert!(book.undo().unwrap());
    assert!(book.sheet_validations(0).is_empty());
    assert_eq!(cell(&book, 0, "E1"), "");
}
