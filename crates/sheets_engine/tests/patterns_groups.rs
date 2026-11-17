//! Pattern fills in shapes and charts inside groups, against
//! `27_patterns_group_charts.xlsx` and LibreOffice's `.ods` and `.xls` of it
//! (`scripts/corpus/gen_patterns_groups.py`): read, saved, copied with
//! their sheet, imported.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::ShapeFill;
use sheets_engine::Workbook;
use sheets_engine::drawing::{Drawing, DrawingKind, GROUP_CHART};
use sheets_engine::shape_format::{self as fmt};

fn corpus(dir: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus")
        .join(dir)
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("patterns_groups");
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

fn named(wb: &Workbook, sheet: u32, name: &str) -> Drawing {
    wb.drawings(sheet)
        .into_iter()
        .find(|d| d.name == name)
        .unwrap_or_else(|| {
            let all: Vec<String> = wb.drawings(sheet).into_iter().map(|d| d.name).collect();
            panic!("no {name} in {all:?}")
        })
}

fn shape_xml(d: &Drawing) -> String {
    match &*d.kind {
        DrawingKind::Shape { xml, .. } => xml.to_string(),
        k => panic!("{} is {k:?}", d.name),
    }
}

/// The charts inside a group: (`r:id`, chart XML).
fn group_charts(d: &Drawing) -> Vec<(String, String)> {
    match &*d.kind {
        DrawingKind::Shape { media, .. } => media
            .iter()
            .filter(|(_, ext, _)| ext == GROUP_CHART)
            .map(|(id, _, data)| (id.clone(), String::from_utf8_lossy(data).into_owned()))
            .collect(),
        k => panic!("{} is {k:?}", d.name),
    }
}

#[test]
fn pattern_fills_are_read_and_set() {
    let wb = Workbook::open(corpus("xlsx", "27_patterns_group_charts.xlsx")).unwrap();
    let xml = shape_xml(&named(&wb, 0, "Diagonal box"));
    assert!(xml.contains("<a:pattFill prst=\"dkUpDiag\">"), "{xml}");
    let set = fmt::set_fill(
        &xml,
        ShapeFill::Pattern {
            prst: "lgCheck",
            fg: [0, 0, 0],
            bg: [255, 255, 255],
        },
    )
    .unwrap();
    assert!(set.contains("<a:pattFill prst=\"lgCheck\">") && !set.contains("dkUpDiag"));
}

#[test]
fn a_groups_chart_is_read_saved_and_copied() {
    let mut wb = Workbook::open(corpus("xlsx", "27_patterns_group_charts.xlsx")).unwrap();
    let group = named(&wb, 0, "Chart group");
    let charts = group_charts(&group);
    assert_eq!(charts.len(), 1, "{charts:?}");
    assert!(charts[0].1.contains("<c:f>Patterns!$B$2:$B$5</c:f>"));
    assert!(charts[0].1.contains("prst=\"ltUpDiag\""));
    // The chart outside a group is a chart as before.
    assert!(matches!(
        *named(&wb, 0, "Plain chart").kind,
        DrawingKind::Chart { .. }
    ));

    // Saved unchanged: the group keeps its chart.
    let path = scratch("saved.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let back = Workbook::open(&path).unwrap();
    assert_eq!(group_charts(&named(&back, 0, "Chart group")).len(), 1);

    // Move or Copy: the copy's group has a chart of its own reading the
    // copy's cells.
    let copy = wb.copy_sheet(0, 1).unwrap();
    let copied = named(&wb, copy, "Chart group");
    let cc = group_charts(&copied);
    assert_eq!(cc.len(), 1);
    assert!(cc[0].1.contains("'Patterns (2)'!$B$2:$B$5"), "{}", cc[0].1);
    let path = scratch("copied.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    let charts: Vec<&String> = p
        .keys()
        .filter(|k| k.starts_with("xl/charts/chart"))
        .collect();
    // Two of the original, the copied plain chart and the copied group's.
    assert_eq!(charts.len(), 4, "{charts:?}");
    let back = Workbook::open(&path).unwrap();
    let c = back
        .sheet_names()
        .iter()
        .position(|n| n == "Patterns (2)")
        .unwrap() as u32;
    let cc = group_charts(&named(&back, c, "Chart group"));
    assert_eq!(cc.len(), 1);
    assert!(cc[0].1.contains("'Patterns (2)'!$B$2:$B$5"));
    // The original's group still reads the original's cells.
    let oc = group_charts(&named(&back, 0, "Chart group"));
    assert!(oc[0].1.contains("<c:f>Patterns!$B$2:$B$5</c:f>"));
}

#[test]
fn ods_hatches_become_patterns_and_grouped_charts_come_in() {
    let wb = Workbook::open(corpus("xls", "patterns_groups.ods")).unwrap();
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());
    let xml = shape_xml(&named(&wb, 0, "Diagonal box"));
    // LibreOffice's hatch for dkUpDiag: close lines at 45°.
    assert!(xml.contains("<a:pattFill prst=\"dkUpDiag\">"), "{xml}");
    assert!(
        xml.contains("<a:fgClr><a:srgbClr val=\"4472C4\"/>"),
        "{xml}"
    );
    assert!(
        xml.contains("<a:bgClr><a:srgbClr val=\"FFFFFF\"/>"),
        "{xml}"
    );
    let oval = shape_xml(&named(&wb, 0, "Checker oval"));
    assert!(oval.contains("<a:pattFill prst=\"diagCross\">"), "{oval}");
    let group = wb
        .drawings(0)
        .into_iter()
        .find(|d| !group_charts_or_empty(d).is_empty())
        .expect("a group with its chart");
    let charts = group_charts(&group);
    assert_eq!(charts.len(), 1);
    assert!(charts[0].1.contains("<c:barChart>"), "{}", charts[0].1);
}

fn group_charts_or_empty(d: &Drawing) -> Vec<(String, String)> {
    match &*d.kind {
        DrawingKind::Shape { .. } => group_charts(d),
        _ => Vec::new(),
    }
}

#[test]
fn xls_grouped_charts_come_in() {
    let wb = Workbook::open(corpus("xls", "patterns_groups.xls")).unwrap();
    assert!(
        !wb.import_notes().iter().any(|n| n.contains("chart")),
        "{:?}",
        wb.import_notes()
    );
    let group = wb
        .drawings(0)
        .into_iter()
        .find(|d| !group_charts_or_empty(d).is_empty())
        .expect("a group with its chart");
    let charts = group_charts(&group);
    assert_eq!(charts.len(), 1);
    // Saved as .xlsx and opened again: still there.
    let mut wb = wb;
    let path = scratch("from_xls.xlsx");
    wb.save_as(&path).unwrap();
    let back = Workbook::open(&path).unwrap();
    assert!(
        back.drawings(0)
            .iter()
            .any(|d| !group_charts_or_empty(d).is_empty())
    );
}
