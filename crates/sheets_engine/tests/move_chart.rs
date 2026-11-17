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

#[test]
fn every_chart_moves_after_its_options_change() {
    use sheets_engine::{AxisOptions, DataLabels, DisplayUnits};
    let mut wb = Workbook::open(corpus("20_charts.xlsx")).unwrap();
    let keys = charts(&wb, 0);
    for key in &keys {
        let Some(info) = wb.chart_info(*key) else {
            continue;
        };
        if info.fixed {
            continue;
        }
        let mut spec = info.spec.clone();
        spec.labels = vec![
            DataLabels {
                value: true,
                pos: info.label_positions.first().copied(),
                ..DataLabels::default()
            };
            info.series.len()
        ];
        spec.y_axis = AxisOptions {
            log_base: Some(10.0),
            units: DisplayUnits::Thousands,
            ..AxisOptions::default()
        };
        wb.set_chart(*key, spec).unwrap();
    }
    for (i, key) in keys.iter().enumerate() {
        if wb.chart_info(*key).is_none() {
            continue;
        }
        let moved = wb
            .move_chart(*key, ChartPlace::NewSheet(format!("Moved {i}")))
            .unwrap_or_else(|e| panic!("chart {i}: {e}"));
        let MovedChart::ChartSheet(k) = moved else {
            panic!()
        };
        let d = wb.chart_sheet_drawings(k)[0].key;
        assert!(wb.chart_info(d).is_some(), "chart {i} on its sheet");
        wb.move_chart(d, ChartPlace::ObjectIn(0))
            .unwrap_or_else(|e| panic!("chart {i} back: {e}"));
    }
    let path = scratch("all-moved.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
}

const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, 0x49, 0x48, 0x44, 0x52, 0, 0, 0,
    1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1F, 0x15, 0xC4, 0x89, 0, 0, 0, 0x0D, 0x49, 0x44, 0x41, 0x54,
    0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0xF0, 0x1F, 0, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D,
    0x1D, 0, 0, 0, 0, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

/// 20_charts.xlsx with chart1 linked to an outside workbook
/// (`c:externalData`) and holding user shapes (a text box and a picture,
/// `c:userShapes`), and a group with a picture on the first sheet.
fn linked_book() -> PathBuf {
    use std::io::Write as _;
    let src = std::fs::read(corpus("20_charts.xlsx")).unwrap();
    let mut zin = zip::ZipArchive::new(std::io::Cursor::new(src)).unwrap();
    let path = scratch("linked.xlsx");
    let mut out = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    for i in 0..zin.len() {
        let mut e = zin.by_index(i).unwrap();
        let name = e.name().unwrap().into_owned();
        let mut b = Vec::new();
        e.read_to_end(&mut b).unwrap();
        let mut text = String::from_utf8_lossy(&b).into_owned();
        match name.as_str() {
            "xl/charts/chart1.xml" => {
                text = text.replace(
                    "</c:chartSpace>",
                    &format!("<c:externalData xmlns:r=\"{R}\" r:id=\"rId1\"><c:autoUpdate val=\"0\"/></c:externalData><c:userShapes xmlns:r=\"{R}\" r:id=\"rId2\"/></c:chartSpace>"),
                );
            }
            "[Content_Types].xml" => {
                text = text.replace(
                    "</Types>",
                    "<Override PartName=\"/xl/drawings/drawing9.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.drawingml.chartshapes+xml\"/></Types>",
                );
            }
            "xl/drawings/drawing1.xml" => {
                text = text.replace(
                    "</xdr:wsDr>",
                    &format!("<xdr:twoCellAnchor><xdr:from><xdr:col>12</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>2</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:from><xdr:to><xdr:col>15</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>8</xdr:row><xdr:rowOff>0</xdr:rowOff></xdr:to><xdr:grpSp><xdr:nvGrpSpPr><xdr:cNvPr id=\"90\" name=\"Logo group\"/><xdr:cNvGrpSpPr/></xdr:nvGrpSpPr><xdr:grpSpPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"1800000\" cy=\"1100000\"/><a:chOff x=\"0\" y=\"0\"/><a:chExt cx=\"1800000\" cy=\"1100000\"/></a:xfrm></xdr:grpSpPr><xdr:sp><xdr:nvSpPr><xdr:cNvPr id=\"91\" name=\"Box\"/><xdr:cNvSpPr/></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"1800000\" cy=\"1100000\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val=\"DDEEFF\"/></a:solidFill></xdr:spPr></xdr:sp><xdr:pic><xdr:nvPicPr><xdr:cNvPr id=\"92\" name=\"Logo\"/><xdr:cNvPicPr/></xdr:nvPicPr><xdr:blipFill><a:blip xmlns:r=\"{R}\" r:embed=\"rId90\"/><a:stretch><a:fillRect/></a:stretch></xdr:blipFill><xdr:spPr><a:xfrm><a:off x=\"100000\" y=\"100000\"/><a:ext cx=\"600000\" cy=\"600000\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></xdr:spPr></xdr:pic></xdr:grpSp><xdr:clientData/></xdr:twoCellAnchor></xdr:wsDr>"),
                );
            }
            "xl/drawings/_rels/drawing1.xml.rels" => {
                text = text.replace(
                    "</Relationships>",
                    &format!("<Relationship Id=\"rId90\" Type=\"{R}/image\" Target=\"../media/image7.png\"/></Relationships>"),
                );
            }
            _ => {}
        }
        out.start_file(name.clone(), opts).unwrap();
        if name.ends_with(".xml") || name.ends_with(".rels") {
            out.write_all(text.as_bytes()).unwrap();
        } else {
            out.write_all(&b).unwrap();
        }
    }
    let add = |out: &mut zip::ZipWriter<std::fs::File>, name: &str, data: &[u8]| {
        out.start_file(name, opts).unwrap();
        out.write_all(data).unwrap();
    };
    add(
        &mut out,
        "xl/charts/_rels/chart1.xml.rels",
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{R}/oleObject\" Target=\"file:///C:/Data/Sales%20source.xlsx\" TargetMode=\"External\"/><Relationship Id=\"rId2\" Type=\"{R}/chartUserShapes\" Target=\"../drawings/drawing9.xml\"/></Relationships>").as_bytes(),
    );
    add(
        &mut out,
        "xl/drawings/drawing9.xml",
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<c:userShapes xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:cdr=\"http://schemas.openxmlformats.org/drawingml/2006/chartDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"{R}\"><cdr:relSizeAnchor><cdr:from><cdr:x>0.05</cdr:x><cdr:y>0.05</cdr:y></cdr:from><cdr:to><cdr:x>0.2</cdr:x><cdr:y>0.2</cdr:y></cdr:to><cdr:pic><cdr:nvPicPr><cdr:cNvPr id=\"2\" name=\"Stamp\"/><cdr:cNvPicPr/></cdr:nvPicPr><cdr:blipFill><a:blip r:embed=\"rId1\"/><a:stretch><a:fillRect/></a:stretch></cdr:blipFill><cdr:spPr><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></cdr:spPr></cdr:pic></cdr:relSizeAnchor></c:userShapes>").as_bytes(),
    );
    add(
        &mut out,
        "xl/drawings/_rels/drawing9.xml.rels",
        format!("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"{R}/image\" Target=\"../media/image8.png\"/></Relationships>").as_bytes(),
    );
    add(&mut out, "xl/media/image7.png", PNG);
    add(&mut out, "xl/media/image8.png", PNG);
    out.finish().unwrap();
    path
}

/// The chart part `chart` links to, by relationship type: target part (or
/// external target).
fn chart_rels(p: &BTreeMap<String, String>, chart: &str) -> BTreeMap<String, String> {
    let (dir, file) = chart.rsplit_once('/').unwrap();
    let doc = Document::parse(&p[&format!("{dir}/_rels/{file}.rels")]).unwrap();
    doc.root_element()
        .children()
        .filter(|n| n.is_element())
        .map(|r| {
            let kind = r
                .attribute("Type")
                .unwrap()
                .rsplit('/')
                .next()
                .unwrap()
                .to_owned();
            let t = r.attribute("Target").unwrap();
            let t = if r.attribute("TargetMode") == Some("External") {
                t.to_owned()
            } else {
                format!("xl/{}", t.trim_start_matches("../"))
            };
            (kind, t)
        })
        .collect()
}

fn check_links(p: &BTreeMap<String, String>, chart: &str) {
    assert!(p[chart].contains("<c:userShapes"), "{}", p[chart]);
    assert!(p[chart].contains("<c:externalData"), "{}", p[chart]);
    let rels = chart_rels(p, chart);
    assert_eq!(rels["oleObject"], "file:///C:/Data/Sales%20source.xlsx");
    let us = &rels["chartUserShapes"];
    assert!(p[us].contains("name=\"Stamp\""), "{us}");
    let pics = chart_rels(p, us);
    assert!(p.contains_key(&pics["image"]), "{pics:?}");
    assert!(p["[Content_Types].xml"].contains(&format!("PartName=\"/{us}\"")));
}

#[test]
fn moved_charts_keep_their_links() {
    let src = linked_book();
    let mut wb = Workbook::open(&src).unwrap();
    let key = wb
        .drawings(0)
        .iter()
        .find(|d| matches!(&*d.kind, DrawingKind::Chart { part, .. } if &**part == "xl/charts/chart1.xml"))
        .unwrap()
        .key;
    // To a new chart sheet.
    let MovedChart::ChartSheet(k) = wb
        .move_chart(key, ChartPlace::NewSheet("Linked".into()))
        .unwrap()
    else {
        panic!()
    };
    let path = scratch("linked-sheet.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    assert!(
        chart_sheet_charts(&p)
            .iter()
            .any(|x| x.contains("<c:userShapes"))
    );
    let moved = p
        .keys()
        .find(|n| n.starts_with("xl/charts/kantorcs") && !n.contains("_rels"))
        .expect("the chart sheet's chart");
    check_links(&p, moved);
    // Back onto the second worksheet: still linked.
    let cs = wb.chart_sheet_drawings(k)[0].key;
    wb.move_chart(cs, ChartPlace::ObjectIn(1)).unwrap();
    let path = scratch("linked-object.xlsx");
    wb.save_as(&path).unwrap();
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    let back = Workbook::open(&path).unwrap();
    let part = back
        .drawings(1)
        .iter()
        .find_map(|d| match &*d.kind {
            DrawingKind::Chart { part, xml, .. } if xml.contains("userShapes") => {
                Some(part.to_string())
            }
            _ => None,
        })
        .expect("the moved chart on sheet 2");
    check_links(&p, &part);
}

#[test]
fn copied_sheets_bring_group_pictures_and_chart_links() {
    let src = linked_book();
    let mut wb = Workbook::open(&src).unwrap();
    let before = wb.drawings(0).len();
    let copy = wb.copy_sheet(0, 2).unwrap();
    assert_eq!(
        wb.drawings(copy).len(),
        before,
        "the group and the linked chart came too"
    );
    let path = scratch("linked-copy.xlsx");
    wb.save_as(&path).unwrap();
    assert!(wb.save_notes().is_empty(), "{:?}", wb.save_notes());
    assert!(sheets_engine::package_problems(&std::fs::read(&path).unwrap()).is_empty());
    let p = parts(&path);
    let back = Workbook::open(&path).unwrap();
    let group = back
        .drawings(copy)
        .into_iter()
        .find(|d| d.name == "Logo group")
        .expect("the copied group");
    assert!(matches!(&*group.kind, DrawingKind::Shape { .. }));
    // Its picture's r:embed points at an image of the copy's drawing.
    let drawing = p
        .iter()
        .filter(|(n, x)| {
            n.starts_with("xl/drawings/drawing") && n.ends_with(".xml") && x.contains("Logo group")
        })
        .map(|(n, _)| n.clone())
        .find(|n| n != "xl/drawings/drawing1.xml")
        .expect("the copy's drawing");
    let doc = Document::parse(&p[&drawing]).unwrap();
    let embed = doc
        .descendants()
        .find(|n| n.has_tag_name("blip") && n.ancestors().any(|a| a.has_tag_name("grpSp")))
        .and_then(|b| b.attribute((R, "embed")))
        .unwrap()
        .to_owned();
    let (dir, file) = drawing.rsplit_once('/').unwrap();
    let rels = Document::parse(&p[&format!("{dir}/_rels/{file}.rels")]).unwrap();
    let target = rels
        .descendants()
        .find(|r| r.attribute("Id") == Some(embed.as_str()))
        .and_then(|r| r.attribute("Target"))
        .unwrap();
    assert!(
        p.contains_key(&format!("xl/{}", target.trim_start_matches("../"))),
        "{target}"
    );
    // The copied chart keeps its links too.
    let linked: Vec<&String> = p
        .iter()
        .filter(|(n, x)| n.starts_with("xl/charts/") && x.contains("<c:userShapes"))
        .map(|(n, _)| n)
        .collect();
    assert_eq!(linked.len(), 2, "{linked:?}");
    for c in linked {
        check_links(&p, c);
    }
    // Saved again: still one picture per group, no new relationships.
    let path2 = scratch("linked-copy2.xlsx");
    wb.save_as(&path2).unwrap();
    assert!(sheets_engine::package_problems(&std::fs::read(&path2).unwrap()).is_empty());
}
