use super::*;

use std::path::Path;

fn open() -> (Workbook, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("charts.xlsx");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/20_charts.xlsx"),
        &path,
    )
    .unwrap();
    (Workbook::open(&path).unwrap(), dir)
}

fn charts(book: &Workbook) -> Vec<Drawing> {
    book.drawings(0)
        .into_iter()
        .filter(|d| matches!(*d.kind, DrawingKind::Chart { .. }))
        .collect()
}

fn texts(items: &[Item]) -> usize {
    items
        .iter()
        .filter(|i| matches!(i.prim, Prim::Text(_)))
        .count()
}

/// Bars of the first series: their heights, in item order.
fn bar_heights(items: &[Item]) -> Vec<f32> {
    items
        .iter()
        .filter_map(|i| match &i.prim {
            Prim::Fill { path, .. } => {
                let p = path.transformed(&i.transform);
                let ys: Vec<f32> = p
                    .segs
                    .iter()
                    .filter_map(|s| match s {
                        slides_scene::Seg::Move(q) | slides_scene::Seg::Line(q) => Some(q.y),
                        _ => None,
                    })
                    .collect();
                let (lo, hi) = ys
                    .iter()
                    .fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(*y), b.max(*y)));
                (ys.len() == 4 || ys.len() == 5).then_some(hi - lo)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn every_chart_of_the_corpus_is_drawn() {
    let (book, _dir) = open();
    let mut engine = Engine::new();
    let mut scenes = DrawingScenes::default();
    let charts = charts(&book);
    assert_eq!(charts.len(), 7);
    for d in &charts {
        let items = scenes.items(&mut engine, &book, d, 480.0, 290.0);
        assert!(
            !items
                .iter()
                .any(|i| matches!(i.prim, Prim::Unsupported { .. })),
            "{} not drawn",
            d.name
        );
        assert!(texts(&items) >= 2, "{} has a title and labels", d.name);
    }
    assert_eq!(scenes.take_builds(), 7);
    // Asking again (scrolling) builds nothing.
    for d in &charts {
        scenes.items(&mut engine, &book, d, 480.0, 290.0);
    }
    assert_eq!(scenes.take_builds(), 0);
}

#[test]
fn charts_follow_their_cells() {
    let (mut book, _dir) = open();
    let mut engine = Engine::new();
    let mut scenes = DrawingScenes::default();
    let column = charts(&book)[0].clone();
    let before = scenes.items(&mut engine, &book, &column, 480.0, 290.0);
    scenes.take_builds();
    // An edit elsewhere: the chart's data is the same, nothing is rebuilt.
    book.set_input(0, 40, 0, "unrelated").unwrap();
    let same = scenes.items(&mut engine, &book, &column, 480.0, 290.0);
    assert!(Arc::ptr_eq(&before, &same));
    assert_eq!(scenes.take_builds(), 0);
    // Jan's sales go up: the chart is rebuilt and its first bar grows.
    book.set_input(0, 1, 1, "400").unwrap();
    let after = scenes.items(&mut engine, &book, &column, 480.0, 290.0);
    assert_eq!(scenes.take_builds(), 1);
    let (b0, b1) = (bar_heights(&before), bar_heights(&after));
    assert!(!b0.is_empty() && b0.len() == b1.len(), "{b0:?} {b1:?}");
    let grew = b0.iter().zip(&b1).any(|(a, b)| b > a);
    assert!(grew, "{b0:?} -> {b1:?}");
    // Rows inserted above the data: same values, same chart.
    book.insert_rows(0, 0, 1).unwrap();
    let moved = charts(&book)[0].clone();
    scenes.items(&mut engine, &book, &moved, 480.0, 290.0);
    assert_eq!(scenes.take_builds(), 0, "the data didn't change");
}

#[test]
fn live_data_replaces_the_caches() {
    let (mut book, _dir) = open();
    let theme = ChartTheme::of(&book);
    let DrawingKind::Chart { xml, .. } = &*charts(&book)[0].kind else {
        panic!()
    };
    let base = theme.parse(xml).unwrap();
    book.set_input(0, 1, 0, "January").unwrap();
    book.set_input(0, 0, 1, "Revenue").unwrap();
    book.set_input(0, 2, 1, "=B2*2").unwrap();
    let (live, _) = live_chart(&base, &book, false);
    let s = &live.groups[0].series[0];
    assert_eq!(s.name.as_deref(), Some("Revenue"));
    assert_eq!(s.cats[0], "January");
    assert_eq!(s.vals[0], Some(120.0));
    assert_eq!(s.vals[1], Some(240.0));
    assert_eq!(s.fmt.as_deref(), Some("#,##0"));
    // The other series keeps its own values.
    assert_eq!(live.groups[0].series[1].vals[0], Some(80.0));
}

#[test]
fn pictures_and_placement() {
    let (book, _dir) = open();
    let mut engine = Engine::new();
    let mut scenes = DrawingScenes::default();
    let pic = book
        .drawings(0)
        .into_iter()
        .find(|d| matches!(*d.kind, DrawingKind::Picture { .. }))
        .unwrap();
    let items = scenes.items(&mut engine, &book, &pic, 60.0, 50.0);
    let Prim::Picture(p) = &items[0].prim else {
        panic!()
    };
    assert_eq!(p.crop, [0.25, 0.25, 0.0, 0.0]);
    assert_eq!(p.rect, [0.0, 0.0, 60.0, 50.0]);
    let moved = translated(&items, 10.0, 20.0);
    let at = moved[0].transform.apply(slides_scene::p(0.0, 0.0));
    assert_eq!((at.x, at.y), (10.0, 20.0));
}

/// A file's chart restyled into every type is still drawn, with its new
/// title, and follows its cells.
#[test]
fn restyled_file_charts_are_drawn() {
    let (mut book, _dir) = open();
    let mut engine = Engine::new();
    let mut scenes = DrawingScenes::default();
    let key = charts(&book)[0].key;
    for kind in sheets_engine::ChartType::ALL {
        let mut spec = book.chart_info(key).unwrap().spec;
        spec.kind = kind;
        spec.title = format!("As {}", kind.label());
        book.set_chart(key, spec).unwrap();
        let d = charts(&book).into_iter().find(|d| d.key == key).unwrap();
        let items = scenes.items(&mut engine, &book, &d, 480.0, 290.0);
        assert!(
            !items
                .iter()
                .any(|i| matches!(i.prim, Prim::Unsupported { .. })),
            "{kind:?} not drawn"
        );
        assert!(texts(&items) >= 2, "{kind:?}");
    }
    // Still live: a changed cell rebuilds it.
    scenes.take_builds();
    book.set_input(0, 1, 1, "999").unwrap();
    let d = charts(&book).into_iter().find(|d| d.key == key).unwrap();
    scenes.items(&mut engine, &book, &d, 480.0, 290.0);
    assert_eq!(scenes.take_builds(), 1);
}

fn strings(items: &[Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match &i.prim {
            Prim::Text(t) => Some(t.runs.iter().map(|r| r.sample()).collect::<String>()),
            _ => None,
        })
        .collect()
}

/// Data labels and axis options set in the Chart panel are drawn.
#[test]
fn data_labels_and_axis_options_are_drawn() {
    let (mut book, _dir) = open();
    let mut engine = Engine::new();
    let mut scenes = DrawingScenes::default();
    let key = charts(&book)[0].key;
    let info = book.chart_info(key).unwrap();
    let mut spec = info.spec.clone();
    spec.kind = sheets_engine::ChartType::Column;
    spec.labels = vec![
        sheets_engine::DataLabels {
            value: true,
            num_fmt: Some("0.0".into()),
            pos: Some(sheets_engine::LabelPos::InsideEnd),
            ..Default::default()
        };
        info.series.len()
    ];
    spec.y_axis = sheets_engine::AxisOptions {
        units: sheets_engine::DisplayUnits::Hundreds,
        ..Default::default()
    };
    book.set_chart(key, spec).unwrap();
    let d = charts(&book).into_iter().find(|d| d.key == key).unwrap();
    let items = scenes.items(&mut engine, &book, &d, 480.0, 290.0);
    let s = strings(&items);
    assert!(s.iter().any(|t| t == "Hundreds"), "{s:?}");
    assert!(
        s.iter()
            .filter(|t| t.contains('.') && t.parse::<f64>().is_ok())
            .count()
            >= 3,
        "value labels with one decimal: {s:?}"
    );
}

/// `23_shapes.xlsx`'s shapes, by name, built into 200 × 100 px.
fn shape_scenes() -> Vec<(String, Arc<Vec<Item>>)> {
    let book = Workbook::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/23_shapes.xlsx"),
    )
    .unwrap();
    let mut engine = Engine::new();
    let mut scenes = DrawingScenes::default();
    book.drawings(0)
        .into_iter()
        .filter(|d| matches!(*d.kind, DrawingKind::Shape { .. }))
        .map(|d| {
            let items = scenes.items(&mut engine, &book, &d, 200.0, 100.0);
            (d.name.clone(), items)
        })
        .collect()
}

fn count(items: &[Item], f: impl Fn(&Prim) -> bool) -> usize {
    items.iter().filter(|i| f(&i.prim)).count()
}

#[test]
fn shapes_draw_fills_lines_arrowheads_and_text() {
    let all = shape_scenes();
    let get = |n: &str| all.iter().find(|(m, _)| m == n).unwrap().1.clone();
    for (name, items) in &all {
        assert!(
            !items
                .iter()
                .any(|i| matches!(i.prim, Prim::Unsupported { .. })),
            "{name} is a placeholder"
        );
    }
    // Solid blue fill, dark blue 2 pt outline (2.67 px), white text.
    let rev = get("Revenue box");
    assert!(rev.iter().any(|i| matches!(
        &i.prim,
        Prim::Fill { paint: slides_scene::Paint::Solid(c), .. } if c.0[..3] == [0x44, 0x72, 0xC4]
    )));
    assert!(rev.iter().any(|i| matches!(
        &i.prim,
        Prim::Stroke { stroke, .. } if (stroke.width - 25400.0 / 9525.0).abs() < 0.01
    )));
    let Some(Prim::Text(t)) = rev
        .iter()
        .map(|i| &i.prim)
        .find(|p| matches!(p, Prim::Text(_)))
    else {
        panic!("no text")
    };
    let run = &t.runs[0].run;
    assert_eq!(run.color.0[..3], [255, 255, 255]);
    // Centred: the text starts well right of the left edge.
    assert!(t.origin.x + run.origin_x > 50.0, "{:?}", t.origin);
    // A gradient, no outline.
    let oval = get("Gradient oval");
    assert!(oval.iter().any(|i| matches!(
        &i.prim,
        Prim::Fill {
            paint: slides_scene::Paint::Linear { .. },
            ..
        }
    )));
    assert_eq!(count(&oval, |p| matches!(p, Prim::Stroke { .. })), 0);
    // No fill, a dashed outline, two paragraphs.
    let frame = get("Dashed frame");
    assert_eq!(count(&frame, |p| matches!(p, Prim::Fill { .. })), 0);
    assert!(frame.iter().any(|i| matches!(
        &i.prim,
        Prim::Stroke { stroke, .. } if stroke.dash.is_some()
    )));
    // A line with an oval head and a triangle tail: two filled heads.
    let line = get("Arrow line");
    assert_eq!(count(&line, |p| matches!(p, Prim::Stroke { .. })), 1);
    assert_eq!(count(&line, |p| matches!(p, Prim::Fill { .. })), 2);
    // Filled by the theme's accent 2 (orange in Office's theme).
    let arrow = get("Turned arrow");
    assert!(arrow.iter().any(|i| matches!(&i.prim, Prim::Fill { .. })));
    let turned = arrow
        .iter()
        .find(|i| matches!(i.prim, Prim::Fill { .. }))
        .unwrap();
    assert!(!turned.transform.is_upright());
    // The group's two shapes.
    let pair = get("Pair");
    assert_eq!(count(&pair, |p| matches!(p, Prim::Fill { .. })), 2);
    // Text boxes and the form button show their text.
    for n in ["Notes box", "Button 1", "Note callout"] {
        assert!(count(&get(n), |p| matches!(p, Prim::Text(_))) > 0, "{n}");
    }
}

#[test]
fn shapes_are_built_once_per_size_and_text() {
    let book = Workbook::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/23_shapes.xlsx"),
    )
    .unwrap();
    let mut book = book;
    let mut engine = Engine::new();
    let mut scenes = DrawingScenes::default();
    let d = book
        .drawings(0)
        .into_iter()
        .find(|d| d.name == "Revenue box")
        .unwrap();
    let a = scenes.items(&mut engine, &book, &d, 200.0, 100.0);
    let b = scenes.items(&mut engine, &book, &d, 200.0, 100.0);
    assert!(Arc::ptr_eq(&a, &b));
    book.set_shape_text(d.key, "Changed").unwrap();
    let d = book
        .drawings(0)
        .into_iter()
        .find(|d| d.name == "Revenue box")
        .unwrap();
    let c = scenes.items(&mut engine, &book, &d, 200.0, 100.0);
    assert!(!Arc::ptr_eq(&a, &c));
}

fn text_colors(items: &[Item]) -> Vec<[u8; 3]> {
    items
        .iter()
        .filter_map(|i| match &i.prim {
            Prim::Text(t) => Some(t.runs.iter().map(|r| {
                let c = r.run.color.0;
                [c[0], c[1], c[2]]
            })),
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn default_text_on_no_fill_reads_on_a_dark_grid() {
    let mut book = Workbook::open(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/xlsx/23_shapes.xlsx"),
    )
    .unwrap();
    let mut engine = Engine::new();
    let mut scenes = DrawingScenes::default();
    let find =
        |book: &Workbook, n: &str| book.drawings(0).into_iter().find(|d| d.name == n).unwrap();
    // A text box with its fill taken off: black text, white on dark.
    let d = find(&book, "Notes box");
    book.edit_shape(d.key, |x| {
        sheets_engine::shape_format::set_fill(x, sheets_engine::ShapeFill::None)
    })
    .unwrap();
    let d = find(&book, "Notes box");
    let light = scenes.items(&mut engine, &book, &d, 200.0, 100.0);
    assert!(
        text_colors(&light).iter().all(|c| *c == [0, 0, 0]),
        "{:?}",
        text_colors(&light)
    );
    scenes.set_dark_text(Some(Rgb([0xEE, 0xEE, 0xEE])));
    let dark = scenes.items(&mut engine, &book, &d, 200.0, 100.0);
    let cs = text_colors(&dark);
    assert!(
        !cs.is_empty() && cs.iter().all(|c| *c == [0xEE, 0xEE, 0xEE]),
        "{cs:?}"
    );
    // White text on a filled shape keeps its colour.
    let rev = scenes.items(
        &mut engine,
        &book,
        &find(&book, "Revenue box"),
        200.0,
        100.0,
    );
    assert!(text_colors(&rev).iter().all(|c| *c == [255, 255, 255]));

fn image(sizing: u8, width: Option<f64>, height: Option<f64>) -> CellImage {
    CellImage {
        source: "https://example.com/a.png".to_string(),
        alt_text: String::new(),
        sizing,
        height,
        width,
        data: Arc::new(vec![1, 2, 3]),
        pixel_width: 200,
        pixel_height: 100,
    }
}

#[test]
fn image_formula_sizing_modes() {
    // A 64 × 20 px cell, a 200 × 100 px picture.
    // 0: fit, keeping the aspect ratio (40 × 20, centred).
    assert_eq!(
        cell_image_rect(&image(0, None, None), 64.0, 20.0),
        [12.0, 0.0, 40.0, 20.0]
    );
    // 1: fill.
    assert_eq!(
        cell_image_rect(&image(1, None, None), 64.0, 20.0),
        [0.0, 0.0, 64.0, 20.0]
    );
    // 2: its own size, centred, cut to the cell.
    assert_eq!(
        cell_image_rect(&image(2, None, None), 64.0, 20.0),
        [-68.0, -40.0, 200.0, 100.0]
    );
    // 3: width and height; one alone keeps the aspect ratio.
    assert_eq!(
        cell_image_rect(&image(3, Some(30.0), Some(10.0)), 64.0, 20.0),
        [17.0, 5.0, 30.0, 10.0]
    );
    assert_eq!(
        cell_image_rect(&image(3, None, Some(10.0)), 64.0, 20.0),
        [22.0, 5.0, 20.0, 10.0]
    );

    let items = cell_image_items(&image(2, None, None), 64.0, 20.0);
    let Prim::Picture(p) = &items[0].prim else {
        panic!("not a picture");
    };
    assert_eq!(p.rect, [0.0, 0.0, 64.0, 20.0]);
    assert_eq!(p.crop, [0.34, 0.4, 0.34, 0.4]);
    // The whole picture still lands where it should.
    let whole = p.image_rect();
    for (a, b) in whole.iter().zip([-68.0, -40.0, 200.0, 100.0]) {
        assert!((a - b).abs() < 1e-3, "{whole:?}");
    }
}
