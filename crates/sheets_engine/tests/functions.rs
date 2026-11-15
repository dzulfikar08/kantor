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

/// Rewrites `xl/worksheets/sheet1.xml` of an xlsx with `edit`.
fn edit_sheet_xml(path: &std::path::Path, edit: impl Fn(&str) -> String) {
    use std::io::Write;
    let bytes = std::fs::read(path).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).unwrap();
        let name = file.name().unwrap().to_string();
        let mut data = Vec::new();
        file.read_to_end(&mut data).unwrap();
        if name == "xl/worksheets/sheet1.xml" {
            data = edit(std::str::from_utf8(&data).unwrap()).into_bytes();
        }
        out.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        out.write_all(&data).unwrap();
    }
    std::fs::write(path, out.finish().unwrap().into_inner()).unwrap();
}

#[test]
fn forecast_ets_euroconvert_filterxml_round_trip_through_xlsx() {
    let mut wb = Workbook::new_empty().unwrap();
    let values = [
        10.0, 12.5, 11.0, 14.0, 13.5, 16.0, 15.0, 18.5, 17.0, 20.0, 19.5, 22.0,
    ];
    for (i, v) in values.iter().enumerate() {
        wb.set_input(0, i as u32, 0, &(i + 1).to_string()).unwrap();
        wb.set_input(0, i as u32, 1, &v.to_string()).unwrap();
    }
    wb.set_input(0, 0, 3, "=FORECAST.ETS(13.5,B1:B12,A1:A12,0)")
        .unwrap();
    wb.set_input(0, 1, 3, "=FORECAST.ETS.STAT(B1:B12,A1:A12,8)")
        .unwrap();
    wb.set_input(0, 2, 3, "=EUROCONVERT(1.2,\"DEM\",\"EUR\")")
        .unwrap();
    wb.set_input(0, 3, 3, "=FILTERXML(\"<a><b>7</b></a>\",\"//b\")*2")
        .unwrap();
    wb.set_input(0, 4, 3, "=GROUPBY(A1:A3,B1:B3,HSTACK(SUM,MAX),0,0)")
        .unwrap();
    // Excel 365's result, to the last digit.
    assert_eq!(wb.display_value(0, 0, 3), "22.56794634");
    assert_eq!(wb.display_value(0, 1, 3), "1");
    assert_eq!(wb.display_value(0, 2, 3), "0.61");
    assert_eq!(wb.display_value(0, 3, 3), "14");
    assert_eq!(wb.display_value(0, 4, 4), "SUM");
    assert_eq!(wb.display_value(0, 5, 5), "10");

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ets.xlsx");
    wb.save_as(&path).unwrap();
    let xml = sheet_xml(&path);
    for formula in [
        "_xlfn.FORECAST.ETS(13.5,B1:B12,A1:A12,0)",
        "_xlfn.FORECAST.ETS.STAT(B1:B12,A1:A12,8)",
        "EUROCONVERT(1.2,&quot;DEM&quot;,&quot;EUR&quot;)",
        "_xlfn.FILTERXML(",
        "_xlfn.GROUPBY(A1:A3,B1:B3,_xlfn.HSTACK(_xleta.SUM,_xleta.MAX),0,0)",
    ] {
        assert!(xml.contains(formula), "{formula} not in {xml}");
    }
    let mut back = Workbook::open(&path).unwrap();
    back.recalculate();
    assert_eq!(back.input(0, 0, 3), "=FORECAST.ETS(13.5,B1:B12,A1:A12,0)");
    assert_eq!(back.display_value(0, 0, 3), "22.56794634");
    assert_eq!(back.display_value(0, 4, 5), "MAX");
}

#[test]
fn online_functions_keep_the_values_saved_in_the_file() {
    // A file as Excel saves it: STOCKHISTORY's last results cached in the
    // cells, one spilled. Made from a Kantor save with the formulas swapped.
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "=123.5").unwrap();
    wb.set_input(0, 0, 2, "=SEQUENCE(2,2)").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("online.xlsx");
    wb.save_as(&path).unwrap();
    edit_sheet_xml(&path, |xml| {
        assert!(xml.contains("<f>123.5</f>"), "{xml}");
        assert!(xml.contains("_xlfn.SEQUENCE(2,2)"), "{xml}");
        xml.replace(
            "<f>123.5</f>",
            "<f>_xlfn.STOCKHISTORY(\"MSFT\",45000,45000,0,0,1)</f>",
        )
        .replace(
            "_xlfn.SEQUENCE(2,2)",
            "_xlfn.STOCKHISTORY(\"MSFT\",45000,45001,0,0)",
        )
    });

    let mut back = Workbook::open(&path).unwrap();
    back.recalculate();
    assert_eq!(back.display_value(0, 0, 0), "123.5");
    assert_eq!(back.display_value(0, 0, 2), "1");
    assert_eq!(back.display_value(0, 1, 3), "4");
    // Saved again: formulas and values unchanged.
    let again = dir.path().join("again.xlsx");
    back.save_as(&again).unwrap();
    let xml = sheet_xml(&again);
    assert!(
        xml.contains("_xlfn.STOCKHISTORY(&quot;MSFT&quot;,45000,45000,0,0,1)</f><v>123.5</v>"),
        "{xml}"
    );
    let mut third = Workbook::open(&again).unwrap();
    third.recalculate();
    assert_eq!(third.display_value(0, 1, 3), "4");

    // Typed in Kantor, it can't connect.
    third
        .set_input(0, 5, 0, "=STOCKHISTORY(\"MSFT\",45000)")
        .unwrap();
    assert_eq!(third.display_value(0, 5, 0), "#CONNECT!");
}

#[test]
fn image_cells_show_what_the_app_fetched() {
    use sheets_engine::web::{WebKind, WebResult};
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(
        0,
        0,
        0,
        "=IMAGE(\"https://example.com/logo.png\",\"Logo\",0)",
    )
    .unwrap();
    assert_eq!(wb.display_value(0, 0, 0), "#BLOCKED!");
    wb.set_web_enabled(true);
    wb.recalculate();
    assert_eq!(wb.display_value(0, 0, 0), "#BUSY!");
    let requests = wb.web_requests();
    assert_eq!(
        requests,
        [("https://example.com/logo.png".to_string(), WebKind::Image)]
    );
    let png = {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&64u32.to_be_bytes());
        v.extend_from_slice(&32u32.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        v
    };
    let answer = sheets_engine::web::answer(png, WebKind::Image);
    assert!(matches!(answer, WebResult::Image { width: 64, .. }));
    wb.set_web_result(&requests[0].0, WebKind::Image, answer);
    wb.recalculate();
    assert_eq!(wb.display_value(0, 0, 0), "Logo");
    let image = wb.cell_image(0, 0, 0).unwrap();
    assert_eq!((image.pixel_width, image.pixel_height), (64, 32));

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("image.xlsx");
    wb.save_as(&path).unwrap();
    assert!(sheet_xml(&path).contains("_xlfn.IMAGE(&quot;https://example.com/logo.png&quot;"));
}
