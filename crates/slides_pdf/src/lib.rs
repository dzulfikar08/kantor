//! Kantor Slides → PDF (no GPUI).
//!
//! Each slide's [`slides_scene::SlideScene`], the display list the canvas
//! paints, is written with [krilla] as one page at the presentation's
//! slide size: real gradients (PDF shadings), dashes, caps, joins,
//! arrowheads, pictures with cropping and opacity, and text with embedded
//! subset fonts and ToUnicode (it can be searched and copied). Unlike the
//! screen, the PDF turns text and pictures with their shapes.
//!
//! Export is two steps so the slow one can leave the UI thread:
//! [`deck`] builds the scenes (needs the presentation and the layout
//! engine), [`write`] encodes them (`Deck` is `Send`). [`export`] does
//! both for headless callers (`kantor convert`).
//!
//! Left out: objects Kantor can't draw yet (tables, charts, groups,
//! SmartArt: the space stays empty), pictures krilla can't embed (EMF,
//! WMF, SVG, TIFF), hidden slides unless asked for.

use std::sync::Arc;

use anyhow::Result;
use docs_pdf::{Fonts, Images, TextRun, draw_run, fill, metadata};
use krilla::color::rgb;
use krilla::error::KrillaError;
use krilla::geom::{PathBuilder, Rect, Size, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{
    Fill, FillRule as KFillRule, LineCap as KCap, LineJoin as KJoin, LinearGradient,
    RadialGradient, SpreadMethod, Stop as KStop, Stroke as KStroke, StrokeDash,
};
use krilla::surface::Surface;
use slides_engine::{LineCap, LineJoin, Presentation};
use slides_scene::{
    Affine, Builder, Engine, FillRule, Item, Options, Paint, Path, Picture, Prim, Rgba, Seg,
    SlideScene, Stop, Stroke, Text,
};

pub use docs_pdf::write_file;

pub mod print;
pub use print::{PrintJob, PrintLayout};

/// px (96 dpi, the scene's unit) to PDF points.
const TO_PT: f32 = 72.0 / 96.0;

/// A presentation's slides ready to encode.
#[derive(Clone, Debug)]
pub struct Deck {
    pub title: String,
    /// Slide size, px.
    pub width: f32,
    pub height: f32,
    pub slides: Vec<SlideScene>,
}

/// The scenes of every slide to print; hidden slides only when
/// `include_hidden`.
pub fn deck(
    pres: &Presentation,
    builder: &Builder,
    engine: &mut Engine,
    include_hidden: bool,
) -> Deck {
    let slides = (0..pres.len())
        .filter(|&i| include_hidden || !pres.slide(i).hidden)
        .map(|i| builder.slide(engine, pres, i, &Options::default()))
        .collect();
    Deck {
        title: pres.title(),
        width: pres.size.cx as f32 / slides_engine::EMU_PER_PX,
        height: pres.size.cy as f32 / slides_engine::EMU_PER_PX,
        slides,
    }
}

/// Build and encode a presentation (hidden slides left out). Returns the
/// PDF and its page count.
pub fn export(pres: &Presentation, engine: &mut Engine) -> Result<(Vec<u8>, usize)> {
    let d = deck(pres, &Builder::new(), engine, false);
    let pages = d.slides.len().max(1);
    Ok((write(&d)?, pages))
}

/// Encode a deck: one page per slide (one blank page when it has none).
pub fn write(deck: &Deck) -> Result<Vec<u8>> {
    docs_pdf::encode_retrying(|images| encode(deck, images))
}

fn encode(deck: &Deck, images: &mut Images) -> Result<Vec<u8>, KrillaError> {
    let mut fonts = Fonts::default();
    let mut pdf = krilla::Document::new();
    pdf.set_metadata(metadata(&deck.title));
    let size = || {
        PageSettings::from_wh(
            (deck.width * TO_PT).max(1.0),
            (deck.height * TO_PT).max(1.0),
        )
        .expect("positive page size")
    };
    if deck.slides.is_empty() {
        let mut page = pdf.start_page_with(size());
        page.surface().finish();
        page.finish();
    }
    for scene in &deck.slides {
        let mut page = pdf.start_page_with(size());
        let mut surface = page.surface();
        draw_items(&mut surface, &scene.items, images, &mut fonts);
        surface.finish();
        page.finish();
    }
    pdf.finish()
}

/// Draw scene items (in px) on a surface whose user space is in points:
/// the items land at px × 0.75 from its origin. Shared with Sheets, which
/// prints worksheet charts and pictures this way.
pub fn draw_items(surface: &mut Surface, items: &[Item], images: &mut Images, fonts: &mut Fonts) {
    draw_items_at(surface, items, Affine::IDENTITY, images, fonts);
}

/// [`draw_items`], each item first moved by `place` (px to px), e.g. a
/// slide scaled into a cell of a handout page.
pub fn draw_items_at(
    surface: &mut Surface,
    items: &[Item],
    place: Affine,
    images: &mut Images,
    fonts: &mut Fonts,
) {
    for item in items {
        let item_t = item.transform.then(place);
        let to_page = item_t.then(Affine::scale(TO_PT, TO_PT));
        match &item.prim {
            Prim::Fill { path, paint, rule } => {
                surface.push_transform(&transform(&to_page));
                fill_path(surface, path, paint, *rule);
                surface.pop();
            }
            Prim::Stroke { path, stroke } => {
                surface.push_transform(&transform(&to_page));
                stroke_path(surface, path, stroke);
                surface.pop();
            }
            Prim::Picture(pic) => {
                surface.push_transform(&transform(&to_page));
                draw_picture(surface, pic, images);
                surface.pop();
            }
            Prim::Text(text) => draw_text(surface, text, &item_t, fonts),
            Prim::Unsupported { .. } | Prim::PlaceholderOutline { .. } => {}
        }
    }
}

fn transform(a: &Affine) -> Transform {
    Transform::from_row(a.a, a.b, a.c, a.d, a.e, a.f)
}

fn kpath(path: &Path) -> Option<krilla::geom::Path> {
    let mut pb = PathBuilder::new();
    for s in &path.segs {
        match *s {
            Seg::Move(q) => pb.move_to(q.x, q.y),
            Seg::Line(q) => pb.line_to(q.x, q.y),
            Seg::Quad(c, q) => pb.quad_to(c.x, c.y, q.x, q.y),
            Seg::Cubic(a, b, q) => pb.cubic_to(a.x, a.y, b.x, b.y, q.x, q.y),
            Seg::Close => pb.close(),
        }
    }
    pb.finish()
}

fn color(c: Rgba) -> (rgb::Color, NormalizedF32) {
    let [r, g, b, a] = c.0;
    (
        rgb::Color::new(r, g, b),
        NormalizedF32::new(a as f32 / 255.0).unwrap_or(NormalizedF32::ONE),
    )
}

fn stops(stops: &[Stop]) -> Vec<KStop> {
    stops
        .iter()
        .map(|s| {
            let (c, o) = color(s.color);
            KStop {
                offset: NormalizedF32::new(s.offset.clamp(0.0, 1.0)).unwrap_or(NormalizedF32::ZERO),
                color: c.into(),
                opacity: o,
            }
        })
        .collect()
}

fn rule(r: FillRule) -> KFillRule {
    match r {
        FillRule::NonZero => KFillRule::NonZero,
        FillRule::EvenOdd => KFillRule::EvenOdd,
    }
}

fn fill_path(surface: &mut Surface, path: &Path, paint: &Paint, r: FillRule) {
    let Some(kp) = kpath(path) else { return };
    let f = match paint {
        Paint::Solid(c) => Fill {
            rule: rule(r),
            ..fill(*c)
        },
        Paint::Linear { from, to, stops: s } => Fill {
            paint: LinearGradient {
                x1: from.x,
                y1: from.y,
                x2: to.x,
                y2: to.y,
                transform: Transform::identity(),
                spread_method: SpreadMethod::Pad,
                stops: stops(s),
                anti_alias: true,
            }
            .into(),
            opacity: NormalizedF32::ONE,
            rule: rule(r),
        },
        Paint::Radial {
            center,
            rx,
            ry,
            stops: s,
        } => {
            // A circle of radius rx, squeezed to ry vertically.
            let k = ry / rx.max(1e-3);
            let squeeze = Transform::from_row(1.0, 0.0, 0.0, k, 0.0, center.y * (1.0 - k));
            Fill {
                paint: RadialGradient {
                    fx: center.x,
                    fy: center.y,
                    fr: 0.0,
                    cx: center.x,
                    cy: center.y,
                    cr: rx.max(1e-3),
                    transform: squeeze,
                    spread_method: SpreadMethod::Pad,
                    stops: stops(s),
                    anti_alias: true,
                }
                .into(),
                opacity: NormalizedF32::ONE,
                rule: rule(r),
            }
        }
    };
    surface.set_fill(Some(f));
    surface.set_stroke(None);
    surface.draw_path(&kp);
    surface.set_fill(None);
}

fn stroke_path(surface: &mut Surface, path: &Path, s: &Stroke) {
    let Some(kp) = kpath(path) else { return };
    let (c, opacity) = color(s.color);
    surface.set_fill(None);
    surface.set_stroke(Some(KStroke {
        paint: c.into(),
        width: s.width,
        miter_limit: 8.0,
        line_cap: match s.cap {
            LineCap::Flat => KCap::Butt,
            LineCap::Round => KCap::Round,
            LineCap::Square => KCap::Square,
        },
        line_join: match s.join {
            LineJoin::Round => KJoin::Round,
            LineJoin::Bevel => KJoin::Bevel,
            LineJoin::Miter => KJoin::Miter,
        },
        opacity,
        dash: s.dash.as_ref().map(|d| StrokeDash {
            array: d.clone(),
            offset: 0.0,
        }),
    }));
    surface.draw_path(&kp);
    surface.set_stroke(None);
}

fn draw_picture(surface: &mut Surface, pic: &Picture, images: &mut Images) {
    let data: Arc<dyn AsRef<[u8]> + Send + Sync> = pic.data.clone();
    let Some(image) = images.get(data) else {
        return;
    };
    let [x, y, w, h] = pic.image_rect();
    let Some(size) = Size::from_wh(w.max(1e-3), h.max(1e-3)) else {
        return;
    };
    let mut pushed = 0;
    let clip = match &pic.clip {
        Some(c) => kpath(c),
        None if pic.cropped() => {
            let [bx, by, bw, bh] = pic.rect;
            Rect::from_xywh(bx, by, bw.max(1e-3), bh.max(1e-3)).and_then(|r| {
                let mut pb = PathBuilder::new();
                pb.push_rect(r);
                pb.finish()
            })
        }
        None => None,
    };
    if let Some(c) = &clip {
        surface.push_clip_path(c, &KFillRule::NonZero);
        pushed += 1;
    }
    if pic.alpha < 0.999 {
        surface.push_opacity(
            NormalizedF32::new(pic.alpha.clamp(0.0, 1.0)).unwrap_or(NormalizedF32::ONE),
        );
        pushed += 1;
    }
    surface.push_transform(&Transform::from_translate(x, y));
    surface.draw_image(image, size);
    surface.pop();
    for _ in 0..pushed {
        surface.pop();
    }
}

/// Text: `docs_pdf` draws runs in px scaled to pt, so the shape's turn is
/// given in pt (the px transform conjugated by the px → pt scale).
fn draw_text(surface: &mut Surface, text: &Text, t: &Affine, fonts: &mut Fonts) {
    let local = Affine::translate(text.origin.x, text.origin.y).then(*t);
    let to_pt = Affine::scale(1.0 / TO_PT, 1.0 / TO_PT)
        .then(local)
        .then(Affine::scale(TO_PT, TO_PT));
    surface.push_transform(&transform(&to_pt));
    for r in &text.runs {
        let Some(font) = fonts.get(&r.run.font) else {
            continue;
        };
        draw_run(
            surface,
            &TextRun::from_glyph_run(&r.run, r.text.clone()),
            font,
        );
    }
    surface.pop();
}
