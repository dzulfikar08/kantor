//! Turning a slide into paint calls. [`Painter::items`] resolves a slide
//! (positions, inherited formatting, laid-out text, decoded pictures) into
//! owned [`Item`]s; [`paint_slide`] paints them at any scale, so the
//! canvas, the thumbnails and presenter mode share one renderer.
//!
//! Slide content is painted in the file's own colours; only chrome
//! (selection, handles, placeholders for objects Kantor can't draw) uses
//! theme tokens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use docs_layout::parley::LineHeight;
use docs_layout::{Engine, Marker, PageSetup, PagedDocument, Rgba as LayoutRgba, Span};
use docs_ui::FontBridge;
use gpui_kit::*;
use slides_engine::render::LineSpacing;
use slides_engine::{
    Anchor, EMU_PER_PT, EMU_PER_PX, Geom, Layer, Presentation, RenderText, Rgb, ShapeKind, Xfrm,
};

/// A text body laid out in slide pixels (EMU / 9525), origin at the top
/// left of the box's inner (inset) area.
pub(crate) struct TextLayout {
    pub paged: PagedDocument,
    pub texts: Vec<String>,
    /// Height of the text.
    pub height: f32,
}

pub(crate) struct LaidText {
    pub layout: Rc<TextLayout>,
    /// Offset of the layout's origin from the shape's top left (slide px).
    pub dx: f32,
    pub dy: f32,
}

pub(crate) enum Look {
    Geometry {
        geom: Geom,
        fill: Option<Rgb>,
        line: Option<(Rgb, i64)>,
    },
    Picture(Option<Arc<RenderImage>>),
    /// Something Kantor keeps but can't draw (table, chart, group...).
    Other,
}

pub(crate) struct Item {
    pub layer: Layer,
    pub xfrm: Xfrm,
    pub look: Look,
    pub text: Option<LaidText>,
    /// A slide placeholder with no text (dashed outline on the canvas).
    pub empty_placeholder: bool,
}

/// Caches shared by every surface of one view.
pub(crate) struct Painter {
    pub engine: Rc<RefCell<Engine>>,
    pub bridge: Rc<RefCell<FontBridge>>,
    texts: RefCell<HashMap<u64, Rc<TextLayout>>>,
    images: RefCell<HashMap<String, Option<Arc<RenderImage>>>>,
    /// Rotated / flipped pictures, by part, whole degrees, flips and aspect.
    turned: RefCell<HashMap<TurnKey, Arc<RenderImage>>>,
    /// Rasters dropped from `turned`, to free from the sprite atlas.
    evicted: RefCell<Vec<Arc<RenderImage>>>,
}

type TurnKey = (String, i32, bool, bool, i64);

/// Largest side of a rotated picture's raster, px.
const TURNED_MAX: f32 = 1600.0;

/// A picture `src` (`sw` × `sh` 4-byte pixels) stretched to a `w` × `h`
/// box, flipped, then turned clockwise by `angle` radians, resampled
/// (nearest pixel) into the box around the turned one. Corners outside
/// the picture are transparent. Returns the pixels and their size.
pub(crate) fn turn_pixels(
    src: &[u8],
    (sw, sh): (u32, u32),
    (w, h): (f32, f32),
    angle: f32,
    (flip_h, flip_v): (bool, bool),
) -> (Vec<u8>, u32, u32) {
    let k = (TURNED_MAX.min(sw.max(sh) as f32) / w.max(h).max(1e-3)).max(1e-3);
    let (w, h) = ((w * k).max(1.0), (h * k).max(1.0));
    let (sin, cos) = angle.sin_cos();
    // Less a hair, so 90° turns don't gain a pixel from rounding.
    let bw = (w * cos.abs() + h * sin.abs() - 1e-3).ceil().max(1.0) as u32;
    let bh = (w * sin.abs() + h * cos.abs() - 1e-3).ceil().max(1.0) as u32;
    let mut out = vec![0u8; (bw * bh * 4) as usize];
    for j in 0..bh {
        for i in 0..bw {
            let u = i as f32 + 0.5 - bw as f32 / 2.0;
            let v = j as f32 + 0.5 - bh as f32 / 2.0;
            // Back into the unturned box.
            let mut lx = u * cos + v * sin + w / 2.0;
            let mut ly = -u * sin + v * cos + h / 2.0;
            if lx < 0.0 || ly < 0.0 || lx >= w || ly >= h {
                continue;
            }
            if flip_h {
                lx = w - lx;
            }
            if flip_v {
                ly = h - ly;
            }
            let sx = ((lx / w * sw as f32) as u32).min(sw - 1);
            let sy = ((ly / h * sh as f32) as u32).min(sh - 1);
            let s = ((sy * sw + sx) * 4) as usize;
            let d = ((j * bw + i) * 4) as usize;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    (out, bw, bh)
}

fn rgba(c: Rgb, alpha: u8) -> LayoutRgba {
    let [r, g, b] = c.0;
    LayoutRgba([r, g, b, alpha])
}

pub(crate) fn hsla(c: Rgb) -> Hsla {
    let [r, g, b] = c.0;
    gpui_kit::Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a: 1.,
    }
    .into()
}

/// Layout paragraphs for a resolved text body.
pub(crate) fn layout_paragraphs(t: &RenderText) -> Vec<docs_layout::Paragraph> {
    // Prompts are the placeholder's own colour, faded to a muted grey.
    let alpha = if t.prompt { 110 } else { 255 };
    t.paras
        .iter()
        .map(|p| {
            let mut lp = docs_layout::Paragraph::new(p.text.clone());
            lp.family = docs_engine::font_stack(&p.base.font);
            lp.size_pt = p.base.size_pt;
            lp.line_height = Some(match p.line {
                Some(LineSpacing::Multiple(m)) => LineHeight::MetricsRelative(m.max(0.1)),
                // Layout units are slide px (1 pt = 4/3 px).
                Some(LineSpacing::Points(pt)) => {
                    LineHeight::Absolute(pt.max(1.0) * EMU_PER_PT / EMU_PER_PX)
                }
                None => LineHeight::MetricsRelative(1.0),
            });
            lp.space_before_pt = p.space_before_pt;
            lp.space_after_pt = p.space_after_pt;
            lp.indent_left_pt = p.indent_pt.max(0.0);
            lp.first_line_pt = p.first_line_pt.max(-p.indent_pt.max(0.0));
            lp.marker = p.marker.as_ref().map(|m| Marker {
                text: m.text.clone(),
                style: Span {
                    range: 0..0,
                    bold: m.bold,
                    italic: m.italic,
                    underline: false,
                    size_pt: Some(m.size_pt),
                    color: Some(rgba(m.color, alpha)),
                    family: Some(docs_engine::font_stack(&m.font)),
                },
            });
            lp.align = match p.align {
                slides_engine::Align::Left => docs_layout::Align::Start,
                slides_engine::Align::Center => docs_layout::Align::Center,
                slides_engine::Align::Right => docs_layout::Align::End,
                slides_engine::Align::Justify => docs_layout::Align::Justify,
            };
            lp.spans = p
                .runs
                .iter()
                .map(|r| Span {
                    range: r.range.clone(),
                    bold: r.bold,
                    italic: r.italic,
                    underline: r.underline,
                    size_pt: Some(r.size_pt),
                    color: Some(rgba(r.color, alpha)),
                    family: Some(docs_engine::font_stack(&r.font)),
                })
                .collect();
            lp
        })
        .collect()
}

fn layout_key(paras: &[docs_layout::Paragraph], width: f32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    width.to_bits().hash(&mut h);
    for p in paras {
        p.text.hash(&mut h);
        p.family.hash(&mut h);
        p.size_pt.to_bits().hash(&mut h);
        (p.align as u8).hash(&mut h);
        for v in [
            p.space_before_pt,
            p.space_after_pt,
            p.indent_left_pt,
            p.first_line_pt,
        ] {
            v.to_bits().hash(&mut h);
        }
        format!("{:?}", p.line_height).hash(&mut h);
        if let Some(m) = &p.marker {
            m.text.hash(&mut h);
            m.style.size_pt.map(f32::to_bits).hash(&mut h);
            m.style.color.hash(&mut h);
            m.style.family.hash(&mut h);
            (m.style.bold, m.style.italic).hash(&mut h);
        }
        for s in &p.spans {
            s.range.hash(&mut h);
            (s.bold, s.italic, s.underline).hash(&mut h);
            s.size_pt.map(f32::to_bits).hash(&mut h);
            s.color.hash(&mut h);
            s.family.hash(&mut h);
        }
    }
    h.finish()
}

/// Width of a text body's layout area in slide px.
pub(crate) fn text_width(t: &RenderText, xfrm: &Xfrm) -> f32 {
    if !t.wrap {
        return 100_000.0;
    }
    ((xfrm.cx - t.insets[0] - t.insets[2]) as f32 / EMU_PER_PX).max(1.0)
}

impl Painter {
    pub fn new(engine: Rc<RefCell<Engine>>, bridge: Rc<RefCell<FontBridge>>) -> Self {
        Self {
            engine,
            bridge,
            texts: RefCell::default(),
            images: RefCell::default(),
            turned: RefCell::default(),
            evicted: RefCell::default(),
        }
    }

    /// Free rotated-picture rasters that left the cache.
    pub fn drop_evicted(&self, window: &mut Window) {
        for img in self.evicted.borrow_mut().drain(..) {
            let _ = window.drop_image(img);
        }
    }

    /// A picture as drawn into its rotated box's bounding box.
    fn turned_image(&self, part: &str, img: &Arc<RenderImage>, x: &Xfrm) -> Arc<RenderImage> {
        let deg = (x.rot as f32 / slides_engine::ROT_PER_DEG as f32).round() as i32 % 360;
        let aspect = (x.cx as f64 / x.cy.max(1) as f64 * 1000.0) as i64;
        let key = (part.to_owned(), deg, x.flip_h, x.flip_v, aspect);
        if let Some(t) = self.turned.borrow().get(&key) {
            return t.clone();
        }
        let size = img.size(0);
        let (sw, sh) = (size.width.0.max(1) as u32, size.height.0.max(1) as u32);
        let Some(bytes) = img
            .as_bytes(0)
            .filter(|b| b.len() >= (sw * sh * 4) as usize)
        else {
            return img.clone();
        };
        let (px, bw, bh) = turn_pixels(
            bytes,
            (sw, sh),
            (x.cx as f32, x.cy as f32),
            (deg as f32).to_radians(),
            (x.flip_h, x.flip_v),
        );
        let Some(buf) = image::RgbaImage::from_raw(bw, bh, px) else {
            return img.clone();
        };
        let turned = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
        let mut cache = self.turned.borrow_mut();
        if cache.len() >= 24 {
            self.evicted
                .borrow_mut()
                .extend(cache.drain().map(|(_, v)| v));
        }
        cache.insert(key, turned.clone());
        turned
    }

    pub fn layout(&self, t: &RenderText, xfrm: &Xfrm) -> Rc<TextLayout> {
        let paras = layout_paragraphs(t);
        let width = text_width(t, xfrm);
        let key = layout_key(&paras, width);
        if let Some(l) = self.texts.borrow().get(&key) {
            return l.clone();
        }
        let setup = PageSetup {
            width,
            height: 1.0e7,
            margin_top: 0.0,
            margin_bottom: 0.0,
            margin_left: 0.0,
            margin_right: 0.0,
        };
        let paged = self.engine.borrow_mut().layout_document(&paras, setup);
        let height = paged
            .pages
            .first()
            .and_then(|p| p.lines.last())
            .map_or(0.0, |l| l.top + l.height);
        let layout = Rc::new(TextLayout {
            paged,
            texts: paras.into_iter().map(|p| p.text).collect(),
            height,
        });
        let mut cache = self.texts.borrow_mut();
        if cache.len() > 2000 {
            cache.clear();
        }
        cache.insert(key, layout.clone());
        layout
    }

    fn image(&self, pres: &Presentation, part: &str, cx: &App) -> Option<Arc<RenderImage>> {
        if let Some(i) = self.images.borrow().get(part) {
            return i.clone();
        }
        let decoded = pres.part_bytes(part).and_then(|bytes| {
            let format = match part.rsplit('.').next()?.to_ascii_lowercase().as_str() {
                "png" => ImageFormat::Png,
                "jpg" | "jpeg" => ImageFormat::Jpeg,
                "gif" => ImageFormat::Gif,
                "bmp" => ImageFormat::Bmp,
                "svg" => ImageFormat::Svg,
                "webp" => ImageFormat::Webp,
                "tif" | "tiff" => ImageFormat::Tiff,
                _ => return None,
            };
            Image::from_bytes(format, bytes.as_ref().clone())
                .to_image_data(cx.svg_renderer())
                .map_err(|e| log::warn!("slides: can't decode {part}: {e:#}"))
                .ok()
        });
        self.images
            .borrow_mut()
            .insert(part.to_owned(), decoded.clone());
        decoded
    }

    /// Text placement of a resolved body inside `xfrm`.
    pub fn place(&self, t: &RenderText, xfrm: &Xfrm) -> LaidText {
        let layout = self.layout(t, xfrm);
        let inner_h = (xfrm.cy - t.insets[1] - t.insets[3]) as f32 / EMU_PER_PX;
        let free = inner_h - layout.height;
        let dy = t.insets[1] as f32 / EMU_PER_PX
            + match t.anchor {
                Anchor::Top => 0.0,
                Anchor::Middle => free / 2.0,
                Anchor::Bottom => free,
            };
        LaidText {
            layout,
            dx: t.insets[0] as f32 / EMU_PER_PX,
            dy,
        }
    }

    /// Everything to draw for slide `slide`. `preview` overrides one slide
    /// shape's position (live drag); `prompts` shows placeholder prompts.
    pub fn items(
        &self,
        pres: &Presentation,
        slide: usize,
        prompts: bool,
        preview: Option<(usize, Xfrm)>,
        cx: &App,
    ) -> Vec<Item> {
        pres.render_list(slide, prompts)
            .into_iter()
            .map(|r| {
                let xfrm = match (r.layer, preview) {
                    (Layer::Slide(i), Some((j, x))) if i == j => x,
                    _ => r.xfrm,
                };
                let look = match &r.shape.kind {
                    ShapeKind::Shape { geom, .. } => Look::Geometry {
                        geom: geom.clone(),
                        fill: r.shape.look.fill,
                        line: r.shape.look.line,
                    },
                    ShapeKind::Picture { part, .. } => {
                        Look::Picture(part.as_deref().and_then(|p| {
                            let img = self.image(pres, p, cx)?;
                            Some(if xfrm.rot != 0 || xfrm.flip_h || xfrm.flip_v {
                                self.turned_image(p, &img, &xfrm)
                            } else {
                                img
                            })
                        }))
                    }
                    ShapeKind::Other { .. } => Look::Other,
                };
                let text = r.text.as_ref().map(|t| self.place(t, &xfrm));
                Item {
                    layer: r.layer,
                    xfrm,
                    look,
                    text,
                    empty_placeholder: r.is_empty_placeholder(),
                }
            })
            .collect()
    }
}

/// Slide → screen mapping: `origin` is the slide's top left on screen,
/// `scale` screen px per EMU.
#[derive(Clone, Copy, Debug)]
pub(crate) struct View {
    pub origin: Point<Pixels>,
    pub scale: f32,
}

impl View {
    pub fn point(self, x: f32, y: f32) -> Point<Pixels> {
        point(
            self.origin.x + px(x * self.scale),
            self.origin.y + px(y * self.scale),
        )
    }

    pub fn rect(self, x: &Xfrm) -> Bounds<Pixels> {
        Bounds::from_corners(
            self.point(x.x as f32, x.y as f32),
            self.point((x.x + x.cx) as f32, (x.y + x.cy) as f32),
        )
    }

    /// A screen point in EMU.
    pub fn to_emu(self, p: Point<Pixels>) -> (i64, i64) {
        (
            (f32::from(p.x - self.origin.x) / self.scale) as i64,
            (f32::from(p.y - self.origin.y) / self.scale) as i64,
        )
    }

    /// Screen px per slide px.
    pub fn px_scale(self) -> f32 {
        self.scale * EMU_PER_PX
    }
}

/// A shape's rotation on screen: about the centre of its box.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Turn {
    pub c: Point<Pixels>,
    sin: f32,
    cos: f32,
}

impl Turn {
    /// `None` for an unrotated shape.
    pub fn of(x: &Xfrm, view: View) -> Option<Self> {
        (x.rot != 0).then(|| {
            let (sin, cos) = (x.angle() as f32).sin_cos();
            Turn {
                c: view.rect(x).center(),
                sin,
                cos,
            }
        })
    }

    pub fn apply(&self, p: Point<Pixels>) -> Point<Pixels> {
        let (dx, dy) = (f32::from(p.x - self.c.x), f32::from(p.y - self.c.y));
        point(
            self.c.x + px(dx * self.cos - dy * self.sin),
            self.c.y + px(dx * self.sin + dy * self.cos),
        )
    }

    /// The inverse of [`Turn::apply`].
    pub fn unapply(&self, p: Point<Pixels>) -> Point<Pixels> {
        let (dx, dy) = (f32::from(p.x - self.c.x), f32::from(p.y - self.c.y));
        point(
            self.c.x + px(dx * self.cos + dy * self.sin),
            self.c.y + px(-dx * self.sin + dy * self.cos),
        )
    }
}

pub(crate) fn turned(t: Option<Turn>, p: Point<Pixels>) -> Point<Pixels> {
    t.map_or(p, |t| t.apply(p))
}

fn rect_points(b: Bounds<Pixels>) -> Vec<Point<Pixels>> {
    vec![b.origin, b.top_right(), b.bottom_right(), b.bottom_left()]
}

fn round_rect_points(b: Bounds<Pixels>) -> Vec<Point<Pixels>> {
    let r = f32::from(b.size.width.min(b.size.height)) * 0.16;
    let mut out = Vec::with_capacity(40);
    let corners = [
        (b.right() - px(r), b.top() + px(r), -90.0f32),
        (b.right() - px(r), b.bottom() - px(r), 0.0),
        (b.left() + px(r), b.bottom() - px(r), 90.0),
        (b.left() + px(r), b.top() + px(r), 180.0),
    ];
    for (cx, cy, start) in corners {
        for i in 0..=8 {
            let a = (start + i as f32 * 90.0 / 8.0).to_radians();
            out.push(point(cx + px(r * a.cos()), cy + px(r * a.sin())));
        }
    }
    out
}

/// Fill a polygon (screen space).
pub(crate) fn fill_polygon(window: &mut Window, pts: &[Point<Pixels>], color: Hsla) {
    let mut p = PathBuilder::fill();
    p.add_polygon(pts, true);
    if let Ok(path) = p.build() {
        window.paint_path(path, color);
    }
}

/// Stroke a closed polygon (screen space).
pub(crate) fn stroke_polygon(window: &mut Window, pts: &[Point<Pixels>], color: Hsla, w: Pixels) {
    let mut p = PathBuilder::stroke(w);
    p.add_polygon(pts, true);
    if let Ok(path) = p.build() {
        window.paint_path(path, color);
    }
}

/// The corners of a box turned with its shape.
pub(crate) fn turned_rect(b: Bounds<Pixels>, t: Option<Turn>) -> Vec<Point<Pixels>> {
    rect_points(b).into_iter().map(|p| turned(t, p)).collect()
}

fn ellipse_points(b: Bounds<Pixels>) -> Vec<Point<Pixels>> {
    let c = b.center();
    let (rx, ry) = (
        f32::from(b.size.width) / 2.0,
        f32::from(b.size.height) / 2.0,
    );
    (0..64)
        .map(|i| {
            let a = i as f32 / 64.0 * std::f32::consts::TAU;
            point(c.x + px(rx * a.cos()), c.y + px(ry * a.sin()))
        })
        .collect()
}

fn paint_outline(
    window: &mut Window,
    b: Bounds<Pixels>,
    ellipse: bool,
    color: Hsla,
    width: Pixels,
) {
    if ellipse {
        let mut p = PathBuilder::stroke(width);
        p.add_polygon(&ellipse_points(b), true);
        if let Ok(path) = p.build() {
            window.paint_path(path, color);
        }
    } else {
        window.paint_quad(quad(
            b,
            Corners::default(),
            transparent_black(),
            Edges::all(width),
            color,
            BorderStyle::Solid,
        ));
    }
}

/// How `paint_slide` draws: `other` is the colour for objects Kantor
/// can't draw yet (a chrome token); `editor` adds the editor-only dashed
/// outline around empty placeholders (not in thumbnails or presenting).
#[derive(Clone, Copy)]
pub(crate) struct Surface {
    pub other: Hsla,
    pub editor: bool,
}

/// Dashed outline of an empty placeholder: grey that reads on the slide's
/// own background (dark on light slides, light on dark ones).
pub(crate) fn placeholder_outline(background: Rgb) -> Hsla {
    let [r, g, b] = background.0;
    let lum = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0;
    let l = if lum > 0.5 { 0.0 } else { 1.0 };
    Hsla {
        h: 0.,
        s: 0.,
        l,
        a: 0.38,
    }
}

/// Paint `items` with `view` into `slide_bounds`.
pub(crate) fn paint_slide(
    items: &[Item],
    background: Rgb,
    slide_bounds: Bounds<Pixels>,
    view: View,
    surface: Surface,
    bridge: &RefCell<FontBridge>,
    window: &mut Window,
) {
    let other = surface.other;
    window.paint_quad(fill(slide_bounds, hsla(background)));
    window.with_content_mask(
        Some(ContentMask {
            bounds: slide_bounds,
        }),
        |window| {
            for item in items {
                let b = view.rect(&item.xfrm);
                let turn = Turn::of(&item.xfrm, view);
                match &item.look {
                    Look::Geometry {
                        geom,
                        fill: f,
                        line,
                    } => {
                        let ellipse = *geom == Geom::Ellipse;
                        let outline = || -> Vec<Point<Pixels>> {
                            let pts = match geom {
                                Geom::Ellipse => ellipse_points(b),
                                Geom::RoundRect => round_rect_points(b),
                                _ => rect_points(b),
                            };
                            pts.into_iter().map(|p| turned(turn, p)).collect()
                        };
                        if let Some(c) = f
                            && *geom != Geom::Line
                        {
                            if turn.is_some() || ellipse {
                                fill_polygon(window, &outline(), hsla(*c));
                            } else {
                                let r = if *geom == Geom::RoundRect {
                                    px(f32::from(b.size.width.min(b.size.height)) * 0.16)
                                } else {
                                    px(0.)
                                };
                                window.paint_quad(fill(b, hsla(*c)).corner_radii(r));
                            }
                        }
                        if let Some((c, w)) = line {
                            let w = px((*w as f32 * view.scale).max(1.0));
                            if *geom == Geom::Line {
                                // Flips choose the diagonal.
                                let x = &item.xfrm;
                                let (x0, x1) = if x.flip_h {
                                    (b.right(), b.left())
                                } else {
                                    (b.left(), b.right())
                                };
                                let (y0, y1) = if x.flip_v {
                                    (b.bottom(), b.top())
                                } else {
                                    (b.top(), b.bottom())
                                };
                                let mut p = PathBuilder::stroke(w);
                                p.move_to(turned(turn, point(x0, y0)));
                                p.line_to(turned(turn, point(x1, y1)));
                                if let Ok(path) = p.build() {
                                    window.paint_path(path, hsla(*c));
                                }
                            } else if turn.is_some() || ellipse {
                                stroke_polygon(window, &outline(), hsla(*c), w);
                            } else {
                                paint_outline(window, b, false, hsla(*c), w);
                            }
                        }
                    }
                    Look::Picture(Some(img)) => {
                        // Rotated / flipped pictures are pre-turned rasters
                        // of the turned box's bounding box.
                        let bb = view.rect(&item.xfrm.bounding_box());
                        let _ =
                            window.paint_image(bb, bb, Corners::default(), img.clone(), 0, false);
                    }
                    Look::Picture(None) | Look::Other => {
                        if turn.is_some() {
                            let pts = turned_rect(b, turn);
                            fill_polygon(window, &pts, other.opacity(0.12));
                            stroke_polygon(window, &pts, other.opacity(0.6), px(1.));
                        } else {
                            window.paint_quad(fill(b, other.opacity(0.12)));
                            paint_outline(window, b, false, other.opacity(0.6), px(1.));
                        }
                    }
                }
                if surface.editor && item.empty_placeholder {
                    if turn.is_some() {
                        let pts = turned_rect(b, turn);
                        stroke_polygon(window, &pts, placeholder_outline(background), px(1.));
                    } else {
                        window.paint_quad(quad(
                            b,
                            Corners::default(),
                            transparent_black(),
                            Edges::all(px(1.)),
                            placeholder_outline(background),
                            BorderStyle::Dashed,
                        ));
                    }
                }
                if let Some(t) = &item.text {
                    paint_text(t, &item.xfrm, view, bridge, window);
                }
            }
        },
    );
}

pub(crate) fn text_origin(t: &LaidText, xfrm: &Xfrm, view: View) -> Point<Pixels> {
    view.point(
        xfrm.x as f32 + t.dx * EMU_PER_PX,
        xfrm.y as f32 + t.dy * EMU_PER_PX,
    )
}

fn paint_text(
    t: &LaidText,
    xfrm: &Xfrm,
    view: View,
    bridge: &RefCell<FontBridge>,
    window: &mut Window,
) {
    let origin = text_origin(t, xfrm, view);
    let turn = Turn::of(xfrm, view);
    let s = view.px_scale();
    let ts = window.text_system().clone();
    let mut bridge = bridge.borrow_mut();
    let paged = &t.layout.paged;
    for run in paged.glyph_runs(0) {
        let para_text = t.layout.texts.get(run.para).map_or("", String::as_str);
        let sample = paged
            .run_text(&run, para_text)
            .get(run.text_range.clone())
            .unwrap_or_default();
        let Some(font_id) = bridge.font_id(&run.font, sample, &ts) else {
            continue;
        };
        let [r, g, b, a] = run.color.0;
        let color: Hsla = gpui_kit::Rgba {
            r: r as f32 / 255.,
            g: g as f32 / 255.,
            b: b as f32 / 255.,
            a: a as f32 / 255.,
        }
        .into();
        let size = px(run.font_size * s);
        // GPUI can't turn glyphs: in a rotated shape each glyph is placed
        // along the rotation but drawn upright.
        for glyph in &run.glyphs {
            let o = point(origin.x + px(glyph.x * s), origin.y + px(glyph.y * s));
            let o = turned(turn, o);
            let _ = window.paint_glyph(o, font_id, GlyphId(glyph.id), size, color);
        }
        if let Some((offset, thick)) = run.underline {
            let y = run.baseline + offset;
            let b = Bounds::new(
                point(origin.x + px(run.origin_x * s), origin.y + px(y * s)),
                size_of(run.advance * s, (thick * s).max(1.0)),
            );
            if turn.is_some() {
                fill_polygon(window, &turned_rect(b, turn), color);
            } else {
                window.paint_quad(fill(b, color));
            }
        }
    }
}

fn size_of(w: f32, h: f32) -> Size<Pixels> {
    size(px(w), px(h))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 × 1 picture (red, blue) turned 90° clockwise is 1 × 2 with red
    /// on top; flipped horizontally it's (blue, red).
    #[std::prelude::v1::test]
    fn pictures_turn_and_flip() {
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let src: Vec<u8> = [red, blue].concat();
        let (px_, w, h) = turn_pixels(&src, (2, 1), (2.0, 1.0), 90f32.to_radians(), (false, false));
        assert_eq!((w, h), (1, 2));
        assert_eq!(&px_[0..4], &red);
        assert_eq!(&px_[4..8], &blue);
        let (px_, w, h) = turn_pixels(&src, (2, 1), (2.0, 1.0), 0.0, (true, false));
        assert_eq!((w, h), (2, 1));
        assert_eq!(&px_[0..4], &blue);
        // 45°: the corners of the bounding box are transparent.
        let (px_, w, _) = turn_pixels(
            &src,
            (2, 1),
            (20.0, 20.0),
            45f32.to_radians(),
            (false, false),
        );
        assert_eq!(&px_[0..4], &[0, 0, 0, 0]);
        assert!(w > 2);
    }

    #[std::prelude::v1::test]
    fn turn_round_trips() {
        let x = Xfrm::new(0, 0, 1000, 500).with_degrees(30.0);
        let view = View {
            origin: point(px(10.), px(10.)),
            scale: 0.1,
        };
        let t = Turn::of(&x, view).unwrap();
        let p = point(px(17.), px(3.));
        let back = t.unapply(t.apply(p));
        assert!((f32::from(back.x) - 17.).abs() < 1e-3 && (f32::from(back.y) - 3.).abs() < 1e-3);
    }
}
