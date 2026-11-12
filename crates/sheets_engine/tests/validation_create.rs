//! Data Validation… in Kantor: rules set on cells (taking them from other
//! rules), Clear All, undo, the `<dataValidations>` a save writes (read back
//! the same), and sheets nobody edited stay byte for byte.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::path::{Path, PathBuf};

use pivot_check::{el, els, parts};
use roxmltree::Document;
use sheets_engine::{
    CellRange, ErrorStyle, Message, Validation, ValidationOperator, ValidationType, Workbook,
    list_source, validation_operand,
};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("validation_create");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn whole_1_to_10() -> Validation {
    Validation {
        kind: ValidationType::Whole,
        operator: ValidationOperator::Between,
        allow_blank: true,
        dropdown: true,
        prompt: Some(Message {
            title: "Quantity".into(),
            text: "A whole number from 1 to 10".into(),
        }),
        error: Some((
            ErrorStyle::Warning,
            Message {
                title: "Check".into(),
                text: "1 to 10 & no more".into(),
            },
        )),
        formula1: Some(validation_operand(ValidationType::Whole, "1").unwrap()),
        formula2: Some(validation_operand(ValidationType::Whole, "10").unwrap()),
        ranges: Vec::new(),
    }
}

fn list_rule() -> Validation {
    Validation {
        kind: ValidationType::List,
        operator: ValidationOperator::Between,
        allow_blank: true,
        dropdown: true,
        prompt: None,
        error: Some((ErrorStyle::Stop, Message::default())),
        formula1: Some(list_source("Open, Closed, Pending").unwrap()),
        formula2: None,
        ranges: Vec::new(),
    }
}

#[test]
fn set_undo_save_and_read_back() {
    let mut wb = Workbook::new_empty().unwrap();
    let qty = CellRange::from_corners((1, 1), (10, 1));
    wb.set_validation(0, &[qty], whole_1_to_10()).unwrap();
    wb.set_input(0, 2, 1, "12").unwrap();
    assert!(wb.validate_cell(0, 2, 1).is_some(), "12 breaks the rule");
    wb.set_input(0, 2, 1, "5").unwrap();
    assert!(wb.validate_cell(0, 2, 1).is_none());
    let status = CellRange::from_corners((1, 0), (10, 0));
    wb.set_validation(0, &[status], list_rule()).unwrap();
    let items: Vec<String> = wb
        .validation_list(0, 3, 0)
        .unwrap()
        .into_iter()
        .map(|i| i.text)
        .collect();
    assert_eq!(items, vec!["Open", "Closed", "Pending"]);
    assert!(wb.undo().unwrap());
    assert!(wb.validation(0, 3, 0).is_none());
    assert!(wb.redo().unwrap());
    assert!(wb.validation(0, 3, 0).is_some());

    let path = scratch("new.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    let doc = Document::parse(&p["xl/worksheets/sheet1.xml"]).unwrap();
    let dvs = el(doc.root_element(), "dataValidations").expect("dataValidations");
    let list: Vec<_> = els(dvs, "dataValidation").collect();
    assert_eq!(dvs.attribute("count"), Some("2"));
    let w = list
        .iter()
        .find(|d| d.attribute("type") == Some("whole"))
        .unwrap();
    assert_eq!(w.attribute("sqref"), Some("B2:B11"));
    assert_eq!(w.attribute("errorStyle"), Some("warning"));
    assert_eq!(w.attribute("error"), Some("1 to 10 & no more"));
    assert_eq!(w.attribute("showInputMessage"), Some("1"));
    assert_eq!(el(*w, "formula2").unwrap().text(), Some("10"));
    let l = list
        .iter()
        .find(|d| d.attribute("type") == Some("list"))
        .unwrap();
    assert_eq!(
        el(*l, "formula1").unwrap().text(),
        Some("\"Open,Closed,Pending\"")
    );
    assert_eq!(l.attribute("showDropDown"), None);

    let mut back = Workbook::open(&path).unwrap();
    let mut a = back.sheet_validations(0);
    let mut b = wb.sheet_validations(0);
    a.sort_by_key(|v| v.ranges[0].left);
    b.sort_by_key(|v| v.ranges[0].left);
    assert_eq!(a, b);
    assert!(back.validation(0, 3, 0).is_some());
}

#[test]
fn a_new_rule_takes_cells_from_others_and_clear_all() {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_validation(
        0,
        &[CellRange::from_corners((0, 0), (9, 3))],
        whole_1_to_10(),
    )
    .unwrap();
    wb.set_validation(0, &[CellRange::from_corners((2, 1), (4, 2))], list_rule())
        .unwrap();
    let v = wb.sheet_validations(0);
    assert_eq!(v.len(), 2);
    let whole = v.iter().find(|r| r.kind == ValidationType::Whole).unwrap();
    let cells: u32 = whole
        .ranges
        .iter()
        .map(|r| r.rows_count() * (r.right - r.left + 1))
        .sum();
    assert_eq!(cells, 40 - 6);
    assert_eq!(wb.validation(0, 3, 1).unwrap().kind, ValidationType::List);
    // Clear All on a block that covers the list and part of the rest.
    wb.clear_validation(0, &[CellRange::from_corners((0, 0), (9, 2))])
        .unwrap();
    let v = wb.sheet_validations(0);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].ranges, vec![CellRange::from_corners((0, 3), (9, 3))]);
    assert!(wb.undo().unwrap());
    assert_eq!(wb.sheet_validations(0).len(), 2);
}

#[test]
fn rules_follow_inserted_rows() {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_validation(
        0,
        &[CellRange::from_corners((1, 1), (3, 1))],
        whole_1_to_10(),
    )
    .unwrap();
    wb.insert_rows(0, 0, 2).unwrap();
    assert_eq!(
        wb.sheet_validations(0)[0].ranges,
        vec![CellRange::from_corners((3, 1), (5, 1))]
    );
    let path = scratch("moved.xlsx");
    wb.save_as(&path).unwrap();
    let p = parts(&path);
    assert!(p["xl/worksheets/sheet1.xml"].contains("sqref=\"B4:B6\""));
}

#[test]
fn edited_sheet_drops_the_x14_copy_and_others_stay() {
    let src = corpus("19_data_validation.xlsx");
    let mut plain = Workbook::open(&src).unwrap();
    let plain_path = scratch("plain19.xlsx");
    plain.save_as(&plain_path).unwrap();
    let mut wb = Workbook::open(&src).unwrap();
    let before = wb.sheet_validations(0);
    // Change the rule of E2:E6 only.
    let mut rule = whole_1_to_10();
    rule.operator = ValidationOperator::GreaterThan;
    rule.formula1 = Some("0".into());
    rule.formula2 = None;
    wb.set_validation(0, &[CellRange::from_corners((1, 4), (5, 4))], rule)
        .unwrap();
    let path = scratch("edited19.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let a = parts(&plain_path);
    let b = parts(&path);
    assert_eq!(a["xl/worksheets/sheet2.xml"], b["xl/worksheets/sheet2.xml"]);
    let sheet = &b["xl/worksheets/sheet1.xml"];
    assert!(!sheet.contains("CCE6A557"), "the x14 copy is gone");
    let mut back = Workbook::open(&path).unwrap();
    let after = back.sheet_validations(0);
    assert_eq!(after.len(), before.len());
    let e = back.validation(0, 2, 4).unwrap();
    assert_eq!(e.operator, ValidationOperator::GreaterThan);
    // Rules that came from the x14 extension are now plain ones, the same.
    for v in &before {
        if v.ranges.iter().any(|r| r.left == 4) {
            continue;
        }
        assert!(after.contains(v), "{v:?} lost");
    }
}
