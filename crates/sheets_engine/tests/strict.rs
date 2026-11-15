//! Strict Open XML workbooks (ISO/IEC 29500 Strict): `tests/corpus/strict/dates.xlsx`
//! is `xlsx/04_dates.xlsx` as Strict (scripts/corpus/gen_strict.py), with its
//! two date constants as ISO 8601 date cells. It opens as the original does
//! and saves as Transitional.

use std::io::Read as _;
use std::path::PathBuf;

use sheets_engine::Workbook;

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus")
        .join(name)
}

fn same_cells(a: &Workbook, b: &Workbook) {
    assert_eq!(a.sheet_names(), b.sheet_names());
    for sheet in 0..a.sheet_names().len() as u32 {
        for row in 0..40 {
            for col in 0..6 {
                assert_eq!(
                    (a.input(sheet, row, col), a.display_value(sheet, row, col)),
                    (b.input(sheet, row, col), b.display_value(sheet, row, col)),
                    "sheet {sheet} {}",
                    sheets_engine::cell_name(row, col)
                );
            }
        }
    }
}

fn part(path: &std::path::Path, name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut s = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

#[test]
fn strict_workbook_opens_like_its_original() {
    let original = Workbook::open(corpus("xlsx/04_dates.xlsx")).unwrap();
    let mut strict = Workbook::open(corpus("strict/dates.xlsx")).unwrap();
    same_cells(&original, &strict);
    // The ISO dates are date serials again, shown with the cell's format.
    assert_eq!(strict.input(0, 0, 1), original.input(0, 0, 1));
    assert!(!strict.input(0, 0, 1).is_empty());

    // Saved in place as Transitional (Excel's default), and reopened the same.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dates.xlsx");
    std::fs::copy(corpus("strict/dates.xlsx"), &path).unwrap();
    let mut book = Workbook::open(&path).unwrap();
    assert_eq!(book.path(), Some(path.as_path()));
    book.set_input(0, 30, 0, "edited").unwrap();
    book.save().unwrap();
    for name in ["_rels/.rels", "xl/workbook.xml", "xl/worksheets/sheet1.xml"] {
        let xml = part(&path, name);
        assert!(!xml.contains("purl.oclc.org"), "{name}: {xml}");
        assert!(!xml.contains("conformance"), "{name}: {xml}");
    }
    assert!(!part(&path, "xl/worksheets/sheet1.xml").contains("t=\"d\""));
    let back = Workbook::open(&path).unwrap();
    strict.set_input(0, 30, 0, "edited").unwrap();
    same_cells(&strict, &back);
}
