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

fn serial(d: &str) -> f64 {
    // Days since 1899-12-30.
    let (y, m, day): (i64, i64, i64) = {
        let v: Vec<i64> = d.split('-').map(|x| x.parse().unwrap()).collect();
        (v[0], v[1], v[2])
    };
    let a = (14 - m) / 12;
    let (y2, m2) = (y + 4800 - a, m + 12 * a - 3);
    let jdn = day + (153 * m2 + 2) / 5 + 365 * y2 + y2 / 4 - y2 / 100 + y2 / 400 - 32045;
    (jdn - 2_415_019) as f64
}

const TIMELINE: &str = "NativeTimeline_Date";

#[test]
fn a_timeline_filters_its_pivot_table() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    assert_eq!(serial("2026-01-01"), 46023.0);
    // January to March, as a drag over three months sets it.
    wb.timeline_select(
        REPORT,
        TIMELINE,
        Some((serial("2026-01-01"), serial("2026-03-31"))),
    )
    .unwrap();
    assert_eq!(pivot_values(&wb), ["348", "195", "270", "813", ""]);
    let t = wb.timeline_view(REPORT, TIMELINE).unwrap();
    assert_eq!(t.label.as_deref(), Some("Jan - Mar 2026"));
    assert!(t.filtered);
    assert_eq!(
        t.periods.iter().map(|p| p.selected).collect::<Vec<_>>()[..4],
        [true, true, true, false]
    );
    // The Region slicer still filters on top of it.
    wb.slicer_select(REPORT, "Region", &[true, false, false])
        .unwrap();
    assert_eq!(pivot_values(&wb)[3], "309");
    wb.undo().unwrap();
    // Saved as Excel saves it: the pivot table's named date filter and the
    // timeline cache's selection, naming that filter.
    let (path, p) = save(&mut wb, dir.path(), "timeline-q1.xlsx");
    let pivot = &p["xl/pivotTables/pivotTable1.xml"];
    assert!(
        pivot.contains("type=\"dateBetween\"") && pivot.contains("name=\"NativeTimeline_Date\""),
        "{pivot}"
    );
    let cache = &p["xl/timelineCaches/timelineCache1.xml"];
    assert!(
        cache.contains(
            "<selection startDate=\"2026-01-01T00:00:00\" endDate=\"2026-03-31T00:00:00\"/>"
        ),
        "{cache}"
    );
    assert!(cache.contains("filterType=\"dateBetween\""), "{cache}");
    assert!(cache.contains("filterPivotName=\"PivotTable1\""), "{cache}");
    let back = Workbook::open(&path).unwrap();
    assert_eq!(pivot_values(&back), ["348", "195", "270", "813", ""]);
    assert_eq!(
        back.timeline_view(REPORT, TIMELINE)
            .unwrap()
            .label
            .as_deref(),
        Some("Jan - Mar 2026")
    );
    // Clear Filter, saved: no selection, no filter.
    wb.timeline_select(REPORT, TIMELINE, None).unwrap();
    assert_eq!(pivot_values(&wb)[3], "1068");
    let (_, p2) = save(&mut wb, dir.path(), "timeline-all.xlsx");
    let cache = &p2["xl/timelineCaches/timelineCache1.xml"];
    assert!(
        !cache.contains("<selection") && !cache.contains("pivotFilter"),
        "{cache}"
    );
    assert!(!p2["xl/pivotTables/pivotTable1.xml"].contains("dateBetween"));
    // Undo brings the filter back.
    wb.undo().unwrap();
    assert_eq!(pivot_values(&wb)[3], "813");
}

#[test]
fn a_timeline_changes_its_level() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    wb.timeline_set_level(REPORT, TIMELINE, 1).unwrap();
    let t = wb.timeline_view(REPORT, TIMELINE).unwrap();
    assert_eq!(t.level.as_deref(), Some("QUARTERS"));
    let labels: Vec<_> = t.periods.iter().map(|p| p.label.as_str()).collect();
    assert_eq!(labels, ["Q1", "Q2", "Q3", "Q4"]);
    // A quarter selected at that level.
    let q2 = (t.periods[1].start, t.periods[1].end - 1.0);
    wb.timeline_select(REPORT, TIMELINE, Some(q2)).unwrap();
    assert_eq!(pivot_values(&wb)[0], "240");
    let (path, p) = save(&mut wb, dir.path(), "timeline-quarters.xlsx");
    assert!(p["xl/timelines/timeline1.xml"].contains("level=\"1\""));
    let back = Workbook::open(&path).unwrap();
    let t = back.timeline_view(REPORT, TIMELINE).unwrap();
    assert_eq!(t.level.as_deref(), Some("QUARTERS"));
    assert_eq!(t.label.as_deref(), Some("Q2 2026"));
    // Days: more than fit; scrolling moves the first one.
    wb.timeline_set_level(REPORT, TIMELINE, 3).unwrap();
    let t = wb.timeline_view(REPORT, TIMELINE).unwrap();
    assert_eq!(t.total, 365);
    assert_eq!(t.periods[0].label, "1", "scrolled to the selection");
    wb.timeline_scroll(REPORT, TIMELINE, 5).unwrap();
    assert_eq!(wb.timeline_view(REPORT, TIMELINE).unwrap().before, 95);
}

fn drawing_names(wb: &Workbook, sheet: u32) -> Vec<String> {
    wb.drawings(sheet)
        .iter()
        .map(|d| match &*d.kind {
            DrawingKind::Slicer { name } => format!("slicer {name}"),
            DrawingKind::Timeline { name } => format!("timeline {name}"),
            k => format!("{k:?}"),
        })
        .collect()
}

#[test]
fn insert_slicer_on_a_pivot_table() {
    use sheets_engine::slicer_edit::SlicerSource;
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let source = SlicerSource::Pivot {
        sheet: 1,
        name: "PivotTable1".into(),
    };
    let fields = wb.slicer_fields(&source).unwrap();
    assert_eq!(fields, ["Region", "Product", "Date", "Units", "Revenue"]);
    let (sheet, at) = wb.slicer_place(&source).unwrap();
    assert_eq!(sheet, 1);
    let keys = wb
        .insert_slicers(sheet, &source, &["Product".into(), "Region".into()], at)
        .unwrap();
    assert_eq!(keys.len(), 2);
    assert_eq!(
        drawing_names(&wb, 1),
        [
            "slicer Region",
            "timeline NativeTimeline_Date",
            "slicer Product 1",
            "slicer Region 1"
        ]
    );
    assert_eq!(
        selected(&wb, REPORT, "Product 1"),
        items(&[
            ("Apples", true, false),
            ("Pears", true, false),
            ("Plums", true, false)
        ])
    );
    // It filters, and the file's Region slicer sees it (no data greyed).
    wb.slicer_select(REPORT, "Product 1", &[false, false, true])
        .unwrap();
    assert_eq!(pivot_values(&wb)[0], "270");
    let s = wb.slicer_settings(REPORT, "Region 1").unwrap();
    assert_eq!(s.cache_name, "Slicer_Region1");
    // Saved with its parts, extensions and name; reopened as it was.
    let (path, p) = save(&mut wb, dir.path(), "inserted.xlsx");
    let wbx = &p["xl/workbook.xml"];
    assert!(
        wbx.contains("<definedName name=\"Slicer_Product1\">#N/A</definedName>"),
        "{wbx}"
    );
    assert!(wbx.contains("Slicer_Region1"), "{wbx}");
    let caches: Vec<&String> = p
        .keys()
        .filter(|k| k.starts_with("xl/slicerCaches/") && !k.contains("_rels"))
        .collect();
    assert_eq!(caches.len(), 4, "{caches:?}");
    assert!(
        p["xl/pivotCache/pivotCacheDefinition1.xml"].contains("pivotCacheId=\"1\""),
        "the cache's Excel 2010 id"
    );
    let back = Workbook::open(&path).unwrap();
    assert_eq!(
        drawing_names(&back, 1),
        [
            "slicer Region",
            "timeline NativeTimeline_Date",
            "slicer Product 1",
            "slicer Region 1"
        ]
    );
    assert_eq!(
        selected(&back, REPORT, "Product 1"),
        items(&[
            ("Apples", false, false),
            ("Pears", false, false),
            ("Plums", true, false)
        ])
    );
    assert_eq!(pivot_values(&back)[0], "270");
    // Saving again keeps one of each.
    let (_, p2) = save(&mut wb, dir.path(), "inserted-again.xlsx");
    assert_eq!(
        p2.keys()
            .filter(|k| k.starts_with("xl/slicerCaches/") && !k.contains("_rels"))
            .count(),
        4
    );
    assert_eq!(p2["xl/workbook.xml"].matches("Slicer_Product1").count(), 1);
    // Undo the filter and the insert: the next save drops them again.
    wb.undo().unwrap();
    wb.undo().unwrap();
    assert_eq!(
        drawing_names(&wb, 1),
        ["slicer Region", "timeline NativeTimeline_Date"]
    );
    let (_, p3) = save(&mut wb, dir.path(), "inserted-undone.xlsx");
    assert!(!p3["xl/workbook.xml"].contains("Slicer_Product1"));
    assert_eq!(
        p3.keys()
            .filter(|k| k.starts_with("xl/slicerCaches/") && !k.contains("_rels"))
            .count(),
        2
    );
    assert!(!p3.keys().any(|k| k == "xl/slicers/slicer3.xml"));
}

#[test]
fn insert_slicer_on_a_table_and_delete_one() {
    use sheets_engine::slicer_edit::SlicerSource;
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let source = SlicerSource::Table("Sales".into());
    let (sheet, at) = wb.slicer_place(&source).unwrap();
    // Right of the table and of its Product slicer (G2:J13).
    assert_eq!((sheet, at), (0, (0, 10)));
    wb.insert_slicers(sheet, &source, &["Region".into()], at)
        .unwrap();
    wb.slicer_select(DATA, "Region 1", &[true, false, false])
        .unwrap();
    let hidden: Vec<u32> = (1..13).filter(|r| wb.is_row_hidden(0, *r)).collect();
    assert_eq!(hidden, [2, 3, 5, 6, 8, 9, 11, 12]);
    // Delete the file's Product slicer.
    let key = wb
        .drawings(0)
        .into_iter()
        .find(|d| matches!(&*d.kind, DrawingKind::Slicer { name } if name == "Product"))
        .unwrap()
        .key;
    wb.delete_drawing(key).unwrap();
    assert_eq!(wb.slicer_names(DATA), ["Region 1"]);
    let (path, p) = save(&mut wb, dir.path(), "table-slicers.xlsx");
    let wbx = &p["xl/workbook.xml"];
    assert!(
        !wbx.contains("Slicer_Product"),
        "its name goes with its cache"
    );
    assert!(wbx.contains("Slicer_Region1"), "{wbx}");
    let sheet1 = &p["xl/worksheets/sheet1.xml"];
    assert_eq!(sheet1.matches("<x14:slicer ").count(), 1, "{sheet1}");
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.slicer_names(DATA), ["Region 1"]);
    assert_eq!(
        selected(&back, DATA, "Region 1"),
        items(&[
            ("East", true, false),
            ("North", false, false),
            ("West", false, false)
        ])
    );
}

#[test]
fn slicer_settings_are_saved() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let mut s = wb.slicer_settings(REPORT, "Region").unwrap();
    assert_eq!(s.style, "SlicerStyleLight1");
    s.caption = "Area".into();
    s.descending = true;
    s.style = "SlicerStyleDark2".into();
    s.columns = 2;
    s.mark_no_data = false;
    s.no_data_last = false;
    wb.set_slicer_settings(REPORT, "Region", &s).unwrap();
    let v = wb.slicer_view(REPORT, "Region").unwrap();
    assert_eq!(v.caption, "Area");
    assert_eq!(v.columns, 2);
    let labels: Vec<_> = v.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["West", "North", "East"]);
    let (path, p) = save(&mut wb, dir.path(), "settings.xlsx");
    let part = &p["xl/slicers/slicer2.xml"];
    assert!(
        part.contains("caption=\"Area\"") && part.contains("style=\"SlicerStyleDark2\""),
        "{part}"
    );
    let cache = &p["xl/slicerCaches/slicerCache2.xml"];
    assert!(
        cache.contains("sortOrder=\"descending\"") && cache.contains("crossFilter=\"none\""),
        "{cache}"
    );
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.slicer_settings(REPORT, "Region").unwrap(), s);
    // Undo, save: as the file had it.
    wb.undo().unwrap();
    let (_, p2) = save(&mut wb, dir.path(), "settings-undone.xlsx");
    assert!(p2["xl/slicers/slicer2.xml"].contains("caption=\"Region\""));
    assert!(!p2["xl/slicerCaches/slicerCache2.xml"].contains("sortOrder"));
}

#[test]
fn insert_timeline_on_a_pivot_table() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    // Only Date holds dates.
    assert_eq!(wb.timeline_fields(1, "PivotTable1").unwrap(), ["Date"]);
    let (sheet, at) = wb.timeline_place(1, "PivotTable1").unwrap();
    assert_eq!(sheet, 1);
    let keys = wb
        .insert_timelines(sheet, "PivotTable1", &["Date".into()], at)
        .unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(wb.timeline_of(keys[0]).as_deref(), Some("Date"));
    assert_eq!(
        drawing_names(&wb, 1),
        [
            "slicer Region",
            "timeline NativeTimeline_Date",
            "timeline Date"
        ]
    );
    let t = wb.timeline_view(REPORT, "Date").unwrap();
    assert_eq!(t.level.as_deref(), Some("MONTHS"));
    assert_eq!(t.label.as_deref(), Some("All Periods"));
    assert_eq!(t.total, 12, "the field's whole year");
    let s = wb.timeline_settings(REPORT, "Date").unwrap();
    assert_eq!(s.cache_name, "NativeTimeline_Date1");
    assert_eq!(s.source_name, "Date");
    // It filters the pivot table as the file's timeline does.
    wb.timeline_select(
        REPORT,
        "Date",
        Some((serial("2026-01-01"), serial("2026-03-31"))),
    )
    .unwrap();
    assert_eq!(pivot_values(&wb), ["348", "195", "270", "813", ""]);
    // The file's timeline on the same field shows it (a field keeps one
    // date filter).
    assert_eq!(
        wb.timeline_view(REPORT, TIMELINE).unwrap().label.as_deref(),
        Some("Jan - Mar 2026")
    );
    // Saved with its parts, extensions and name; reopened as it was.
    let (path, p) = save(&mut wb, dir.path(), "timeline-inserted.xlsx");
    let wbx = &p["xl/workbook.xml"];
    assert!(
        wbx.contains("<definedName name=\"NativeTimeline_Date1\">#N/A</definedName>"),
        "{wbx}"
    );
    assert_eq!(wbx.matches("timelineCacheRef ").count(), 2, "{wbx}");
    let cache = &p["xl/timelineCaches/timelineCache2.xml"];
    assert!(cache.contains("name=\"NativeTimeline_Date1\""), "{cache}");
    assert!(
        cache.contains(
            "<bounds startDate=\"2026-01-01T00:00:00\" endDate=\"2027-01-01T00:00:00\"/>"
        ),
        "{cache}"
    );
    assert!(cache.contains("filterType=\"dateBetween\""), "{cache}");
    assert!(cache.contains("pivotCacheId=\"1\""), "{cache}");
    let part = &p["xl/timelines/timeline2.xml"];
    assert!(
        part.contains("name=\"Date\" cache=\"NativeTimeline_Date1\" caption=\"Date\""),
        "{part}"
    );
    let sheet2 = &p["xl/worksheets/sheet2.xml"];
    assert_eq!(sheet2.matches("x15:timelineRef ").count(), 2, "{sheet2}");
    let drawing = &p["xl/drawings/drawing2.xml"];
    assert!(drawing.contains("<tsle:timeslicer"), "{drawing}");
    let back = Workbook::open(&path).unwrap();
    assert_eq!(
        drawing_names(&back, 1),
        [
            "slicer Region",
            "timeline NativeTimeline_Date",
            "timeline Date"
        ]
    );
    assert_eq!(
        back.timeline_view(REPORT, "Date").unwrap().label.as_deref(),
        Some("Jan - Mar 2026")
    );
    assert_eq!(pivot_values(&back)[3], "813");
    // Saving again keeps one of each.
    let (_, p2) = save(&mut wb, dir.path(), "timeline-inserted-again.xlsx");
    assert_eq!(
        p2.keys()
            .filter(|k| k.starts_with("xl/timelineCaches/") && !k.contains("_rels"))
            .count(),
        2
    );
    assert_eq!(
        p2["xl/workbook.xml"]
            .matches("NativeTimeline_Date1")
            .count(),
        1
    );
    // Undo the filter and the insert: the next save drops them again.
    wb.undo().unwrap();
    wb.undo().unwrap();
    assert_eq!(
        drawing_names(&wb, 1),
        ["slicer Region", "timeline NativeTimeline_Date"]
    );
    let (_, p3) = save(&mut wb, dir.path(), "timeline-undone.xlsx");
    assert!(!p3["xl/workbook.xml"].contains("NativeTimeline_Date1"));
    assert!(!p3.keys().any(|k| k == "xl/timelines/timeline2.xml"));
    assert!(
        !p3.keys()
            .any(|k| k == "xl/timelineCaches/timelineCache2.xml")
    );
    assert_eq!(
        p3["xl/worksheets/sheet2.xml"]
            .matches("x15:timelineRef ")
            .count(),
        1
    );
}

#[test]
fn timeline_settings_are_saved_and_a_deleted_timeline_goes() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let mut s = wb.timeline_settings(REPORT, TIMELINE).unwrap();
    assert_eq!(s.style, "TimeSlicerStyleLight1");
    assert!(s.show_header && s.show_scrollbar);
    s.caption = "Order date".into();
    s.show_selection_label = false;
    s.show_scrollbar = false;
    s.style = "TimeSlicerStyleDark2".into();
    wb.set_timeline_settings(REPORT, TIMELINE, &s).unwrap();
    let v = wb.timeline_view(REPORT, TIMELINE).unwrap();
    assert_eq!(v.caption, "Order date");
    assert_eq!(v.label, None);
    assert!(!v.show_scrollbar);
    let (path, p) = save(&mut wb, dir.path(), "timeline-settings.xlsx");
    let part = &p["xl/timelines/timeline1.xml"];
    assert!(
        part.contains("caption=\"Order date\"")
            && part.contains("showSelectionLabel=\"0\"")
            && part.contains("showHorizontalScrollbar=\"0\"")
            && part.contains("style=\"TimeSlicerStyleDark2\""),
        "{part}"
    );
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.timeline_settings(REPORT, TIMELINE).unwrap(), s);
    // Undone: as the file had it.
    wb.undo().unwrap();
    let (_, p2) = save(&mut wb, dir.path(), "timeline-settings-undone.xlsx");
    let part = &p2["xl/timelines/timeline1.xml"];
    assert!(
        part.contains("caption=\"Date\"") && !part.contains("style="),
        "{part}"
    );
    // Deleting the file's timeline takes its parts, extensions and name.
    let key = wb
        .drawings(1)
        .into_iter()
        .find(|d| matches!(&*d.kind, DrawingKind::Timeline { .. }))
        .unwrap()
        .key;
    wb.delete_drawing(key).unwrap();
    let (path, p3) = save(&mut wb, dir.path(), "timeline-deleted.xlsx");
    assert!(
        !p3.keys().any(|k| k.starts_with("xl/timeline")),
        "{:?}",
        p3.keys()
    );
    assert!(!p3["xl/workbook.xml"].contains("NativeTimeline_Date"));
    assert!(!p3["xl/worksheets/sheet2.xml"].contains("timelineRef"));
    let back = Workbook::open(&path).unwrap();
    assert_eq!(drawing_names(&back, 1), ["slicer Region"]);
}

#[test]
fn a_timeline_scrolls_to_a_period_and_previews_a_drag() {
    let mut wb = Workbook::open(fixture()).unwrap();
    wb.timeline_set_level(REPORT, TIMELINE, 3).unwrap();
    wb.timeline_scroll_to(REPORT, TIMELINE, 40).unwrap();
    let t = wb.timeline_view(REPORT, TIMELINE).unwrap();
    assert_eq!(t.before, 40);
    assert_eq!(t.periods[0].label, "10", "Feb 10");
    assert_eq!(t.last_scroll(8), 357);
    let p = t.with_selection((serial("2026-02-11"), serial("2026-02-12")));
    assert_eq!(p.label.as_deref(), Some("11 - 12 Feb 2026"));
    let sel: Vec<bool> = p.periods.iter().take(4).map(|p| p.selected).collect();
    assert_eq!(sel, [false, true, true, false]);
    // Only a preview: nothing is filtered.
    assert_eq!(pivot_values(&wb)[3], "1068");
}

#[test]
fn insert_timeline_on_a_new_pivot_table() {
    use sheets_engine::PivotDestination;
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::new_empty().unwrap();
    let mut cells = vec![
        (0, 0, "Day".to_string()),
        (0, 1, "Item".to_string()),
        (0, 2, "Amount".to_string()),
    ];
    for (r, (day, item, amount)) in [
        ("2025-11-03", "Tea", "4"),
        ("2025-12-14", "Cake", "9"),
        ("2026-01-20", "Tea", "5"),
        ("2026-02-02", "Cake", "7"),
    ]
    .iter()
    .enumerate()
    {
        let r = r as u32 + 1;
        cells.push((r, 0, day.to_string()));
        cells.push((r, 1, item.to_string()));
        cells.push((r, 2, amount.to_string()));
    }
    wb.set_inputs(0, &cells).unwrap();
    let (sheet, _, _) = wb
        .insert_pivot("Sheet1!$A$1:$C$5", 0, PivotDestination::NewSheet)
        .unwrap();
    let name = wb.pivot_tables(sheet)[0].name.clone();
    assert_eq!(wb.timeline_fields(sheet, &name).unwrap(), ["Day"]);
    let (s, at) = wb.timeline_place(sheet, &name).unwrap();
    wb.insert_timelines(s, &name, &["Day".into()], at).unwrap();
    let sid = wb.sheet_id(sheet);
    let t = wb.timeline_view(sid, "Day").unwrap();
    assert_eq!(t.total, 24, "two years of months");
    assert_eq!(t.periods[0].group.as_deref(), Some("2025"));
    let (path, p) = save(&mut wb, dir.path(), "timeline-new-pivot.xlsx");
    let cache = p
        .iter()
        .find(|(k, _)| k.starts_with("xl/timelineCaches/") && !k.contains("_rels"))
        .map(|(_, v)| v)
        .unwrap();
    assert!(
        cache.contains("startDate=\"2025-01-01T00:00:00\" endDate=\"2027-01-01T00:00:00\""),
        "{cache}"
    );
    let back = Workbook::open(&path).unwrap();
    assert_eq!(
        back.timeline_view(sid, "Day").unwrap().label.as_deref(),
        Some("All Periods")
    );
}
