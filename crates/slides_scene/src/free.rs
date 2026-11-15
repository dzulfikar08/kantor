//! Shapes drawn without a presentation behind them: a worksheet's shapes,
//! text boxes, connectors and groups (Kantor Sheets). The same outlines,
//! fills, lines, arrowheads and text as slide shapes; picture fills and
//! pictures inside groups (which need the package's media) aren't drawn.

use docs_layout::Engine;
use slides_engine::frame::{FrameContent, compose};
use slides_engine::{EMU_PER_PX, Fill, RenderText, Shape, ShapeKind, Xfrm};

use crate::build::{Builder, px, shape_transform, strokes, text_transform};
use crate::geom::PathFill;
use crate::paint::{fill_paint, rgba};
use crate::{FillRule, Item, Prim, shape_outline};

impl Builder {
    /// `shape` drawn at `xfrm` (EMU, in the coordinates the items are in),
    /// its text (and that of a group's shapes) from `text`.
    pub fn free_shape(
        &self,
        engine: &mut Engine,
        shape: &Shape,
        xfrm: &Xfrm,
        text: &dyn Fn(&Shape, &Xfrm) -> Option<RenderText>,
        out: &mut Vec<Item>,
    ) {
        let (w, h) = (px(xfrm.cx), px(xfrm.cy));
        let t = shape_transform(xfrm);
        let push = |out: &mut Vec<Item>, prim: Prim| {
            out.push(Item {
                layer: None,
                transform: t,
                prim,
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
                for sp in outline.iter().filter(|s| s.fill != PathFill::None) {
                    if let Some(paint) = fill_paint(&fill, sp.fill, w, h, &t) {
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
            ShapeKind::Other { frame, .. } => {
                if let Some(FrameContent::Group(g)) = frame.get() {
                    for child in &g.children {
                        let Some(own) = child.xfrm else { continue };
                        let at = compose(xfrm, &g.child_box, &own);
                        self.free_shape(engine, child, &at, text, out);
                    }
                }
            }
            ShapeKind::Picture { .. } => {}
        }
    }
}
