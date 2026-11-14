//! Pictures and charts of Excel 97-2003 imports, against
//! `tests/corpus/xls/drawings.xls` (made by `scripts/corpus/gen_xls.py`).

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use sheets_engine::drawing::{Anchor, Drawing, DrawingKind, EMU_PER_PX, EditAs, Tab};
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

fn text(parts: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(
        parts
            .get(name)
            .unwrap_or_else(|| panic!("no {name}"))
            .clone(),
    )
    .unwrap()
}

fn chart_xml(d: &Drawing) -> &str {
    match &*d.kind {
        DrawingKind::Chart { xml, .. } => xml,
        k => panic!("not a chart: {k:?}"),
    }
}

/// (col, row) of a corner.
type Cell = (u32, u32);

/// A two-cell anchor's corners and its end's offsets in px.
fn corners(a: &Anchor) -> (Cell, Cell, (f64, f64), EditAs) {
    let Anchor::TwoCell { from, to, edit_as } = *a else {
        panic!("{a:?}")
    };
    let px = |emu: i64| (emu as f64 / EMU_PER_PX).round();
    (
        (from.col, from.row),
        (to.col, to.row),
        (px(to.col_off), px(to.row_off)),
        edit_as,
    )
}

fn check_pictures(wb: &Workbook) {
    let pictures: Vec<Drawing> = wb
        .drawings(0)
        .into_iter()
        .filter(|d| matches!(&*d.kind, DrawingKind::Picture { .. }))
        .collect();
    assert_eq!(pictures.len(), 2);
    let DrawingKind::Picture { data, part, .. } = &*pictures[0].kind else {
        unreachable!()
    };
    assert!(data.starts_with(b"\x89PNG"), "{part}");
    assert!(part.ends_with(".png"), "{part}");
    // 120 x 80 px at A8: across A to B, down rows 8-11 (20 px each).
    let (from, to, off, edit_as) = corners(&pictures[0].anchor);
    assert_eq!((from, to), ((0, 7), (1, 10)));
    assert_eq!(off.1, 20.0, "{off:?}");
    assert_eq!(edit_as, EditAs::TwoCell);
    let DrawingKind::Picture { data, part, .. } = &*pictures[1].kind else {
        unreachable!()
    };
    assert!(data.starts_with(&[0xFF, 0xD8]), "{part}");
    let (from, to, _, _) = corners(&pictures[1].anchor);
    assert_eq!((from, to), ((2, 7), (3, 9)));
}

#[test]
fn pictures_and_charts_are_imported() {
    let wb = Workbook::open(corpus("drawings.xls")).unwrap();
    assert_eq!(
        wb.import_notes(),
        ["1 chart not imported (radar, surface, bubble, bar of pie or combination charts)"]
    );
    assert_eq!(wb.sheet_names(), ["Data", "Charts"]);
    assert_eq!(wb.tabs(), [Tab::Sheet(0), Tab::Sheet(1), Tab::Chart(0)]);
    check_pictures(&wb);

    // The column chart on Data, from E2.
    let on_data = wb.drawings(0);
    let col = &on_data[0];
    let (from, to, _, _) = corners(&col.anchor);
    assert_eq!((from, to), ((4, 1), (12, 15)));
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
    assert!(xml.contains("<c:f>Data!$C$1</c:f>"), "{xml}");
    assert!(xml.contains("<c:v>18</c:v>"), "{xml}");
    // The chart reads its cells live.
    let data = wb.chart_reference("Data!$B$2:$B$5").unwrap();
    assert_eq!(data.shown, ["10", "14", "12", "18"]);

    // The Charts sheet: one of each type Kantor draws (the radar is left
    // out).
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
    assert!(bar.contains("<c:overlap val=\"100\"/>"), "{bar}");
    let scatter = chart_xml(&charts[5]);
    assert!(
        scatter.contains("<c:xVal><c:numRef><c:f>Data!$B$2:$B$5</c:f>"),
        "{scatter}"
    );
    assert!(
        scatter.contains("<c:yVal><c:numRef><c:f>Data!$C$2:$C$5</c:f>"),
        "{scatter}"
    );
    assert!(chart_xml(&charts[4]).contains("<c:doughnutChart>"));

    // The chart sheet.
    assert_eq!(wb.chart_sheet_name(0), Some("Overview"));
    let sheet = wb.chart_sheet_drawings(0);
    assert_eq!(sheet.len(), 1);
    assert!(matches!(sheet[0].anchor, Anchor::Absolute { .. }));
    assert!(chart_xml(&sheet[0]).contains("<a:t>Overview</a:t>"));
}

#[test]
fn imported_drawings_save_to_xlsx_and_reopen() {
    let mut wb = Workbook::open(corpus("drawings.xls")).unwrap();
    // A row inserted above moves them.
    wb.insert_rows(0, 0, 1).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("drawings.xlsx");
    wb.save_as(&out).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let bytes = std::fs::read(&out).unwrap();
    assert!(
        sheets_engine::package_problems(&bytes).is_empty(),
        "{:?}",
        sheets_engine::package_problems(&bytes)
    );
    let p = parts(&out);
    assert!(p.contains_key("xl/media/image1.png"));
    assert!(p.contains_key("xl/media/image2.jpeg"));
    let book = text(&p, "xl/workbook.xml");
    assert!(book.contains("name=\"Overview\""), "{book}");
    assert!(
        p.keys().any(|k| k.starts_with("xl/chartsheets/")),
        "{:?}",
        p.keys()
    );
    let sheet1 = text(&p, "xl/worksheets/sheet1.xml");
    assert!(sheet1.contains("<drawing r:id="), "{sheet1}");
    // Series references followed the inserted row.
    let charts: String = p
        .iter()
        .filter(|(k, _)| k.starts_with("xl/charts/"))
        .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
        .collect();
    assert!(charts.contains("<c:f>Data!$B$3:$B$6</c:f>"), "{charts}");
    assert!(!charts.contains("<c:f>Data!$B$2:$B$5</c:f>"), "{charts}");

    let mut back = Workbook::open(&out).unwrap();
    assert_eq!(back.tabs(), [Tab::Sheet(0), Tab::Sheet(1), Tab::Chart(0)]);
    let on_data = back.drawings(0);
    assert_eq!(on_data.len(), 3);
    let (from, _, _, _) = corners(&on_data[0].anchor);
    assert_eq!(from, (4, 2));
    let info = back.chart_info(on_data[0].key).unwrap();
    assert_eq!(info.spec.title, "Sales by quarter");
    assert_eq!(info.spec.data, CellRange::from_corners((1, 0), (5, 2)));
    assert_eq!(back.drawings(1).len(), 6);
    assert_eq!(back.chart_sheet_drawings(0).len(), 1);

    // Saved again: the same parts.
    let again = dir.path().join("again.xlsx");
    back.save_as(&again).unwrap();
    let q = parts(&again);
    for (name, data) in &p {
        // docProps/core.xml holds the save time.
        if name == "docProps/core.xml" {
            continue;
        }
        if name.starts_with("xl/charts/")
            || name.starts_with("xl/drawings/")
            || name.starts_with("xl/media/")
        {
            assert_eq!(q.get(name), Some(data), "{name}");
        }
    }
}

#[test]
fn imported_pictures_move_and_delete_like_xlsx_ones() {
    let mut wb = Workbook::open(corpus("drawings.xls")).unwrap();
    let pic = wb.drawings(0)[1].clone();
    assert!(pic.editable);
    wb.delete_drawing(pic.key).unwrap();
    assert_eq!(wb.drawings(0).len(), 2);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("deleted.xlsx");
    wb.save_as(&out).unwrap();
    let back = Workbook::open(&out).unwrap();
    let pictures = back
        .drawings(0)
        .into_iter()
        .filter(|d| matches!(&*d.kind, DrawingKind::Picture { .. }))
        .count();
    assert_eq!(pictures, 1);
}

#[test]
fn files_without_drawings_have_none() {
    let wb = Workbook::open(corpus("features.xls")).unwrap();
    assert!(!wb.has_drawings());
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());
}
