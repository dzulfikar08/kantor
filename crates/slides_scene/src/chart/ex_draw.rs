//! Layout of the chart types with their own geometry: surface (as
//! PowerPoint's 2-D contour view), pie of pie / bar of pie, and the
//! chartex funnel, treemap, sunburst, box & whisker and region maps. A child of
//! [`super`] (the layout), sharing its helpers.

use super::super::ex::{BoxStats, ExKind, ExPlot, Node2, hierarchy};
use super::super::geo::{CacheState, GeoMap, Projector, RegionLabels, Scale as GeoScale};
use super::*;

fn contrast(fill: &Fill) -> Rgba {
    let c = fill.main_color().unwrap_or(Rgb([255, 255, 255]));
    let l = 0.299 * c.0[0] as f32 + 0.587 * c.0[1] as f32 + 0.114 * c.0[2] as f32;
    if l < 150.0 {
        Rgba([255, 255, 255, 255])
    } else {
        Rgba([0x40, 0x40, 0x40, 255])
    }
}

fn white_line(w: f32) -> Stroke {
    Stroke {
        width: w,
        color: Rgba([255, 255, 255, 255]),
        dash: None,
        cap: LineCap::Flat,
        join: LineJoin::Miter,
    }
}

fn shade(c: Rgb, f: f32) -> Rgb {
    Rgb(c.0.map(|x| (x as f32 * f).round() as u8))
}

/// A label inside `r` at its top left (or centred), drawn only if it fits.
fn label_in(o: &mut Out, text: &str, font: &TextFont, r: Rect, centred: bool) -> bool {
    if text.is_empty() {
        return false;
    }
    let pad = 3.0;
    let (w, h) = o.measure(text, font, None);
    let (tw, th, wrap) = if w + 2.0 * pad <= r[2] {
        (w, h, None)
    } else {
        let wrap = (r[2] - 2.0 * pad).max(1.0);
        let (tw, th) = o.measure(text, font, Some(wrap));
        (tw, th, Some(wrap))
    };
    if tw + 2.0 * pad > r[2] + 0.5 || th + 2.0 * pad > r[3] + 0.5 {
        return false;
    }
    let (x, y) = if centred {
        (r[0] + (r[2] - tw) / 2.0, r[1] + (r[3] - th) / 2.0)
    } else {
        (r[0] + pad, r[1] + pad)
    };
    let _ = wrap;
    o.text(Label {
        text: text.to_owned(),
        font: font.clone(),
        x,
        y,
        w: tw,
        h: th,
        align: if centred {
            HAlign::Center
        } else {
            HAlign::Left
        },
        rot: 0.0,
    });
    true
}

// ── funnel ─────────────────────────────────────────────────────────────────

/// A funnel: one centred bar per category, top to bottom, its width the
/// value's share of the largest; category labels on the left.
pub(super) fn funnel(chart: &Chart, g: &Group, plot: &ExPlot, o: &mut Out, area: Rect) {
    let Some(s) = g.series.first() else { return };
    let n = s.vals.len().max(s.cats.len());
    if n == 0 {
        return;
    }
    let base = o.base.size_pt.unwrap_or(18.0);
    let cat_ax = chart.axes.iter().find(|a| a.kind == AxisKind::Cat);
    let show_cats = cat_ax.is_none_or(|a| !a.delete && a.tick_lbl_pos != "none");
    let font = o.font(cat_ax.map_or(&Font::default(), |a| &a.font), base, false);
    let gap = 6.0;
    let mut left = 0.0f32;
    if show_cats {
        for i in 0..n {
            left = left.max(o.measure(&cat_label(s, i, None), &font, None).0);
        }
        left = (left + gap).min(area[2] * 0.4);
    }
    let plot_r = [area[0] + left, area[1], (area[2] - left).max(1.0), area[3]];
    plot_back(chart, o, plot_r);
    let max = s.vals.iter().flatten().fold(0.0f64, |m, v| m.max(v.abs()));
    let band = plot_r[3] / n as f32;
    let bh = band / (1.0 + plot.gap / 100.0);
    let cx = plot_r[0] + plot_r[2] / 2.0;
    for i in 0..n {
        let y = plot_r[1] + band * i as f32 + (band - bh) / 2.0;
        if show_cats {
            o.label_at(
                &cat_label(s, i, None),
                &font,
                p(plot_r[0] - gap, y + bh / 2.0),
                1.0,
                0.5,
            );
        }
        let Some(v) = s.vals.get(i).copied().flatten() else {
            continue;
        };
        let w = if max > 0.0 {
            (v.abs() / max) as f32 * plot_r[2]
        } else {
            0.0
        };
        let r = [cx - w / 2.0, y, w, bh];
        let st = point_style(g, s, i, o.d);
        o.boxed(Path::rect(r[0], r[1], r[2], r[3]), r, &st.fill, st.line);
        if let Some(l) = series_labels(g, s, i)
            && let Some(text) = label_text(&l, s, i, v, None)
        {
            let lf = label_font(o, &l);
            o.label_at(&text, &lf, p(cx, y + bh / 2.0), 0.5, 0.5);
        }
    }
}

// ── treemap ────────────────────────────────────────────────────────────────

/// Squarified rectangles for `vals` (all > 0) in `r`, in the order given.
pub fn squarify(vals: &[f64], r: Rect) -> Vec<Rect> {
    let mut out = vec![[r[0], r[1], 0.0, 0.0]; vals.len()];
    let mut order: Vec<usize> = (0..vals.len()).filter(|&i| vals[i] > 0.0).collect();
    order.sort_by(|a, b| vals[*b].total_cmp(&vals[*a]));
    let total: f64 = order.iter().map(|&i| vals[i]).sum();
    if total <= 0.0 || r[2] <= 0.0 || r[3] <= 0.0 {
        return out;
    }
    let scale = (r[2] as f64 * r[3] as f64) / total;
    let mut free = r;
    let mut k = 0;
    while k < order.len() {
        let short = free[2].min(free[3]) as f64;
        let worst = |row: &[usize]| {
            let sum: f64 = row.iter().map(|&i| vals[i] * scale).sum();
            let (mx, mn) = row.iter().fold((0.0f64, f64::INFINITY), |(a, b), &i| {
                let v = vals[i] * scale;
                (a.max(v), b.min(v))
            });
            let s2 = short * short;
            (s2 * mx / (sum * sum)).max(sum * sum / (s2 * mn))
        };
        let mut end = k + 1;
        while end < order.len() && worst(&order[k..=end]) <= worst(&order[k..end]) {
            end += 1;
        }
        let row = &order[k..end];
        let sum: f64 = row.iter().map(|&i| vals[i] * scale).sum();
        let last = end == order.len();
        if free[2] >= free[3] {
            // A column on the left.
            let w = if last {
                free[2]
            } else {
                (sum / free[3] as f64) as f32
            };
            let mut y = free[1];
            for (j, &i) in row.iter().enumerate() {
                let h = if j + 1 == row.len() {
                    free[1] + free[3] - y
                } else {
                    (vals[i] * scale / w as f64) as f32
                };
                out[i] = [free[0], y, w, h];
                y += h;
            }
            free = [free[0] + w, free[1], (free[2] - w).max(0.0), free[3]];
        } else {
            let h = if last {
                free[3]
            } else {
                (sum / free[2] as f64) as f32
            };
            let mut x = free[0];
            for (j, &i) in row.iter().enumerate() {
                let w = if j + 1 == row.len() {
                    free[0] + free[2] - x
                } else {
                    (vals[i] * scale / h as f64) as f32
                };
                out[i] = [x, free[1], w, h];
                x += w;
            }
            free = [free[0], free[1] + h, free[2], (free[3] - h).max(0.0)];
        }
        k = end;
    }
    out
}

/// A treemap: the top-level categories in their colours, each divided
/// among its children, leaves labelled with their category.
pub(super) fn treemap(chart: &Chart, g: &Group, plot: &ExPlot, o: &mut Out, area: Rect) {
    let Some(s) = g.series.first() else { return };
    let ExKind::Treemap { parent_labels } = &plot.kind else {
        return;
    };
    plot_back(chart, o, area);
    let roots = hierarchy(s);
    let base = o.base.size_pt.unwrap_or(18.0);
    let leaf_font = match s.labels.as_ref() {
        Some(l) => label_font(o, l),
        None => o.font(&Font::default(), base * 0.7, false),
    };
    let mut parent_font = leaf_font.clone();
    parent_font.size_pt *= 1.25;
    parent_font.bold = true;
    let vals: Vec<f64> = roots.iter().map(|n| n.value).collect();
    let rects = squarify(&vals, area);
    let mut labels: Vec<(String, TextFont, Rect, bool)> = Vec::new();
    for (k, (node, r)) in roots.iter().zip(rects).enumerate() {
        let color = auto_color(o.d, k);
        let mut inner = r;
        let has_kids = !node.children.is_empty();
        if has_kids {
            let lh = parent_font.size_pt * PX_PER_PT * 1.25 + 8.0;
            match parent_labels.as_str() {
                "banner" if r[3] > lh * 2.0 => {
                    let banner = [r[0], r[1], r[2], lh];
                    let fill = Fill::Solid(shade(color, 0.8), 255);
                    o.boxed(
                        Path::rect(banner[0], banner[1], banner[2], banner[3]),
                        banner,
                        &fill,
                        Some(white_line(1.0)),
                    );
                    let mut f = parent_font.clone();
                    f.color = contrast(&fill);
                    labels.push((node.label.clone(), f, banner, false));
                    inner = [r[0], r[1] + lh, r[2], r[3] - lh];
                }
                "none" | "banner" => {}
                _ => {
                    let mut f = parent_font.clone();
                    f.color = contrast(&Fill::Solid(color, 255));
                    labels.push((node.label.clone(), f, r, false));
                }
            }
        }
        tree_cells(
            o,
            g,
            s,
            node,
            inner,
            color,
            &leaf_font,
            &mut labels,
            has_kids,
        );
    }
    for (text, font, r, centred) in labels {
        label_in(o, &text, &font, r, centred);
    }
}

#[allow(clippy::too_many_arguments)]
fn tree_cells(
    o: &mut Out,
    g: &Group,
    s: &Series,
    node: &Node2,
    r: Rect,
    color: Rgb,
    font: &TextFont,
    labels: &mut Vec<(String, TextFont, Rect, bool)>,
    under_parent: bool,
) {
    if node.children.is_empty() {
        let fill = node
            .point
            .and_then(|i| s.point(i))
            .and_then(|p| p.look.fill.clone())
            .unwrap_or(Fill::Solid(color, 255));
        o.boxed(
            Path::rect(r[0], r[1], r[2], r[3]),
            r,
            &fill,
            Some(white_line(1.0)),
        );
        let text = match (node.point, s.labels.as_ref()) {
            (Some(i), Some(_)) => series_labels(g, s, i)
                .and_then(|l| label_text(&l, s, i, s.vals[i].unwrap_or(0.0), None)),
            _ => Some(node.label.clone()),
        };
        if let Some(text) = text {
            let mut f = font.clone();
            f.color = contrast(&fill);
            // Below an overlapping parent label.
            let lr = if under_parent {
                let d = font.size_pt * PX_PER_PT * 1.6;
                [r[0], r[1] + d.min(r[3] / 2.0), r[2], (r[3] - d).max(0.0)]
            } else {
                r
            };
            labels.push((text, f, lr, false));
        }
        return;
    }
    let vals: Vec<f64> = node.children.iter().map(|n| n.value).collect();
    for (k, (c, cr)) in node.children.iter().zip(squarify(&vals, r)).enumerate() {
        tree_cells(o, g, s, c, cr, color, font, labels, under_parent && k == 0);
    }
}

// ── sunburst ───────────────────────────────────────────────────────────────

fn depth(n: &[Node2]) -> usize {
    n.iter().map(|c| 1 + depth(&c.children)).max().unwrap_or(0)
}

/// A sunburst: rings from the top level (inside) outwards, clockwise from
/// 12 o'clock, each node's sweep its share of its parent.
pub(super) fn sunburst(chart: &Chart, g: &Group, o: &mut Out, area: Rect) {
    let Some(s) = g.series.first() else { return };
    let roots = hierarchy(s);
    let levels = depth(&roots);
    if levels == 0 {
        return;
    }
    plot_back(chart, o, area);
    let c = p(area[0] + area[2] / 2.0, area[1] + area[3] / 2.0);
    let r = area[2].min(area[3]) / 2.0;
    let hole = 0.35;
    let ring = r / (levels as f32 + hole);
    let total: f64 = roots.iter().map(|n| n.value).sum();
    if total <= 0.0 {
        return;
    }
    let base = o.base.size_pt.unwrap_or(18.0);
    let font = match s.labels.as_ref() {
        Some(l) => label_font(o, l),
        None => o.font(&Font::default(), base * 0.7, false),
    };
    let mut labels = Vec::new();
    let mut at = -90.0f32;
    for (k, n) in roots.iter().enumerate() {
        let sweep = (n.value / total * 360.0) as f32;
        sun_node(
            o,
            s,
            n,
            0,
            at,
            sweep,
            c,
            ring,
            hole,
            auto_color(o.d, k),
            &font,
            &mut labels,
        );
        at += sweep;
    }
    for (text, font, q) in labels {
        o.label_at(&text, &font, q, 0.5, 0.5);
    }
}

#[allow(clippy::too_many_arguments)]
fn sun_node(
    o: &mut Out,
    s: &Series,
    n: &Node2,
    level: usize,
    start: f32,
    sweep: f32,
    c: P,
    ring: f32,
    hole: f32,
    color: Rgb,
    font: &TextFont,
    labels: &mut Vec<(String, TextFont, P)>,
) {
    if sweep <= 0.0 {
        return;
    }
    let r0 = ring * (hole + level as f32);
    // A leaf above the outer rings reaches the edge.
    let r1 = if n.children.is_empty() {
        ring * (hole + level as f32 + 1.0)
    } else {
        r0 + ring
    };
    let fill = n
        .point
        .and_then(|i| s.point(i))
        .and_then(|p| p.look.fill.clone())
        .unwrap_or(Fill::Solid(color, 255));
    o.boxed(
        slice(c, r0, r1, start, sweep),
        [c.x - r1, c.y - r1, 2.0 * r1, 2.0 * r1],
        &fill,
        Some(white_line(1.0)),
    );
    let mid = (start + sweep / 2.0).to_radians();
    let rm = (r0 + r0 + ring) / 2.0;
    let (tw, th) = o.measure(&n.label, font, None);
    let arc = rm * sweep.to_radians();
    if tw <= ring.max(arc) * 0.95 && th <= ring.min(arc) * 0.95 + th * 0.2 && tw <= arc * 1.2 {
        let mut f = font.clone();
        f.color = contrast(&fill);
        labels.push((
            n.label.clone(),
            f,
            p(c.x + rm * mid.cos(), c.y + rm * mid.sin()),
        ));
    }
    let total: f64 = n.children.iter().map(|k| k.value).sum();
    let mut at = start;
    for k in &n.children {
        let sw = if total > 0.0 {
            (k.value / total) as f32 * sweep
        } else {
            0.0
        };
        sun_node(
            o,
            s,
            k,
            level + 1,
            at,
            sw,
            c,
            ring,
            hole,
            color,
            font,
            labels,
        );
        at += sw;
    }
}

// ── box & whisker ──────────────────────────────────────────────────────────

/// Value range of a box & whisker plot (whiskers and the points shown).
pub(super) fn box_range(plot: &ExPlot) -> (f64, f64) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for b in plot.boxes.iter().flatten().flatten() {
        lo = lo.min(b.low);
        hi = hi.max(b.high);
        for v in &b.outliers {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
    }
    (lo, hi)
}

/// Boxes, whiskers, means and points, the series side by side in each
/// category.
pub(super) fn boxes(g: &Group, plot: &ExPlot, ca: &Ax, va: &Ax, area: Rect, o: &mut Out) {
    let ExKind::BoxWhisker {
        mean_line,
        mean_marker,
        inner,
        outliers,
        ..
    } = plot.kind
    else {
        return;
    };
    let n = g.series.len().max(1) as f32;
    let gp = plot.gap / 100.0;
    let bw = 1.0 / (n + gp);
    let at = |u: f64, v: f64| {
        let (cu, vv) = (ca.px(u, area), va.px(v, area));
        if ca.vertical { p(vv, cu) } else { p(cu, vv) }
    };
    for (k, s) in g.series.iter().enumerate() {
        let st = series_style(g, s, o.d);
        let base = st.fill.main_color().unwrap_or(o.d.accents[0]);
        let dark = shade(base, 0.6);
        let line = Stroke {
            width: 1.0,
            color: rgba(dark, 255),
            dash: None,
            cap: LineCap::Flat,
            join: LineJoin::Miter,
        };
        let line = stroke_of(s.look.line.as_ref(), Some((dark, 1.0))).unwrap_or(line);
        let mut means = Vec::new();
        for (i, b) in plot.boxes.get(k).into_iter().flatten().enumerate() {
            let Some(b) = b else { continue };
            let u0 = i as f64 + (gp * bw / 2.0 + k as f32 * bw) as f64;
            let (u1, um) = (u0 + bw as f64, u0 + bw as f64 / 2.0);
            let BoxStats {
                low,
                q1,
                median,
                q3,
                high,
                mean,
                ..
            } = *b;
            // Whiskers and their caps.
            let cap = bw as f64 * 0.25;
            for (a, z) in [(q3, high), (q1, low)] {
                o.line(at(um, a), at(um, z), &line);
                o.line(at(um - cap, z), at(um + cap, z), &line);
            }
            let (a, z) = (at(u0, q1), at(u1, q3));
            let r = [
                a.x.min(z.x),
                a.y.min(z.y),
                (z.x - a.x).abs(),
                (z.y - a.y).abs(),
            ];
            o.boxed(
                Path::rect(r[0], r[1], r[2], r[3]),
                r,
                &st.fill,
                Some(line.clone()),
            );
            o.line(at(u0, median), at(u1, median), &line);
            let mc = at(um, mean);
            means.push(mc);
            if mean_marker {
                marker(
                    o,
                    Symbol::X,
                    mc,
                    7.0,
                    &Fill::Solid(dark, 255),
                    Some(line.clone()),
                );
            }
            let dot = |o: &mut Out, v: f64| {
                marker(
                    o,
                    Symbol::Circle,
                    at(um, v),
                    5.0,
                    &Fill::Solid(Rgb([255, 255, 255]), 255),
                    Some(line.clone()),
                )
            };
            if outliers {
                for v in &b.outliers {
                    dot(o, *v);
                }
            }
            if inner {
                for v in &b.inner {
                    dot(o, *v);
                }
            }
        }
        if mean_line && means.len() >= 2 {
            o.stroke(polyline(&means, false), line.clone());
        }
    }
}

// ── surface ────────────────────────────────────────────────────────────────

/// The value bands of a surface chart: the value axis' ticks.
pub(super) fn surface_bands(chart: &Chart) -> Vec<(f64, f64)> {
    let Some(g) = chart.groups.first() else {
        return Vec::new();
    };
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in g.series.iter().flat_map(|s| s.vals.iter().flatten()) {
        lo = lo.min(*v);
        hi = hi.max(*v);
    }
    let va = chart.axes.iter().find(|a| a.kind == AxisKind::Val);
    let sc = auto_scale(
        lo,
        hi,
        Fixed {
            min: va.and_then(|a| a.min),
            max: va.and_then(|a| a.max),
            unit: va.and_then(|a| a.major_unit),
        },
        8,
    );
    let t = sc.ticks();
    t.windows(2).map(|w| (w[0], w[1])).collect()
}

fn surface_fmt(chart: &Chart) -> Option<String> {
    chart
        .axes
        .iter()
        .find(|a| a.kind == AxisKind::Val)
        .and_then(|a| a.num_fmt.as_ref())
        .filter(|(_, linked)| !linked)
        .map(|f| f.0.clone())
        .or_else(|| {
            chart
                .groups
                .first()?
                .series
                .iter()
                .find_map(|s| s.fmt.clone())
        })
}

/// Legend text of band `(a, b)`.
pub(super) fn band_label(chart: &Chart, (a, b): (f64, f64)) -> String {
    let f = surface_fmt(chart);
    format!(
        "{}-{}",
        format_number(a, f.as_deref()),
        format_number(b, f.as_deref())
    )
}

pub(super) fn band_fill(g: &Group, k: usize, d: &Defaults) -> Fill {
    let Kind::Surface { bands, .. } = &g.kind else {
        return Fill::Solid(auto_color(d, k), 255);
    };
    bands
        .iter()
        .find(|(i, _)| *i == k)
        .and_then(|(_, l)| l.fill.clone())
        .unwrap_or(Fill::Solid(auto_color(d, k), 255))
}

/// A vertex of the surface grid: position and value.
type V3 = (f32, f32, f64);

/// The part of polygon `poly` where `keep(z)` (z linear along edges).
fn clip(poly: &[V3], t: f64, above: bool) -> Vec<V3> {
    let inside = |v: &V3| if above { v.2 >= t } else { v.2 <= t };
    let mut out = Vec::new();
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        if inside(&a) {
            out.push(a);
        }
        if inside(&a) != inside(&b) {
            let f = ((t - a.2) / (b.2 - a.2)) as f32;
            out.push((a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f, t));
        }
    }
    out
}

/// A surface chart seen from above (PowerPoint's contour view): value
/// bands filled in their colours (or their edges only for a wireframe),
/// categories along the bottom, series up the side.
pub(super) fn surface(chart: &Chart, o: &mut Out, area: Rect) {
    let Some(g) = chart.groups.first() else {
        return;
    };
    let Kind::Surface { wireframe, .. } = g.kind else {
        return;
    };
    let series: Vec<&Series> = g.series.iter().collect();
    let ns = series.len();
    let nc = series
        .iter()
        .map(|s| s.vals.len().max(s.cats.len()))
        .max()
        .unwrap_or(0);
    if ns == 0 || nc == 0 {
        return;
    }
    let base = o.base.size_pt.unwrap_or(18.0);
    let cat_ax = chart.axes.iter().find(|a| a.kind == AxisKind::Cat);
    let ser_ax = chart.axes.iter().find(|a| a.kind == AxisKind::Ser);
    let cat_font = o.font(cat_ax.map_or(&Font::default(), |a| &a.font), base, false);
    let ser_font = o.font(ser_ax.map_or(&Font::default(), |a| &a.font), base, false);
    let show_cats = cat_ax.is_none_or(|a| !a.delete && a.tick_lbl_pos != "none");
    let show_sers = ser_ax.is_none_or(|a| !a.delete && a.tick_lbl_pos != "none");
    let names: Vec<String> = series
        .iter()
        .map(|s| {
            s.name
                .clone()
                .unwrap_or_else(|| trf("Series{}", &[&(s.idx + 1)]))
        })
        .collect();
    let gap = 4.0;
    let mut left = 0.0f32;
    if show_sers {
        for n in &names {
            left = left.max(o.measure(n, &ser_font, None).0);
        }
        left = (left + gap).min(area[2] * 0.4);
    }
    let lh = cat_font.size_pt * PX_PER_PT * 1.2;
    let bottom = if show_cats { lh + gap } else { 0.0 };
    let pr = [
        area[0] + left,
        area[1] + lh / 2.0,
        (area[2] - left - lh).max(4.0),
        (area[3] - bottom - lh / 2.0).max(4.0),
    ];
    plot_back(chart, o, pr);
    let x = |i: usize| {
        if nc > 1 {
            pr[0] + pr[2] * i as f32 / (nc - 1) as f32
        } else {
            pr[0] + pr[2] / 2.0
        }
    };
    let y = |j: usize| {
        if ns > 1 {
            pr[1] + pr[3] * (1.0 - j as f32 / (ns - 1) as f32)
        } else {
            pr[1] + pr[3] / 2.0
        }
    };
    let bands = surface_bands(chart);
    let z = |j: usize, i: usize| series[j].vals.get(i).copied().flatten();
    // Two triangles per grid cell.
    let mut tris: Vec<[V3; 3]> = Vec::new();
    for j in 0..ns.saturating_sub(1) {
        for i in 0..nc.saturating_sub(1) {
            let (Some(a), Some(b), Some(c), Some(d)) =
                (z(j, i), z(j, i + 1), z(j + 1, i + 1), z(j + 1, i))
            else {
                continue;
            };
            let va = (x(i), y(j), a);
            let vb = (x(i + 1), y(j), b);
            let vc = (x(i + 1), y(j + 1), c);
            let vd = (x(i), y(j + 1), d);
            tris.push([va, vb, vc]);
            tris.push([va, vc, vd]);
        }
    }
    let grid = Stroke {
        width: 0.75,
        color: rgba(GRID, 255),
        dash: None,
        cap: LineCap::Flat,
        join: LineJoin::Miter,
    };
    if wireframe {
        // The bands' edges, in the colour of the band above each.
        for (k, (lo, _)) in bands.iter().enumerate() {
            let mut path = Path::default();
            for t in &tris {
                let pts: Vec<V3> = (0..3)
                    .filter_map(|e| {
                        let (a, b) = (t[e], t[(e + 1) % 3]);
                        ((a.2 - lo) * (b.2 - lo) < 0.0 || (a.2 == *lo && b.2 != *lo)).then(|| {
                            let f = ((lo - a.2) / (b.2 - a.2)) as f32;
                            (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f, *lo)
                        })
                    })
                    .collect();
                if pts.len() >= 2 {
                    path.move_to(p(pts[0].0, pts[0].1));
                    path.line_to(p(pts[1].0, pts[1].1));
                }
            }
            if let Some(c) = band_fill(g, k, o.d).main_color()
                && !path.is_empty()
            {
                let mut st = grid.clone();
                st.width = 1.5;
                st.color = rgba(c, 255);
                o.stroke(path, st);
            }
        }
    } else {
        for (k, (lo, hi)) in bands.iter().enumerate() {
            let mut path = Path::default();
            let last = k + 1 == bands.len();
            for t in &tris {
                let mut poly = clip(t, *lo, true);
                if !last {
                    poly = clip(&poly, *hi, false);
                }
                if poly.len() < 3 {
                    continue;
                }
                path.move_to(p(poly[0].0, poly[0].1));
                for v in &poly[1..] {
                    path.line_to(p(v.0, v.1));
                }
                path.close();
            }
            if path.is_empty() {
                continue;
            }
            let fill = band_fill(g, k, o.d);
            o.boxed(path, pr, &fill, None);
        }
    }
    // Grid at the data points, then the labels.
    for i in 0..nc {
        o.line(p(x(i), pr[1]), p(x(i), pr[1] + pr[3]), &grid);
    }
    for j in 0..ns {
        o.line(p(pr[0], y(j)), p(pr[0] + pr[2], y(j)), &grid);
    }
    if show_cats && let Some(s) = series.first() {
        for i in 0..nc {
            o.label_at(
                &cat_label(s, i, None),
                &cat_font,
                p(x(i), pr[1] + pr[3] + gap),
                0.5,
                0.0,
            );
        }
    }
    if show_sers {
        for (j, n) in names.iter().enumerate() {
            o.label_at(n, &ser_font, p(pr[0] - gap, y(j)), 1.0, 0.5);
        }
    }
}

// ── pie of pie, bar of pie ─────────────────────────────────────────────────

/// A pie whose last (or chosen) points are taken out into a second, smaller
/// pie or a stacked bar on its right; the first pie shows them as one
/// "Other" slice facing it, joined to it by series lines.
pub(super) fn of_pie(g: &Group, of: &OfPie, o: &mut Out, area: Rect) {
    let Some(s) = g.series.first() else { return };
    let n = s.vals.len();
    let val = |i: usize| s.vals.get(i).copied().flatten().unwrap_or(0.0).max(0.0);
    let second: Vec<usize> = (0..n).filter(|&i| of.split.second(&s.vals, i)).collect();
    let first: Vec<usize> = (0..n).filter(|i| !second.contains(i)).collect();
    let total: f64 = (0..n).map(val).sum();
    let other: f64 = second.iter().map(|&i| val(i)).sum();
    if total <= 0.0 {
        return;
    }
    let k = of.second_size / 100.0;
    let gapf = of.gap / 100.0;
    let lab = s.labels.as_ref().or(g.labels.as_ref());
    let outside = lab.is_some_and(|l| l.shown() && l.pos.as_deref() == Some("outEnd"));
    let height_k = if of.bar { (1.3 * k).max(1.0) } else { 1.0 };
    let mut r1 = (area[3] / 2.0 / height_k).min(area[2] / (2.0 + gapf * k + 2.0 * k));
    if outside {
        r1 *= 0.8;
    }
    let r2 = r1 * k;
    let gap = gapf * r2;
    let width = 2.0 * r1 + gap + 2.0 * r2;
    let c1 = p(
        area[0] + (area[2] - width) / 2.0 + r1,
        area[1] + area[3] / 2.0,
    );
    let c2 = p(c1.x + r1 + gap + r2, c1.y);
    // A temporary series with "Other" as the last point, for labels.
    let mut with_other = s.clone();
    with_other.cats.resize(n, String::new());
    with_other.cats.push(tr("Other").to_owned());
    with_other.vals.resize(n, None);
    with_other.vals.push(Some(other));
    let mut labels: Vec<(String, TextFont, P)> = Vec::new();
    let mut label = |o: &mut Out, i: usize, v: f64, at: P| {
        if let Some(l) = series_labels(g, &with_other, i)
            && let Some(text) = label_text(&l, &with_other, i, v, Some(v / total))
        {
            labels.push((text, label_font(o, &l), at));
        }
    };
    // The first pie: Other centred on the right.
    let other_sweep = (other / total * 360.0) as f32;
    let mut at = other_sweep / 2.0;
    let mut pie_slice = |o: &mut Out, i: usize, sweep: f32, start: f32, fill: Fill, line| {
        o.boxed(
            slice(c1, 0.0, r1, start, sweep),
            [c1.x - r1, c1.y - r1, 2.0 * r1, 2.0 * r1],
            &fill,
            line,
        );
        let mid = (start + sweep / 2.0).to_radians();
        let rr = if outside { r1 + 10.0 } else { r1 * 0.65 };
        let v = if i == n { other } else { val(i) };
        label(o, i, v, p(c1.x + rr * mid.cos(), c1.y + rr * mid.sin()));
    };
    for &i in &first {
        let sweep = (val(i) / total * 360.0) as f32;
        if sweep > 0.0 {
            let st = point_style(g, s, i, o.d);
            pie_slice(o, i, sweep, at, st.fill, st.line);
        }
        at += sweep;
    }
    let other_line = point_style(g, s, 0, o.d).line;
    if other_sweep > 0.0 {
        let fill = Fill::Solid(auto_color(o.d, n), 255);
        pie_slice(o, n, other_sweep, -other_sweep / 2.0, fill, other_line);
    }
    // The second plot.
    let mut top_bottom = (p(c2.x, c2.y - r2), p(c2.x, c2.y + r2));
    if other > 0.0 {
        if of.bar {
            let (bw, bh) = (r2 * 0.6, (2.6 * r2).min(area[3]));
            let x0 = c2.x - bw / 2.0;
            let mut y = c2.y - bh / 2.0;
            top_bottom = (p(x0, y), p(x0, y + bh));
            for &i in &second {
                let h = (val(i) / other) as f32 * bh;
                let st = point_style(g, s, i, o.d);
                let r = [x0, y, bw, h];
                o.boxed(Path::rect(r[0], r[1], r[2], r[3]), r, &st.fill, st.line);
                label(o, i, val(i), p(c2.x, y + h / 2.0));
                y += h;
            }
        } else {
            let mut at = 180.0 - (val(*second.first().unwrap_or(&0)) / other * 180.0) as f32;
            for &i in &second {
                let sweep = (val(i) / other * 360.0) as f32;
                if sweep > 0.0 {
                    let st = point_style(g, s, i, o.d);
                    o.boxed(
                        slice(c2, 0.0, r2, at, sweep),
                        [c2.x - r2, c2.y - r2, 2.0 * r2, 2.0 * r2],
                        &st.fill,
                        st.line,
                    );
                    let mid = (at + sweep / 2.0).to_radians();
                    let rr = if outside { r2 + 10.0 } else { r2 * 0.65 };
                    label(
                        o,
                        i,
                        val(i),
                        p(c2.x + rr * mid.cos(), c2.y + rr * mid.sin()),
                    );
                }
                at += sweep;
            }
        }
        // Series lines from the Other slice's edges.
        let st = stroke_of(
            of.lines.as_ref().and_then(|l| l.line.as_ref()),
            Some((AXIS, 0.75 * PX_PER_PT)),
        );
        if let Some(st) = st.filter(|_| of.lines.is_some()) {
            let h = (other_sweep / 2.0).min(89.0).to_radians();
            let a = p(c1.x + r1 * h.cos(), c1.y - r1 * h.sin());
            let b = p(c1.x + r1 * h.cos(), c1.y + r1 * h.sin());
            o.line(a, top_bottom.0, &st);
            o.line(b, top_bottom.1, &st);
        }
    }
    for (text, font, q) in labels {
        o.label_at(&text, &font, q, 0.5, 0.5);
    }
}

// ── region maps ────────────────────────────────────────────────────────────

/// The region each data point of a map is shown in.
pub(super) fn map_points(s: &Series, map: &GeoMap) -> Vec<Option<usize>> {
    let n = s.vals.len().max(s.cats.len()).max(map.color_strs.len());
    (0..n)
        .map(|i| {
            let leaf = s.cats.get(i).map_or("", String::as_str);
            let outer: Vec<&str> = s
                .levels
                .iter()
                .filter_map(|l| l.get(i).map(String::as_str))
                .collect();
            map.region_for(leaf, &outer)
        })
        .collect()
}

/// A categorical map (text values): its categories in order of first use.
pub(super) fn map_categories(s: &Series, map: &GeoMap) -> Option<Vec<String>> {
    if map.color_strs.is_empty() || s.vals.iter().any(Option::is_some) {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    for c in &map.color_strs {
        if !c.is_empty() && !out.contains(c) {
            out.push(c.clone());
        }
    }
    Some(out)
}

const NO_DATA: Rgb = Rgb([0xD9, 0xD9, 0xD9]);

/// A region map: each region filled with its value's colour on the
/// series' scale (or its category's colour), a gradient legend for values,
/// region labels where they fit. Without region outlines (no cache, or one
/// Kantor can't read) the values are listed instead.
pub(super) fn region_map(chart: &Chart, g: &Group, map: &GeoMap, o: &mut Out, area: Rect) {
    let Some(s) = g.series.first() else { return };
    let points = map_points(s, map);
    let drawable = map.regions.iter().any(|r| !r.rings.is_empty())
        && points.iter().any(Option::is_some);
    if !drawable {
        map_fallback(chart, s, map, o, area);
        return;
    }
    let cats = map_categories(s, map);
    let vals: Vec<f64> = points
        .iter()
        .enumerate()
        .filter(|(_, r)| r.is_some())
        .filter_map(|(i, _)| s.vals.get(i).copied().flatten())
        .collect();
    let (lo, hi) = vals
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), v| {
            (a.min(*v), b.max(*v))
        });
    let scale = (cats.is_none() && lo.is_finite())
        .then(|| GeoScale::new(&map.colors, lo, hi, o.d.accents[0]));
    let mut plot = area;
    if let (Some(sc), Some(lg)) = (&scale, &chart.legend) {
        plot = map_legend(lg, s, sc, o, area);
    }
    plot_back(chart, o, plot);
    // Each region's fill: from its first data point.
    let mut fills: Vec<Option<(Rgb, usize)>> = vec![None; map.regions.len()];
    for (i, r) in points.iter().enumerate() {
        let Some(r) = *r else { continue };
        if fills[r].is_some() {
            continue;
        }
        let color = match (&cats, &scale) {
            (Some(cats), _) => map
                .color_strs
                .get(i)
                .and_then(|c| cats.iter().position(|k| k == c))
                .map(|k| auto_color(o.d, k)),
            (None, Some(sc)) => s.vals.get(i).copied().flatten().map(|v| sc.color(v)),
            _ => None,
        };
        if let Some(c) = color {
            fills[r] = Some((c, i));
        }
    }
    // Fit the regions' projected outlines in the plot.
    let shown: Vec<usize> = (0..map.regions.len())
        .filter(|r| !map.regions[*r].rings.is_empty())
        .collect();
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for r in &shown {
        for ring in &map.regions[*r].rings {
            for q in ring {
                b = [b[0].min(q[0]), b[1].min(q[1]), b[2].max(q[0]), b[3].max(q[1])];
            }
        }
    }
    let proj = Projector::new(map.projection, b);
    let projected: Vec<Vec<Vec<[f64; 2]>>> = shown
        .iter()
        .map(|r| {
            map.regions[*r]
                .rings
                .iter()
                .map(|ring| ring.iter().map(|q| proj.project(q[0], q[1])).collect())
                .collect()
        })
        .collect();
    let mut pb = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for q in projected.iter().flatten().flatten() {
        pb = [pb[0].min(q[0]), pb[1].min(q[1]), pb[2].max(q[0]), pb[3].max(q[1])];
    }
    let margin = 4.0f32;
    let (pw, ph) = ((pb[2] - pb[0]).max(1e-9), (pb[3] - pb[1]).max(1e-9));
    let k = ((plot[2] - 2.0 * margin).max(1.0) as f64 / pw)
        .min((plot[3] - 2.0 * margin).max(1.0) as f64 / ph);
    let ox = plot[0] as f64 + (plot[2] as f64 - pw * k) / 2.0;
    let oy = plot[1] as f64 + (plot[3] as f64 - ph * k) / 2.0;
    let at = |q: &[f64; 2]| p((ox + (q[0] - pb[0]) * k) as f32, (oy + (pb[3] - q[1]) * k) as f32);
    let border = stroke_of(s.look.line.as_ref(), None).unwrap_or_else(|| white_line(0.75));
    let lbl = map.labels != RegionLabels::None;
    let mut labels = Vec::new();
    for (j, r) in shown.iter().enumerate() {
        let mut path = Path::default();
        // The biggest ring (for the label) and its area.
        let mut big: Option<(f32, [f32; 4], P)> = None;
        for ring in &projected[j] {
            let pts: Vec<P> = ring.iter().map(&at).collect();
            let Some((first, rest)) = pts.split_first() else {
                continue;
            };
            path.move_to(*first);
            for q in rest {
                path.line_to(*q);
            }
            path.close();
            let (a, c) = ring_area_centroid(&pts);
            if big.as_ref().is_none_or(|b| a > b.0) {
                let bb = pts.iter().fold(
                    [f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY],
                    |m, q| [m[0].min(q.x), m[1].min(q.y), m[2].max(q.x), m[3].max(q.y)],
                );
                big = Some((a, bb, c));
            }
        }
        if path.is_empty() {
            continue;
        }
        let fill = fills[*r].map_or(NO_DATA, |f| f.0);
        o.marks.push(Mark::Prim {
            prim: Prim::Fill {
                path: path.clone(),
                paint: crate::Paint::Solid(rgba(fill, 255)),
                rule: FillRule::EvenOdd,
            },
            at: Affine::IDENTITY,
        });
        o.stroke(path, border.clone());
        if lbl
            && let (Some((_, i)), Some(big)) = (fills[*r], big)
        {
            labels.push((i, fill, big.1, big.2));
        }
    }
    for (i, fill, bb, c) in labels {
        let Some(l) = series_labels(g, s, i) else {
            continue;
        };
        let mut parts = Vec::new();
        if l.show_ser
            && let Some(n) = &s.name
        {
            parts.push(n.clone());
        }
        if l.show_cat {
            parts.push(cat_label(s, i, None));
        }
        if l.show_val {
            if let Some(v) = s.vals.get(i).copied().flatten() {
                let code = l.num_fmt.as_deref().or(s.fmt.as_deref());
                parts.push(format_number(v, code));
            } else if let Some(c) = map.color_strs.get(i).filter(|c| !c.is_empty()) {
                parts.push(c.clone());
            }
        }
        let sep = l.separator.clone().unwrap_or_else(|| ", ".into());
        let text = parts.join(&sep);
        if text.is_empty() {
            continue;
        }
        let mut font = label_font(o, &l);
        if l.font.color.is_none() {
            font.color = contrast(&Fill::Solid(fill, 255));
        }
        let (tw, th) = o.measure(&text, &font, None);
        if map.labels == RegionLabels::BestFit && (tw > bb[2] - bb[0] || th > bb[3] - bb[1]) {
            continue;
        }
        o.label_at(&text, &font, c, 0.5, 0.5);
    }
}

/// Area (absolute) and centroid of a closed ring.
fn ring_area_centroid(pts: &[P]) -> (f32, P) {
    let (mut a, mut cx, mut cy) = (0.0f64, 0.0f64, 0.0f64);
    for (k, q) in pts.iter().enumerate() {
        let r = pts[(k + 1) % pts.len()];
        let cross = q.x as f64 * r.y as f64 - r.x as f64 * q.y as f64;
        a += cross;
        cx += (q.x as f64 + r.x as f64) * cross;
        cy += (q.y as f64 + r.y as f64) * cross;
    }
    if a.abs() < 1e-9 {
        let n = pts.len().max(1) as f32;
        let s = pts.iter().fold(p(0.0, 0.0), |m, q| m.plus(*q));
        return (0.0, p(s.x / n, s.y / n));
    }
    let c = p((cx / (3.0 * a)) as f32, (cy / (3.0 * a)) as f32);
    ((a / 2.0).abs() as f32, c)
}

/// A value map's legend: a colour bar from the lowest to the highest value
/// with the stops' values beside it, where the chart's legend is. The plot
/// rectangle left.
fn map_legend(lg: &Legend, s: &Series, sc: &GeoScale, o: &mut Out, area: Rect) -> Rect {
    let base = o.base.size_pt.unwrap_or(18.0);
    let font = o.font(&lg.font, base, false);
    let texts: Vec<(f64, String)> = sc
        .stops
        .iter()
        .map(|(v, _)| (*v, format_number(*v, s.fmt.as_deref())))
        .collect();
    let tw = texts
        .iter()
        .map(|(_, t)| o.measure(t, &font, None).0)
        .fold(0.0, f32::max);
    let th = font.size_pt * PX_PER_PT * 1.2;
    let bar = (th * 0.8).max(6.0);
    let gap = th * 0.4;
    let (lo, hi) = sc.range();
    let steps = 48;
    let vertical = matches!(lg.pos, LegendPos::R | LegendPos::L | LegendPos::Tr);
    let mut plot = area;
    if vertical {
        let w = bar + gap + tw;
        let len = (area[3] * 0.6).max(th * 2.0).min(area[3]);
        let x = if lg.pos == LegendPos::L {
            area[0]
        } else {
            area[0] + area[2] - w
        };
        let y = area[1] + (area[3] - len) / 2.0;
        // Highest value at the top.
        for k in 0..steps {
            let t0 = k as f32 / steps as f32;
            let v = hi - (hi - lo) * (k as f64 + 0.5) / steps as f64;
            let r = [x, y + len * t0, bar, len / steps as f32 + 0.5];
            o.boxed(
                Path::rect(r[0], r[1], r[2], r[3]),
                r,
                &Fill::Solid(sc.color(v), 255),
                None,
            );
        }
        for (v, t) in &texts {
            let f = if hi > lo { ((hi - v) / (hi - lo)) as f32 } else { 0.5 };
            o.label_at(t, &font, p(x + bar + gap, y + len * f.clamp(0.0, 1.0)), 0.0, 0.5);
        }
        if !lg.overlay {
            if lg.pos == LegendPos::L {
                plot[0] += w + gap;
            }
            plot[2] = (plot[2] - w - gap).max(1.0);
        }
    } else {
        let h = bar + gap + th;
        let len = (area[2] * 0.6).min(area[2]);
        let x = area[0] + (area[2] - len) / 2.0;
        let y = if lg.pos == LegendPos::T {
            area[1]
        } else {
            area[1] + area[3] - h
        };
        for k in 0..steps {
            let v = lo + (hi - lo) * (k as f64 + 0.5) / steps as f64;
            let r = [x + len * k as f32 / steps as f32, y, len / steps as f32 + 0.5, bar];
            o.boxed(
                Path::rect(r[0], r[1], r[2], r[3]),
                r,
                &Fill::Solid(sc.color(v), 255),
                None,
            );
        }
        for (v, t) in &texts {
            let f = if hi > lo { ((v - lo) / (hi - lo)) as f32 } else { 0.5 };
            o.label_at(t, &font, p(x + len * f.clamp(0.0, 1.0), y + bar + gap), 0.5, 0.0);
        }
        if !lg.overlay {
            if lg.pos == LegendPos::T {
                plot[1] += h + gap;
            }
            plot[3] = (plot[3] - h - gap).max(1.0);
        }
    }
    plot
}

/// No region outlines to draw: say so, and list the values.
fn map_fallback(chart: &Chart, s: &Series, map: &GeoMap, o: &mut Out, area: Rect) {
    plot_back(chart, o, area);
    let base = o.base.size_pt.unwrap_or(18.0);
    let font = o.font(&Font::default(), base * 0.8, false);
    let mut head = font.clone();
    head.bold = true;
    let msg = match map.cache {
        CacheState::Unreadable => tr("Map: the map data saved with this chart can't be read"),
        _ => tr("Map: no map data saved with this chart"),
    };
    let pad = 6.0;
    let wrap = (area[2] - 2.0 * pad).max(10.0);
    let (mw, mh) = o.measure(msg, &head, Some(wrap));
    o.text(Label {
        text: msg.to_owned(),
        font: head,
        x: area[0] + (area[2] - mw) / 2.0,
        y: area[1] + pad,
        w: mw,
        h: mh,
        align: HAlign::Center,
        rot: 0.0,
    });
    let line_h = font.size_pt * PX_PER_PT * 1.25;
    let mut y = area[1] + pad + mh + line_h * 0.5;
    let n = s.vals.len().max(s.cats.len()).max(map.color_strs.len());
    for i in 0..n {
        let value = match s.vals.get(i).copied().flatten() {
            Some(v) => format_number(v, s.fmt.as_deref()),
            None => map.color_strs.get(i).cloned().unwrap_or_default(),
        };
        let name = cat_label(s, i, None);
        let text = if value.is_empty() {
            name
        } else {
            format!("{name}: {value}")
        };
        if y + 2.0 * line_h > area[1] + area[3] && i + 1 < n {
            o.label_at("…", &font, p(area[0] + area[2] / 2.0, y), 0.5, 0.0);
            break;
        }
        o.label_at(&text, &font, p(area[0] + area[2] / 2.0, y), 0.5, 0.0);
        y += line_h;
    }
}
