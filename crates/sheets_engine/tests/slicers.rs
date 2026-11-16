//! Slicers and timelines (file 25 of the corpus, written by
//! `scripts/corpus/gen_slicers_xlsx.py`): shown as drawings, a pivot slicer
//! and a table slicer filtering as in Excel, their state saved back, across
//! saves of one session.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pivot_check::{parts, pivot_problems};
use sheets_engine::Workbook;
use sheets_engine::drawing::DrawingKind;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/25_slicers.xlsx")
}

fn save(wb: &mut Workbook, dir: &Path, name: &str) -> (PathBuf, BTreeMap<String, String>) {
    let path = dir.join(name);
    wb.save_as(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        sheets_engine::package_problems(&bytes),
        Vec::<String>::new()
    );
    let p = parts(&path);
    assert_eq!(pivot_problems(&p), Vec::<String>::new());
    if let Ok(dir) = std::env::var("KANTOR_PIVOT_SAMPLES") {
        std::fs::copy(&path, Path::new(&dir).join(name)).unwrap();
    }
    (path, p)
}

/// Data is sheet 0 (sheetId 1), Report sheet 1 (sheetId 2).
const DATA: u32 = 1;
const REPORT: u32 = 2;

fn selected(wb: &Workbook, sheet_id: u32, name: &str) -> Vec<(String, bool, bool)> {
    wb.slicer_view(sheet_id, name)
        .unwrap()
        .items
        .into_iter()
        .map(|i| (i.label, i.selected, i.no_data))
        .collect()
}

fn items(v: &[(&str, bool, bool)]) -> Vec<(String, bool, bool)> {
    v.iter().map(|(l, s, n)| (l.to_string(), *s, *n)).collect()
}

/// Report!A3: Row Labels / Apples / Pears / Plums / Grand Total.
fn pivot_values(wb: &Workbook) -> Vec<String> {
    (3..8).map(|r| wb.display_value(1, r, 1)).collect()
}

#[test]
fn slicers_and_timelines_are_drawings() {
    let wb = Workbook::open(fixture()).unwrap();
    let kinds = |sheet: u32| -> Vec<String> {
        wb.drawings(sheet)
            .iter()
            .map(|d| match &*d.kind {
                DrawingKind::Slicer { name } => format!("slicer {name}"),
                DrawingKind::Timeline { name } => format!("timeline {name}"),
                k => format!("{k:?}"),
            })
            .collect()
    };
    assert_eq!(kinds(0), ["slicer Product"]);
    assert_eq!(kinds(1), ["slicer Region", "timeline NativeTimeline_Date"]);
    assert!(
        wb.drawings(1).iter().all(|d| d.editable),
        "they can be moved"
    );
    assert_eq!(wb.slicer_names(REPORT), ["Region"]);
    let t = wb.timeline_view(REPORT, "NativeTimeline_Date").unwrap();
    assert_eq!(t.caption, "Date");
    assert_eq!(t.label.as_deref(), Some("All Periods"));
    assert_eq!(t.level.as_deref(), Some("MONTHS"));
    assert_eq!(t.periods.len(), 12, "Jan to Dec 2026");
    assert_eq!(t.periods[0].label, "Jan");
    assert_eq!(t.periods[0].group.as_deref(), Some("2026"));
    assert!(t.periods.iter().all(|p| !p.selected));
}

#[test]
fn a_pivot_slicer_filters_its_pivot_table() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    assert_eq!(
        selected(&wb, REPORT, "Region"),
        items(&[
            ("East", true, false),
            ("North", true, false),
            ("West", true, false)
        ])
    );
    let v = wb.slicer_view(REPORT, "Region").unwrap();
    assert!(!v.filtered);
    assert_eq!(v.problem, None);
    assert_eq!(pivot_values(&wb), ["588", "210", "270", "1068", ""]);
    // East only: Region is on no axis of the pivot table, and filters it.
    wb.slicer_select(REPORT, "Region", &[true, false, false])
        .unwrap();
    assert_eq!(pivot_values(&wb), ["228", "45", "144", "417", ""]);
    assert!(wb.slicer_view(REPORT, "Region").unwrap().filtered);
    assert!(
        wb.slicer_select(REPORT, "Region", &[false, false, false])
            .is_err(),
        "one item at least"
    );
    // Saved as Excel saves it: the cache's items and the pivot field's.
    let (path, p) = save(&mut wb, dir.path(), "slicer-east.xlsx");
    let cache = &p["xl/slicerCaches/slicerCache2.xml"];
    assert!(
        cache.contains(r#"<items count="3"><i x="0" s="1"/><i x="1"/><i x="2"/></items>"#),
        "{cache}"
    );
    assert!(p["xl/pivotTables/pivotTable1.xml"].contains("h=\"1\""));
    let back = Workbook::open(&path).unwrap();
    assert_eq!(
        selected(&back, REPORT, "Region"),
        items(&[
            ("East", true, false),
            ("North", false, false),
            ("West", false, false)
        ])
    );
    assert_eq!(pivot_values(&back), ["228", "45", "144", "417", ""]);
    // Two items, then clear; save → undo → save puts East only back.
    wb.slicer_select(REPORT, "Region", &[true, true, false])
        .unwrap();
    assert_eq!(pivot_values(&wb)[3], "813");
    wb.slicer_select(REPORT, "Region", &[true, true, true])
        .unwrap();
    assert_eq!(pivot_values(&wb)[3], "1068");
    let (_, p2) = save(&mut wb, dir.path(), "slicer-all.xlsx");
    assert!(p2["xl/slicerCaches/slicerCache2.xml"].contains(r#"<i x="1" s="1"/>"#));
    assert!(wb.undo().unwrap());
    assert!(wb.undo().unwrap());
    assert_eq!(pivot_values(&wb)[3], "417");
    let (_, p3) = save(&mut wb, dir.path(), "slicer-undone.xlsx");
    assert!(
        p3["xl/slicerCaches/slicerCache2.xml"].contains(r#"<i x="0" s="1"/><i x="1"/><i x="2"/>"#)
    );
    // The other parts are carried: the table slicer's cache as it was.
    let orig = parts(&fixture());
    assert_eq!(
        p3["xl/slicerCaches/slicerCache1.xml"],
        orig["xl/slicerCaches/slicerCache1.xml"]
    );
    assert_eq!(
        p3["xl/timelineCaches/timelineCache1.xml"],
        orig["xl/timelineCaches/timelineCache1.xml"]
    );
}

#[test]
fn a_table_slicer_filters_its_table() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    assert_eq!(
        selected(&wb, DATA, "Product"),
        items(&[
            ("Apples", true, false),
            ("Pears", true, false),
            ("Plums", true, false)
        ])
    );
    wb.slicer_select(DATA, "Product", &[false, true, false])
        .unwrap();
    // Pears' rows show (2, 4, 9, 11), the others hide.
    let hidden: Vec<u32> = (1..13).filter(|r| wb.is_row_hidden(0, *r)).collect();
    assert_eq!(hidden, [1, 3, 5, 6, 7, 8, 10, 12]);
    assert!(wb.slicer_view(DATA, "Product").unwrap().filtered);
    // The table saves it as its AutoFilter; read back the same.
    let (path, p) = save(&mut wb, dir.path(), "table-slicer.xlsx");
    assert!(p["xl/tables/table1.xml"].contains("<filter val=\"Pears\"/>"));
    let back = Workbook::open(&path).unwrap();
    assert_eq!(
        selected(&back, DATA, "Product"),
        items(&[
            ("Apples", false, false),
            ("Pears", true, false),
            ("Plums", false, false)
        ])
    );
    // Another column's filter leaves items without data: greyed, last.
    wb.slicer_select(DATA, "Product", &[true, true, true])
        .unwrap();
    let scope = sheets_engine::FilterScope::Table("Sales".into());
    wb.set_column_filter(
        0,
        &scope,
        3,
        Some(sheets_engine::FilterKind::Values {
            values: vec!["10".into()],
            blanks: false,
            dates: Vec::new(),
        }),
    )
    .unwrap();
    assert_eq!(
        selected(&wb, DATA, "Product"),
        items(&[
            ("Apples", true, false),
            ("Pears", true, true),
            ("Plums", true, true)
        ])
    );
}
