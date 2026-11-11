//! Row heights from an `.xlsx` (default height, custom, hidden) and back.
//! Fixture: `scripts/corpus/gen_row_heights_xlsx.py` (openpyxl).

use std::io::Read;
use std::path::{Path, PathBuf};

use sheets_engine::{RowEntry, RowHeight, Workbook};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/row_heights.xlsx")
}

fn sheet_xml(path: &Path) -> String {
    let file = std::fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut xml = String::new();
    zip.by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    xml
}

fn entry(wb: &Workbook, row: u32) -> Option<RowEntry> {
    wb.row_spec(0).rows.into_iter().find(|r| r.row == row)
}

#[test]
fn file_heights_are_read() {
    let wb = Workbook::open(fixture()).unwrap();
    let spec = wb.row_spec(0);
    assert_eq!(spec.default_points, 18.0, "sheetFormatPr defaultRowHeight");
    let tall = entry(&wb, 1).unwrap();
    assert_eq!((tall.points, tall.custom, tall.hidden), (40.0, true, false));
    let hidden = entry(&wb, 2).unwrap();
    assert!(hidden.hidden && !hidden.custom);
    // Rows without `ht` have no custom height.
    assert!(entry(&wb, 0).is_none_or(|r| !r.custom && !r.hidden));
    assert!(entry(&wb, 3).is_none_or(|r| !r.custom));
    // The wrapped and the large-font cells can make their rows taller.
    assert_eq!(wb.row_fit_cells(0, 3).len(), 1);
    assert_eq!(wb.row_fit_cells(0, 4).len(), 1);
    // Merged cells don't count.
    assert!(wb.row_fit_cells(0, 0).is_empty());
}

#[test]
fn heights_round_trip_through_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rows.xlsx");
    std::fs::copy(fixture(), &path).unwrap();

    let mut wb = Workbook::open(&path).unwrap();
    assert!(
        wb.set_row_heights(0, 5, 6, RowHeight::Points(33.5))
            .unwrap()
    );
    // The tall row back to automatic.
    assert!(wb.set_row_heights(0, 1, 1, RowHeight::Auto(18.0)).unwrap());
    wb.save().unwrap();

    let xml = sheet_xml(&path);
    assert!(
        xml.contains(r#"defaultRowHeight="18""#),
        "sheetFormatPr carried: {xml}"
    );
    assert!(xml.contains(r#"<row r="6" s="0" ht="33.5" customHeight="1""#));
    assert!(xml.contains(r#"<row r="7" s="0" ht="33.5" customHeight="1""#));
    assert!(xml.contains(r#"hidden="1""#), "the hidden row stays hidden");
    assert!(!xml.contains(r#"ht="40""#), "row 2 is automatic again");

    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.row_spec(0).default_points, 18.0);
    let r6 = entry(&back, 5).unwrap();
    assert_eq!((r6.points, r6.custom), (33.5, true));
    assert!(entry(&back, 2).unwrap().hidden);
    assert!(entry(&back, 1).is_none_or(|r| !r.custom));
}

#[test]
fn setting_a_height_shows_a_hidden_row_and_undo_hides_it_again() {
    let mut wb = Workbook::open(fixture()).unwrap();
    wb.set_row_heights(0, 2, 2, RowHeight::Points(20.0))
        .unwrap();
    let r = entry(&wb, 2).unwrap();
    assert_eq!((r.points, r.custom, r.hidden), (20.0, true, false));
    assert!(wb.undo().unwrap());
    let r = entry(&wb, 2).unwrap();
    assert!(r.hidden && !r.custom);
    assert!(!wb.is_dirty());
}
