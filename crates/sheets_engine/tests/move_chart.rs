//! Move Chart: an embedded chart to a new chart sheet and a chart sheet's
//! chart onto a worksheet; undo; the saved package (chart sheet part, its
//! drawing, relationships, content types; the chart gone from the old
//! drawing); save → undo → save.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use roxmltree::Document;
use sheets_engine::drawing::{DrawingKey, DrawingKind};
use sheets_engine::{CellRange, ChartPlace, ChartType, MovedChart, Tab, Workbook};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("move_chart");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn parts(path: &Path) -> BTreeMap<String, String> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut e = zip.by_index(i).unwrap();
        let mut b = Vec::new();
        e.read_to_end(&mut b).unwrap();
        out.insert(
            e.name().unwrap().into_owned(),
            String::from_utf8_lossy(&b).into_owned(),
        );
    }
    out
}

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

/// `<sheet name=…>` of workbook.xml in order.
fn listed(p: &BTreeMap<String, String>) -> Vec<String> {
    let doc = Document::parse(&p["xl/workbook.xml"]).unwrap();
    doc.descendants()
        .filter(|n| n.has_tag_name("sheet"))
        .map(|n| n.attribute("name").unwrap().to_owned())
        .collect()
}

fn charts(wb: &Workbook, sheet: u32) -> Vec<DrawingKey> {
    wb.drawings(sheet)
        .iter()
        .filter(|d| matches!(*d.kind, DrawingKind::Chart { .. }))
        .map(|d| d.key)
        .collect()
}

/// The chart parts the chart sheets' drawings link to, checking every
/// link resolves and has a content type.
fn chart_sheet_charts(p: &BTreeMap<String, String>) -> Vec<String> {
    let types = &p["[Content_Types].xml"];
    let mut out = Vec::new();
    for (name, xml) in p {
        if !name.starts_with("xl/chartsheets/") || name.contains("_rels") {
            continue;
        }
        assert!(types.contains(&format!("PartName=\"/{name}\"")), "{name}");
        let (dir, file) = name.rsplit_once('/').unwrap();
        let rels = &p[&format!("{dir}/_rels/{file}.rels")];
        assert!(xml.contains("<drawing r:id="), "{xml}");
        let target = rels.split("Target=\"../drawings/").nth(1).unwrap();
        let drawing = format!("xl/drawings/{}", &target[..target.find('"').unwrap()]);
        assert!(
            types.contains(&format!("PartName=\"/{drawing}\"")),
            "{drawing}"
        );
        let drels = &p[&format!(
            "xl/drawings/_rels/{}.rels",
            drawing.rsplit('/').next().unwrap()
        )];
        let target = drels.split("Target=\"../charts/").nth(1).unwrap();
        let chart = format!("xl/charts/{}", &target[..target.find('"').unwrap()]);
        assert!(types.contains(&format!("PartName=\"/{chart}\"")), "{chart}");
        out.push(p[&chart].clone());
    }
    out
}

#[test]
fn file_chart_to_a_new_sheet_and_back() {
    let src = corpus("20_charts.xlsx");
    let mut wb = Workbook::open(&src).unwrap();
    let before = charts(&wb, 0);
    let key = before[0];
    let title = "Moved away";
    let mut spec = wb.chart_info(key).unwrap().spec;
    spec.title = title.into();
    wb.set_chart(key, spec).unwrap();
    assert_eq!(wb.next_chart_sheet_name(), "Chart1");
    let tabs_before = tab_names(&wb);
    let moved = wb
        .move_chart(key, ChartPlace::NewSheet("Chart1".into()))
        .unwrap();
    let MovedChart::ChartSheet(k) = moved else {
        panic!("{moved:?}")
    };
    // Before the sheet it was on, as Excel puts it.
    let mut want = vec!["[Chart1]".to_owned()];
    want.extend(tabs_before.iter().cloned());
    assert_eq!(tab_names(&wb), want);
    assert_eq!(charts(&wb, 0).len(), before.len() - 1);
    let on_sheet = wb.chart_sheet_drawings(k);
    assert_eq!(on_sheet.len(), 1);
    let info = wb.chart_info(on_sheet[0].key).unwrap();
    assert_eq!(info.spec.title, title);
    // The name is taken now.
    assert_eq!(wb.next_chart_sheet_name(), "Chart2");
    assert!(
        wb.move_chart(before[1], ChartPlace::NewSheet("chart1".into()))
            .is_err()
    );

    let path = scratch("to-sheet.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    assert_eq!(listed(&p)[0], "Chart1");
    let cs = chart_sheet_charts(&p);
    assert_eq!(cs.len(), 1);
    assert!(cs[0].contains(title), "{}", cs[0]);
    // Gone from the worksheet's drawing.
    let on_ws: Vec<&String> = p
        .iter()
        .filter(|(n, _)| n.starts_with("xl/charts/") && !n.contains("_rels"))
        .filter(|(_, x)| x.contains(title))
        .map(|(n, _)| n)
        .collect();
    assert_eq!(on_ws.len(), 1, "{on_ws:?}");
    let back = Workbook::open(&path).unwrap();
    assert_eq!(tab_names(&back), want);
    assert_eq!(charts(&back, 0).len(), before.len() - 1);

    // And back onto the sheet: the chart sheet goes.
    let moved = wb
        .move_chart(on_sheet[0].key, ChartPlace::ObjectIn(0))
        .unwrap();
    let MovedChart::Object { sheet: 0, key: k2 } = moved else {
        panic!("{moved:?}")
    };
    assert_eq!(tab_names(&wb), tabs_before);
    assert_eq!(charts(&wb, 0).len(), before.len());
    assert_eq!(wb.chart_info(k2).unwrap().spec.title, title);
    // It is still a chart Kantor restyles in place.
    let mut spec = wb.chart_info(k2).unwrap().spec;
    spec.kind = ChartType::Line;
    wb.set_chart(k2, spec).unwrap();
    assert_eq!(wb.chart_info(k2).unwrap().spec.kind, ChartType::Line);
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let p = parts(&path);
    assert!(chart_sheet_charts(&p).is_empty());
    assert!(!listed(&p).contains(&"Chart1".to_owned()));
    let back = Workbook::open(&path).unwrap();
    assert_eq!(charts(&back, 0).len(), before.len());

    // Undo all the way: the file's chart is back on its sheet.
    for _ in 0..3 {
        assert!(wb.undo().unwrap());
    }
    assert_eq!(tab_names(&wb), tabs_before);
    assert_eq!(charts(&wb, 0), before);
}

#[test]
fn save_undo_save_and_redo() {
    let src = corpus("20_charts.xlsx");
    let original = {
        let p = scratch("plain.xlsx");
        Workbook::open(&src).unwrap().save_as(&p).unwrap();
        parts(&p)
    };
    let mut wb = Workbook::open(&src).unwrap();
    let key = charts(&wb, 0)[0];
    wb.move_chart(key, ChartPlace::NewSheet("Chart1".into()))
        .unwrap();
    let path = scratch("sus.xlsx");
    wb.save_as(&path).unwrap();
    assert_eq!(chart_sheet_charts(&parts(&path)).len(), 1);
    assert!(wb.undo().unwrap());
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let p = parts(&path);
    assert!(chart_sheet_charts(&p).is_empty());
    assert_eq!(listed(&p), listed(&original));
    let back = Workbook::open(&path).unwrap();
    assert_eq!(charts(&back, 0).len(), charts(&wb, 0).len());
    // Redo, saved twice: one chart sheet.
    assert!(wb.redo().unwrap());
    wb.save_as(&path).unwrap();
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let p = parts(&path);
    assert_eq!(chart_sheet_charts(&p).len(), 1);
    assert_eq!(listed(&p).iter().filter(|n| *n == "Chart1").count(), 1);
}

#[test]
fn chart_sheet_of_a_file_to_a_worksheet() {
    let mut wb = Workbook::open(corpus("22_chart_sheets.xlsx")).unwrap();
    let Tab::Chart(k) = wb.tabs()[0] else {
        panic!("a chart sheet first")
    };
    let key = wb.chart_sheet_drawings(k)[0].key;
    let notes = wb.sheet_names().iter().position(|n| n == "Notes").unwrap() as u32;
    let moved = wb.move_chart(key, ChartPlace::ObjectIn(notes)).unwrap();
    let MovedChart::Object { key: new, .. } = moved else {
        panic!()
    };
    assert!(wb.chart_sheet_name(k).is_none());
    assert_eq!(charts(&wb, notes), vec![new]);
    let path = scratch("cs-to-ws.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let back = Workbook::open(&path).unwrap();
    let ks = charts(&back, notes);
    assert_eq!(ks.len(), 1);
    let DrawingKind::Chart { xml, .. } = &*back.drawings(notes)[0].kind else {
        panic!()
    };
    assert!(xml.contains("Revenue by region"));
    assert!(!tab_names(&back).contains(&"[Revenue chart]".to_owned()));
    assert!(wb.undo().unwrap());
    assert_eq!(wb.chart_sheet_name(k).as_deref(), Some("Revenue chart"));
    assert!(charts(&wb, notes).is_empty());
}

#[test]
fn chart_made_in_kantor_in_a_new_workbook() {
    let mut wb = Workbook::new_empty().unwrap();
    let mut cells = Vec::new();
    for (r, (a, b)) in [
        ("Month", "Sales"),
        ("Jan", "10"),
        ("Feb", "20"),
        ("Mar", "15"),
    ]
    .iter()
    .enumerate()
    {
        cells.push((r as u32, 0, a.to_string()));
        cells.push((r as u32, 1, b.to_string()));
    }
    wb.set_inputs(0, &cells).unwrap();
    let key = wb
        .insert_chart(
            0,
            CellRange::from_corners((0, 0), (3, 1)),
            ChartType::Column,
        )
        .unwrap();
    let MovedChart::ChartSheet(k) = wb
        .move_chart(key, ChartPlace::NewSheet("Sales chart".into()))
        .unwrap()
    else {
        panic!()
    };
    assert!(charts(&wb, 0).is_empty());
    // Data changed after the move: the chart follows.
    wb.set_input(0, 1, 1, "99").unwrap();
    let path = scratch("new-book.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    assert_eq!(listed(&p), ["Sales chart", "Sheet1"]);
    let cs = chart_sheet_charts(&p);
    assert_eq!(cs.len(), 1);
    assert!(cs[0].contains("Sheet1!$B$2:$B$4"), "{}", cs[0]);
    let back = Workbook::open(&path).unwrap();
    assert_eq!(tab_names(&back), ["[Sales chart]", "Sheet1"]);
    let Tab::Chart(bk) = back.tabs()[0] else {
        panic!()
    };
    assert_eq!(back.chart_sheet_drawings(bk).len(), 1);
    let _ = k;
}
