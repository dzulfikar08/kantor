//! Functions Kantor adds to IronCalc (vendor/ironcalc_base, KANTOR_PATCHES.md
//! §10): they calculate, and their names round-trip through xlsx with the
//! prefixes Excel writes (`_xlfn.`, `_xleta.` for a function passed by name).

use std::io::Read;

use sheets_engine::Workbook;

fn sheet_xml(path: &std::path::Path) -> String {
    let file = std::fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut xml = String::new();
    zip.by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut xml)
        .unwrap();
    xml
}

#[test]
fn new_functions_calculate_and_round_trip_through_xlsx() {
    let mut wb = Workbook::new_empty().unwrap();
    let data = [
        ("Region", "Amount"),
        ("East", "10"),
        ("West", "20"),
        ("East", "5"),
    ];
    for (row, (a, b)) in data.iter().enumerate() {
        wb.set_input(0, row as u32, 0, a).unwrap();
        wb.set_input(0, row as u32, 1, b).unwrap();
    }
    wb.set_input(0, 0, 3, "=GROUPBY(A2:A4,B2:B4,SUM)").unwrap();
    wb.set_input(0, 0, 6, "=AGGREGATE(14,6,B2:B4,1)").unwrap();
    wb.set_input(0, 1, 6, "=BAHTTEXT(B3)").unwrap();
    wb.set_input(0, 2, 6, "=PIVOTBY(A2:A4,A2:A4,B2:B4,COUNT,,0,,0)")
        .unwrap();

    assert_eq!(wb.display_value(0, 0, 3), "East");
    assert_eq!(wb.display_value(0, 0, 4), "15");
    assert_eq!(wb.display_value(0, 2, 3), "Total");
    assert_eq!(wb.display_value(0, 2, 4), "35");
    assert_eq!(wb.display_value(0, 0, 6), "20");
    assert_eq!(wb.display_value(0, 1, 6), "ยี่สิบบาทถ้วน");
    assert_eq!(wb.display_value(0, 3, 7), "2");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("functions.xlsx");
    wb.save_as(&path).unwrap();
    let xml = sheet_xml(&path);
    for formula in [
        "_xlfn.GROUPBY(A2:A4,B2:B4,_xleta.SUM)",
        "_xlfn.AGGREGATE(14,6,B2:B4,1)",
        "BAHTTEXT(B3)",
        "_xlfn.PIVOTBY(A2:A4,A2:A4,B2:B4,_xleta.COUNT,,0,,0)",
    ] {
        assert!(xml.contains(formula), "{formula} not in {xml}");
    }

    let mut back = Workbook::open(&path).unwrap();
    assert_eq!(back.input(0, 0, 3), "=GROUPBY(A2:A4,B2:B4,SUM)");
    assert_eq!(back.input(0, 0, 6), "=AGGREGATE(14,6,B2:B4,1)");
    back.recalculate();
    assert_eq!(back.display_value(0, 1, 4), "20");
    assert_eq!(back.display_value(0, 2, 4), "35");
    assert_eq!(back.display_value(0, 1, 6), "ยี่สิบบาทถ้วน");
}
