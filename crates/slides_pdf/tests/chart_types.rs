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
