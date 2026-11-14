//! New content in the interface language: a pivot table made with a
//! translator set gets its captions in that language (written into the
//! file), while pivot tables read from a file keep theirs. Its own test
//! binary, as the translator is set once per process.

use std::path::Path;

use sheets_engine::{AreaEntry, PivotDestination, Workbook};

fn indonesian(en: &'static str) -> &'static str {
    match en {
        "Row Labels" => "Label Baris",
        "Column Labels" => "Label Kolom",
        "Grand Total" => "Total Keseluruhan",
        "Values" => "Nilai",
        "{} of {}" => "{} dari {}",
        "Sum" => "Jumlah",
        "{} Total" => "Total {}",
        other => other,
    }
}

fn data_book() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let rows = [
        ["Region", "Product", "Revenue"],
        ["East", "Apples", "120"],
        ["West", "Pears", "60"],
        ["East", "Pears", "45"],
    ];
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
