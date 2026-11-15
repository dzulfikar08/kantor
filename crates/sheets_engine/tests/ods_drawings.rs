//! Pictures, charts and shapes of OpenDocument imports, against
//! `tests/corpus/xls/drawings.ods` and `shapes.ods` (made by
//! `scripts/corpus/gen_xls.py`).

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::drawing::{Anchor, Drawing, DrawingKind, EMU_PER_PX};
use sheets_engine::{CellRange, ChartType, LegendPos, Workbook};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xls")
        .join(name)
}

fn parts(path: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    (0..zip.len())
        .map(|i| {
            let mut f = zip.by_index(i).unwrap();
            let mut b = Vec::new();
            f.read_to_end(&mut b).unwrap();
            (f.name().unwrap().into_owned(), b)
        })
        .collect()
}

fn chart_xml(d: &Drawing) -> &str {
    match &*d.kind {
        DrawingKind::Chart { xml, .. } => xml,
        k => panic!("not a chart: {k:?}"),
    }
}

fn shape_xml(d: &Drawing) -> &str {
    match &*d.kind {
        DrawingKind::Shape { xml, .. } => xml,
        k => panic!("not a shape: {k:?}"),
    }
}

/// (col, row) of both corners.
fn corners(a: &Anchor) -> ((u32, u32), (u32, u32)) {
    let Anchor::TwoCell { from, to, .. } = *a else {
        panic!("{a:?}")
    };
    ((from.col, from.row), (to.col, to.row))
}

#[test]
fn ods_pictures_and_charts_are_imported() {
    let wb = Workbook::open(corpus("drawings.ods")).unwrap();
    assert_eq!(
        wb.import_notes(),
        ["1 chart not imported (radar, surface, bubble, bar of pie or combination charts)"]
    );
    assert_eq!(wb.sheet_names(), ["Data", "Charts", "Overview"]);

    let on_data = wb.drawings(0);
    assert_eq!(on_data.len(), 3, "{on_data:?}");
    // The column chart from E2.
    let col = &on_data[0];
    assert_eq!(corners(&col.anchor).0, (4, 1));
    let info = wb.chart_info(col.key).unwrap();
    assert_eq!(info.spec.kind, ChartType::Column);
    assert_eq!(info.spec.title, "Sales by quarter");
    assert_eq!(info.spec.x_title, "Quarter");
    assert_eq!(info.spec.y_title, "Units");
    assert_eq!(info.spec.legend, LegendPos::Right);
    assert_eq!(info.spec.data, CellRange::from_corners((0, 0), (4, 2)));
    assert_eq!(info.series, ["North", "South"]);
    let xml = chart_xml(col);
    assert!(xml.contains("<c:f>Data!$B$2:$B$5</c:f>"), "{xml}");
    assert!(xml.contains("<c:f>Data!$A$2:$A$5</c:f>"), "{xml}");
    assert!(xml.contains("<c:v>18</c:v>"), "{xml}");

    // The pictures at A8 and C8: 120 x 80 and 90 x 60 px (LibreOffice's
    // rows are 17 px).
    let pictures: Vec<&Drawing> = on_data
        .iter()
        .filter(|d| matches!(&*d.kind, DrawingKind::Picture { .. }))
        .collect();
    assert_eq!(pictures.len(), 2);
    let DrawingKind::Picture { data, part, .. } = &*pictures[0].kind else {
        unreachable!()
    };
    assert!(data.starts_with(b"\x89PNG"), "{part}");
    assert_eq!(corners(&pictures[0].anchor), ((0, 7), (1, 11)));
    let DrawingKind::Picture { data, .. } = &*pictures[1].kind else {
        unreachable!()
    };
    assert!(data.starts_with(&[0xFF, 0xD8]));
    assert_eq!(corners(&pictures[1].anchor).0, (2, 7));
    let Anchor::TwoCell { from, to, .. } = pictures[1].anchor else {
        unreachable!()
    };
    // 90 px wide from C: the default column is 64 px.
    let right = f64::from(to.col - from.col) * 64.0 + to.col_off as f64 / EMU_PER_PX;
    assert!((right - 90.0).abs() < 2.0, "{right}");

    let charts = wb.drawings(1);
    let kinds: Vec<(ChartType, String, String)> = charts
        .iter()
        .map(|d| {
            let i = wb.chart_info(d.key).unwrap();
            (i.spec.kind, i.kind_label, i.spec.title)
        })
        .collect();
    let want = [
        (ChartType::StackedBar, "Stacked Bar", "Stacked bars"),
        (ChartType::Line, "Line", "Line"),
        (ChartType::StackedArea, "Stacked Area", "Area"),
        (ChartType::Pie, "Pie", "Pie"),
        (ChartType::Other, "Doughnut", "Doughnut"),
        (ChartType::Scatter, "Scatter", "Scatter"),
    ];
    assert_eq!(kinds.len(), want.len(), "{kinds:?}");
    for (got, want) in kinds.iter().zip(want) {
        assert_eq!((got.0, got.1.as_str(), got.2.as_str()), want, "{kinds:?}");
    }
    let bar = chart_xml(&charts[0]);
    assert!(
        bar.contains("<c:barDir val=\"bar\"/><c:grouping val=\"stacked\"/>"),
        "{bar}"
    );
    let scatter = chart_xml(&charts[5]);
    assert!(
        scatter.contains("<c:xVal><c:numRef><c:f>Data!$B$2:$B$5</c:f>"),
        "{scatter}"
    );
    assert!(scatter.contains("<c:v>South vs North</c:v>"), "{scatter}");

    // "Overview" holds one chart (no size in the file: the chart's own).
    let overview = wb.drawings(2);
    assert_eq!(overview.len(), 1);
    assert!(chart_xml(&overview[0]).contains("<a:t>Overview</a:t>"));
}

#[test]
fn ods_shapes_are_imported() {
    let wb = Workbook::open(corpus("shapes.ods")).unwrap();
    let shapes = wb.drawings(0);
    let names: Vec<&str> = shapes.iter().map(|d| d.name.as_str()).collect();
    for want in [
        "Revenue box",
        "Gradient oval",
        "Dashed frame",
        "Arrow line",
        "Turned arrow",
        "Notes box",
        "Square",
        "Circle",
        "Note callout",
        "Hidden shape",
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    let by = |n: &str| shapes.iter().find(|d| d.name == n).unwrap();
    let revenue = shape_xml(by("Revenue box"));
    assert!(revenue.contains("prst=\"rect\""), "{revenue}");
    assert!(revenue.contains("<a:t>Revenue</a:t>"), "{revenue}");
    assert!(revenue.contains(" b=\"1\""), "{revenue}");
    assert!(revenue.contains("<a:pPr algn=\"ctr\"/>"), "{revenue}");
    let oval = shape_xml(by("Gradient oval"));
    assert!(oval.contains("prst=\"ellipse\""), "{oval}");
    assert!(oval.contains("<a:gradFill"), "{oval}");
    let frame = shape_xml(by("Dashed frame"));
    assert!(frame.contains("prst=\"roundRect\""), "{frame}");
    assert!(frame.contains("<a:prstDash val=\"dash\"/>"), "{frame}");
    assert!(frame.contains("<a:noFill/>"), "{frame}");
    let line = shape_xml(by("Arrow line"));
    assert!(line.starts_with("<xdr:cxnSp"), "{line}");
    assert!(
        line.contains("Arrow") || line.contains("End type="),
        "{line}"
    );
    let turned = shape_xml(by("Turned arrow"));
    assert!(turned.contains("prst=\"rightArrow\""), "{turned}");
    assert!(turned.contains(" rot=\""), "{turned}");
    let callout = shape_xml(by("Note callout"));
    assert!(callout.contains("prst=\"wedgeRectCallout\""), "{callout}");
    assert!(callout.contains("name=\"adj1\""), "{callout}");
    assert!(by("Hidden shape").hidden);
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());
}

#[test]
fn ods_drawings_save_to_xlsx_and_reopen() {
    for file in ["drawings.ods", "shapes.ods"] {
        let mut wb = Workbook::open(corpus(file)).unwrap();
        let before: Vec<usize> = (0..wb.sheet_names().len())
            .map(|i| wb.drawings(i as u32).len())
            .collect();
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("out.xlsx");
        wb.save_as(&out).unwrap();
        assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
        let bytes = std::fs::read(&out).unwrap();
        assert!(
            sheets_engine::package_problems(&bytes).is_empty(),
            "{:?}",
            sheets_engine::package_problems(&bytes)
        );
        let p = parts(&out);
        assert!(p.keys().any(|k| k.starts_with("xl/drawings/")), "{file}");
        let back = Workbook::open(&out).unwrap();
        let after: Vec<usize> = (0..back.sheet_names().len())
            .map(|i| back.drawings(i as u32).len())
            .collect();
        assert_eq!(before, after, "{file}");
    }
}
