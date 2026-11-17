//! Workbooks with macros (`.xlsm`): the VBA project is kept, not run, and
//! saved back into a macro-enabled workbook; a save as `.xlsx` leaves it
//! out, as Excel does.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use sheets_engine::{ImportNote, Workbook};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("macros");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Every part of the package at `path`, as bytes.
fn parts(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut data = Vec::new();
        e.read_to_end(&mut data).unwrap();
        out.insert(e.name().unwrap().into_owned(), data);
    }
    out
}

fn text(parts: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(parts[name].clone()).unwrap()
}

/// Stand-in bytes for a VBA project (Kantor never reads inside it).
const VBA: &[u8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1synthetic vba project for a test";

/// A synthetic `.xlsm`: a Kantor-saved workbook with a VBA project added
/// the way Excel stores one.
fn xlsm(path: &Path) {
    let plain = path.with_extension("plain.xlsx");
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "Total").unwrap();
    wb.set_input(0, 0, 1, "=1+2").unwrap();
    wb.save_as(&plain).unwrap();
    let mut parts = parts(&plain);
    let ct = text(&parts, "[Content_Types].xml")
        .replace(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
            "application/vnd.ms-excel.sheet.macroEnabled.main+xml",
        )
        .replace(
            "</Types>",
            "<Default Extension=\"bin\" ContentType=\"application/vnd.ms-office.vbaProject\"/></Types>",
        );
    parts.insert("[Content_Types].xml".into(), ct.into_bytes());
    let rels = text(&parts, "xl/_rels/workbook.xml.rels").replace(
        "</Relationships>",
        "<Relationship Id=\"rId99\" Type=\"http://schemas.microsoft.com/office/2006/relationships/vbaProject\" Target=\"vbaProject.bin\"/></Relationships>",
    );
    parts.insert("xl/_rels/workbook.xml.rels".into(), rels.into_bytes());
    let workbook = text(&parts, "xl/workbook.xml");
    let at = workbook.find("<bookViews>").unwrap();
    let workbook = format!(
        "{}<workbookPr codeName=\"ThisWorkbook\"/>{}",
        &workbook[..at],
        &workbook[at..]
    );
    parts.insert("xl/workbook.xml".into(), workbook.into_bytes());
    parts.insert("xl/vbaProject.bin".into(), VBA.to_vec());

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    // [Content_Types].xml first, as Office writes it.
    let mut names: Vec<&String> = parts.keys().collect();
    names.sort_by_key(|n| *n != "[Content_Types].xml");
    for name in names {
        zip.start_file(name.as_str(), options).unwrap();
        zip.write_all(&parts[name]).unwrap();
    }
    std::fs::write(path, zip.finish().unwrap().into_inner()).unwrap();
}

fn assert_has_macros(path: &Path) {
    let parts = parts(path);
    assert_eq!(parts["xl/vbaProject.bin"], VBA);
    let ct = text(&parts, "[Content_Types].xml");
    assert!(
        ct.contains("application/vnd.ms-excel.sheet.macroEnabled.main+xml"),
        "{ct}"
    );
    assert!(
        !ct.contains("spreadsheetml.sheet.main+xml"),
        "{ct}"
    );
    assert!(ct.contains("application/vnd.ms-office.vbaProject"), "{ct}");
    let rels = text(&parts, "xl/_rels/workbook.xml.rels");
    assert!(
        rels.contains("relationships/vbaProject\" Target=\"vbaProject.bin\""),
        "{rels}"
    );
    let workbook = text(&parts, "xl/workbook.xml");
    assert!(workbook.contains("codeName=\"ThisWorkbook\""), "{workbook}");
}

#[test]
fn xlsm_keeps_its_macros() {
    let path = scratch("book.xlsm");
    xlsm(&path);
    let mut wb = Workbook::open(&path).unwrap();
    assert!(wb.import_note_list().contains(&ImportNote::MacrosNotRun));
    assert!(wb.has_macros());
    assert_eq!(wb.display_value(0, 0, 1), "3");

    // Save edits back to the .xlsm: the project stays.
    wb.set_input(0, 1, 0, "edited").unwrap();
    wb.save().unwrap();
    assert!(
        !wb.save_notes().iter().any(|n| n.contains("vbaProject")),
        "{:?}",
        wb.save_notes()
    );
    assert_has_macros(&path);

    // And again (the saved file is the next save's original).
    wb.set_input(0, 2, 0, "again").unwrap();
    wb.save().unwrap();
    assert_has_macros(&path);
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.display_value(0, 2, 0), "again");
    assert!(back.has_macros());
}

#[test]
fn save_as_xlsx_drops_macros() {
    let path = scratch("drop.xlsm");
    xlsm(&path);
    let mut wb = Workbook::open(&path).unwrap();
    let plain = scratch("drop.xlsx");
    wb.save_as(&plain).unwrap();
    let parts = parts(&plain);
    assert!(!parts.contains_key("xl/vbaProject.bin"));
    let ct = text(&parts, "[Content_Types].xml");
    assert!(!ct.contains("macroEnabled"), "{ct}");
    assert!(
        !text(&parts, "xl/_rels/workbook.xml.rels").contains("vbaProject"),
        "macros dropped"
    );
    // The workbook is now the .xlsx: nothing left to keep.
    assert!(!wb.has_macros());

    // An .xlsm saved from it has no macros either, but is still a
    // macro-enabled workbook (Excel refuses an .xlsm that isn't).
    let again = scratch("drop-again.xlsm");
    wb.save_as(&again).unwrap();
    assert!(!parts_has_vba(&again));
    let ct = text(&parts_of(&again), "[Content_Types].xml");
    assert!(ct.contains("macroEnabled.main+xml"), "{ct}");
}

#[test]
fn new_workbook_saved_as_xlsm_is_macro_enabled() {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "x").unwrap();
    let path = scratch("new.xlsm");
    wb.save_as(&path).unwrap();
    let ct = text(&parts_of(&path), "[Content_Types].xml");
    assert!(ct.contains("macroEnabled.main+xml"), "{ct}");
    assert!(Workbook::open(&path).is_ok());
}

#[test]
fn xlsx_saved_as_xlsm_from_an_xlsm() {
    // Save As another .xlsm keeps the project too.
    let path = scratch("copy-from.xlsm");
    xlsm(&path);
    let mut wb = Workbook::open(&path).unwrap();
    let copy = scratch("copy-to.xlsm");
    wb.save_as(&copy).unwrap();
    assert_has_macros(&copy);
}

fn parts_of(path: &Path) -> BTreeMap<String, Vec<u8>> {
    parts(path)
}

fn parts_has_vba(path: &Path) -> bool {
    parts(path).keys().any(|k| k.contains("vbaProject"))
}
