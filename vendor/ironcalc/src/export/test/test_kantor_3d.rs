//! KANTOR PATCH (3-D references): kept on save and read back.

use std::io::{Cursor, Read};

use ironcalc_base::Model;

use crate::export::save_xlsx_to_writer;
use crate::import::load_from_xlsx_bytes;

#[test]
fn three_d_references_round_trip() {
    let mut model = Model::new_empty("model", "en", "UTC", "en").unwrap();
    model.rename_sheet("Sheet1", "Summary").unwrap();
    for (name, value) in [("Jan", "1"), ("Feb 2", "2"), ("Mar", "4")] {
        model.add_sheet(name).unwrap();
        let index = model.workbook.worksheets.len() as u32 - 1;
        model.set_user_input(index, 2, 2, value.to_string()).unwrap();
    }
    model
        .set_user_input(0, 1, 1, "=SUM(Jan:Mar!B2)".to_string())
        .unwrap();
    model
        .set_user_input(0, 2, 1, "=MAX('Feb 2:Mar'!B1:B2)".to_string())
        .unwrap();
    model.evaluate();
    assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "7");

    let bytes = save_xlsx_to_writer(&model, Cursor::new(Vec::new()))
        .unwrap()
        .into_inner();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
    let mut sheet = String::new();
    zip.by_name("xl/worksheets/sheet1.xml")
        .unwrap()
        .read_to_string(&mut sheet)
        .unwrap();
    assert!(sheet.contains("<f>SUM(Jan:Mar!B2)</f>"), "{sheet}");
    assert!(sheet.contains("<f>MAX(&apos;Feb 2:Mar&apos;!B1:B2)</f>") || sheet.contains("<f>MAX('Feb 2:Mar'!B1:B2)</f>"), "{sheet}");

    let workbook = load_from_xlsx_bytes(&bytes, "model", "en", "UTC").unwrap();
    let model = Model::from_workbook(workbook, "en").unwrap();
    assert_eq!(
        model.get_cell_formula(0, 1, 1).unwrap().unwrap(),
        "=SUM(Jan:Mar!B2)"
    );
    assert_eq!(model.get_formatted_cell_value(0, 1, 1).unwrap(), "7");
    assert_eq!(model.get_formatted_cell_value(0, 2, 1).unwrap(), "4");
}
