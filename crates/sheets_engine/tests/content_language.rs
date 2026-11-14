//! New content in the interface language: a pivot table made with a
//! translator set gets its captions in that language (written into the
//! file), while pivot tables read from a file keep theirs. Its own test
//! binary, as the translator is set once per process.
//!
//! Only the labels the file has an attribute for stay translated once the
//! file is reopened and refreshed (see the table in `sheets_engine::pivot`).

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::path::Path;

use pivot_check::{parts, pivot_problems};
use sheets_engine::{AreaEntry, PivotDestination, SubtotalPlace, Workbook};

fn indonesian(en: &'static str) -> &'static str {
    match en {
        "Row Labels" => "Label Baris",
        "Column Labels" => "Label Kolom",
        "Grand Total" => "Total Keseluruhan",
        "Values" => "Nilai",
        "{} of {}" => "{} dari {}",
        "Sum" => "Jumlah",
        "{} Total" => "Total {}",
        "(blank)" => "(kosong)",
        "(All)" => "(Semua)",
        other => other,
    }
}

fn data_book() -> Workbook {
    book(&[
        ["Region", "Product", "Revenue"],
        ["East", "Apples", "120"],
        ["West", "Pears", "60"],
        ["East", "Pears", "45"],
    ])
}

fn book(rows: &[[&str; 3]]) -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let cells: Vec<(u32, u32, String)> = rows
        .iter()
        .enumerate()
        .flat_map(|(r, row)| {
            row.iter()
                .enumerate()
                .map(move |(c, v)| (r as u32, c as u32, v.to_string()))
        })
        .collect();
    wb.set_inputs(0, &cells).unwrap();
    wb
}

fn grid(wb: &Workbook, sheet: u32, (t, l, b, r): (u32, u32, u32, u32)) -> Vec<Vec<String>> {
    (t..=b)
        .map(|row| (l..=r).map(|c| wb.display_value(sheet, row, c)).collect())
        .collect()
}

#[test]
fn new_pivot_tables_follow_the_translator_and_file_ones_keep_english() {
    sheets_engine::set_content_translator(indonesian);
    let mut wb = data_book();
    let (s, row, col) = wb
        .insert_pivot("Sheet1!$A$1:$C$4", 0, PivotDestination::NewSheet)
        .unwrap();
    let mut l = wb.pivot_layout(s, row, col).unwrap();
    let f = |l: &sheets_engine::PivotLayout, n: &str| {
        l.fields.iter().position(|f| f.name == n).unwrap()
    };
    l.toggle_field(f(&l, "Region"));
    l.toggle_field(f(&l, "Revenue"));
    assert_eq!(l.value_caption(0), "Jumlah dari Revenue");
    wb.set_pivot_layout(s, row, col, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (2, 0, 5, 1)),
        [
            ["Label Baris", "Jumlah dari Revenue"],
            ["East", "165"],
            ["West", "60"],
            ["Total Keseluruhan", "225"],
        ]
    );
    let mut l = wb.pivot_layout(s, row, col).unwrap();
    l.move_entry(AreaEntry::Field(f(&l, "Product")), "Columns", 0);
    wb.set_pivot_layout(s, row, col, &l).unwrap();
    assert_eq!(wb.display_value(s, 2, 1), "Label Kolom");

    // The captions are in the file, so they read back the same.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pivot.xlsx");
    wb.save_as(&path).unwrap();
    let mut back = Workbook::open(&path).unwrap();
    back.refresh_all_pivots().unwrap();
    assert_eq!(back.display_value(s, 3, 0), "Label Baris");
    assert_eq!(back.display_value(s, 2, 1), "Label Kolom");

    // A file's pivot tables keep the captions Excel wrote.
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/21_pivot_tables.xlsx");
    let mut book = Workbook::open(&fixture).unwrap();
    book.refresh_all_pivots().unwrap();
    for sheet in 0..book.sheet_names().len() as u32 {
        for p in book.pivot_tables(sheet) {
            let a = p.area;
            for r in grid(&book, sheet, (a.top, a.left, a.bottom, a.right)) {
                for v in r {
                    assert!(
                        !["Label Baris", "Total Keseluruhan", "Jumlah dari", "Nilai"]
                            .iter()
                            .any(|id| v.contains(id)),
                        "{v:?} in a file's pivot table"
                    );
                }
            }
        }
    }
}

/// Every label in the pivot tables of `sheet`, report filters included.
fn pivot_text(wb: &Workbook, sheet: u32) -> Vec<String> {
    wb.pivot_tables(sheet)
        .iter()
        .flat_map(|p| {
            let a = p.bounds;
            grid(wb, sheet, (a.top, a.left, a.bottom, a.right))
        })
        .flatten()
        .filter(|s| !s.is_empty())
        .collect()
}

/// The pivot table definition parts of a package.
fn pivot_defs(path: &Path) -> Vec<(String, String)> {
    parts(path)
        .into_iter()
        .filter(|(n, _)| n.starts_with("xl/pivotTables/pivotTable"))
        .collect()
}

/// Every part of `b` as in `a`, but the save time.
fn assert_same_package(a: &Path, b: &Path) {
    let (a, b) = (parts(a), parts(b));
    assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>());
    for (name, text) in &a {
        // docProps/core.xml holds the save time, which may tick over between the two saves.
        if name == "docProps/core.xml" {
            continue;
        }
        assert_eq!(Some(text), b.get(name), "{name} changed");
    }
}

#[test]
fn captions_with_an_attribute_stay_translated_after_reopening() {
    sheets_engine::set_content_translator(indonesian);
    let mut wb = book(&[
        ["Region", "Product", "Revenue"],
        ["East", "Apples", "120"],
        ["West", "Pears", "60"],
        ["East", "Pears", "45"],
        ["", "Pears", "10"],
    ]);
    let (s, row, col) = wb
        .insert_pivot("Sheet1!$A$1:$C$5", 0, PivotDestination::NewSheet)
        .unwrap();
    let mut l = wb.pivot_layout(s, row, col).unwrap();
    let f = |l: &sheets_engine::PivotLayout, n: &str| {
        l.fields.iter().position(|f| f.name == n).unwrap()
    };
    for name in ["Region", "Product", "Revenue"] {
        l.toggle_field(f(&l, name));
    }
    l.subtotals = SubtotalPlace::Bottom;
    wb.set_pivot_layout(s, row, col, &l).unwrap();
    let made = pivot_text(&wb, s);
    for label in ["Label Baris", "Total Keseluruhan", "(kosong)", "Total East"] {
        assert!(made.contains(&label.to_owned()), "{label} not in {made:?}");
    }

    let dir = tempfile::tempdir().unwrap();
    let saved = dir.path().join("made.xlsx");
    wb.save_as(&saved).unwrap();
    let defs = pivot_defs(&saved);
    assert_eq!(defs.len(), 1);
    let xml = &defs[0].1;
    for attr in [
        r#"rowHeaderCaption="Label Baris""#,
        r#"grandTotalCaption="Total Keseluruhan""#,
        r#"dataCaption="Nilai""#,
        r#"n="(kosong)""#,
    ] {
        assert!(xml.contains(attr), "{attr} not in {xml}");
    }
    // The file format has no pattern for "Total East".
    assert!(!xml.contains("subtotalCaption"), "{xml}");
    assert!(pivot_problems(&parts(&saved)).is_empty());

    // Reopened and refreshed: what has an attribute stays Indonesian; the
    // subtotals are Excel's English, as Excel would show them.
    let mut back = Workbook::open(&saved).unwrap();
    back.refresh_all_pivots().unwrap();
    let reopened = pivot_text(&back, s);
    for label in ["Label Baris", "Total Keseluruhan", "(kosong)", "East Total"] {
        assert!(
            reopened.contains(&label.to_owned()),
            "{label} not in {reopened:?}"
        );
    }
    assert!(
        !reopened
            .iter()
            .any(|v| v == "(blank)" || v == "Grand Total")
    );

    // save → refresh → save: the second save is the first.
    let again = dir.path().join("again.xlsx");
    wb.refresh_all_pivots().unwrap();
    wb.save_as(&again).unwrap();
    assert_same_package(&saved, &again);
    // Likewise for the reopened file.
    let reopened_saved = dir.path().join("reopened.xlsx");
    back.save_as(&reopened_saved).unwrap();
    let reopened_again = dir.path().join("reopened-again.xlsx");
    back.refresh_all_pivots().unwrap();
    back.save_as(&reopened_again).unwrap();
    assert_same_package(&reopened_saved, &reopened_again);
}

#[test]
fn file_pivot_tables_get_no_new_captions() {
    sheets_engine::set_content_translator(indonesian);
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/21_pivot_tables.xlsx");
    let mut book = Workbook::open(&fixture).unwrap();
    book.refresh_all_pivots().unwrap();
    let english: Vec<String> = (0..book.sheet_names().len() as u32)
        .flat_map(|s| pivot_text(&book, s))
        .collect();
    assert!(english.iter().any(|v| v == "Grand Total"), "{english:?}");
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corpus.xlsx");
    book.save_as(&path).unwrap();
    let (before, after) = (pivot_defs(&fixture), pivot_defs(&path));
    assert_eq!(before.len(), after.len());
    for attr in [
        "rowHeaderCaption",
        "colHeaderCaption",
        "grandTotalCaption",
        "subtotalCaption",
        "missingCaption",
        "errorCaption",
        " n=\"",
    ] {
        let count = |defs: &[(String, String)]| {
            defs.iter()
                .map(|(_, x)| x.matches(attr).count())
                .sum::<usize>()
        };
        assert_eq!(count(&before), count(&after), "{attr}");
    }
}
