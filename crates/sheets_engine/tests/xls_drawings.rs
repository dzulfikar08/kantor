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
    // The radar chart comes in too.
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());
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
        (ChartType::Other, "Radar", "Radar"),
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
    assert_eq!(wb.chart_sheet_name(0).as_deref(), Some("Overview"));
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
    assert_eq!(back.drawings(1).len(), 7);
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

/// EMF and WMF pictures of `.xls` and `.ods` (metafiles.*, from
/// scripts/corpus/gen_metafiles.py) are kept as they are.
#[test]
fn metafile_pictures_are_imported() {
    for name in ["metafiles.xls", "metafiles.ods"] {
        let wb = Workbook::open(corpus(name)).unwrap();
        assert!(
            wb.import_notes().is_empty(),
            "{name}: {:?}",
            wb.import_notes()
        );
        let pics: Vec<_> = wb
            .drawings(0)
            .iter()
            .filter_map(|d| match &*d.kind {
                DrawingKind::Picture { data, part, .. } => Some((part.clone(), data.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(pics.len(), 2, "{name}");
        for (part, data) in pics {
            assert!(kantor_metafile::kind(&data).is_some(), "{name}: {part}");
            assert!(
                kantor_metafile::to_png(&data, 256).is_some(),
                "{name}: {part}"
            );
        }
    }
}

/// Shapes and text boxes of an `.xls` (shapes.xls: 23_shapes.xlsx as
/// LibreOffice saves it in Excel 97 format, from scripts/corpus/gen_xls.py).
#[test]
fn xls_shapes_and_text_boxes() {
    let wb = Workbook::open(corpus("shapes.xls")).unwrap();
    let shapes = wb.drawings(0);
    let xml = |d: &Drawing| match &*d.kind {
        DrawingKind::Shape { xml, .. } => xml.to_string(),
        k => panic!("not a shape: {k:?}"),
    };
    let names: Vec<&str> = shapes.iter().map(|d| d.name.as_str()).collect();
    for want in [
        "Revenue box",
        "Gradient oval",
        "Dashed frame",
        "Arrow line",
        "Turned arrow",
        "Notes box",
        "Note callout",
        "Hidden shape",
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    let by = |n: &str| shapes.iter().find(|d| d.name == n).unwrap();
    let revenue = xml(by("Revenue box"));
    assert!(revenue.contains("prst=\"rect\""), "{revenue}");
    assert!(revenue.contains("<a:t>Revenue</a:t>"), "{revenue}");
    // LibreOffice writes its shapes as freeforms: the oval's path, curved.
    let oval = xml(by("Gradient oval"));
    assert!(
        oval.contains("<a:custGeom>") && oval.contains("<a:cubicBezTo>"),
        "{oval}"
    );
    assert!(oval.contains("<a:t>Gradient</a:t>"), "{oval}");
    let frame = xml(by("Dashed frame"));
    assert!(frame.contains("<a:prstDash val=\"dash\"/>"), "{frame}");
    let line = xml(by("Arrow line"));
    assert!(line.starts_with("<xdr:cxnSp"), "{line}");
    let turned = xml(by("Turned arrow"));
    assert!(turned.contains(" rot=\""), "{turned}");
    assert!(by("Hidden shape").hidden);
    // The group comes in as a group, its shapes in its child space.
    let group = shapes
        .iter()
        .map(xml)
        .find(|x| x.starts_with("<xdr:grpSp"))
        .expect("a group");
    assert!(group.contains("<a:chOff "), "{group}");
    assert!(group.matches("<xdr:sp ").count() >= 2, "{group}");
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());
}

/// The radar chart of drawings.xls and drawings.ods is brought in as a
/// radar chart.
#[test]
fn radar_charts_are_imported() {
    for name in ["drawings.xls", "drawings.ods"] {
        let wb = Workbook::open(corpus(name)).unwrap();
        let radar = (0..wb.sheet_names().len() as u32)
            .flat_map(|s| wb.drawings(s))
            .filter_map(|d| match &*d.kind {
                DrawingKind::Chart { xml, .. } => Some(xml.to_string()),
                _ => None,
            })
            .find(|x| x.contains("<c:radarChart>"));
        let xml = radar.unwrap_or_else(|| panic!("{name}: no radar chart"));
        assert!(xml.contains("<c:radarStyle "), "{xml}");
        assert!(xml.matches("<c:ser>").count() == 2, "{xml}");
    }
}

/// chartopts.xls: data label number formats, a date axis with its bounds,
/// and combinations with scatter and pie plots (scripts/corpus/gen_xls.py).
#[test]
fn chart_options_of_xls() {
    let wb = Workbook::open(corpus("chartopts.xls")).unwrap();
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());
    let charts: Vec<String> = wb
        .drawings(0)
        .iter()
        .map(|d| chart_xml(d).to_owned())
        .collect();
    assert_eq!(charts.len(), 5, "every chart brought in");
    let with = |title: &str| {
        charts
            .iter()
            .find(|x| x.contains(&format!("<a:t>{title}</a:t>")))
            .unwrap_or_else(|| panic!("no chart {title}"))
    };
    let labels = with("Labels");
    assert!(
        labels.contains("<c:dLbls><c:numFmt formatCode=\"0.0%\" sourceLinked=\"0\"/>"),
        "{labels}"
    );
    let dates = with("Dates");
    for want in [
        "<c:dateAx>",
        "<c:max val=\"46080\"/><c:min val=\"46030\"/>",
        "<c:majorUnit val=\"14\"/><c:majorTimeUnit val=\"days\"/>",
    ] {
        assert!(dates.contains(want), "{want} in {dates}");
    }
    let line = with("Column and line");
    assert!(line.contains("<c:barChart>") && line.contains("<c:lineChart>"));
    let scatter = with("Column and scatter");
    assert!(
        scatter.contains("<c:barChart>") && scatter.contains("<c:scatterChart>"),
        "{scatter}"
    );
    // The scatter plot on a hidden X axis spanning the four categories.
    assert!(
        scatter.contains("<c:axId val=\"500000003\"/><c:scaling><c:orientation val=\"minMax\"/><c:max val=\"4.5\"/><c:min val=\"0.5\"/></c:scaling><c:delete val=\"1\"/>"),
        "{scatter}"
    );
    let pie = with("Column and pie");
    assert!(
        pie.contains("<c:barChart>") && pie.contains("<c:pieChart>"),
        "{pie}"
    );
}

/// odsdraw.ods (scripts/corpus/gen_ods_drawings.py): freeform shapes,
/// groups kept as groups, callouts, linked shapes, form controls and an
/// embedded object's replacement picture.
#[test]
fn ods_freeforms_groups_and_controls() {
    let wb = Workbook::open(corpus("odsdraw.ods")).unwrap();
    assert!(wb.import_notes().is_empty(), "{:?}", wb.import_notes());
    let all = wb.drawings(0);
    let xml = |name: &str| -> String {
        let d = all.iter().find(|d| d.name == name).unwrap_or_else(|| {
            panic!(
                "no {name} in {:?}",
                all.iter().map(|d| &d.name).collect::<Vec<_>>()
            )
        });
        match &*d.kind {
            DrawingKind::Shape { xml, .. } => xml.to_string(),
            DrawingKind::Picture { .. } => "picture".into(),
            k => panic!("{name}: {k:?}"),
        }
    };
    let wave = xml("Wave");
    assert!(
        wave.contains("<a:custGeom>") && wave.contains("<a:cubicBezTo>"),
        "{wave}"
    );
    assert!(wave.contains("<a:path w=\"4000\" h=\"2000\">"), "{wave}");
    assert!(xml("Arrow polygon").contains("<a:close/>"));
    let zig = xml("Zigzag");
    assert!(
        zig.contains("fill=\"none\"") && !zig.contains("<a:close/>"),
        "{zig}"
    );
    assert_eq!(xml("Hexagon").matches("<a:lnTo>").count(), 5);
    assert_eq!(xml("Star").matches("<a:lnTo>").count(), 9);
    // Arcs are curves, not lines to their end.
    let arch = xml("Arch");
    assert!(
        arch.matches("<a:cubicBezTo>").count() == 2 && !arch.contains("<a:lnTo>"),
        "{arch}"
    );
    // Custom shapes without a preset, from their enhanced path.
    let pie = xml("Pie");
    assert!(
        pie.contains("<a:moveTo><a:pt x=\"10800\" y=\"10800\"/>") && pie.contains("<a:cubicBezTo>"),
        "{pie}"
    );
    // Counter-clockwise from 0° to 90°: a quarter, ending at the top.
    assert_eq!(pie.matches("<a:cubicBezTo>").count(), 1, "{pie}");
    assert!(
        pie.contains("<a:pt x=\"10800\" y=\"0\"/></a:cubicBezTo>"),
        "{pie}"
    );
    let ring = xml("Ring");
    assert_eq!(ring.matches("<a:path ").count(), 2, "{ring}");
    assert_eq!(ring.matches("<a:cubicBezTo>").count(), 8, "{ring}");
    assert!(ring.contains("fill=\"none\""), "{ring}");
    let rounded = xml("Rounded");
    assert!(
        rounded.matches("<a:cubicBezTo>").count() == 3 && rounded.contains("<a:quadBezTo>"),
        "{rounded}"
    );
    let pair = xml("Pair");
    assert!(pair.starts_with("<xdr:grpSp"), "{pair}");
    assert_eq!(
        pair.matches("<xdr:grpSp>").count() + 1,
        2,
        "a group inside: {pair}"
    );
    assert!(
        pair.contains("name=\"Pair circle\"") && pair.contains("<xdr:cxnSp"),
        "{pair}"
    );
    assert!(xml("Callout").contains("wedgeRectCallout"));
    assert!(xml("Linked box").contains("<a:t>Link</a:t>"));
    assert!(xml("Run button").contains("<a:t>Run</a:t>"));
    assert!(xml("Agree box").contains("\u{2611} Agree"));
    assert!(xml("Pick list").contains("Beta"));
    assert!(xml("Field box").contains("typed"));
    assert!(xml("Options frame").contains("Options"));
    assert_eq!(xml("Embedded"), "picture");
}

/// odsdraw.ods sets no column widths or row heights: its 2 cm × 2 cm
/// shapes are placed on the sheet's own default cells (Excel px: 64 by 20),
/// so they stay square on screen and in print.
#[test]
fn ods_shapes_keep_their_size_on_default_cells() {
    let wb = Workbook::open(corpus("odsdraw.ods")).unwrap();
    let col = wb.column_width(0, 1000);
    let row = wb.default_row_points(0) * 96.0 / 72.0;
    let all = wb.drawings(0);
    for name in ["Pie", "Ring"] {
        let d = all.iter().find(|d| d.name == name).unwrap();
        let Anchor::TwoCell { from, to, .. } = d.anchor else {
            panic!("{name}: {:?}", d.anchor);
        };
        let off = |emu: i64| emu as f64 / EMU_PER_PX;
        let w = f64::from(to.col - from.col) * col + off(to.col_off) - off(from.col_off);
        let h = f64::from(to.row - from.row) * row + off(to.row_off) - off(from.row_off);
        let cm2 = 2.0 * 96.0 / 2.54;
        assert!(
            (w - cm2).abs() < 1.0 && (h - cm2).abs() < 1.0,
            "{name}: {w} × {h}"
        );
    }
}
