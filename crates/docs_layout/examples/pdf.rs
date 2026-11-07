//! Writes the spike's laid-out pages to a PDF with krilla, proving the
//! glyph-data path (font bytes + glyph ids + positions) end to end.
//!
//! `cargo run --release -p docs_layout --example pdf -- [out.pdf] [sections]`

use std::collections::HashMap;

use docs_layout::{Engine, PageSetup, PagedDocument, sample};
use krilla::Document;
use krilla::color::rgb;
use krilla::geom::Point;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::Fill;
use krilla::text::{Font, GlyphId, KrillaGlyph};

/// px (96 dpi) to PDF points.
const TO_PT: f32 = 72.0 / 96.0;

fn write_pdf(doc: &PagedDocument, texts: &[&str]) -> Vec<u8> {
    let setup = doc.setup;
    let mut fonts: HashMap<(u64, u32), Font> = HashMap::new();
    let mut pdf = Document::new();
    for page_ix in 0..doc.pages.len() {
        let settings = PageSettings::from_wh(setup.width * TO_PT, setup.height * TO_PT)
            .expect("valid page size");
        let mut page = pdf.start_page_with(settings);
        let mut surface = page.surface();
        for run in doc.glyph_runs(page_ix) {
            let font = fonts
                .entry((run.font.data.id(), run.font.index))
                .or_insert_with(|| {
                    let bytes = run.font.data.data().to_vec();
                    Font::new(bytes.into(), run.font.index).expect("parley font loads in krilla")
                })
                .clone();
            let [r, g, b, _] = run.color.0;
            surface.set_fill(Some(Fill {
                paint: rgb::Color::new(r, g, b).into(),
                opacity: NormalizedF32::ONE,
                rule: Default::default(),
            }));
            // krilla wants advances/offsets in font-size units relative to a pen.
            let size = run.font_size;
            if run.glyphs.is_empty() {
                continue;
            }
            let mut pen = run.origin_x;
            let glyphs: Vec<KrillaGlyph> = run
                .glyphs
                .iter()
                .map(|g| {
                    // Advances already include justification space.
                    let glyph = KrillaGlyph::new(
                        GlyphId::new(g.id),
                        g.advance / size,
                        (g.x - pen) / size,
                        (g.y - run.baseline) / size,
                        0.0,
                        g.cluster.clone(),
                        None,
                    );
                    pen += g.advance;
                    glyph
                })
                .collect();
            let text = texts[run.para];
            surface.draw_glyphs(
                Point::from_xy(run.origin_x * TO_PT, run.baseline * TO_PT),
                &glyphs,
                font,
                text,
                size * TO_PT,
                false,
            );
        }
        surface.finish();
        page.finish();
    }
    pdf.finish().expect("pdf serialises")
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .unwrap_or_else(|| "docs_layout_spike.pdf".into());
    let sections: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(12);

    let paras = sample::document(sections);
    let mut engine = Engine::new();
    let doc = engine.layout_document(&paras, PageSetup::a4());
    let texts: Vec<&str> = paras.iter().map(|p| p.text.as_str()).collect();

    let t = std::time::Instant::now();
    let bytes = write_pdf(&doc, &texts);
    let elapsed = t.elapsed();
    std::fs::write(&out, &bytes).expect("write pdf");
    println!(
        "wrote {out}: {} pages, {:.1} KB, krilla export {:.2?}",
        doc.pages.len(),
        bytes.len() as f64 / 1024.0,
        elapsed
    );
}
