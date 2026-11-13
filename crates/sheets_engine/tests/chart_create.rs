//! Insert Chart in Kantor: a chart from the selection, the Chart panel's
//! changes, move/resize/delete, undo, and the drawing and chart parts a save
//! writes (with `c:f` references and cached values), read back as a chart;
//! parts nobody touched stay byte for byte.

#[path = "support/pivot_check.rs"]
mod pivot_check;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pivot_check::{REL, el, els, parts, targets};
use roxmltree::Document;
use sheets_engine::drawing::{Anchor, DrawingKind, Marker};
use sheets_engine::{CellRange, ChartType, LegendPos, Workbook};

const C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("chart_create");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
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
        ("May", "170", "100"),
        ("Jun", "160", "105"),
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

fn chart_xml(wb: &Workbook) -> String {
    let d = wb.drawings(0);
    let d = d
        .iter()
        .find(|d| matches!(*d.kind, DrawingKind::Chart { .. }))
        .expect("a chart");
    match &*d.kind {
        DrawingKind::Chart { xml, .. } => xml.to_string(),
        _ => unreachable!(),
    }
}

/// Each worksheet's chart parts, following `<drawing>` → drawing part →
/// `c:chart` relationships, with the checks Excel makes first: every link
/// resolves, content types are declared, `cNvPr` ids are unique per drawing.
fn chart_parts(p: &BTreeMap<String, String>, sheet: &str) -> Vec<String> {
    let types = &p["[Content_Types].xml"];
    let doc = Document::parse(&p[sheet]).unwrap();
    let Some(drawing) = el(doc.root_element(), "drawing") else {
        return Vec::new();
    };
    let rid = drawing.attribute((REL, "id")).unwrap();
    let (kind, dpart) = &targets(p, sheet)[rid];
    assert_eq!(kind, "drawing");
    assert!(types.contains(&format!("PartName=\"/{dpart}\"")), "{dpart}");
    let ddoc = Document::parse(&p[dpart]).unwrap();
    let links = targets(p, dpart);
    let mut ids = std::collections::HashSet::new();
    let mut out = Vec::new();
    for n in ddoc
        .descendants()
        .filter(|n| n.tag_name().name() == "cNvPr")
    {
        assert!(
            ids.insert(n.attribute("id").unwrap().to_owned()),
            "cNvPr id twice"
        );
    }
    for c in ddoc
        .descendants()
        .filter(|n| n.tag_name().name() == "chart" && n.tag_name().namespace() == Some(C))
    {
        let (kind, part) = &links[c.attribute((REL, "id")).unwrap()];
        assert_eq!(kind, "chart");
        assert!(types.contains(&format!("PartName=\"/{part}\"")), "{part}");
        out.push(part.clone());
    }
    out
}

#[test]
fn insert_chart_saves_and_reads_back() {
    let mut wb = data_book();
    let data = CellRange::from_corners((0, 0), (6, 2));
    let key = wb.insert_chart(0, data, ChartType::Column).unwrap();
    assert_eq!(wb.drawings(0).len(), 1);
    let d = &wb.drawings(0)[0];
    assert_eq!(d.key, key);
    assert!(d.editable);
    // Beside the data.
    let Anchor::TwoCell { from, .. } = d.anchor else {
        panic!("two-cell anchor")
    };
    assert_eq!((from.col, from.row), (4, 0));
    let xml = chart_xml(&wb);
    assert!(xml.contains("<c:barDir val=\"col\"/>"));
    assert!(xml.contains("<c:f>Sheet1!$B$2:$B$7</c:f>"), "{xml}");
    assert!(xml.contains("<c:f>Sheet1!$A$2:$A$7</c:f>"));
    assert!(xml.contains("<c:f>Sheet1!$C$1</c:f>"));
    assert!(wb.undo().unwrap());
    assert!(wb.drawings(0).is_empty());
    assert!(wb.redo().unwrap());
    assert_eq!(wb.drawings(0).len(), 1);

    let path = scratch("new.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    let charts = chart_parts(&p, "xl/worksheets/sheet1.xml");
    assert_eq!(charts.len(), 1);
    let cdoc = Document::parse(&p[&charts[0]]).unwrap();
    let sers: Vec<_> = cdoc
        .descendants()
        .filter(|n| n.tag_name().name() == "ser")
        .collect();
    assert_eq!(sers.len(), 2);
    let cache = sers[0]
        .descendants()
        .find(|n| n.tag_name().name() == "numCache")
        .unwrap();
    assert_eq!(el(cache, "ptCount").unwrap().attribute("val"), Some("6"));
    assert_eq!(els(cache, "pt").count(), 6);

    // Read back: a chart of the file, reading the same cells.
    let back = Workbook::open(&path).unwrap();
    let d = back.drawings(0);
    assert_eq!(d.len(), 1);
    assert!(matches!(
        *d[0].kind,
        DrawingKind::Chart {
            current_refs: false,
            ..
        }
    ));
    let r = back.chart_reference("Sheet1!$B$2:$B$7").unwrap();
    assert_eq!(r.shown, vec!["120", "135", "150", "110", "170", "160"]);
}

#[test]
fn every_type_and_the_chart_panel() {
    for kind in ChartType::ALL {
        let mut wb = data_book();
        let key = wb
            .insert_chart(0, CellRange::from_corners((0, 0), (6, 2)), kind)
            .unwrap();
        let mut spec = wb.chart_spec(key).unwrap();
        spec.title = "Sales & costs".into();
        spec.legend = LegendPos::Bottom;
        wb.set_chart(key, spec.clone()).unwrap();
        assert_eq!(wb.chart_spec(key), Some(spec));
        let xml = chart_xml(&wb);
        let want = match kind {
            ChartType::Column
            | ChartType::StackedColumn
            | ChartType::Bar
            | ChartType::StackedBar => "c:barChart",
            ChartType::Line | ChartType::StackedLine => "c:lineChart",
            ChartType::Pie => "c:pieChart",
            ChartType::Area | ChartType::StackedArea => "c:areaChart",
            ChartType::Scatter => "c:scatterChart",
            ChartType::Other => unreachable!(),
        };
        assert!(xml.contains(want), "{kind:?}: {xml}");
        assert!(xml.contains("<a:t>Sales &amp; costs</a:t>"));
        assert!(xml.contains("<c:legendPos val=\"b\"/>"));
        Document::parse(&xml).unwrap();
        let path = scratch(&format!("{kind:?}.xlsx"));
        wb.save_as(&path).unwrap();
        assert!(
            wb.save_notes().is_empty(),
            "{kind:?}: {:?}",
            wb.save_notes()
        );
        assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
        assert_eq!(Workbook::open(&path).unwrap().drawings(0).len(), 1);
    }
}

#[test]
fn series_from_rows_and_move_resize_delete() {
    let mut wb = data_book();
    let key = wb
        .insert_chart(0, CellRange::from_corners((0, 0), (6, 2)), ChartType::Line)
        .unwrap();
    let mut spec = wb.chart_spec(key).unwrap();
    assert!(!spec.by_rows);
    spec.by_rows = true;
    wb.set_chart(key, spec).unwrap();
    let xml = chart_xml(&wb);
    assert_eq!(xml.matches("<c:ser>").count(), 6, "one series per month");
    let moved = Anchor::TwoCell {
        from: Marker {
            col: 1,
            col_off: 0,
            row: 10,
            row_off: 0,
        },
        to: Marker {
            col: 7,
            col_off: 0,
            row: 22,
            row_off: 0,
        },
        edit_as: sheets_engine::drawing::EditAs::OneCell,
    };
    wb.place_drawing(key, moved).unwrap();
    assert_eq!(wb.drawings(0)[0].anchor, moved);
    // Rows inserted above move the chart and its data.
    wb.insert_rows(0, 0, 1).unwrap();
    let d = &wb.drawings(0)[0];
    let Anchor::TwoCell { from, .. } = d.anchor else {
        panic!()
    };
    assert_eq!(from.row, 11);
    assert!(
        chart_xml(&wb).contains("Sheet1!$B$3:$C$3"),
        "{}",
        chart_xml(&wb)
    );
    let path = scratch("moved.xlsx");
    wb.save_as(&path).unwrap();
    let p = parts(&path);
    let charts = chart_parts(&p, "xl/worksheets/sheet1.xml");
    assert!(p[&charts[0]].contains("Sheet1!$B$3:$C$3"));
    wb.delete_drawing(key).unwrap();
    assert!(wb.drawings(0).is_empty());
    let path = scratch("deleted.xlsx");
    wb.save_as(&path).unwrap();
    let p = parts(&path);
    assert!(chart_parts(&p, "xl/worksheets/sheet1.xml").is_empty());
    assert!(!p.keys().any(|k| k.starts_with("xl/charts/")));
}

#[test]
fn a_chart_added_beside_a_files_charts() {
    let src = corpus("20_charts.xlsx");
    let mut plain = Workbook::open(&src).unwrap();
    let plain_path = scratch("plain20.xlsx");
    plain.save_as(&plain_path).unwrap();
    let mut wb = Workbook::open(&src).unwrap();
    let before = wb.drawings(1).len();
    wb.set_input(1, 20, 0, "7").unwrap();
    wb.set_input(1, 21, 0, "9").unwrap();
    plain.set_input(1, 20, 0, "7").unwrap();
    plain.set_input(1, 21, 0, "9").unwrap();
    plain.save_as(&plain_path).unwrap();
    wb.insert_chart(1, CellRange::from_corners((20, 0), (21, 0)), ChartType::Pie)
        .unwrap();
    let path = scratch("added20.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let a = parts(&plain_path);
    let b = parts(&path);
    // The other sheet's drawing, every chart of the file: as they were.
    for (name, text) in &a {
        // docProps/core.xml holds the save time, which may tick over between the two saves.
        if name.contains("drawing2") || name == "[Content_Types].xml" || name == "docProps/core.xml"
        {
            continue;
        }
        assert_eq!(Some(text), b.get(name), "{name} changed");
    }
    let charts = chart_parts(&b, "xl/worksheets/sheet2.xml");
    assert_eq!(charts.len(), before + 1);
    // The file's anchor is still first, unchanged.
    let old = &a["xl/drawings/drawing2.xml"];
    let new = &b["xl/drawings/drawing2.xml"];
    let cut = old.rfind("</").unwrap();
    assert!(new.starts_with(&old[..cut]));
    let back = Workbook::open(&path).unwrap();
    assert_eq!(back.drawings(1).len(), before + 1);
}
