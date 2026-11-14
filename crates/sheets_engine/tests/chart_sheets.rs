//! Chart sheets (tests/corpus/xlsx/22_chart_sheets.xlsx): shown as tabs in
//! the order saving keeps, their charts read, and their parts saved.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::drawing::DrawingKind;
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
    assert!(!objects[0].editable);
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
