//! Fills and strokes in shape coordinates: gradient geometry, dashes and
//! arrowheads.

use slides_engine::{Arrow, ArrowKind, Fill, GradPath, Gradient, LineStyle, Rgb};

use crate::geom::{Affine, P, Path, PathFill, p};
use crate::{Paint, Rgba, Stop};

/// A colour lightened or darkened as a preset path asks.
pub(crate) fn shade(c: Rgba, how: PathFill) -> Rgba {
    let [r, g, b, a] = c.0;
    let f = |x: u8| -> u8 {
        let x = x as f32;
        (match how {
            PathFill::Lighten => x + (255.0 - x) * 0.4,
            PathFill::LightenLess => x + (255.0 - x) * 0.2,
            PathFill::Darken => x * 0.6,
            PathFill::DarkenLess => x * 0.8,
            PathFill::Norm | PathFill::None => x,
        })
        .round()
        .clamp(0.0, 255.0) as u8
    };
    Rgba([f(r), f(g), f(b), a])
}

pub(crate) fn rgba(c: Rgb, a: u8) -> Rgba {
    let [r, g, b] = c.0;
    Rgba([r, g, b, a])
}

fn stops(g: &Gradient, how: PathFill) -> Vec<Stop> {
    g.stops
        .iter()
        .map(|s| Stop {
            offset: s.pos as f32 / 100_000.0,
            color: shade(rgba(s.color, s.alpha), how),
        })
        .collect()
}

/// The two ends of a linear gradient in a `w` × `h` box: stop 0 lies on
/// the line through `from`, stop 1 on the line through `to` (both
/// perpendicular to `to - from`).
///
/// `ang` runs clockwise from left → right. A scaled gradient is computed
/// in the unit square and stretched with the box; otherwise the direction
/// is kept and the gradient spans the box corner to corner. `turn` is the
/// shape's transform when the gradient must not turn with the shape.
pub fn linear_ends(ang_deg: f32, scaled: bool, w: f32, h: f32, turn: Option<&Affine>) -> (P, P) {
    let (s, c) = ang_deg.to_radians().sin_cos();
    let mut d = p(c, s);
    if let Some(t) = turn
        && let Some(inv) = t.invert()
    {
        d = inv.apply_vec(d).unit();
    }
    let (w, h) = (w.max(1e-3), h.max(1e-3));
    // t(q) = dot(q − centre, g) + ½ is 0 and 1 at opposite corners.
    let g = if scaled && turn.is_none() {
        p(d.x / w, d.y / h).scale(1.0 / (d.x.abs() + d.y.abs()).max(1e-6))
    } else {
        d.scale(1.0 / (w * d.x.abs() + h * d.y.abs()).max(1e-6))
    };
    let center = p(w / 2.0, h / 2.0);
    let k = 0.5 / g.dot(g).max(1e-12);
    (center.minus(g.scale(k)), center.plus(g.scale(k)))
}

/// Centre and radii of a path gradient in a `w` × `h` box: position 0 at
/// the focus rectangle's centre, 1 at the box's corners (circle) or edges
/// (rect and shape gradients, approximated by an ellipse).
pub fn radial_shape(g: &Gradient, w: f32, h: f32) -> (P, f32, f32) {
    let [l, t, r, b] = g.focus.map(|v| v as f32 / 100_000.0);
    let x0 = l * w;
    let y0 = t * h;
    let x1 = w - r * w;
    let y1 = h - b * h;
    let c = p((x0 + x1) / 2.0, (y0 + y1) / 2.0);
    let far_x = c.x.max(w - c.x).max(1e-3);
    let far_y = c.y.max(h - c.y).max(1e-3);
    match g.path {
        GradPath::Circle => {
            let r = far_x.hypot(far_y);
            (c, r, r)
        }
        _ => (c, far_x, far_y),
    }
}

/// A fill as paint in a `w` × `h` box. `turn` is the shape's transform
/// (for gradients that don't turn with it). `None` for no fill and for
/// pictures (drawn separately).
pub fn fill_paint(fill: &Fill, how: PathFill, w: f32, h: f32, turn: &Affine) -> Option<Paint> {
    match fill {
        Fill::None | Fill::Picture(_) => None,
        Fill::Solid(c, a) => Some(Paint::Solid(shade(rgba(*c, *a), how))),
        Fill::Pattern(pf) => Some(Paint::Pattern {
            fg: shade(rgba(pf.fg.0, pf.fg.1), how),
            bg: shade(rgba(pf.bg.0, pf.bg.1), how),
            bits: pf.bits(),
            origin: p(0.0, 0.0),
            cell: 1.0,
        }),
        Fill::Gradient(g) => {
            let stops = stops(g, how);
            if stops.len() == 1 {
                return Some(Paint::Solid(stops[0].color));
            }
            Some(match g.path {
                GradPath::Linear { angle, scaled } => {
                    let fixed = (!g.rot_with_shape).then_some(turn);
                    let (from, to) = linear_ends(angle as f32 / 60_000.0, scaled, w, h, fixed);
                    Paint::Linear { from, to, stops }
                }
                _ => {
                    let (center, rx, ry) = radial_shape(g, w, h);
                    Paint::Radial {
                        center,
                        rx,
                        ry,
                        stops,
                    }
                }
            })
        }
    }
}

/// A pattern's two colours mixed by its share of foreground bits.
pub fn mix(fg: Rgba, bg: Rgba, bits: &[u8; 8]) -> Rgba {
    let k = kantor_ooxml::pattern::coverage(bits);
    let m = |i: usize| (bg.0[i] as f32 + (fg.0[i] as f32 - bg.0[i] as f32) * k).round() as u8;
    Rgba([m(0), m(1), m(2), m(3)])
}

/// Most rectangles a pattern is drawn with on screen (beyond that, its
/// mixed colour).
pub const PATTERN_CELLS: usize = 60_000;

/// The foreground cells of a pattern inside `path` (item coordinates), as
/// rectangles `[x, y, w, h]`, for renderers without tiling patterns (the
/// screen). Each row of cells is sampled at its middle against the
/// outline, so edges step by one cell; runs of cells on a row are one
/// rectangle and identical runs on the next rows extend it. `None` when
/// there would be more than `limit` rectangles.
pub fn pattern_cells(
    path: &Path,
    even_odd: bool,
    bits: &[u8; 8],
    origin: P,
    cell: f32,
    limit: usize,
) -> Option<Vec<[f32; 4]>> {
    let cell = cell.max(0.05);
    let polys: Vec<Vec<P>> = path
        .flatten(0.25)
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| p.len() > 2)
        .collect();
    let all = polys.iter().flatten();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for q in all {
        x0 = x0.min(q.x);
        y0 = y0.min(q.y);
        x1 = x1.max(q.x);
        y1 = y1.max(q.y);
    }
    if x0 >= x1 || y0 >= y1 {
        return Some(Vec::new());
    }
    let row0 = ((y0 - origin.y) / cell).floor() as i64;
    let row1 = ((y1 - origin.y) / cell).ceil() as i64;
    let mut out: Vec<[f32; 4]> = Vec::new();
    // Rectangles of the previous row (index into `out`) by their x range.
    let mut open: Vec<(i64, i64, usize)> = Vec::new();
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for row in row0..row1 {
        let top = origin.y + row as f32 * cell;
        let yc = top + cell / 2.0;
        xs.clear();
        for poly in &polys {
            for i in 0..poly.len() {
                let (a, b) = (poly[i], poly[(i + 1) % poly.len()]);
                if (a.y <= yc) != (b.y <= yc) {
                    let x = a.x + (yc - a.y) / (b.y - a.y) * (b.x - a.x);
                    xs.push((x, if b.y > a.y { 1 } else { -1 }));
                }
            }
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let bits_row = bits[row.rem_euclid(8) as usize];
        let mut next: Vec<(i64, i64, usize)> = Vec::new();
        let mut wind = 0;
        for w in 0..xs.len() {
            let before = wind;
            wind += xs[w].1;
            let inside = |n: i32| if even_odd { n & 1 != 0 } else { n != 0 };
            if inside(before) || !inside(wind) || w + 1 >= xs.len() {
                continue;
            }
            // Entering: the span runs to where we leave.
            let (sa, mut sb, mut n) = (xs[w].0, xs[w].0, wind);
            for e in &xs[w + 1..] {
                n += e.1;
                sb = e.0;
                if !inside(n) {
                    break;
                }
            }
            // Columns whose centre lies in the span.
            let c0 = ((sa - origin.x) / cell - 0.5).ceil() as i64;
            let c1 = ((sb - origin.x) / cell - 0.5).floor() as i64;
            let mut c = c0;
            while c <= c1 {
                if bits_row >> c.rem_euclid(8) & 1 == 0 {
                    c += 1;
                    continue;
                }
                let start = c;
                while c <= c1 && bits_row >> c.rem_euclid(8) & 1 == 1 {
                    c += 1;
                }
                let end = c; // exclusive
                if let Some(&(_, _, i)) = open.iter().find(|o| o.0 == start && o.1 == end) {
                    out[i][3] += cell;
                    next.push((start, end, i));
                } else {
                    if out.len() >= limit {
                        return None;
                    }
                    let x = origin.x + start as f32 * cell;
                    out.push([x, top, (end - start) as f32 * cell, cell]);
                    next.push((start, end, out.len() - 1));
                }
            }
        }
        open = next;
    }
    Some(out)
}

/// Dash lengths for a line of `width` (absolute), `None` for solid.
pub fn dash_array(style: &LineStyle, width: f32) -> Option<Vec<f32>> {
    let w = width.max(1.0);
    style
        .dash
        .pattern()
        .map(|v| v.into_iter().map(|x| (x * w).max(0.01)).collect())
}

/// What an arrowhead is drawn with.
pub enum Head {
    /// A filled outline.
    Filled(Path),
    /// Two strokes (open arrow).
    Open(Path),
}

/// Length and width of an arrowhead on a line of `width`.
pub fn arrow_size(a: &Arrow, width: f32) -> (f32, f32) {
    let w = width.max(1.0);
    (a.length.factor() * w, a.width.factor() * w)
}

/// The arrowhead `a` with its tip at `at`, pointing along `dir` (a unit
/// vector out of the line).
pub fn arrowhead(a: &Arrow, at: P, dir: P, width: f32) -> Head {
    let (len, wid) = arrow_size(a, width);
    let n = p(-dir.y, dir.x);
    let back = at.minus(dir.scale(len));
    let side = n.scale(wid / 2.0);
    let mut pa = Path::default();
    match a.kind {
        ArrowKind::Triangle => {
            pa.move_to(at);
            pa.line_to(back.plus(side));
            pa.line_to(back.minus(side));
            pa.close();
            Head::Filled(pa)
        }
        ArrowKind::Stealth => {
            pa.move_to(at);
            pa.line_to(back.plus(side));
            pa.line_to(at.minus(dir.scale(len * 0.6)));
            pa.line_to(back.minus(side));
            pa.close();
            Head::Filled(pa)
        }
        ArrowKind::Diamond => {
            let half = dir.scale(len / 2.0);
            pa.move_to(at.plus(half));
            pa.line_to(at.plus(side));
            pa.line_to(at.minus(half));
            pa.line_to(at.minus(side));
            pa.close();
            Head::Filled(pa)
        }
        ArrowKind::Oval => {
            // An ellipse centred on the end, its axes along the line.
            let e = Path::ellipse(0.0, 0.0, len / 2.0, wid / 2.0);
            let t = Affine {
                a: dir.x,
                b: dir.y,
                c: n.x,
                d: n.y,
                e: at.x,
                f: at.y,
            };
            Head::Filled(e.transformed(&t))
        }
        ArrowKind::Arrow => {
            pa.move_to(back.plus(side));
            pa.line_to(at);
            pa.line_to(back.minus(side));
            Head::Open(pa)
        }
    }
}

/// How far a line is pulled back from its end under arrowhead `a`, so a
/// thick line doesn't show past a pointed tip.
pub fn arrow_inset(a: &Arrow, width: f32) -> f32 {
    let (len, wid) = arrow_size(a, width);
    match a.kind {
        ArrowKind::Triangle | ArrowKind::Stealth => len * (width / wid.max(1e-3)).min(1.0),
        ArrowKind::Arrow => width * 0.5,
        ArrowKind::Oval | ArrowKind::Diamond => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slides_engine::{ArrowSize, GradStop};

    fn near(a: P, b: P) -> bool {
        (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3
    }

    #[test]
    fn pattern_cells_follow_the_bitmap_and_the_outline() {
        let bits = kantor_ooxml::pattern::bits("ltVert");
        // Vertical lines every 4 cells (columns 3 and 7): one tall
        // rectangle each, rows merged.
        let r = pattern_cells(
            &Path::rect(0.0, 0.0, 16.0, 8.0),
            false,
            &bits,
            p(0.0, 0.0),
            1.0,
            100,
        )
        .unwrap();
        assert_eq!(
            r,
            vec![
                [3.0, 0.0, 1.0, 8.0],
                [7.0, 0.0, 1.0, 8.0],
                [11.0, 0.0, 1.0, 8.0],
                [15.0, 0.0, 1.0, 8.0]
            ]
        );
        // A circle keeps cells inside it only.
        let solid = [0xff; 8];
        let c = pattern_cells(
            &Path::ellipse(10.0, 10.0, 10.0, 10.0),
            false,
            &solid,
            p(0.0, 0.0),
            1.0,
            1000,
        )
        .unwrap();
        let area: f32 = c.iter().map(|r| r[2] * r[3]).sum();
        assert!((area - std::f32::consts::PI * 100.0).abs() < 12.0, "{area}");
        assert!(c.iter().all(|r| r[0] >= 0.0 && r[0] + r[2] <= 20.0));
        // Too many: None.
        assert!(
            pattern_cells(
                &Path::rect(0.0, 0.0, 100.0, 100.0),
                false,
                &kantor_ooxml::pattern::bits("pct50"),
                p(0.0, 0.0),
                1.0,
                10
            )
            .is_none()
        );
        // A pattern fill's paint keeps its colours and bitmap.
        let f = Fill::Pattern(std::sync::Arc::new(slides_engine::PatternFill {
            prst: "smGrid".into(),
            fg: (Rgb([255, 0, 0]), 255),
            bg: (Rgb([0, 0, 255]), 255),
        }));
        let Some(Paint::Pattern { fg, bg, bits, .. }) =
            fill_paint(&f, PathFill::Norm, 10.0, 10.0, &Affine::IDENTITY)
        else {
            panic!()
        };
        assert_eq!((fg, bg), (Rgba([255, 0, 0, 255]), Rgba([0, 0, 255, 255])));
        assert_eq!(bits, kantor_ooxml::pattern::bits("smGrid"));
    }

    /// Position (0..1) of `q` along a linear gradient.
    fn t_at(from: P, to: P, q: P) -> f32 {
        let d = to.minus(from);
        q.minus(from).dot(d) / d.dot(d)
    }

    #[test]
    fn linear_angles_map_onto_the_box() {
        // 0°: left → right across the box.
        let (f, t) = linear_ends(0.0, false, 200.0, 100.0, None);
        assert!(
            near(f, p(0.0, 50.0)) && near(t, p(200.0, 50.0)),
            "{f:?} {t:?}"
        );
        // 90°: top → bottom.
        let (f, t) = linear_ends(90.0, false, 200.0, 100.0, None);
        assert!(
            near(f, p(100.0, 0.0)) && near(t, p(100.0, 100.0)),
            "{f:?} {t:?}"
        );
        // 45° unscaled: the corners are 0 and 1, the direction stays 45°.
        let (f, t) = linear_ends(45.0, false, 200.0, 100.0, None);
        assert!((t_at(f, t, p(0.0, 0.0))).abs() < 1e-4);
        assert!((t_at(f, t, p(200.0, 100.0)) - 1.0).abs() < 1e-4);
        let d = t.minus(f).unit();
        assert!((d.x - d.y).abs() < 1e-4, "{d:?}");
        // 45° scaled: corner to corner too, but along the diagonal of the
        // stretched unit square, so the other corners are both ½.
        let (f, t) = linear_ends(45.0, true, 200.0, 100.0, None);
        assert!((t_at(f, t, p(200.0, 100.0)) - 1.0).abs() < 1e-4);
        assert!((t_at(f, t, p(200.0, 0.0)) - 0.5).abs() < 1e-4);
        assert!((t_at(f, t, p(0.0, 100.0)) - 0.5).abs() < 1e-4);
        // Not turning with a shape turned 90°: a left → right gradient on
        // the page runs bottom → top in the shape's own box... (y down:
        // local −y is page +x after a clockwise quarter turn).
        let turn = Affine::rotate(90f32.to_radians());
        let (f, t) = linear_ends(0.0, false, 100.0, 100.0, Some(&turn));
        let d = t.minus(f).unit();
        assert!(near(d, p(0.0, -1.0)), "{d:?}");
    }

    #[test]
    fn path_gradients_centre_on_their_focus() {
        let mut g = Gradient {
            stops: vec![
                GradStop {
                    pos: 0,
                    color: Rgb([255, 255, 255]),
                    alpha: 255,
                },
                GradStop {
                    pos: 100_000,
                    color: Rgb([0, 0, 0]),
                    alpha: 255,
                },
            ],
            path: GradPath::Circle,
            focus: [50_000; 4],
            rot_with_shape: true,
        };
        let (c, rx, ry) = radial_shape(&g, 200.0, 100.0);
        assert!(near(c, p(100.0, 50.0)));
        assert!((rx - 100f32.hypot(50.0)).abs() < 1e-3 && rx == ry);
        // From the top left corner.
        g.focus = [0, 0, 100_000, 100_000];
        g.path = GradPath::Rect;
        let (c, rx, ry) = radial_shape(&g, 200.0, 100.0);
        assert!(near(c, p(0.0, 0.0)) && rx == 200.0 && ry == 100.0);
        let Some(Paint::Radial { stops, .. }) = fill_paint(
            &Fill::Gradient(g.into()),
            PathFill::Darken,
            10.0,
            10.0,
            &Affine::IDENTITY,
        ) else {
            panic!()
        };
        assert_eq!(stops[0].color, Rgba([153, 153, 153, 255]), "darkened");
    }

    #[test]
    fn dashes_scale_with_the_line() {
        let style = LineStyle {
            dash: slides_engine::Dash::DashDot,
            ..LineStyle::default()
        };
        assert_eq!(dash_array(&style, 2.0).unwrap(), vec![8.0, 6.0, 2.0, 6.0]);
        // Hairlines dash as if 1 px wide.
        assert_eq!(dash_array(&style, 0.25).unwrap()[0], 4.0);
        assert!(dash_array(&LineStyle::default(), 2.0).is_none());
    }

    #[test]
    fn arrowheads_sit_on_the_line_end() {
        let a = Arrow {
            kind: ArrowKind::Triangle,
            width: ArrowSize::Medium,
            length: ArrowSize::Large,
        };
        // Line ending at (100, 0) going right; width 2 → 10 long, 6 wide.
        let Head::Filled(tri) = arrowhead(&a, p(100.0, 0.0), p(1.0, 0.0), 2.0) else {
            panic!()
        };
        let b = tri.bounds().unwrap();
        assert!((b[0] - 90.0).abs() < 1e-4 && (b[2] - 100.0).abs() < 1e-4);
        assert!((b[1] + 3.0).abs() < 1e-4 && (b[3] - 3.0).abs() < 1e-4);
        assert!(arrow_inset(&a, 2.0) > 0.0);
        let open = Arrow {
            kind: ArrowKind::Arrow,
            ..a
        };
        assert!(matches!(
            arrowhead(&open, p(0.0, 0.0), p(0.0, -1.0), 1.0),
            Head::Open(_)
        ));
        let oval = Arrow {
            kind: ArrowKind::Oval,
            width: ArrowSize::Small,
            length: ArrowSize::Small,
        };
        let Head::Filled(o) = arrowhead(&oval, p(5.0, 5.0), p(0.0, 1.0), 1.0) else {
            panic!()
        };
        let b = o.bounds().unwrap();
        assert!(
            (b[0] - 4.0).abs() < 0.01 && (b[2] - 6.0).abs() < 0.01,
            "{b:?}"
        );
    }
}
