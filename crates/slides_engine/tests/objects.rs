//! Editing tables and chart data in tests/corpus/pptx/deck.pptx (generated
//! by scripts/corpus/gen_pptx.py): open, edit, undo, save, reopen.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _};
use std::path::PathBuf;

use slides_engine::{ChartData, Presentation, Rgb, ShapeKind, TableOp};

fn corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/pptx/deck.pptx")
}

fn parts(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        out.insert(name, v);
    }
    out
}

/// (slide, shape) of the first frame matching `want`.
fn find(p: &Presentation, want: &str) -> (usize, usize) {
    for i in 0..p.len() {
        for (k, s) in p.slide(i).shapes.iter().enumerate() {
            let hit = match want {
                "table" => p.table(i, k).is_some(),
                _ => s.chart_part().is_some(),
            };
            if hit {
                return (i, k);
            }
        }
    }
    panic!("no {want}");
}

#[test]
fn edit_table_cells_rows_columns_and_fill() {
    let mut p = Presentation::open(&corpus()).unwrap();
    let (s, k) = find(&p, "table");
    let t = p.table_text(s, k).unwrap();
    assert_eq!(t.len(), 5);
    assert_eq!(t[0], ["Region", "Q1", "Q2", "Q3"]);
    let h0 = p.slide(s).shapes[k].xfrm.unwrap().cy;
    p.edit_table(
        s,
        k,
        &[
            TableOp::Text {
                row: 1,
                col: 0,
                text: "Utara".into(),
            },
            TableOp::InsertRow {
                row: 4,
                below: true,
            },
            TableOp::Text {
                row: 5,
                col: 0,
                text: "Central".into(),
            },
            TableOp::InsertCol {
                col: 3,
                right: true,
            },
            TableOp::Text {
                row: 0,
                col: 4,
                text: "Q4".into(),
            },
            TableOp::Fill {
                row: 0,
                col: 4,
                color: Some(Rgb([0x1F, 0x5F, 0xBF])),
            },
        ],
    )
    .unwrap();
    let t = p.table_text(s, k).unwrap();
    assert_eq!(t.len(), 6);
    assert_eq!(t[0], ["Region", "Q1", "Q2", "Q3", "Q4"]);
    assert_eq!(t[1][0], "Utara");
    assert_eq!(t[5], ["Central", "", "", "", ""]);
    assert!(p.slide(s).shapes[k].xfrm.unwrap().cy > h0, "the frame grew");
    // One undo step.
    p.undo();
    assert_eq!(p.table_text(s, k).unwrap()[1][0], "North");
    p.redo();

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("t.pptx");
    p.save_as(&out).unwrap();
    let orig = parts(&std::fs::read(corpus()).unwrap());
    let saved = parts(&std::fs::read(&out).unwrap());
    let slide_part = format!("ppt/slides/slide{}.xml", s + 1);
    for (name, bytes) in &orig {
        if *name != slide_part && !name.starts_with("docProps") {
            assert_eq!(&saved[name], bytes, "{name} unchanged");
        }
    }
    let xml = String::from_utf8(saved[&slide_part].clone()).unwrap();
    assert!(xml.contains(r#"<a:srgbClr val="1F5FBF"/>"#));
    let back = Presentation::open(&out).unwrap();
    assert_eq!(back.table_text(s, k).unwrap(), t);
    assert!(matches!(
        back.slide(s).shapes[k].kind,
        ShapeKind::Other { .. }
    ));
}

#[test]
fn edit_chart_data_and_its_workbook() {
    let mut p = Presentation::open(&corpus()).unwrap();
    let (s, k) = find(&p, "chart");
    let d = p.chart_data(s, k).unwrap();
    assert_eq!(d.categories, ["Q1", "Q2", "Q3", "Q4"]);
    assert_eq!(d.series.len(), 2);
    assert_eq!(d.series[0].name, "2025");
    assert_eq!(d.series[1].values[3], Some(17.1));
    let mut e: ChartData = d.clone();
    e.series[1].values[3] = Some(21.5);
    e.series[1].name = "2026 (plan)".into();
    e.categories.push("Q5".into());
    for s in &mut e.series {
        s.values.push(Some(9.0));
    }
    p.set_chart_data(s, k, &e).unwrap();
    assert_eq!(p.chart_data(s, k).unwrap(), e);
    p.undo();
    assert_eq!(p.chart_data(s, k).unwrap(), d);
    p.redo();

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("c.pptx");
    p.save_as(&out).unwrap();
    let back = Presentation::open(&out).unwrap();
    assert_eq!(back.chart_data(s, k).unwrap(), e);
    let saved = parts(&std::fs::read(&out).unwrap());
    let orig = parts(&std::fs::read(corpus()).unwrap());
    let chart = String::from_utf8(saved["ppt/charts/chart1.xml"].clone()).unwrap();
    assert!(chart.contains("<c:f>Sheet1!$A$2:$A$6</c:f>"), "{chart}");
    // The other chart and its workbook are untouched.
    assert_eq!(
        saved["ppt/charts/chart2.xml"],
        orig["ppt/charts/chart2.xml"]
    );
    assert_eq!(
        saved["ppt/embeddings/Microsoft_Excel_Sheet2.xlsx"],
        orig["ppt/embeddings/Microsoft_Excel_Sheet2.xlsx"]
    );
    // The embedded workbook has the new values where the chart points.
    let wb = parts(&saved["ppt/embeddings/Microsoft_Excel_Sheet1.xlsx"]);
    let sheet = String::from_utf8(wb["xl/worksheets/sheet1.xml"].clone()).unwrap();
    assert!(
        sheet.contains(r#"<c r="C5" s="1"><v>21.5</v></c>"#),
        "{sheet}"
    );
    assert!(
        sheet.contains(r#"<c r="A6" t="inlineStr"><is><t xml:space="preserve">Q5</t></is></c>"#)
    );
    assert!(sheet.contains(r#"<t xml:space="preserve">2026 (plan)</t>"#));
    // Series are added with add_chart_series, not by passing more.
    let mut more = e.clone();
    more.series.push(more.series[0].clone());
    assert!(p.set_chart_data(s, k, &more).is_err());
}

#[test]
fn add_and_remove_chart_series() {
    let mut p = Presentation::open(&corpus()).unwrap();
    let (s, k) = find(&p, "chart");
    let d = p.chart_data(s, k).unwrap();
    p.add_chart_series(s, k, "2027").unwrap();
    let mut e = p.chart_data(s, k).unwrap();
    assert_eq!(e.series.len(), 3);
    assert_eq!(e.series[2].name, "2027");
    assert_eq!(e.series[2].values, vec![None; 4]);
    e.series[2].values = vec![Some(1.0), Some(2.0), Some(3.0), Some(4.5)];
    p.set_chart_data(s, k, &e).unwrap();
    let (_, part) = (0, p.slide(s).shapes[k].chart_part().unwrap().to_owned());
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("series.pptx");
    p.save_as(&out).unwrap();
    let saved = parts(&std::fs::read(&out).unwrap());
    let chart = String::from_utf8(saved[&part].clone()).unwrap();
    assert!(chart.contains("<c:f>Sheet1!$D$1</c:f>"), "{chart}");
    assert!(chart.contains("<c:f>Sheet1!$D$2:$D$5</c:f>"));
    assert_eq!(chart.matches("<c:ser>").count(), 3);
    assert_eq!(chart.matches("<c:idx val=\"2\"/>").count(), 1);
    let wb = parts(&saved["ppt/embeddings/Microsoft_Excel_Sheet1.xlsx"]);
    let sheet = String::from_utf8(wb["xl/worksheets/sheet1.xml"].clone()).unwrap();
    assert!(
        sheet.contains(r#"<t xml:space="preserve">2027</t>"#),
        "{sheet}"
    );
    assert!(sheet.contains(r#"<c r="D5"><v>4.5</v></c>"#), "{sheet}");
    assert!(sheet.contains(r#"<dimension ref="A1:D5"/>"#), "{sheet}");
    let back = Presentation::open(&out).unwrap();
    assert_eq!(back.chart_data(s, k).unwrap(), e);

    // Remove the first: the others move into its cells.
    p.remove_chart_series(s, k, 0).unwrap();
    let r = p.chart_data(s, k).unwrap();
    assert_eq!(r.series.len(), 2);
    assert_eq!(r.series[0], e.series[1]);
    assert_eq!(r.series[1], e.series[2]);
    p.save_as(&out).unwrap();
    let saved = parts(&std::fs::read(&out).unwrap());
    let chart = String::from_utf8(saved[&part].clone()).unwrap();
    assert!(chart.contains("<c:f>Sheet1!$B$2:$B$5</c:f>"));
    assert!(chart.contains("<c:f>Sheet1!$C$2:$C$5</c:f>"));
    assert!(!chart.contains("$D$"), "{chart}");
    let wb = parts(&saved["ppt/embeddings/Microsoft_Excel_Sheet1.xlsx"]);
    let sheet = String::from_utf8(wb["xl/worksheets/sheet1.xml"].clone()).unwrap();
    assert!(
        sheet.contains(r#"<c r="C5"><v>4.5</v></c>"#)
            || sheet.contains(r#"<c r="C5" s="1"><v>4.5</v></c>"#),
        "{sheet}"
    );
    assert!(sheet.contains(r#"<c r="D5"/>"#), "D emptied: {sheet}");
    assert!(Presentation::open(&out).unwrap().chart_data(s, k).unwrap() == r);
    // Undo twice: the original data.
    p.undo();
    p.undo();
    p.undo();
    assert_eq!(p.chart_data(s, k).unwrap(), d);
    // The last series can't go.
    p.remove_chart_series(s, k, 0).unwrap();
    assert!(p.remove_chart_series(s, k, 0).is_err());
}
