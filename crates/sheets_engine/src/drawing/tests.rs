use super::*;

use std::path::{Path, PathBuf};

/// A grid of explicit column widths and row heights (then defaults), in
/// grid px, with a scale per axis.
struct Grid {
    cols: Vec<f64>,
    rows: Vec<f64>,
    default_col: f64,
    default_row: f64,
    scale: (f64, f64),
}

impl Grid {
    fn w(&self, c: u32) -> f64 {
        self.cols
            .get(c as usize)
            .copied()
            .unwrap_or(self.default_col)
    }
    fn h(&self, r: u32) -> f64 {
        self.rows
            .get(r as usize)
            .copied()
            .unwrap_or(self.default_row)
    }
}

impl SheetGeometry for Grid {
    fn col_left(&self, col: u32) -> f64 {
        (0..col).map(|c| self.w(c)).sum()
    }
    fn col_width(&self, col: u32) -> f64 {
        self.w(col)
    }
    fn row_top(&self, row: u32) -> f64 {
        (0..row).map(|r| self.h(r)).sum()
    }
    fn row_height(&self, row: u32) -> f64 {
        self.h(row)
    }
    fn col_at(&self, x: f64) -> u32 {
        let mut c = 0;
        while self.col_left(c + 1) <= x {
            c += 1;
        }
        c
    }
    fn row_at(&self, y: f64) -> u32 {
        let mut r = 0;
        while self.row_top(r + 1) <= y {
            r += 1;
        }
        r
    }
    fn scale(&self) -> (f64, f64) {
        self.scale
    }
}

fn grid() -> Grid {
    Grid {
        // Column C is wide, D hidden-narrow; row 1 tall, row 2 hidden.
        cols: vec![100.0, 50.0, 200.0, 10.0],
        rows: vec![20.0, 60.0, 0.0, 30.0],
        default_col: 80.0,
        default_row: 25.0,
        scale: (1.5, 1.25),
    }
}

fn px(n: f64) -> i64 {
    (n * EMU_PER_PX) as i64
}

fn close(a: [f64; 4], b: [f64; 4]) {
    for i in 0..4 {
        assert!((a[i] - b[i]).abs() < 1e-6, "{a:?} != {b:?}");
    }
}

#[test]
fn two_cell_anchors_follow_mixed_widths_and_heights() {
    let g = grid();
    // From B2 + (10, 8) Excel px to D4 + (4, 2) Excel px.
    let a = Anchor::TwoCell {
        from: Marker {
            col: 1,
            col_off: px(10.0),
            row: 1,
            row_off: px(8.0),
        },
        to: Marker {
            col: 3,
            col_off: px(4.0),
            row: 3,
            row_off: px(2.0),
        },
        edit_as: EditAs::TwoCell,
    };
    // x: A(100) + 10*1.5; y: row 0 (20) + 8*1.25.
    let x0 = 100.0 + 15.0;
    let y0 = 20.0 + 10.0;
    // D starts at 100+50+200; its offset 4*1.5 = 6 is kept inside its 10 px.
    let x1 = 350.0 + 6.0;
    // Row 3 starts at 20+60+0; offset 2*1.25.
    let y1 = 80.0 + 2.5;
    close(a.rect(&g), [x0, y0, x1 - x0, y1 - y0]);
    // An offset past its cell stops at the cell's edge (Excel keeps them
    // inside, Kantor's cells can be narrower).
    let b = Anchor::TwoCell {
        from: Marker {
            col: 3,
            col_off: px(100.0),
            row: 2,
            row_off: px(50.0),
        },
        to: Marker {
            col: 5,
            col_off: 0,
            row: 5,
            row_off: 0,
        },
        edit_as: EditAs::OneCell,
    };
    let r = b.rect(&g);
    assert_eq!(
        (r[0], r[1]),
        (360.0, 80.0),
        "clamped into D and the hidden row"
    );
    // Placing it back where it is gives the same rectangle.
    let back = a.with_rect(a.rect(&g), &g);
    close(back.rect(&g), a.rect(&g));
    assert!(matches!(
        back,
        Anchor::TwoCell {
            edit_as: EditAs::TwoCell,
            ..
        }
    ));
}

#[test]
fn one_cell_and_absolute_anchors_scale_their_size() {
    let g = grid();
    let a = Anchor::OneCell {
        from: Marker {
            col: 2,
            col_off: 0,
            row: 3,
            row_off: px(4.0),
        },
        cx: px(100.0),
        cy: px(40.0),
    };
    close(a.rect(&g), [150.0, 85.0, 150.0, 50.0]);
    let moved = a.with_rect([170.0, 21.0, 90.0, 25.0], &g);
    let Anchor::OneCell { from, cx, cy } = moved else {
        panic!("kind kept")
    };
    assert_eq!((from.col, from.row), (2, 1));
    assert_eq!(from.col_off, px(20.0 / 1.5));
    assert_eq!((cx, cy), (px(60.0), px(20.0)));
    let abs = Anchor::Absolute {
        x: px(10.0),
        y: px(20.0),
        cx: px(30.0),
        cy: px(40.0),
    };
    close(abs.rect(&g), [15.0, 25.0, 45.0, 50.0]);
    close(abs.with_rect(abs.rect(&g), &g).rect(&g), abs.rect(&g));
}

#[test]
fn anchors_follow_inserted_rows_and_columns() {
    let a = Anchor::TwoCell {
        from: Marker {
            col: 2,
            col_off: 5,
            row: 4,
            row_off: 6,
        },
        to: Marker {
            col: 6,
            col_off: 0,
            row: 10,
            row_off: 0,
        },
        edit_as: EditAs::TwoCell,
    };
    let moves = Moves::new(vec![
        crate::shift::Op::InsertRows {
            sheet: 1,
            at: 2,
            count: 3,
        },
        crate::shift::Op::DeleteColumns {
            sheet: 1,
            at: 3,
            count: 1,
        },
        crate::shift::Op::InsertRows {
            sheet: 2,
            at: 0,
            count: 9,
        },
    ]);
    let Anchor::TwoCell { from, to, .. } = a.moved(&moves, 1) else {
        panic!()
    };
    assert_eq!((from.row, from.col, from.row_off), (7, 2, 6));
    assert_eq!((to.row, to.col), (13, 5));
}

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/xlsx")
        .join(name)
}

fn open_charts() -> (Workbook, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("charts.xlsx");
    std::fs::copy(corpus("20_charts.xlsx"), &path).unwrap();
    (Workbook::open(&path).unwrap(), dir)
}

#[test]
fn the_corpus_drawings_are_read() {
    let (book, _dir) = open_charts();
    let d = book.drawings(0);
    let charts = d
        .iter()
        .filter(|d| matches!(*d.kind, DrawingKind::Chart { .. }))
        .count();
    assert_eq!(charts, 7);
    let pic = d
        .iter()
        .find_map(|d| match &*d.kind {
            DrawingKind::Picture { crop, data, .. } => Some((d, *crop, data.len())),
            _ => None,
        })
        .expect("a picture");
    assert_eq!(pic.1, [0.25, 0.25, 0.0, 0.0]);
    assert!(pic.2 > 50);
    assert_eq!(pic.0.descr, "Cropped quadrants");
    assert!(d.iter().all(|d| d.editable));
    // F2: the column chart's anchor starts at column 5, row 1.
    let Anchor::TwoCell { from, .. } = d[1].anchor else {
        panic!("{:?}", d[1].anchor)
    };
    assert_eq!((from.col, from.row), (5, 1));
    assert_eq!(book.drawings(1).len(), 1);
    assert!(book.theme_xml().is_some_and(|t| t.contains("accent1")));
}

#[test]
fn chart_references_read_live_cells() {
    let (mut book, _dir) = open_charts();
    let r = book.chart_reference("Charts!$B$2:$B$4").unwrap();
    assert_eq!(
        r.values,
        vec![
            RefValue::Number(120.0),
            RefValue::Number(135.0),
            RefValue::Number(150.0)
        ]
    );
    assert_eq!(r.format.as_deref(), Some("#,##0"));
    let cats = book.chart_reference("Charts!$A$2:$A$3").unwrap();
    assert_eq!(
        cats.values,
        vec![RefValue::Text("Jan".into()), RefValue::Text("Feb".into())]
    );
    // Formula results, and an edit changing them.
    let m = book.chart_reference("Charts!$D$2").unwrap();
    assert!(matches!(m.values[0], RefValue::Number(v) if (v - 40.0 / 120.0).abs() < 1e-9));
    book.set_input(0, 1, 1, "200").unwrap();
    let m = book.chart_reference("Charts!$D$2").unwrap();
    assert!(matches!(m.values[0], RefValue::Number(v) if (v - 120.0 / 200.0).abs() < 1e-9));
    // Rows inserted above the data: the reference as opened still finds it.
    book.insert_rows(0, 0, 2).unwrap();
    let r = book.chart_reference("Charts!$B$2:$B$3").unwrap();
    assert_eq!(
        r.values,
        vec![RefValue::Number(200.0), RefValue::Number(135.0)]
    );
    // ...and the drawings moved down with the cells.
    let Anchor::TwoCell { from, .. } = book.drawings(0)[1].anchor else {
        panic!()
    };
    assert_eq!(from.row, 3);
    assert!(book.chart_reference("Nowhere!A1").is_none());
    assert!(book.chart_reference("SomeName").is_none());
}

fn drawing_xml(path: &Path) -> String {
    let file = std::fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut s = String::new();
    std::io::Read::read_to_string(
        &mut zip.by_name("xl/drawings/drawing1.xml").unwrap(),
        &mut s,
    )
    .unwrap();
    s
}

fn anchors(xml: &str) -> usize {
    xml.matches("<xdr:twoCellAnchor").count()
        + xml.matches("<xdr:oneCellAnchor").count()
        + xml.matches("<xdr:absoluteAnchor").count()
}

#[test]
fn moving_and_deleting_are_undoable_and_saved() {
    let (mut book, dir) = open_charts();
    let before = book.drawings(0);
    let n = before.len();
    let chart = before[1].clone();
    let to = Anchor::TwoCell {
        from: Marker {
            col: 30,
            col_off: 0,
            row: 40,
            row_off: px(3.0),
        },
        to: Marker {
            col: 36,
            col_off: 0,
            row: 55,
            row_off: 0,
        },
        edit_as: EditAs::TwoCell,
    };
    assert!(!book.is_dirty());
    book.place_drawing(chart.key, to).unwrap();
    assert!(book.is_dirty());
    assert_eq!(book.drawings(0)[1].anchor, to);
    book.delete_drawing(before[0].key).unwrap();
    assert_eq!(book.drawings(0).len(), n - 1);
    assert!(book.undo().unwrap());
    assert_eq!(book.drawings(0).len(), n);
    assert!(book.undo().unwrap());
    assert_eq!(book.drawings(0)[1].anchor, chart.anchor);
    assert!(!book.is_dirty());
    assert!(book.redo().unwrap());
    assert!(book.redo().unwrap());
    assert!(!book.redo().unwrap());
    assert_eq!(book.drawings(0).len(), n - 1);

    // Saved: the chart's new anchor, the picture gone, the rest as it was.
    let out = dir.path().join("saved.xlsx");
    book.save_as(&out).unwrap();
    let xml = drawing_xml(&out);
    assert_eq!(anchors(&xml), n - 1);
    assert!(xml.contains("<xdr:from><xdr:col>30</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>40</xdr:row><xdr:rowOff>28575</xdr:rowOff></xdr:from>"));
    assert!(!xml.contains("srcRect"), "the picture was deleted");
    assert!(crate::package_problems(&std::fs::read(&out).unwrap()).is_empty());
    let again = Workbook::open(&out).unwrap();
    assert_eq!(again.drawings(0).len(), n - 1);

    // Undo past the save, save again: the picture is back where it was.
    assert!(book.undo().unwrap());
    assert!(book.undo().unwrap());
    let out2 = dir.path().join("saved2.xlsx");
    book.save_as(&out2).unwrap();
    let xml = drawing_xml(&out2);
    assert_eq!(anchors(&xml), n);
    assert!(xml.contains("srcRect"));
    let reopened = Workbook::open(&out2).unwrap();
    let d = reopened.drawings(0);
    assert_eq!(d.len(), n);
    assert!(d.iter().any(|x| x.anchor == chart.anchor));
}

#[test]
fn untouched_drawings_round_trip_unchanged() {
    let (mut book, dir) = open_charts();
    let original = drawing_xml(&corpus("20_charts.xlsx"));
    book.set_input(0, 30, 0, "x").unwrap();
    let out = dir.path().join("saved.xlsx");
    book.save_as(&out).unwrap();
    assert_eq!(drawing_xml(&out), original);
}

#[test]
fn edits_and_inserted_rows_combine() {
    let (mut book, dir) = open_charts();
    let chart = book.drawings(0)[2].clone();
    let to = Anchor::OneCell {
        from: Marker {
            col: 8,
            col_off: 0,
            row: 70,
            row_off: 0,
        },
        cx: px(300.0),
        cy: px(200.0),
    };
    book.place_drawing(chart.key, to).unwrap();
    // Rows inserted after the move push the placed chart down.
    book.insert_rows(0, 10, 5).unwrap();
    let Anchor::OneCell { from, .. } = book.drawings(0)[2].anchor else {
        panic!()
    };
    assert_eq!(from.row, 75);
    let out = dir.path().join("saved.xlsx");
    book.save_as(&out).unwrap();
    let xml = drawing_xml(&out);
    assert!(xml.contains("<xdr:oneCellAnchor><xdr:from><xdr:col>8</xdr:col><xdr:colOff>0</xdr:colOff><xdr:row>75</xdr:row>"), "{xml}");
    // The other charts moved with the rows as before.
    let reopened = Workbook::open(&out).unwrap();
    let Anchor::TwoCell { from, .. } = reopened.drawings(0)[3].anchor else {
        panic!()
    };
    assert_eq!(from.row, 24, "F20 moved down by 5");
}

#[test]
fn chart_sheet_objects_fill_the_pane() {
    let d = |index, anchor| Drawing {
        key: DrawingKey { sheet_id: 7, index },
        name: String::new(),
        descr: String::new(),
        anchor,
        kind: Arc::new(DrawingKind::Other("sp".into())),
        editable: false,
        hidden: false,
        prints: true,
    };
    let chart = d(
        0,
        Anchor::Absolute {
            x: 0,
            y: 0,
            cx: 1000,
            cy: 500,
        },
    );
    let label = d(
        1,
        Anchor::Absolute {
            x: 500,
            y: 250,
            cx: 100,
            cy: 50,
        },
    );
    let rects = chart_sheet_rects(&[chart, label], 800.0, 600.0);
    assert_eq!(rects[0], [0.0, 0.0, 800.0, 600.0]);
    assert_eq!(rects[1], [400.0, 300.0, 80.0, 60.0]);
}
