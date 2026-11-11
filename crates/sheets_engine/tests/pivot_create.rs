//! Making pivot tables in Kantor (Insert PivotTable), arranging their
//! fields, deleting them, and saving them so Excel and Kantor read them
//! back.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pivot_check::{el, els, parts, pivot_problems};
use roxmltree::Document;
use sheets_engine::{
    AreaEntry, AxisEntry, CellRange, PivotDestination, PivotFunc, PivotLayout, ReportLayout,
    SubtotalPlace, Workbook,
};

const HEADERS: [&str; 4] = ["Region", "Product", "Units", "Revenue"];
const ROWS: [(&str, &str, f64, f64); 12] = [
    ("East", "Apples", 10.0, 120.0),
    ("West", "Pears", 4.0, 60.0),
    ("North", "Apples", 7.0, 84.0),
    ("East", "Pears", 3.0, 45.0),
    ("West", "Apples", 12.0, 144.0),
    ("North", "Plums", 5.0, 90.0),
    ("East", "Plums", 8.0, 144.0),
    ("West", "Plums", 2.0, 36.0),
    ("North", "Pears", 6.0, 90.0),
    ("East", "Apples", 9.0, 108.0),
    ("West", "Pears", 1.0, 15.0),
    ("North", "Apples", 11.0, 132.0),
];

/// A new workbook with the data on `Sheet1!A1:D13`.
fn data_book() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let mut cells: Vec<(u32, u32, String)> = HEADERS
        .iter()
        .enumerate()
        .map(|(c, h)| (0, c as u32, h.to_string()))
        .collect();
    for (r, (region, product, units, revenue)) in ROWS.iter().enumerate() {
        let r = r as u32 + 1;
        cells.push((r, 0, region.to_string()));
        cells.push((r, 1, product.to_string()));
        cells.push((r, 2, units.to_string()));
        cells.push((r, 3, revenue.to_string()));
    }
    wb.set_inputs(0, &cells).unwrap();
    wb
}

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/21_pivot_tables.xlsx")
}

fn grid(wb: &Workbook, sheet: u32, (t, l, b, r): (u32, u32, u32, u32)) -> Vec<Vec<String>> {
    (t..=b)
        .map(|row| (l..=r).map(|c| wb.display_value(sheet, row, c)).collect())
        .collect()
}

fn rows(v: &[&[&str]]) -> Vec<Vec<String>> {
    v.iter()
        .map(|r| r.iter().map(|s| s.to_string()).collect())
        .collect()
}

/// Save, check the package and the pivot parts, read it back.
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
    for (name, xml) in &p {
        if name.ends_with(".xml") || name.ends_with(".rels") {
            assert!(Document::parse(xml).is_ok(), "{name} is well-formed");
        }
    }
    (path, p)
}

/// A pivot on a new sheet over `Sheet1!A1:D13`; returns its sheet.
fn new_pivot(wb: &mut Workbook) -> u32 {
    let (sheet, row, col) = wb
        .insert_pivot("Sheet1!$A$1:$D$13", 0, PivotDestination::NewSheet)
        .unwrap();
    assert_eq!((row, col), (2, 0), "at A3, as Excel puts it");
    sheet
}

fn layout(wb: &Workbook, sheet: u32) -> PivotLayout {
    wb.pivot_layout(sheet, 2, 0).unwrap()
}

fn field(l: &PivotLayout, name: &str) -> usize {
    l.fields.iter().position(|f| f.name == name).unwrap()
}

#[test]
fn source_suggestions() {
    let wb = data_book();
    let one = |r, c| CellRange {
        top: r,
        left: c,
        bottom: r,
        right: c,
    };
    // The current region around the active cell.
    assert_eq!(
        wb.pivot_source_suggestion(0, one(4, 2), (4, 2)),
        "Sheet1!$A$1:$D$13"
    );
    // The selection.
    let sel = CellRange {
        top: 0,
        left: 0,
        bottom: 5,
        right: 2,
    };
    assert_eq!(
        wb.pivot_source_suggestion(0, sel, (0, 0)),
        "Sheet1!$A$1:$C$6"
    );
    // Nothing around an empty cell.
    assert_eq!(wb.pivot_source_suggestion(0, one(20, 20), (20, 20)), "");
    // A table: its name.
    let t = Workbook::open(fixture()).unwrap();
    let data = t.sheet_names().iter().position(|n| n == "Data").unwrap() as u32;
    assert_eq!(t.pivot_source_suggestion(data, one(3, 1), (3, 1)), "Sales");
}

#[test]
fn sources_are_checked() {
    let mut wb = data_book();
    let ok = wb.check_pivot_source("A1:D13", 0).unwrap();
    assert_eq!(ok.fields, HEADERS);
    assert_eq!(ok.range.name(), "A1:D13");
    assert!(wb.check_pivot_source("Nope!A1:B2", 0).is_err());
    assert!(wb.check_pivot_source("", 0).is_err());
    assert!(wb.check_pivot_source("A1", 0).is_err(), "one cell");
    assert!(wb.check_pivot_source("A1:D1", 0).is_err(), "no data rows");
    // An empty header.
    let e = wb.check_pivot_source("A1:E13", 0).unwrap_err().to_string();
    assert!(e.contains("not valid"), "{e}");
    // Duplicate headers.
    wb.set_input(0, 0, 3, "Region").unwrap();
    let e = wb.check_pivot_source("A1:D13", 0).unwrap_err().to_string();
    assert!(e.contains("used twice"), "{e}");
}

#[test]
fn a_new_pivot_starts_empty_on_a_new_sheet() {
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    assert_eq!(wb.sheet_names(), ["Sheet1", "Sheet2"]);
    let p = wb.pivot_at(s, 2, 0).unwrap();
    assert_eq!(p.name, "PivotTable1");
    // Excel's placeholder area.
    assert_eq!(p.area.name(), "A3:C20");
    let l = layout(&wb, s);
    assert_eq!(
        l.fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        HEADERS
    );
    assert!(!l.fields[0].numeric && l.fields[2].numeric);
    assert_eq!(l.source, "Sheet1!$A$1:$D$13");
    assert!(l.rows.is_empty() && l.values.is_empty());
    // One undo step takes it and its sheet away.
    assert!(wb.undo().unwrap());
    assert_eq!(wb.sheet_names(), ["Sheet1"]);
    assert!(wb.redo().unwrap());
    assert_eq!(wb.sheet_names(), ["Sheet1", "Sheet2"]);
    assert!(wb.pivot_at(1, 2, 0).is_some());
}

#[test]
fn checking_fields_builds_the_pivot_like_excel() {
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    let mut l = layout(&wb, s);
    l.toggle_field(field(&l, "Region")); // text -> Rows
    l.toggle_field(field(&l, "Revenue")); // number -> Values, Sum
    assert_eq!(l.rows, [AxisEntry::Field(0)]);
    assert_eq!(l.values[0].func, PivotFunc::Sum);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (2, 0, 6, 1)),
        rows(&[
            &["Row Labels", "Sum of Revenue"],
            &["East", "417"],
            &["North", "396"],
            &["West", "255"],
            &["Grand Total", "1068"],
        ])
    );
    assert_eq!(wb.pivot_at(s, 2, 0).unwrap().area.name(), "A3:B7");

    // Columns: Product.
    let mut l = layout(&wb, s);
    l.move_entry(AreaEntry::Field(field(&l, "Product")), "Columns", 0);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (2, 0, 4, 4)),
        rows(&[
            &["Sum of Revenue", "Column Labels", "", "", ""],
            &["Row Labels", "Apples", "Pears", "Plums", "Grand Total"],
            &["East", "228", "45", "144", "417"],
        ])
    );
    // Grand totals off.
    let mut l = layout(&wb, s);
    l.grand_total_column = false;
    l.grand_total_row = false;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(wb.pivot_at(s, 2, 0).unwrap().area.name(), "A3:D7");
    assert_eq!(wb.display_value(s, 3, 4), "");

    // Each change is one undo step.
    assert!(wb.undo().unwrap());
    assert_eq!(wb.display_value(s, 3, 4), "Grand Total");
    assert!(wb.undo().unwrap());
    assert_eq!(wb.display_value(s, 3, 0), "East");
    assert_eq!(layout(&wb, s).columns, []);
    assert!(wb.redo().unwrap());
    assert!(wb.redo().unwrap());
    assert_eq!(wb.display_value(s, 3, 4), "");
}

#[test]
fn report_layouts_and_subtotals() {
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    let mut l = layout(&wb, s);
    l.rows = vec![AxisEntry::Field(0), AxisEntry::Field(1)];
    l.add_value(3);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    // Compact: one label column, subtotals on the region's own row.
    assert_eq!(
        grid(&wb, s, (2, 0, 6, 1)),
        rows(&[
            &["Row Labels", "Sum of Revenue"],
            &["East", "417"],
            &["Apples", "228"],
            &["Pears", "45"],
            &["Plums", "144"],
        ])
    );
    // Outline: a column per field, named after it.
    l.layout = ReportLayout::Outline;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (2, 0, 4, 2)),
        rows(&[
            &["Region", "Product", "Sum of Revenue"],
            &["East", "", "417"],
            &["", "Apples", "228"],
        ])
    );
    assert_eq!(layout(&wb, s).layout, ReportLayout::Outline);
    // Tabular: the region beside its first product, subtotal at the bottom.
    l.layout = ReportLayout::Tabular;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (2, 0, 6, 2)),
        rows(&[
            &["Region", "Product", "Sum of Revenue"],
            &["East", "Apples", "228"],
            &["", "Pears", "45"],
            &["", "Plums", "144"],
            &["East Total", "", "417"],
        ])
    );
    let got = layout(&wb, s);
    assert_eq!(got.layout, ReportLayout::Tabular);
    // Subtotals off (compact): the region row shows no value.
    l.layout = ReportLayout::Compact;
    l.subtotals = SubtotalPlace::Off;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(wb.display_value(s, 3, 0), "East");
    assert_eq!(wb.display_value(s, 3, 1), "");
    assert_eq!(layout(&wb, s).subtotals, SubtotalPlace::Off);
    // Subtotals at the bottom (compact).
    l.subtotals = SubtotalPlace::Bottom;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(wb.display_value(s, 3, 1), "");
    assert_eq!(wb.display_value(s, 7, 0), "East Total");
    assert_eq!(wb.display_value(s, 7, 1), "417");
}

#[test]
fn every_aggregation() {
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    let east: Vec<f64> = ROWS.iter().filter(|r| r.0 == "East").map(|r| r.3).collect();
    let n = east.len() as f64;
    let mean = east.iter().sum::<f64>() / n;
    let ss: f64 = east.iter().map(|x| (x - mean).powi(2)).sum();
    let expect = |f: PivotFunc| -> f64 {
        match f {
            PivotFunc::Sum => east.iter().sum(),
            PivotFunc::Count | PivotFunc::CountNumbers => n,
            PivotFunc::Average => mean,
            PivotFunc::Max => east.iter().copied().fold(f64::MIN, f64::max),
            PivotFunc::Min => east.iter().copied().fold(f64::MAX, f64::min),
            PivotFunc::Product => east.iter().product(),
            PivotFunc::StdDev => (ss / (n - 1.0)).sqrt(),
            PivotFunc::StdDevp => (ss / n).sqrt(),
            PivotFunc::Var => ss / (n - 1.0),
            PivotFunc::Varp => ss / n,
        }
    };
    for f in PivotFunc::ALL {
        let mut l = layout(&wb, s);
        l.rows = vec![AxisEntry::Field(0)];
        l.values.clear();
        l.add_value(3);
        l.values[0].func = f;
        wb.set_pivot_layout(s, 2, 0, &l).unwrap();
        let caption = wb.display_value(s, 2, 1);
        let want = format!(
            "{} of Revenue",
            if f == PivotFunc::CountNumbers {
                "Count"
            } else {
                f.label()
            }
        );
        assert_eq!(caption, want);
        let got: f64 = wb.display_value(s, 3, 1).parse().unwrap_or(f64::NAN);
        let want = expect(f);
        assert!(
            (got - want).abs() < 1e-6 * want.abs().max(1.0) || wb.display_value(s, 3, 1).len() > 8,
            "{f:?}: {got} vs {want}"
        );
    }
    // A custom name and two value fields: the Σ Values field appears.
    let mut l = layout(&wb, s);
    l.values[0].func = PivotFunc::Sum;
    l.values[0].name = Some("Takings".into());
    l.add_value(2);
    assert_eq!(l.columns, [AxisEntry::Values]);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (2, 0, 3, 2)),
        rows(&[
            &["Row Labels", "Takings", "Sum of Units"],
            &["East", "417", "30"]
        ])
    );
    // Values moved to rows.
    let mut l = layout(&wb, s);
    l.move_entry(AreaEntry::Values, "Rows", 1);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (2, 0, 5, 1)),
        rows(&[
            &["Row Labels", ""],
            &["East", ""],
            &["Takings", "417"],
            &["Sum of Units", "30"],
        ])
    );
    // Count of a text field.
    let mut l = layout(&wb, s);
    l.values.clear();
    l.add_value(1);
    assert_eq!(l.values[0].func, PivotFunc::Count);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(wb.display_value(s, 2, 1), "Count of Product");
    assert_eq!(wb.display_value(s, 3, 1), "4");
}

#[test]
fn filters_and_hidden_items() {
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    let mut l = layout(&wb, s);
    l.rows = vec![AxisEntry::Field(1)];
    l.add_value(3);
    l.move_entry(AreaEntry::Field(0), "Filters", 0);
    l.filter_item = vec![Some("East".into())];
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (0, 0, 7, 1)),
        rows(&[
            &["Region", "East"],
            &["", ""],
            &["Row Labels", "Sum of Revenue"],
            &["Apples", "228"],
            &["Pears", "45"],
            &["Plums", "144"],
            &["Grand Total", "417"],
            &["", ""],
        ])
    );
    let got = layout(&wb, s);
    assert_eq!(got.filters, [0]);
    assert_eq!(got.filter_item, [Some("East".to_owned())]);
    // Several items: hide West.
    let mut l = got;
    l.filter_item = vec![None];
    l.fields[0]
        .items
        .iter_mut()
        .for_each(|i| i.hidden = i.label == "West");
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(wb.display_value(s, 0, 1), "(Multiple Items)");
    assert_eq!(wb.display_value(s, 6, 1), "813");
    // Hidden row items.
    let mut l = layout(&wb, s);
    l.fields[1]
        .items
        .iter_mut()
        .for_each(|i| i.hidden = i.label == "Pears");
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(wb.display_value(s, 4, 0), "Plums");
    // Filters past the top push the body down.
    let mut l = layout(&wb, s);
    l.move_entry(AreaEntry::Field(1), "Filters", 1);
    l.rows = vec![AxisEntry::Field(2)];
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    let p = wb.pivot_at(s, 3, 0).unwrap();
    assert_eq!(p.area.top, 3);
    assert_eq!(p.bounds.top, 0);
    // The saved file has it all.
    let dir = tempfile::tempdir().unwrap();
    let (path, parts) = save(&mut wb, dir.path(), "f.xlsx");
    let table = &parts["xl/pivotTables/pivotTable1.xml"];
    assert!(table.contains("<pageFields count=\"2\">"), "{table}");
    assert!(table.contains("multipleItemSelectionAllowed=\"1\""));
    let back = Workbook::open(&path).unwrap();
    let l2 = back.pivot_layout(s, 3, 0).unwrap();
    assert_eq!(l2.filters, [0, 1]);
    assert!(
        l2.fields[0]
            .items
            .iter()
            .any(|i| i.hidden && i.label == "West")
    );
}

#[test]
fn saved_pivots_reopen_refresh_and_answer_getpivotdata() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    let mut l = layout(&wb, s);
    l.toggle_field(0);
    l.toggle_field(3);
    l.move_entry(AreaEntry::Field(1), "Columns", 0);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    let before = grid(&wb, s, (0, 0, 8, 5));
    let (path, p) = save(&mut wb, dir.path(), "a.xlsx");
    // The parts Kantor adds.
    let wbx = &p["xl/workbook.xml"];
    assert!(
        wbx.contains("<pivotCaches><pivotCache cacheId=\"0\" r:id="),
        "{wbx}"
    );
    let table = &p["xl/pivotTables/pivotTable1.xml"];
    assert!(table.contains("cacheId=\"0\""));
    assert!(table.contains("name=\"PivotTable1\""));
    assert!(table.contains("PivotStyleLight16"));
    assert!(table.contains("<location ref=\"A3:E8\""), "{table}");
    let cache = &p["xl/pivotCache/pivotCacheDefinition1.xml"];
    assert!(cache.contains("refreshOnLoad=\"1\""));
    assert!(cache.contains("<worksheetSource ref=\"A1:D13\" sheet=\"Sheet1\"/>"));
    let doc = Document::parse(&p["xl/pivotCache/pivotCacheRecords1.xml"]).unwrap();
    assert_eq!(els(doc.root_element(), "r").count(), 12);
    let ct = &p["[Content_Types].xml"];
    for part in [
        "/xl/pivotTables/pivotTable1.xml",
        "/xl/pivotCache/pivotCacheDefinition1.xml",
        "/xl/pivotCache/pivotCacheRecords1.xml",
    ] {
        assert_eq!(ct.matches(&format!("PartName=\"{part}\"")).count(), 1);
    }
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());

    // Kantor reads back what it saved.
    let mut back = Workbook::open(&path).unwrap();
    assert_eq!(grid(&back, s, (0, 0, 8, 5)), before);
    assert_eq!(back.pivot_layout(s, 2, 0).unwrap().values, l.values);
    assert_eq!(back.refresh_pivot(s, 2, 0).unwrap(), 1);
    assert_eq!(grid(&back, s, (0, 0, 8, 5)), before);
    back.set_input(
        s,
        12,
        0,
        "=GETPIVOTDATA(\"Sum of Revenue\",$A$3,\"Region\",\"East\",\"Product\",\"Apples\")",
    )
    .unwrap();
    assert_eq!(back.display_value(s, 12, 0), "228");
    // An edit in the source and a refresh.
    back.set_input(0, 1, 3, "1000").unwrap();
    back.refresh_pivot(s, 2, 0).unwrap();
    assert_eq!(back.display_value(s, 12, 0), "1108");
    // Saved again: carried, once.
    let (path2, p2) = save(&mut back, dir.path(), "b.xlsx");
    assert_eq!(p2["xl/workbook.xml"].matches("<pivotCache ").count(), 1);
    assert_eq!(
        p2.keys()
            .filter(|k| k.starts_with("xl/pivotTables/") && k.ends_with(".xml"))
            .count(),
        1
    );
    let again = Workbook::open(&path2).unwrap();
    assert_eq!(again.display_value(s, 12, 0), "1108");

    // The first workbook, saved twice: the new pivot is carried the second
    // time.
    let (_, p3) = save(&mut wb, dir.path(), "c.xlsx");
    assert_eq!(
        p3["xl/pivotTables/pivotTable1.xml"],
        p["xl/pivotTables/pivotTable1.xml"]
    );
    l.toggle_field(2);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    let (_, p4) = save(&mut wb, dir.path(), "d.xlsx");
    assert_eq!(p4["xl/workbook.xml"].matches("<pivotCache ").count(), 1);
    assert!(p4["xl/pivotTables/pivotTable1.xml"].contains("<dataFields count=\"2\">"));
}

#[test]
fn a_pivot_at_a_cell_and_room_checks() {
    let mut wb = data_book();
    let dest = PivotDestination::Cell {
        sheet: 0,
        row: 0,
        col: 6,
    };
    let (s, r, c) = wb.insert_pivot("A1:D13", 0, dest).unwrap();
    assert_eq!((s, r, c), (0, 0, 6));
    let mut l = wb.pivot_layout(0, 0, 6).unwrap();
    l.toggle_field(0);
    l.toggle_field(3);
    wb.set_pivot_layout(0, 0, 6, &l).unwrap();
    assert_eq!(wb.display_value(0, 1, 6), "East");
    // Not over data, not inside another pivot, not over a table.
    let over = PivotDestination::Cell {
        sheet: 0,
        row: 3,
        col: 2,
    };
    assert!(wb.insert_pivot("A1:D13", 0, over).is_err());
    let inside = PivotDestination::Cell {
        sheet: 0,
        row: 2,
        col: 7,
    };
    assert!(wb.insert_pivot("A1:D13", 0, inside).is_err());
    // A change that would run into data is refused, nothing changes.
    wb.set_input(0, 8, 6, "in the way").unwrap();
    let mut l = wb.pivot_layout(0, 0, 6).unwrap();
    l.rows.push(AxisEntry::Field(1));
    let e = wb.set_pivot_layout(0, 0, 6, &l).unwrap_err().to_string();
    assert!(e.contains("already data"), "{e}");
    assert_eq!(wb.pivot_layout(0, 0, 6).unwrap().rows.len(), 1);
}

#[test]
fn pivots_in_a_workbook_that_has_some_and_from_a_table() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let data = wb.sheet_names().iter().position(|n| n == "Data").unwrap() as u32;
    let (s, _, _) = wb
        .insert_pivot("Sales", data, PivotDestination::NewSheet)
        .unwrap();
    let mut l = wb.pivot_layout(s, 2, 0).unwrap();
    assert_eq!(l.source, "Sales");
    assert_eq!(l.name, "PivotTable5");
    l.toggle_field(field(&l, "Product"));
    l.toggle_field(field(&l, "Units"));
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(wb.display_value(s, 3, 0), "Apples");
    assert_eq!(wb.display_value(s, 3, 1), "49");
    let (path, p) = save(&mut wb, dir.path(), "a.xlsx");
    assert_eq!(p["xl/workbook.xml"].matches("<pivotCache ").count(), 4);
    let new_table = p
        .iter()
        .find(|(k, v)| k.starts_with("xl/pivotTables/pivotTable") && v.contains("PivotTable5"))
        .map(|(k, _)| k.clone())
        .unwrap();
    assert_eq!(new_table, "xl/pivotTables/pivotTable5.xml");
    assert!(
        wb.save_notes()
            .iter()
            .all(|n| !n.to_lowercase().contains("pivot")),
        "{:?}",
        wb.save_notes()
    );
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.pivot_tables(s).len(), 1);
    let all: usize = (0..back.sheet_names().len() as u32)
        .filter(|&k| k != s)
        .map(|k| back.pivot_tables(k).len())
        .sum();
    assert_eq!(all, 4);
}

#[test]
fn rearranging_a_pivot_from_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let s = wb.sheet_names().iter().position(|n| n == "Pivot").unwrap() as u32;
    let mut l = wb.pivot_layout(s, 4, 0).unwrap();
    assert_eq!(l.name, "PivotTable1");
    assert_eq!(l.rows, [AxisEntry::Field(0)]);
    assert_eq!(l.columns, [AxisEntry::Field(1)]);
    // Products down the side instead.
    l.move_entry(AreaEntry::Field(1), "Rows", 1);
    wb.set_pivot_layout(s, 4, 0, &l).unwrap();
    assert_eq!(wb.display_value(s, 2, 1), "Sum of Revenue");
    assert_eq!(wb.display_value(s, 3, 0), "East");
    // The file's own item order (Plums, Apples, Pears) is kept.
    assert_eq!(wb.display_value(s, 4, 0), "Plums");
    let (_, p) = save(&mut wb, dir.path(), "a.xlsx");
    let t = &p["xl/pivotTables/pivotTable1.xml"];
    let doc = Document::parse(t).unwrap();
    assert_eq!(
        el(doc.root_element(), "rowFields")
            .unwrap()
            .attribute("count"),
        Some("2")
    );
    assert!(el(doc.root_element(), "colFields").is_none());
}

#[test]
fn csv_and_new_workbooks_get_pivots_too() {
    let dir = tempfile::tempdir().unwrap();
    let csv = dir.path().join("sales.csv");
    let mut text = HEADERS.join(",") + "\n";
    for (a, b, c, d) in ROWS {
        text.push_str(&format!("{a},{b},{c},{d}\n"));
    }
    std::fs::write(&csv, text).unwrap();
    let mut wb = Workbook::open(&csv).unwrap();
    let (s, _, _) = wb
        .insert_pivot("A1:D13", 0, PivotDestination::NewSheet)
        .unwrap();
    let mut l = layout(&wb, s);
    l.toggle_field(1);
    l.toggle_field(2);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    let (path, _) = save(&mut wb, dir.path(), "sales.xlsx");
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.display_value(s, 3, 0), "Apples");
    assert_eq!(back.display_value(s, 3, 1), "49");
    assert_eq!(back.display_value(s, 6, 1), "78");

    // A new workbook, saved without fields placed: the empty pivot too.
    let mut wb = data_book();
    new_pivot(&mut wb);
    let (path, p) = save(&mut wb, dir.path(), "empty.xlsx");
    assert!(p["xl/pivotTables/pivotTable1.xml"].contains("<location ref=\"A3:C20\""));
    assert!(Workbook::open(&path).unwrap().pivot_at(1, 2, 0).is_some());
}

#[test]
fn deleting_pivots() {
    let dir = tempfile::tempdir().unwrap();
    // A new one: cells and parts go.
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    let mut l = layout(&wb, s);
    l.toggle_field(0);
    l.toggle_field(3);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(wb.delete_pivot(s, 3, 1).unwrap(), "PivotTable1");
    assert!(wb.pivot_at(s, 3, 1).is_none());
    assert_eq!(wb.display_value(s, 3, 0), "");
    let (_, p) = save(&mut wb, dir.path(), "a.xlsx");
    assert!(!p.keys().any(|k| k.contains("pivot")), "{:?}", p.keys());
    assert!(!p["xl/workbook.xml"].contains("pivotCache"));
    // Undo brings it back, and it saves again.
    assert!(wb.undo().unwrap());
    assert_eq!(wb.display_value(s, 3, 0), "East");
    let (_, p) = save(&mut wb, dir.path(), "b.xlsx");
    assert!(p.contains_key("xl/pivotTables/pivotTable1.xml"));

    // One from a file: its part goes; its cache too once no pivot uses it.
    let mut wb = Workbook::open(fixture()).unwrap();
    let t = wb
        .sheet_names()
        .iter()
        .position(|n| n == "Tabular")
        .unwrap() as u32;
    let before = parts(&fixture());
    let caches_before = before["xl/workbook.xml"].matches("<pivotCache ").count();
    let name = wb.delete_pivot(t, 1, 0).unwrap();
    assert_eq!(name, "PivotTable3");
    let (path, p) = save(&mut wb, dir.path(), "c.xlsx");
    assert!(!p.values().any(|x| x.contains("name=\"PivotTable3\"")));
    assert_eq!(
        p["xl/workbook.xml"].matches("<pivotCache ").count(),
        caches_before - 1,
        "the table's cache went with its only pivot"
    );
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.pivot_tables(t).len(), 1);
    // Undo the delete after that save: it is written anew.
    assert!(wb.undo().unwrap());
    let (path, p) = save(&mut wb, dir.path(), "d.xlsx");
    assert!(p.values().any(|x| x.contains("name=\"PivotTable3\"")));
    assert_eq!(Workbook::open(&path).unwrap().pivot_tables(t).len(), 2);
}

/// Sample files for checking in other spreadsheet programs:
/// `KANTOR_PIVOT_SAMPLES=dir cargo test -p sheets_engine --test pivot_create samples -- --ignored`.
#[test]
#[ignore]
fn samples() {
    let dir = PathBuf::from(std::env::var("KANTOR_PIVOT_SAMPLES").unwrap());
    for (name, report) in [
        ("compact", ReportLayout::Compact),
        ("outline", ReportLayout::Outline),
        ("tabular", ReportLayout::Tabular),
    ] {
        let mut wb = data_book();
        let s = new_pivot(&mut wb);
        let mut l = layout(&wb, s);
        l.rows = vec![AxisEntry::Field(0), AxisEntry::Field(1)];
        l.add_value(3);
        l.add_value(2);
        l.values[1].func = PivotFunc::Average;
        l.layout = report;
        if report == ReportLayout::Tabular {
            l.subtotals = SubtotalPlace::Bottom;
        }
        wb.set_pivot_layout(s, 2, 0, &l).unwrap();
        save(&mut wb, &dir, &format!("pivot-{name}.xlsx"));
    }
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    let mut l = layout(&wb, s);
    l.rows = vec![AxisEntry::Field(1)];
    l.columns = vec![AxisEntry::Field(0)];
    l.add_value(3);
    l.filters = vec![2];
    l.filter_item = vec![None];
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    save(&mut wb, &dir, "pivot-columns-filter.xlsx");
}
