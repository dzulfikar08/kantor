//! Print layouts: slides (one per page, as [`crate::write`]), notes pages
//! (the slide above its speaker notes) and handouts (1, 2, 3, 4, 6 or 9
//! slides per page; with 3, ruled lines for writing beside each slide), as
//! PowerPoint prints them. Notes pages and handouts are A4 portrait; each
//! slide is scaled into its box, clipped to it and framed with a thin line.

use std::sync::Arc;

use anyhow::Result;
use docs_layout::{MM, PageSetup, Rgba};
use docs_pdf::{Fonts, Images, TextRun, draw_run, fill, metadata};
use krilla::error::KrillaError;
use krilla::geom::{PathBuilder, Rect, Transform};
use krilla::page::PageSettings;
use krilla::paint::{FillRule, Stroke};
use krilla::surface::Surface;
use slides_engine::Presentation;
use slides_scene::{Affine, Builder, Engine};

use crate::{Deck, TO_PT, draw_items_at};

/// How the slides are put on paper.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PrintLayout {
    /// One slide per page at the slide's own size.
    #[default]
    Slides,
    /// The slide on the top half of an A4 page, its speaker notes below.
    Notes,
    /// N slides per A4 page (1, 2, 3, 4, 6 or 9).
    Handouts(u8),
}

impl PrintLayout {
    /// Slides per page the handout layouts offer, as PowerPoint does.
    pub const HANDOUTS: [u8; 6] = [1, 2, 3, 4, 6, 9];

    pub fn label(self) -> String {
        match self {
            Self::Slides => "Full page slides".into(),
            Self::Notes => "Notes pages".into(),
            Self::Handouts(1) => "Handouts, 1 slide per page".into(),
            Self::Handouts(n) => format!("Handouts, {n} slides per page"),
        }
    }

    /// Slides on one page.
    pub fn per_page(self) -> usize {
        match self {
            Self::Slides | Self::Notes => 1,
            Self::Handouts(n) => (n as usize).clamp(1, 9),
        }
    }
}

/// Everything a print needs, ready to encode off the UI thread (`Send`).
#[derive(Clone, Debug)]
pub struct PrintJob {
    pub deck: Deck,
    pub layout: PrintLayout,
    /// For notes pages: each printed slide's notes, laid out in the notes
    /// box (px, origin at the box's top left).
    pub notes: Vec<Vec<TextRun>>,
}

impl PrintJob {
    /// Pages the job prints.
    pub fn pages(&self) -> usize {
        match self.layout {
            PrintLayout::Slides | PrintLayout::Notes => self.deck.slides.len().max(1),
            l => self.deck.slides.len().div_ceil(l.per_page()).max(1),
        }
    }
}

/// A4 portrait, px.
const PAGE_W: f32 = 210.0 * MM;
const PAGE_H: f32 = 297.0 * MM;
const MARGIN: f32 = 15.0 * MM;
const FRAME: Rgba = Rgba([0x80, 0x80, 0x80, 255]);
const RULE: Rgba = Rgba([0xB0, 0xB0, 0xB0, 255]);

/// Build a print job: the shown slides' scenes and, for notes pages, the
/// notes laid out for the notes box.
pub fn job(
    pres: &Presentation,
    builder: &Builder,
    engine: &mut Engine,
    layout: PrintLayout,
) -> PrintJob {
    let deck = crate::deck(pres, builder, engine, false);
    let notes = if layout == PrintLayout::Notes {
        let (_, notes_box) = notes_page(deck.width, deck.height);
        (0..pres.len())
            .filter(|&i| !pres.slide(i).hidden)
            .map(|i| lay_out_notes(engine, &pres.notes_text(i), notes_box[2]))
            .collect()
    } else {
        Vec::new()
    };
    PrintJob {
        deck,
        layout,
        notes,
    }
}

/// [`job`] with a fresh scene builder (headless callers: `kantor convert`).
pub fn job_headless(pres: &Presentation, engine: &mut Engine, layout: PrintLayout) -> PrintJob {
    job(pres, &Builder::new(), engine, layout)
}

/// Notes as 12 pt paragraphs broken at `width` px.
fn lay_out_notes(engine: &mut Engine, text: &str, width: f32) -> Vec<TextRun> {
    let text = text.trim_end();
    if text.is_empty() {
        return Vec::new();
    }
    let paras: Vec<docs_layout::Paragraph> = text
        .split('\n')
        .map(|line| {
            let mut p = docs_layout::Paragraph::new(line);
            p.size_pt = 12.0;
            p.space_after_pt = 4.0;
            p
        })
        .collect();
    let texts: Vec<Arc<str>> = paras.iter().map(|p| Arc::from(p.text.as_str())).collect();
    let setup = PageSetup {
        width,
        // One tall page: the box clips what doesn't fit.
        height: 1.0e6,
        margin_top: 0.0,
        margin_bottom: 0.0,
        margin_left: 0.0,
        margin_right: 0.0,
    };
    let paged = engine.layout_document(&paras, setup);
    (0..paged.pages.len())
        .flat_map(|page| paged.glyph_runs(page))
        .filter(|r| r.source == docs_layout::RunSource::Text)
        .map(|r| TextRun::from_glyph_run(&r, texts[r.para].clone()))
        .collect()
}

/// `[x, y, w, h]` of a `sw` × `sh` slide fitted and centred in `cell`.
fn fit(sw: f32, sh: f32, [x, y, w, h]: [f32; 4]) -> [f32; 4] {
    let k = (w / sw.max(1.0)).min(h / sh.max(1.0));
    let (fw, fh) = (sw * k, sh * k);
    [x + (w - fw) / 2.0, y + (h - fh) / 2.0, fw, fh]
}

/// A notes page: the slide's box and the notes box, px.
fn notes_page(sw: f32, sh: f32) -> ([f32; 4], [f32; 4]) {
    let content_w = PAGE_W - 2.0 * MARGIN;
    let top_h = PAGE_H * 0.45 - MARGIN;
    let slide = fit(sw, sh, [MARGIN, MARGIN, content_w, top_h]);
    let notes_y = slide[1] + slide[3] + 10.0 * MM;
    let notes = [
        MARGIN + 5.0 * MM,
        notes_y,
        content_w - 10.0 * MM,
        PAGE_H - MARGIN - notes_y,
    ];
    (slide, notes)
}

/// Where each slide of a handout page goes, and the writing lines (as
/// `[x, y, w]`) of the 3-per-page layout.
fn handout_cells(n: usize, sw: f32, sh: f32) -> (Vec<[f32; 4]>, Vec<[f32; 3]>) {
    let (cols, rows) = match n {
        1 => (1, 1),
        2 => (1, 2),
        3 => (1, 3),
        4 => (2, 2),
        6 => (2, 3),
        _ => (3, 3),
    };
    let gap = 8.0 * MM;
    let area_w = PAGE_W - 2.0 * MARGIN;
    let area_h = PAGE_H - 2.0 * MARGIN;
    let cw = (area_w - gap * (cols - 1) as f32) / cols as f32;
    let ch = (area_h - gap * (rows - 1) as f32) / rows as f32;
    let mut cells = Vec::new();
    let mut lines = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let x = MARGIN + c as f32 * (cw + gap);
            let y = MARGIN + r as f32 * (ch + gap);
            if n == 3 {
                // Slide on the left half, lines for notes on the right.
                let half = (cw - gap) / 2.0;
                let slide = fit(sw, sh, [x, y, half, ch]);
                // Top-align the slide in its row; lines span its height.
                let slide = [slide[0], y, slide[2], slide[3]];
                cells.push(slide);
                let lx = x + half + gap;
                let count = 6;
                for k in 1..=count {
                    let ly = y + slide[3] * k as f32 / count as f32;
                    lines.push([lx, ly, half]);
                }
            } else {
                cells.push(fit(sw, sh, [x, y, cw, ch]));
            }
        }
    }
    (cells, lines)
}

/// Encode a print job. Returns the PDF and its page count.
pub fn write_job(job: &PrintJob) -> Result<(Vec<u8>, usize)> {
    if job.layout == PrintLayout::Slides {
        return Ok((crate::write(&job.deck)?, job.pages()));
    }
    let bytes = docs_pdf::encode_retrying(|images| encode(job, images))?;
    Ok((bytes, job.pages()))
}

fn rect_path([x, y, w, h]: [f32; 4]) -> Option<krilla::geom::Path> {
    let rect = Rect::from_xywh(x * TO_PT, y * TO_PT, w.max(0.1) * TO_PT, h.max(0.1) * TO_PT)?;
    let mut pb = PathBuilder::new();
    pb.push_rect(rect);
    pb.finish()
}

/// One slide scaled into `rect`, clipped to it and framed.
fn draw_slide(
    surface: &mut Surface,
    deck: &Deck,
    index: usize,
    rect: [f32; 4],
    images: &mut Images,
    fonts: &mut Fonts,
) {
    let Some(scene) = deck.slides.get(index) else {
        return;
    };
    let k = rect[2] / deck.width.max(1.0);
    let place = Affine::scale(k, k).then(Affine::translate(rect[0], rect[1]));
    let Some(clip) = rect_path(rect) else { return };
    surface.push_clip_path(&clip, &FillRule::NonZero);
    // White paper under slides that have no background of their own.
    surface.set_stroke(None);
    surface.set_fill(Some(fill(Rgba([255, 255, 255, 255]))));
    surface.draw_path(&clip);
    draw_items_at(surface, &scene.items, place, images, fonts);
    surface.pop();
    surface.set_fill(None);
    surface.set_stroke(Some(Stroke {
        paint: krilla::color::rgb::Color::new(FRAME.0[0], FRAME.0[1], FRAME.0[2]).into(),
        width: 0.75,
        ..Stroke::default()
    }));
    surface.draw_path(&clip);
    surface.set_stroke(None);
}

fn encode(job: &PrintJob, images: &mut Images) -> Result<Vec<u8>, KrillaError> {
    let deck = &job.deck;
    let mut fonts = Fonts::default();
    let mut pdf = krilla::Document::new();
    pdf.set_metadata(metadata(&deck.title));
    let settings =
        || PageSettings::from_wh(PAGE_W * TO_PT, PAGE_H * TO_PT).expect("positive page size");
    let pages = job.pages();
    let per = job.layout.per_page();
    for p in 0..pages {
        let mut page = pdf.start_page_with(settings());
        let mut surface = page.surface();
        match job.layout {
            PrintLayout::Notes => {
                let (slide, notes) = notes_page(deck.width, deck.height);
                draw_slide(&mut surface, deck, p, slide, images, &mut fonts);
                if let Some(runs) = job.notes.get(p)
                    && let Some(clip) = rect_path(notes)
                {
                    surface.push_clip_path(&clip, &FillRule::NonZero);
                    surface.push_transform(&Transform::from_translate(
                        notes[0] * TO_PT,
                        notes[1] * TO_PT,
                    ));
                    for run in runs {
                        if let Some(font) = fonts.get(&run.font) {
                            draw_run(&mut surface, run, font);
                        }
                    }
                    surface.pop();
                    surface.pop();
                }
            }
            _ => {
                let (cells, lines) = handout_cells(per, deck.width, deck.height);
                for (k, cell) in cells.iter().enumerate() {
                    let index = p * per + k;
                    if index >= deck.slides.len() {
                        break;
                    }
                    draw_slide(&mut surface, deck, index, *cell, images, &mut fonts);
                    if per == 3 {
                        for &[x, y, w] in &lines[k * 6..(k + 1) * 6] {
                            if let Some(path) = rect_path([x, y, w, 0.75]) {
                                surface.set_fill(Some(fill(RULE)));
                                surface.draw_path(&path);
                            }
                        }
                    }
                }
            }
        }
        surface.finish();
        page.finish();
    }
    pdf.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handout_cells_fit_inside_the_page_and_keep_the_aspect() {
        for n in PrintLayout::HANDOUTS {
            let (cells, lines) = handout_cells(n as usize, 1280.0, 720.0);
            assert_eq!(cells.len(), n as usize);
            for [x, y, w, h] in cells {
                assert!(x >= MARGIN - 0.01 && y >= MARGIN - 0.01);
                assert!(x + w <= PAGE_W - MARGIN + 0.01 && y + h <= PAGE_H - MARGIN + 0.01);
                assert!((w / h - 16.0 / 9.0).abs() < 0.01);
            }
            assert_eq!(lines.len(), if n == 3 { 18 } else { 0 });
        }
    }

    #[test]
    fn notes_box_is_below_the_slide() {
        let (slide, notes) = notes_page(1280.0, 720.0);
        assert!(notes[1] > slide[1] + slide[3]);
        assert!(notes[1] + notes[3] <= PAGE_H);
    }
}
