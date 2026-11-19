//! Bold in a font installed only as a variable font: the PDF embeds the bold
//! instance as its own static font, named like it (`…-Bold`), not the
//! variable font's default (regular) outlines. Uses the committed test font
//! (tests/fonts/KantorVarTest.ttf), never system fonts.

use std::sync::Arc;

use docs_layout::parley::{FontData, fontique::Blob};
use docs_layout::{PositionedGlyph, Rgba};
use docs_pdf::{Fonts, TextRun, draw_run};
use krilla::page::PageSettings;
use skrifa::MetadataProvider as _;

fn fixture() -> FontData {
    let bytes = include_bytes!("../../../tests/fonts/KantorVarTest.ttf").to_vec();
    FontData::new(Blob::new(Arc::new(bytes)), 0)
}

fn run(font: &FontData, variations: Vec<([u8; 4], f32)>, baseline: f32) -> TextRun {
    let f = skrifa::FontRef::new(font.data.data()).unwrap();
    let gid = f.charmap().map('m').unwrap().to_u32();
    TextRun {
        font: font.clone(),
        font_size: 24.0,
        color: Rgba([0, 0, 0, 255]),
        variations,
        embolden: false,
        skew_deg: None,
        origin_x: 10.0,
        baseline,
        advance: 20.0,
        underline: None,
        glyphs: vec![PositionedGlyph {
            id: gid,
            x: 10.0,
            y: baseline,
            advance: 20.0,
            cluster: 0..1,
        }],
        text: Arc::from("m"),
    }
}

#[test]
fn bold_of_a_variable_font_embeds_the_bold_instance() {
    let font = fixture();
    let mut fonts = Fonts::default();
    let mut pdf = krilla::Document::new();
    let mut page = pdf.start_page_with(PageSettings::from_wh(200.0, 100.0).unwrap());
    let mut surface = page.surface();
    for (vars, y) in [(vec![(*b"wght", 700.0)], 30.0), (vec![], 60.0)] {
        let r = run(&font, vars, y);
        let k = fonts.get(&r.font, &r.variations).expect("font");
        draw_run(&mut surface, &r, k);
    }
    surface.finish();
    page.finish();
    let bytes = pdf.finish().expect("pdf");
    let has = |name: &[u8]| bytes.windows(name.len()).any(|w| w == name);
    assert!(has(b"KantorVarTest-Bold"), "bold instance embedded");
    assert!(has(b"KantorVarTest-Regular"), "regular embedded too");
}
