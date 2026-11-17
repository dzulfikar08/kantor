//! 3-D charts drawn in 3-D, as Office draws them: columns and bars as
//! boxes, lines as ribbons, areas as slabs, a tilted pie with its edge,
//! surfaces as a mesh; on a floor with a back and a side wall, value
//! gridlines on the walls, seen as `c:view3D` says (turned by `rotX` and
//! `rotY`, with perspective unless the axes are at right angles). A child
//! of [`super`] (the chart layout), sharing its helpers.
//!
//! Faces are drawn back to front (the painter's algorithm, by each
//! solid's depth, then its faces' depth), shaded by which way they face.

use super::*;

/// A point in the chart's own space: x across (categories, 0..1), y up
/// (values, 0..h), z into the page (series, 0..d).
#[derive(Clone, Copy, Debug, PartialEq)]
struct P3 {
    x: f32,
    y: f32,
    z: f32,
}

fn p3(x: f32, y: f32, z: f32) -> P3 {
    P3 { x, y, z }
}

/// The view: the box's size and how it's turned and projected.
struct Cam {
    w: f32,
    h: f32,
    d: f32,
    sx: f32,
    cx: f32,
    sy: f32,
    cy: f32,
    /// Distance of the eye (perspective), `None`: parallel projection.
    eye: Option<f32>,
    oblique: bool,
    /// Fit: scale and offset into the plot.
    k: f32,
    off: P,
}

impl Cam {
    fn new(v: &View3D, w: f32, h: f32, d: f32) -> Self {
        let (ry, rx) = (v.rot_y.to_radians(), v.rot_x.to_radians());
        let size = (w * w + h * h + d * d).sqrt();
        let eye = (!v.r_ang_ax && v.perspective > 0.5)
            .then(|| size / 2.0 / (v.perspective.to_radians() / 2.0).tan().max(0.05) + size / 2.0);
        Cam {
            w,
            h,
            d,
            sx: rx.sin(),
            cx: rx.cos(),
            sy: ry.sin(),
            cy: ry.cos(),
            eye,
            oblique: v.r_ang_ax,
            k: 1.0,
            off: p(0.0, 0.0),
        }
    }

    /// Screen position (before fitting) and depth (larger: farther).
    fn raw(&self, q: P3) -> (P, f32) {
        let (x, y, z) = (q.x - self.w / 2.0, q.y - self.h / 2.0, q.z - self.d / 2.0);
        // Turned about the vertical axis, then tilted towards the viewer.
        let x1 = x * self.cy + z * self.sy;
        let z1 = -x * self.sy + z * self.cy;
        let y2 = y * self.cx + z1 * self.sx;
        let z2 = -y * self.sx + z1 * self.cx;
        if self.oblique {
            // Front face square on, depth receding up and to the side.
            let k = 0.5;
            return (p(x + z * self.sy * k, -(y + z * self.sx * k)), z2);
        }
        let f = self.eye.map_or(1.0, |e| e / (e + z2));
        (p(x1 * f, -y2 * f), z2)
    }

    fn at(&self, q: P3) -> P {
        let (s, _) = self.raw(q);
        p(self.off.x + s.x * self.k, self.off.y + s.y * self.k)
    }

    fn depth(&self, q: P3) -> f32 {
        self.raw(q).1
    }

    /// Fit the box's corners (and `extra` points) into `r`.
    fn fit(&mut self, r: Rect, extra: &[P3]) {
        let mut pts: Vec<P3> = Vec::new();
        for x in [0.0, self.w] {
            for y in [0.0, self.h] {
                for z in [0.0, self.d] {
                    pts.push(p3(x, y, z));
                }
            }
        }
        pts.extend_from_slice(extra);
        let raw: Vec<P> = pts.iter().map(|q| self.raw(*q).0).collect();
        let (x0, x1) = minmax(raw.iter().map(|q| q.x));
        let (y0, y1) = minmax(raw.iter().map(|q| q.y));
        let k = (r[2] / (x1 - x0).max(1e-6)).min(r[3] / (y1 - y0).max(1e-6));
        self.k = k;
        self.off = p(
            r[0] + (r[2] - (x1 - x0) * k) / 2.0 - x0 * k,
            r[1] + (r[3] - (y1 - y0) * k) / 2.0 - y0 * k,
        );
    }
}

fn minmax(it: impl Iterator<Item = f32>) -> (f32, f32) {
    it.fold((f32::INFINITY, f32::NEG_INFINITY), |(a, b), v| {
        (a.min(v), b.max(v))
    })
}

/// A face to draw: its corners, fill and outline.
struct Face {
    pts: Vec<P3>,
    fill: Fill,
    line: Option<Stroke>,
}

/// A solid: its faces, drawn together, ordered by its centre's depth.
struct Solid {
    centre: P3,
    faces: Vec<Face>,
}

/// `c` lighter (k > 0) or darker (k < 0) by `k` (0..1).
fn shade(fill: &Fill, k: f32) -> Fill {
    let f = |c: Rgb| {
        Rgb(c.0.map(|v| {
            let v = v as f32;
            (if k >= 0.0 {
                v + (255.0 - v) * k
            } else {
                v * (1.0 + k)
            })
            .round() as u8
        }))
    };
    match fill {
        Fill::Solid(c, a) => Fill::Solid(f(*c), *a),
        Fill::Gradient(_) => fill
            .main_color()
            .map_or(Fill::None, |c| Fill::Solid(f(c), 255)),
        other => other.clone(),
    }
}

/// A box from `a` (low corner) to `b`, shaded: top lighter, sides and
/// bottom darker.
fn cuboid(a: P3, b: P3, fill: &Fill, line: &Option<Stroke>) -> Solid {
    let c = |x: bool, y: bool, z: bool| {
        p3(
            if x { b.x } else { a.x },
            if y { b.y } else { a.y },
            if z { b.z } else { a.z },
        )
    };
    let face = |pts: [P3; 4], k: f32| Face {
        pts: pts.to_vec(),
        fill: shade(fill, k),
        line: line.clone(),
    };
    Solid {
        centre: p3((a.x + b.x) / 2.0, (a.y + b.y) / 2.0, (a.z + b.z) / 2.0),
        faces: vec![
            face(
                [
                    c(false, false, true),
                    c(true, false, true),
                    c(true, true, true),
                    c(false, true, true),
                ],
                -0.1,
            ),
            face(
                [
                    c(false, false, false),
                    c(true, false, false),
                    c(true, false, true),
                    c(false, false, true),
                ],
                -0.3,
            ),
            face(
                [
                    c(false, false, false),
                    c(false, true, false),
                    c(false, true, true),
                    c(false, false, true),
                ],
                -0.2,
            ),
            face(
                [
                    c(true, false, false),
                    c(true, true, false),
                    c(true, true, true),
                    c(true, false, true),
                ],
                -0.2,
            ),
            face(
                [
                    c(false, true, false),
                    c(true, true, false),
                    c(true, true, true),
                    c(false, true, true),
                ],
                0.2,
            ),
            face(
                [
                    c(false, false, false),
                    c(true, false, false),
                    c(true, true, false),
                    c(false, true, false),
                ],
                0.0,
            ),
        ],
    }
}

fn path_of(cam: &Cam, pts: &[P3]) -> Path {
    let mut path = Path::default();
    for (i, q) in pts.iter().enumerate() {
        let s = cam.at(*q);
        if i == 0 {
            path.move_to(s);
        } else {
            path.line_to(s);
        }
    }
    path.close();
    path
}

fn draw_face(o: &mut Out, cam: &Cam, f: &Face) {
    let path = path_of(cam, &f.pts);
    let Some(b) = path.bounds() else { return };
    o.boxed(
        path,
        [b[0], b[1], b[2] - b[0], b[3] - b[1]],
        &f.fill,
        f.line.clone(),
    );
}

fn draw_solids(o: &mut Out, cam: &Cam, mut solids: Vec<Solid>) {
    solids.sort_by(|a, b| cam.depth(b.centre).total_cmp(&cam.depth(a.centre)));
    for mut s in solids {
        let mid = |f: &Face| {
            let n = f.pts.len().max(1) as f32;
            let c = f.pts.iter().fold(p3(0.0, 0.0, 0.0), |a, q| {
                p3(a.x + q.x, a.y + q.y, a.z + q.z)
            });
            cam.depth(p3(c.x / n, c.y / n, c.z / n))
        };
        s.faces.sort_by(|a, b| mid(b).total_cmp(&mid(a)));
        for f in &s.faces {
            draw_face(o, cam, f);
        }
    }
}

/// The series in order and the number of categories.
fn data(g: &Group) -> (Vec<&Series>, usize) {
    let ser: Vec<&Series> = g.series.iter().collect();
    let n = ser
        .iter()
        .map(|s| s.vals.len().max(s.cats.len()))
        .max()
        .unwrap_or(0);
    (ser, n)
}

/// Category labels of a group.
fn cat_labels(g: &Group, n: usize, ax: Option<&Axis>) -> Vec<String> {
    let fmt = ax
        .and_then(|a| a.num_fmt.as_ref())
        .filter(|(code, linked)| !linked || is_date_code(code))
        .map(|f| f.0.clone());
    match g
        .series
        .iter()
        .find(|s| !s.cats.is_empty() || !s.cat_nums.is_empty())
    {
        Some(s) => (0..n).map(|i| cat_label(s, i, fmt.as_deref())).collect(),
        None => (1..=n).map(|i| i.to_string()).collect(),
    }
}

/// Draw a 3-D chart (`chart.view3d` set) in `area`.
pub(super) fn draw(chart: &Chart, o: &mut Out, area: Rect) {
    let Some(v) = &chart.view3d else { return };
    let Some(g) = chart.groups.first() else {
        return;
    };
    let plot = manual_rect(chart.plot_layout, o.size[0], o.size[1]).unwrap_or(area);
    match v.kind {
        Kind3D::Pie => pie3d(g, v, o, plot),
        _ => boxed3d(chart, g, v, o, plot),
    }
}

/// Columns, bars, lines, areas and surfaces: in a box with walls.
fn boxed3d(chart: &Chart, g: &Group, v: &View3D, o: &mut Out, plot: Rect) {
    let (ser, n) = data(g);
    if n == 0 || ser.is_empty() {
        return;
    }
    let horizontal = matches!(
        g.kind,
        Kind::Bar {
            horizontal: true,
            ..
        }
    );
    let grouping = group_grouping(g);
    let rows = if v.series_depth { ser.len() } else { 1 };
    // The box: 1 across, its depth by depthPercent (a row of series as deep
    // as a category is wide), its height automatic or hPercent.
    let slot = 1.0 / n as f32;
    let row_d = slot * v.depth_percent / 100.0 * (1.0 + v.gap_depth / 300.0);
    let d = (row_d * rows as f32).clamp(0.02, 3.0);
    let h = 0.6 * v.h_percent.map_or(1.0, |h| h / 100.0);
    let (w, h) = if horizontal {
        (1.0, h.max(0.3) * 1.2)
    } else {
        (1.0, h)
    };
    // Values and their scale.
    let vals: Vec<&[Option<f64>]> = ser.iter().map(|s| s.vals.as_slice()).collect();
    let stacked = !v.series_depth && grouping.stacked();
    let stacks = stacked_values(
        &vals,
        if v.series_depth {
            Grouping::Standard
        } else {
            grouping
        },
    );
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for (a, b) in stacks.iter().flatten() {
        lo = lo.min(a.min(*b));
        hi = hi.max(a.max(*b));
    }
    if !lo.is_finite() {
        (lo, hi) = (0.0, 1.0);
    }
    let va = chart.axes.iter().find(|a| a.kind == AxisKind::Val);
    let percent = !v.series_depth && grouping == Grouping::PercentStacked;
    let sc = if percent {
        auto_scale(
            0.0,
            1.0,
            Fixed {
                min: Some(0.0),
                max: Some(1.0),
                unit: None,
            },
            5,
        )
    } else {
        auto_scale(
            lo,
            hi,
            Fixed {
                min: va.and_then(|a| a.min),
                max: va.and_then(|a| a.max),
                unit: va.and_then(|a| a.major_unit),
            },
            5,
        )
    };
    let frac = |x: f64| sc.frac(x.clamp(sc.min, sc.max)) as f32;
    let len_v = if horizontal { w } else { h };
    let len_c = if horizontal { h } else { w };
    // A point in the box from category position `c` (0..1 along the
    // category axis), value fraction `f` and depth `z`.
    let at = |c: f32, f: f32, z: f32| {
        if horizontal {
            p3(f * len_v, c * len_c, z)
        } else {
            p3(c * len_c, f * len_v, z)
        }
    };
    let mut cam = Cam::new(v, w, h, d);
    // Labels: categories under the front edge, values beside the wall,
    // series beside the floor's side (a series axis).
    let base = o.base.size_pt.unwrap_or(18.0);
    let ca = chart
        .axes
        .iter()
        .find(|a| matches!(a.kind, AxisKind::Cat | AxisKind::Date));
    let sa = chart.axes.iter().find(|a| a.kind == AxisKind::Ser);
    let cfont = o.font(
        &ca.map(|a| a.font.clone()).unwrap_or_default(),
        base * 0.9,
        false,
    );
    let vfont = o.font(
        &va.map(|a| a.font.clone()).unwrap_or_default(),
        base * 0.9,
        false,
    );
    let cats = cat_labels(g, n, ca);
    let fmt = va
        .and_then(|a| a.num_fmt.as_ref())
        .filter(|(_, linked)| !linked)
        .map(|f| f.0.clone())
        .or_else(|| ser.iter().find_map(|s| s.fmt.clone()))
        .or_else(|| percent.then(|| "0%".to_owned()));
    let ticks: Vec<(f64, String)> = sc
        .ticks()
        .into_iter()
        .map(|t| (t, format_number(t, fmt.as_deref())))
        .collect();
    let show = |a: Option<&Axis>| a.is_none_or(|a| !a.delete && a.tick_lbl_pos != "none");
    let vw = if show(va) {
        ticks
            .iter()
            .map(|t| o.measure(&t.1, &vfont, None).0)
            .fold(0.0, f32::max)
    } else {
        0.0
    };
    let ch = if show(ca) {
        cats.iter()
            .map(|t| o.measure(t, &cfont, None).1)
            .fold(0.0, f32::max)
    } else {
        0.0
    };
    let names: Vec<String> = ser
        .iter()
        .enumerate()
        .map(|(k, s)| s.name.clone().unwrap_or_else(|| format!("Series{}", k + 1)))
        .collect();
    let sw = if v.series_depth && show(sa) && v.kind != Kind3D::Surface
        || v.kind == Kind3D::Surface && show(sa)
    {
        names
            .iter()
            .map(|t| o.measure(t, &cfont, None).0)
            .fold(0.0, f32::max)
    } else {
        0.0
    };
    let pad = 6.0;
    let inner = [
        plot[0] + vw + pad,
        plot[1] + pad,
        (plot[2] - vw - sw - 3.0 * pad).max(8.0),
        (plot[3] - ch - 3.0 * pad).max(8.0),
    ];
    cam.fit(inner, &[]);
    // Walls and floor: the back wall, the side wall away from the viewer.
    let wall_x = if cam.sy >= 0.0 || cam.oblique { 0.0 } else { w };
    let floor_y = if cam.sx >= 0.0 { 0.0 } else { h };
    let wall_fill = |l: &Look| fill_of(l.fill.as_ref(), None);
    let wall_line = |l: &Look| stroke_of(l.line.as_ref(), None);
    let walls = [
        (
            vec![p3(0.0, 0.0, d), p3(w, 0.0, d), p3(w, h, d), p3(0.0, h, d)],
            &v.back_wall,
        ),
        (
            vec![
                p3(wall_x, 0.0, 0.0),
                p3(wall_x, 0.0, d),
                p3(wall_x, h, d),
                p3(wall_x, h, 0.0),
            ],
            &v.side_wall,
        ),
        (
            vec![
                p3(0.0, floor_y, 0.0),
                p3(w, floor_y, 0.0),
                p3(w, floor_y, d),
                p3(0.0, floor_y, d),
            ],
            &v.floor,
        ),
    ];
    for (pts, look) in &walls {
        let f = Face {
            pts: pts.clone(),
            fill: wall_fill(look),
            line: wall_line(look),
        };
        draw_face(o, &cam, &f);
    }
    // Value gridlines on the walls.
    let grid = va
        .and_then(|a| a.major_grid.as_ref())
        .and_then(|l| stroke_of(l.line.as_ref(), Some((GRID, 0.75 * PX_PER_PT))));
    if let Some(st) = &grid {
        for (t, _) in &ticks {
            let f = frac(*t);
            if horizontal {
                let x = f * w;
                o.line(cam.at(p3(x, 0.0, d)), cam.at(p3(x, h, d)), st);
                o.line(cam.at(p3(x, floor_y, 0.0)), cam.at(p3(x, floor_y, d)), st);
            } else {
                let y = f * h;
                o.line(cam.at(p3(0.0, y, d)), cam.at(p3(w, y, d)), st);
                o.line(cam.at(p3(wall_x, y, 0.0)), cam.at(p3(wall_x, y, d)), st);
            }
        }
    }
    // The data.
    let mut solids = Vec::new();
    let gap = match g.kind {
        Kind::Bar { gap, .. } => gap / 100.0,
        _ => 1.5,
    };
    let row_z = |r: usize| {
        let z0 = r as f32 * row_d;
        let depth = row_d / (1.0 + v.gap_depth / 100.0);
        let m = (row_d - depth) / 2.0;
        (z0 + m, z0 + m + depth)
    };
    match v.kind {
        Kind3D::Bar => {
            let across = if v.series_depth || stacked {
                1.0
            } else {
                ser.len() as f32
            };
            let bw = 1.0 / (across + gap);
            for (k, s) in ser.iter().enumerate() {
                let (z0, z1) = row_z(if v.series_depth { k } else { 0 });
                let lane = if v.series_depth || stacked {
                    0.0
                } else {
                    k as f32
                };
                for (i, &(b0, b1)) in stacks[k].iter().enumerate().take(n) {
                    let Some(Some(_)) = s.vals.get(i) else {
                        continue;
                    };
                    let b0 = if stacked { b0 } else { 0.0 };
                    let u0 = (i as f32 + gap * bw / 2.0 + lane * bw) * slot;
                    let u1 = u0 + bw * slot;
                    let (f0, f1) = (frac(b0), frac(b1));
                    let st = point_style(g, s, i, o.d);
                    let a = at(u0, f0.min(f1), z0);
                    let b = at(u1, f0.max(f1), z1);
                    let lo3 = p3(a.x.min(b.x), a.y.min(b.y), z0);
                    let hi3 = p3(a.x.max(b.x), a.y.max(b.y), z1);
                    solids.push(cuboid(lo3, hi3, &st.fill, &st.line));
                }
            }
        }
        Kind3D::Line | Kind3D::Area => {
            let area = v.kind == Kind3D::Area;
            for (k, s) in ser.iter().enumerate() {
                let (z0, z1) = row_z(if v.series_depth { k } else { 0 });
                let st = series_style(g, s, o.d);
                let fill = if area {
                    st.fill.clone()
                } else {
                    Fill::Solid(
                        st.line
                            .as_ref()
                            .map_or(auto_color(o.d, s.idx), |l| rgb_of(l.color)),
                        255,
                    )
                };
                let edge = if area { st.line.clone() } else { None };
                let pts: Vec<Option<(f32, f32, f32)>> = (0..n)
                    .map(|i| {
                        s.vals.get(i).copied().flatten()?;
                        let (b0, b1) = stacks[k][i];
                        let lo = if stacked { frac(b0) } else { frac(0.0) };
                        Some(((i as f32 + 0.5) * slot, frac(b1) * h, lo * h))
                    })
                    .collect();
                let thick = h * 0.02;
                for i in 0..n.saturating_sub(1) {
                    let (Some(a), Some(b)) = (pts[i], pts[i + 1]) else {
                        continue;
                    };
                    let mut faces = Vec::new();
                    // The top (a ribbon, or the area's top).
                    faces.push(Face {
                        pts: vec![
                            p3(a.0, a.1, z0),
                            p3(b.0, b.1, z0),
                            p3(b.0, b.1, z1),
                            p3(a.0, a.1, z1),
                        ],
                        fill: shade(&fill, 0.15),
                        line: edge.clone(),
                    });
                    let (fa, fb) = if area {
                        (a.2, b.2)
                    } else {
                        (a.1 - thick, b.1 - thick)
                    };
                    // A ribbon's front edge (an area's front is one face, below).
                    if !area {
                        faces.push(Face {
                            pts: vec![
                                p3(a.0, a.1, z0),
                                p3(b.0, b.1, z0),
                                p3(b.0, fb, z0),
                                p3(a.0, fa, z0),
                            ],
                            fill: fill.clone(),
                            line: edge.clone(),
                        });
                    }
                    // The underside: a ribbon's, an area's seen from below.
                    if !area || v.rot_x < 0.0 {
                        faces.push(Face {
                            pts: vec![
                                p3(a.0, fa, z0),
                                p3(b.0, fb, z0),
                                p3(b.0, fb, z1),
                                p3(a.0, fa, z1),
                            ],
                            fill: shade(&fill, -0.3),
                            line: None,
                        });
                    }
                    solids.push(Solid {
                        centre: p3((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0, (z0 + z1) / 2.0),
                        faces,
                    });
                }
                if area {
                    // The front, one face from the first point to the last.
                    let run: Vec<(f32, f32, f32)> = pts.iter().flatten().copied().collect();
                    if run.len() >= 2 {
                        let mut front: Vec<P3> = run.iter().map(|q| p3(q.0, q.1, z0)).collect();
                        front.extend(run.iter().rev().map(|q| p3(q.0, q.2, z0)));
                        // Sorted with the row's nearest end.
                        let mid = if cam.sy >= 0.0 {
                            run[run.len() - 1].0
                        } else {
                            run[0].0
                        };
                        solids.push(Solid {
                            centre: p3(mid, h / 2.0, z0 - 1e-4),
                            faces: vec![Face {
                                pts: front,
                                fill: fill.clone(),
                                line: edge.clone(),
                            }],
                        });
                    }
                    // The ends.
                    for e in [pts.iter().flatten().next(), pts.iter().flatten().last()]
                        .into_iter()
                        .flatten()
                    {
                        solids.push(Solid {
                            centre: p3(e.0, (e.1 + e.2) / 2.0, (z0 + z1) / 2.0),
                            faces: vec![Face {
                                pts: vec![
                                    p3(e.0, e.2, z0),
                                    p3(e.0, e.1, z0),
                                    p3(e.0, e.1, z1),
                                    p3(e.0, e.2, z1),
                                ],
                                fill: shade(&fill, -0.2),
                                line: edge.clone(),
                            }],
                        });
                    }
                }
            }
        }
        Kind3D::Surface => {
            let wireframe = matches!(
                g.kind,
                Kind::Surface {
                    wireframe: true,
                    ..
                }
            );
            let bands = ex_draw::surface_bands(chart);
            let band_of = |x: f64| {
                bands
                    .iter()
                    .position(|(a, b)| x >= *a && x <= *b)
                    .unwrap_or(if x < sc.min {
                        0
                    } else {
                        bands.len().saturating_sub(1)
                    })
            };
            let cell_d = d / ser.len().max(1) as f32;
            let zc = |k: usize| (k as f32 + 0.5) * cell_d;
            for k in 0..ser
                .len()
                .saturating_sub(1)
                .max(if ser.len() == 1 { 0 } else { 1 })
            {
                for i in 0..n.saturating_sub(1) {
                    let get = |k: usize, i: usize| {
                        ser.get(k).and_then(|s| s.vals.get(i).copied().flatten())
                    };
                    let (Some(a), Some(b), Some(c), Some(e)) =
                        (get(k, i), get(k, i + 1), get(k + 1, i + 1), get(k + 1, i))
                    else {
                        continue;
                    };
                    let x = |i: usize| (i as f32 + 0.5) * slot;
                    let pts = vec![
                        p3(x(i), frac(a) * h, zc(k)),
                        p3(x(i + 1), frac(b) * h, zc(k)),
                        p3(x(i + 1), frac(c) * h, zc(k + 1)),
                        p3(x(i), frac(e) * h, zc(k + 1)),
                    ];
                    let mean = (a + b + c + e) / 4.0;
                    let fill = ex_draw::band_fill(g, band_of(mean), o.d);
                    let line_c = fill
                        .main_color()
                        .map_or(GRID, |c| Rgb(c.0.map(|v| (v as f32 * 0.7) as u8)));
                    let face = Face {
                        pts: pts.clone(),
                        fill: if wireframe { Fill::None } else { fill },
                        line: Some(Stroke {
                            width: 0.75,
                            color: rgba(
                                if wireframe {
                                    line_c
                                } else {
                                    Rgb([255, 255, 255])
                                },
                                200,
                            ),
                            dash: None,
                            cap: LineCap::Flat,
                            join: LineJoin::Round,
                        }),
                    };
                    let c3 = pts.iter().fold(p3(0.0, 0.0, 0.0), |s, q| {
                        p3(s.x + q.x / 4.0, s.y + q.y / 4.0, s.z + q.z / 4.0)
                    });
                    solids.push(Solid {
                        centre: c3,
                        faces: vec![face],
                    });
                }
            }
        }
        Kind3D::Pie => {}
    }
    draw_solids(o, &cam, solids);
    // Axis labels.
    let col = |f: &TextFont| f.clone();
    if show(va) {
        for (t, text) in &ticks {
            let f = frac(*t);
            if horizontal {
                let q = cam.at(p3(f * w, 0.0, 0.0));
                o.label_at(text, &col(&vfont), p(q.x, q.y + 3.0), 0.5, 0.0);
            } else {
                let q = cam.at(p3(0.0, f * h, 0.0));
                o.label_at(text, &col(&vfont), p(q.x - 4.0, q.y), 1.0, 0.5);
            }
        }
    }
    if show(ca) {
        let step = (n / 12).max(1);
        for (i, text) in cats.iter().enumerate().step_by(step) {
            let c = (i as f32 + 0.5) * slot;
            if horizontal {
                let q = cam.at(p3(0.0, c * len_c, 0.0));
                o.label_at(text, &col(&cfont), p(q.x - 4.0, q.y), 1.0, 0.5);
            } else {
                let q = cam.at(p3(c, 0.0, 0.0));
                o.label_at(text, &col(&cfont), p(q.x, q.y + 3.0), 0.5, 0.0);
            }
        }
    }
    if sw > 0.0 {
        for (k, name) in names.iter().enumerate() {
            let z = if v.kind == Kind3D::Surface {
                (k as f32 + 0.5) * d / ser.len().max(1) as f32
            } else {
                let (a, b) = row_z(k);
                (a + b) / 2.0
            };
            let q = cam.at(p3(w, 0.0, z));
            o.label_at(name, &col(&cfont), p(q.x + 4.0, q.y), 0.0, 0.5);
        }
    }
}

/// A 3-D pie: tilted by `rotX`, turned by `rotY`, with its edge.
fn pie3d(g: &Group, v: &View3D, o: &mut Out, plot: Rect) {
    let Some(s) = g.series.first() else { return };
    let angles = pie_angles(&s.vals, v.rot_y);
    let tilt = v.rot_x.abs().clamp(10.0, 90.0).to_radians().sin();
    let thick_k = v.h_percent.map_or(1.0, |h| h / 100.0) * 0.1;
    let expl = s
        .points
        .iter()
        .filter_map(|p| p.explosion)
        .chain(std::iter::once(s.explosion))
        .fold(0.0f32, f32::max)
        .min(400.0)
        / 100.0;
    let rx = (plot[2] / 2.0).min(plot[3] / (2.0 * tilt + 2.0 * thick_k * 2.0)) / (1.0 + expl);
    let ry = rx * tilt;
    let t = rx * thick_k * 2.0 * (1.0 - tilt * 0.5);
    let c = p(plot[0] + plot[2] / 2.0, plot[1] + (plot[3] - t) / 2.0);
    // Angles: 0 at 12 o'clock, clockwise (as pie_angles); on the tilted
    // ellipse, x = sin, y = -cos * tilt.
    let on = |cc: P, a: f32, dy: f32| {
        let r = a.to_radians();
        p(cc.x + rx * r.sin(), cc.y - ry * r.cos() + dy)
    };
    struct Slice {
        i: usize,
        a0: f32,
        sweep: f32,
        centre: P,
        style: Style,
    }
    let mut slices: Vec<Slice> = Vec::new();
    for (i, (a0, sweep)) in angles.iter().enumerate() {
        if *sweep <= 0.0 {
            continue;
        }
        let e = s.point(i).and_then(|p| p.explosion).unwrap_or(s.explosion) / 100.0;
        let mid = (a0 + sweep / 2.0).to_radians();
        let centre = p(c.x + rx * e * mid.sin(), c.y - ry * e * mid.cos());
        slices.push(Slice {
            i,
            a0: *a0,
            sweep: *sweep,
            centre,
            style: point_style(g, s, i, o.d),
        });
    }
    let arc = |cc: P, a0: f32, a1: f32, dy: f32| -> Vec<P> {
        let steps = (((a1 - a0).abs() / 4.0).ceil() as usize).max(2);
        (0..=steps)
            .map(|k| on(cc, a0 + (a1 - a0) * k as f32 / steps as f32, dy))
            .collect()
    };
    let poly = |pts: &[P]| {
        let mut path = Path::default();
        for (k, q) in pts.iter().enumerate() {
            if k == 0 {
                path.move_to(*q);
            } else {
                path.line_to(*q);
            }
        }
        path.close();
        path
    };
    let boxed = |o: &mut Out, pts: &[P], fill: &Fill, line: Option<Stroke>| {
        let path = poly(pts);
        if let Some(b) = path.bounds() {
            o.boxed(path, [b[0], b[1], b[2] - b[0], b[3] - b[1]], fill, line);
        }
    };
    // Sides first, the far ones first: cut faces (radial walls) and the
    // front of the rim (angles 90..270, the half towards the viewer).
    let mut sides: Vec<(f32, Vec<P>, Fill, Option<Stroke>)> = Vec::new();
    for sl in &slices {
        let (a0, a1) = (sl.a0, sl.a0 + sl.sweep);
        for a in [a0, a1] {
            let q0 = sl.centre;
            let q1 = on(sl.centre, a, 0.0);
            sides.push((
                0.0,
                vec![q0, q1, p(q1.x, q1.y + t), p(q0.x, q0.y + t)],
                shade(&sl.style.fill, -0.35),
                sl.style.line.clone(),
            ));
        }
        // The rim between a0 and a1, clipped to the front half.
        let (lo, hi) = (a0.max(90.0), a1.min(270.0));
        let ranges = [(lo, hi), (a0.max(450.0), a1.min(630.0))];
        for (b0, b1) in ranges {
            if b1 > b0 {
                let top = arc(sl.centre, b0, b1, 0.0);
                let mut bottom = arc(sl.centre, b1, b0, t);
                let mut pts = top;
                pts.append(&mut bottom);
                sides.push((
                    0.0,
                    pts,
                    shade(&sl.style.fill, -0.25),
                    sl.style.line.clone(),
                ));
            }
        }
    }
    // Nearer (lower on the page) last.
    for s in &mut sides {
        s.0 = s.1.iter().map(|q| q.y).sum::<f32>() / s.1.len().max(1) as f32;
    }
    sides.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, pts, fill, line) in &sides {
        boxed(o, pts, fill, line.clone());
    }
    // Tops.
    for sl in &slices {
        let mut pts = vec![sl.centre];
        pts.extend(arc(sl.centre, sl.a0, sl.a0 + sl.sweep, 0.0));
        boxed(o, &pts, &sl.style.fill, sl.style.line.clone());
        let _ = sl.i;
    }
}
