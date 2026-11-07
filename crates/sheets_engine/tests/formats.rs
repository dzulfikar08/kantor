//! Opening and saving CSV / TSV, and importing ODS / XLS fixtures.

use std::fs;
use std::path::PathBuf;

use sheets_engine::{SourceFormat, Workbook};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn csv_opens_clean_with_typed_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("people.csv");
    fs::write(
        &path,
        "\u{feff}name;score;note\r\n\"Doe; Jane\";12.5;=1+1\r\nBob;7;'quoted\r\n",
    )
    .unwrap();

    let wb = Workbook::open(&path).unwrap();
    assert!(!wb.is_dirty());
    assert!(!wb.can_undo());
    assert_eq!(wb.source_format(), SourceFormat::Csv { delimiter: b';' });
    assert_eq!(wb.path(), Some(path.as_path()));
    assert_eq!(wb.title(), "people.csv");
    assert_eq!(wb.sheet_names(), vec!["people".to_owned()]);
    assert_eq!(wb.used_size(0), (3, 3));
    assert_eq!(wb.display_value(0, 0, 0), "name", "BOM stripped");
    assert_eq!(wb.display_value(0, 1, 0), "Doe; Jane");
    // Numbers are numbers: a formula can add them.
    let mut wb = wb;
    wb.set_input(0, 3, 1, "=B2+B3").unwrap();
    assert_eq!(wb.display_value(0, 3, 1), "19.5");
    // A leading `=` stays text, never evaluated.
    assert_eq!(wb.display_value(0, 1, 2), "=1+1");
    assert_eq!(wb.display_value(0, 2, 2), "'quoted");
}

#[test]
fn csv_round_trip_keeps_delimiter_and_quotes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rt.csv");
    fs::write(&path, "a;b\n\"x;y\";\"multi\nline\"\n1;2\n").unwrap();

    let mut wb = Workbook::open(&path).unwrap();
    wb.set_input(0, 2, 1, "=A3*10").unwrap();
    assert!(wb.is_dirty());
    wb.save().unwrap();
    assert!(!wb.is_dirty());

    let text = fs::read_to_string(&path).unwrap();
    assert_eq!(text, "a;b\r\n\"x;y\";\"multi\nline\"\r\n1;10\r\n");

    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.display_value(0, 1, 1), "multi\nline");
    assert_eq!(back.display_value(0, 2, 1), "10");
}

#[test]
fn tsv_and_new_workbook_to_csv() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "Item").unwrap();
    wb.set_input(0, 0, 1, "with, comma").unwrap();
    wb.set_input(0, 1, 0, "3").unwrap();
    wb.set_input(0, 1, 1, "=A2/4").unwrap();

    let csv = dir.path().join("out.csv");
    wb.save_as(&csv).unwrap();
    assert_eq!(
        fs::read_to_string(&csv).unwrap(),
        "Item,\"with, comma\"\r\n3,0.75\r\n"
    );
    assert_eq!(wb.source_format(), SourceFormat::Csv { delimiter: b',' });

    let tsv = dir.path().join("out.tsv");
    wb.save_as(&tsv).unwrap();
    assert_eq!(
        fs::read_to_string(&tsv).unwrap(),
        "Item\twith, comma\r\n3\t0.75\r\n"
    );
    let back = Workbook::open(&tsv).unwrap();
    assert_eq!(back.display_value(0, 0, 1), "with, comma");
}

#[test]
fn csv_save_writes_only_the_active_sheet() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture("simple.ods")).unwrap();
    assert_eq!(wb.sheets_lost_in_csv(), 1);
    assert_eq!(wb.active_sheet(), 0);
    wb.set_active_sheet(1).unwrap();
    assert!(!wb.is_dirty(), "switching sheets is not an edit");
    let path = dir.path().join("other.csv");
    wb.save_as(&path).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "8\r\n");
}

#[test]
fn refuses_to_save_foreign_formats() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::new_empty().unwrap();
    assert!(wb.save_as(dir.path().join("x.ods")).is_err());
    assert!(wb.save_as(dir.path().join("x.xls")).is_err());
    assert!(!dir.path().join("x.ods").exists());
}

fn check_imported(name: &str, format: SourceFormat) {
    let src = fixture(name);
    let mut wb = Workbook::open(&src).unwrap();
    assert_eq!(wb.source_format(), format);
    assert!(wb.source_format().is_imported());
    assert_eq!(wb.path(), None, "Save must go to Save As");
    assert_eq!(wb.source_path(), Some(src.as_path()));
    assert_eq!(wb.title(), name);
    assert_eq!(wb.suggested_file_name(), "simple.xlsx");
    assert!(!wb.is_dirty());
    assert!(!wb.can_undo());
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());

    assert_eq!(
        wb.sheet_names(),
        vec!["Data".to_owned(), "Other".to_owned()]
    );
    assert_eq!(wb.display_value(0, 0, 2), "When");
    assert_eq!(wb.display_value(0, 1, 0), "Apples");
    assert_eq!(wb.display_value(0, 1, 2), "2024-01-15");
    assert_eq!(wb.display_value(0, 3, 0), "=text");
    // Formulas came over as formulas and recalculate.
    assert_eq!(wb.input(0, 3, 1), "=SUM(B2:B3)");
    assert_eq!(wb.display_value(0, 3, 1), "7");
    assert_eq!(wb.display_value(0, 3, 2), "14");
    assert_eq!(wb.input(1, 0, 0), "=Data!B4+1");
    assert_eq!(wb.display_value(1, 0, 0), "8");
    wb.set_input(0, 1, 1, "10").unwrap();
    assert_eq!(wb.display_value(1, 0, 0), "15");

    // Save without a path is refused; Save As to xlsx works and re-opens.
    assert!(wb.save().is_err());
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join(wb.suggested_file_name());
    wb.save_as(&out).unwrap();
    assert_eq!(wb.path(), Some(out.as_path()));
    assert_eq!(wb.source_format(), SourceFormat::Xlsx);
    let back = Workbook::open(&out).unwrap();
    assert_eq!(back.display_value(1, 0, 0), "15");
}

#[test]
fn imports_ods() {
    check_imported("simple.ods", SourceFormat::Ods);
}

#[test]
fn imports_xls() {
    check_imported("simple.xls", SourceFormat::Xls);
}

#[test]
fn opens_by_extension_only() {
    assert!(Workbook::open("notes.txt").is_err());
    assert!(Workbook::open("missing.ods").is_err());
    assert!(Workbook::open("missing.csv").is_err());
}

#[test]
fn unreproducible_formulas_keep_cached_values() {
    let wb = Workbook::open(fixture("fallback.ods")).unwrap();
    // Unknown function: the file's result, not #NAME?.
    assert_eq!(wb.display_value(0, 0, 1), "42");
    assert_eq!(wb.input(0, 0, 1), "42");
    // Cached 999 disagrees with =A1*100: trust the file.
    assert_eq!(wb.display_value(0, 0, 2), "999");
    // A formula that reproduces its cached result stays a formula.
    assert_eq!(wb.input(0, 0, 3), "=A1+1");
    assert_eq!(wb.import_notes(), ["2 formulas kept as values"]);
    assert!(!wb.is_dirty());
}
