//! The same document shaped by GPUI's own Linux text system (cosmic-text via
//! `gpui_wgpu::CosmicTextSystem`, used headless) with the greedy line
//! breaking a GPUI-based Docs engine would have to write itself.
//!
//! `cargo run --release -p docs_layout --example gpui_compare`
//!
//! What it shows: GPUI shapes one line at one font size (`layout_line(text,
//! font_size, &[FontRun { len, font_id }])`), so per-run sizes are dropped
//! here; there is no UAX #14 breaking, justification, bidi line reordering
//! or font-bytes access, which is why the comparison stops at line counts.

use std::time::Instant;

use docs_layout::{Engine, PT, PageSetup, Paragraph, sample};
use std::collections::HashMap;

use gpui::{
    FontFallbacks, FontId, FontRun, FontStyle, FontWeight, GlyphId, LineLayout, PlatformTextSystem,
    font, px,
};
use gpui_wgpu::CosmicTextSystem;
use skrifa::raw::TableProvider as _;
use skrifa::{MetadataProvider as _, string::StringId};

fn font_id(ts: &CosmicTextSystem, family: &str, bold: bool, italic: bool) -> FontId {
    let mut f = font(family.to_string());
    f.fallbacks = Some(FontFallbacks::from_fonts(vec![
        "Noto Sans CJK SC".into(),
        "Noto Sans Arabic".into(),
    ]));
    if bold {
        f = f.bold();
    }
    if italic {
        f = f.italic();
    }
    ts.font_id(&f).expect("font resolves")
}

/// Font runs for a paragraph; span sizes cannot be expressed and are ignored.
fn runs(ts: &CosmicTextSystem, p: &Paragraph) -> Vec<FontRun> {
    let mut cuts = vec![0, p.text.len()];
    for s in &p.spans {
        cuts.extend([s.range.start, s.range.end]);
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .map(|w| {
            let span = p
                .spans
                .iter()
                .find(|s| s.range.start <= w[0] && w[1] <= s.range.end);
            let family = span
                .and_then(|s| s.family.as_deref())
                .unwrap_or("Carlito")
                .split(',')
                .next()
                .unwrap_or("Carlito");
            let (bold, italic) = span.map_or((false, false), |s| (s.bold, s.italic));
            FontRun {
                len: w[1] - w[0],
                font_id: font_id(ts, family.trim(), bold, italic),
            }
        })
        .collect()
}

/// Greedy breaking on the shaped glyph positions: break after spaces, or
/// between any two CJK glyphs.
fn count_lines(text: &str, line: &LineLayout, width: f32) -> usize {
    let mut lines = 1;
    let mut line_start_x = 0.0;
    let mut last_break_x = None;
    for run in &line.runs {
        for g in &run.glyphs {
            let x = f32::from(g.position.x);
            let ch = text[g.index..].chars().next().unwrap_or(' ');
            if x - line_start_x > width {
                line_start_x = last_break_x.unwrap_or(x);
                last_break_x = None;
                lines += 1;
            }
            if ch == ' ' || ('\u{3000}'..='\u{9fff}').contains(&ch) {
                last_break_x = Some(x);
            }
        }
    }
    lines
}

/// The crux of painting parley output with GPUI: map a parley font (bytes +
/// collection index) to a GPUI `FontId` for the *same file*, so parley's glyph
/// ids are valid for `Window::paint_glyph`. Resolve by family name, weight and
/// style, then verify identity (cmap and advances agree). Returns
/// (fonts, verified, chars compared).
fn font_bridge(
    ts: &CosmicTextSystem,
    paras: &[Paragraph],
    doc: &docs_layout::PagedDocument,
) -> (usize, usize, usize) {
    // Each parley font with a sample of the text it shaped.
    let mut fonts: HashMap<(u64, u32), (docs_layout::parley::FontData, String)> = HashMap::new();
    for page in 0..doc.pages.len().min(2) {
        for run in doc.glyph_runs(page) {
            let entry = fonts
                .entry((run.font.data.id(), run.font.index))
                .or_insert_with(|| (run.font.clone(), String::new()));
            if entry.1.len() < 64 {
                entry
                    .1
                    .push_str(&paras[run.para].text[run.text_range.clone()]);
            }
        }
    }
    let mut verified = 0;
    let mut compared = 0;
    for (font_data, sample_text) in fonts.values() {
        let f = skrifa::FontRef::from_index(font_data.data.data(), font_data.index)
            .expect("parley font parses");
        let family = f
            .localized_strings(StringId::TYPOGRAPHIC_FAMILY_NAME)
            .english_or_first()
            .or_else(|| {
                f.localized_strings(StringId::FAMILY_NAME)
                    .english_or_first()
            })
            .map(|s| s.to_string())
            .unwrap_or_default();
        let attrs = f.attributes();
        let mut gf = font(family.clone());
        gf.weight = FontWeight(attrs.weight.value());
        gf.style = match attrs.style {
            skrifa::attribute::Style::Normal => FontStyle::Normal,
            _ => FontStyle::Italic,
        };
        // GPUI only hands out a FontId by name for faces that have a Latin
        // 'm' (cosmic_text_system.rs `load_family`), so script-only faces
        // (Arabic, Devanagari...) are reached through its fallback path:
        // shape their text with the family as a fallback and read the run's id.
        let fid = match ts.font_id(&gf) {
            Ok(fid) => fid,
            Err(_) => {
                let mut primary = font("Liberation Sans");
                primary.fallbacks = Some(FontFallbacks::from_fonts(vec![family.clone()]));
                let pid = ts.font_id(&primary).expect("primary font");
                let line = ts.layout_line(
                    sample_text,
                    px(16.0),
                    &[FontRun {
                        len: sample_text.len(),
                        font_id: pid,
                    }],
                );
                match line.runs.iter().map(|r| r.font_id).find(|id| *id != pid) {
                    Some(fid) => fid,
                    None => {
                        println!("  bridge: {family:?} not reachable from GPUI");
                        continue;
                    }
                }
            }
        };
        // Identity check over every mapped code point (sampled).
        let cmap = f.charmap();
        let hmtx = f.hmtx().ok();
        let mut ok = true;
        let mut n = 0;
        for (cp, gid) in cmap.mappings().step_by(97).take(200) {
            let Some(ch) = char::from_u32(cp) else {
                continue;
            };
            n += 1;
            let theirs = ts.glyph_for_char(fid, ch);
            let same_gid = theirs == Some(GlyphId(gid.to_u32()));
            let same_adv = hmtx.as_ref().is_none_or(|h| {
                let adv = h.advance(gid).unwrap_or(0) as f32;
                ts.advance(fid, GlyphId(gid.to_u32()))
                    .is_ok_and(|a| (a.width - adv).abs() < 0.5)
            });
            ok &= same_gid && same_adv;
        }
        compared += n;
        if ok {
            verified += 1;
        }
        println!(
            "  bridge: {family:?} w{} {:?} -> GPUI {fid:?}: {} ({n} code points)",
            attrs.weight.value(),
            attrs.style,
            if ok { "same font file" } else { "MISMATCH" }
        );
    }
    (fonts.len(), verified, compared)
}

fn main() {
    let t = Instant::now();
    let ts = CosmicTextSystem::new("Liberation Sans");
    let mut paras = sample::document(70);
    let width = PageSetup::a4().content_width();
    // Warm the font loading caches.
    let _ = runs(&ts, &paras[1]);
    let init = t.elapsed();

    let mut best = None;
    let mut total_lines = 0;
    for _ in 0..10 {
        let t = Instant::now();
        total_lines = 0;
        for p in &paras {
            let r = runs(&ts, p);
            let layout = ts.layout_line(&p.text, px(p.size_pt * PT), &r);
            total_lines += count_lines(&p.text, &layout, width);
        }
        let e = t.elapsed();
        best = Some(best.map_or(e, |b: std::time::Duration| b.min(e)));
    }

    // parley on the same paragraphs, without pagination, for a like-for-like number.
    let mut engine = Engine::new();
    for p in &mut paras {
        p.align = docs_layout::Align::Start;
    }
    let mut parley_best = None;
    let mut parley_lines = 0;
    for _ in 0..10 {
        let t = Instant::now();
        parley_lines = paras
            .iter()
            .map(|p| engine.layout_paragraph(p, width).len())
            .sum();
        let e = t.elapsed();
        parley_best = Some(parley_best.map_or(e, |b: std::time::Duration| b.min(e)));
    }

    let bridge_paras = sample::document(4);
    let doc = engine.layout_document(&bridge_paras, PageSetup::a4());
    let (fonts, verified, compared) = font_bridge(&ts, &bridge_paras, &doc);
    println!(
        "  font bridge: {verified}/{fonts} parley fonts map to the same GPUI face ({compared} code points checked)"
    );

    println!("{} paragraphs, content width {width:.1} px", paras.len());
    println!(
        "  GPUI/cosmic-text  init {init:.2?}, shape + greedy break {:.2?}, {total_lines} lines",
        best.unwrap_or_default()
    );
    println!(
        "  parley            shape + UAX#14 break  {:.2?}, {parley_lines} lines",
        parley_best.unwrap_or_default()
    );
}
