//! Shapes drawn without a presentation behind them: a worksheet's shapes,
//! text boxes, connectors and groups (Kantor Sheets). The same outlines,
//! fills, lines, arrowheads and text as slide shapes; picture fills and a
//! group's pictures from bytes the caller has (`pictures`); outer shadows;
//! 3-D effects drawn approximately: an extrusion as darker copies behind
//! the shape, a bevel as shading over it. Charts inside groups are drawn
//! by the caller ([`Charts`]).

use std::sync::Arc;

use docs_layout::Engine;
use slides_engine::frame::{FrameContent, compose};
use slides_engine::{EMU_PER_PX, Fill, RenderText, Rgb, Shape, ShapeKind, Xfrm};

use crate::build::{Builder, px, shape_transform, strokes, text_transform};
use crate::geom::{Affine, Path, PathFill, p};
use crate::paint::{fill_paint, rgba};
use crate::{
    FillRule, Item, LineCap, LineJoin, Paint, Picture, Prim, Rgba, Stop, Stroke, shape_outline,
};

/// Picture bytes by the part name a shape's picture has.
pub type Pictures<'a> = &'a dyn Fn(&str) -> Option<Arc<Vec<u8>>>;

/// A chart inside a group, by its part name, drawn into `w` × `h` px (its
/// own coordinates); `None`: a box Kantor can't draw.
pub type Charts<'a> = &'a dyn Fn(&mut Engine, &str, f32, f32) -> Option<Vec<Item>>;

fn darker(c: Rgb, f: f32) -> Rgb {
    Rgb(c.0.map(|x| (x as f32 * f).round().clamp(0.0, 255.0) as u8))
}

impl Builder {
    /// `shape` drawn at `xfrm` (EMU, in the coordinates the items are in),
    /// its text (and that of a group's shapes) from `text`, its pictures
    /// from `pictures`.
    pub fn free_shape(
        &self,
        engine: &mut Engine,
        shape: &Shape,
        xfrm: &Xfrm,
        text: &dyn Fn(&Shape, &Xfrm) -> Option<RenderText>,
        pictures: Pictures,
        out: &mut Vec<Item>,
    ) {
        self.free_shape_with(engine, shape, xfrm, text, pictures, &|_, _, _, _| None, out);
    }

    /// [`Builder::free_shape`], with the charts inside groups from
    /// `charts`.
    #[allow(clippy::too_many_arguments)]
    pub fn free_shape_with(
        &self,
        engine: &mut Engine,
        shape: &Shape,
        xfrm: &Xfrm,
        text: &dyn Fn(&Shape, &Xfrm) -> Option<RenderText>,
        pictures: Pictures,
        charts: Charts,
        out: &mut Vec<Item>,
    ) {
        let (w, h) = (px(xfrm.cx), px(xfrm.cy));
        let t = shape_transform(xfrm);
        let push_at = |out: &mut Vec<Item>, at: Affine, prim: Prim| {
            out.push(Item {
                layer: None,
                transform: at,
                prim,
            })
        };
        let push = |out: &mut Vec<Item>, prim: Prim| push_at(out, t, prim);
        let picture = |part: &str, rect: [f32; 4], crop: [i32; 4], alpha: u8, clip| {
            Some(Picture {
                part: Arc::from(part),
                data: pictures(part)?,
                rect,
                crop: crop.map(|v| v as f32 / 100_000.0),
                alpha: alpha as f32 / 255.0,
                clip,
            })
        };
        match &shape.kind {
            ShapeKind::Shape { .. } => {
                let look = &shape.look;
                let fill = look
                    .fill_as_read
                    .clone()
                    .or_else(|| look.fill.map(|c| Fill::Solid(c, 255)))
                    .unwrap_or(Fill::None);
                let outline = shape_outline(shape, w, h);
                let filled: Vec<Path> = outline
                    .iter()
                    .filter(|s| s.fill != PathFill::None)
                    .map(|s| s.path.clone())
                    .collect();
                let has_fill = !matches!(fill, Fill::None);
                // What a shadow or extrusion takes the shape of: its fill,
                // else its line.
                let silhouette: Vec<(Path, bool)> = if has_fill {
                    filled.iter().map(|p| (p.clone(), true)).collect()
                } else {
                    outline
                        .iter()
                        .filter(|s| s.stroke)
                        .map(|s| (s.path.clone(), false))
                        .collect()
                };
                let line_w = look
                    .line
                    .map(|(_, wd)| (wd as f32 / EMU_PER_PX).max(1.0))
                    .unwrap_or(1.0);
                let mark = |out: &mut Vec<Item>, at: Affine, color: Rgba| {
                    for (path, fills) in &silhouette {
                        let prim = if *fills {
                            Prim::Fill {
                                path: path.clone(),
                                paint: Paint::Solid(color),
                                rule: FillRule::EvenOdd,
                            }
                        } else {
                            Prim::Stroke {
                                path: path.clone(),
                                stroke: Stroke {
                                    width: line_w,
                                    color,
                                    dash: None,
                                    cap: LineCap::Round,
                                    join: LineJoin::Round,
                                },
                            }
                        };
                        push_at(out, at, prim);
                    }
                };
                // The outer shadow: the shape moved by the shadow's
                // distance and direction, its blur as fainter widening
                // edges.
                if let Some(sh) = &look.effects.shadow {
                    let dist = px(sh.dist);
                    let dir = (sh.dir as f32 / 60_000.0).to_radians();
                    let at = t.then(Affine::translate(dist * dir.cos(), dist * dir.sin()));
                    let blur = px(sh.blur);
                    let layers = if blur > 0.5 { 3 } else { 0 };
                    if layers > 0 {
                        for k in (1..=layers).rev() {
                            let a = (sh.alpha as f32 / 2.0 / layers as f32).round().max(1.0) as u8;
                            for (path, _) in &silhouette {
                                push_at(
                                    out,
                                    at,
                                    Prim::Stroke {
                                        path: path.clone(),
                                        stroke: Stroke {
                                            width: blur * k as f32 / layers as f32
                                                + if has_fill { 0.0 } else { line_w },
                                            color: rgba(sh.color, a),
                                            dash: None,
                                            cap: LineCap::Round,
                                            join: LineJoin::Round,
                                        },
                                    },
                                );
                            }
                        }
                    }
                    let core = if layers > 0 { sh.alpha / 2 } else { sh.alpha };
                    mark(out, at, rgba(sh.color, core.max(1)));
                }
                // An extrusion: darker copies stepping back up and to the
                // right (Excel's default oblique view), behind the shape.
                if let Some((depth, color)) = look.effects.extrusion {
                    let d = px(depth).min(w.max(h));
                    let base = color
                        .or(fill.main_color())
                        .or(look.line.map(|l| l.0))
                        .unwrap_or(Rgb([128, 128, 128]));
                    let steps = (d / 1.5).ceil().clamp(1.0, 40.0) as usize;
                    for k in (1..=steps).rev() {
                        let f = k as f32 / steps as f32;
                        let at = t.then(Affine::translate(d * f * 0.6, -d * f * 0.6));
                        mark(out, at, rgba(darker(base, 0.55 + 0.15 * (1.0 - f)), 255));
                    }
                }
                for sp in outline.iter().filter(|s| s.fill != PathFill::None) {
                    match &fill {
                        Fill::Picture(b) => {
                            if let Some(part) = b.part.as_deref()
                                && let Some(pic) = picture(
                                    part,
                                    [0.0, 0.0, w, h],
                                    b.crop,
                                    b.alpha,
                                    Some(sp.path.clone()),
                                )
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
                // A bevel: light from the top left, shade to the bottom
                // right, strongest near the edges' width.
                if let Some(bw) = look.effects.bevel
                    && has_fill
                {
                    let edge = (px(bw) / w.min(h).max(1.0)).clamp(0.04, 0.45);
                    let stops = vec![
                        Stop {
                            offset: 0.0,
                            color: Rgba([255, 255, 255, 110]),
                        },
                        Stop {
                            offset: edge,
                            color: Rgba([255, 255, 255, 0]),
                        },
                        Stop {
                            offset: 1.0 - edge,
                            color: Rgba([0, 0, 0, 0]),
                        },
                        Stop {
                            offset: 1.0,
                            color: Rgba([0, 0, 0, 90]),
                        },
                    ];
                    for path in &filled {
                        push(
                            out,
                            Prim::Fill {
                                path: path.clone(),
                                paint: Paint::Linear {
                                    from: p(0.0, 0.0),
                                    to: p(w, h),
                                    stops: stops.clone(),
                                },
                                rule: FillRule::EvenOdd,
                            },
                        );
                    }
                }
                if let Some((c, wd)) = look.line {
                    let width = (wd as f32 / EMU_PER_PX).max(0.0);
                    if width > 0.0 {
                        let style = &look.line_style;
                        let stroked = outline
                            .iter()
                            .filter(|s| s.stroke)
                            .map(|s| s.path.clone())
                            .collect();
                        strokes(stroked, rgba(c, style.alpha), width, style, &mut |prim| {
                            push(out, prim)
                        });
                    }
                }
                if let Some(rt) = text(shape, xfrm) {
                    let local = Xfrm::new(0, 0, xfrm.cx, xfrm.cy);
                    self.push_text(engine, &rt, &local, &text_transform(xfrm), None, out);
                }
            }
            ShapeKind::Other { frame, .. } => match frame.get() {
                Some(FrameContent::Group(g)) => {
                    for child in &g.children {
                        let Some(own) = child.xfrm else { continue };
                        let at = compose(xfrm, &g.child_box, &own);
                        self.free_shape_with(engine, child, &at, text, pictures, charts, out);
                    }
                }
                Some(FrameContent::Chart(c)) => {
                    let drawn = c.part.as_deref().and_then(|p| charts(engine, p, w, h));
                    match drawn {
                        Some(items) => out.extend(items.into_iter().map(|i| Item {
                            transform: i.transform.then(t),
                            ..i
                        })),
                        None => push(out, Prim::Unsupported { w, h }),
                    }
                }
                _ => {}
            },
            ShapeKind::Picture { part, .. } => {
                let look = &shape.look;
                if let Some(pic) = part.as_deref().and_then(|part| {
                    picture(
                        part,
                        [0.0, 0.0, w, h],
                        look.picture.crop,
                        look.picture.alpha,
                        None,
                    )
                }) {
                    push(out, Prim::Picture(pic));
                }
                if let Some((c, wd)) = look.line {
                    let width = (wd as f32 / EMU_PER_PX).max(0.0);
                    if width > 0.0 {
                        let rect = vec![Path::rect(0.0, 0.0, w, h)];
                        strokes(
                            rect,
                            rgba(c, look.line_style.alpha),
                            width,
                            &look.line_style,
                            &mut |prim| push(out, prim),
                        );
                    }
                }
            }
        }
    }
}
