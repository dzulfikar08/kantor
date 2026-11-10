//! Slide → [`SlideScene`].

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use docs_layout::Engine;
use slides_engine::frame::{ChartRef, Frame, FrameContent, Group, Vert, compose};
use slides_engine::{
    EMU_PER_PX, Fill, Layer, LineStyle, Presentation, RenderShape, RenderText, Rgb, Shape,
    ShapeKind, Xfrm,
};

use crate::geom::{Affine, P, Path, PathFill, p, shape_outline};
use crate::paint::{Head, arrow_inset, arrowhead, dash_array, fill_paint, rgba};
use crate::text::{LaidText, TextCache, glyph_runs};
use crate::{FillRule, Item, Paint, Picture, Prim, SlideScene, Stroke, Text};

/// What to draw besides the slide itself.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Options {
    /// Prompts ("Click to add title") and dashed outlines in empty
    /// placeholders (the editor's canvas).
    pub prompts: bool,
    /// A slide shape drawn at another position (live drag).
    pub preview: Option<(usize, Xfrm)>,
    /// A slide shape being edited: its own (possibly empty) text, no prompt.
    pub editing: Option<usize>,
}

/// Builds scenes, caching text layouts and picture bytes.
#[derive(Default)]
pub struct Builder {
    pub texts: TextCache,
    bytes: RefCell<HashMap<String, Option<Arc<Vec<u8>>>>>,
    /// SmartArt drawings by data part.
    diagrams: RefCell<HashMap<String, Option<Arc<Group>>>>,
}

pub(crate) fn px(emu: i64) -> f32 {
    emu as f32 / EMU_PER_PX
}

/// Shape box → slide px: flip in the box, turn about its centre, place.
pub fn shape_transform(x: &Xfrm) -> Affine {
    let (w, h) = (px(x.cx), px(x.cy));
    let flip = Affine::scale(
        if x.flip_h { -1.0 } else { 1.0 },
        if x.flip_v { -1.0 } else { 1.0 },
    );
    Affine::translate(-w / 2.0, -h / 2.0)
        .then(flip)
        .then(Affine::rotate(x.angle() as f32))
        .then(Affine::translate(px(x.x) + w / 2.0, px(x.y) + h / 2.0))
}

/// Shape box → slide px for its text: turned, never flipped.
pub fn text_transform(x: &Xfrm) -> Affine {
    let (w, h) = (px(x.cx), px(x.cy));
    Affine::translate(-w / 2.0, -h / 2.0)
        .then(Affine::rotate(x.angle() as f32))
        .then(Affine::translate(px(x.x) + w / 2.0, px(x.y) + h / 2.0))
}

impl Builder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Text placement of a resolved body inside `xfrm` (cached layout).
    pub fn place(&self, engine: &mut Engine, t: &RenderText, xfrm: &Xfrm) -> LaidText {
        self.texts.place(engine, t, xfrm)
    }

    fn bytes(&self, pres: &Presentation, part: &str) -> Option<Arc<Vec<u8>>> {
        if let Some(b) = self.bytes.borrow().get(part) {
            return b.clone();
        }
        let b = pres.part_bytes(part);
        let mut cache = self.bytes.borrow_mut();
        if cache.len() > 256 {
            cache.clear();
        }
        cache.insert(part.to_owned(), b.clone());
        b
    }

    /// Forget cached picture bytes (the presentation was replaced).
    pub fn clear_pictures(&self) {
        self.bytes.borrow_mut().clear();
        self.diagrams.borrow_mut().clear();
    }

    /// Everything drawn on slide `slide`.
    pub fn slide(
        &self,
        engine: &mut Engine,
        pres: &Presentation,
        slide: usize,
        opts: &Options,
    ) -> SlideScene {
        let (w, h) = (px(pres.size.cx), px(pres.size.cy));
        let mut items = Vec::new();
        let bg = pres.background_fill(slide);
        self.background(pres, &bg, w, h, &mut items);
        let mut list = pres.render_list(slide, opts.prompts);
        if let Some(i) = opts.editing
            && opts.prompts
        {
            // The edited placeholder shows its own text, even when empty.
            let own = pres.render_list(slide, false);
            if let (Some(a), Some(b)) = (
                list.iter_mut().find(|r| r.layer == Layer::Slide(i)),
                own.into_iter().find(|r| r.layer == Layer::Slide(i)),
            ) {
                a.text = b.text;
            }
        }
        for r in &list {
            let xfrm = match (r.layer, opts.preview) {
                (Layer::Slide(i), Some((j, x))) if i == j => x,
                _ => r.xfrm,
            };
            self.shape(engine, pres, slide, r, &xfrm, opts.prompts, &mut items);
        }
        SlideScene {
            width: w,
            height: h,
            background: pres.background(slide),
            items,
            hidden: pres.slide(slide).hidden,
        }
    }

    fn background(&self, pres: &Presentation, fill: &Fill, w: f32, h: f32, out: &mut Vec<Item>) {
        let rect = Path::rect(0.0, 0.0, w, h);
        match fill {
            Fill::Picture(b) => {
                // Pictures may be transparent: white below, as PowerPoint.
                out.push(Item {
                    layer: None,
                    transform: Affine::IDENTITY,
                    prim: Prim::Fill {
                        path: rect.clone(),
                        paint: Paint::Solid(rgba(Rgb([255, 255, 255]), 255)),
                        rule: FillRule::NonZero,
                    },
                });
                if let Some(pic) = self.blip(pres, b, [0.0, 0.0, w, h], None) {
                    out.push(Item {
                        layer: None,
                        transform: Affine::IDENTITY,
                        prim: Prim::Picture(pic),
                    });
                }
            }
            f => {
                let paint = fill_paint(f, PathFill::Norm, w, h, &Affine::IDENTITY)
                    .unwrap_or(Paint::Solid(rgba(Rgb([255, 255, 255]), 255)));
                out.push(Item {
                    layer: None,
                    transform: Affine::IDENTITY,
                    prim: Prim::Fill {
                        path: rect,
                        paint,
                        rule: FillRule::NonZero,
                    },
                });
            }
        }
    }

    pub(crate) fn blip(
        &self,
        pres: &Presentation,
        b: &slides_engine::BlipFill,
        rect: [f32; 4],
        clip: Option<Path>,
    ) -> Option<Picture> {
        let part = b.part.as_deref()?;
        let data = self.bytes(pres, part)?;
        Some(Picture {
            part: Arc::from(part),
            data,
            rect,
            crop: b.crop.map(|v| v as f32 / 100_000.0),
            alpha: b.alpha as f32 / 255.0,
            clip,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn shape(
        &self,
        engine: &mut Engine,
        pres: &Presentation,
        slide: usize,
        r: &RenderShape,
        xfrm: &Xfrm,
        editor: bool,
        out: &mut Vec<Item>,
    ) {
        let outline = editor && r.is_empty_placeholder();
        let d = Draw {
            slide,
            layer: Some(r.layer),
        };
        self.draw(
            engine,
            pres,
            d,
            r.shape,
            xfrm,
            r.text.as_ref(),
            outline,
            out,
        );
    }

    /// One shape at `xfrm` with its resolved text.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        engine: &mut Engine,
        pres: &Presentation,
        d: Draw,
        shape: &Shape,
        xfrm: &Xfrm,
        text: Option<&RenderText>,
        outline: bool,
        out: &mut Vec<Item>,
    ) {
        let layer = d.layer;
        let (w, h) = (px(xfrm.cx), px(xfrm.cy));
        let t = shape_transform(xfrm);
        let push = |out: &mut Vec<Item>, prim: Prim| {
            out.push(Item {
                layer,
                transform: t,
                prim,
            })
        };
        let look = &shape.look;
        // The fill and line as shown: edits in Kantor replace what was read.
        let fill = if shape.paint.fill.is_some() {
            look.fill.map_or(Fill::None, |c| Fill::Solid(c, 255))
        } else {
            look.fill_as_read
                .clone()
                .or_else(|| look.fill.map(|c| Fill::Solid(c, 255)))
                .unwrap_or(Fill::None)
        };
        let line = look.line.map(|(c, wd)| {
            let style = &look.line_style;
            let alpha = if shape.paint.line.is_some() {
                255
            } else {
                style.alpha
            };
            (rgba(c, alpha), (wd as f32 / EMU_PER_PX).max(0.0), style)
        });
        match &shape.kind {
            ShapeKind::Shape { .. } => {
                let outline = shape_outline(shape, w, h);
                for sp in outline.iter().filter(|s| s.fill != PathFill::None) {
                    match &fill {
                        Fill::Picture(b) => {
                            if let Some(pic) =
                                self.blip(pres, b, [0.0, 0.0, w, h], Some(sp.path.clone()))
                            {
                                push(out, Prim::Picture(pic));
                            }
                        }
                        f => {
                            if let Some(paint) = fill_paint(f, sp.fill, w, h, &t) {
                                push(
                                    out,
                                    Prim::Fill {
                                        path: sp.path.clone(),
                                        paint,
                                        rule: FillRule::EvenOdd,
                                    },
                                );
                            }
                        }
                    }
                }
                if let Some((color, width, style)) = &line
                    && *width > 0.0
                {
                    let stroked: Vec<Path> = outline
                        .iter()
                        .filter(|s| s.stroke)
                        .map(|s| s.path.clone())
                        .collect();
                    strokes(stroked, *color, *width, style, &mut |prim| push(out, prim));
                }
            }
            ShapeKind::Picture { part, .. } => {
                if let Some(paint) = fill_paint(&fill, PathFill::Norm, w, h, &t) {
                    push(
                        out,
                        Prim::Fill {
                            path: Path::rect(0.0, 0.0, w, h),
                            paint,
                            rule: FillRule::NonZero,
                        },
                    );
                }
                let pic = part.as_deref().and_then(|part| {
                    let data = self.bytes(pres, part)?;
                    Some(Picture {
                        part: Arc::from(part),
                        data,
                        rect: [0.0, 0.0, w, h],
                        crop: look.picture.crop.map(|v| v as f32 / 100_000.0),
                        alpha: look.picture.alpha as f32 / 255.0,
                        clip: None,
                    })
                });
                match pic {
                    Some(pic) => push(out, Prim::Picture(pic)),
                    None => push(out, Prim::Unsupported { w, h }),
                }
                if let Some((color, width, style)) = &line
                    && *width > 0.0
                {
                    let rect = vec![Path::rect(0.0, 0.0, w, h)];
                    strokes(rect, *color, *width, style, &mut |prim| push(out, prim));
                }
            }
            ShapeKind::Other { frame, .. } => {
                let own = shape.xfrm.unwrap_or(*xfrm);
                if !self.frame(engine, pres, d, frame, &own, xfrm, out) {
                    push(out, Prim::Unsupported { w, h });
                }
            }
        }
        let tt = text_transform(xfrm);
        if outline {
            out.push(Item {
                layer,
                transform: tt,
                prim: Prim::PlaceholderOutline { w, h },
            });
        }
        if let Some(text) = text {
            let local = Xfrm::new(0, 0, xfrm.cx, xfrm.cy);
            self.push_text(engine, text, &local, &tt, layer, out);
        }
    }

    /// The content of a group or graphic frame drawn at `xfrm` (`own` is
    /// its box as read). `false` if there's nothing Kantor can draw.
    #[allow(clippy::too_many_arguments)]
    fn frame(
        &self,
        engine: &mut Engine,
        pres: &Presentation,
        d: Draw,
        frame: &Frame,
        own: &Xfrm,
        xfrm: &Xfrm,
        out: &mut Vec<Item>,
    ) -> bool {
        match frame.get() {
            Some(FrameContent::Group(g)) => {
                self.group(engine, pres, d, g, &g.child_box, xfrm, out);
                true
            }
            Some(FrameContent::Table(t)) => self.table(engine, pres, d, t, xfrm, out),
            Some(FrameContent::Diagram(dg)) => {
                let Some(key) = dg.data.clone() else {
                    return false;
                };
                let cached = self.diagrams.borrow().get(&key).cloned();
                let g = match cached {
                    Some(g) => g,
                    None => {
                        let g = pres.diagram_drawing(d.slide, dg).map(Arc::new);
                        self.diagrams.borrow_mut().insert(key, g.clone());
                        g
                    }
                };
                let Some(g) = g else { return false };
                // The drawing is in the frame's own space.
                let space = Xfrm::new(0, 0, own.cx, own.cy);
                self.group(engine, pres, d, &g, &space, xfrm, out);
                true
            }
            Some(FrameContent::Chart(c)) => self.chart_frame(engine, pres, d, c, xfrm, out),
            None => false,
        }
    }

    /// Charts: drawn by the chart renderer once it lands; `false` keeps
    /// the placeholder box.
    #[allow(clippy::too_many_arguments)]
    fn chart_frame(
        &self,
        _engine: &mut Engine,
        _pres: &Presentation,
        _d: Draw,
        _chart: &ChartRef,
        _xfrm: &Xfrm,
        _out: &mut Vec<Item>,
    ) -> bool {
        false
    }

    /// The children of a group (child space `space`) drawn into `xfrm`.
    #[allow(clippy::too_many_arguments)]
    fn group(
        &self,
        engine: &mut Engine,
        pres: &Presentation,
        d: Draw,
        g: &Group,
        space: &Xfrm,
        xfrm: &Xfrm,
        out: &mut Vec<Item>,
    ) {
        for (i, child) in g.children.iter().enumerate() {
            let Some(own) = child.xfrm else { continue };
            let at = compose(xfrm, space, &own);
            match g.text_boxes.get(i).copied().flatten() {
                Some(tb) => {
                    // SmartArt: the text has its own box.
                    let tx = compose(xfrm, space, &tb);
                    self.draw(engine, pres, d, child, &at, None, false, out);
                    if let Some(text) = pres.shape_text(d.slide, child, &tx, true) {
                        let local = Xfrm::new(0, 0, tx.cx, tx.cy);
                        self.push_text(engine, &text, &local, &text_transform(&tx), d.layer, out);
                    }
                }
                None => {
                    let text = pres.shape_text(d.slide, child, &at, false);
                    self.draw(engine, pres, d, child, &at, text.as_ref(), false, out);
                }
            }
        }
    }

    /// Text laid out in `bx` (EMU, in the coordinates `base` maps to slide
    /// px), turned a quarter for vertical text.
    pub(crate) fn push_text(
        &self,
        engine: &mut Engine,
        text: &RenderText,
        bx: &Xfrm,
        base: &Affine,
        layer: Option<Layer>,
        out: &mut Vec<Item>,
    ) {
        let (x, y, w, h) = (px(bx.x), px(bx.y), px(bx.cx), px(bx.cy));
        let (laid, transform) = match text.vert {
            Vert::Horz => {
                let laid = self
                    .texts
                    .place(engine, text, &Xfrm::new(0, 0, bx.cx, bx.cy));
                (laid, Affine::translate(x, y).then(*base))
            }
            v => {
                // Laid out in the box turned a quarter: its insets turn too.
                let [l, t, r, b] = text.insets;
                let (insets, angle) = match v {
                    Vert::Down => ([t, r, b, l], std::f32::consts::FRAC_PI_2),
                    _ => ([b, l, t, r], -std::f32::consts::FRAC_PI_2),
                };
                let turned = RenderText {
                    insets,
                    ..text.clone()
                };
                let laid = self
                    .texts
                    .place(engine, &turned, &Xfrm::new(0, 0, bx.cy, bx.cx));
                let t = Affine::translate(-h / 2.0, -w / 2.0)
                    .then(Affine::rotate(angle))
                    .then(Affine::translate(x + w / 2.0, y + h / 2.0))
                    .then(*base);
                (laid, t)
            }
        };
        let runs = glyph_runs(&laid.layout);
        if !runs.is_empty() {
            out.push(Item {
                layer,
                transform,
                prim: Prim::Text(Text {
                    origin: p(laid.dx, laid.dy),
                    runs,
                }),
            });
        }
    }
}

/// Where a shape is drawn from: the slide (for its master, rels and
/// text styles) and the layer its items belong to.
#[derive(Clone, Copy)]
pub(crate) struct Draw {
    pub slide: usize,
    pub layer: Option<Layer>,
}

/// The strokes of `paths` (and their arrowheads: the head on the first
/// open path's start, the tail on the last one's end).
fn strokes(
    mut paths: Vec<Path>,
    color: crate::Rgba,
    width: f32,
    style: &LineStyle,
    push: &mut dyn FnMut(Prim),
) {
    let stroke = Stroke {
        width,
        color,
        dash: dash_array(style, width),
        cap: style.cap,
        join: style.join,
    };
    let mut heads: Vec<(P, P, slides_engine::Arrow)> = Vec::new();
    if let Some(a) = style.head
        && let Some(first) = paths.iter_mut().find(|p| !p.is_closed())
        && let Some((at, dir)) = first.start_end()
    {
        first.shorten(true, arrow_inset(&a, width));
        heads.push((at, dir, a));
    }
    if let Some(a) = style.tail
        && let Some(last) = paths.iter_mut().rev().find(|p| !p.is_closed())
        && let Some((at, dir)) = last.finish_end()
    {
        last.shorten(false, arrow_inset(&a, width));
        heads.push((at, dir, a));
    }
    for path in paths {
        push(Prim::Stroke {
            path,
            stroke: stroke.clone(),
        });
    }
    for (at, dir, a) in heads {
        match arrowhead(&a, at, dir, width) {
            Head::Filled(path) => push(Prim::Fill {
                path,
                paint: Paint::Solid(color),
                rule: FillRule::NonZero,
            }),
            Head::Open(path) => push(Prim::Stroke {
                path,
                stroke: Stroke {
                    dash: None,
                    ..stroke.clone()
                },
            }),
        }
    }
}
