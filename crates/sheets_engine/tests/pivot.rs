//! Pivot tables: carried through saves with their caches, moved with
//! inserted and deleted rows and columns (file 21 of the corpus, written by
//! `scripts/corpus/gen_pivot_xlsx.py`).

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pivot_check::{REL, el, els, parts, pivot_problems, targets};
use roxmltree::Document;
use sheets_engine::Workbook;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/21_pivot_tables.xlsx")
}

fn save(wb: &mut Workbook, dir: &Path, name: &str) -> BTreeMap<String, String> {
    let path = dir.join(name);
    wb.save_as(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        sheets_engine::package_problems(&bytes),
        Vec::<String>::new()
    );
    Workbook::open(&path).unwrap();
    let p = parts(&path);
    assert_eq!(pivot_problems(&p), Vec::<String>::new());
    p
}

fn sheet_index(wb: &Workbook, name: &str) -> u32 {
    wb.sheet_names().iter().position(|n| n == name).unwrap() as u32
}

fn location(p: &BTreeMap<String, String>, part: &str) -> String {
    let doc = Document::parse(&p[part]).unwrap();
    el(doc.root_element(), "location")
        .unwrap()
        .attribute("ref")
        .unwrap()
        .to_owned()
}

fn source(p: &BTreeMap<String, String>, part: &str) -> (Option<String>, Option<String>) {
    let doc = Document::parse(&p[part]).unwrap();
    let s = doc
        .descendants()
        .find(|n| n.tag_name().name() == "worksheetSource")
        .unwrap();
    (
        s.attribute("ref").map(str::to_owned),
        s.attribute("name").map(str::to_owned),
    )
}

#[test]
fn the_fixture_is_consistent() {
    let p = parts(&fixture());
    assert_eq!(pivot_problems(&p), Vec::<String>::new());
}

#[test]
fn pivot_tables_survive_a_save() {
    let dir = tempfile::tempdir().unwrap();
    let orig = parts(&fixture());
    let mut wb = Workbook::open(fixture()).unwrap();
    let p = save(&mut wb, dir.path(), "a.xlsx");
    assert!(
        wb.save_notes()
            .iter()
            .all(|n| !n.to_lowercase().contains("pivot")),
        "{:?}",
        wb.save_notes()
    );
    for k in 1..=3 {
        let part = format!("xl/pivotTables/pivotTable{k}.xml");
        assert_eq!(p[&part], orig[&part], "{part} is carried as it was");
    }
    for k in 1..=2 {
        for kind in ["pivotCacheDefinition", "pivotCacheRecords"] {
            let part = format!("xl/pivotCache/{kind}{k}.xml");
            assert_eq!(p[&part], orig[&part]);
        }
    }
    let wbx = &p["xl/workbook.xml"];
    assert!(
        wbx.contains(r#"<pivotCache cacheId="1""#) && wbx.contains(r#"<pivotCache cacheId="2""#)
    );
    // The list comes after calcPr, as CT_Workbook orders it.
    assert!(wbx.find("<calcPr").unwrap() < wbx.find("<pivotCaches>").unwrap());
    assert_eq!(location(&p, "xl/pivotTables/pivotTable1.xml"), "A3:E8");
    // The pivot's cells are ordinary cells.
    let s = sheet_index(&wb, "Pivot");
    assert_eq!(wb.display_value(s, 4, 0), "East");
    assert_eq!(wb.display_value(s, 7, 4), "1068");

    // Saved again from the saved file: still there, once.
    let mut wb = Workbook::open(dir.path().join("a.xlsx")).unwrap();
    let p2 = save(&mut wb, dir.path(), "b.xlsx");
    assert_eq!(p2["xl/workbook.xml"].matches("<pivotCache ").count(), 3);
    assert_eq!(
        p2["xl/pivotTables/pivotTable3.xml"],
        orig["xl/pivotTables/pivotTable3.xml"]
    );
}

#[test]
fn pivots_follow_inserted_and_deleted_rows_and_columns() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let pivot = sheet_index(&wb, "Pivot");
    let data = sheet_index(&wb, "Data");
    wb.insert_rows(pivot, 0, 2).unwrap();
    wb.insert_columns(pivot, 0, 1).unwrap();
    wb.insert_rows(data, 5, 3).unwrap(); // inside the source
    let p = save(&mut wb, dir.path(), "a.xlsx");
    assert_eq!(location(&p, "xl/pivotTables/pivotTable1.xml"), "B5:F10");
    assert_eq!(location(&p, "xl/pivotTables/pivotTable2.xml"), "I5:K8");
    assert_eq!(
        location(&p, "xl/pivotTables/pivotTable3.xml"),
        "A1:C14",
        "other sheet"
    );
    assert_eq!(
        source(&p, "xl/pivotCache/pivotCacheDefinition1.xml")
            .0
            .as_deref(),
        Some("A1:E16")
    );
    // A source by name follows the table.
    assert_eq!(
        source(&p, "xl/pivotCache/pivotCacheDefinition2.xml")
            .1
            .as_deref(),
        Some("Sales")
    );
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());

    wb.delete_rows(data, 5, 7).unwrap();
    wb.delete_columns(pivot, 0, 0).unwrap();
    let p = save(&mut wb, dir.path(), "b.xlsx");
    assert_eq!(location(&p, "xl/pivotTables/pivotTable1.xml"), "A5:E10");
    assert_eq!(
        source(&p, "xl/pivotCache/pivotCacheDefinition1.xml")
            .0
            .as_deref(),
        Some("A1:E13")
    );
}

/// The cells of `range` (`top, left, bottom, right`, 0-based) as shown.
fn grid(wb: &Workbook, sheet: u32, (t, l, b, r): (u32, u32, u32, u32)) -> Vec<Vec<String>> {
    (t..=b)
        .map(|row| (l..=r).map(|c| wb.display_value(sheet, row, c)).collect())
        .collect()
}

#[test]
fn pivots_are_found_and_styled() {
    let wb = Workbook::open(fixture()).unwrap();
    let s = sheet_index(&wb, "Pivot");
    let p = wb.pivot_at(s, 4, 1).unwrap();
    assert_eq!(p.name, "PivotTable1");
    assert_eq!(p.area.name(), "A3:E8");
    assert_eq!(p.refresh_problem, None);
    // PivotTable2's report filter is part of it.
    assert_eq!(wb.pivot_at(s, 0, 8).unwrap().name, "PivotTable2");
    assert_eq!(wb.pivot_at(s, 0, 8).unwrap().bounds.name(), "H1:J6");
    assert!(wb.pivot_at(s, 10, 0).is_none());
    assert_eq!(wb.pivot_tables(sheet_index(&wb, "Tabular")).len(), 2);

    // PivotStyleLight16: tinted, bold header and grand total rows.
    let header = wb.table_cell_look(s, 3, 0).unwrap();
    assert!(header.bold && header.fill.is_some());
    let body = wb.table_cell_look(s, 4, 1).unwrap();
    assert!(!body.bold && body.fill.is_none());
    let grand = wb.table_cell_look(s, 7, 2).unwrap();
    assert!(grand.bold && grand.fill == header.fill && grand.top.is_some());
    // Grand total column: bold.
    assert!(wb.table_cell_look(s, 5, 4).unwrap().bold);
    // Tabular (Medium 9): subtotal rows are bold and tinted.
    let t = sheet_index(&wb, "Tabular");
    let sub = wb.table_cell_look(t, 4, 2).unwrap(); // "East Total"
    assert!(sub.bold && sub.fill.is_some());
    assert!(!wb.table_cell_look(t, 1, 2).unwrap().bold);
}

#[test]
fn refreshing_unchanged_data_gives_the_same_cells() {
    // The generator laid the pivots out the way Excel does; refreshing from
    // the same data must give the same cells, definitions and caches.
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let s = sheet_index(&wb, "Pivot");
    let t = sheet_index(&wb, "Tabular");
    let before_p = grid(&wb, s, (0, 0, 9, 10));
    let before_t = grid(&wb, t, (0, 0, 15, 8));
    assert_eq!(
        wb.refresh_pivot(s, 4, 1).unwrap(),
        2,
        "both pivots of the cache"
    );
    assert_eq!(grid(&wb, s, (0, 0, 9, 10)), before_p);
    assert_eq!(wb.refresh_all_pivots().unwrap(), 4);
    assert_eq!(grid(&wb, t, (0, 0, 15, 8)), before_t);
    assert_eq!(grid(&wb, s, (0, 0, 9, 10)), before_p);

    let orig = parts(&fixture());
    let p = save(&mut wb, dir.path(), "a.xlsx");
    let items = |xml: &str, list: &str| {
        let doc = Document::parse(xml).unwrap();
        el(doc.root_element(), list).map(|n| xml[n.range()].to_owned())
    };
    for k in 1..=4 {
        let part = format!("xl/pivotTables/pivotTable{k}.xml");
        for list in [
            "location",
            "pivotFields",
            "rowItems",
            "colItems",
            "dataFields",
            "pageFields",
        ] {
            let (a, b) = (items(&orig[&part], list), items(&p[&part], list));
            assert_eq!(norm(a), norm(b), "{part} {list}");
        }
    }
    for k in 1..=3 {
        let part = format!("xl/pivotCache/pivotCacheRecords{k}.xml");
        let doc = Document::parse(&p[&part]).unwrap();
        assert_eq!(els(doc.root_element(), "r").count(), 12);
    }
}

/// Element text with the attribute order and self-closing forms Excel and
/// Kantor may write differently made alike.
fn norm(x: Option<String>) -> Option<String> {
    let x = x?;
    let doc = Document::parse(&x).ok()?;
    let mut out = String::new();
    for n in doc.descendants().filter(|n| n.is_element()) {
        let mut attrs: Vec<String> = n
            .attributes()
            .map(|a| format!("{}={}", a.name(), a.value()))
            .collect();
        attrs.sort();
        out.push_str(&format!("<{} {}>", n.tag_name().name(), attrs.join(" ")));
    }
    Some(out)
}

#[test]
fn refresh_picks_up_edited_and_new_data() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let d = sheet_index(&wb, "Data");
    let s = sheet_index(&wb, "Pivot");
    let t = sheet_index(&wb, "Tabular");
    // East / Apples revenue 120 -> 220.
    wb.set_input(d, 1, 4, "220").unwrap();
    assert_eq!(wb.display_value(s, 4, 2), "228", "not refreshed yet");
    wb.refresh_pivot(s, 3, 0).unwrap();
    // Columns: Plums, Apples, Pears (the definition's order).
    assert_eq!(
        grid(&wb, s, (4, 0, 4, 4)),
        [["East", "144", "328", "45", "517"]]
    );
    assert_eq!(wb.display_value(s, 7, 4), "1168");
    // PivotTable2: West stays hidden; East's average revenue is now 129.25.
    assert_eq!(
        grid(&wb, s, (3, 7, 5, 9)),
        [
            ["East", "30", "129.25"],
            ["North", "29", "99"],
            ["Grand Total", "59", "114.125"],
        ]
    );

    // A new region typed under the table: the table grows, and so does the
    // tabular pivot over it.
    for (c, v) in ["South", "Figs", "2026-05-01", "4", "48"]
        .iter()
        .enumerate()
    {
        wb.set_input(d, 13, c as u32, v).unwrap();
    }
    assert_eq!(wb.refresh_pivot(t, 0, 0).unwrap(), 1);
    let tab = grid(&wb, t, (0, 0, 19, 2));
    assert_eq!(tab[0], ["Region", "Product", "Sum of Units"]);
    // New items come after the known ones in a field without a sort order,
    // as in Excel.
    assert_eq!(tab[9], ["West", "Apples", "12"]);
    assert_eq!(tab[12], ["West Total", "", "19"]);
    assert_eq!(tab[13], ["South", "Figs", "4"]);
    assert_eq!(tab[14], ["South Total", "", "4"]);
    assert_eq!(tab[15], ["Grand Total", "", "82"]);
    assert_eq!(tab[16], ["", "", ""]);
    let p = save(&mut wb, dir.path(), "a.xlsx");
    assert_eq!(location(&p, "xl/pivotTables/pivotTable3.xml"), "A1:C16");
    let cache = Document::parse(&p["xl/pivotCache/pivotCacheDefinition2.xml"]).unwrap();
    assert_eq!(cache.root_element().attribute("recordCount"), Some("13"));
    assert!(p["xl/pivotCache/pivotCacheDefinition2.xml"].contains("<s v=\"South\"/>"));
    // The range source didn't grow (row 14 is outside A1:E13).
    assert_eq!(wb.display_value(s, 7, 4), "1168");

    // Undo: the tabular pivot shrinks back, and so does what a save writes.
    wb.undo().unwrap();
    assert_eq!(wb.display_value(t, 13, 0), "Grand Total");
    let p = save(&mut wb, dir.path(), "b.xlsx");
    assert_eq!(location(&p, "xl/pivotTables/pivotTable3.xml"), "A1:C14");
    let mut wb = Workbook::open(dir.path().join("a.xlsx")).unwrap();
    // The refreshed file refreshes to itself (PivotTable4 beside it wasn't
    // refreshed before that save: it picks up the new revenue now).
    let before = grid(&wb, t, (0, 0, 19, 2));
    assert_eq!(wb.display_value(t, 1, 7), "144");
    wb.refresh_all_pivots().unwrap();
    assert_eq!(grid(&wb, t, (0, 0, 19, 2)), before);
    assert_eq!(wb.display_value(t, 1, 7), "220");
}

#[test]
fn rows_and_columns_through_a_pivot_are_refused() {
    let mut wb = Workbook::open(fixture()).unwrap();
    let s = sheet_index(&wb, "Pivot");
    assert!(wb.insert_rows(s, 5, 1).is_err(), "inside PivotTable1");
    assert!(wb.delete_rows(s, 1, 1).is_err(), "PivotTable2's filter row");
    assert!(wb.delete_columns(s, 4, 4).is_err());
    assert!(wb.insert_columns(s, 6, 1).is_ok(), "between the two");
    assert!(
        wb.insert_rows(s, 2, 1).is_err(),
        "PivotTable2's filter is above"
    );
    assert!(wb.insert_rows(s, 0, 1).is_ok(), "above both");
    assert_eq!(wb.pivot_at(s, 4, 0).unwrap().area.name(), "A4:E9");
    // A refresh after the moves writes where the pivot is now.
    wb.refresh_all_pivots().unwrap();
    assert_eq!(wb.display_value(s, 8, 4), "1068");
}

#[test]
fn a_new_value_overlapping_other_data_is_refused() {
    let mut wb = Workbook::open(fixture()).unwrap();
    let d = sheet_index(&wb, "Data");
    let s = sheet_index(&wb, "Pivot");
    // A fourth product makes PivotTable1 a column wider, into H... no: into F,
    // which is empty; but a note typed in F5 is in the way.
    wb.set_input(d, 1, 1, "Kiwis").unwrap();
    wb.set_input(s, 4, 5, "note").unwrap();
    let e = wb.refresh_pivot(s, 3, 0).unwrap_err().to_string();
    assert!(e.contains("already data in F5"), "{e}");
    assert_eq!(wb.display_value(s, 4, 2), "228", "nothing changed");
}

#[test]
fn getpivotdata_reads_the_pivots() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(fixture()).unwrap();
    let d = sheet_index(&wb, "Data");
    let s = sheet_index(&wb, "Pivot");
    let t = sheet_index(&wb, "Tabular");
    let formulas = [
        (
            r#"=GETPIVOTDATA("Sum of Revenue",Pivot!$A$3,"Region","East","Product","Apples")"#,
            "228",
        ),
        (
            r#"=GETPIVOTDATA("Revenue",Pivot!$C$6,"product","PEARS","Region","West")"#,
            "75",
        ),
        (
            r#"=GETPIVOTDATA("Sum of Revenue",Pivot!$A$3,"Region","North")"#,
            "396",
        ),
        (r#"=GETPIVOTDATA("Sum of Revenue",Pivot!$A$3)"#, "1068"),
        (
            r#"=GETPIVOTDATA("Average of Revenue",Pivot!$H$3,"Region","East")"#,
            "104.25",
        ),
        (r#"=GETPIVOTDATA("Sum of Units",Pivot!$H$1)"#, "59"),
        (
            r#"=GETPIVOTDATA("Sum of Units",Pivot!$H$3,"Region","West")"#,
            "#REF!",
        ),
        (
            r#"=GETPIVOTDATA("Sum of Units",Tabular!$A$1,"Region","North")"#,
            "29",
        ),
        (
            r#"=GETPIVOTDATA("Sum of Units",Tabular!$A$1,"Region","North","Product","Plums")"#,
            "5",
        ),
        (
            r#"=GETPIVOTDATA("Max of Revenue",Tabular!$F$1,"Product","Apples")"#,
            "144",
        ),
        (r#"=GETPIVOTDATA("Sum of Revenue",Data!$A$1)"#, "#REF!"),
    ];
    for (i, (f, _)) in formulas.iter().enumerate() {
        wb.set_input(t, 20 + i as u32, 0, f).unwrap();
    }
    let values = |wb: &Workbook| -> Vec<String> {
        (0..formulas.len())
            .map(|i| wb.display_value(t, 20 + i as u32, 0))
            .collect()
    };
    let expected: Vec<String> = formulas.iter().map(|(_, v)| v.to_string()).collect();
    assert_eq!(values(&wb), expected);

    // A refresh with new data: the values follow.
    wb.set_input(d, 1, 4, "220").unwrap();
    wb.refresh_all_pivots().unwrap();
    assert_eq!(wb.display_value(t, 20, 0), "328");
    assert_eq!(wb.display_value(t, 23, 0), "1168");
    // The pivots move down: still found.
    wb.insert_rows(s, 0, 2).unwrap();
    assert_eq!(wb.display_value(t, 20, 0), "328");
    // Undone: the rows, then the refresh.
    wb.undo().unwrap();
    assert_eq!(
        wb.display_value(t, 20, 0),
        "328",
        "the refresh is still done"
    );
    wb.undo().unwrap();
    assert_eq!(wb.display_value(t, 20, 0), "228");
    wb.undo().unwrap(); // the edited revenue

    // Saved as GETPIVOTDATA (no _xlfn. prefix) and read back.
    let p = save(&mut wb, dir.path(), "a.xlsx");
    let x = &p["xl/worksheets/sheet3.xml"];
    assert!(x.contains("GETPIVOTDATA(&quot;Sum of Revenue&quot;,Pivot!$A$3"));
    assert!(!x.contains("_xlfn.GETPIVOTDATA"));
    let wb = Workbook::open(dir.path().join("a.xlsx")).unwrap();
    assert_eq!(values(&wb), expected);
}

#[test]
fn slicers_survive_a_save() {
    let dir = tempfile::tempdir().unwrap();
    let orig = parts(&fixture());
    let check = |p: &BTreeMap<String, String>| {
        for part in [
            "xl/slicerCaches/slicerCache1.xml",
            "xl/slicers/slicer1.xml",
            "xl/drawings/drawing1.xml",
        ] {
            assert_eq!(p.get(part), orig.get(part), "{part}");
            assert!(
                p["[Content_Types].xml"].contains(&format!("PartName=\"/{part}\"")),
                "{part}"
            );
        }
        // The workbook lists the slicer cache, through a relationship that
        // leads to it.
        let wb = Document::parse(&p["xl/workbook.xml"]).unwrap();
        let cache = wb
            .descendants()
            .find(|n| n.tag_name().name() == "slicerCache")
            .expect("workbook slicerCaches");
        let rid = cache.attribute((REL, "id")).unwrap();
        assert_eq!(
            targets(p, "xl/workbook.xml")[rid],
            (
                "slicerCache".to_owned(),
                "xl/slicerCaches/slicerCache1.xml".to_owned()
            )
        );
        assert!(
            p["xl/workbook.xml"]
                .contains(r#"<definedName name="Slicer_Region">#N/A</definedName>"#)
        );
        // The sheet lists the slicer and keeps its drawing.
        let sheet = Document::parse(&p["xl/worksheets/sheet2.xml"]).unwrap();
        let slicer = sheet
            .descendants()
            .find(|n| n.tag_name().name() == "slicer")
            .expect("sheet slicerList");
        let rels = targets(p, "xl/worksheets/sheet2.xml");
        assert_eq!(
            rels[slicer.attribute((REL, "id")).unwrap()].1,
            "xl/slicers/slicer1.xml"
        );
        assert!(
            rels.values()
                .any(|(k, t)| k == "drawing" && t == "xl/drawings/drawing1.xml")
        );
        // The cache keeps the id the slicer cache points at.
        assert!(p["xl/pivotCache/pivotCacheDefinition1.xml"].contains(r#"pivotCacheId="1""#));
    };
    let mut wb = Workbook::open(fixture()).unwrap();
    let p = save(&mut wb, dir.path(), "a.xlsx");
    assert_eq!(wb.save_notes(), &[] as &[String]);
    check(&p);
    let mut wb = Workbook::open(dir.path().join("a.xlsx")).unwrap();
    let s = sheet_index(&wb, "Pivot");
    wb.refresh_pivot(s, 3, 0).unwrap();
    let p = save(&mut wb, dir.path(), "b.xlsx");
    assert_eq!(wb.save_notes(), &[] as &[String]);
    check(&p);
}
