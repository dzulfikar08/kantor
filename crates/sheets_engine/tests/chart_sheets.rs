//! Chart sheets (tests/corpus/xlsx/22_chart_sheets.xlsx): shown as tabs in
//! the order saving keeps, their charts read, and their parts saved.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::drawing::{DrawingKey, DrawingKind};
use sheets_engine::{Tab, Workbook};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn parts(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut b = Vec::new();
        e.read_to_end(&mut b).unwrap();
        out.insert(e.name().unwrap().into_owned(), b);
    }
    out
}

/// Tab names in order.
fn tab_names(wb: &Workbook) -> Vec<String> {
    let names = wb.sheet_names();
    wb.tabs()
        .into_iter()
        .map(|t| match t {
            Tab::Sheet(i) => names[i as usize].clone(),
            Tab::Chart(k) => format!("[{}]", wb.chart_sheet_name(k).unwrap()),
        })
        .collect()
}

/// `<sheet name=…>` of workbook.xml in order, with `state` when hidden.
fn listed(p: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let wb = String::from_utf8(p["xl/workbook.xml"].clone()).unwrap();
    let doc = roxmltree::Document::parse(&wb).unwrap();
    doc.descendants()
        .filter(|n| n.has_tag_name("sheet"))
        .map(|n| {
            let name = n.attribute("name").unwrap().to_owned();
            match n.attribute("state") {
                Some(s) => format!("{name} ({s})"),
                None => name,
            }
        })
        .collect()
}

const TABS: [&str; 4] = ["[Revenue chart]", "Data", "[Share]", "Notes"];

#[test]
fn chart_sheets_are_tabs_with_their_chart() {
    let wb = Workbook::open(corpus("22_chart_sheets.xlsx")).unwrap();
    assert_eq!(wb.sheet_names(), ["Data", "Notes"]);
    // The hidden chart sheet has no tab.
    assert_eq!(tab_names(&wb), TABS);
    let Tab::Chart(k) = wb.tabs()[0] else {
        panic!("first tab is a chart sheet")
    };
    let objects = wb.chart_sheet_drawings(k);
    assert_eq!(objects.len(), 1);
    let DrawingKind::Chart { xml, .. } = &*objects[0].kind else {
        panic!("a chart")
    };
    assert!(xml.contains("Revenue by region"));
    // Its chart takes the Chart panel's edits.
    assert!(objects[0].editable);
    // Its series read the Data sheet live.
    let r = wb.chart_reference("Data!$B$2:$B$5").unwrap();
    assert_eq!(r.shown, ["120", "95", "143", "88"]);
}

#[test]
fn saving_keeps_chart_sheets_where_the_tabs_show_them() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("22_chart_sheets.xlsx")).unwrap();
    wb.set_input(0, 1, 1, "200").unwrap();
    let a = dir.path().join("a.xlsx");
    wb.save_as(&a).unwrap();
    let p = parts(&a);
    assert_eq!(
        listed(&p),
        [
            "Revenue chart",
            "Data",
            "Share",
            "Hidden chart (hidden)",
            "Notes"
        ]
    );
    for part in [
        "xl/chartsheets/sheet1.xml",
        "xl/chartsheets/sheet2.xml",
        "xl/chartsheets/sheet3.xml",
        "xl/charts/chart1.xml",
        "xl/charts/chart2.xml",
        "xl/charts/chart3.xml",
    ] {
        assert!(p.contains_key(part), "{part} kept");
    }
    let problems = sheets_engine::package_problems(&std::fs::read(&a).unwrap());
    assert!(problems.is_empty(), "{problems:?}");
    // Saved again (the saved file is now the original), and reopened.
    wb.save_as(dir.path().join("b.xlsx")).unwrap();
    let again = Workbook::open(dir.path().join("b.xlsx")).unwrap();
    assert_eq!(tab_names(&again), TABS);
    assert_eq!(listed(&parts(&dir.path().join("b.xlsx"))), listed(&p));
    assert_eq!(again.display(0, 1, 1).text, "200");
}

fn text(p: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(p[name].clone()).unwrap()
}

/// Parts that differ between two saves (docProps/core.xml holds the save
/// time and is skipped).
fn changed(a: &BTreeMap<String, Vec<u8>>, b: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let mut out: Vec<String> = a
        .keys()
        .chain(b.keys())
        .filter(|n| *n != "docProps/core.xml" && a.get(*n) != b.get(*n))
        .cloned()
        .collect();
    out.sort();
    out.dedup();
    out
}

fn save(wb: &mut Workbook, dir: &Path, name: &str) -> BTreeMap<String, Vec<u8>> {
    let path = dir.join(name);
    wb.save_as(&path).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let problems = sheets_engine::package_problems(&bytes);
    assert!(problems.is_empty(), "{name}: {problems:?}");
    parts(&path)
}

/// Chart sheet `name`'s index and its chart's key.
fn chart_of(wb: &Workbook, name: &str) -> (usize, DrawingKey) {
    let k = wb
        .tabs()
        .into_iter()
        .find_map(|t| match t {
            Tab::Chart(k) if wb.chart_sheet_name(k).as_deref() == Some(name) => Some(k),
            _ => None,
        })
        .unwrap();
    (k, wb.chart_sheet_drawings(k)[0].key)
}

#[test]
fn chart_sheet_charts_restyle_and_save_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("22_chart_sheets.xlsx")).unwrap();
    let plain = save(&mut wb, dir.path(), "plain.xlsx");
    let (_, key) = chart_of(&wb, "Revenue chart");
    let info = wb.chart_info(key).expect("the panel shows it");
    assert_eq!(info.spec.title, "Revenue by region");
    assert!(!info.data_unknown, "{info:?}");
    // Its data is on Data, which the panel names.
    assert_eq!(info.spec.data_sheet, Some(wb.sheet_id(0)));
    let mut spec = info.spec.clone();
    spec.title = "Revenue 2026".into();
    spec.kind = sheets_engine::ChartType::Line;
    spec.legend = sheets_engine::LegendPos::Bottom;
    wb.set_chart(key, spec).unwrap();
    // Shown as edited.
    let DrawingKind::Chart { xml, .. } = &*wb.chart_sheet_drawings(0)[0].kind else {
        panic!()
    };
    assert!(xml.contains("Revenue 2026") && xml.contains("<c:lineChart>"));
    let edited = save(&mut wb, dir.path(), "edited.xlsx");
    // Only the chart part changed.
    assert_eq!(changed(&plain, &edited), ["xl/charts/chart1.xml"]);
    let chart = text(&edited, "xl/charts/chart1.xml");
    assert!(chart.contains("Revenue 2026") && chart.contains("<c:lineChart>"));
    assert!(chart.contains("<c:f>Data!$B$2:$B$5</c:f>"));
    // Saved again: the same.
    let again = save(&mut wb, dir.path(), "again.xlsx");
    assert!(changed(&edited, &again).is_empty());
    // Undone and saved: the part as it was.
    assert!(wb.undo().unwrap());
    let undone = save(&mut wb, dir.path(), "undone.xlsx");
    assert!(
        changed(&plain, &undone).is_empty(),
        "{:?}",
        changed(&plain, &undone)
    );
    // Redone: edited again.
    assert!(wb.redo().unwrap());
    let redone = save(&mut wb, dir.path(), "redone.xlsx");
    assert!(changed(&edited, &redone).is_empty());
    // Reopened: the edit is there, on the chart sheet.
    let back = Workbook::open(dir.path().join("redone.xlsx")).unwrap();
    assert_eq!(tab_names(&back), TABS);
    let (_, key) = chart_of(&back, "Revenue chart");
    assert_eq!(back.chart_info(key).unwrap().spec.title, "Revenue 2026");
}

#[test]
fn chart_sheet_data_range_and_inserted_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("22_chart_sheets.xlsx")).unwrap();
    let (_, key) = chart_of(&wb, "Share");
    let mut spec = wb.chart_info(key).unwrap().spec;
    // Typed without the sheet's name, the data stays on Data.
    spec.data = sheets_engine::CellRange::from_corners((0, 0), (3, 1));
    wb.set_chart(key, spec).unwrap();
    wb.insert_rows(0, 0, 2).unwrap();
    let p = save(&mut wb, dir.path(), "a.xlsx");
    let share = text(&p, "xl/charts/chart2.xml");
    assert!(share.contains("<c:f>Data!$B$4:$B$6</c:f>"), "{share}");
    // The untouched chart sheet's chart follows the rows too.
    let revenue = text(&p, "xl/charts/chart1.xml");
    assert!(revenue.contains("<c:f>Data!$B$4:$B$7</c:f>"), "{revenue}");
    // And still after a save (the saved file is the next original).
    wb.insert_rows(0, 0, 1).unwrap();
    let p = save(&mut wb, dir.path(), "b.xlsx");
    assert!(text(&p, "xl/charts/chart2.xml").contains("<c:f>Data!$B$5:$B$7</c:f>"));
    assert!(text(&p, "xl/charts/chart1.xml").contains("<c:f>Data!$B$5:$B$8</c:f>"));
}

#[test]
fn chart_sheet_tabs_rename_move_hide_and_delete() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("22_chart_sheets.xlsx")).unwrap();
    let plain = save(&mut wb, dir.path(), "plain.xlsx");
    let (rev, _) = chart_of(&wb, "Revenue chart");
    let (share, _) = chart_of(&wb, "Share");

    // Rename, following Excel's rules.
    assert!(
        wb.rename_chart_sheet(rev, "data").is_err(),
        "a worksheet's name"
    );
    assert!(
        wb.rename_chart_sheet(rev, "share").is_err(),
        "a chart sheet's"
    );
    assert!(
        wb.rename_chart_sheet(rev, "Hidden chart").is_err(),
        "a hidden one's"
    );
    assert!(wb.rename_chart_sheet(rev, "a/b").is_err());
    assert!(wb.rename_chart_sheet(rev, "").is_err());
    wb.rename_chart_sheet(rev, "Revenue").unwrap();
    assert_eq!(tab_names(&wb), ["[Revenue]", "Data", "[Share]", "Notes"]);

    // Move: Revenue to the end, Share to the front.
    wb.move_chart_sheet(rev, 3).unwrap();
    assert_eq!(tab_names(&wb), ["Data", "[Share]", "Notes", "[Revenue]"]);
    wb.move_chart_sheet(share, 0).unwrap();
    assert_eq!(tab_names(&wb), ["[Share]", "Data", "Notes", "[Revenue]"]);

    // Hide and unhide.
    wb.set_chart_sheet_hidden(share, true).unwrap();
    assert_eq!(tab_names(&wb), ["Data", "Notes", "[Revenue]"]);
    let hidden: Vec<String> = wb.hidden_chart_sheets().into_iter().map(|h| h.1).collect();
    assert_eq!(hidden, ["Share", "Hidden chart"]);
    let p = save(&mut wb, dir.path(), "a.xlsx");
    assert_eq!(
        listed(&p),
        [
            "Share (hidden)",
            "Data",
            "Hidden chart (hidden)",
            "Notes",
            "Revenue"
        ]
    );
    // Parts as they were: only the workbook's list (and the ids of its
    // relationships, given in tab order) changed.
    assert_eq!(
        changed(&plain, &p),
        ["xl/_rels/workbook.xml.rels", "xl/workbook.xml"]
    );
    let back = Workbook::open(dir.path().join("a.xlsx")).unwrap();
    assert_eq!(tab_names(&back), ["Data", "Notes", "[Revenue]"]);

    // Delete, save, undo, save: put back as opened.
    wb.set_chart_sheet_hidden(share, false).unwrap();
    wb.delete_chart_sheet(rev).unwrap();
    assert_eq!(tab_names(&wb), ["[Share]", "Data", "Notes"]);
    assert_eq!(wb.chart_sheet_name(rev), None);
    assert!(wb.chart_sheet_drawings(rev).is_empty());
    let p = save(&mut wb, dir.path(), "b.xlsx");
    assert!(!p.contains_key("xl/chartsheets/sheet1.xml"));
    assert!(!p.contains_key("xl/charts/chart1.xml"));
    assert!(!text(&p, "[Content_Types].xml").contains("chartsheets/sheet1.xml"));
    assert_eq!(
        listed(&p),
        ["Share", "Data", "Hidden chart (hidden)", "Notes"]
    );
    for _ in 0..6 {
        assert!(wb.undo().unwrap());
    }
    assert_eq!(tab_names(&wb), TABS);
    let p = save(&mut wb, dir.path(), "c.xlsx");
    assert_eq!(listed(&p), listed(&plain));
    // Every part as it was (the workbook's relationship ids and the order
    // of the content types may differ).
    let diff = changed(&plain, &p);
    assert!(
        diff.iter().all(|n| matches!(
            n.as_str(),
            "xl/_rels/workbook.xml.rels" | "xl/workbook.xml" | "[Content_Types].xml"
        )),
        "{diff:?}"
    );
    let ct = text(&p, "[Content_Types].xml");
    for part in [
        "/xl/chartsheets/sheet1.xml",
        "/xl/drawings/drawing1.xml",
        "/xl/charts/chart1.xml",
    ] {
        assert_eq!(
            ct.matches(&format!("\"{part}\"")).count(),
            1,
            "{part}: {ct}"
        );
    }
    let back = Workbook::open(dir.path().join("c.xlsx")).unwrap();
    assert_eq!(tab_names(&back), TABS);
    let (_, key) = chart_of(&back, "Revenue chart");
    assert_eq!(
        back.chart_info(key).unwrap().spec.title,
        "Revenue by region"
    );
}

#[test]
fn restored_chart_sheet_follows_rows_inserted_since_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut wb = Workbook::open(corpus("22_chart_sheets.xlsx")).unwrap();
    let (rev, _) = chart_of(&wb, "Revenue chart");
    wb.delete_chart_sheet(rev).unwrap();
    wb.insert_rows(0, 0, 1).unwrap();
    let p = save(&mut wb, dir.path(), "c.xlsx");
    assert!(!p.contains_key("xl/chartsheets/sheet1.xml"));
    wb.undo().unwrap(); // the row
    wb.undo().unwrap(); // the deletion
    wb.insert_rows(0, 0, 2).unwrap();
    let p = save(&mut wb, dir.path(), "d.xlsx");
    let chart = text(&p, "xl/charts/chart1.xml");
    assert!(chart.contains("<c:f>Data!$B$4:$B$7</c:f>"), "{chart}");
    assert_eq!(listed(&p)[0], "Revenue chart");
}

#[test]
fn names_scoped_to_sheets_follow_moved_chart_sheets() {
    // The fixture with a print area on Notes (sheet 5 of the list).
    let dir = tempfile::tempdir().unwrap();
    let src = parts(&corpus("22_chart_sheets.xlsx"));
    let path = dir.path().join("names.xlsx");
    {
        let f = std::fs::File::create(&path).unwrap();
        let mut zip = zip::ZipWriter::new(f);
        for (name, data) in &src {
            zip.start_file(name.as_str(), zip::write::SimpleFileOptions::default())
                .unwrap();
            let mut data = data.clone();
            if name == "xl/workbook.xml" {
                data = String::from_utf8(data)
                    .unwrap()
                    .replace(
                        "</sheets>",
                        "</sheets><definedNames><definedName name=\"_xlnm.Print_Area\" localSheetId=\"4\">Notes!$A$1:$B$3</definedName></definedNames>",
                    )
                    .into_bytes();
            }
            std::io::Write::write_all(&mut zip, &data).unwrap();
        }
        zip.finish().unwrap();
    }
    let mut wb = Workbook::open(&path).unwrap();
    let (rev, _) = chart_of(&wb, "Revenue chart");
    // Revenue chart after Notes: Notes is the 4th sheet of the list.
    wb.move_chart_sheet(rev, 3).unwrap();
    let p = save(&mut wb, dir.path(), "a.xlsx");
    assert_eq!(
        listed(&p),
        [
            "Data",
            "Share",
            "Hidden chart (hidden)",
            "Notes",
            "Revenue chart"
        ]
    );
    let wbx = text(&p, "xl/workbook.xml");
    assert!(wbx.contains("localSheetId=\"3\""), "{wbx}");
    // Deleted: Notes is the 3rd.
    let (share, _) = chart_of(&wb, "Share");
    wb.delete_chart_sheet(share).unwrap();
    let p = save(&mut wb, dir.path(), "b.xlsx");
    let wbx = text(&p, "xl/workbook.xml");
    assert!(wbx.contains("localSheetId=\"2\""), "{wbx}");
}
