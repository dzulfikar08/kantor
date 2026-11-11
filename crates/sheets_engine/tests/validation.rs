//! Data validation: rules read from the file, list sources, checks of typed
//! values, moves with rows and columns, and round trips. Fixture:
//! `tests/corpus/xlsx/19_data_validation.xlsx` (`scripts/corpus/gen_validation_xlsx.py`).

use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::{ErrorStyle, ValidationOperator, ValidationType, Workbook};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/19_data_validation.xlsx")
}

fn part(path: &Path, name: &str) -> String {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut s = String::new();
    zip.by_name(name).unwrap().read_to_string(&mut s).unwrap();
    s
}

/// `<tag ...>...</tag>` (the first one) in `xml`.
fn element<'a>(xml: &'a str, open: &str, close: &str) -> &'a str {
    let a = xml
        .find(open)
        .unwrap_or_else(|| panic!("no {open} in {xml}"));
    let b = a + xml[a..].find(close).unwrap() + close.len();
    &xml[a..b]
}

/// 0-based column of a letter.
fn col(c: char) -> u32 {
    c as u32 - 'A' as u32
}

fn items(wb: &mut Workbook, row: u32, c: char) -> Vec<String> {
    wb.validation_list(0, row, col(c))
        .unwrap_or_else(|| panic!("{c}{} has no list", row + 1))
        .into_iter()
        .map(|i| i.text)
        .collect()
}

/// Type `input` into the cell and check it like the grid does: whether the
/// stored value passes its validation.
fn valid(wb: &mut Workbook, row: u32, c: char, input: &str) -> bool {
    wb.set_input(0, row, col(c), input).unwrap();
    let ok = wb.validate_cell(0, row, col(c)).is_none();
    wb.revert_edit().unwrap();
    ok
}

#[test]
fn reads_rules_and_messages() {
    let mut wb = Workbook::open(fixture()).unwrap();
    assert!(wb.has_validations(0));
    assert!(!wb.has_validations(1));
    let a = wb.validation(0, 1, 0).unwrap();
    assert_eq!(a.kind, ValidationType::List);
    assert!(a.dropdown && a.allow_blank);
    let p = a.prompt.as_ref().unwrap();
    assert_eq!(
        (p.title.as_str(), p.text.as_str()),
        ("Status", "Pick a status from the list.")
    );
    let (style, msg) = a.error.as_ref().unwrap();
    assert_eq!(*style, ErrorStyle::Stop);
    assert_eq!(msg.title, "Not a status");
    // Headers have none; the range ends at row 6.
    assert!(wb.validation(0, 0, 0).is_none());
    assert!(wb.validation(0, 6, 0).is_none());
    let e = wb.validation(0, 1, col('E')).unwrap();
    assert_eq!(e.kind, ValidationType::Whole);
    assert_eq!(e.error.as_ref().unwrap().0, ErrorStyle::Warning);
    assert_eq!(
        e.error.as_ref().unwrap().1.text,
        "",
        "no text: the default message"
    );
    let f = wb.validation(0, 1, col('F')).unwrap();
    assert_eq!(f.operator, ValidationOperator::GreaterThan);
    assert_eq!(f.error.as_ref().unwrap().0, ErrorStyle::Information);
    // showDropDown="1" hides the button.
    assert!(!wb.validation(0, 1, col('M')).unwrap().dropdown);
    // "Any value" with only an input message.
    let q = wb.validation(0, 1, col('Q')).unwrap();
    assert_eq!(q.kind, ValidationType::Any);
    assert_eq!(q.prompt.as_ref().unwrap().text, "Anything goes.");
    assert!(wb.validation_list(0, 1, col('Q')).is_none());
}

#[test]
fn list_sources() {
    let mut wb = Workbook::open(fixture()).unwrap();
    assert_eq!(items(&mut wb, 1, 'A'), ["Open", "Closed", "Pending"]);
    assert_eq!(items(&mut wb, 1, 'B'), ["Red", "Green", "Blue"]);
    // On another sheet (the x14 form), through a defined name, INDIRECT, OFFSET.
    let fruit = ["Apple", "Pear", "Plum", "Fig"];
    assert_eq!(items(&mut wb, 1, 'C'), fruit);
    assert_eq!(items(&mut wb, 4, 'D'), fruit);
    assert_eq!(items(&mut wb, 1, 'N'), ["Apple", "Pear"]);
    assert_eq!(items(&mut wb, 1, 'P'), ["Pear", "Plum"]);
    assert_eq!(items(&mut wb, 1, 'M'), ["x", "y"]);
    // Sources follow the cells they read.
    wb.set_input(0, 1, col('J'), "3").unwrap(); // J2: INDIRECT's length
    assert_eq!(items(&mut wb, 1, 'N'), ["Apple", "Pear", "Plum"]);
    wb.set_input(0, 1, col('H'), "Teal").unwrap();
    assert_eq!(items(&mut wb, 1, 'B'), ["Red", "Teal", "Blue"]);
    wb.set_input(1, 0, 0, "Apricot").unwrap();
    assert_eq!(items(&mut wb, 1, 'C')[0], "Apricot");
    // A value that would read as a number stays text when picked.
    wb.set_input(0, 2, col('H'), "'007").unwrap();
    let list = wb.validation_list(0, 1, col('B')).unwrap();
    assert_eq!(list[2].text, "007");
    assert_eq!(list[2].input, "'007");
}

#[test]
fn checks_typed_values() {
    let mut wb = Workbook::open(fixture()).unwrap();
    let r = 1; // row 2
    // Lists: case-insensitive, blank allowed.
    assert!(valid(&mut wb, r, 'A', "Closed"));
    assert!(valid(&mut wb, r, 'A', "closed"));
    assert!(!valid(&mut wb, r, 'A', "Nope"));
    assert!(valid(&mut wb, r, 'A', ""));
    assert!(valid(&mut wb, r, 'B', "Green"));
    assert!(!valid(&mut wb, r, 'B', "Pink"));
    assert!(valid(&mut wb, r, 'C', "Fig"));
    assert!(!valid(&mut wb, r, 'C', "Kiwi"));
    assert!(valid(&mut wb, r, 'D', "Plum"));
    assert!(!valid(&mut wb, r, 'N', "Plum"), "INDIRECT gives two items");
    // Whole number between 1 and 10.
    assert!(valid(&mut wb, r, 'E', "1"));
    assert!(valid(&mut wb, r, 'E', "10"));
    assert!(!valid(&mut wb, r, 'E', "11"));
    assert!(!valid(&mut wb, r, 'E', "2.5"));
    assert!(!valid(&mut wb, r, 'E', "abc"));
    // A formula is checked by its value.
    assert!(valid(&mut wb, r, 'E', "=2*3"));
    assert!(!valid(&mut wb, r, 'E', "=2*30"));
    // Decimal greater than $J$1 (2.5).
    assert!(valid(&mut wb, r, 'F', "2.6"));
    assert!(!valid(&mut wb, r, 'F', "2.5"));
    // Dates in 2024 (typed as a date or as a serial).
    assert!(valid(&mut wb, r, 'G', "2024-06-01"));
    assert!(valid(&mut wb, r, 'G', "45300"));
    assert!(!valid(&mut wb, r, 'G', "2025-01-01"));
    // Times before noon.
    assert!(valid(&mut wb, r, 'I', "0.25"));
    // (IronCalc reads a typed "9:30" as text, so times go in as formulas.)
    assert!(valid(&mut wb, r, 'I', "=TIME(9,30,0)"));
    assert!(!valid(&mut wb, r, 'I', "=TIME(15,0,0)"));
    assert!(!valid(&mut wb, r, 'I', "9:30"), "text is not a time");
    // At most 5 characters, numbers by their digits.
    assert!(valid(&mut wb, r, 'K', "hello"));
    assert!(!valid(&mut wb, r, 'K', "hello!"));
    assert!(valid(&mut wb, r, 'K', "12345"));
    assert!(!valid(&mut wb, r, 'K', "123456"));
    // Custom: MOD(L2,2)=0, relative to each cell.
    assert!(valid(&mut wb, r, 'L', "4"));
    assert!(!valid(&mut wb, r, 'L', "3"));
    assert!(valid(&mut wb, 4, 'L', "6"));
    assert!(!valid(&mut wb, 4, 'L', "7"));
    // Not between 1 and 5.
    assert!(!valid(&mut wb, r, 'O', "3"));
    assert!(valid(&mut wb, r, 'O', "7"));
    // Any value.
    assert!(valid(&mut wb, r, 'Q', "whatever"));
    // Outside every rule.
    assert!(valid(&mut wb, 10, 'A', "Nope"));
}

#[test]
fn invalid_cells_and_revert() {
    let mut wb = Workbook::open(fixture()).unwrap();
    assert!(wb.invalid_cell(0, 2, col('A')), "A3 holds Bogus");
    assert!(wb.invalid_cell(0, 2, col('E')), "E3 holds 42");
    assert!(wb.invalid_cell(0, 2, col('L')), "L3 holds 3");
    assert!(!wb.invalid_cell(0, 1, col('A')), "blank");
    assert!(!wb.invalid_cell(0, 0, col('A')), "header, no rule");
    // A rejected value is taken back for good: no redo, not dirty.
    wb.set_input(0, 1, col('E'), "11").unwrap();
    assert!(wb.validate_cell(0, 1, col('E')).is_some());
    wb.revert_edit().unwrap();
    assert_eq!(wb.input(0, 1, col('E')), "");
    assert!(!wb.can_redo());
    assert!(!wb.is_dirty());
}

#[test]
fn rules_follow_rows_and_columns() {
    let mut wb = Workbook::open(fixture()).unwrap();
    wb.insert_rows(0, 1, 2).unwrap(); // before row 2
    assert!(wb.validation(0, 1, 0).is_none());
    assert!(wb.validation(0, 3, 0).is_some());
    assert!(wb.validation(0, 7, 0).is_some(), "A2:A6 is now A4:A8");
    assert!(wb.validation(0, 8, 0).is_none());
    // The custom rule's relative reference moved with it.
    assert!(wb.invalid_cell(0, 4, col('L')), "L3 (now L5) still holds 3");
    wb.set_input(0, 3, col('L'), "8").unwrap();
    assert!(wb.validate_cell(0, 3, col('L')).is_none());
    // The range source grew around the inserted rows (H1:H3 -> H1:H5).
    assert_eq!(items(&mut wb, 3, 'B'), ["Red", "Green", "Blue"]);
    wb.undo().unwrap();
    wb.undo().unwrap();
    assert!(wb.validation(0, 1, 0).is_some());

    // Deleting column A takes its rule; B's moves into A.
    wb.delete_columns(0, 0, 0).unwrap();
    assert_eq!(items(&mut wb, 1, 'A'), ["Red", "Green", "Blue"]);
    assert_eq!(
        wb.validation(0, 1, 0).unwrap().formula1.as_deref(),
        Some("$G$1:$G$3")
    );
}

#[test]
fn round_trip_keeps_validations_as_they_were() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let saved = dir.path().join("a.xlsx");
    wb.save_as(&saved).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let (orig, new) = (
        part(&fixture(), "xl/worksheets/sheet1.xml"),
        part(&saved, "xl/worksheets/sheet1.xml"),
    );
    let dv = |x| element(x, "<dataValidations", "</dataValidations>").to_owned();
    assert_eq!(dv(&orig), dv(&new), "unchanged, byte for byte");
    let ext = |x| element(x, "<x14:dataValidations", "</x14:dataValidations>").to_owned();
    assert_eq!(ext(&orig), ext(&new));

    // The saved file reads back the same, and its rules still work.
    let mut again = Workbook::open(&saved).unwrap();
    assert_eq!(items(&mut again, 1, 'C'), ["Apple", "Pear", "Plum", "Fig"]);
    assert_eq!(
        again.validation(0, 1, col('E')),
        Workbook::open(fixture())
            .unwrap()
            .validation(0, 1, col('E'))
    );
}

#[test]
fn saved_rules_follow_rows_and_columns() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    wb.insert_rows(0, 1, 2).unwrap();
    wb.insert_columns(0, 0, 1).unwrap(); // before A
    let saved = dir.path().join("a.xlsx");
    wb.save_as(&saved).unwrap();
    let x = part(&saved, "xl/worksheets/sheet1.xml");
    assert!(x.contains(r#"sqref="B4:B8""#), "{x}");
    assert!(x.contains("<formula1>$I$1:$I$5</formula1>"), "{x}");
    assert!(x.contains("<formula1>MOD(M4,2)=0</formula1>"), "{x}");
    assert!(x.contains("<xm:sqref>D4:D8</xm:sqref>"), "{x}");
    // What the grid shows agrees with what was saved, before and after
    // reopening (the opened file is now the base the rules move from).
    let before = wb.validation(0, 3, 1);
    let mut again = Workbook::open(&saved).unwrap();
    assert_eq!(again.validation(0, 3, 1), before);
    assert!(again.invalid_cell(0, 4, col('M')));
    wb.insert_rows(0, 0, 1).unwrap();
    assert!(wb.validation(0, 4, 1).is_some() && wb.validation(0, 3, 1).is_none());
}
