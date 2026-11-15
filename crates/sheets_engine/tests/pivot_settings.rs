//! Pivot table settings made in Kantor: grouping, calculated fields, value
//! field settings (summarise by, show values as), field settings
//! (subtotals, layout, filters, sorting), styles, shared caches and sheet
//! copies; each saved so Kantor (and Excel's schema) read it back.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pivot_check::{parts, pivot_problems};
use roxmltree::Document;
use sheets_engine::{
    AxisEntry, DateLevel, FieldSubtotals, PivotBaseItem, PivotDestination, PivotFieldFilter,
    PivotFunc, PivotGrouping, PivotLayout, PivotSort, ReportLayout, ShowAs, Workbook,
};

/// Date, Region, Product, Units, Revenue.
const ROWS: [(&str, &str, &str, f64, f64); 12] = [
    ("2025-12-05", "East", "Apples", 10.0, 120.0),
    ("2025-12-20", "West", "Pears", 4.0, 60.0),
    ("2026-01-03", "North", "Apples", 7.0, 84.0),
    ("2026-01-15", "East", "Pears", 3.0, 45.0),
    ("2026-01-28", "West", "Apples", 12.0, 144.0),
    ("2026-02-02", "North", "Plums", 5.0, 90.0),
    ("2026-02-14", "East", "Plums", 8.0, 144.0),
    ("2026-02-27", "West", "Plums", 2.0, 36.0),
    ("2026-03-01", "North", "Pears", 6.0, 90.0),
    ("2026-03-09", "East", "Apples", 9.0, 108.0),
    ("2026-03-17", "West", "Pears", 1.0, 15.0),
    ("2026-03-30", "North", "Apples", 11.0, 132.0),
];

fn data_book() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let mut cells: Vec<(u32, u32, String)> = ["Date", "Region", "Product", "Units", "Revenue"]
        .iter()
        .enumerate()
        .map(|(c, h)| (0, c as u32, h.to_string()))
        .collect();
    for (r, (date, region, product, units, revenue)) in ROWS.iter().enumerate() {
        let r = r as u32 + 1;
        cells.push((r, 0, date.to_string()));
        cells.push((r, 1, region.to_string()));
        cells.push((r, 2, product.to_string()));
        cells.push((r, 3, units.to_string()));
        cells.push((r, 4, revenue.to_string()));
    }
    wb.set_inputs(0, &cells).unwrap();
    wb
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

/// The first column of the pivot's body, top to bottom.
fn labels(wb: &Workbook, sheet: u32) -> Vec<String> {
    let p = wb.pivot_at(sheet, 2, 0).unwrap();
    (p.area.top..=p.area.bottom)
        .map(|r| wb.display_value(sheet, r, p.area.left))
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
    if std::env::var("KDUMP").is_ok() {
        for (n, x) in &p {
            if n.contains("pivot") {
                eprintln!("== {n}\n{x}");
            }
        }
    }
    assert_eq!(pivot_problems(&p), Vec::<String>::new());
    for (name, xml) in &p {
        if name.ends_with(".xml") || name.ends_with(".rels") {
            assert!(Document::parse(xml).is_ok(), "{name} is well-formed");
        }
    }
    if let Ok(dir) = std::env::var("KANTOR_PIVOT_SAMPLES") {
        std::fs::copy(&path, Path::new(&dir).join(name)).unwrap();
    }
    (path, p)
}

/// A pivot on a new sheet over the data; returns its sheet.
fn new_pivot(wb: &mut Workbook) -> u32 {
    let (sheet, _, _) = wb
        .insert_pivot("Sheet1!$A$1:$E$13", 0, PivotDestination::NewSheet)
        .unwrap();
    sheet
}

fn layout(wb: &Workbook, sheet: u32) -> PivotLayout {
    wb.pivot_layout(sheet, 2, 0).unwrap()
}

fn field(l: &PivotLayout, name: &str) -> usize {
    l.fields
        .iter()
        .position(|f| f.name == name)
        .unwrap_or_else(|| panic!("no field {name}"))
}

/// Rows `rows`, Sum of Revenue.
fn arrange(wb: &mut Workbook, s: u32, rows: &[&str]) {
    let mut l = layout(wb, s);
    l.rows = rows
        .iter()
        .map(|n| AxisEntry::Field(field(&l, n)))
        .collect();
    l.values.clear();
    l.add_value(field(&l, "Revenue"));
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
}

fn dates(levels: &[DateLevel]) -> PivotGrouping {
    PivotGrouping::Dates {
        start: 0.0,
        end: 0.0,
        auto_start: true,
        auto_end: true,
        levels: levels.to_vec(),
        days: 1,
    }
}

#[test]
fn the_data_has_dates() {
    let wb = data_book();
    let mut wb = wb;
    let s = new_pivot(&mut wb);
    let l = layout(&wb, s);
    assert!(l.fields[0].dates, "typed ISO dates are dates");
}

#[test]
fn grouping_dates_by_months_and_years() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Date"]);
    let before = grid(&wb, s, (2, 0, 16, 1));
    wb.group_pivot_field(s, 2, 0, 0, &dates(&[DateLevel::Months, DateLevel::Years]))
        .unwrap();
    // Years is a field of its own, outside the months.
    let l = layout(&wb, s);
    let years = field(&l, "Years");
    assert_eq!(l.rows, [AxisEntry::Field(years), AxisEntry::Field(0)]);
    assert_eq!(l.fields[years].group_base, Some(0));
    assert!(matches!(
        &l.fields[0].grouping,
        Some(PivotGrouping::Dates { levels, .. }) if *levels == [DateLevel::Months, DateLevel::Years]
    ));
    assert_eq!(
        grid(&wb, s, (2, 0, 9, 1)),
        rows(&[
            &["Row Labels", "Sum of Revenue"],
            &["2025", "180"],
            &["Dec", "180"],
            &["2026", "888"],
            &["Jan", "273"],
            &["Feb", "270"],
            &["Mar", "345"],
            &["Grand Total", "1068"],
        ])
    );
    // Saved as Excel does: the months on the field, Years a grouping field.
    let (path, p) = save(&mut wb, dir.path(), "group-dates.xlsx");
    let cache = &p["xl/pivotCache/pivotCacheDefinition1.xml"];
    assert!(cache.contains("groupBy=\"months\""), "{cache}");
    assert!(cache.contains("<cacheField name=\"Years\" numFmtId=\"0\" databaseField=\"0\">"));
    assert!(cache.contains("<s v=\"Jan\"/>"));
    let back = Workbook::open(&path).unwrap();
    assert_eq!(grid(&back, s, (2, 0, 9, 1)), grid(&wb, s, (2, 0, 9, 1)));
    let mut back = back;
    assert_eq!(back.refresh_pivot(s, 2, 0).unwrap(), 1);
    assert_eq!(grid(&back, s, (2, 0, 9, 1)), grid(&wb, s, (2, 0, 9, 1)));
    // Quarters added: regrouping replaces the levels.
    wb.group_pivot_field(
        s,
        2,
        0,
        0,
        &dates(&[DateLevel::Months, DateLevel::Quarters, DateLevel::Years]),
    )
    .unwrap();
    assert_eq!(
        labels(&wb, s)[..6],
        ["Row Labels", "2025", "Qtr4", "Dec", "2026", "Qtr1"]
    );
    // Ungroup: back to the dates, one undo step each.
    wb.ungroup_pivot_field(s, 2, 0, 0, None).unwrap();
    let l = layout(&wb, s);
    assert_eq!(l.fields.len(), 5);
    assert_eq!(grid(&wb, s, (2, 0, 16, 1)), before);
    assert!(wb.undo().unwrap());
    assert_eq!(labels(&wb, s)[2], "Qtr4");
    assert!(wb.undo().unwrap());
    assert_eq!(labels(&wb, s)[2], "Dec");
    assert!(wb.undo().unwrap());
    assert_eq!(grid(&wb, s, (2, 0, 16, 1)), before);
    let (_, p) = save(&mut wb, dir.path(), "ungrouped.xlsx");
    assert!(!p["xl/pivotCache/pivotCacheDefinition1.xml"].contains("fieldGroup"));
}

#[test]
fn grouping_days_and_numbers() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Units"]);
    wb.group_pivot_field(
        s,
        2,
        0,
        3,
        &PivotGrouping::Numbers {
            start: 1.0,
            end: 12.0,
            by: 5.0,
            auto_start: false,
            auto_end: false,
        },
    )
    .unwrap();
    assert_eq!(
        grid(&wb, s, (2, 0, 6, 1)),
        rows(&[
            &["Row Labels", "Sum of Revenue"],
            &["1-5", "246"],
            &["6-10", "546"],
            &["11-15", "276"],
            &["Grand Total", "1068"],
        ])
    );
    let l = layout(&wb, s);
    assert!(matches!(
        l.fields[3].grouping,
        Some(PivotGrouping::Numbers { by, .. }) if by == 5.0
    ));
    let (_, p) = save(&mut wb, dir.path(), "group-numbers.xlsx");
    let cache = &p["xl/pivotCache/pivotCacheDefinition1.xml"];
    assert!(
        cache.contains("<rangePr autoStart=\"0\" autoEnd=\"0\" startNum=\"1\" endNum=\"12\" groupInterval=\"5\"/>"),
        "{cache}"
    );
    // Weeks: seven days from the first date.
    arrange(&mut wb, s, &["Date"]);
    wb.group_pivot_field(
        s,
        2,
        0,
        0,
        &PivotGrouping::Dates {
            start: 0.0,
            end: 0.0,
            auto_start: true,
            auto_end: true,
            levels: vec![DateLevel::Days],
            days: 28,
        },
    )
    .unwrap();
    assert_eq!(
        labels(&wb, s)[1..3],
        ["12/5/2025 - 1/1/2026", "1/2/2026 - 1/29/2026"]
    );
    // Text can't be grouped by ranges.
    assert!(
        wb.group_pivot_field(s, 2, 0, 1, &dates(&[DateLevel::Months]))
            .is_err()
    );
    save(&mut wb, dir.path(), "group-days.xlsx");
}

#[test]
fn grouping_selected_items() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region"]);
    let name = wb
        .group_pivot_items(s, 2, 0, 1, &["East".into(), "West".into()])
        .unwrap();
    assert_eq!(name, "Group1");
    let l = layout(&wb, s);
    let g = field(&l, "Region2");
    assert_eq!(l.rows, [AxisEntry::Field(g), AxisEntry::Field(1)]);
    assert_eq!(
        grid(&wb, s, (2, 0, 9, 1)),
        rows(&[
            &["Row Labels", "Sum of Revenue"],
            &["Group1", "672"],
            &["East", "417"],
            &["West", "255"],
            &["North", "396"],
            &["North", "396"],
            &["Grand Total", "1068"],
            &["", ""],
        ])
    );
    let (path, p) = save(&mut wb, dir.path(), "group-items.xlsx");
    let cache = &p["xl/pivotCache/pivotCacheDefinition1.xml"];
    assert!(cache.contains("<discretePr count=\"3\">"), "{cache}");
    assert!(cache.contains("<s v=\"Group1\"/>"));
    let back = Workbook::open(&path).unwrap();
    assert_eq!(grid(&back, s, (2, 0, 8, 1)), grid(&wb, s, (2, 0, 8, 1)));
    // Ungrouping the one group takes the grouping field away.
    wb.ungroup_pivot_field(s, 2, 0, g, Some("Group1")).unwrap();
    let l = layout(&wb, s);
    assert!(l.fields.iter().all(|f| f.name != "Region2"));
    assert_eq!(labels(&wb, s)[1], "East");
    assert!(wb.undo().unwrap());
    assert_eq!(labels(&wb, s)[1], "Group1");
}

#[test]
fn calculated_fields() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region"]);
    assert!(wb.check_pivot_formula(s, 2, 0, "=Revenue/Cost").is_err());
    assert!(
        wb.set_pivot_calculated_field(s, 2, 0, "Region", "=Units*2")
            .is_err(),
        "a source field's name"
    );
    wb.set_pivot_calculated_field(s, 2, 0, "Price", "=Revenue/Units")
        .unwrap();
    // Added to Values, worked out from the sums (East: 417 / 30).
    assert_eq!(
        grid(&wb, s, (2, 0, 6, 2)),
        rows(&[
            &["Row Labels", "Sum of Revenue", "Sum of Price"],
            &["East", "417", "13.9"],
            &["North", "396", "13.65517241"],
            &["West", "255", "13.42105263"],
            &["Grand Total", "1068", "13.69230769"],
        ])
    );
    let l = layout(&wb, s);
    let price = field(&l, "Price");
    assert_eq!(l.fields[price].formula.as_deref(), Some("=Revenue/Units"));
    assert!(l.fields[price].numeric);
    // Saved as Excel does; read back and refreshed the same.
    let (path, p) = save(&mut wb, dir.path(), "calculated.xlsx");
    let cache = &p["xl/pivotCache/pivotCacheDefinition1.xml"];
    assert!(
        cache.contains(
            "<cacheField name=\"Price\" numFmtId=\"0\" formula=\"Revenue/Units\" databaseField=\"0\"/>"
        ),
        "{cache}"
    );
    let mut back = Workbook::open(&path).unwrap();
    assert_eq!(back.pivot_layout(s, 2, 0).unwrap().refresh_problem, None);
    assert_eq!(back.refresh_pivot(s, 2, 0).unwrap(), 1);
    assert_eq!(grid(&back, s, (2, 0, 6, 2)), grid(&wb, s, (2, 0, 6, 2)));
    // Modify, then delete; each one undo step.
    wb.set_pivot_calculated_field(s, 2, 0, "price", "=Revenue*2")
        .unwrap();
    assert_eq!(wb.display_value(s, 3, 2), "834");
    wb.delete_pivot_calculated_field(s, 2, 0, "Price").unwrap();
    assert_eq!(wb.display_value(s, 2, 2), "");
    assert!(layout(&wb, s).fields.iter().all(|f| f.name != "Price"));
    assert!(wb.undo().unwrap());
    assert_eq!(wb.display_value(s, 3, 2), "834");
    assert!(wb.undo().unwrap());
    assert_eq!(wb.display_value(s, 3, 2), "13.9");
    // A calculated field stays in Values.
    let mut l = layout(&wb, s);
    l.rows.push(AxisEntry::Field(field(&l, "Price")));
    assert!(wb.set_pivot_layout(s, 2, 0, &l).is_err());
}
