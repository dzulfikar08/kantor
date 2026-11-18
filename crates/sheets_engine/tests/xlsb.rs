//! Formatting of `.xlsb` imports, against `tests/corpus/xls/styled.xlsb`
//! (written record by record by `scripts/corpus/gen_xlsb.py`; its
//! docstring describes the sheets).

use sheets_engine::{CellRange, HAlign, Workbook};

fn open() -> Workbook {
    Workbook::open(format!(
        "{}/../../tests/corpus/xls/styled.xlsb",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn xlsb_cell_formats() {
    let wb = open();
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());
    assert_eq!(wb.sheet_names(), ["Data", "Plain"]);
    let title = wb.cell_style(0, 0, 0);
    assert!(title.bold);
    assert_eq!(title.font_size, 14);
    assert_eq!(title.align, HAlign::Center);
    assert!(title.font_color.is_some());
    assert!(wb.cell_style(0, 1, 1).borders.bottom.is_some());
    assert_eq!(wb.cell_style(0, 2, 1).num_fmt, "#,##0.00");
    assert_eq!(wb.display_value(0, 2, 1), "1,234.50");
    assert!(wb.cell_style(0, 3, 1).fill.is_some(), "yellow fill");
    assert!(wb.cell_style(0, 2, 1).fill.is_none());
    let pct = wb.cell_style(0, 2, 2);
    assert_eq!(wb.display_value(0, 2, 2), "25.00%");
    assert!(pct.wrap);
    assert_eq!(pct.align, HAlign::Right);
    assert!(!wb.cell_style(1, 0, 0).bold);
}

#[test]
fn xlsb_layout() {
    let wb = open();
    // Merges A1:C1 and B6:C7.
    let merges = wb.merges_in(0, CellRange::from_corners((0, 0), (10, 5)));
    assert_eq!(merges.len(), 2, "{merges:?}");
    assert!(merges.contains(&CellRange::from_corners((0, 0), (0, 2))));
    // Column A wider than B, both wider than the default; D hidden.
    let (a, b, c) = (
        wb.column_width(0, 0),
        wb.column_width(0, 1),
        wb.column_width(0, 2),
    );
    assert!(a > b && b > c, "{a} {b} {c}");
    assert!(wb.is_column_hidden(0, 3));
    // Row 1 is 30 pt, taller than row 2.
    assert!(wb.row_height(0, 0) > wb.row_height(0, 1) * 1.5);
}

/// features.xlsb: defined names, data validation, a pattern fill, and a
/// drawing with a picture and a chart.
#[test]
fn xlsb_names_validation_and_drawings() {
    use sheets_engine::drawing::DrawingKind;
    use sheets_engine::{ValidationOperator, ValidationType};
    let mut wb = Workbook::open(format!(
        "{}/../../tests/corpus/xls/features.xlsb",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());

    let list = wb.sheet_validations(0);
    assert_eq!(list.len(), 2, "{list:?}");
    let yes_no = list.iter().find(|v| v.ranges[0].left == 3).unwrap();
    assert_eq!(yes_no.kind, ValidationType::List);
    assert_eq!(yes_no.formula1.as_deref(), Some("\"Yes,No\""));
    assert_eq!(yes_no.ranges, vec![CellRange::from_corners((2, 3), (3, 3))]);
    assert_eq!(yes_no.error.as_ref().unwrap().1.text, "Pick Yes or No");
    let score = list.iter().find(|v| v.ranges[0].left == 4).unwrap();
    assert_eq!(
        (score.kind, score.operator),
        (ValidationType::Whole, ValidationOperator::Between)
    );
    assert_eq!(
        (score.formula1.as_deref(), score.formula2.as_deref()),
        (Some("1"), Some("10"))
    );
    assert_eq!(score.prompt.as_ref().unwrap().text, "1 to 10");

    let p = wb.fill_pattern(0, 4, 0).expect("a pattern");
    assert_eq!((p.kind.as_str(), p.fg), ("darkDown", 0xC00000));

    let drawings = wb.drawings(0);
    assert_eq!(drawings.len(), 2, "{drawings:?}");
    assert!(matches!(&*drawings[0].kind, DrawingKind::Picture { .. }));
    match &*drawings[1].kind {
        DrawingKind::Chart { xml, .. } => assert!(xml.contains("Data!$B$3:$B$4")),
        k => panic!("{k:?}"),
    }

    // Names: a formula using them, and both saved.
    wb.set_input(1, 3, 0, "=SUM(Amounts)").unwrap();
    assert_eq!(wb.display_value(1, 3, 0), "2221.75");
    wb.set_input(0, 8, 0, "=Rate*100").unwrap();
    assert_eq!(wb.display_value(0, 8, 0), "25");
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("features.xlsx");
    wb.save_as(&out).unwrap();
    let back = Workbook::open(&out).unwrap();
    assert_eq!(back.drawings(0).len(), 2);
    assert_eq!(back.sheet_validations(0).len(), 2);
    assert_eq!(back.display_value(1, 3, 0), "2221.75");
}

/// features.xlsb's conditional formatting (see gen_xlsb.py): every rule
/// comes in as Kantor's own kind, in priority order, with its format.
#[test]
fn xlsb_conditional_formatting() {
    let mut wb = Workbook::open(format!(
        "{}/../../tests/corpus/xls/features.xlsb",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let rules = wb.cf_rules(0, None);
    let got: Vec<(String, String)> = rules
        .iter()
        .map(|r| (r.range.clone(), r.summary.clone()))
        .collect();
    let ranges: Vec<&str> = got.iter().map(|(r, _)| r.as_str()).collect();
    assert_eq!(
        ranges,
        [
            "B3:B4", "A3:A4", "C3", "H1:H5", "I1:I5", "J1:J5", "K1:K5", "L1:L5", "M1:M5", "N1:N5",
            "O1:O5", "P1:P5", "Q1:Q5", "R1:R5", "S1:S5", "T1:T5", "U1:U5", "V1:V5"
        ],
        "{got:#?}"
    );
    let summary = |range: &str| got.iter().find(|(r, _)| r == range).unwrap().1.clone();
    assert_eq!(summary("B3:B4"), "Cell value greater than 1000");
    assert_eq!(summary("A3:A4"), "Text containing \"orth\"");
    assert_eq!(summary("C3"), "Formula: =C3<1");
    assert_eq!(summary("Q1:Q5"), "Cell value between 3 and 6");
    assert_eq!(summary("M1:M5"), "Above average");
    assert_eq!(summary("U1:U5"), "Equal to or above average");
    assert_eq!(summary("V1:V5"), "2 std dev below average");
    for (range, kind) in [
        ("H1:H5", "color scale"),
        ("I1:I5", "data bar"),
        ("J1:J5", "icon set"),
        ("K1:K5", "top 2"),
        ("L1:L5", "bottom 10%"),
        ("M1:M5", "above average"),
        ("N1:N5", "duplicate"),
        ("O1:O5", "blank"),
        ("P1:P5", "yesterday"),
        ("R1:R5", "rating"),
    ] {
        assert!(
            summary(range).to_lowercase().contains(kind),
            "{range}: {}",
            summary(range)
        );
    }
    assert!(rules[0].stop_if_true);
    // The differential formats (BrtDXF) are drawn: 1234.5 > 1000 on light
    // red, "North" contains "orth" on green, "South" doesn't.
    let fill = |wb: &mut Workbook, r: u32, c: u32| {
        wb.conditional(0, r, c)
            .and_then(|x| x.style.as_ref().and_then(|s| s.fill))
    };
    assert_eq!(fill(&mut wb, 2, 1), Some(0xFFC7CE));
    assert_eq!(fill(&mut wb, 2, 0), Some(0xC6EFCE));
    assert_eq!(fill(&mut wb, 3, 0), None);
    // Saved as .xlsx and read back, the rules stay.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("cf.xlsx");
    wb.save_as(&out).unwrap();
    let back = Workbook::open(&out).unwrap();
    assert_eq!(back.cf_rules(0, None).len(), 18);
}

/// features.xlsb's Excel 2010 conditional formatting (gen_xlsb.py
/// `conditional_formats14`): column S's data bar takes its extras from the
/// rule its BrtCFRuleExt names, column T's custom icon set exists only
/// there; both are drawn and saved as Excel 2010 markup.
#[test]
fn xlsb_excel_2010_conditional_formatting() {
    let mut wb = Workbook::open(format!(
        "{}/../../tests/corpus/xls/features.xlsb",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    for (r, v) in ["-5", "0", "10"].iter().enumerate() {
        wb.set_input(0, r as u32, 18, v).unwrap();
        wb.set_input(0, r as u32, 19, &(r * 50).to_string())
            .unwrap();
    }
    let bar = |wb: &mut Workbook, r: u32| wb.conditional(0, r, 18).and_then(|c| c.bar);
    // A negative value: its own colour and the axis in the middle, with
    // a border.
    let neg = bar(&mut wb, 0).expect("bar for -5");
    assert_eq!(neg.color, 0xC00000);
    assert!(!neg.gradient);
    assert_eq!(neg.axis, Some(0.5));
    assert_eq!(neg.border, Some(0x800000));
    assert_eq!(neg.axis_color, Some(0x008000));
    let pos = bar(&mut wb, 2).expect("bar for 10");
    assert_eq!(pos.color, 0x638EC6);
    assert_eq!(pos.border, Some(0x000080));
    // The custom icon set: a red light, no icon, a tick.
    let icon = |wb: &mut Workbook, r: u32| wb.conditional(0, r, 19).and_then(|c| c.icon);
    assert!(icon(&mut wb, 0).is_some());
    assert!(icon(&mut wb, 1).is_none());
    assert!(icon(&mut wb, 2).is_some());
    // Saved as .xlsx: the Excel 2010 markup goes with them.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("cf14.xlsx");
    wb.save_as(&out).unwrap();
    let sheet = {
        use std::io::Read as _;
        let mut z = zip::ZipArchive::new(std::fs::File::open(&out).unwrap()).unwrap();
        let mut s = String::new();
        z.by_name("xl/worksheets/sheet1.xml")
            .unwrap()
            .read_to_string(&mut s)
            .unwrap();
        s
    };
    assert!(sheet.contains("axisPosition=\"middle\""), "{sheet}");
    assert!(sheet.contains("custom=\"1\""), "{sheet}");
    assert!(sheet.contains("iconSet=\"NoIcons\""), "{sheet}");
    assert!(sheet.contains("equalAverage=\"1\""), "{sheet}");
    assert!(sheet.contains("stdDev=\"2\""), "{sheet}");
    let back = Workbook::open(&out).unwrap();
    let rules = back.cf_rules(0, None);
    assert_eq!(rules.len(), 18);
    let t = rules.iter().find(|r| r.range == "T1:T5").unwrap();
    assert!(t.summary.to_lowercase().contains("icon"), "{}", t.summary);
}
