//! Excel tables, structured references, sort and AutoFilter, through open,
//! edit and save. Fixture: `scripts/corpus/gen_tables_xlsx.py` (openpyxl).

use std::io::Read;
use std::path::{Path, PathBuf};

use sheets_engine::{
    CellRange, Condition, FilterKind, FilterOp, FilterScope, SortKey, Workbook, checked_values,
};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tables.xlsx")
}

fn part(path: &Path, name: &str) -> String {
    let file = std::fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut xml = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut xml).unwrap();
    xml
}

/// A copy of the fixture to edit and save.
fn copy() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.xlsx");
    std::fs::copy(fixture(), &path).unwrap();
    (dir, path)
}

fn col(wb: &Workbook, sheet: u32, c: u32, rows: std::ops::RangeInclusive<u32>) -> Vec<String> {
    rows.map(|r| wb.display_value(sheet, r, c)).collect()
}

const SALES: u32 = 0;
const PLAIN: u32 = 1;
const SORT: u32 = 2;

#[test]
fn tables_are_read_with_their_style() {
    let wb = Workbook::open(fixture()).unwrap();
    let t = wb.tables(SALES);
    assert_eq!(t.len(), 2);
    let t = &t[0];
    assert_eq!(t.name, "Sales");
    assert_eq!(t.area, CellRange::from_corners((0, 0), (6, 4)));
    assert_eq!((t.header_rows, t.totals_rows), (1, 1));
    assert!(t.filter_buttons && t.row_stripes);
    assert_eq!(t.style.as_deref(), Some("TableStyleMedium2"));
    // Header filled with accent 1, white bold; bands alternate.
    let head = wb.table_cell_look(SALES, 0, 1).unwrap();
    assert!(head.bold && head.fill.is_some() && head.font_color == Some(0xFFFFFF));
    let band = wb.table_cell_look(SALES, 1, 1).unwrap().fill;
    assert!(band.is_some());
    assert_eq!(wb.table_cell_look(SALES, 2, 1).unwrap().fill, None);
    assert_eq!(wb.table_cell_look(SALES, 3, 1).unwrap().fill, band);
    assert!(wb.table_cell_look(SALES, 6, 2).unwrap().bold);
    assert!(wb.table_cell_look(SALES, 7, 1).is_none());
}

#[test]
fn structured_references_evaluate() {
    let wb = Workbook::open(fixture()).unwrap();
    // Total = Units * Unit Price, per row.
    assert_eq!(col(&wb, SALES, 4, 1..=5), ["25", "60", "20", "12", "24"]);
    assert_eq!(wb.display_value(SALES, 6, 2), "55");
    assert_eq!(wb.display_value(SALES, 6, 4), "141");
    assert_eq!(
        col(&wb, SALES, 6, 0..=5),
        ["55", "55", "7", "5", "68", "55"]
    );
    assert_eq!(wb.input(SALES, 1, 4), "=Sales[@Units]*Sales[@[Unit Price]]");
}

#[test]
fn structured_references_are_saved_as_written() {
    let (_dir, path) = copy();
    let mut wb = Workbook::open(&path).unwrap();
    wb.set_input(SALES, 1, 2, "11").unwrap();
    wb.save().unwrap();
    let xml = part(&path, "xl/worksheets/sheet1.xml");
    assert!(xml.contains("Sales[[#This Row],[Units]]*Sales[[#This Row],[Unit Price]]"));
    assert!(xml.contains("SUM(Sales[Units])"));
    assert!(xml.contains("Sales[[#Totals],[Units]]"));
    assert!(xml.contains("ROWS(Sales[#All])"));
    assert!(
        !xml.contains("$C$2"),
        "no structured reference became a range"
    );
    let wb = Workbook::open(&path).unwrap();
    assert_eq!(wb.display_value(SALES, 1, 4), "27.5");
    assert_eq!(wb.tables(SALES).len(), 2);
}

#[test]
fn a_table_grows_with_rows_inserted_inside_and_saves_its_range() {
    let (_dir, path) = copy();
    let mut wb = Workbook::open(&path).unwrap();
    wb.insert_rows(SALES, 3, 2).unwrap();
    assert_eq!(wb.tables(SALES)[0].area.bottom, 8);
    wb.set_input(SALES, 3, 2, "100").unwrap();
    assert_eq!(wb.display_value(SALES, 0, 6), "155");
    assert_eq!(wb.display_value(SALES, 8, 2), "155");
    wb.save().unwrap();
    let table = part(&path, "xl/tables/table1.xml");
    assert!(table.contains(r#"ref="A1:E9""#), "{table}");
    assert!(table.contains(r#"<autoFilter ref="A1:E8""#), "{table}");
    wb.undo().unwrap();
    wb.undo().unwrap();
    assert_eq!(wb.tables(SALES)[0].area.bottom, 6);
}

#[test]
fn typing_under_a_table_extends_it() {
    let mut wb = Workbook::open(fixture()).unwrap();
    // Sales has a total row: what is typed under it stays outside.
    wb.set_input(SALES, 7, 0, "x").unwrap();
    assert_eq!(wb.tables(SALES)[0].area.bottom, 6);
    // Rates (I1:K4) has none: a value typed in I5 extends it, and its
    // calculated column follows.
    wb.set_input(SALES, 4, 9, "5").unwrap();
    let rates = wb
        .tables(SALES)
        .into_iter()
        .find(|t| t.name == "Rates")
        .unwrap();
    assert_eq!(rates.area.bottom, 4);
    assert_eq!(wb.display_value(SALES, 4, 10), "10");
    assert_eq!(wb.input(SALES, 4, 10), "=Rates[@Rate]*2");
    wb.undo().unwrap();
    let rates = wb
        .tables(SALES)
        .into_iter()
        .find(|t| t.name == "Rates")
        .unwrap();
    assert_eq!(rates.area.bottom, 3);
    assert_eq!(wb.display_value(SALES, 4, 9), "");
    assert_eq!(wb.display_value(SALES, 4, 10), "");
}

#[test]
fn sort_orders_like_excel_and_is_stable() {
    let mut wb = Workbook::open(fixture()).unwrap();
    let range = CellRange::from_corners((0, 0), (10, 3));
    assert!(wb.guess_header(SORT, range));
    let n = wb
        .sort_range(
            SORT,
            range,
            &[SortKey {
                col: 0,
                descending: false,
            }],
            true,
        )
        .unwrap();
    assert_eq!(n, 10);
    // Numbers, text (any case, stable: "Apple" before "apple"), FALSE,
    // TRUE, blank last.
    assert_eq!(
        col(&wb, SORT, 0, 1..=10),
        [
            "1", "2.5", "3", "Apple", "apple", "banana", "cherry", "FALSE", "TRUE", ""
        ]
    );
    assert_eq!(
        col(&wb, SORT, 1, 1..=10),
        ["7", "10", "2", "4", "8", "1", "6", "9", "5", "3"]
    );
    // Formulas moved with their rows: B*2 of the same row, and $F$1 stays.
    assert_eq!(wb.input(SORT, 1, 2), "=B2*2");
    assert_eq!(col(&wb, SORT, 2, 1..=3), ["14", "20", "4"]);
    assert_eq!(col(&wb, SORT, 3, 1..=3), ["70", "100", "20"]);
    // Descending keeps blanks last.
    wb.sort_range(
        SORT,
        range,
        &[SortKey {
            col: 0,
            descending: true,
        }],
        true,
    )
    .unwrap();
    assert_eq!(wb.display_value(SORT, 1, 0), "TRUE");
    assert_eq!(wb.display_value(SORT, 10, 0), "");
    // One undo step each.
    wb.undo().unwrap();
    wb.undo().unwrap();
    assert_eq!(wb.display_value(SORT, 1, 0), "banana");
}

#[test]
fn sort_by_two_keys_and_a_table_column() {
    let mut wb = Workbook::open(fixture()).unwrap();
    let range = CellRange::from_corners((1, 0), (5, 4));
    wb.sort_range(
        SALES,
        range,
        &[
            SortKey {
                col: 0,
                descending: false,
            },
            SortKey {
                col: 2,
                descending: true,
            },
        ],
        false,
    )
    .unwrap();
    assert_eq!(
        col(&wb, SALES, 0, 1..=5),
        ["East", "North", "North", "South", "West"]
    );
    assert_eq!(col(&wb, SALES, 2, 1..=5), ["5", "12", "10", "20", "8"]);
    // The calculated column still multiplies its own row.
    assert_eq!(col(&wb, SALES, 4, 1..=5), ["20", "24", "25", "60", "12"]);
    wb.sort_table(SALES, "Sales", 3, true).unwrap();
    assert_eq!(col(&wb, SALES, 3, 1..=5), ["4", "3", "2.5", "2", "1.5"]);
    // The total row stays where it is.
    assert_eq!(wb.display_value(SALES, 6, 0), "Total");
}

#[test]
fn a_filter_from_the_file_is_read_and_counted_by_subtotal() {
    let wb = Workbook::open(fixture()).unwrap();
    let f = wb.filter(PLAIN, &FilterScope::Sheet).unwrap();
    assert_eq!(f.area, CellRange::from_corners((0, 0), (8, 2)));
    assert!(f.is_filtered(0) && !f.is_filtered(1));
    let values = wb.filter_values(PLAIN, &FilterScope::Sheet, 0);
    let texts: Vec<&str> = values.iter().map(|v| v.text.as_str()).collect();
    // "west" and "West" are one value (filters ignore case).
    assert_eq!(texts, ["East", "North", "South", "West"]);
    assert_eq!(
        checked_values(&values),
        ["East", "North"].iter().map(|s| s.to_string()).collect()
    );
    // SUBTOTAL skips rows a filter hides (9 and 109 alike); SUM doesn't.
    assert_eq!(wb.display_value(PLAIN, 0, 4), "1600");
    assert_eq!(wb.display_value(PLAIN, 1, 4), "1600");
    assert_eq!(wb.display_value(PLAIN, 2, 4), "3600");
}

#[test]
fn filters_by_values_and_conditions_and_round_trip() {
    let (_dir, path) = copy();
    let mut wb = Workbook::open(&path).unwrap();
    let sheet = FilterScope::Sheet;
    // Amount >= 300 and <= 600, on top of Region in (North, East).
    let between = FilterKind::Custom {
        and: true,
        conditions: vec![
            Condition {
                op: FilterOp::GreaterThanOrEqual,
                value: "300".into(),
            },
            Condition {
                op: FilterOp::LessThanOrEqual,
                value: "600".into(),
            },
        ],
    };
    let (shown, total) = wb
        .set_column_filter(PLAIN, &sheet, 1, Some(between))
        .unwrap();
    assert_eq!((shown, total), (2, 8));
    let hidden: Vec<bool> = (1..=8).map(|r| wb.row_hidden(PLAIN, r)).collect();
    assert_eq!(hidden, [true, true, false, true, false, true, true, true]);
    assert_eq!(wb.display_value(PLAIN, 0, 4), "800");
    // A date filter: Day before 2025-03-01 (a serial), Region cleared.
    wb.set_column_filter(PLAIN, &sheet, 0, None).unwrap();
    wb.set_column_filter(PLAIN, &sheet, 1, None).unwrap();
    let before = FilterKind::Custom {
        and: false,
        conditions: vec![Condition {
            op: FilterOp::LessThan,
            value: "45717".into(),
        }],
    };
    assert_eq!(
        wb.set_column_filter(PLAIN, &sheet, 2, Some(before))
            .unwrap()
            .0,
        4
    );
    // Values, with "west" matching "West".
    let values = FilterKind::Values {
        values: vec!["West".into()],
        blanks: false,
        dates: vec![],
    };
    wb.set_column_filter(PLAIN, &sheet, 2, None).unwrap();
    assert_eq!(
        wb.set_column_filter(PLAIN, &sheet, 0, Some(values))
            .unwrap()
            .0,
        2
    );
    wb.save().unwrap();
    let xml = part(&path, "xl/worksheets/sheet2.xml");
    assert!(xml.contains(r#"<autoFilter ref="A1:C9"><filterColumn colId="0"><filters><filter val="West"/></filters></filterColumn></autoFilter>"#), "{xml}");
    assert!(
        xml.contains(r#"<row r="2" hidden="1""#)
            || xml.contains(r#"r="2" hidden="1""#)
            || xml.contains("hidden=\"1\"")
    );
    let wb = Workbook::open(&path).unwrap();
    let f = wb.filter(PLAIN, &FilterScope::Sheet).unwrap();
    assert!(f.is_filtered(0));
    let shown: Vec<u32> = (1..=8).filter(|&r| !wb.row_hidden(PLAIN, r)).collect();
    assert_eq!(shown, [4, 8]);
    assert_eq!(wb.display_value(PLAIN, 0, 4), "1200");
}

#[test]
fn clearing_and_undoing_a_filter() {
    let mut wb = Workbook::open(fixture()).unwrap();
    wb.clear_filter(PLAIN, &FilterScope::Sheet).unwrap();
    assert!((1..=8).all(|r| !wb.row_hidden(PLAIN, r)));
    assert_eq!(wb.display_value(PLAIN, 0, 4), "3600");
    wb.undo().unwrap();
    assert!(wb.row_hidden(PLAIN, 2));
    assert!(
        wb.filter(PLAIN, &FilterScope::Sheet)
            .unwrap()
            .is_filtered(0)
    );
    // Turning the sheet's filter off shows every row and saves no filter.
    assert!(
        !wb.toggle_filter(PLAIN, CellRange::from_corners((0, 0), (0, 0)))
            .unwrap()
    );
    assert!(wb.filter(PLAIN, &FilterScope::Sheet).is_none());
    assert!((1..=8).all(|r| !wb.row_hidden(PLAIN, r)));
}

#[test]
fn a_table_filter_saves_into_the_table_part() {
    let (_dir, path) = copy();
    let mut wb = Workbook::open(&path).unwrap();
    let scope = FilterScope::Table("Sales".into());
    let north = FilterKind::Values {
        values: vec!["North".into()],
        blanks: false,
        dates: vec![],
    };
    assert_eq!(
        wb.set_column_filter(SALES, &scope, 0, Some(north)).unwrap(),
        (2, 5)
    );
    // The total row's SUBTOTAL(109) sees the filtered table.
    assert_eq!(wb.display_value(SALES, 6, 2), "22");
    // SUBTOTAL(9) skips filtered rows too.
    assert_eq!(wb.display_value(SALES, 5, 6), "22");
    wb.save().unwrap();
    let table = part(&path, "xl/tables/table1.xml");
    assert!(table.contains(r#"<autoFilter ref="A1:E6"><filterColumn colId="0"><filters><filter val="North"/></filters></filterColumn></autoFilter>"#), "{table}");
    let wb = Workbook::open(&path).unwrap();
    assert!(wb.filter(SALES, &scope).unwrap().is_filtered(0));
    assert!(wb.row_hidden(SALES, 2));
    assert_eq!(wb.display_value(SALES, 6, 2), "22");
}

#[test]
fn renaming_a_header_renames_the_column_and_its_references() {
    let mut wb = Workbook::open(fixture()).unwrap();
    wb.set_input(SALES, 0, 2, "Qty").unwrap();
    assert_eq!(wb.input(SALES, 0, 6), "=SUM(Sales[Qty])");
    assert_eq!(wb.input(SALES, 1, 4), "=Sales[@Qty]*Sales[@[Unit Price]]");
    assert_eq!(wb.display_value(SALES, 0, 6), "55");
    // A name already taken gets a number.
    wb.set_input(SALES, 0, 1, "qty").unwrap();
    assert_eq!(wb.display_value(SALES, 0, 1), "qty2");
    wb.undo().unwrap();
    wb.undo().unwrap();
    assert_eq!(wb.input(SALES, 0, 6), "=SUM(Sales[Units])");
    assert_eq!(wb.display_value(SALES, 0, 2), "Units");
}
