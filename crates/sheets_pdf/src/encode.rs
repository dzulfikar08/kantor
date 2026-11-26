//! Writing pages with krilla: fills, data bars and icons, cell text (shaped
//! here, placed by its alignment), gridlines and borders, charts and
//! pictures, then the header and footer.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, anyhow};
use docs_layout::Rgba;
use docs_pdf::{Fonts, Images, draw_run, fill, metadata};
use krilla::color::rgb;
use krilla::error::KrillaError;
use krilla::geom::{PathBuilder, Rect, Transform};
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{FillRule, LineCap, LinearGradient, SpreadMethod, Stop, Stroke, StrokeDash};
use krilla::surface::Surface;
use sheets_engine::{LineStyle, VAlign};

use crate::extract::{PAD_X, family, icon_size};
use crate::text::{FontKey, Line, Shaper};
use crate::{Align, CellText, HfLayout, LineKind, OverlayKind, PageContent, hf};

/// Room above top-aligned and below bottom-aligned text, points.
const PAD_Y: f32 = 0.9;

fn color(c: u32) -> Rgba {
    Rgba([(c >> 16) as u8, (c >> 8) as u8, c as u8, 255])
}

fn krgb(c: u32) -> rgb::Color {
    rgb::Color::new((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn rect_path([x, y, w, h]: [f32; 4]) -> Option<krilla::geom::Path> {
    let r = Rect::from_xywh(x, y, w.max(0.001), h.max(0.001))?;
    let mut pb = PathBuilder::new();
    pb.push_rect(r);
    pb.finish()
}

fn fill_rect(surface: &mut Surface, rect: [f32; 4], c: u32) {
    if let Some(p) = rect_path(rect) {
        surface.set_stroke(None);
        surface.set_fill(Some(fill(color(c))));
        surface.draw_path(&p);
    }
}

/// Built chart items by object, data and size (quarter points).
type ChartKey = (sheets_engine::drawing::DrawingKey, u64, u32, u32);

/// Most laid-out charts kept (a chart split over pages is reused).
const MAX_CHARTS: usize = 64;

/// A picture krilla couldn't decode, found when the PDF was finished: the
/// address of its bytes. Encode again with it in
/// [`Encoder::skip_pictures`] to leave it out.
#[derive(Debug, Clone, Copy)]
pub struct BadPicture(pub usize);

impl std::fmt::Display for BadPicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a picture can't be decoded")
    }
}

impl std::error::Error for BadPicture {}

/// Writes pages as they come; [`Encoder::finish`] returns the PDF.
pub struct Encoder {
    pdf: krilla::Document,
    fonts: Fonts,
    shaper: Shaper,
    pages: usize,
    images: Images,
    /// Lays out charts (made on the first chart, on the shaper's fonts).
    engine: Option<docs_layout::Engine>,
    charts: HashMap<ChartKey, Arc<Vec<slides_scene::Item>>>,
}

impl Encoder {
    pub fn new(title: &str, shaper: Shaper) -> Self {
        let mut pdf = krilla::Document::new();
        pdf.set_metadata(metadata(title));
        Self {
            pdf,
            fonts: Fonts::default(),
            shaper,
            pages: 0,
            images: Images::default(),
            engine: None,
            charts: HashMap::new(),
        }
    }

    /// Leave out pictures that failed before (see [`BadPicture`]).
    pub fn skip_pictures(mut self, skip: &HashSet<usize>) -> Self {
        self.images = Images::skipping(skip.iter().map(|&a| a as *const u8).collect());
        self
    }

    pub fn pages(&self) -> usize {
        self.pages
    }

    /// Write one page.
    pub fn page(&mut self, page: &PageContent) {
        let settings = PageSettings::from_wh(page.width.max(1.0), page.height.max(1.0))
            .expect("positive page size");
        let mut p = self.pdf.start_page_with(settings);
        let mut surface = p.surface();
        let mut pen = Pen {
            fonts: &mut self.fonts,
            shaper: &mut self.shaper,
        };
        surface.push_transform(&Transform::from_row(
            page.scale,
            0.0,
            0.0,
            page.scale,
            page.origin[0],
            page.origin[1],
        ));
        for (rect, c) in &page.fills {
            // A hair wider, so neighbouring fills leave no seam.
            let [x, y, w, h] = *rect;
            fill_rect(&mut surface, [x, y, w + 0.03, h + 0.03], *c);
        }
        for (rect, tile, c) in &page.patterns {
            draw_pattern(&mut surface, *rect, tile, *c);
        }
        for bar in &page.bars {
            draw_bar(&mut surface, bar);
        }
        for icon in &page.icons {
            draw_icon(&mut surface, icon);
        }
        for spark in &page.sparklines {
            draw_sparkline(&mut surface, spark);
        }
        for t in &page.texts {
            pen.cell_text(&mut surface, t);
        }
        for l in &page.lines {
            draw_line(&mut surface, l);
        }
        for o in &page.overlays {
            let items = overlay_items(o, &mut self.engine, &mut self.charts, pen.shaper);
            draw_overlay(&mut surface, o, &items, &mut self.images, pen.fonts);
        }
        // Notes as displayed: a pale yellow box with a thin black border.
        for n in &page.notes {
            let [x, y, w, h] = n.rect;
            fill_rect(&mut surface, n.rect, 0xFFFFE1);
            let t = 0.5;
            fill_rect(&mut surface, [x, y, w, t], 0);
            fill_rect(&mut surface, [x, y + h - t, w, t], 0);
            fill_rect(&mut surface, [x, y, t, h], 0);
            fill_rect(&mut surface, [x + w - t, y, t, h], 0);
            if n.spans.is_empty() {
                pen.cell_text(&mut surface, &n.text);
            } else {
                pen.rich_text(&mut surface, &n.text, &n.spans);
            }
        }
        surface.pop();
        let images = &mut self.images;
        pen.header_footer(&mut surface, &page.header, &page.hf, true, page, images);
        pen.header_footer(&mut surface, &page.footer, &page.hf, false, page, images);
        surface.finish();
        p.finish();
        self.pages += 1;
    }

    /// The PDF (one blank page when nothing was printed). A picture that
    /// can't be decoded fails with a [`BadPicture`] error.
    pub fn finish(self) -> Result<Vec<u8>> {
        self.finish_keeping_shaper().0
    }

    /// [`Encoder::finish`], handing back the shaper (and its font
    /// database) for the next document.
    pub fn finish_keeping_shaper(mut self) -> (Result<Vec<u8>>, Shaper) {
        let shaper = std::mem::take(&mut self.shaper);
        (self.finish_pdf(), shaper)
    }

    fn finish_pdf(mut self) -> Result<Vec<u8>> {
        if self.pages == 0 {
            let settings = PageSettings::from_wh(612.0, 792.0).expect("positive page size");
            let mut p = self.pdf.start_page_with(settings);
            p.surface().finish();
            p.finish();
        }
        match self.pdf.finish() {
            Ok(bytes) => Ok(bytes),
            Err(KrillaError::Image(bad, _, why)) => match self.images.key_of(&bad) {
                Some(key) => {
                    log::warn!("PDF export: a picture can't be decoded: {why}");
                    Err(BadPicture(key as usize).into())
                }
                None => Err(anyhow!("writing PDF: picture: {why}")),
            },
            Err(e) => Err(anyhow!("writing PDF: {e:?}")),
        }
    }
}

/// Stroke width and dash pattern of a line, points.
fn line_look(kind: LineKind) -> (f32, Option<Vec<f32>>) {
    let LineKind::Border(s) = kind else {
        return (0.25, None);
    };
    let d = |v: &[f32]| Some(v.to_vec());
    match s {
        LineStyle::Hair => (0.25, None),
        LineStyle::Thin => (0.5, None),
        LineStyle::Medium => (1.0, None),
        LineStyle::Thick => (1.5, None),
        LineStyle::Double => (0.5, None),
        LineStyle::Dotted => (0.5, d(&[0.75, 0.75])),
        LineStyle::Dashed => (0.5, d(&[2.25, 0.75])),
        LineStyle::DashDot => (0.5, d(&[3.0, 1.0, 1.0, 1.0])),
        LineStyle::DashDotDot => (0.5, d(&[3.0, 1.0, 1.0, 1.0, 1.0, 1.0])),
        LineStyle::MediumDashed => (1.0, d(&[4.5, 1.5])),
        LineStyle::MediumDashDot => (1.0, d(&[4.5, 1.5, 1.5, 1.5])),
        LineStyle::MediumDashDotDot => (1.0, d(&[4.5, 1.5, 1.5, 1.5, 1.5, 1.5])),
        LineStyle::SlantDashDot => (1.0, d(&[4.5, 0.75, 1.5, 0.75])),
    }
}

fn stroke_segment(
    surface: &mut Surface,
    from: [f32; 2],
    to: [f32; 2],
    width: f32,
    dash: Option<Vec<f32>>,
    c: u32,
) {
    let mut pb = PathBuilder::new();
    pb.move_to(from[0], from[1]);
    pb.line_to(to[0], to[1]);
    let Some(path) = pb.finish() else { return };
    surface.set_fill(None);
    surface.set_stroke(Some(Stroke {
        paint: krgb(c).into(),
        width,
        line_cap: if dash.is_some() {
            LineCap::Butt
        } else {
            LineCap::Square
        },
        dash: dash.map(|array| StrokeDash { array, offset: 0.0 }),
        ..Stroke::default()
    }));
    surface.draw_path(&path);
    surface.set_stroke(None);
}

fn draw_line(surface: &mut Surface, l: &crate::Line) {
    let (width, dash) = line_look(l.style);
    if l.style == LineKind::Border(LineStyle::Double) {
        // Two thin lines either side of the edge.
        let horizontal = (l.from[1] - l.to[1]).abs() < 0.001;
        for off in [-0.6, 0.6] {
            let (dx, dy) = if horizontal { (0.0, off) } else { (off, 0.0) };
            stroke_segment(
                surface,
                [l.from[0] + dx, l.from[1] + dy],
                [l.to[0] + dx, l.to[1] + dy],
                0.4,
                None,
                l.color,
            );
        }
        return;
    }
    stroke_segment(surface, l.from, l.to, width, dash, l.color);
}

fn draw_bar(surface: &mut Surface, b: &crate::Bar) {
    let [x, y, w, h] = b.rect;
    let inset = 1.5_f32.min(h / 4.0);
    let inner = (w - 2.0 * inset).max(0.0);
    let bx = x + inset + b.bar.from * inner;
    let bw = (b.bar.to - b.bar.from).max(0.0) * inner;
    let rect = [bx, y + inset, bw, (h - 2.0 * inset).max(0.0)];
    let Some(path) = rect_path(rect) else { return };
    let c = b.bar.color;
    if b.bar.gradient {
        let pale = {
            let mix = |s: u32| {
                let v = ((c >> s) & 0xff) as f32;
                ((v + (255.0 - v) * 0.85).round() as u32) << s
            };
            mix(16) | mix(8) | mix(0)
        };
        let (from, to) = if b.bar.negative { (pale, c) } else { (c, pale) };
        let stop = |offset: f32, c: u32| Stop {
            offset: NormalizedF32::new(offset).unwrap_or(NormalizedF32::ZERO),
            color: krgb(c).into(),
            opacity: NormalizedF32::ONE,
        };
        surface.set_stroke(None);
        surface.set_fill(Some(krilla::paint::Fill {
            paint: LinearGradient {
                x1: bx,
                y1: 0.0,
                x2: bx + bw.max(0.01),
                y2: 0.0,
                transform: Transform::identity(),
                spread_method: SpreadMethod::Pad,
                stops: vec![stop(0.0, from), stop(1.0, to)],
                anti_alias: true,
            }
            .into(),
            opacity: NormalizedF32::ONE,
            rule: FillRule::NonZero,
        }));
        surface.draw_path(&path);
        // A gradient bar's border, when it has one.
        if let Some(bc) = b.bar.border {
            surface.set_fill(None);
            surface.set_stroke(Some(Stroke {
                paint: krgb(bc).into(),
                width: 0.5,
                ..Stroke::default()
            }));
            surface.draw_path(&path);
            surface.set_stroke(None);
        }
    } else {
        surface.set_fill(Some(fill(color(c))));
        surface.set_stroke(Some(Stroke {
            paint: krgb(b.bar.border.unwrap_or(c)).into(),
            width: 0.5,
            ..Stroke::default()
        }));
        surface.draw_path(&path);
        surface.set_stroke(None);
    }
    if let Some(axis) = b.bar.axis {
        let ax = x + inset + axis * inner;
        stroke_segment(
            surface,
            [ax, y + 0.75],
            [ax, y + h - 0.75],
            0.5,
            Some(vec![1.0, 1.0]),
            b.bar.axis_color.unwrap_or(0x000000),
        );
    }
}

/// A sparkline in its cell ([`sheets_engine::Sparkline::geometry`], as on
/// screen), in points.
fn draw_sparkline(surface: &mut Surface, s: &crate::Spark) {
    let [x, y, w, h] = s.rect;
    let g = s.spark.geometry(w, h, 1.0);
    let ink = |c: u32| if s.black { 0x000000 } else { c };
    if let Some((ay, c)) = g.axis {
        fill_rect(surface, [x, y + ay - 0.375, w, 0.75], ink(c));
    }
    for ([bx, by, bw, bh], c) in &g.bars {
        fill_rect(surface, [x + bx, y + by, *bw, *bh], ink(*c));
    }
    for run in g.lines.iter().filter(|r| r.len() >= 2) {
        let mut pb = PathBuilder::new();
        pb.move_to(x + run[0][0], y + run[0][1]);
        for p in &run[1..] {
            pb.line_to(x + p[0], y + p[1]);
        }
        let Some(path) = pb.finish() else { continue };
        surface.set_fill(None);
        surface.set_stroke(Some(Stroke {
            paint: krgb(ink(s.spark.series)).into(),
            width: g.line_width,
            line_join: krilla::paint::LineJoin::Round,
            line_cap: krilla::paint::LineCap::Round,
            ..Stroke::default()
        }));
        surface.draw_path(&path);
        surface.set_stroke(None);
    }
    let r = g.marker_size / 2.0;
    for ([mx, my], c) in &g.markers {
        // A circle as four Bézier quarters.
        let (cx, cy, k) = (x + mx, y + my, r * 0.552_284_8);
        let mut pb = PathBuilder::new();
        pb.move_to(cx + r, cy);
        pb.cubic_to(cx + r, cy + k, cx + k, cy + r, cx, cy + r);
        pb.cubic_to(cx - k, cy + r, cx - r, cy + k, cx - r, cy);
        pb.cubic_to(cx - r, cy - k, cx - k, cy - r, cx, cy - r);
        pb.cubic_to(cx + k, cy - r, cx + r, cy - k, cx + r, cy);
        pb.close();
        let Some(path) = pb.finish() else { continue };
        surface.set_stroke(None);
        surface.set_fill(Some(fill(color(ink(*c)))));
        surface.draw_path(&path);
    }
}

fn draw_icon(surface: &mut Surface, i: &crate::Icon) {
    let [x, y, _, h] = i.rect;
    let s = icon_size(h);
    let (ox, oy) = (x + 1.5, y + (h - s) / 2.0);
    for p in i.icon.paths() {
        let mut pb = PathBuilder::new();
        for (k, &(px, py)) in p.points.iter().enumerate() {
            let (qx, qy) = (ox + px * s, oy + py * s);
            if k == 0 {
                pb.move_to(qx, qy);
            } else {
                pb.line_to(qx, qy);
            }
        }
        pb.close();
        let Some(path) = pb.finish() else { continue };
        match p.stroke {
            Some(w) => {
                surface.set_fill(None);
                surface.set_stroke(Some(Stroke {
                    paint: krgb(p.color).into(),
                    width: w * s,
                    ..Stroke::default()
                }));
            }
            None => {
                surface.set_stroke(None);
                surface.set_fill(Some(fill(color(p.color))));
            }
        }
        surface.draw_path(&path);
        surface.set_stroke(None);
    }
}

/// The display list of a chart or picture, in px of its own box.
fn overlay_items(
    o: &crate::Overlay,
    engine: &mut Option<docs_layout::Engine>,
    charts: &mut HashMap<ChartKey, Arc<Vec<slides_scene::Item>>>,
    shaper: &Shaper,
) -> Arc<Vec<slides_scene::Item>> {
    // Points to the scene's px.
    let (w, h) = (o.rect[2] / 0.75, o.rect[3] / 0.75);
    match &o.kind {
        OverlayKind::Picture {
            part,
            data,
            crop,
            shape,
            outline,
            theme,
        } => {
            let mut items = sheets_scene::picture_items(part, data, *crop, shape.as_deref(), w, h);
            if let Some(outline) = outline {
                let engine = engine.get_or_insert_with(|| {
                    docs_layout::Engine::with_fonts(shaper.font_context().clone())
                });
                let look = sheets_scene::ChartTheme::new(theme.as_deref());
                let xml = sheets_scene::picture_outline_xml(shape.as_deref(), outline);
                items.extend(sheets_scene::shape_items_on(
                    &slides_scene::Builder::new(),
                    engine,
                    &look,
                    &xml,
                    &Default::default(),
                    &[],
                    w,
                    h,
                    None,
                ));
            }
            Arc::new(items)
        }
        OverlayKind::Chart {
            key,
            hash,
            chart,
            defaults,
        } => {
            let q = |v: f32| (v.max(0.0) * 4.0).round() as u32;
            let k = (*key, *hash, q(o.rect[2]), q(o.rect[3]));
            if let Some(items) = charts.get(&k) {
                return items.clone();
            }
            let engine = engine.get_or_insert_with(|| {
                docs_layout::Engine::with_fonts(shaper.font_context().clone())
            });
            let items = Arc::new(sheets_scene::chart_items(engine, chart, defaults, w, h));
            if charts.len() >= MAX_CHARTS {
                charts.clear();
            }
            charts.insert(k, items.clone());
            items
        }
        OverlayKind::Shape {
            key,
            hash,
            xml,
            media,
            charts: group,
            theme,
        } => {
            let q = |v: f32| (v.max(0.0) * 4.0).round() as u32;
            let k = (*key, *hash, q(o.rect[2]), q(o.rect[3]));
            if let Some(items) = charts.get(&k) {
                return items.clone();
            }
            let engine = engine.get_or_insert_with(|| {
                docs_layout::Engine::with_fonts(shaper.font_context().clone())
            });
            let look = sheets_scene::ChartTheme::new(theme.as_deref());
            let builder = slides_scene::Builder::new();
            let items = Arc::new(sheets_scene::shape_items_on(
                &builder, engine, &look, xml, media, group, w, h, None,
            ));
            if charts.len() >= MAX_CHARTS {
                charts.clear();
            }
            charts.insert(k, items.clone());
            items
        }
    }
}

/// Bounds of an item in its box's px (conservative: control points and
/// half the stroke included); `None` for what draws nothing.
fn item_bounds(item: &slides_scene::Item) -> Option<[f32; 4]> {
    use slides_scene::{P, Prim, Seg, p};
    let mut pts: Vec<P> = Vec::new();
    let mut grow = 0.0f32;
    let path_points = |path: &slides_scene::Path, pts: &mut Vec<P>| {
        for s in &path.segs {
            match *s {
                Seg::Move(q) | Seg::Line(q) => pts.push(q),
                Seg::Quad(c, q) => pts.extend([c, q]),
                Seg::Cubic(a, b, q) => pts.extend([a, b, q]),
                Seg::Close => {}
            }
        }
    };
    match &item.prim {
        Prim::Fill { path, .. } => path_points(path, &mut pts),
        Prim::Stroke { path, stroke } => {
            path_points(path, &mut pts);
            grow = stroke.width;
        }
        Prim::Picture(pic) => {
            let [x, y, w, h] = pic.rect;
            pts.extend([p(x, y), p(x + w, y + h), p(x + w, y), p(x, y + h)]);
        }
        Prim::Text(t) => {
            for r in &t.runs {
                let (x0, x1) = (r.run.origin_x, r.run.origin_x + r.run.advance);
                let size = r.run.font_size;
                let (y0, y1) = (r.run.baseline - size * 1.2, r.run.baseline + size * 0.5);
                for (x, y) in [(x0, y0), (x1, y1), (x1, y0), (x0, y1)] {
                    pts.push(p(t.origin.x + x, t.origin.y + y));
                }
            }
            grow = 2.0;
        }
        Prim::Unsupported { .. } | Prim::PlaceholderOutline { .. } => return None,
    }
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for q in pts {
        let q = item.transform.apply(q);
        b = [b[0].min(q.x), b[1].min(q.y), b[2].max(q.x), b[3].max(q.y)];
    }
    (b[0] <= b[2]).then(|| [b[0] - grow, b[1] - grow, b[2] + grow, b[3] + grow])
}

/// A chart or picture, clipped to the page's cells. When only part of it
/// is on the page, items wholly outside are left out (a split chart's
/// title text isn't written, and found, on the page that doesn't show it).
fn draw_overlay(
    surface: &mut Surface,
    o: &crate::Overlay,
    items: &[slides_scene::Item],
    images: &mut Images,
    fonts: &mut Fonts,
) {
    let [x, y, w, h] = o.rect;
    let [cx, cy, cw, ch] = o.clip;
    let inside =
        x >= cx - 0.01 && y >= cy - 0.01 && x + w <= cx + cw + 0.01 && y + h <= cy + ch + 0.01;
    let clip = clip_to(surface, o.clip);
    surface.push_transform(&Transform::from_translate(x, y));
    if inside {
        slides_pdf::draw_items(surface, items, images, fonts);
    } else {
        // The page's part, in the box's px.
        let v = [
            (cx - x) / 0.75,
            (cy - y) / 0.75,
            (cx + cw - x) / 0.75,
            (cy + ch - y) / 0.75,
        ];
        let shown: Vec<slides_scene::Item> = items
            .iter()
            .filter(|i| {
                item_bounds(i)
                    .is_some_and(|b| b[0] < v[2] && b[2] > v[0] && b[1] < v[3] && b[3] > v[1])
            })
            .cloned()
            .collect();
        slides_pdf::draw_items(surface, &shown, images, fonts);
    }
    surface.pop();
    if clip {
        surface.pop();
    }
}

/// Text drawing state.
struct Pen<'a> {
    fonts: &'a mut Fonts,
    shaper: &'a mut Shaper,
}

impl Pen<'_> {
    /// Draw a shaped line with its pen at `(x, baseline)`, scaled by `k`.
    fn runs(&mut self, surface: &mut Surface, line: &Line, t: Transform, c: u32) {
        surface.push_transform(&t);
        for run in &line.runs {
            let Some(font) = self.fonts.get(&run.font, &run.variations) else {
                continue;
            };
            let mut run = run.clone();
            run.color = color(c);
            draw_run(surface, &run, font);
        }
        surface.pop();
    }

    /// Rich text wrapped in `t`'s cell from its top left (a note's box):
    /// each run in its own colour, cut to the cell.
    fn rich_text(&mut self, surface: &mut Surface, t: &CellText, spans: &[crate::Span]) {
        let [x0, y0, w, _] = t.cell;
        let avail = (w - 2.0 * PAD_X).max(1.0);
        let block = self.shaper.wrapped_rich(&t.text, &t.font, spans, avail);
        let clip = clip_to(surface, intersect(t.clip, t.cell));
        for (line, base) in &block.lines {
            let tr = Transform::from_translate(x0 + PAD_X, y0 + PAD_Y + base);
            surface.push_transform(&tr);
            for run in &line.runs {
                if let Some(font) = self.fonts.get(&run.font, &run.variations) {
                    draw_run(surface, run, font);
                }
            }
            surface.pop();
        }
        if clip {
            surface.pop();
        }
    }

    fn cell_text(&mut self, surface: &mut Surface, t: &CellText) {
        let size = t.font.size_pt();
        let (lead, trail, avail) = room(t);

        // Wrapped and justified text.
        if t.wrap && t.rotation == 0 {
            let block =
                self.shaper
                    .wrapped(&t.text, &t.font, avail.max(1.0), t.align == Align::Justify);
            let top = wrapped_top(t, block.height);
            let clip = clip_to(surface, intersect(t.clip, t.cell));
            for (line, base) in &block.lines {
                let x = wrapped_x(t, line.width, lead, trail, avail);
                let tr = Transform::from_translate(x, top + base);
                self.runs(surface, line, tr, t.color);
                decorations(
                    surface,
                    tr,
                    line.width,
                    size,
                    t.underline,
                    t.strike,
                    t.color,
                );
            }
            if clip {
                surface.pop();
            }
            return;
        }

        if t.rotation == 255 {
            self.stacked(surface, t, avail, lead);
            return;
        }

        let line = self.shaper.line(&t.text, &t.font);
        if t.rotation != 0 {
            self.rotated(surface, t, &line);
            return;
        }
        let p = place(self.shaper, t, line, lead, trail, avail);
        let (line, k, repeat, x, width, base, asc, desc) =
            (p.line, p.k, p.repeat, p.x, p.width, p.base, p.asc, p.desc);
        // Clip only when the text reaches out of its room.
        let inside = x >= t.clip[0] - 0.01
            && x + width <= t.clip[0] + t.clip[2] + 0.01
            && base - asc >= t.clip[1] - 0.5
            && base + desc <= t.clip[1] + t.clip[3] + 0.5;
        let clip = !inside && clip_to(surface, t.clip);
        for i in 0..repeat {
            let tr = Transform::from_row(k, 0.0, 0.0, k, x + i as f32 * line.width, base);
            self.runs(surface, &line, tr, t.color);
            decorations(
                surface,
                tr,
                line.width,
                size,
                t.underline,
                t.strike,
                t.color,
            );
        }
        if clip {
            surface.pop();
        }
    }

    /// Text turned by the cell's rotation, placed by its bounding box.
    fn rotated(&mut self, surface: &mut Surface, t: &CellText, line: &Line) {
        let [x0, y0, w, h] = t.cell;
        let deg = if t.rotation <= 90 {
            t.rotation as f32
        } else {
            -((t.rotation.min(180) - 90) as f32)
        };
        let (s, c) = deg.to_radians().sin_cos();
        let th = line.ascent + line.descent;
        let bw = line.width * c.abs() + th * s.abs();
        let bh = line.width * s.abs() + th * c.abs();
        let cx = match t.align {
            Align::Left => x0 + PAD_X + t.indent + bw / 2.0,
            Align::Right => x0 + w - PAD_X - bw / 2.0,
            _ => x0 + w / 2.0,
        };
        let cy = match t.valign {
            VAlign::Top => y0 + PAD_Y + bh / 2.0,
            VAlign::Center => y0 + h / 2.0,
            VAlign::Bottom => y0 + h - PAD_Y - bh / 2.0,
        };
        // Text space: x along the baseline, the line's middle at
        // (width / 2, (descent - ascent) / 2); turned counter-clockwise on
        // the page (y down) and moved to the box's centre.
        let (mx, my) = (line.width / 2.0, (line.descent - line.ascent) / 2.0);
        let tx = cx - (c * mx + s * my);
        let ty = cy - (-s * mx + c * my);
        let tr = Transform::from_row(c, -s, s, c, tx, ty);
        let clip = clip_to(surface, t.clip);
        self.runs(surface, line, tr, t.color);
        decorations(
            surface,
            tr,
            line.width,
            t.font.size_pt(),
            t.underline,
            t.strike,
            t.color,
        );
        if clip {
            surface.pop();
        }
    }

    /// Stacked letters (rotation 255): one character a line.
    fn stacked(&mut self, surface: &mut Surface, t: &CellText, avail: f32, lead: f32) {
        let [x0, y0, _, h] = t.cell;
        let lines: Vec<std::sync::Arc<Line>> = t
            .text
            .chars()
            .filter(|c| !c.is_control())
            .map(|ch| self.shaper.line(&ch.to_string(), &t.font))
            .collect();
        let lh = lines.iter().map(|l| l.height).fold(0.0, f32::max);
        let total = lh * lines.len() as f32;
        let mut y = match t.valign {
            VAlign::Top => y0 + PAD_Y,
            VAlign::Center => y0 + (h - total) / 2.0,
            VAlign::Bottom => y0 + h - PAD_Y - total,
        };
        let clip = clip_to(surface, t.clip);
        for l in &lines {
            let x = match t.align {
                Align::Left => x0 + lead,
                Align::Right => x0 + lead + avail - l.width,
                _ => x0 + lead + (avail - l.width) / 2.0,
            };
            self.runs(
                surface,
                l,
                Transform::from_translate(x, y + l.ascent),
                t.color,
            );
            y += lh;
        }
        if clip {
            surface.pop();
        }
    }

    fn header_footer(
        &mut self,
        surface: &mut Surface,
        parts: &hf::Parts,
        lay: &HfLayout,
        header: bool,
        page: &PageContent,
        images: &mut Images,
    ) {
        if parts.is_empty() {
            return;
        }
        for (which, section) in [(0, &parts.left), (1, &parts.center), (2, &parts.right)] {
            // Shape every run, line by line.
            let mut shaped = Vec::new();
            // Pictures: (line, x in the line, width, height, picture).
            let mut pictures = Vec::new();
            for runs in section {
                let mut items = Vec::new();
                let (mut asc, mut desc, mut lh, mut width) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
                for r in runs {
                    if r.picture {
                        // `&G`: the section's picture, standing on the line.
                        let pic = page.hf_pictures[usize::from(!header)][which].as_ref();
                        if let Some(pic) = pic {
                            let (w, h) = (
                                pic.width_pt as f32 * lay.scale,
                                pic.height_pt as f32 * lay.scale,
                            );
                            asc = asc.max(h);
                            lh = lh.max(h);
                            pictures.push((shaped.len(), width, w, h, pic));
                            width += w;
                        }
                        continue;
                    }
                    let base = r.font.size.unwrap_or(lay.size) * lay.scale;
                    let size = if r.font.shift != 0 { base * 0.66 } else { base };
                    let fam = r
                        .font
                        .family
                        .as_deref()
                        .map(family)
                        .unwrap_or_else(|| lay.family.clone());
                    let key = FontKey::new(fam, size, r.font.bold, r.font.italic);
                    let line = self.shaper.line(&r.text, &key);
                    let shift = -(r.font.shift as f32) * base * 0.33;
                    asc = asc.max(line.ascent - shift);
                    desc = desc.max(line.descent + shift);
                    lh = lh.max(line.height);
                    items.push((line.clone(), width, shift, r, size));
                    width += line.width;
                }
                if runs.is_empty() {
                    // An empty line still takes a line's height.
                    let key = FontKey::new(lay.family.clone(), lay.size * lay.scale, false, false);
                    let l = self.shaper.line(" ", &key);
                    asc = l.ascent;
                    desc = l.descent;
                    lh = l.height;
                }
                shaped.push((items, asc, desc, lh.max(asc + desc), width));
            }
            let total: f32 = shaped.iter().map(|l| l.3).sum();
            let mut y = if header {
                lay.header
            } else {
                page.height - lay.footer - total
            };
            for (n, (items, asc, _, lh, width)) in shaped.into_iter().enumerate() {
                let x = match which {
                    0 => lay.left,
                    1 => (page.width - width) / 2.0,
                    _ => lay.right - width,
                };
                for &(_, dx, w, h, pic) in pictures.iter().filter(|p| p.0 == n) {
                    let items = sheets_scene::picture_items(
                        &pic.part,
                        &pic.data,
                        [0.0; 4],
                        None,
                        w / 0.75,
                        h / 0.75,
                    );
                    surface.push_transform(&Transform::from_translate(x + dx, y + asc - h));
                    slides_pdf::draw_items(surface, &items, images, self.fonts);
                    surface.pop();
                }
                for (line, dx, shift, r, size) in items {
                    let tr = Transform::from_translate(x + dx, y + asc + shift);
                    let c = r.font.color.unwrap_or(0);
                    self.runs(surface, &line, tr, c);
                    decorations(
                        surface,
                        tr,
                        line.width,
                        size,
                        r.font.underline,
                        r.font.strike,
                        c,
                    );
                }
                y += lh;
            }
        }
    }
}

/// A patterned fill: Excel's 8 × 8 tile of 1 px dots (0.75 pt) repeated
/// over the cell, each row's dots merged into runs.
fn draw_pattern(surface: &mut Surface, rect: [f32; 4], tile: &[u8; 8], c: u32) {
    const DOT: f32 = 0.75;
    let [x0, y0, w, h] = rect;
    let (cols, rows) = ((w / DOT).ceil() as usize, (h / DOT).ceil() as usize);
    for r in 0..rows {
        let bits = tile[r % 8];
        if bits == 0 {
            continue;
        }
        let y = y0 + r as f32 * DOT;
        let dh = DOT.min(y0 + h - y);
        let mut start: Option<usize> = None;
        for col in 0..=cols {
            let on = col < cols && bits & (0x80 >> (col % 8)) != 0;
            match (on, start) {
                (true, None) => start = Some(col),
                (false, Some(s)) => {
                    let x = x0 + s as f32 * DOT;
                    let dw = ((col - s) as f32 * DOT).min(x0 + w - x);
                    fill_rect(surface, [x, y, dw, dh], c);
                    start = None;
                }
                _ => {}
            }
        }
    }
}

/// Underline and strikethrough for a line drawn at `t`.
fn decorations(
    surface: &mut Surface,
    t: Transform,
    width: f32,
    size: f32,
    under: bool,
    strike: bool,
    c: u32,
) {
    if !under && !strike {
        return;
    }
    surface.push_transform(&t);
    let thick = (size / 16.0).max(0.4);
    if under {
        fill_rect(surface, [0.0, size * 0.12, width, thick], c);
    }
    if strike {
        fill_rect(surface, [0.0, -size * 0.28, width, thick], c);
    }
    surface.pop();
}

/// The room before and after a cell's text (padding, indent, an icon) and
/// the width left between, points.
fn room(t: &CellText) -> (f32, f32, f32) {
    let w = t.cell[2];
    let lead = PAD_X
        + t.icon_room
        + if t.align == Align::Left {
            t.indent
        } else {
            0.0
        };
    let trail = PAD_X
        + if t.align == Align::Right {
            t.indent
        } else {
            0.0
        };
    (lead, trail, (w - lead - trail).max(0.0))
}

/// The top of a wrapped block `height` points tall in its cell.
fn wrapped_top(t: &CellText, height: f32) -> f32 {
    let [_, y0, _, h] = t.cell;
    match t.valign {
        VAlign::Top => y0 + PAD_Y,
        VAlign::Center => y0 + (h - height) / 2.0,
        VAlign::Bottom => y0 + h - PAD_Y - height,
    }
}

/// The left edge of a wrapped line `width` points wide.
fn wrapped_x(t: &CellText, width: f32, lead: f32, trail: f32, avail: f32) -> f32 {
    let [x0, _, w, _] = t.cell;
    match t.align {
        Align::Center => x0 + lead + (avail - width) / 2.0,
        Align::Right => x0 + w - trail - width,
        _ => x0 + lead,
    }
}

/// A one-line text placed in its cell.
struct Placed {
    /// The line drawn: `####` for a number that doesn't fit, a number
    /// format's repeat character filled in.
    line: Arc<Line>,
    /// Shrink to fit's scale.
    k: f32,
    /// Times drawn side by side (fill alignment).
    repeat: usize,
    /// Left edge, width (all of it), baseline, ascent and descent, points.
    x: f32,
    width: f32,
    base: f32,
    asc: f32,
    desc: f32,
    /// `####` replaced a number wider than `avail`: the number's width.
    hashes: Option<f32>,
    /// A General number wider than `avail` shown shorter (fewer decimals
    /// or scientific notation): the full text's width and the text shown.
    shortened: Option<(f32, String)>,
}

/// Place one line of `t` (`line`, already shaped) in its cell, as
/// [`Encoder`] draws it.
fn place(
    shaper: &mut Shaper,
    t: &CellText,
    mut line: Arc<Line>,
    lead: f32,
    trail: f32,
    avail: f32,
) -> Placed {
    let [x0, y0, w, h] = t.cell;
    let (x1, y1) = (x0 + w, y0 + h);
    let mut k = 1.0;
    let mut repeat = 1;
    let mut hashes = None;
    let mut shortened = None;
    if t.number && line.width > avail + 0.5 && !t.shrink {
        // A number that doesn't fit: a General number with fewer decimals
        // (or in scientific notation), else ####, as Excel shows it and the
        // grid draws it.
        let fit = sheets_engine::fit_number(&t.text, t.general, |s| {
            shaper.line(s, &t.font).width <= avail + 0.5
        });
        match fit {
            sheets_engine::NumberFit::Fits => {}
            sheets_engine::NumberFit::Shorter(s) => {
                let full = line.width;
                line = shaper.line(&s, &t.font);
                shortened = Some((full, s));
            }
            sheets_engine::NumberFit::Hashes => {
                hashes = Some(line.width);
                let hash = shaper.line("#", &t.font);
                let n = ((avail / hash.width.max(0.1)).floor() as usize).max(1);
                line = shaper.line(&"#".repeat(n), &t.font);
            }
        }
    } else if let Some((ch, at)) = t.repeat {
        // `*x` in the number format: x repeated to fill the cell.
        let one = shaper.line(&ch.to_string(), &t.font);
        let n = ((avail - line.width) / one.width.max(0.1)).floor().max(0.0) as usize;
        if n > 0 && at <= t.text.len() && t.text.is_char_boundary(at) {
            let mut s = String::with_capacity(t.text.len() + n);
            s.push_str(&t.text[..at]);
            s.extend(std::iter::repeat_n(ch, n.min(2000)));
            s.push_str(&t.text[at..]);
            line = shaper.line(&s, &t.font);
        }
    } else if t.align == Align::Fill && line.width > 0.0 {
        repeat = ((avail / line.width).floor() as usize).clamp(1, 2000);
    } else if t.shrink && line.width > avail && line.width > 0.0 {
        k = avail / line.width;
    }
    let width = line.width * k * repeat as f32;
    let x = if t.repeat.is_some() {
        x0 + lead
    } else {
        match t.align {
            Align::Center => x0 + lead + (avail - width) / 2.0,
            Align::Right => x1 - trail - width,
            _ => x0 + lead,
        }
    };
    let (asc, desc) = (line.ascent * k, line.descent * k);
    let base = match t.valign {
        VAlign::Top => y0 + PAD_Y + asc,
        VAlign::Center => y0 + (h - asc - desc) / 2.0 + asc,
        VAlign::Bottom => y1 - PAD_Y - desc,
    };
    Placed {
        line,
        k,
        repeat,
        x,
        width,
        base,
        asc,
        desc,
        hashes,
        shortened,
    }
}

/// What a cell's room cuts off of its text, as [`Encoder`] draws it (see
/// [`overrun`]). Sizes in points.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Overrun {
    /// The text's width (one line) or height (wrapped).
    pub need: f32,
    /// The room it has: the width it may use (the cell, or the empty cells
    /// it overflows into, within the page) or the cell's height.
    pub room: f32,
    /// How much is cut at the left, right, top and bottom (0: nothing).
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
    /// A number shown as `####`: it is wider than its cell.
    pub hashes: bool,
    /// A General number wider than its cell, shown with fewer decimals or
    /// in scientific notation (as Excel does): the text shown. `need` is
    /// the full text's width.
    pub shortened: Option<String>,
}

/// Cuts smaller than these don't count, points: across (a glyph's side
/// bearing) and down.
const CUT_SLACK: f32 = 1.0;
const CUT_SLACK_V: f32 = 0.5;
/// Glyph tops and bottoms from the baseline, in font sizes: ascenders
/// (`l`, `d`, the dot of `i`) and descenders, without the font's empty
/// line gap.
const GLYPH_TOP: f32 = 0.75;
const GLYPH_BOTTOM: f32 = 0.2;

/// What of `t` its room cuts off, as [`Encoder`] draws it; `None` when
/// all of it shows (or it is turned, stacked, shrunk to fit or repeated
/// to fill). A General number shown shorter is reported with
/// [`Overrun::shortened`]. Glyph tops and bottoms are taken as 0.75 and 0.2 of the font
/// size from the baseline, so the empty part of a font's ascent above its
/// letters doesn't count as cut. A cut text's `need` down is its line
/// height (wrapped: all its lines).
pub fn overrun(shaper: &mut Shaper, t: &CellText) -> Option<Overrun> {
    if t.rotation != 0 || t.text.trim().is_empty() {
        return None;
    }
    let size = t.font.size_pt();
    let (lead, trail, avail) = room(t);
    if t.wrap {
        let clip = intersect(t.clip, t.cell);
        let block = shaper.wrapped(&t.text, &t.font, avail.max(1.0), t.align == Align::Justify);
        let top = wrapped_top(t, block.height);
        let (first, last) = (block.lines.first()?, block.lines.last()?);
        let glyph_top = top + first.1 - GLYPH_TOP * size;
        let glyph_bottom = top + last.1 + GLYPH_BOTTOM * size;
        let cut_top = (clip[1] - glyph_top).max(0.0);
        let cut_bottom = (glyph_bottom - (clip[1] + clip[3])).max(0.0);
        return (cut_top > CUT_SLACK_V || cut_bottom > CUT_SLACK_V).then_some(Overrun {
            need: block.height,
            room: clip[3],
            top: cut_top,
            bottom: cut_bottom,
            ..Overrun::default()
        });
    }
    let line = shaper.line(&t.text, &t.font);
    let p = place(shaper, t, line, lead, trail, avail);
    if let Some(need) = p.hashes {
        return Some(Overrun {
            need,
            room: avail,
            hashes: true,
            ..Overrun::default()
        });
    }
    if let Some((need, shown)) = p.shortened {
        return Some(Overrun {
            need,
            room: avail,
            shortened: Some(shown),
            ..Overrun::default()
        });
    }
    let c = t.clip;
    let (mut left, mut right) = (0.0, 0.0);
    if !(t.shrink || t.align == Align::Fill || t.repeat.is_some()) {
        left = (c[0] - p.x).max(0.0);
        right = (p.x + p.width - (c[0] + c[2])).max(0.0);
    }
    let glyph_top = p.base - GLYPH_TOP * size * p.k;
    let glyph_bottom = p.base + GLYPH_BOTTOM * size * p.k;
    let top = (c[1] - glyph_top).max(0.0);
    let bottom = (glyph_bottom - (c[1] + c[3])).max(0.0);
    let horizontal = left > CUT_SLACK || right > CUT_SLACK;
    let vertical = top > CUT_SLACK_V || bottom > CUT_SLACK_V;
    (horizontal || vertical).then_some(Overrun {
        need: if horizontal { p.width } else { p.asc + p.desc },
        room: if horizontal { c[2] } else { c[3] },
        left: if horizontal { left } else { 0.0 },
        right: if horizontal { right } else { 0.0 },
        top: if vertical { top } else { 0.0 },
        bottom: if vertical { bottom } else { 0.0 },
        hashes: false,
        shortened: None,
    })
}

fn intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

/// Clip to `rect`; returns whether a clip was pushed (pop it after).
fn clip_to(surface: &mut Surface, rect: [f32; 4]) -> bool {
    match rect_path(rect) {
        Some(p) => {
            surface.push_clip_path(&p, &FillRule::NonZero);
            true
        }
        None => false,
    }
}
