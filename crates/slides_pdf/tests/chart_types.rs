//! Radar, bubble, stock, combo and 3-D charts (tests/corpus/pptx/charts.pptx
//! from scripts/corpus/gen_pptx_charts.py) are drawn on the slide and in
//! the PDF, not as placeholders.

use std::path::PathBuf;

use slides_engine::Presentation;
use slides_scene::{Builder, Engine, Prim};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/pptx")
        .join(name)
}

fn words(items: &[slides_scene::Item]) -> Vec<String> {
    items
        .iter()
        .filter_map(|i| match &i.prim {
            Prim::Text(t) => Some(t.runs.iter().map(|r| r.sample()).collect::<String>()),
            _ => None,
        })
        .collect()
}

#[test]
fn every_chart_type_in_the_fixture_is_drawn() {
    let pres = Presentation::open(&corpus("charts.pptx")).unwrap();
    assert_eq!(pres.len(), 6);
    assert!(
        pres.unsupported_objects().is_empty(),
        "{:?}",
        pres.unsupported_objects()
    );
    let mut engine = Engine::new();
    let expect: [&[&str]; 6] = [
        &["Speed", "Comfort", "Model A", "Model B"],
        &["North", "Coast", "2025"],
        &["Retail", "Online"],
        &["Mon", "Fri", "High", "Close"],
        &["Q1", "Q4", "Revenue", "Margin", "20%"],
        &["Rent", "Food", "Travel"],
    ];
    for (k, want) in expect.iter().enumerate() {
        let scene = Builder::new().slide(&mut engine, &pres, k, &Default::default());
        assert!(
            !scene
                .items
                .iter()
                .any(|i| matches!(i.prim, Prim::Unsupported { .. })),
            "slide {} has a placeholder",
            k + 1
        );
        let w = words(&scene.items);
        for t in *want {
            assert!(w.iter().any(|x| x == t), "slide {}: {t} in {w:?}", k + 1);
        }
    }
    let (pdf, _) = slides_pdf::export(&pres, &mut engine).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    // For a look at the pages: KANTOR_CHART_PDF=/path/out.pdf.
    if let Ok(out) = std::env::var("KANTOR_CHART_PDF") {
        std::fs::write(out, &pdf).unwrap();
    }
}

/// Surface, pie of pie, bar of pie and the Office 2016 types
/// (tests/corpus/pptx/chartex.pptx from scripts/corpus/gen_pptx_chartex.py).
#[test]
fn surface_of_pie_and_chartex_types_are_drawn() {
    let pres = Presentation::open(&corpus("chartex.pptx")).unwrap();
    assert_eq!(pres.len(), 11);
    assert!(
        pres.unsupported_objects().is_empty(),
        "{:?}",
        pres.unsupported_objects()
    );
    let mut engine = Engine::new();
    let expect: [&[&str]; 11] = [
        &["Jan", "May", "Low", "High", "10-20"],
        &["Mid", "40-50"],
        &["Rent", "Books", "Other"],
        &["Games", "Other"],
        &["Cash flow", "Start", "End", "Increase", "-30"],
        &["Pipeline", "Leads", "Deals", "500"],
        &["Sales by store", "North", "Alpha"],
        &["Sales by area", "Europe", "Asia"],
        &["Scores", "[12, 32]", "(72, 92]"],
        &["Complaints", "Late", "Price", "100%"],
        &["Waiting times", "Mon", "Tue", "Morning", "Evening"],
    ];
    for (k, want) in expect.iter().enumerate() {
        let scene = Builder::new().slide(&mut engine, &pres, k, &Default::default());
        assert!(
            !scene
                .items
                .iter()
                .any(|i| matches!(i.prim, Prim::Unsupported { .. })),
            "slide {} has a placeholder",
            k + 1
        );
        let w = words(&scene.items);
        for t in *want {
            assert!(w.iter().any(|x| x == t), "slide {}: {t} in {w:?}", k + 1);
        }
    }
    let (pdf, _) = slides_pdf::export(&pres, &mut engine).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    if let Ok(out) = std::env::var("KANTOR_CHARTEX_PDF") {
        std::fs::write(out, &pdf).unwrap();
    }
}

/// A moved chartex frame is saved with its new place (the choice's
/// `p:xfrm`) and the chart part kept.
#[test]
fn chartex_frames_move_and_save() {
    let mut pres = Presentation::open(&corpus("chartex.pptx")).unwrap();
    let slide = 4;
    let i = pres.slides()[slide]
        .shapes
        .iter()
        .position(|s| s.is_chart())
        .expect("the waterfall frame");
    assert!(pres.slides()[slide].shapes[i].movable());
    assert!(
        pres.chart_series_fixed(slide, i),
        "its data is edited in the panel, its series stay"
    );
    let mut x = pres.slides()[slide].shapes[i].xfrm.unwrap();
    x.x += 91440;
    pres.set_xfrm((slide, i), x, false).unwrap();
    let bytes = pres.to_bytes().unwrap();
    let back = Presentation::from_bytes(bytes).unwrap();
    let s = back.slides()[slide]
        .shapes
        .iter()
        .find(|s| s.is_chart())
        .unwrap();
    assert_eq!(s.xfrm.unwrap().x, x.x);
    let mut engine = Engine::new();
    let scene = Builder::new().slide(&mut engine, &back, slide, &Default::default());
    assert!(words(&scene.items).iter().any(|w| w == "Cash flow"));
}
