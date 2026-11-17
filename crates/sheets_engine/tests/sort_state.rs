//! A sort under a filter is remembered as Excel does (`<sortState>`): saved
//! next to the sheet's or the table's `<autoFilter>`, read back, shown on the
//! filter button and sorted again by Reapply.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::path::{Path, PathBuf};

use pivot_check::parts;
use sheets_engine::{CellRange, FilterScope, SortKey, SortState, Workbook};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sort_state");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn column(wb: &Workbook, col: u32, rows: std::ops::RangeInclusive<u32>) -> Vec<String> {
    rows.map(|r| wb.display_value(0, r, col)).collect()
}

fn sheet_xml(path: &Path) -> String {
    parts(path)["xl/worksheets/sheet1.xml"].clone()
}

#[test]
fn a_sheet_filter_remembers_its_sort() {
    let mut wb = Workbook::new_empty().unwrap();
    for (r, (name, n)) in [("Name", ""), ("b", "2"), ("a", "3"), ("c", "1")]
        .into_iter()
        .enumerate()
    {
        wb.set_input(0, r as u32, 0, name).unwrap();
        wb.set_input(0, r as u32, 1, if r == 0 { "Qty" } else { n })
            .unwrap();
    }
    assert!(
        wb.toggle_filter(0, CellRange::from_corners((0, 0), (3, 1)))
            .unwrap()
    );
    let f = wb.filter(0, &FilterScope::Sheet).unwrap();
    assert_eq!(f.sort, None);
    let keys = [SortKey {
        col: 1,
        descending: true,
    }];
    wb.sort_range(0, f.area, &keys, true).unwrap();
    assert_eq!(column(&wb, 0, 1..=3), ["a", "b", "c"]);
    let f = wb.filter(0, &FilterScope::Sheet).unwrap();
    let sort = SortState {
        keys: keys.to_vec(),
        by_value: true,
        levels: keys.iter().map(|&k| k.into()).collect(),
    };
    assert_eq!(f.sort.as_ref(), Some(&sort));
    assert_eq!(f.sorted(1), Some(true));
    assert_eq!(f.sorted(0), None);

    let path = scratch("sheet.xlsx");
    wb.save_as(&path).unwrap();
    let xml = sheet_xml(&path);
    assert!(
        xml.contains(
            r#"<autoFilter ref="A1:B4"/><sortState ref="A2:B4"><sortCondition descending="1" ref="B2:B4"/></sortState>"#
        ),
        "{xml}"
    );

    // Read back; Reapply sorts again after an edit.
    let mut back = Workbook::open(&path).unwrap();
    assert_eq!(
        back.filter(0, &FilterScope::Sheet).unwrap().sort,
        Some(sort)
    );
    back.set_input(0, 3, 1, "9").unwrap();
    back.reapply_filter(0, &FilterScope::Sheet).unwrap();
    assert_eq!(column(&back, 0, 1..=3), ["c", "a", "b"]);
    // Saved again unchanged, the sheet stays the same.
    let again = scratch("sheet-again.xlsx");
    back.save_as(&again).unwrap();
    let twice = scratch("sheet-twice.xlsx");
    back.save_as(&twice).unwrap();
    assert_eq!(sheet_xml(&again), sheet_xml(&twice));

    // Undoing the sort forgets it.
    wb.undo().unwrap();
    assert_eq!(column(&wb, 0, 1..=3), ["b", "a", "c"]);
    assert_eq!(wb.filter(0, &FilterScope::Sheet).unwrap().sort, None);
    let undone = scratch("sheet-undone.xlsx");
    wb.save_as(&undone).unwrap();
    assert!(!sheet_xml(&undone).contains("sortState"));
}

#[test]
fn a_table_remembers_its_sort() {
    let src = corpus("13_tables.xlsx");
    let mut wb = Workbook::open(&src).unwrap();
    let plain = scratch("plain13.xlsx");
    wb.save_as(&plain).unwrap();
    let mut wb = Workbook::open(&src).unwrap();
    let units = wb
        .tables(0)
        .into_iter()
        .find(|t| t.name == "Sales")
        .unwrap()
        .area
        .left
        + 1;
    wb.sort_table(0, "Sales", units, true).unwrap();
    let scope = FilterScope::Table("Sales".into());
    assert_eq!(wb.filter(0, &scope).unwrap().sorted(units), Some(true));
    let path = scratch("table.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let p = parts(&path);
    let table = &p["xl/tables/table1.xml"];
    assert!(
        table.contains(
            r#"<autoFilter ref="A1:D7"/><sortState ref="A2:D7"><sortCondition descending="1" ref="B2:B7"/></sortState><tableColumns"#
        ),
        "{table}"
    );
    // Read back, and saved again unchanged the part stays as it is.
    let mut back = Workbook::open(&path).unwrap();
    assert_eq!(back.filter(0, &scope).unwrap().sorted(units), Some(true));
    let again = scratch("table-again.xlsx");
    back.save_as(&again).unwrap();
    assert_eq!(&parts(&again)["xl/tables/table1.xml"], table);

    // Undone, the save is the plain one.
    wb.undo().unwrap();
    assert_eq!(wb.filter(0, &scope).unwrap().sort, None);
    let undone = scratch("table-undone.xlsx");
    wb.save_as(&undone).unwrap();
    let (a, b) = (parts(&plain), parts(&undone));
    for (name, text) in &a {
        if name == "docProps/core.xml" {
            continue;
        }
        assert_eq!(Some(text), b.get(name), "{name} changed");
    }
}
