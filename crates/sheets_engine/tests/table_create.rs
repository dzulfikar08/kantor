//! Format as Table and Table Design in Kantor: making a table, its options,
//! total row functions, rename and Convert to Range, each one undo step,
//! and the table parts a save writes (read back the same; tables and sheets
//! nobody touched stay byte for byte).

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::path::{Path, PathBuf};

use pivot_check::{MAIN, el, els, parts, targets};
use roxmltree::Document;
use sheets_engine::{CellRange, TotalFunc, Workbook};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("table_create");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

/// Region / Units / Price on A1:C7 of a new workbook.
fn data_book() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let rows = [
        ("Region", "Units", "Price"),
        ("East", "10", "2.5"),
        ("West", "4", "3"),
        ("North", "7", "1.5"),
        ("East", "3", "4"),
        ("West", "12", "2"),
        ("North", "5", "6"),
    ];
    let mut cells = Vec::new();
    for (r, (a, b, c)) in rows.iter().enumerate() {
        cells.push((r as u32, 0, a.to_string()));
        cells.push((r as u32, 1, b.to_string()));
        cells.push((r as u32, 2, c.to_string()));
    }
    wb.set_inputs(0, &cells).unwrap();
    wb
}

/// The table parts of a saved package, by table name, with the checks Excel
/// makes first: the sheet's `<tableParts>` link every part, ids are unique,
/// the content type is declared, column names match the header cells.
fn checked_tables(path: &Path) -> Vec<(String, String)> {
    let p = parts(path);
    assert!(
        sheets_engine::package_problems(&std::fs::read(path).unwrap()).is_empty(),
        "{:?}",
        sheets_engine::package_problems(&std::fs::read(path).unwrap())
    );
    let types = &p["[Content_Types].xml"];
    let mut out = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for (name, xml) in &p {
        if !(name.starts_with("xl/worksheets/sheet") && name.ends_with(".xml")) {
            continue;
        }
        let doc = Document::parse(xml).unwrap();
        let Some(tp) = el(doc.root_element(), "tableParts") else {
            continue;
        };
        let links = targets(&p, name);
        let list: Vec<_> = els(tp, "tablePart").collect();
        assert_eq!(tp.attribute("count"), Some(list.len().to_string().as_str()));
        for t in list {
            let rid = t
                .attribute((pivot_check::REL, "id"))
                .expect("tablePart r:id");
            let (kind, part) = &links[rid];
            assert_eq!(kind, "table");
            assert!(
                types.contains(&format!("PartName=\"/{part}\"")),
                "no content type for {part}"
            );
            let txml = &p[part];
            let tdoc = Document::parse(txml).unwrap();
            let root = tdoc.root_element();
            assert_eq!(root.tag_name().namespace(), Some(MAIN));
            let id = root.attribute("id").unwrap().to_owned();
            assert!(ids.insert(id.clone()), "table id {id} used twice");
            let cols = el(root, "tableColumns").unwrap();
            assert_eq!(
                cols.attribute("count").unwrap(),
                els(cols, "tableColumn").count().to_string()
            );
            out.push((root.attribute("name").unwrap().to_owned(), txml.clone()));
        }
    }
    out.sort();
    out
}

#[test]
fn format_as_table_saves_and_reads_back() {
    let mut wb = data_book();
    let range = wb.table_suggestion(0, CellRange::from_corners((2, 1), (2, 1)), (2, 1));
    assert_eq!(range, CellRange::from_corners((0, 0), (6, 2)));
    let name = wb
        .create_table(0, range, true, "TableStyleMedium2")
        .unwrap();
    assert_eq!(name, "Table1");
    let t = wb.table_at(0, 3, 1).unwrap();
    assert_eq!(t.columns, vec!["Region", "Units", "Price"]);
    assert!(t.filter_buttons && t.row_stripes);
    assert_eq!(t.style.as_deref(), Some("TableStyleMedium2"));
    assert!(wb.undo().unwrap());
    assert!(wb.table_at(0, 3, 1).is_none());
    assert!(wb.redo().unwrap());
    assert_eq!(wb.table_at(0, 3, 1).as_ref(), Some(&t));

    let path = scratch("new.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let tables = checked_tables(&path);
    assert_eq!(tables.len(), 1);
    let doc = Document::parse(&tables[0].1).unwrap();
    let root = doc.root_element();
    assert_eq!(root.attribute("ref"), Some("A1:C7"));
    assert_eq!(root.attribute("totalsRowShown"), Some("0"));
    assert_eq!(
        el(root, "autoFilter").unwrap().attribute("ref"),
        Some("A1:C7")
    );
    let style = el(root, "tableStyleInfo").unwrap();
    assert_eq!(style.attribute("name"), Some("TableStyleMedium2"));
    assert_eq!(style.attribute("showRowStripes"), Some("1"));

    let mut back = Workbook::open(&path).unwrap();
    assert_eq!(back.table_at(0, 3, 1), Some(t));
    // A second save carries the table it now has from the file.
    let again = scratch("again.xlsx");
    back.save_as(&again).unwrap();
    assert_eq!(checked_tables(&again).len(), 1);
}

#[test]
fn total_row_and_functions() {
    let mut wb = data_book();
    let range = CellRange::from_corners((0, 0), (6, 2));
    wb.create_table(0, range, true, "TableStyleLight9").unwrap();
    let mut o = wb.table_options("Table1").unwrap();
    o.total_row = true;
    o.banded_columns = true;
    o.first_column = true;
    wb.set_table_options("Table1", o.clone()).unwrap();
    assert_eq!(wb.table_options("Table1"), Some(o));
    let t = wb.table_at(0, 0, 0).unwrap();
    assert_eq!(t.totals_row(), Some(7));
    assert_eq!(wb.input(0, 7, 0), "Total");
    assert_eq!(wb.input(0, 7, 2), "=SUBTOTAL(109,Table1[Price])");
    assert_eq!(wb.display_value(0, 7, 2), "19");
    wb.set_table_total("Table1", 1, TotalFunc::Average).unwrap();
    assert_eq!(wb.input(0, 7, 1), "=SUBTOTAL(101,Table1[Units])");
    assert_eq!(wb.display_value(0, 7, 1), "6.833333333");
    assert_eq!(
        wb.table_totals("Table1"),
        vec![TotalFunc::None, TotalFunc::Average, TotalFunc::Sum]
    );
    // One undo step each.
    assert!(wb.undo().unwrap());
    assert_eq!(wb.input(0, 7, 1), "");
    assert!(wb.redo().unwrap());

    let path = scratch("totals.xlsx");
    wb.save_as(&path).unwrap();
    let tables = checked_tables(&path);
    let doc = Document::parse(&tables[0].1).unwrap();
    let root = doc.root_element();
    assert_eq!(root.attribute("ref"), Some("A1:C8"));
    assert_eq!(root.attribute("totalsRowCount"), Some("1"));
    assert_eq!(
        el(root, "autoFilter").unwrap().attribute("ref"),
        Some("A1:C7")
    );
    let cols: Vec<_> = els(el(root, "tableColumns").unwrap(), "tableColumn").collect();
    assert_eq!(cols[0].attribute("totalsRowLabel"), Some("Total"));
    assert_eq!(cols[1].attribute("totalsRowFunction"), Some("average"));
    assert_eq!(cols[2].attribute("totalsRowFunction"), Some("sum"));
    let sheet = &parts(&path)["xl/worksheets/sheet1.xml"];
    assert!(
        sheet.contains("<f>SUBTOTAL(101,Table1[Units])</f>"),
        "{sheet}"
    );
    let back = Workbook::open(&path).unwrap();
    assert_eq!(
        back.table_totals("Table1"),
        vec![TotalFunc::None, TotalFunc::Average, TotalFunc::Sum]
    );
    assert_eq!(back.display_value(0, 7, 1), "6.833333333");
    assert_eq!(back.table_options("Table1"), wb.table_options("Table1"));

    // Turning the total row off empties it.
    let mut o = wb.table_options("Table1").unwrap();
    o.total_row = false;
    wb.set_table_options("Table1", o).unwrap();
    assert_eq!(wb.input(0, 7, 2), "");
    assert_eq!(wb.table_at(0, 0, 0).unwrap().area.bottom, 6);
}

#[test]
fn total_row_inserts_a_row_when_the_one_below_is_used() {
    let mut wb = data_book();
    wb.set_input(0, 7, 0, "Note below").unwrap();
    wb.create_table(
        0,
        CellRange::from_corners((0, 0), (6, 2)),
        true,
        "TableStyleMedium2",
    )
    .unwrap();
    let mut o = wb.table_options("Table1").unwrap();
    o.total_row = true;
    wb.set_table_options("Table1", o).unwrap();
    assert_eq!(wb.input(0, 8, 0), "Note below");
    assert_eq!(wb.input(0, 7, 0), "Total");
    assert!(wb.undo().unwrap());
    assert_eq!(wb.input(0, 7, 0), "Note below");
}

#[test]
fn rename_follows_formulas_and_convert_to_range() {
    let mut wb = data_book();
    wb.create_table(
        0,
        CellRange::from_corners((0, 0), (6, 2)),
        true,
        "TableStyleMedium2",
    )
    .unwrap();
    wb.set_input(0, 0, 5, "=SUM(Table1[Units])").unwrap();
    assert_eq!(wb.display_value(0, 0, 5), "41");
    assert!(wb.rename_table("Table1", "two words").is_err());
    assert!(wb.rename_table("Table1", "A1").is_err());
    wb.rename_table("Table1", "Sales").unwrap();
    assert_eq!(wb.input(0, 0, 5), "=SUM(Sales[Units])");
    assert_eq!(wb.display_value(0, 0, 5), "41");
    let path = scratch("renamed.xlsx");
    wb.save_as(&path).unwrap();
    let tables = checked_tables(&path);
    assert_eq!(
        tables.iter().map(|t| t.0.as_str()).collect::<Vec<_>>(),
        vec!["Sales"]
    );
    assert!(wb.undo().unwrap());
    assert_eq!(wb.input(0, 0, 5), "=SUM(Table1[Units])");
    assert!(wb.redo().unwrap());

    wb.convert_table_to_range("Sales").unwrap();
    assert!(wb.tables(0).is_empty());
    assert_eq!(wb.input(0, 0, 5), "=SUM($B$2:$B$7)");
    assert_eq!(wb.display_value(0, 0, 5), "41");
    let path = scratch("converted.xlsx");
    wb.save_as(&path).unwrap();
    assert!(checked_tables(&path).is_empty());
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(Workbook::open(&path).unwrap().tables(0).is_empty());
}

#[test]
fn without_headers_a_header_row_is_added() {
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_inputs(
        0,
        &[
            (2, 1, "1".into()),
            (2, 2, "2".into()),
            (3, 1, "3".into()),
            (3, 2, "4".into()),
        ],
    )
    .unwrap();
    let range = CellRange::from_corners((2, 1), (3, 2));
    wb.create_table(0, range, false, "TableStyleLight1")
        .unwrap();
    let t = wb.table_at(0, 2, 1).unwrap();
    assert_eq!(t.area, CellRange::from_corners((1, 1), (3, 2)));
    assert_eq!(wb.input(0, 1, 1), "Column1");
    assert_eq!(wb.input(0, 1, 2), "Column2");
    // At the top of the sheet a row is inserted.
    let mut wb = Workbook::new_empty().unwrap();
    wb.set_inputs(0, &[(0, 0, "5".into()), (1, 0, "6".into())])
        .unwrap();
    wb.create_table(
        0,
        CellRange::from_corners((0, 0), (1, 0)),
        false,
        "TableStyleLight1",
    )
    .unwrap();
    assert_eq!(wb.input(0, 0, 0), "Column1");
    assert_eq!(wb.input(0, 1, 0), "5");
    assert!(wb.undo().unwrap());
    assert_eq!(wb.input(0, 0, 0), "5");
}

#[test]
fn overlaps_are_refused() {
    let mut wb = data_book();
    wb.create_table(
        0,
        CellRange::from_corners((0, 0), (6, 2)),
        true,
        "TableStyleMedium2",
    )
    .unwrap();
    assert!(
        wb.create_table(
            0,
            CellRange::from_corners((5, 2), (9, 4)),
            true,
            "TableStyleMedium2"
        )
        .is_err()
    );
    assert!(wb.table_problem(0, CellRange::columns(0, 1)).is_some());
}

#[test]
fn a_file_table_restyled_keeps_its_part_and_markup() {
    let src = corpus("13_tables.xlsx");
    let mut plain = Workbook::open(&src).unwrap();
    let plain_path = scratch("plain13.xlsx");
    plain.save_as(&plain_path).unwrap();
    let mut wb = Workbook::open(&src).unwrap();
    let mut o = wb.table_options("Sales").unwrap();
    assert_eq!(o.style.as_deref(), Some("TableStyleMedium9"));
    o.style = Some("TableStyleDark3".into());
    o.banded_rows = false;
    wb.set_table_options("Sales", o.clone()).unwrap();
    let path = scratch("restyled13.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let a = parts(&plain_path);
    let b = parts(&path);
    let tables = checked_tables(&path);
    let sales = &tables.iter().find(|t| t.0 == "Sales").unwrap().1;
    let doc = Document::parse(sales).unwrap();
    let root = doc.root_element();
    assert_eq!(root.attribute("id"), Some("1"));
    assert_eq!(
        el(root, "tableStyleInfo").unwrap().attribute("name"),
        Some("TableStyleDark3")
    );
    // The calculated column formula is kept.
    assert!(sales.contains("<calculatedColumnFormula>"), "{sales}");
    // Everything else as a plain save wrote it.
    for (name, text) in &a {
        if name == "xl/tables/table1.xml" {
            continue;
        }
        assert_eq!(Some(text), b.get(name), "{name} changed");
    }
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.table_options("Sales"), Some(o));
}
