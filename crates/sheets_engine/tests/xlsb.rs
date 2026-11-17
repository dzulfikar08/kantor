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
    let (a, b, c) = (wb.column_width(0, 0), wb.column_width(0, 1), wb.column_width(0, 2));
    assert!(a > b && b > c, "{a} {b} {c}");
    assert!(wb.is_column_hidden(0, 3));
    // Row 1 is 30 pt, taller than row 2.
    assert!(wb.row_height(0, 0) > wb.row_height(0, 1) * 1.5);
}
