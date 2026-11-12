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
