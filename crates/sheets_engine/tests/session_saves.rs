//! Saving twice in one session: what Kantor added is not added again, and
//! an edit saved and then undone is undone in the next file too (whose
//! original is the first save).

use std::path::{Path, PathBuf};

use sheets_engine::{
    CellRange, CfCompare, CfFormat, CfRuleDef, CfSpec, ChartType, ErrorStyle, Message, Paper,
    Validation, ValidationOperator, ValidationType, Workbook,
};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("session_saves");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn book() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let mut cells = vec![(0, 0, "Name".to_owned()), (0, 1, "Value".to_owned())];
    for r in 1..6u32 {
        cells.push((r, 0, format!("N{r}")));
        cells.push((r, 1, (r * 10).to_string()));
    }
    wb.set_inputs(0, &cells).unwrap();
    wb
}

fn count(path: &Path, needle: &str) -> usize {
    let p = sheets_engine_parts(path);
    p.values().map(|x| x.matches(needle).count()).sum()
}

fn sheets_engine_parts(path: &Path) -> std::collections::BTreeMap<String, String> {
    use std::io::Read as _;
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = std::collections::BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut s = String::new();
        if e.read_to_string(&mut s).is_ok() {
            out.insert(e.name().unwrap().into_owned(), s);
        }
    }
    out
}

#[test]
fn saving_twice_adds_nothing_twice() {
    let mut wb = book();
    let data = CellRange::from_corners((0, 0), (5, 1));
    wb.create_table(0, data, true, "TableStyleMedium2").unwrap();
    wb.insert_chart(0, data, ChartType::Column).unwrap();
    let a = scratch("twice-a.xlsx");
    let b = scratch("twice-b.xlsx");
    wb.save_as(&a).unwrap();
    wb.save_as(&b).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert_eq!(count(&b, "<tablePart "), 1);
    assert_eq!(count(&b, "<c:chart "), 1);
    let back = Workbook::open(&b).unwrap();
    assert_eq!(back.tables(0).len(), 1);
    assert_eq!(back.drawings(0).len(), 1);
}

#[test]
fn an_edit_saved_then_undone_is_undone_in_the_next_save() {
    let mut wb = book();
    let data = CellRange::from_corners((0, 0), (5, 1));
    let mut p = wb.print_settings(0);
    p.setup.paper = Paper::LEGAL;
    wb.set_print_settings(0, p).unwrap();
    wb.add_cf_rule(
        0,
        "B2:B6",
        &CfRuleDef {
            spec: CfSpec::CellValue {
                op: CfCompare::GreaterThan,
                value: "20".into(),
                value2: String::new(),
            },
            format: CfFormat::PRESETS[0].1,
            stop_if_true: false,
        },
    )
    .unwrap();
    wb.set_validation(
        0,
        &[CellRange::from_corners((1, 1), (5, 1))],
        Validation {
            kind: ValidationType::Whole,
            operator: ValidationOperator::GreaterThan,
            allow_blank: true,
            dropdown: true,
            prompt: None,
            error: Some((ErrorStyle::Stop, Message::default())),
            formula1: Some("0".into()),
            formula2: None,
            ranges: Vec::new(),
        },
    )
    .unwrap();
    wb.create_table(0, data, true, "TableStyleMedium2").unwrap();
    wb.insert_chart(0, data, ChartType::Pie).unwrap();
    let first = scratch("undo-a.xlsx");
    wb.save_as(&first).unwrap();
    for _ in 0..5 {
        assert!(wb.undo().unwrap());
    }
    let second = scratch("undo-b.xlsx");
    wb.save_as(&second).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let mut back = Workbook::open(&second).unwrap();
    assert_eq!(back.print_settings(0).setup.paper, Paper::LETTER);
    assert!(back.cf_rules(0, None).is_empty());
    assert!(back.sheet_validations(0).is_empty());
    assert!(back.tables(0).is_empty());
    assert!(back.drawings(0).is_empty());
    assert!(back.validation(0, 2, 1).is_none());
    // Redo all and save again: everything is back.
    for _ in 0..5 {
        assert!(wb.redo().unwrap());
    }
    let third = scratch("undo-c.xlsx");
    wb.save_as(&third).unwrap();
    let back = Workbook::open(&third).unwrap();
    assert_eq!(back.print_settings(0).setup.paper, Paper::LEGAL);
    assert_eq!(back.cf_rules(0, None).len(), 1);
    assert_eq!(back.tables(0).len(), 1);
    assert_eq!(back.drawings(0).len(), 1);
}
