//! Shared helpers for the object-model tests: run a module headless, read
//! cells, save and reopen a workbook.
#![allow(dead_code)]

use sheets_engine::Workbook;
use sheets_engine::vba::ModuleKind;
use sheets_script::run::ModuleSource;
use sheets_script::{
    HeadlessUi, MacroError, MacroProject, RunOptions, RunOutcome, View, run_headless,
};

pub fn project(code: &str) -> MacroProject {
    MacroProject {
        modules: vec![ModuleSource::new(
            "Module1",
            ModuleKind::Standard,
            &code.replace('\n', "\r\n"),
        )],
    }
}

/// Runs `name` from `code`; returns the workbook, the outcome and the
/// `Debug.Print` lines (trimmed).
pub fn run_printed(book: Workbook, code: &str, name: &str) -> (Workbook, RunOutcome, Vec<String>) {
    let ui = HeadlessUi::new(Vec::new(), false);
    let printed = ui.printed.clone();
    let (book, outcome) = run_headless(
        book,
        &project(code),
        name,
        View::default(),
        ui,
        &RunOptions {
            step_limit: Some(50_000_000),
            ..Default::default()
        },
    );
    // `Debug.Print 3` prints " 3 " (VBA pads numbers); the tests compare
    // trimmed lines.
    let printed = printed
        .borrow()
        .iter()
        .map(|l| l.trim().to_owned())
        .collect();
    (book, outcome, printed)
}

pub fn run(book: Workbook, code: &str, name: &str) -> (Workbook, RunOutcome) {
    let (book, outcome, _) = run_printed(book, code, name);
    (book, outcome)
}

pub fn ok(outcome: &RunOutcome) {
    if let Err(e) = &outcome.result {
        panic!("macro failed: {e}");
    }
}

pub fn err(outcome: &RunOutcome) -> &MacroError {
    outcome.result.as_ref().expect_err("the macro should fail")
}

pub fn cell(book: &Workbook, sheet: u32, a1: &str) -> String {
    let (r, c) = sheets_script::addr::parse_cell(a1).unwrap();
    book.display_value(sheet, r, c)
}

pub fn input(book: &Workbook, sheet: u32, a1: &str) -> String {
    let (r, c) = sheets_script::addr::parse_cell(a1).unwrap();
    book.input(sheet, r, c)
}

pub fn set(book: &mut Workbook, sheet: u32, a1: &str, v: &str) {
    let (r, c) = sheets_script::addr::parse_cell(a1).unwrap();
    book.set_input(sheet, r, c, v).unwrap();
}

/// A small table on Sheet1: Name, Region, Amount (A1:C5).
pub fn sales() -> Workbook {
    let mut b = Workbook::new_empty().unwrap();
    let rows = [
        ["Name", "Region", "Amount"],
        ["Dewi", "East", "300"],
        ["Budi", "West", "150"],
        ["Andi", "East", "200"],
        ["Citra", "North", "50"],
    ];
    for (i, row) in rows.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            b.set_input(0, i as u32, j as u32, v).unwrap();
        }
    }
    b
}

/// Saves `book` as `file` (e.g. `"t.xlsx"`) in a fresh temporary folder and
/// opens the saved file again. The folder is returned so it lives as long
/// as the caller needs the file.
pub fn reopen(mut book: Workbook, file: &str) -> (Workbook, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(file);
    book.save_as(&path).unwrap();
    (Workbook::open(&path).unwrap(), dir)
}

/// One entry of a saved package as text.
pub fn package_text(path: &std::path::Path, entry: &str) -> Option<String> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut z = zip::ZipArchive::new(f).ok()?;
    let mut e = z.by_name(entry).ok()?;
    let mut s = String::new();
    e.read_to_string(&mut s).ok()?;
    Some(s)
}
