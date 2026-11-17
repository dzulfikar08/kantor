//! Metafile pictures (WMF, EMF, PICT) drawn into a PDF as vector graphics:
//! `kantor_metafile` plays them into SVG, usvg reads that into a tree
//! (text already turned into outlines), and this walks the tree into
//! krilla paths, clips, gradients, patterns and embedded bitmaps.
//!
//! Covers what `kantor_metafile` writes: paths with fills and strokes,
//! clip paths (nested ones intersect), linear and radial gradients,
//! patterns (hatches, tiled brushes), bitmaps, group opacity and blend
//! modes. Masks and filters (never written by it) are left out.

use std::sync::Arc;

use kantor_metafile::usvg;
use krilla::blend::BlendMode;
use krilla::color::rgb;
use krilla::geom::{Path, PathBuilder, Rect, Size, Transform};
use krilla::image::Image;
use krilla::num::NormalizedF32;
use krilla::paint::{
    Fill, FillRule, LineCap, LineJoin, LinearGradient, Paint, Pattern, RadialGradient,
    SpreadMethod, Stop, Stroke, StrokeDash,
};
use krilla::surface::Surface;

/// Draw `tree` stretched to `size` (pt) at the current origin, clipped to
/// that box as an SVG viewport is.
pub fn draw_tree(surface: &mut Surface, tree: &usvg::Tree, size: Size) {
    let natural = tree.size();
    let (nw, nh) = (natural.width().max(1e-3), natural.height().max(1e-3));
    let clip = Rect::from_xywh(0.0, 0.0, size.width(), size.height()).and_then(rect_path);
    if let Some(c) = &clip {
        surface.push_clip_path(c, &FillRule::NonZero);
    }
    surface.push_transform(&Transform::from_scale(
        size.width() / nw,
        size.height() / nh,
    ));
    draw_group(surface, tree.root());
    surface.pop();
    if clip.is_some() {
        surface.pop();
    }
}

fn rect_path(r: Rect) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.push_rect(r);
    pb.finish()
}

fn transform(t: usvg::Transform) -> Transform {
    Transform::from_row(t.sx, t.ky, t.kx, t.sy, t.tx, t.ty)
}

fn path(p: &usvg::tiny_skia_path::Path) -> Option<Path> {
    use usvg::tiny_skia_path::PathSegment as S;
    let mut pb = PathBuilder::new();
    for seg in p.segments() {
        match seg {
            S::MoveTo(a) => pb.move_to(a.x, a.y),
            S::LineTo(a) => pb.line_to(a.x, a.y),
            S::QuadTo(a, b) => pb.quad_to(a.x, a.y, b.x, b.y),
            S::CubicTo(a, b, c) => pb.cubic_to(a.x, a.y, b.x, b.y, c.x, c.y),
            S::Close => pb.close(),
        }
    }
    pb.finish()
}

fn rule(r: usvg::FillRule) -> FillRule {
    match r {
        usvg::FillRule::NonZero => FillRule::NonZero,
        usvg::FillRule::EvenOdd => FillRule::EvenOdd,
    }
}

fn norm(v: f32) -> NormalizedF32 {
    NormalizedF32::new(v.clamp(0.0, 1.0)).unwrap_or(NormalizedF32::ONE)
}

fn blend(b: usvg::BlendMode) -> BlendMode {
    use usvg::BlendMode as B;
    match b {
        B::Normal => BlendMode::Normal,
        B::Multiply => BlendMode::Multiply,
        B::Screen => BlendMode::Screen,
        B::Overlay => BlendMode::Overlay,
        B::Darken => BlendMode::Darken,
        B::Lighten => BlendMode::Lighten,
        B::ColorDodge => BlendMode::ColorDodge,
        B::ColorBurn => BlendMode::ColorBurn,
        B::HardLight => BlendMode::HardLight,
        B::SoftLight => BlendMode::SoftLight,
        B::Difference => BlendMode::Difference,
        B::Exclusion => BlendMode::Exclusion,
        B::Hue => BlendMode::Hue,
        B::Saturation => BlendMode::Saturation,
        B::Color => BlendMode::Color,
        B::Luminosity => BlendMode::Luminosity,
    }
}

/// Push the clip path `clip` (and the ones it is clipped by); returns how
/// many were pushed.
fn push_clip(surface: &mut Surface, clip: &usvg::ClipPath) -> usize {
    let mut pushed = clip
        .clip_path()
        .map_or(0, |inner| push_clip(surface, inner));
    // One path per clip (what kantor_metafile writes); several are joined
    // into one non-zero path.
    let mut paths: Vec<(&usvg::Path, usvg::Transform)> = Vec::new();
    collect_paths(clip.root(), usvg::Transform::default(), &mut paths);
    let (outline, fill_rule) = match paths.as_slice() {
        [(p, t)] => (
            p.data().clone().transform(*t),
            p.fill().map_or(FillRule::NonZero, |f| rule(f.rule())),
        ),
        _ => {
            let mut pb = usvg::tiny_skia_path::PathBuilder::new();
            for (p, t) in &paths {
                if let Some(d) = p.data().clone().transform(*t) {
                    pb.push_path(&d);
                }
            }
            (pb.finish(), FillRule::NonZero)
        }
    };
    surface.push_transform(&transform(clip.transform()));
    // An empty clip hides everything.
    let empty = || Rect::from_xywh(0.0, 0.0, 0.001, 0.001).and_then(rect_path);
    let shape = outline.as_ref().and_then(path).or_else(empty);
    if let Some(shape) = shape {
        surface.push_clip_path(&shape, &fill_rule);
        pushed += 1;
    }
    surface.pop();
    pushed
}

fn collect_paths<'a>(
    g: &'a usvg::Group,
    t: usvg::Transform,
    out: &mut Vec<(&'a usvg::Path, usvg::Transform)>,
) {
    let t = t.pre_concat(g.transform());
    for node in g.children() {
        match node {
            usvg::Node::Path(p) => out.push((p, t)),
            usvg::Node::Group(g) => collect_paths(g, t, out),
            usvg::Node::Text(text) => collect_paths(text.flattened(), t, out),
            usvg::Node::Image(_) => {}
        }
    }
}

fn draw_group(surface: &mut Surface, g: &usvg::Group) {
    let mut pushed = 1;
    surface.push_transform(&transform(g.transform()));
    if let Some(clip) = g.clip_path() {
        pushed += push_clip(surface, clip);
    }
    if g.blend_mode() != usvg::BlendMode::Normal {
        surface.push_blend_mode(blend(g.blend_mode()));
        pushed += 1;
    }
    if g.opacity().get() < 1.0 {
        surface.push_opacity(norm(g.opacity().get()));
        pushed += 1;
    }
    for node in g.children() {
        match node {
            usvg::Node::Group(g) => draw_group(surface, g),
            usvg::Node::Path(p) => draw_path(surface, p),
            usvg::Node::Image(i) => draw_image(surface, i),
            usvg::Node::Text(t) => draw_group(surface, t.flattened()),
        }
    }
    for _ in 0..pushed {
        surface.pop();
    }
}

fn stops(s: &[usvg::Stop]) -> Vec<Stop> {
    s.iter()
        .map(|s| Stop {
            offset: norm(s.offset().get()),
            color: rgb::Color::new(s.color().red, s.color().green, s.color().blue).into(),
            opacity: norm(s.opacity().get()),
        })
        .collect()
}

fn spread(s: usvg::SpreadMethod) -> SpreadMethod {
    match s {
        usvg::SpreadMethod::Pad => SpreadMethod::Pad,
        usvg::SpreadMethod::Reflect => SpreadMethod::Reflect,
        usvg::SpreadMethod::Repeat => SpreadMethod::Repeat,
    }
}

fn paint(surface: &mut Surface, p: &usvg::Paint) -> Paint {
    match p {
        usvg::Paint::Color(c) => rgb::Color::new(c.red, c.green, c.blue).into(),
        usvg::Paint::LinearGradient(g) => LinearGradient {
            x1: g.x1(),
            y1: g.y1(),
            x2: g.x2(),
            y2: g.y2(),
            transform: transform(g.transform()),
            spread_method: spread(g.spread_method()),
            stops: stops(g.stops()),
            anti_alias: true,
        }
        .into(),
        usvg::Paint::RadialGradient(g) => RadialGradient {
            fx: g.fx(),
            fy: g.fy(),
            fr: 0.0,
            cx: g.cx(),
            cy: g.cy(),
            cr: g.r().get(),
            transform: transform(g.transform()),
            spread_method: spread(g.spread_method()),
            stops: stops(g.stops()),
            anti_alias: true,
        }
        .into(),
        usvg::Paint::Pattern(pat) => {
            let r = pat.rect();
            let mut builder = surface.stream_builder();
            let mut tile = builder.surface();
            draw_group(&mut tile, pat.root());
            tile.finish();
            let stream = builder.finish();
            Pattern {
                stream,
                transform: transform(pat.transform().pre_translate(r.x(), r.y())),
                width: r.width(),
                height: r.height(),
            }
            .into()
        }
    }
}

fn draw_path(surface: &mut Surface, p: &usvg::Path) {
    if !p.is_visible() {
        return;
    }
    let Some(shape) = path(p.data()) else {
        return;
    };
    let fill = |surface: &mut Surface| {
        if let Some(f) = p.fill() {
            let paint = paint(surface, f.paint());
            surface.set_fill(Some(Fill {
                paint,
                opacity: norm(f.opacity().get()),
                rule: rule(f.rule()),
            }));
            surface.set_stroke(None);
            surface.draw_path(&shape);
        }
    };
    let stroke = |surface: &mut Surface| {
        if let Some(s) = p.stroke() {
            let paint = paint(surface, s.paint());
            surface.set_fill(None);
            surface.set_stroke(Some(Stroke {
                paint,
                width: s.width().get(),
                miter_limit: s.miterlimit().get(),
                line_cap: match s.linecap() {
                    usvg::LineCap::Butt => LineCap::Butt,
                    usvg::LineCap::Round => LineCap::Round,
                    usvg::LineCap::Square => LineCap::Square,
                },
                line_join: match s.linejoin() {
                    usvg::LineJoin::Round => LineJoin::Round,
                    usvg::LineJoin::Bevel => LineJoin::Bevel,
                    usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => LineJoin::Miter,
                },
                opacity: norm(s.opacity().get()),
                dash: s.dasharray().map(|d| StrokeDash {
                    array: d.to_vec(),
                    offset: s.dashoffset(),
                }),
            }));
            surface.draw_path(&shape);
        }
    };
    match p.paint_order() {
        usvg::PaintOrder::FillAndStroke => {
            fill(surface);
            stroke(surface);
        }
        usvg::PaintOrder::StrokeAndFill => {
            stroke(surface);
            fill(surface);
        }
    }
    surface.set_fill(None);
    surface.set_stroke(None);
}

fn draw_image(surface: &mut Surface, i: &usvg::Image) {
    if !i.is_visible() {
        return;
    }
    let size = i.size();
    let Some(sz) = Size::from_wh(size.width(), size.height()) else {
        return;
    };
    let data = |d: &Arc<Vec<u8>>| -> Arc<dyn AsRef<[u8]> + Send + Sync> { d.clone() };
    let image = match i.kind() {
        usvg::ImageKind::PNG(d) => Image::from_png(data(d).into(), true),
        usvg::ImageKind::JPEG(d) => Image::from_jpeg(data(d).into(), true),
        usvg::ImageKind::GIF(d) => Image::from_gif(data(d).into(), true),
        usvg::ImageKind::WEBP(d) => Image::from_webp(data(d).into(), true),
        usvg::ImageKind::SVG(tree) => {
            draw_tree(surface, tree, sz);
            return;
        }
    };
    match image {
        Ok(image) => surface.draw_image(image, sz),
        Err(e) => log::warn!("PDF export: a metafile's bitmap left out: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_gradients_patterns_and_text_become_pdf_vectors() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="100" viewBox="0 0 200 100">
<defs>
<linearGradient id="g" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="200" y2="0"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f"/></linearGradient>
<pattern id="h" patternUnits="userSpaceOnUse" width="8" height="8"><line x1="0" y1="0" x2="8" y2="8" stroke="#000" stroke-width="1"/></pattern>
<clipPath id="c" clipPathUnits="userSpaceOnUse"><path d="M0 0H150V100H0Z"/></clipPath>
</defs>
<g clip-path="url(#c)">
<path d="M10 10H190V40H10Z" fill="url(#g)" stroke="#123456" stroke-width="2" stroke-dasharray="4 2"/>
<path d="M10 50H190V90H10Z" fill="url(#h)"/>
</g>
</svg>"##;
        let tree = usvg::Tree::from_str(svg, &usvg::Options::default()).expect("parsed");
        let mut doc = krilla::Document::new();
        let mut page =
            doc.start_page_with(krilla::page::PageSettings::from_wh(300.0, 200.0).unwrap());
        let mut surface = page.surface();
        draw_tree(&mut surface, &tree, Size::from_wh(100.0, 50.0).unwrap());
        surface.finish();
        page.finish();
        let pdf = doc.finish().expect("PDF written");
        let text = String::from_utf8_lossy(&pdf);
        // A shading for the gradient, a tiling pattern for the hatch: no image.
        assert!(text.contains("/ShadingType"), "no gradient shading");
        assert!(text.contains("/PatternType 1"), "no tiling pattern");
        assert!(!text.contains("/Subtype /Image"), "drawn as a bitmap");
    }
}
