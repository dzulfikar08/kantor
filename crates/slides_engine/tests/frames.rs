//! Tables, charts and SmartArt frames on the slide (tests/corpus/pptx/
//! frames.pptx from scripts/corpus/gen_pptx_frames.py, and groups.pptx):
//! move, resize, copy and paste, group; tables with merged cells; typing
//! into table cells. Each is saved and read again.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _};
use std::path::PathBuf;

use slides_engine::{ChartData, Presentation, ShapeRef, TableOp, TextPara, Xfrm, package_problems};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/pptx")
        .join(name)
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

fn idx(p: &Presentation, slide: usize, name: &str) -> usize {
    p.slide(slide)
        .shapes
        .iter()
        .position(|s| s.name == name)
        .unwrap_or_else(|| panic!("no shape {name}"))
}

fn reopen(p: &Presentation) -> (Presentation, BTreeMap<String, Vec<u8>>) {
    let bytes = p.to_bytes().unwrap();
    assert!(
        package_problems(&bytes).is_empty(),
        "{:?}",
        package_problems(&bytes)
    );
    (
        Presentation::from_bytes(bytes.clone()).unwrap(),
        parts(&bytes),
    )
}

fn changed(a: &BTreeMap<String, Vec<u8>>, b: &BTreeMap<String, Vec<u8>>) -> Vec<String> {
    let mut names: Vec<String> = a
        .iter()
        .filter(|(k, v)| b.get(*k) != Some(*v))
        .map(|(k, _)| k.clone())
        .collect();
    names.extend(b.keys().filter(|k| !a.contains_key(*k)).cloned());
    names.sort();
    names
}

#[test]
fn frames_move_and_resize_and_save_in_place() {
    let path = corpus("frames.pptx");
    let orig = parts(&std::fs::read(&path).unwrap());
    let mut p = Presentation::open(&path).unwrap();
    let chart = idx(&p, 1, "Sales");
    assert!(p.slide(1).shapes[chart].movable());
    assert!(!p.slide(1).shapes[chart].rotatable());
    let x = p.slide(1).shapes[chart].xfrm.unwrap();
    // A nudge, then a resize: the chart re-lays out at its new size.
    p.set_xfrm(
        (1, chart),
        Xfrm {
            x: x.x + 12_700,
            ..x
        },
        true,
    )
    .unwrap();
    p.set_xfrm(
        (1, chart),
        Xfrm {
            x: x.x + 25_400,
            ..x
        },
        true,
    )
    .unwrap();
    let big = Xfrm {
        x: x.x + 25_400,
        cx: x.cx + 914_400,
        cy: x.cy / 2,
        ..x
    };
    p.set_xfrm((1, chart), big, false).unwrap();
    let (q, saved) = reopen(&p);
    assert_eq!(q.slide(1).shapes[chart].xfrm, Some(big));
    // Only the slide changed, and in it only the frame's p:xfrm.
    assert_eq!(changed(&orig, &saved), ["ppt/slides/slide2.xml"]);
    let a = String::from_utf8(orig["ppt/slides/slide2.xml"].clone()).unwrap();
    let b = String::from_utf8(saved["ppt/slides/slide2.xml"].clone()).unwrap();
    let cut = |s: &str| {
        let i = s.find("<p:graphicFrame").unwrap();
        let x0 = i + s[i..].find("<p:xfrm>").unwrap();
        let x1 = x0 + s[x0..].find("</p:xfrm>").unwrap();
        (s[..x0].to_owned(), s[x1..].to_owned())
    };
    assert_eq!(cut(&a), cut(&b));
    // Two nudges were one undo step, the resize another.
    p.undo().unwrap();
    assert_eq!(p.slide(1).shapes[chart].xfrm.unwrap().x, x.x + 25_400);
    p.undo().unwrap();
    assert_eq!(p.slide(1).shapes[chart].xfrm, Some(x));
    assert!(!p.is_dirty());
}

#[test]
fn resizing_a_table_scales_its_columns_and_rows() {
    let mut p = Presentation::open(&corpus("frames.pptx")).unwrap();
    let t = idx(&p, 0, "Plan");
    let x = p.slide(0).shapes[t].xfrm.unwrap();
    let cols0 = p.table(0, t).unwrap().cols.clone();
    let to = Xfrm {
        x: x.x + 100,
        cx: x.cx * 3 / 2,
        cy: x.cy * 2,
        ..x
    };
    p.set_xfrm((0, t), to, false).unwrap();
    let tb = p.table(0, t).unwrap();
    for (a, b) in cols0.iter().zip(&tb.cols) {
        assert!((b - a * 3 / 2).abs() <= 1, "{a} → {b}");
    }
    assert_eq!(tb.cols.iter().sum::<i64>(), to.cx);
    assert_eq!(tb.rows.iter().map(|r| r.h).sum::<i64>(), to.cy);
    assert_eq!(p.slide(0).shapes[t].xfrm, Some(to));
    let (q, _) = reopen(&p);
    assert_eq!(q.table(0, t).unwrap().cols, tb.cols);
    // Then a cell edit keeps the new size and place.
    p.edit_table(
        0,
        t,
        &[TableOp::Text {
            row: 2,
            col: 0,
            text: "Utara".into(),
        }],
    )
    .unwrap();
    assert_eq!(p.slide(0).shapes[t].xfrm, Some(to));
    p.undo().unwrap();
    p.undo().unwrap();
    assert_eq!(p.table(0, t).unwrap().cols, cols0);
    assert_eq!(p.slide(0).shapes[t].xfrm, Some(x));
}

#[test]
fn rows_and_columns_in_a_merged_table() {
    let mut p = Presentation::open(&corpus("frames.pptx")).unwrap();
    let t = idx(&p, 0, "Plan");
    let h0 = p.slide(0).shapes[t].xfrm.unwrap().cy;
    // Inside "Area" (rows 0–1) and "Budget" (columns 1–2).
    p.edit_table(
        0,
        t,
        &[
            TableOp::InsertRow {
                row: 0,
                below: true,
            },
            TableOp::InsertCol {
                col: 1,
                right: true,
            },
        ],
    )
    .unwrap();
    let tb = p.table(0, t).unwrap();
    assert_eq!((tb.rows.len(), tb.cols.len()), (5, 5));
    assert_eq!(tb.rows[0].cells[0].row_span, 3);
    assert_eq!(tb.rows[0].cells[1].grid_span, 3);
    assert!(tb.rows[1].cells[0].v_merge && tb.rows[0].cells[2].h_merge);
    assert!(p.slide(0).shapes[t].xfrm.unwrap().cy > h0);
    let (q, _) = reopen(&p);
    let text = q.table_text(0, t).unwrap();
    assert_eq!(text[0][0], "Area");
    assert_eq!(text[0][1], "Budget");
    // Removing the first row: the row below takes "Area".
    p.edit_table(0, t, &[TableOp::DeleteRow(0)]).unwrap();
    let text = p.table_text(0, t).unwrap();
    assert_eq!(text[0][0], "Area");
    assert_eq!(p.table(0, t).unwrap().rows[0].cells[0].row_span, 2);
    // The "Total" block (rows 2–3, columns 3–4) loses a column.
    p.edit_table(0, t, &[TableOp::DeleteCol(4)]).unwrap();
    let tb = p.table(0, t).unwrap();
    let total = tb
        .rows
        .iter()
        .flat_map(|r| &r.cells)
        .find(|c| c.text.as_ref().is_some_and(|b| b.plain_text() == "Total"))
        .unwrap();
    assert_eq!((total.row_span, total.grid_span), (2, 1));
    reopen(&p);
}

#[test]
fn typing_into_a_table_cell_keeps_its_formatting() {
    let mut p = Presentation::open(&corpus("frames.pptx")).unwrap();
    let t = idx(&p, 0, "Plan");
    let mut paras = p.table(0, t).unwrap().rows[2].cells[0]
        .text
        .clone()
        .unwrap()
        .paras;
    paras[0].insert(5, " East", Default::default());
    paras.push(TextPara::new("second"));
    p.edit_table(
        0,
        t,
        &[TableOp::Paras {
            row: 2,
            col: 0,
            paras,
        }],
    )
    .unwrap();
    let (q, saved) = reopen(&p);
    assert_eq!(q.table_text(0, t).unwrap()[2][0], "North East\nsecond");
    let slide = String::from_utf8(saved["ppt/slides/slide1.xml"].clone()).unwrap();
    assert!(slide.contains("<a:t>Lead</a:t>"));
    // The cell is addressed like a shape inside the frame.
    let r = ShapeRef::from((0, t)).cell(2, 0);
    assert_eq!(r.cell_at(), Some((2, 0)));
    assert!(r.is_top());
}

#[test]
fn copying_a_chart_copies_its_parts() {
    let mut p = Presentation::open(&corpus("frames.pptx")).unwrap();
    let chart = idx(&p, 1, "Sales");
    let original = p.chart_data(1, chart).unwrap();
    let clip = p.copy_shapes(1, &[chart]).unwrap();
    let added = p.paste_shapes(0, &clip).unwrap();
    let k = added[0];
    let part = p.slide(0).shapes[k].chart_part().unwrap().to_owned();
    assert_ne!(Some(part.as_str()), p.slide(1).shapes[chart].chart_part());
    assert_eq!(p.chart_data(0, k).unwrap(), original);
    // Editing the copy leaves the original alone.
    let mut d: ChartData = original.clone();
    d.series[0].values[0] = Some(99.0);
    p.set_chart_data(0, k, &d).unwrap();
    assert_eq!(p.chart_data(1, chart).unwrap(), original);
    let (q, saved) = reopen(&p);
    assert!(saved.contains_key(&part));
    let rels = format!(
        "{}/_rels/{}.rels",
        part.rsplit_once('/').unwrap().0,
        part.rsplit_once('/').unwrap().1
    );
    let rels = String::from_utf8(saved[&rels].clone()).unwrap();
    assert!(rels.contains(".xlsx"), "{rels}");
    let ct = String::from_utf8(saved["[Content_Types].xml"].clone()).unwrap();
    assert!(ct.contains(&format!("/{part}")), "{ct}");
    assert_eq!(q.chart_data(0, k).unwrap().series[0].values[0], Some(99.0));
    assert_eq!(q.chart_data(1, chart).unwrap(), original);
    // Undoing the paste leaves the parts out of the file.
    p.undo().unwrap();
    p.undo().unwrap();
    let (_, saved) = reopen(&p);
    assert!(!saved.contains_key(&part));
}

#[test]
fn charts_paste_into_another_presentation() {
    let src = Presentation::open(&corpus("frames.pptx")).unwrap();
    let chart = idx(&src, 1, "Sales");
    let clip = src.copy_shapes(1, &[chart]).unwrap();
    let mut p = Presentation::new();
    let k = p.paste_shapes(0, &clip).unwrap()[0];
    assert_eq!(p.chart_data(0, k), src.chart_data(1, chart));
    let (q, _) = reopen(&p);
    assert_eq!(q.chart_data(0, k), src.chart_data(1, chart));
}

#[test]
fn copying_smartart_and_tables() {
    let mut p = Presentation::open(&corpus("groups.pptx")).unwrap();
    let d = p
        .slide(1)
        .shapes
        .iter()
        .position(|s| s.smartart().is_some())
        .unwrap();
    let clip = p.copy_shapes(1, &[d]).unwrap();
    let k = p.paste_shapes(1, &clip).unwrap()[0];
    let copy = &p.slide(1).shapes[k];
    assert!(copy.smartart().is_some(), "the copy reads its drawing");
    assert_ne!(
        copy.smartart().unwrap().part,
        p.slide(1).shapes[d].smartart().unwrap().part
    );
    // Its text edits go to its own drawing.
    let r = ShapeRef::from((1, k)).child(0).unwrap();
    let sel = slides_engine::Selection::caret(slides_engine::Position::new(0, 0));
    p.insert_text(r, sel, "New ", None).unwrap();
    let (q, saved) = reopen(&p);
    let orig_drawing = String::from_utf8(saved["ppt/diagrams/drawing1.xml"].clone()).unwrap();
    assert!(!orig_drawing.contains("New "));
    let shapes = &q.slide(1).shapes;
    assert_eq!(shapes.iter().filter(|s| s.smartart().is_some()).count(), 2);

    let mut p = Presentation::open(&corpus("frames.pptx")).unwrap();
    let t = idx(&p, 0, "Plan");
    let k = p.duplicate_shape((0, t)).unwrap();
    assert_eq!(p.table_text(0, k), p.table_text(0, t));
    let (q, _) = reopen(&p);
    assert_eq!(q.table_text(0, k), p.table_text(0, t));
}

#[test]
fn charts_group_and_tables_dont() {
    let mut p = Presentation::open(&corpus("frames.pptx")).unwrap();
    let (chart, note) = (idx(&p, 1, "Sales"), idx(&p, 1, "Note"));
    let g = p.group_shapes(1, &[chart, note]).unwrap();
    assert!(p.slide(1).shapes[g].is_group());
    let (q, _) = reopen(&p);
    assert!(q.slide(1).shapes[g].is_group());
    let t = idx(&p, 0, "Plan");
    let title = 0;
    assert!(p.group_shapes(0, &[title, t]).is_err());
}

#[test]
fn typing_in_a_cell_is_text_editing() {
    use slides_engine::{Position, Selection, Toggle};
    let mut p = Presentation::open(&corpus("frames.pptx")).unwrap();
    let t = idx(&p, 0, "Plan");
    let r = ShapeRef::from((0, t)).cell(3, 1);
    assert_eq!(p.text_paras(r).unwrap()[0].text, "8");
    let mut sel = Selection::caret(Position::new(0, 1));
    for ch in ["0", "0", " ", "k"] {
        sel = p.insert_text(r, sel, ch, None).unwrap();
    }
    assert_eq!(p.table_text(0, t).unwrap()[3][1], "800 k");
    sel = p.insert_text(r, sel, "\n", None).unwrap();
    sel = p.insert_text(r, sel, "x", None).unwrap();
    sel = p.backspace(r, sel).unwrap();
    assert_eq!(sel.head, Position::new(1, 0));
    p.toggle(
        r,
        Selection {
            anchor: Position::new(0, 0),
            head: Position::new(0, 3),
        },
        Toggle::Bold,
    )
    .unwrap();
    assert!(p.is_on(
        r,
        Selection {
            anchor: Position::new(0, 0),
            head: Position::new(0, 3)
        },
        Toggle::Bold
    ));
    let (q, _) = reopen(&p);
    assert_eq!(q.table_text(0, t).unwrap()[3][1], "800 k\n");
    // Covered cells can't be typed into.
    let covered = ShapeRef::from((0, t)).cell(1, 0);
    assert!(
        p.insert_text(covered, Selection::default(), "x", None)
            .is_err()
    );
    // Typing coalesced: a few undo steps take it back.
    while p.can_undo() {
        p.undo();
    }
    assert_eq!(p.table_text(0, t).unwrap()[3][1], "8");
    assert!(!p.is_dirty());
}
