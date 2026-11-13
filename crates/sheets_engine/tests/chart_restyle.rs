//! Restyling charts that came with the file (the Chart panel on a file's
//! chart): the part is changed in place, everything else in it stays byte
//! for byte, the change saves, undoes, follows inserted rows, and a save →
//! undo → save leaves the next file as the first one was.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pivot_check::{REL, parts, targets};
use roxmltree::Document;
use sheets_engine::drawing::{DrawingKey, DrawingKind};
use sheets_engine::{CellRange, ChartType, LegendPos, SeriesColor, Workbook};

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("chart_restyle");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

/// The key of the first chart on sheet 0 whose type is `label`.
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

/// The saved chart part with this title.
fn part_with<'a>(p: &'a BTreeMap<String, String>, needle: &str) -> Option<&'a String> {
    saved_charts(p).into_iter().find_map(|c| {
        let x = &p[&c];
        x.contains(needle).then_some(x)
    })
}

#[test]
fn file_charts_show_in_the_panel() {
    let wb = Workbook::open(corpus("20_charts.xlsx")).unwrap();
    let mut labels = Vec::new();
    for d in wb.drawings(0) {
        if let Some(i) = wb.chart_info(d.key) {
            assert!(i.from_file);
            labels.push(i.kind_label.clone());
        }
    }
    assert!(labels.contains(&"Column".to_owned()), "{labels:?}");
    assert!(labels.contains(&"Combo".to_owned()), "{labels:?}");
    let col = chart_key(&wb, "Column");
    let info = wb.chart_info(col).unwrap();
    assert!(!info.fixed);
    assert!(!info.data_unknown, "{info:?}");
    assert_eq!(info.spec.kind, ChartType::Column);
    assert!(info.has_axes);
    let combo = wb.chart_info(chart_key(&wb, "Combo")).unwrap();
    assert!(combo.fixed);
}

#[test]
fn restyle_saves_in_place_and_keeps_the_rest() {
    let src = corpus("20_charts.xlsx");
    let mut wb = Workbook::open(&src).unwrap();
    let key = chart_key(&wb, "Column");
    let before = wb.chart_info(key).unwrap();
    let mut spec = before.spec.clone();
    spec.kind = ChartType::Line;
    spec.title = "Restyled & saved".into();
    spec.legend = LegendPos::Bottom;
    spec.x_title = "Month".into();
    spec.y_title = "Units".into();
    spec.colors = vec![SeriesColor::Rgb([0x12, 0x34, 0x56]); before.series.len()];
    let anchor = |wb: &Workbook| wb.drawings(0).iter().find(|d| d.key == key).unwrap().anchor;
    let placed = anchor(&wb);
    wb.set_chart(key, spec.clone()).unwrap();
    assert_eq!(anchor(&wb), placed, "restyling doesn't move the chart");
    let after = wb.chart_info(key).unwrap();
    assert_eq!(after.spec, spec);
    let xml = shown_xml(&wb, key);
    assert!(xml.contains("lineChart"), "{xml}");
    // The renderer can draw it.
    assert!(matches!(
        &*wb.drawings(0).iter().find(|d| d.key == key).unwrap().kind,
        DrawingKind::Chart {
            current_refs: true,
            ..
        }
    ));

    let plain = scratch("plain.xlsx");
    Workbook::open(&src).unwrap().save_as(&plain).unwrap();
    let path = scratch("restyled.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let a = parts(&plain);
    let b = parts(&path);
    // (docProps/core.xml has the save time.)
    let changed: Vec<&String> = a
        .keys()
        .filter(|k| *k != "docProps/core.xml" && a.get(*k) != b.get(*k))
        .collect();
    assert_eq!(
        changed.len(),
        1,
        "only the one chart part changes: {changed:?}"
    );
    assert!(changed[0].starts_with("xl/charts/"));
    let new = &b[changed[0]];
    assert!(new.contains("<a:t>Restyled &amp; saved</a:t>"));
    assert!(new.contains("123456"));
    // What the original part had beyond the panel's elements is still there.
    let old = &a[changed[0]];
    for kept in ["<c:txPr>", "<c:plotVisOnly"] {
        if old.contains(kept) {
            assert!(new.contains(kept), "lost {kept}");
        }
    }

    // Reopened: the same panel state, drawn the same way.
    let back = Workbook::open(&path).unwrap();
    let k2 = chart_key(&back, "Line");
    let info = back.chart_info(k2).unwrap();
    assert_eq!(info.spec.title, spec.title);
    assert_eq!(info.spec.legend, LegendPos::Bottom);
    assert_eq!(
        (info.spec.x_title.as_str(), info.spec.y_title.as_str()),
        ("Month", "Units")
    );
    assert_eq!(info.spec.colors, spec.colors);

    // Undo: back to the file's chart.
    assert!(wb.undo().unwrap());
    assert_eq!(wb.chart_info(key).unwrap().spec, before.spec);
}

#[test]
fn save_undo_save_restores_the_part() {
    let src = corpus("20_charts.xlsx");
    let mut wb = Workbook::open(&src).unwrap();
    let key = chart_key(&wb, "Column");
    let original = {
        let p = scratch("sus-plain.xlsx");
        Workbook::open(&src).unwrap().save_as(&p).unwrap();
        parts(&p)
    };
    let mut spec = wb.chart_info(key).unwrap().spec;
    spec.title = "Saved once".into();
    wb.set_chart(key, spec).unwrap();
    let path = scratch("sus.xlsx");
    wb.save_as(&path).unwrap();
    assert!(part_with(&parts(&path), "Saved once").is_some());
    assert!(wb.undo().unwrap());
    wb.save_as(&path).unwrap();
    let p = parts(&path);
    assert!(
        part_with(&p, "Saved once").is_none(),
        "the undone title is gone"
    );
    for c in saved_charts(&p) {
        assert_eq!(p.get(&c), original.get(&c), "{c} back as it was");
    }
    // Saving twice without changes keeps the edit once.
    assert!(wb.redo().unwrap());
    wb.save_as(&path).unwrap();
    wb.save_as(&path).unwrap();
    let p = parts(&path);
    assert_eq!(saved_charts(&p).len(), saved_charts(&original).len());
    assert!(part_with(&p, "Saved once").is_some());
}

#[test]
fn data_range_and_inserted_rows() {
    let src = corpus("20_charts.xlsx");
    let mut wb = Workbook::open(&src).unwrap();
    let key = chart_key(&wb, "Column");
    let info = wb.chart_info(key).unwrap();
    let mut spec = info.spec.clone();
    // Half the months.
    let d = spec.data;
    spec.data = CellRange::from_corners((d.top, d.left), (d.top + 6, d.right));
    wb.set_chart(key, spec.clone()).unwrap();
    let now = wb.chart_info(key).unwrap();
    assert_eq!(now.spec.data, spec.data);
    assert!(!now.data_unknown);
    let xml = shown_xml(&wb, key);
    let last = d.top + 7; // 1-based last row of the new range
    assert!(xml.contains(&format!("$B${last}")), "{xml}");
    // Rows inserted above: the edited chart's references follow.
    wb.insert_rows(0, 0, 2).unwrap();
    let moved = wb.chart_info(key).unwrap();
    assert_eq!(moved.spec.data.top, spec.data.top + 2);
    let path = scratch("rows.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    let p = parts(&path);
    assert!(part_with(&p, &format!("$B${}", last + 2)).is_some());
    let back = Workbook::open(&path).unwrap();
    assert_eq!(
        back.chart_info(chart_key(&back, "Column"))
            .unwrap()
            .spec
            .data,
        moved.spec.data
    );
}

#[test]
fn every_type_from_a_file_chart_draws_and_saves() {
    let src = corpus("20_charts.xlsx");
    for kind in ChartType::ALL {
        let mut wb = Workbook::open(&src).unwrap();
        let key = chart_key(&wb, "Column");
        let mut spec = wb.chart_info(key).unwrap().spec;
        spec.kind = kind;
        wb.set_chart(key, spec).unwrap();
        assert_eq!(wb.chart_info(key).unwrap().spec.kind, kind);
        let path = scratch(&format!("type-{kind:?}.xlsx"));
        wb.save_as(&path).unwrap();
        assert!(
            wb.save_notes().is_empty(),
            "{kind:?}: {:?}",
            wb.save_notes()
        );
        assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
        let back = Workbook::open(&path).unwrap();
        assert_eq!(back.drawings(0).len(), wb.drawings(0).len());
    }
}

#[test]
fn combo_charts_take_titles_but_not_types() {
    let mut wb = Workbook::open(corpus("20_charts.xlsx")).unwrap();
    let key = chart_key(&wb, "Combo");
    let mut spec = wb.chart_info(key).unwrap().spec;
    spec.kind = ChartType::Pie;
    assert!(wb.set_chart(key, spec.clone()).is_err());
    spec.kind = ChartType::Other;
    spec.title = "Combo title".into();
    wb.set_chart(key, spec).unwrap();
    assert_eq!(wb.chart_info(key).unwrap().spec.title, "Combo title");
}
