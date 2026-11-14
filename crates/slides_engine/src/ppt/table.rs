//! PowerPoint 2000+ tables: a group shape tagged `tableProperties` whose
//! cells are rectangles and whose borders are lines. Rebuilt as an
//! `a:tbl` from the cell positions; `None` (drawn as shapes) when the
//! cells don't form a grid.

use super::draw::{Part, Shape};

/// Sorted distinct coordinates, merging values within `tol`.
fn bounds(mut v: Vec<i32>, tol: i32) -> Vec<i32> {
    v.sort_unstable();
    let mut out: Vec<i32> = Vec::new();
    for x in v {
        match out.last() {
            Some(&l) if x - l <= tol => {}
            _ => out.push(x),
        }
    }
    out
}

fn index(b: &[i32], v: i32, tol: i32) -> Option<usize> {
    b.iter().position(|&x| (x - v).abs() <= tol)
}

type Border = Option<(String, u32)>;

pub(super) fn table<'a>(part: &mut Part<'_, 'a>, g: &Shape<'a>, top: bool) -> Option<String> {
    let is_line = |s: &Shape| s.kind == 20 || (32..=40).contains(&s.kind);
    let cells: Vec<&Shape> = g
        .kids
        .iter()
        .filter(|s| !s.is_group && !is_line(s))
        .collect();
    if cells.is_empty() || cells.iter().any(|c| c.child.is_none()) {
        return None;
    }
    let tol = 2;
    let xs = bounds(
        cells
            .iter()
            .flat_map(|c| {
                let [l, _, r, _] = c.child.unwrap_or_default();
                [l, r]
            })
            .collect(),
        tol,
    );
    let ys = bounds(
        cells
            .iter()
            .flat_map(|c| {
                let [_, t, _, b] = c.child.unwrap_or_default();
                [t, b]
            })
            .collect(),
        tol,
    );
    let (cols, rows) = (xs.len().checked_sub(1)?, ys.len().checked_sub(1)?);
    if cols == 0 || rows == 0 || cols * rows > 10_000 {
        return None;
    }
    // Grid slot → (cell, is its top-left).
    let mut grid: Vec<Option<(usize, bool)>> = vec![None; rows * cols];
    let mut spans = Vec::new();
    for (i, c) in cells.iter().enumerate() {
        let [l, t, r, b] = c.child?;
        let (c0, c1) = (index(&xs, l, tol)?, index(&xs, r, tol)?);
        let (r0, r1) = (index(&ys, t, tol)?, index(&ys, b, tol)?);
        if c1 <= c0 || r1 <= r0 {
            return None;
        }
        for row in r0..r1 {
            for col in c0..c1 {
                let slot = &mut grid[row * cols + col];
                if slot.is_some() {
                    return None;
                }
                *slot = Some((i, row == r0 && col == c0));
            }
        }
        spans.push((r0, c0, r1 - r0, c1 - c0));
    }
    // Borders per slot: left, right, top, bottom.
    let mut borders: Vec<[Border; 4]> = vec![Default::default(); rows * cols];
    for l in g.kids.iter().filter(|s| is_line(s)) {
        let Some([x0, y0, x1, y1]) = l.child else {
            continue;
        };
        let props = part.props_of(l);
        let Some(pen) = part.line_color_width(&props) else {
            continue;
        };
        if (y1 - y0).abs() <= tol {
            let Some(rb) = index(&ys, y0, tol) else {
                continue;
            };
            for col in 0..cols {
                let (a, b) = (xs[col], xs[col + 1]);
                if a + tol < x0.min(x1) || b - tol > x0.max(x1) {
                    continue;
                }
                if rb < rows {
                    borders[rb * cols + col][2] = Some(pen.clone());
                }
                if rb > 0 {
                    borders[(rb - 1) * cols + col][3] = Some(pen.clone());
                }
            }
        } else if (x1 - x0).abs() <= tol {
            let Some(cb) = index(&xs, x0, tol) else {
                continue;
            };
            for row in 0..rows {
                let (a, b) = (ys[row], ys[row + 1]);
                if a + tol < y0.min(y1) || b - tol > y0.max(y1) {
                    continue;
                }
                if cb < cols {
                    borders[row * cols + cb][0] = Some(pen.clone());
                }
                if cb > 0 {
                    borders[row * cols + cb - 1][1] = Some(pen.clone());
                }
            }
        }
    }

    let tmp = Shape {
        client: g.client,
        child: g.child,
        ..Shape::default()
    };
    let [x, y, w, h] = frame_rect(&tmp, top)?;
    let [gl, gt, gr, gb] = g.group.unwrap_or([xs[0], ys[0], xs[cols], ys[rows]]);
    let sx = w as f64 / (gr - gl).max(1) as f64;
    let sy = h as f64 / (gb - gt).max(1) as f64;
    let col_w: Vec<i64> = (0..cols)
        .map(|c| ((xs[c + 1] - xs[c]) as f64 * sx).round() as i64)
        .collect();
    let row_h: Vec<i64> = (0..rows)
        .map(|r| ((ys[r + 1] - ys[r]) as f64 * sy).round() as i64)
        .collect();

    let id = part.next_shape_id();
    let mut s = format!(
        "<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id=\"{id}\" name=\"Table {}\"/><p:cNvGraphicFramePr><a:graphicFrameLocks noGrp=\"1\"/></p:cNvGraphicFramePr><p:nvPr/></p:nvGraphicFramePr>\
         <p:xfrm><a:off x=\"{x}\" y=\"{y}\"/><a:ext cx=\"{w}\" cy=\"{h}\"/></p:xfrm>\
         <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/table\"><a:tbl><a:tblPr/><a:tblGrid>",
        id - 1
    );
    for w in &col_w {
        s.push_str(&format!("<a:gridCol w=\"{w}\"/>"));
    }
    s.push_str("</a:tblGrid>");
    let ln = |name: &str, b: &Border| match b {
        Some((c, w)) => format!(
            "<a:{name} w=\"{w}\"><a:solidFill><a:srgbClr val=\"{c}\"/></a:solidFill></a:{name}>"
        ),
        None => format!("<a:{name} w=\"0\"><a:noFill/></a:{name}>"),
    };
    for (r, rh) in row_h.iter().enumerate() {
        s.push_str(&format!("<a:tr h=\"{rh}\">"));
        for c in 0..cols {
            let slot = grid[r * cols + c];
            let mut attrs = String::new();
            let mut body = String::new();
            let mut pr = String::new();
            match slot {
                Some((i, true)) => {
                    let (_, _, rs, cs) = spans[i];
                    if cs > 1 {
                        attrs.push_str(&format!(" gridSpan=\"{cs}\""));
                    }
                    if rs > 1 {
                        attrs.push_str(&format!(" rowSpan=\"{rs}\""));
                    }
                    let cell = cells[i];
                    let cp = part.props_of(cell);
                    body = part.cell_text(cell).unwrap_or_default();
                    let ins = |id: u16, def: i64| {
                        cp.get(id)
                            .map(|v| v as i32 as i64)
                            .unwrap_or(def)
                            .clamp(0, 5_000_000)
                    };
                    let anchor = match cp.get(0x87).unwrap_or(0) % 10 {
                        1 | 4 | 7 => "ctr",
                        2 | 5 | 8 => "b",
                        _ => "t",
                    };
                    pr.push_str(&format!(
                        " marL=\"{}\" marR=\"{}\" marT=\"{}\" marB=\"{}\" anchor=\"{anchor}\"",
                        ins(0x81, 91440),
                        ins(0x83, 91440),
                        ins(0x82, 45720),
                        ins(0x84, 45720)
                    ));
                    let b = &borders[r * cols + c];
                    let fill = part.solid_fill(&cp);
                    let fill = if fill.is_empty() {
                        "<a:noFill/>".to_owned()
                    } else {
                        fill
                    };
                    let right = &borders[r * cols + (c + cs - 1)][1];
                    let bottom = &borders[(r + rs - 1) * cols + c][3];
                    pr = format!(
                        "<a:tcPr{pr}>{}{}{}{}{fill}</a:tcPr>",
                        ln("lnL", &b[0]),
                        ln("lnR", right),
                        ln("lnT", &b[2]),
                        ln("lnB", bottom)
                    );
                }
                Some((i, false)) => {
                    let (r0, c0, _, _) = spans[i];
                    if c > c0 {
                        attrs.push_str(" hMerge=\"1\"");
                    }
                    if r > r0 {
                        attrs.push_str(" vMerge=\"1\"");
                    }
                    pr.push_str("<a:tcPr/>");
                }
                None => {
                    let b = &borders[r * cols + c];
                    pr = format!(
                        "<a:tcPr>{}{}{}{}<a:noFill/></a:tcPr>",
                        ln("lnL", &b[0]),
                        ln("lnR", &b[1]),
                        ln("lnT", &b[2]),
                        ln("lnB", &b[3])
                    );
                }
            }
            if body.is_empty() {
                body = "<a:p><a:endParaRPr lang=\"en-US\"/></a:p>".into();
            }
            s.push_str(&format!(
                "<a:tc{attrs}><a:txBody><a:bodyPr/><a:lstStyle/>{body}</a:txBody>{pr}</a:tc>"
            ));
        }
        s.push_str("</a:tr>");
    }
    s.push_str("</a:tbl></a:graphicData></a:graphic></p:graphicFrame>");
    Some(s)
}

fn frame_rect(s: &Shape, top: bool) -> Option<[i64; 4]> {
    let r = if top {
        s.client.or(s.child)
    } else {
        s.child.or(s.client)
    }?;
    let e = |v: i32| (v as f64 * super::MU_EMU).round() as i64;
    Some([
        e(r[0]),
        e(r[1]),
        e(r[2] - r[0]).max(0),
        e(r[3] - r[1]).max(0),
    ])
}
