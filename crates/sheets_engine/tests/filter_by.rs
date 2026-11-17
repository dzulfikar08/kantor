//! AutoFilter by top 10, average, dates, colour and icon, and sorts by
//! colour, icon and custom list, made, saved, read back and reapplied.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::path::{Path, PathBuf};

use pivot_check::parts;
use sheets_engine::{CellRange, FilterKind, FilterScope, SortLevel, SortOn, StyleChange, Workbook};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("filter_by");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

/// Name / Score / Level in A1:C9 with an AutoFilter.
fn book() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let rows = [
        ("Name", "Score", "Level"),
        ("a", "10", "High"),
        ("b", "50", "Low"),
        ("c", "30", "Medium"),
        ("d", "90", "High"),
        ("e", "70", "Low"),
        ("f", "20", "Medium"),
        ("g", "60", "High"),
        ("h", "40", "Low"),
    ];
    for (r, (a, b, c)) in rows.iter().enumerate() {
        wb.set_input(0, r as u32, 0, a).unwrap();
        wb.set_input(0, r as u32, 1, b).unwrap();
        wb.set_input(0, r as u32, 2, c).unwrap();
    }
    wb.toggle_filter(0, CellRange::from_corners((0, 0), (8, 2)))
        .unwrap();
    wb
}

fn shown(wb: &Workbook) -> Vec<String> {
    let hidden = wb.hidden_row_set(0);
    (1..=8)
        .filter(|r| !hidden.contains(r))
        .map(|r| wb.display_value(0, r, 0))
        .collect()
}

fn names(wb: &Workbook) -> Vec<String> {
    (1..=8).map(|r| wb.display_value(0, r, 0)).collect()
}

#[test]
fn top_ten_and_average() {
    let mut wb = book();
    let top = FilterKind::Top10 {
        top: true,
        percent: false,
        value: 3.0,
    };
    wb.set_column_filter(0, &FilterScope::Sheet, 1, Some(top.clone()))
        .unwrap();
    assert_eq!(shown(&wb), ["d", "e", "g"]);
    let bottom = FilterKind::Top10 {
        top: false,
        percent: true,
        value: 25.0,
    };
    wb.set_column_filter(0, &FilterScope::Sheet, 1, Some(bottom))
        .unwrap();
    assert_eq!(shown(&wb), ["a", "f"]);
    wb.set_column_filter(
        0,
        &FilterScope::Sheet,
        1,
        Some(FilterKind::Dynamic("aboveAverage".into())),
    )
    .unwrap();
    // The average is 46.25.
    assert_eq!(shown(&wb), ["b", "d", "e", "g"]);

    // Saved as Excel writes it and read back.
    wb.set_column_filter(0, &FilterScope::Sheet, 1, Some(top.clone()))
        .unwrap();
    let path = scratch("top.xlsx");
    wb.save_as(&path).unwrap();
    let sheet = &parts(&path)["xl/worksheets/sheet1.xml"];
    assert!(sheet.contains("<top10 val=\"3\"/>"), "{sheet}");
    let back = Workbook::open(&path).unwrap();
    let f = back.filter(0, &FilterScope::Sheet).unwrap();
    assert_eq!(f.column(1), Some(&top));
}

#[test]
fn dates_relative_to_today() {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_input(0, 0, 0, "When").unwrap();
    wb.set_input(0, 1, 0, "=TODAY()").unwrap();
    wb.set_input(0, 2, 0, "=TODAY()-400").unwrap();
    wb.set_input(0, 3, 0, "=TODAY()+400").unwrap();
    wb.toggle_filter(0, CellRange::from_corners((0, 0), (3, 0)))
        .unwrap();
    wb.set_column_filter(
        0,
        &FilterScope::Sheet,
        0,
        Some(FilterKind::Dynamic("thisYear".into())),
    )
    .unwrap();
    let hidden = wb.hidden_row_set(0);
    assert!(!hidden.contains(&1));
    assert!(hidden.contains(&2) && hidden.contains(&3));
}

#[test]
fn by_colour_filter_and_sort() {
    let mut wb = book();
    // Rows b and e are yellow.
    let yellow = 0xFFFF00;
    for r in [2, 5] {
        wb.set_range_style(
            0,
            CellRange::from_corners((r, 0), (r, 0)),
            StyleChange::Fill(Some(yellow)),
        )
        .unwrap();
    }
    let looks = wb.filter_looks(0, &FilterScope::Sheet, 0);
    assert!(looks.fills.contains(&Some(yellow)));
    assert!(looks.fills.contains(&None));

    let by_yellow = wb.color_filter(Some(yellow), false);
    wb.set_column_filter(0, &FilterScope::Sheet, 0, Some(by_yellow.clone()))
        .unwrap();
    assert_eq!(shown(&wb), ["b", "e"]);
    let path = scratch("colour.xlsx");
    wb.save_as(&path).unwrap();
    let back = Workbook::open(&path).unwrap();
    let f = back.filter(0, &FilterScope::Sheet).unwrap();
    let FilterKind::Color { font: false, dxf } = f.column(0).unwrap().clone() else {
        panic!("{:?}", f.column(0));
    };
    assert_eq!(back.dxf_color(dxf, false), Some(yellow));

    // Sort the yellow cells to the top, then Reapply after an edit.
    wb.clear_filter(0, &FilterScope::Sheet).unwrap();
    let FilterKind::Color { dxf, .. } = by_yellow else {
        unreachable!()
    };
    let levels = [SortLevel {
        col: 0,
        descending: false,
        on: SortOn::CellColor { dxf },
    }];
    wb.sort_range_by(0, CellRange::from_corners((0, 0), (8, 2)), &levels, true)
        .unwrap();
    assert_eq!(names(&wb)[..2], ["b", "e"]);
    let f = wb.filter(0, &FilterScope::Sheet).unwrap();
    assert_eq!(f.sort.as_ref().unwrap().levels, levels);
    // Another row turns yellow: Reapply sorts it up too.
    wb.set_range_style(
        0,
        CellRange::from_corners((8, 0), (8, 0)),
        StyleChange::Fill(Some(yellow)),
    )
    .unwrap();
    wb.reapply_filter(0, &FilterScope::Sheet).unwrap();
    let top3: Vec<String> = names(&wb)[..3].to_vec();
    assert!(top3.contains(&"h".to_string()), "{top3:?}");
}

#[test]
fn custom_list_sort_is_remembered() {
    let mut wb = book();
    let levels = [SortLevel {
        col: 2,
        descending: false,
        on: SortOn::List(vec!["Low".into(), "Medium".into(), "High".into()]),
    }];
    wb.sort_range_by(0, CellRange::from_corners((0, 0), (8, 2)), &levels, true)
        .unwrap();
    let levels_now: Vec<String> = (1..=8).map(|r| wb.display_value(0, r, 2)).collect();
    assert_eq!(
        levels_now,
        [
            "Low", "Low", "Low", "Medium", "Medium", "High", "High", "High"
        ]
    );
    let path = scratch("list.xlsx");
    wb.save_as(&path).unwrap();
    let sheet = &parts(&path)["xl/worksheets/sheet1.xml"];
    assert!(sheet.contains("customList=\"Low,Medium,High\""), "{sheet}");
    let back = Workbook::open(&path).unwrap();
    let f = back.filter(0, &FilterScope::Sheet).unwrap();
    assert_eq!(f.sort.unwrap().levels, levels);
}
