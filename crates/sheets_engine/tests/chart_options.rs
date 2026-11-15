//! Data labels and axis options in the Chart panel (Excel's Format Data
//! Labels / Format Axis): on a file's chart (changed in place, everything
//! else byte for byte) and on a chart made in Kantor; save, undo, save →
//! undo → save, read back.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pivot_check::{REL, parts, targets};
use roxmltree::Document;
use sheets_engine::drawing::{DrawingKey, DrawingKind};
use sheets_engine::{
    AxisOptions, CellRange, ChartType, DataLabels, DisplayUnits, LabelPos, Workbook,
};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("chart_options");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn chart_key(wb: &Workbook, label: &str) -> DrawingKey {
    wb.drawings(0)
        .iter()
        .filter(|d| matches!(*d.kind, DrawingKind::Chart { .. }))
        .find(|d| wb.chart_info(d.key).is_some_and(|i| i.kind_label == label))
        .unwrap_or_else(|| panic!("no {label} chart"))
        .key
}

fn shown_xml(wb: &Workbook, key: DrawingKey) -> String {
    let d = wb.drawings(0);
    let d = d.iter().find(|d| d.key == key).unwrap();
    match &*d.kind {
        DrawingKind::Chart { xml, .. } => xml.to_string(),
        _ => unreachable!(),
    }
}

/// Sheet 1's chart parts in drawing order.
fn saved_charts(p: &BTreeMap<String, String>) -> Vec<String> {
    let sheet = "xl/worksheets/sheet1.xml";
    let doc = Document::parse(&p[sheet]).unwrap();
    let rid = doc
        .descendants()
        .find(|n| n.tag_name().name() == "drawing")
        .unwrap()
        .attribute((REL, "id"))
        .unwrap();
    let (_, dpart) = &targets(p, sheet)[rid];
    let links = targets(p, dpart);
    let ddoc = Document::parse(&p[dpart]).unwrap();
    ddoc.descendants()
        .filter(|n| n.tag_name().name() == "chart")
        .map(|c| links[c.attribute((REL, "id")).unwrap()].1.clone())
        .collect()
}

fn labels(pos: Option<LabelPos>) -> DataLabels {
    DataLabels {
        value: true,
        category: true,
        series: false,
        percent: false,
        pos,
        num_fmt: Some("#,##0.0".into()),
    }
}

fn axis() -> AxisOptions {
    AxisOptions {
        min: Some(10.0),
        max: Some(1000.0),
        major: None,
        log_base: Some(10.0),
        reverse: false,
        num_fmt: Some("0.0".into()),
        units: DisplayUnits::Thousands,
    }
}

#[test]
fn file_chart_labels_and_axis_save_in_place() {
    let src = corpus("20_charts.xlsx");
    let mut wb = Workbook::open(&src).unwrap();
    let key = chart_key(&wb, "Column");
    let before = wb.chart_info(key).unwrap();
    assert!(before.y_value && !before.x_value, "{before:?}");
    assert!(before.label_positions.contains(&LabelPos::OutsideEnd));
    assert!(!before.percent_labels);
    let n = before.series.len();
    let mut spec = before.spec.clone();
    spec.labels = vec![labels(Some(LabelPos::OutsideEnd)); n];
    spec.labels[0].percent = true; // not a pie: ignored
    spec.y_axis = axis();
    spec.x_axis.reverse = true;
    wb.set_chart(key, spec.clone()).unwrap();
    let now = wb.chart_info(key).unwrap();
    assert_eq!(now.spec.labels[1], spec.labels[1]);
    assert!(!now.spec.labels[0].percent);
    assert_eq!(now.spec.y_axis, spec.y_axis);
    assert!(now.spec.x_axis.reverse);
    let xml = shown_xml(&wb, key);
    for want in [
        "<c:dLblPos val=\"outEnd\"/>",
        "<c:showVal val=\"1\"/>",
        "<c:showCatName val=\"1\"/>",
        "formatCode=\"#,##0.0\" sourceLinked=\"0\"",
        "<c:logBase val=\"10\"/>",
        "<c:max val=\"1000\"/>",
        "<c:min val=\"10\"/>",
        "<c:orientation val=\"maxMin\"/>",
        "<c:builtInUnit val=\"thousands\"/>",
    ] {
        assert!(xml.contains(want), "{want} in {xml}");
    }
    // The part still parses in schema order for the renderer.
    let doc = Document::parse(&xml).unwrap();
    let val_ax = doc
        .descendants()
        .find(|n| n.tag_name().name() == "valAx")
        .unwrap();
    let names: Vec<&str> = val_ax
        .children()
        .filter(|c| c.is_element())
        .map(|c| c.tag_name().name())
        .collect();
    let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
    assert!(pos("crossAx") < pos("dispUnits"), "{names:?}");
    assert!(pos("axPos") < pos("numFmt"), "{names:?}");

    let plain = scratch("plain.xlsx");
    Workbook::open(&src).unwrap().save_as(&plain).unwrap();
    let path = scratch("options.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let a = parts(&plain);
    let b = parts(&path);
    let changed: Vec<&String> = a
        .keys()
        .filter(|k| *k != "docProps/core.xml" && a.get(*k) != b.get(*k))
        .collect();
    assert_eq!(changed.len(), 1, "only the one chart part: {changed:?}");
    // Reopened: the same panel state.
    let back = Workbook::open(&path).unwrap();
    let info = back.chart_info(chart_key(&back, "Column")).unwrap();
    assert_eq!(info.spec.labels, now.spec.labels);
    assert_eq!(info.spec.y_axis, now.spec.y_axis);
    assert!(info.spec.x_axis.reverse);

    // Labels off again: the series' labels go.
    let mut off = now.spec.clone();
    off.labels = vec![DataLabels::default(); n];
    off.y_axis = AxisOptions::default();
    wb.set_chart(key, off).unwrap();
    let xml = shown_xml(&wb, key);
    assert!(!xml.contains("<c:showVal val=\"1\"/>"), "{xml}");
    assert!(!xml.contains("logBase"), "{xml}");
    assert!(!xml.contains("dispUnits"), "{xml}");

    assert!(wb.undo().unwrap());
    assert!(wb.undo().unwrap());
    assert_eq!(wb.chart_info(key).unwrap().spec, before.spec);
}

#[test]
fn save_undo_save_restores_the_part() {
    let src = corpus("20_charts.xlsx");
    let original = {
        let p = scratch("sus-plain.xlsx");
        Workbook::open(&src).unwrap().save_as(&p).unwrap();
        parts(&p)
    };
    let mut wb = Workbook::open(&src).unwrap();
    let key = chart_key(&wb, "Column");
    let mut spec = wb.chart_info(key).unwrap().spec;
    spec.labels = vec![labels(None)];
    spec.y_axis.major = Some(25.0);
    wb.set_chart(key, spec).unwrap();
    let path = scratch("sus.xlsx");
    wb.save_as(&path).unwrap();
    let p = parts(&path);
    assert!(
        saved_charts(&p)
            .iter()
            .any(|c| p[c].contains("<c:majorUnit val=\"25\"/>"))
    );
    assert!(wb.undo().unwrap());
    wb.save_as(&path).unwrap();
    let p = parts(&path);
    for c in saved_charts(&p) {
        assert_eq!(p.get(&c), original.get(&c), "{c} back as it was");
    }
    for (name, text) in &original {
        if name == "docProps/core.xml" {
            continue;
        }
        assert_eq!(Some(text), p.get(name), "{name} changed");
    }
}

#[test]
fn stacked_columns_have_no_outside_end() {
    let src = corpus("20_charts.xlsx");
    let mut wb = Workbook::open(&src).unwrap();
    let key = chart_key(&wb, "Column");
    let mut spec = wb.chart_info(key).unwrap().spec;
    spec.labels = vec![labels(Some(LabelPos::OutsideEnd))];
    wb.set_chart(key, spec.clone()).unwrap();
    spec = wb.chart_info(key).unwrap().spec;
    spec.kind = ChartType::StackedColumn;
    wb.set_chart(key, spec).unwrap();
    let info = wb.chart_info(key).unwrap();
    assert!(!info.label_positions.contains(&LabelPos::OutsideEnd));
    assert_eq!(info.spec.labels[0].pos, None);
    assert!(info.spec.labels[0].value);
    assert!(!shown_xml(&wb, key).contains("outEnd"));
}

/// Month / Sales / Costs on A1:C7.
fn data_book() -> Workbook {
    let mut wb = Workbook::new_empty().unwrap();
    let rows = [
        ("Month", "Sales", "Costs"),
        ("Jan", "120", "80"),
        ("Feb", "135", "90"),
        ("Mar", "150", "95"),
        ("Apr", "110", "70"),
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

#[test]
fn new_chart_labels_and_axis_read_back() {
    let mut wb = data_book();
    let key = wb
        .insert_chart(0, CellRange::from_corners((0, 0), (4, 2)), ChartType::Pie)
        .unwrap();
    let info = wb.chart_info(key).unwrap();
    assert!(info.percent_labels);
    assert!(info.label_positions.contains(&LabelPos::BestFit));
    let mut spec = info.spec;
    spec.labels = vec![DataLabels {
        percent: true,
        category: true,
        pos: Some(LabelPos::BestFit),
        ..DataLabels::default()
    }];
    wb.set_chart(key, spec.clone()).unwrap();
    assert_eq!(wb.chart_info(key).unwrap().spec.labels, spec.labels);
    // A column chart: Best Fit isn't one of its places.
    spec.kind = ChartType::Column;
    spec.y_axis = AxisOptions {
        min: Some(50.0),
        major: Some(20.0),
        units: DisplayUnits::Hundreds,
        ..AxisOptions::default()
    };
    wb.set_chart(key, spec).unwrap();
    let info = wb.chart_info(key).unwrap();
    assert_eq!(info.spec.labels[0].pos, None);
    assert!(!info.spec.labels[0].percent);
    let path = scratch("new.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let back = Workbook::open(&path).unwrap();
    let k = back
        .drawings(0)
        .iter()
        .find(|d| matches!(*d.kind, DrawingKind::Chart { .. }))
        .unwrap()
        .key;
    let got = back.chart_info(k).unwrap().spec;
    assert_eq!(got.y_axis.min, Some(50.0));
    assert_eq!(got.y_axis.major, Some(20.0));
    assert_eq!(got.y_axis.units, DisplayUnits::Hundreds);
    assert!(got.labels[0].category);
}
