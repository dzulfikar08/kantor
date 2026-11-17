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

/// Rows Region, columns Product, Sum of Revenue shown as `show`.
fn show_as(
    wb: &mut Workbook,
    s: u32,
    show: ShowAs,
    base: Option<&str>,
    item: PivotBaseItem,
) -> Vec<Vec<String>> {
    let mut l = layout(wb, s);
    l.rows = vec![AxisEntry::Field(1)];
    l.columns = vec![AxisEntry::Field(2)];
    l.values.clear();
    l.add_value(4);
    l.values[0].show_as = show;
    l.values[0].base_field = base.map(|b| field(&l, b));
    l.values[0].base_item = item;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    // East, North, West by Apples, Pears, Plums, Grand Total.
    grid(wb, s, (4, 1, 7, 4))
}

#[test]
fn show_values_as() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    let prev = PivotBaseItem::Previous;
    let g = show_as(&mut wb, s, ShowAs::Normal, None, prev.clone());
    assert_eq!(g[0], ["228", "45", "144", "417"]);
    let g = show_as(&mut wb, s, ShowAs::PercentOfTotal, None, prev.clone());
    assert_eq!(g[0][0], "21.35%");
    assert_eq!(g[3][3], "100.00%");
    let g = show_as(&mut wb, s, ShowAs::PercentOfColumn, None, prev.clone());
    assert_eq!(g[0][0], "38.78%");
    assert_eq!(g[3][0], "100.00%");
    let g = show_as(&mut wb, s, ShowAs::PercentOfRow, None, prev.clone());
    assert_eq!(g[0][0], "54.68%");
    assert_eq!(g[0][3], "100.00%");
    let g = show_as(
        &mut wb,
        s,
        ShowAs::PercentOfParentColumn,
        None,
        prev.clone(),
    );
    assert_eq!(
        g[0][0], "54.68%",
        "the parent of a column is the row's total"
    );
    let north = PivotBaseItem::Item("North".into());
    let g = show_as(&mut wb, s, ShowAs::Percent, Some("Region"), north.clone());
    assert_eq!(g[0][0], "105.56%");
    assert_eq!(g[1][0], "100.00%");
    assert_eq!(g[3][0], "", "no Region item in the grand total");
    let g = show_as(&mut wb, s, ShowAs::Difference, Some("Region"), prev.clone());
    assert_eq!(g[0][0], "", "East has nothing before it");
    assert_eq!(g[1][0], "-12");
    assert_eq!(g[2][1], "-15");
    let east = PivotBaseItem::Item("East".into());
    let g = show_as(&mut wb, s, ShowAs::PercentDifference, Some("Region"), east);
    assert_eq!(g[1][0], "-5.26%");
    let g = show_as(
        &mut wb,
        s,
        ShowAs::RunningTotal,
        Some("Region"),
        prev.clone(),
    );
    assert_eq!(g[2][0], "588");
    assert_eq!(g[1][1], "135");
    let g = show_as(
        &mut wb,
        s,
        ShowAs::PercentRunningTotal,
        Some("Region"),
        prev.clone(),
    );
    assert_eq!(g[1][0], "75.51%");
    assert_eq!(g[2][0], "100.00%");
    let g = show_as(
        &mut wb,
        s,
        ShowAs::RankDescending,
        Some("Region"),
        prev.clone(),
    );
    assert_eq!([&g[0][0], &g[1][0], &g[2][0]], ["1", "2", "3"]);
    let g = show_as(
        &mut wb,
        s,
        ShowAs::RankAscending,
        Some("Region"),
        prev.clone(),
    );
    assert_eq!(g[2][0], "1");
    let g = show_as(&mut wb, s, ShowAs::Index, None, prev.clone());
    let index: f64 = g[0][0].parse().unwrap();
    assert!(
        (index - 228.0 * 1068.0 / (417.0 * 588.0)).abs() < 1e-6,
        "{index}"
    );
    // A base field is needed.
    let mut l = layout(&wb, s);
    l.values[0].show_as = ShowAs::Difference;
    l.values[0].base_field = None;
    assert!(wb.set_pivot_layout(s, 2, 0, &l).is_err());

    // Parents with two row fields.
    let mut l = layout(&wb, s);
    l.rows = vec![AxisEntry::Field(1), AxisEntry::Field(2)];
    l.columns.clear();
    l.values[0].show_as = ShowAs::PercentOfParentRow;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (3, 0, 4, 1)),
        rows(&[&["East", "39.04%"], &["Apples", "54.68%"]])
    );
    let mut l = layout(&wb, s);
    l.values[0].show_as = ShowAs::PercentOfParent;
    l.values[0].base_field = Some(1);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (3, 0, 4, 1)),
        rows(&[&["East", "100.00%"], &["Apples", "54.68%"]])
    );
    assert_eq!(layout(&wb, s).values[0].show_as, ShowAs::PercentOfParent);

    // Saved: the 2010 kinds in an x14 extension; read back the same.
    let (path, p) = save(&mut wb, dir.path(), "show-as.xlsx");
    let table = &p["xl/pivotTables/pivotTable1.xml"];
    assert!(table.contains("pivotShowAs=\"percentOfParent\""), "{table}");
    let back = Workbook::open(&path).unwrap();
    let bl = back.pivot_layout(s, 2, 0).unwrap();
    assert_eq!(bl.values, layout(&wb, s).values);
    assert_eq!(bl.refresh_problem, None);
    let mut l = layout(&wb, s);
    l.values[0].show_as = ShowAs::Difference;
    l.values[0].base_item = PivotBaseItem::Item("Pears".into());
    l.values[0].base_field = Some(2);
    l.values[0].func = PivotFunc::Average;
    l.values[0].num_fmt = Some("#,##0.00".into());
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    let (path, p) = save(&mut wb, dir.path(), "difference.xlsx");
    let table = &p["xl/pivotTables/pivotTable1.xml"];
    assert!(
        table.contains(
            "subtotal=\"average\" showDataAs=\"difference\" baseField=\"2\" baseItem=\"1\""
        ),
        "{table}"
    );
    let back = Workbook::open(&path).unwrap();
    assert_eq!(
        back.pivot_layout(s, 2, 0).unwrap().values,
        layout(&wb, s).values
    );
}

fn filter(kind: &str, value_field: Option<usize>, v1: &str, v2: &str) -> PivotFieldFilter {
    PivotFieldFilter {
        kind: kind.into(),
        value_field,
        value1: v1.into(),
        value2: v2.into(),
        top: true,
    }
}

#[test]
fn field_settings_subtotals_and_layout() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region", "Product"]);
    // Custom subtotals for Region: Sum and Count, below its items.
    let mut l = layout(&wb, s);
    l.fields[1].settings.subtotals = FieldSubtotals::Custom(vec![PivotFunc::Count, PivotFunc::Sum]);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (3, 0, 8, 1)),
        rows(&[
            &["East", ""],
            &["Apples", "228"],
            &["Pears", "45"],
            &["Plums", "144"],
            &["East Sum", "417"],
            &["East Count", "4"],
        ])
    );
    assert_eq!(
        layout(&wb, s).fields[1].settings.subtotals,
        FieldSubtotals::Custom(vec![PivotFunc::Sum, PivotFunc::Count])
    );
    // Region in tabular form with its labels repeated and a blank line
    // after each region.
    let mut l = layout(&wb, s);
    l.fields[1].settings.subtotals = FieldSubtotals::None;
    l.fields[1].settings.form = ReportLayout::Tabular;
    l.fields[1].settings.repeat_labels = true;
    l.fields[1].settings.blank_line = true;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (2, 0, 7, 2)),
        rows(&[
            &["Region", "Product", "Sum of Revenue"],
            &["East", "Apples", "228"],
            &["East", "Pears", "45"],
            &["East", "Plums", "144"],
            &["", "", ""],
            &["North", "Apples", "216"],
        ])
    );
    let got = layout(&wb, s).fields[1].settings.clone();
    assert_eq!(got.form, ReportLayout::Tabular);
    assert!(got.repeat_labels && got.blank_line);
    let (path, p) = save(&mut wb, dir.path(), "field-settings.xlsx");
    let table = &p["xl/pivotTables/pivotTable1.xml"];
    assert!(table.contains("insertBlankRow=\"1\""), "{table}");
    assert!(table.contains("fillDownLabels=\"1\""), "{table}");
    assert!(table.contains("<i t=\"blank\">"), "{table}");
    let back = Workbook::open(&path).unwrap();
    assert_eq!(grid(&back, s, (2, 0, 7, 2)), grid(&wb, s, (2, 0, 7, 2)));
    assert_eq!(back.pivot_layout(s, 2, 0).unwrap().fields[1].settings, got);
    // The report layout still sets every field.
    let mut l = layout(&wb, s);
    l.layout = ReportLayout::Outline;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        layout(&wb, s).fields[1].settings.form,
        ReportLayout::Outline
    );
    // Custom subtotals saved and read back.
    let mut l = layout(&wb, s);
    l.fields[1].settings.subtotals = FieldSubtotals::Custom(vec![PivotFunc::Max]);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    let (path, p) = save(&mut wb, dir.path(), "custom-subtotals.xlsx");
    let table = &p["xl/pivotTables/pivotTable1.xml"];
    assert!(table.contains("maxSubtotal=\"1\""), "{table}");
    assert!(table.contains("<item t=\"max\"/>"), "{table}");
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.pivot_layout(s, 2, 0).unwrap().refresh_problem, None);
    assert_eq!(grid(&back, s, (2, 0, 9, 2)), grid(&wb, s, (2, 0, 9, 2)));
}

/// PivotTable Options' "Allow multiple filters per field": a label filter
/// and a value filter on one field at once, saved as Excel writes them.
#[test]
fn multiple_filters_per_field() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region"]);
    assert!(!layout(&wb, s).multiple_filters, "Excel's default: off");
    let mut l = layout(&wb, s);
    l.fields[1].settings.filter = Some(filter("captionNotEqual", None, "North", ""));
    l.fields[1].settings.filter2 = Some(filter("valueGreaterThan", Some(0), "300", ""));
    assert!(
        wb.set_pivot_layout(s, 2, 0, &l).is_err(),
        "two filters need the option"
    );
    l.multiple_filters = true;
    let mut two_labels = l.clone();
    two_labels.fields[1].settings.filter2 = Some(filter("captionBeginsWith", None, "E", ""));
    assert!(wb.set_pivot_layout(s, 2, 0, &two_labels).is_err());
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    // Not North, and more than 300: East only.
    assert_eq!(labels(&wb, s), ["Row Labels", "East", "Grand Total"]);
    let got = layout(&wb, s);
    assert!(got.multiple_filters);
    assert_eq!(got.fields[1].settings.filter2, l.fields[1].settings.filter2);
    let (path, p) = save(&mut wb, dir.path(), "multiple-filters.xlsx");
    let table = &p["xl/pivotTables/pivotTable1.xml"];
    assert!(table.contains("multipleFieldFilters=\"1\""), "{table}");
    assert!(table.contains("<filters count=\"2\">"), "{table}");
    let back = Workbook::open(&path).unwrap();
    let bl = back.pivot_layout(s, 2, 0).unwrap();
    assert!(bl.multiple_filters);
    assert_eq!(bl.fields[1].settings, got.fields[1].settings);
    assert_eq!(labels(&back, s), labels(&wb, s));
    // Off again: one filter per field is written.
    let mut l = layout(&wb, s);
    l.multiple_filters = false;
    l.fields[1].settings.filter2 = None;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    let (_, p) = save(&mut wb, dir.path(), "multiple-filters-off.xlsx");
    assert!(p["xl/pivotTables/pivotTable1.xml"].contains("multipleFieldFilters=\"0\""));
    assert!(wb.undo().unwrap());
    assert!(layout(&wb, s).multiple_filters);
}

#[test]
fn sorting_and_filters() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region"]);
    // Z to A.
    let mut l = layout(&wb, s);
    l.fields[1].settings.sort = PivotSort::Descending;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(labels(&wb, s)[1..4], ["West", "North", "East"]);
    // By value, smallest first.
    let mut l = layout(&wb, s);
    l.fields[1].settings.sort = PivotSort::Ascending;
    l.fields[1].settings.sort_by_value = Some(0);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(labels(&wb, s)[1..4], ["West", "North", "East"]);
    let (_, p) = save(&mut wb, dir.path(), "sort-by-value.xlsx");
    assert!(p["xl/pivotTables/pivotTable1.xml"].contains("<autoSortScope>"));
    let mut l = layout(&wb, s);
    l.fields[1].settings.sort_by_value = None;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(labels(&wb, s)[1..4], ["East", "North", "West"]);

    // Label filters.
    let mut l = layout(&wb, s);
    l.fields[1].settings.filter = Some(filter("captionBeginsWith", None, "n", ""));
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(labels(&wb, s), ["Row Labels", "North", "Grand Total"]);
    assert_eq!(wb.display_value(s, 4, 1), "396");
    let mut l = layout(&wb, s);
    l.fields[1].settings.filter = Some(filter("captionNotEqual", None, "west", ""));
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(labels(&wb, s)[1..3], ["East", "North"]);
    let (path, p) = save(&mut wb, dir.path(), "label-filter.xlsx");
    assert!(p["xl/pivotTables/pivotTable1.xml"].contains("type=\"captionNotEqual\""));
    let back = Workbook::open(&path).unwrap();
    let bl = back.pivot_layout(s, 2, 0).unwrap();
    assert_eq!(bl.refresh_problem, None);
    assert_eq!(
        bl.fields[1].settings.filter,
        layout(&wb, s).fields[1].settings.filter
    );

    // Value filters: greater than, between, Top 10.
    let mut l = layout(&wb, s);
    l.fields[1].settings.filter = Some(filter("valueGreaterThan", Some(0), "300", ""));
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(labels(&wb, s)[1..4], ["East", "North", "Grand Total"]);
    let mut l = layout(&wb, s);
    l.fields[1].settings.filter = Some(filter("valueBetween", Some(0), "250", "400"));
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(labels(&wb, s)[1..4], ["North", "West", "Grand Total"]);
    let mut l = layout(&wb, s);
    let mut top = filter("count", Some(0), "1", "");
    top.top = false;
    l.fields[1].settings.filter = Some(top);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(labels(&wb, s)[1..3], ["West", "Grand Total"]);
    let (path, _) = save(&mut wb, dir.path(), "top10.xlsx");
    let back = Workbook::open(&path).unwrap();
    assert_eq!(
        back.pivot_layout(s, 2, 0).unwrap().fields[1]
            .settings
            .filter,
        layout(&wb, s).fields[1].settings.filter
    );
    // A value filter needs its value field.
    let mut l = layout(&wb, s);
    l.fields[1].settings.filter = Some(filter("valueGreaterThan", None, "1", ""));
    assert!(wb.set_pivot_layout(s, 2, 0, &l).is_err());

    // Date filters on the dates.
    arrange(&mut wb, s, &["Date"]);
    let mut l = layout(&wb, s);
    l.fields[1].settings.filter = None;
    l.fields[0].settings.filter = Some(filter("dateBetween", None, "2026-01-01", "1/31/2026"));
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(labels(&wb, s).len(), 5, "three January days");
    let mut l = layout(&wb, s);
    l.fields[0].settings.filter = Some(filter("Q1", None, "", ""));
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(labels(&wb, s).len(), 12, "January to March");
    let (path, p) = save(&mut wb, dir.path(), "date-filter.xlsx");
    assert!(p["xl/pivotTables/pivotTable1.xml"].contains("<dynamicFilter type=\"Q1\"/>"));
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.pivot_layout(s, 2, 0).unwrap().refresh_problem, None);
    // Undo takes the filter away.
    assert!(wb.undo().unwrap());
    assert_eq!(labels(&wb, s).len(), 5);
}

#[test]
fn style_picker_options() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region", "Product"]);
    assert_eq!(sheets_engine::pivot_style_names().len(), 84);
    let l = layout(&wb, s);
    assert_eq!(l.style.name.as_deref(), Some("PivotStyleLight16"));
    let opts = l.style.clone();
    for name in sheets_engine::pivot_style_names() {
        let p = wb.pivot_style_preview(&name, &opts).unwrap();
        assert_eq!((p.len(), p[0].len()), (8, 4), "{name}");
    }
    // Medium 9 has a filled header; banded rows tint every other row.
    let plain = wb.pivot_style_preview("PivotStyleMedium9", &opts).unwrap();
    assert!(plain[0][1].fill.is_some());
    let mut banded = opts.clone();
    banded.banded_rows = true;
    let b = wb.pivot_style_preview("PivotStyleLight8", &banded).unwrap();
    let p = wb.pivot_style_preview("PivotStyleLight8", &opts).unwrap();
    assert_ne!(b, p);
    // Picked: the cells and the saved file follow.
    let mut l = layout(&wb, s);
    l.style = sheets_engine::PivotStyleOptions {
        name: Some("PivotStyleDark3".into()),
        row_headers: false,
        column_headers: true,
        banded_rows: true,
        banded_columns: false,
    };
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(layout(&wb, s).style, l.style);
    let (path, p) = save(&mut wb, dir.path(), "style.xlsx");
    let table = &p["xl/pivotTables/pivotTable1.xml"];
    assert!(
        table.contains("<pivotTableStyleInfo name=\"PivotStyleDark3\" showRowHeaders=\"0\" showColHeaders=\"1\" showRowStripes=\"1\""),
        "{table}"
    );
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.pivot_layout(s, 2, 0).unwrap().style, l.style);
    // None.
    let mut l = layout(&wb, s);
    l.style.name = None;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(layout(&wb, s).style.name, None);
    assert!(wb.undo().unwrap());
    assert_eq!(
        layout(&wb, s).style.name.as_deref(),
        Some("PivotStyleDark3")
    );
}

#[test]
fn a_second_pivot_on_the_same_data_shares_its_cache() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region"]);
    let (t, _, _) = wb
        .insert_pivot(
            "Sheet1!$A$1:$E$13",
            0,
            PivotDestination::Cell {
                sheet: s,
                row: 2,
                col: 5,
            },
        )
        .unwrap();
    assert_eq!(t, s);
    let mut l = wb.pivot_layout(s, 2, 5).unwrap();
    l.rows = vec![AxisEntry::Field(2)];
    l.add_value(3);
    wb.set_pivot_layout(s, 2, 5, &l).unwrap();
    // Grouping the shared cache changes both, as in Excel.
    wb.group_pivot_items(s, 2, 0, 1, &["East".into(), "West".into()])
        .unwrap();
    assert!(
        wb.pivot_layout(s, 2, 5)
            .unwrap()
            .fields
            .iter()
            .any(|f| f.name == "Region2")
    );
    let (path, p) = save(&mut wb, dir.path(), "shared-cache.xlsx");
    assert_eq!(p["xl/workbook.xml"].matches("<pivotCache ").count(), 1);
    assert!(!p.contains_key("xl/pivotCache/pivotCacheDefinition2.xml"));
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.pivot_tables(s).len(), 2);
    // A third one after reopening shares the cache from the file.
    let mut back = back;
    back.insert_pivot("Sheet1!$A$1:$E$13", 0, PivotDestination::NewSheet)
        .unwrap();
    let (_, p) = save(&mut back, dir.path(), "shared-cache-2.xlsx");
    assert_eq!(p["xl/workbook.xml"].matches("<pivotCache ").count(), 1);
    assert_eq!(
        p.keys()
            .filter(|k| k.starts_with("xl/pivotTables/pivotTable") && k.ends_with(".xml"))
            .count(),
        3
    );
    // Another source: a cache of its own.
    wb.insert_pivot("Sheet1!$A$1:$D$13", 0, PivotDestination::NewSheet)
        .unwrap();
    let (_, p) = save(&mut wb, dir.path(), "two-caches.xlsx");
    assert_eq!(p["xl/workbook.xml"].matches("<pivotCache ").count(), 2);
}

#[test]
fn copying_a_sheet_brings_its_pivots() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region"]);
    let copy = wb.copy_sheet(s, 99).unwrap();
    let p = wb.pivot_at(copy, 2, 0).unwrap();
    // Excel keeps the name: pivot names are unique per sheet.
    assert_eq!(p.name, "PivotTable1");
    assert_eq!(grid(&wb, copy, (2, 0, 6, 1)), grid(&wb, s, (2, 0, 6, 1)));
    // A change made through the copy places fields on the copy only, even
    // with the same name.
    let region = field(&layout(&wb, copy), "Region");
    wb.group_pivot_items(copy, 2, 0, region, &["East".into(), "West".into()])
        .unwrap();
    assert_eq!(layout(&wb, copy).rows.len(), 2);
    assert_eq!(layout(&wb, s).rows.len(), 1);
    assert!(wb.undo().unwrap());
    // Its own pivot: changing it leaves the original alone.
    let mut l = wb.pivot_layout(copy, 2, 0).unwrap();
    l.rows = vec![AxisEntry::Field(2)];
    wb.set_pivot_layout(copy, 2, 0, &l).unwrap();
    assert_eq!(wb.display_value(copy, 3, 0), "Apples");
    assert_eq!(wb.display_value(s, 3, 0), "East");
    let (path, p) = save(&mut wb, dir.path(), "copied-sheet.xlsx");
    assert_eq!(p["xl/workbook.xml"].matches("<pivotCache ").count(), 1);
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.pivot_tables(copy).len(), 1);
    assert_eq!(back.display_value(copy, 3, 0), "Apples");
    // One undo step takes the copy and its pivot away.
    assert!(wb.undo().unwrap());
    assert!(wb.undo().unwrap());
    assert_eq!(wb.sheet_names().len(), 2);
    assert_eq!(wb.pivot_tables(s).len(), 1);
    // A pivot from a file, never laid out here, copies too.
    let mut back = Workbook::open(&path).unwrap();
    let c2 = back.copy_sheet(s, 99).unwrap();
    assert_eq!(grid(&back, c2, (2, 0, 6, 1)), grid(&back, s, (2, 0, 6, 1)));
    let (path, _) = save(&mut back, dir.path(), "copied-file-pivot.xlsx");
    let again = Workbook::open(&path).unwrap();
    assert_eq!(again.pivot_tables(c2).len(), 1);
    let mut again = again;
    assert_eq!(
        again.refresh_pivot(c2, 2, 0).unwrap(),
        3,
        "all three share the cache"
    );
}

/// Settings as Excel writes them (scripts/corpus/gen_pivot_xlsx.py):
/// grouping, a calculated field, Show Values As, custom subtotals,
/// filters and sorting by value, all refreshed and saved.
#[test]
fn settings_written_by_excel_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx/23_pivot_settings.xlsx");
    let mut wb = Workbook::open(&path).unwrap();
    let s = wb
        .sheet_names()
        .iter()
        .position(|n| n == "Settings")
        .unwrap() as u32;
    let pivots = wb.pivot_tables(s);
    assert_eq!(pivots.len(), 3);
    for p in &pivots {
        assert_eq!(p.refresh_problem, None, "{}", p.name);
    }
    assert_eq!(wb.refresh_all_pivots().unwrap(), 3);
    // Years > months, % of column total.
    assert_eq!(
        grid(&wb, s, (2, 0, 8, 1)),
        rows(&[
            &["Row Labels", "Sum of Revenue"],
            &["2026", "100.00%"],
            &["Jan", "16.85%"],
            &["Feb", "25.56%"],
            &["Mar", "33.71%"],
            &["Apr", "23.88%"],
            &["Grand Total", "100.00%"],
        ])
    );
    // Tabular with custom subtotals, a blank line, the West filtered out.
    assert_eq!(
        grid(&wb, s, (3, 4, 7, 7)),
        rows(&[
            &["Group1", "East", "13.9", "100.00%"],
            &["Group1 Sum", "", "13.9", "50.85%"],
            &["Group1 Count", "", "13.9", "50.00%"],
            &["", "", "", ""],
            &["North", "North", "13.65517241", "100.00%"],
        ])
    );
    // Top 2 products by revenue, largest first.
    assert_eq!(
        grid(&wb, s, (2, 11, 5, 12)),
        rows(&[
            &["Row Labels", "Sum of Revenue"],
            &["Apples", "588"],
            &["Plums", "270"],
            &["Grand Total", "858"],
        ])
    );
    let l = wb.pivot_layout(s, 2, 11).unwrap();
    assert_eq!(l.fields[1].settings.sort, PivotSort::Descending);
    assert_eq!(l.fields[1].settings.sort_by_value, Some(0));
    assert!(l.style.banded_rows);
    let before = grid(&wb, s, (2, 0, 12, 12));
    let (saved, _) = save(&mut wb, dir.path(), "excel-settings.xlsx");
    let mut back = Workbook::open(&saved).unwrap();
    assert_eq!(grid(&back, s, (2, 0, 12, 12)), before);
    assert_eq!(back.refresh_all_pivots().unwrap(), 3);
    assert_eq!(grid(&back, s, (2, 0, 12, 12)), before);
}

#[test]
fn calculated_items() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region"]);
    let region = field(&layout(&wb, s), "Region");
    assert!(
        wb.set_pivot_calculated_item(s, 2, 0, region, "Coast", "=East+South")
            .is_err(),
        "no item South"
    );
    assert!(
        wb.set_pivot_calculated_item(s, 2, 0, region, "East", "=West")
            .is_err(),
        "an item's name"
    );
    wb.set_pivot_calculated_item(s, 2, 0, region, "Coast", "=East+West")
        .unwrap();
    // After the source's items; the grand total includes it, as in Excel.
    assert_eq!(
        grid(&wb, s, (2, 0, 7, 1)),
        rows(&[
            &["Row Labels", "Sum of Revenue"],
            &["East", "417"],
            &["North", "396"],
            &["West", "255"],
            &["Coast", "672"],
            &["Grand Total", "1740"],
        ])
    );
    let l = layout(&wb, s);
    assert_eq!(
        l.fields[region].calc_items,
        vec![("Coast".to_string(), "=East+West".to_string())]
    );
    assert_eq!(l.refresh_problem, None);
    // Per cell with a column field: the formula over the same column.
    let mut l = layout(&wb, s);
    l.columns = vec![AxisEntry::Field(field(&l, "Product"))];
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    // Apples: East 228, West 144.
    assert_eq!(wb.display_value(s, 7, 1), "372");
    // Item names with spaces in quotes, Region[Item] form, constants.
    wb.set_pivot_calculated_item(s, 2, 0, region, "Twice North", "=Region[North]*2")
        .unwrap();
    wb.set_pivot_calculated_item(s, 2, 0, region, "Rest", "='Twice North'-1")
        .unwrap();
    let mut l = layout(&wb, s);
    l.columns.clear();
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(labels(&wb, s)[5..8], ["Twice North", "Rest", "Grand Total"]);
    assert_eq!(wb.display_value(s, 7, 1), "792");
    assert_eq!(wb.display_value(s, 8, 1), "791");
    // Saved as Excel does; read back, refreshed and changed the same.
    let (path, p) = save(&mut wb, dir.path(), "calculated_items.xlsx");
    let cache = &p["xl/pivotCache/pivotCacheDefinition1.xml"];
    assert!(cache.contains("<calculatedItems count=\"3\">"), "{cache}");
    assert!(cache.contains("formula=\"East+West\""), "{cache}");
    let table = &p["xl/pivotTables/pivotTable1.xml"];
    assert!(table.contains("f=\"1\""), "{table}");
    let mut back = Workbook::open(&path).unwrap();
    assert_eq!(back.pivot_layout(s, 2, 0).unwrap().refresh_problem, None);
    assert_eq!(back.refresh_pivot(s, 2, 0).unwrap(), 1);
    assert_eq!(grid(&back, s, (2, 0, 10, 1)), grid(&wb, s, (2, 0, 10, 1)));
    let rl = back.pivot_layout(s, 2, 0).unwrap();
    assert_eq!(rl.fields[region].calc_items.len(), 3);
    // Modify, delete, undo.
    wb.set_pivot_calculated_item(s, 2, 0, region, "coast", "=East")
        .unwrap();
    assert_eq!(wb.display_value(s, 6, 1), "417");
    wb.delete_pivot_calculated_item(s, 2, 0, region, "Rest")
        .unwrap();
    assert!(!labels(&wb, s).contains(&"Rest".to_string()));
    assert!(wb.undo().unwrap());
    assert!(labels(&wb, s).contains(&"Rest".to_string()));
    // Grouping a field with calculated items is refused, as in Excel.
    assert!(
        wb.group_pivot_items(s, 2, 0, region, &["East".into(), "West".into()])
            .is_err()
    );
    // Save again after the edits, then save → undo → save.
    let (_, p2) = save(&mut wb, dir.path(), "calculated_items2.xlsx");
    assert!(p2["xl/pivotCache/pivotCacheDefinition1.xml"].contains("formula=\"East\""));
    assert!(wb.undo().unwrap());
    let (_, p3) = save(&mut wb, dir.path(), "calculated_items3.xlsx");
    assert!(p3["xl/pivotCache/pivotCacheDefinition1.xml"].contains("formula=\"East+West\""));
}

/// Calculated items naming items by position: `Region[2]` (the second
/// item), `Region[-1]` / `Region[+1]` (before / after the calculated item).
#[test]
fn calculated_items_by_position() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region"]);
    let region = field(&layout(&wb, s), "Region");
    // Items: East 417, North 396, West 255, then the calculated ones.
    wb.set_pivot_calculated_item(s, 2, 0, region, "Second", "=Region[2]*2")
        .unwrap();
    wb.set_pivot_calculated_item(s, 2, 0, region, "Before", "=Region[-1]+Region[1]")
        .unwrap();
    assert_eq!(
        grid(&wb, s, (2, 0, 7, 1))[4..6],
        rows(&[&["Second", "792"], &["Before", "1209"]])[..]
    );
    assert!(
        wb.set_pivot_calculated_item(s, 2, 0, region, "Past", "=Region[+1]")
            .is_err(),
        "nothing after a new item"
    );
    assert!(
        wb.set_pivot_calculated_item(s, 2, 0, region, "Nine", "=Region[9]")
            .is_err()
    );
    // Saved with the position as Excel writes it, read back the same.
    let (path, p) = save(&mut wb, dir.path(), "calc_positions.xlsx");
    let cache = &p["xl/pivotCache/pivotCacheDefinition1.xml"];
    assert!(cache.contains("Region[-1]+Region[1]"), "{cache}");
    let mut back = Workbook::open(&path).unwrap();
    assert_eq!(back.refresh_pivot(s, 2, 0).unwrap(), 1);
    assert_eq!(grid(&back, s, (2, 0, 8, 1)), grid(&wb, s, (2, 0, 8, 1)));
    // Sorted by value, smallest first: positions count in the order shown
    // (West 255, North 396, East 417, then the calculated items, listed by
    // name), as in Excel: `Region[1]` is West, Before's `Region[-1]` East.
    let mut l = layout(&wb, s);
    l.fields[region].settings.sort = PivotSort::Ascending;
    l.fields[region].settings.sort_by_value = Some(0);
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert_eq!(
        grid(&wb, s, (2, 0, 8, 1))[1..7],
        rows(&[
            &["West", "255"],
            &["North", "396"],
            &["East", "417"],
            &["Before", "672"],
            &["Second", "792"],
            &["Grand Total", "2532"],
        ])[..]
    );
    // A new one by position: the first shown (West), not the first listed.
    wb.set_pivot_calculated_item(s, 2, 0, region, "Lowest", "=Region[1]")
        .unwrap();
    let g = grid(&wb, s, (2, 0, 9, 1));
    assert!(
        g.contains(&vec!["Lowest".to_string(), "255".to_string()]),
        "{g:?}"
    );
}

#[test]
fn calculated_items_written_by_excel() {
    // A cache and table as Excel writes them: the item is a shared item
    // with a calculatedItem pointing at it.
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region"]);
    let region = field(&layout(&wb, s), "Region");
    wb.set_pivot_calculated_item(s, 2, 0, region, "Coast", "=East+West")
        .unwrap();
    let (path, _) = save(&mut wb, dir.path(), "excel_calc.xlsx");
    let back = Workbook::open(&path).unwrap();
    // Opened: shown as saved, the Fields panel works.
    assert_eq!(back.display_value(s, 6, 1), "672");
    assert!(
        back.pivot_layout(s, 2, 0)
            .unwrap()
            .refresh_problem
            .is_none()
    );
}

#[test]
fn compact_inner_items_are_indented() {
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region", "Product"]);
    // Row 3 East (outer), row 4 Apples (inner), ... Grand Total last.
    assert_eq!(wb.display_value(s, 3, 0), "East");
    assert_eq!(wb.display_value(s, 4, 0), "Apples");
    let indent =
        |wb: &Workbook, r: u32, c: u32| wb.table_cell_look(s, r, c).map_or(0, |l| l.indent);
    assert_eq!(indent(&wb, 3, 0), 0);
    assert_eq!(indent(&wb, 4, 0), 1);
    assert_eq!(indent(&wb, 4, 1), 0, "value cells aren't indented");
    let last = wb.pivot_at(s, 2, 0).unwrap().area.bottom;
    assert_eq!(indent(&wb, last, 0), 0, "Grand Total");
    // Outline form: each field in its own column, no indent.
    let mut l = layout(&wb, s);
    l.layout = ReportLayout::Outline;
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    assert!((2..=last).all(|r| indent(&wb, r, 0) == 0 && indent(&wb, r, 1) == 0));
}

fn purple_pivot_style(name: &str) -> sheets_engine::TableStyleSpec {
    use sheets_engine::{TableElement, TableElementFormat};
    let mut spec = sheets_engine::TableStyleSpec::new(name);
    spec.set_format(
        TableElement::HeaderRow,
        TableElementFormat {
            fill: Some(0x7030A0),
            font: Some(0xFFFFFF),
            bold: Some(true),
            ..Default::default()
        },
    );
    spec.set_format(
        TableElement::FirstRowSubheading,
        TableElementFormat {
            fill: Some(0xE4DFEC),
            ..Default::default()
        },
    );
    spec.set_format(
        TableElement::TotalRow,
        TableElementFormat {
            italic: Some(true),
            ..Default::default()
        },
    );
    spec
}

#[test]
fn custom_pivot_styles() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = data_book();
    let s = new_pivot(&mut wb);
    arrange(&mut wb, s, &["Region", "Product"]);
    assert_eq!(wb.next_pivot_style_name(), "PivotTable Style 1");
    wb.add_pivot_style(&purple_pivot_style("Purple"), false)
        .unwrap();
    assert!(
        wb.add_pivot_style(&purple_pivot_style("purple"), false)
            .is_err(),
        "names are unique"
    );
    assert_eq!(wb.custom_pivot_styles(), vec!["Purple".to_string()]);
    assert!(wb.custom_table_styles().is_empty(), "not a table style");
    // Picked in the gallery: drawn with its elements.
    let mut l = layout(&wb, s);
    l.style.name = Some("Purple".into());
    wb.set_pivot_layout(s, 2, 0, &l).unwrap();
    let look = |wb: &Workbook, r: u32, c: u32| wb.table_cell_look(s, r, c).unwrap();
    assert_eq!(look(&wb, 2, 0).fill, Some(0x7030A0), "header row");
    assert_eq!(
        look(&wb, 3, 0).fill,
        Some(0xE4DFEC),
        "East: row subheading 1"
    );
    assert_eq!(look(&wb, 4, 0).fill, None, "Apples");
    let last = wb.pivot_at(s, 2, 0).unwrap().area.bottom;
    assert!(look(&wb, last, 0).italic, "grand total row");
    let preview = wb.pivot_style_preview("Purple", &l.style).unwrap();
    assert_eq!(preview[0][0].fill, Some(0x7030A0));
    // Saved as Excel saves a PivotTable style.
    let (path, p) = save(&mut wb, dir.path(), "pivot-style.xlsx");
    let styles = &p["xl/styles.xml"];
    assert!(
        styles.contains("<tableStyle name=\"Purple\" table=\"0\" count=\"3\">"),
        "{styles}"
    );
    assert!(styles.contains("type=\"firstRowSubheading\""), "{styles}");
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.custom_pivot_styles(), vec!["Purple".to_string()]);
    assert_eq!(look(&back, 3, 0).fill, Some(0xE4DFEC));
    assert_eq!(
        back.pivot_style_spec("Purple")
            .unwrap()
            .format(sheets_engine::TableElement::FirstRowSubheading)
            .fill,
        Some(0xE4DFEC)
    );
    // Renamed: the pivot follows. Deleted: the pivot gets the default.
    let mut spec = purple_pivot_style("Violet");
    spec.set_format(
        sheets_engine::TableElement::FirstRowSubheading,
        Default::default(),
    );
    wb.modify_pivot_style("Purple", &spec, true).unwrap();
    assert_eq!(layout(&wb, s).style.name.as_deref(), Some("Violet"));
    assert_eq!(look(&wb, 3, 0).fill, None);
    assert_eq!(wb.default_pivot_style(), "Violet");
    wb.delete_pivot_style("Violet").unwrap();
    assert_eq!(
        layout(&wb, s).style.name.as_deref(),
        Some("PivotStyleLight16")
    );
    assert!(wb.undo().unwrap());
    assert_eq!(layout(&wb, s).style.name.as_deref(), Some("Violet"));
    // A new pivot table gets the document's default.
    let s2 = new_pivot(&mut wb);
    assert_eq!(layout(&wb, s2).style.name.as_deref(), Some("Violet"));
    assert!(wb.undo().unwrap());
    // Built-in styles can be the start of a new one.
    let dup = wb.pivot_style_spec("PivotStyleMedium9").unwrap();
    assert!(
        !dup.format(sheets_engine::TableElement::HeaderRow)
            .is_empty()
    );
    // save → edit → save, save → undo → save.
    let (_, p2) = save(&mut wb, dir.path(), "pivot-style2.xlsx");
    assert!(p2["xl/styles.xml"].contains("name=\"Violet\""));
    assert!(p2["xl/styles.xml"].contains("defaultPivotStyle=\"Violet\""));
    assert!(wb.undo().unwrap());
    let (_, p3) = save(&mut wb, dir.path(), "pivot-style3.xlsx");
    assert!(p3["xl/styles.xml"].contains("name=\"Purple\""));
    assert!(!p3["xl/styles.xml"].contains("Violet"));
    assert!(p3["xl/pivotTables/pivotTable1.xml"].contains("pivotTableStyleInfo name=\"Purple\""));
}
