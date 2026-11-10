//! Tables (`a:tbl`): cells laid out on the grid, rows grown to fit their
//! text, merged cells, fills, borders and the table style.

use docs_layout::Engine;
use slides_engine::frame::{
    Border, CellLook, EDGE_B, EDGE_BL_TR, EDGE_L, EDGE_R, EDGE_T, EDGE_TL_BR, Table, Vert,
};
use slides_engine::{EMU_PER_PX, Emu, Fill, Presentation, RenderText, Xfrm};

use crate::build::{Builder, Draw, px, shape_transform, text_transform};
use crate::geom::{Affine, Path, PathFill, p};
use crate::paint::{dash_array, fill_paint, rgba};
use crate::{FillRule, Item, Prim, Stroke};

/// A table laid out: column and row edges (px from the frame's top left)
/// and the cells that show.
#[derive(Clone, Debug, PartialEq)]
pub struct TableLayout {
    /// `cols + 1` x positions.
    pub xs: Vec<f32>,
    /// `rows + 1` y positions.
    pub ys: Vec<f32>,
    /// Shown cells: row, column, last row, last column (inclusive).
    pub cells: Vec<[usize; 4]>,
}

impl TableLayout {
    /// The box of a shown cell, `[x, y, w, h]` px.
    pub fn rect(&self, [r0, c0, r1, c1]: [usize; 4]) -> [f32; 4] {
        let (x, y) = (self.xs[c0], self.ys[r0]);
        [x, y, self.xs[c1 + 1] - x, self.ys[r1 + 1] - y]
    }
}

/// Lay a table out: `needed(r, c, width_emu)` is the height (EMU) the cell
/// at `(r, c)` needs at that width (text plus margins).
pub fn layout_table(t: &Table, needed: &mut dyn FnMut(usize, usize, Emu) -> Emu) -> TableLayout {
    let (nr, nc) = (t.rows.len(), t.cols.len());
    let mut xs = vec![0i64; nc + 1];
    for c in 0..nc {
        xs[c + 1] = xs[c] + t.cols[c];
    }
    let mut heights: Vec<Emu> = t.rows.iter().map(|r| r.h).collect();
    let mut cells = Vec::new();
    for r in 0..nr {
        let mut c = 0;
        while c < nc {
            let Some(cell) = t.cell(r, c) else { break };
            let cs = (cell.grid_span.max(1) as usize).min(nc - c);
            if !cell.covered() {
                let rs = (cell.row_span.max(1) as usize).min(nr - r);
                cells.push([r, c, r + rs - 1, c + cs - 1]);
            }
            c += if cell.covered() { 1 } else { cs };
        }
    }
    // Single-row cells first, then spans grow their last row if needed.
    let mut order = cells.clone();
    order.sort_by_key(|[r0, _, r1, _]| r1 - r0);
    for &[r0, c0, r1, c1] in &order {
        let w = xs[c1 + 1] - xs[c0];
        let need = needed(r0, c0, w);
        let have: Emu = heights[r0..=r1].iter().sum();
        if need > have {
            heights[r1] += need - have;
        }
    }
    let mut ys = vec![0.0f32; nr + 1];
    for r in 0..nr {
        ys[r + 1] = ys[r] + px(heights[r]);
    }
    TableLayout {
        xs: xs.into_iter().map(px).collect(),
        ys,
        cells,
    }
}

fn border_stroke(b: &Border) -> Option<Stroke> {
    let (c, a) = b.color?;
    let width = px(b.width);
    (width > 0.0).then(|| Stroke {
        width,
        color: rgba(c, a),
        dash: dash_array(&b.style, width),
        cap: b.style.cap,
        join: b.style.join,
    })
}

/// A shown cell's look and text.
type Shown = (CellLook, Option<RenderText>);

impl Builder {
    /// A table drawn at `xfrm` (grows downwards when rows grow).
    pub(crate) fn table(
        &self,
        engine: &mut Engine,
        pres: &Presentation,
        d: Draw,
        t: &Table,
        xfrm: &Xfrm,
        out: &mut Vec<Item>,
    ) -> bool {
        if t.cols.is_empty() || t.rows.is_empty() || t.cols.iter().sum::<Emu>() <= 0 {
            return false;
        }
        let style = t
            .style_id
            .as_deref()
            .and_then(|id| pres.table_styles(d.slide).get(id));
        let mut looks: Vec<Vec<Option<Shown>>> = vec![vec![None; t.cols.len()]; t.rows.len()];
        let lay = layout_table(t, &mut |r, c, w| {
            let look = t.cell_look(style, r, c);
            let text = t
                .cell(r, c)
                .and_then(|cell| pres.cell_text(d.slide, cell, &look.text));
            let need = match &text {
                Some(tx) if tx.vert == Vert::Horz => {
                    let l = self.texts.layout(engine, tx, &Xfrm::new(0, 0, w, 0));
                    (l.height * EMU_PER_PX) as Emu + tx.insets[1] + tx.insets[3]
                }
                _ => 0,
            };
            looks[r][c] = Some((look, text));
            need
        });
        let base = shape_transform(xfrm);
        let tt = text_transform(xfrm);
        let push = |out: &mut Vec<Item>, transform: Affine, prim: Prim| {
            out.push(Item {
                layer: d.layer,
                transform,
                prim,
            })
        };
        let (tw, th) = (
            *lay.xs.last().unwrap_or(&0.0),
            *lay.ys.last().unwrap_or(&0.0),
        );
        if let Some(bg) = &t.background
            && let Some(paint) = fill_paint(bg, PathFill::Norm, tw, th, &base)
        {
            let prim = Prim::Fill {
                path: Path::rect(0.0, 0.0, tw, th),
                paint,
                rule: FillRule::NonZero,
            };
            push(out, base, prim);
        }
        // Fills, then borders, then text.
        for &cell in &lay.cells {
            let Some((look, _)) = &looks[cell[0]][cell[1]] else {
                continue;
            };
            let [x, y, w, h] = lay.rect(cell);
            let at = Affine::translate(x, y).then(base);
            match &look.fill {
                Some(Fill::Picture(b)) => {
                    if let Some(pic) = self.blip(pres, b, [0.0, 0.0, w, h], None) {
                        push(out, at, Prim::Picture(pic));
                    }
                }
                Some(f) => {
                    if let Some(paint) = fill_paint(f, PathFill::Norm, w, h, &at) {
                        let prim = Prim::Fill {
                            path: Path::rect(0.0, 0.0, w, h),
                            paint,
                            rule: FillRule::NonZero,
                        };
                        push(out, at, prim);
                    }
                }
                None => {}
            }
        }
        for &cell in &lay.cells {
            let Some((look, _)) = &looks[cell[0]][cell[1]] else {
                continue;
            };
            let [x, y, w, h] = lay.rect(cell);
            let at = Affine::translate(x, y).then(base);
            let lines = [
                (EDGE_T, (0.0, 0.0), (w, 0.0)),
                (EDGE_B, (0.0, h), (w, h)),
                (EDGE_L, (0.0, 0.0), (0.0, h)),
                (EDGE_R, (w, 0.0), (w, h)),
                (EDGE_TL_BR, (0.0, 0.0), (w, h)),
                (EDGE_BL_TR, (0.0, h), (w, 0.0)),
            ];
            for (edge, a, b) in lines {
                let Some(stroke) = look.borders[edge].as_ref().and_then(border_stroke) else {
                    continue;
                };
                let mut path = Path::default();
                path.move_to(p(a.0, a.1));
                path.line_to(p(b.0, b.1));
                push(out, at, Prim::Stroke { path, stroke });
            }
        }
        for &cell in &lay.cells {
            let Some((_, Some(text))) = &looks[cell[0]][cell[1]] else {
                continue;
            };
            let [x, y, w, h] = lay.rect(cell);
            let e = |v: f32| (v * EMU_PER_PX).round() as Emu;
            let bx = Xfrm::new(e(x), e(y), e(w), e(h));
            self.push_text(engine, text, &bx, &tt, d.layer, out);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use slides_engine::frame::{Cell, Row};

    use super::*;

    fn cell() -> Cell {
        Cell {
            grid_span: 1,
            row_span: 1,
            ..Cell::default()
        }
    }

    #[test]
    fn merged_cells_span_and_rows_grow_to_fit() {
        // 3 × 3: (0,0) spans two columns, (1,2) spans two rows.
        let mut rows: Vec<Row> = (0..3)
            .map(|_| Row {
                h: 952_500,
                cells: vec![cell(), cell(), cell()],
            })
            .collect();
        rows[0].cells[0].grid_span = 2;
        rows[0].cells[1].h_merge = true;
        rows[1].cells[2].row_span = 2;
        rows[2].cells[2].v_merge = true;
        let t = Table {
            cols: vec![952_500; 3],
            rows,
            ..Table::default()
        };
        // The middle-left cell needs three rows' worth; the span needs
        // more than its two rows give it.
        let lay = layout_table(&t, &mut |r, c, w| match (r, c) {
            (1, 0) => {
                assert_eq!(w, 952_500);
                3 * 952_500
            }
            (1, 2) => 5 * 952_500,
            _ => 0,
        });
        assert_eq!(lay.xs, vec![0.0, 100.0, 200.0, 300.0]);
        assert_eq!(lay.ys, vec![0.0, 100.0, 400.0, 600.0]);
        assert_eq!(
            lay.cells,
            vec![
                [0, 0, 0, 1],
                [0, 2, 0, 2],
                [1, 0, 1, 0],
                [1, 1, 1, 1],
                [1, 2, 2, 2],
                [2, 0, 2, 0],
                [2, 1, 2, 1],
            ]
        );
        assert_eq!(lay.rect([0, 0, 0, 1]), [0.0, 0.0, 200.0, 100.0]);
    }
}
