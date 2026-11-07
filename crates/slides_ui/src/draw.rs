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
use docs_layout::{Engine, PageSetup, PagedDocument, Rgba as LayoutRgba, Span};
use docs_ui::FontBridge;
use gpui_kit::*;
use slides_engine::{
    Anchor, EMU_PER_PX, Geom, Layer, Presentation, RenderText, Rgb, ShapeKind, Xfrm,
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
}

/// Caches shared by every surface of one view.
pub(crate) struct Painter {
    pub engine: Rc<RefCell<Engine>>,
    pub bridge: Rc<RefCell<FontBridge>>,
    texts: RefCell<HashMap<u64, Rc<TextLayout>>>,
    images: RefCell<HashMap<String, Option<Arc<RenderImage>>>>,
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
    let alpha = if t.prompt { 140 } else { 255 };
    t.paras
        .iter()
        .map(|p| {
            let mut lp = docs_layout::Paragraph::new(p.text.clone());
            lp.family = docs_engine::font_stack(&p.base.font);
            lp.size_pt = p.base.size_pt;
            lp.line_height = Some(LineHeight::MetricsRelative(1.0));
            lp.space_before_pt = 0.0;
            lp.space_after_pt = 0.0;
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
        }
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
                        Look::Picture(part.as_deref().and_then(|p| self.image(pres, p, cx)))
                    }
                    ShapeKind::Other { .. } => Look::Other,
                };
                let text = r.text.as_ref().map(|t| self.place(t, &xfrm));
                Item {
                    layer: r.layer,
                    xfrm,
                    look,
                    text,
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

/// Paint `items` with `view`; `other` is the colour for objects Kantor
/// can't draw yet (a chrome token).
pub(crate) fn paint_slide(
    items: &[Item],
    background: Rgb,
    slide_bounds: Bounds<Pixels>,
    view: View,
    other: Hsla,
    bridge: &RefCell<FontBridge>,
    window: &mut Window,
) {
    window.paint_quad(fill(slide_bounds, hsla(background)));
    window.with_content_mask(
        Some(ContentMask {
            bounds: slide_bounds,
        }),
        |window| {
            for item in items {
                let b = view.rect(&item.xfrm);
                match &item.look {
                    Look::Geometry {
                        geom,
                        fill: f,
                        line,
                    } => {
                        let ellipse = *geom == Geom::Ellipse;
                        if let Some(c) = f {
                            if ellipse {
                                let mut p = PathBuilder::fill();
                                p.add_polygon(&ellipse_points(b), true);
                                if let Ok(path) = p.build() {
                                    window.paint_path(path, hsla(*c));
                                }
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
                                let mut p = PathBuilder::stroke(w);
                                p.move_to(b.origin);
                                p.line_to(b.bottom_right());
                                if let Ok(path) = p.build() {
                                    window.paint_path(path, hsla(*c));
                                }
                            } else {
                                paint_outline(window, b, ellipse, hsla(*c), w);
                            }
                        }
                    }
                    Look::Picture(Some(img)) => {
                        let _ = window.paint_image(b, b, Corners::default(), img.clone(), 0, false);
                    }
                    Look::Picture(None) | Look::Other => {
                        window.paint_quad(fill(b, other.opacity(0.12)));
                        paint_outline(window, b, false, other.opacity(0.6), px(1.));
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
    let s = view.px_scale();
    let ts = window.text_system().clone();
    let mut bridge = bridge.borrow_mut();
    let paged = &t.layout.paged;
    for run in paged.glyph_runs(0) {
        let sample = t
            .layout
            .texts
            .get(run.para)
            .and_then(|x| x.get(run.text_range.clone()))
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
        for glyph in &run.glyphs {
            let o = point(origin.x + px(glyph.x * s), origin.y + px(glyph.y * s));
            let _ = window.paint_glyph(o, font_id, GlyphId(glyph.id), size, color);
        }
        if let Some((offset, thick)) = run.underline {
            let y = run.baseline + offset;
            let b = Bounds::new(
                point(origin.x + px(run.origin_x * s), origin.y + px(y * s)),
                size_of(run.advance * s, (thick * s).max(1.0)),
            );
            window.paint_quad(fill(b, color));
        }
    }
}

fn size_of(w: f32, h: f32) -> Size<Pixels> {
    size(px(w), px(h))
}
