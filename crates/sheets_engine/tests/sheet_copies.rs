//! Move or Copy with "Create a copy": a worksheet's charts, pictures and
//! tables come with it; chart sheets are copied and get tab colours. Saved,
//! reopened, undone; save → undo → save.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::drawing::DrawingKind;
use sheets_engine::{Tab, TabColor, Workbook};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("sheet_copies");
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

/// (charts, pictures) on `sheet`, and the chart parts' XML.
fn objects(wb: &Workbook, sheet: u32) -> (usize, usize, Vec<String>) {
    let mut charts = 0;
    let mut pictures = 0;
    let mut xml = Vec::new();
    for d in wb.drawings(sheet) {
        match &*d.kind {
            DrawingKind::Chart { xml: x, .. } => {
                charts += 1;
                xml.push(x.to_string());
            }
            DrawingKind::Picture { .. } => pictures += 1,
            DrawingKind::Other(_) => {}
        }
    }
    (charts, pictures, xml)
}

fn sheet(wb: &Workbook, name: &str) -> u32 {
    wb.sheet_names().iter().position(|n| n == name).unwrap() as u32
}

#[test]
fn copied_sheet_brings_its_charts_and_pictures() {
    let mut wb = Workbook::open(corpus("14_charts_images.xlsx")).unwrap();
    let data = sheet(&wb, "Data");
    let (charts, pictures, _) = objects(&wb, data);
    assert!(charts >= 1 && pictures >= 1, "{charts} {pictures}");
    let at = wb.tabs().len();
    let copy = wb.copy_sheet(data, at).unwrap();
    assert_eq!(wb.sheet_names()[copy as usize], "Data (2)");
    let (c2, p2, xml) = objects(&wb, copy);
    assert_eq!((c2, p2), (charts, pictures));
    // The copy's charts read the copy's cells, as in Excel.
    assert!(
        xml.iter().any(|x| x.contains("'Data (2)'!$B$2:$B$7")),
        "{xml:?}"
    );
    // Moving one on the copy leaves the original alone.
    let k = wb.drawings(copy)[0].key;
    let a = wb.drawings(copy)[0].anchor;
    wb.delete_drawing(k).unwrap();
    assert_eq!(
        objects(&wb, data).0 + objects(&wb, data).1,
        charts + pictures
    );
    assert!(wb.undo().unwrap());
    assert_eq!(wb.drawings(copy)[0].anchor, a);

    let path = scratch("copy.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    assert!(
        p.keys().filter(|n| n.starts_with("xl/media/")).count() >= 2,
        "{:?}",
        p.keys().collect::<Vec<_>>()
    );
    let back = Workbook::open(&path).unwrap();
    let bc = sheet(&back, "Data (2)");
    let (c3, p3, xml) = objects(&back, bc);
    assert_eq!((c3, p3), (charts, pictures));
    assert!(xml.iter().any(|x| x.contains("'Data (2)'!$B$2:$B$7")));
    assert_eq!(
        (
            objects(&back, sheet(&back, "Data")).0,
            objects(&back, sheet(&back, "Data")).1
        ),
        (charts, pictures)
    );

    // Save → undo → save: the copy and its objects are gone.
    assert!(wb.undo().unwrap());
    wb.save_as(&path).unwrap();
    assert!(
        wb.save_notes().iter().all(|n| n.contains("was deleted")),
        "{:?}",
        wb.save_notes()
    );
    let back = Workbook::open(&path).unwrap();
    assert!(!back.sheet_names().contains(&"Data (2)".to_owned()));
    let p = parts(&path);
    assert_eq!(p.keys().filter(|n| n.starts_with("xl/media/")).count(), 1);
    // Saved twice after redo: once.
    assert!(wb.redo().unwrap());
    wb.save_as(&path).unwrap();
    wb.save_as(&path).unwrap();
    let back = Workbook::open(&path).unwrap();
    let (c4, p4, _) = objects(&back, sheet(&back, "Data (2)"));
    assert_eq!((c4, p4), (charts, pictures));
}

#[test]
fn copied_sheet_brings_its_tables_renamed() {
    let mut wb = Workbook::open(corpus("13_tables.xlsx")).unwrap();
    let sales = sheet(&wb, "Sales");
    let copy = wb.copy_sheet(sales, 1).unwrap();
    let copy_name = wb.sheet_names()[copy as usize].clone();
    assert_eq!(copy_name, "Sales (2)");
    // Excel adds a number to the name: Sales → Sales2.
    let t = wb.table_options("Sales2").expect("the copy's table");
    let orig = wb.table_options("Sales").unwrap();
    assert_eq!(t.style, orig.style);
    let path = scratch("tables.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    let tables: Vec<&String> = p
        .iter()
        .filter(|(n, _)| n.starts_with("xl/tables/"))
        .map(|(_, x)| x)
        .collect();
    assert!(tables.iter().any(|x| x.contains("name=\"Sales2\"")));
    assert!(tables.iter().any(|x| x.contains("name=\"Sales\"")));
    let back = Workbook::open(&path).unwrap();
    assert!(back.table_options("Sales2").is_some());
    // Copied again: Sales3.
    let c2 = wb.copy_sheet(sales, 1).unwrap();
    assert!(wb.table_options("Sales3").is_some());
    let _ = c2;
    assert!(wb.undo().unwrap());
    assert!(wb.undo().unwrap());
    assert!(wb.table_options("Sales2").is_none());
    wb.save_as(&path).unwrap();
    let p = parts(&path);
    assert!(
        !p.iter()
            .any(|(n, x)| n.starts_with("xl/tables/") && x.contains("name=\"Sales2\""))
    );
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

#[test]
fn chart_sheets_copy_and_take_tab_colours() {
    let mut wb = Workbook::open(corpus("22_chart_sheets.xlsx")).unwrap();
    let Tab::Chart(k) = wb.tabs()[0] else {
        panic!()
    };
    let before = tab_names(&wb);
    let red = TabColor::Rgb(0xC00000);
    wb.set_chart_sheet_tab_color(k, Some(red)).unwrap();
    assert_eq!(wb.chart_sheet_tab_color(k), Some(red));
    let copy = wb.copy_chart_sheet(k, 1).unwrap();
    let mut want = before.clone();
    want.insert(1, "[Revenue chart (2)]".into());
    assert_eq!(tab_names(&wb), want);
    assert_eq!(wb.chart_sheet_tab_color(copy), Some(red));
    let accent = TabColor::Theme { slot: 5, tint: 0.4 };
    wb.set_chart_sheet_tab_color(copy, Some(accent)).unwrap();
    let chart = wb.chart_sheet_drawings(copy);
    assert_eq!(chart.len(), 1);
    let DrawingKind::Chart { xml, .. } = &*chart[0].kind else {
        panic!()
    };
    assert!(xml.contains("Revenue by region"));

    let path = scratch("cs-copy.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let back = Workbook::open(&path).unwrap();
    assert_eq!(tab_names(&back), want);
    let ks: Vec<usize> = back
        .tabs()
        .into_iter()
        .filter_map(|t| match t {
            Tab::Chart(k) => Some(k),
            _ => None,
        })
        .collect();
    assert_eq!(back.chart_sheet_tab_color(ks[0]), Some(red));
    assert_eq!(back.chart_sheet_tab_color(ks[1]), Some(accent));
    assert_eq!(back.chart_sheet_drawings(ks[1]).len(), 1);

    // Save → undo → save: the colour of the first goes again.
    for _ in 0..3 {
        assert!(wb.undo().unwrap());
    }
    assert_eq!(tab_names(&wb), before);
    assert_eq!(wb.chart_sheet_tab_color(k), None);
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let back = Workbook::open(&path).unwrap();
    assert_eq!(tab_names(&back), before);
    let Tab::Chart(bk) = back.tabs()[0] else {
        panic!()
    };
    assert_eq!(back.chart_sheet_tab_color(bk), None);
    let p = parts(&path);
    let orig = parts(&corpus("22_chart_sheets.xlsx"));
    for (n, x) in &orig {
        if n.starts_with("xl/chartsheets/") {
            assert_eq!(p.get(n), Some(x), "{n} as it was");
        }
    }
}
