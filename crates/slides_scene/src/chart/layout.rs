//! [`Chart`] → marks in the frame's own px (0,0 top left): fills and
//! strokes ready for the scene, and labels still to be laid out. Text is
//! measured through [`Measure`], so this module needs no fonts (tests use
//! a fixed-width measure).

use slides_engine::display_text::{tr, trf};
use slides_engine::{EMU_PER_PT, EMU_PER_PX, Fill, LineCap, LineJoin, LineStyle, Rgb};

use super::format::{format_number, is_date_code};
use super::model::*;
use super::scale::{Fixed, Scale, auto_scale, intervals_for};
use crate::geom::{Affine, P, Path, PathFill, p};
use crate::paint::{dash_array, fill_paint};
use crate::{FillRule, Prim, Rgba, Stroke};

/// Points → px.
pub const PX_PER_PT: f32 = EMU_PER_PT / EMU_PER_PX;

/// Resolved text formatting.
#[derive(Clone, Debug, PartialEq)]
pub struct TextFont {
    pub size_pt: f32,
    pub bold: bool,
    pub italic: bool,
    pub color: Rgba,
    /// Typeface (theme references resolved).
    pub face: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HAlign {
    Left,
    Center,
    Right,
}

/// A text label: its box (before turning) and rotation about its centre.
#[derive(Clone, Debug, PartialEq)]
pub struct Label {
    pub text: String,
    pub font: TextFont,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub align: HAlign,
    /// Degrees, clockwise.
    pub rot: f32,
}

/// One thing to draw, bottom to top.
#[derive(Clone, Debug)]
pub enum Mark {
    /// A fill or stroke in its own coordinates, placed with `at`.
    Prim {
        prim: Prim,
        at: Affine,
    },
    Text(Label),
}

/// Text measuring (docs_layout in the scene, fixed widths in tests).
pub trait Measure {
    /// Width and height of `text`, wrapped at `wrap` px when given.
    fn measure(&mut self, text: &str, font: &TextFont, wrap: Option<f32>) -> (f32, f32);
}

/// What the slide's theme gives the chart.
#[derive(Clone, Debug)]
pub struct Defaults {
    pub accents: [Rgb; 6],
    /// `tx1` and `bg1`.
    pub text: Rgb,
    pub background: Rgb,
    /// Theme fonts (minor = body, major = headings).
    pub minor: String,
    pub major: String,
}

impl Default for Defaults {
    fn default() -> Self {
        // Office theme.
        Self {
            accents: [
                Rgb([0x44, 0x72, 0xC4]),
                Rgb([0xED, 0x7D, 0x31]),
                Rgb([0xA5, 0xA5, 0xA5]),
                Rgb([0xFF, 0xC0, 0x00]),
                Rgb([0x5B, 0x9B, 0xD5]),
                Rgb([0x70, 0xAD, 0x47]),
            ],
            text: Rgb([0, 0, 0]),
            background: Rgb([255, 255, 255]),
            minor: "Calibri".into(),
            major: "Calibri Light".into(),
        }
    }
}

/// Colour `i` of a series or point: the accents in order, then darker
/// and lighter variants of them (as Office's chart colours do).
pub fn auto_color(d: &Defaults, i: usize) -> Rgb {
    let base = d.accents[i % 6];
    let round = i / 6 % 6;
    let shade = |f: f32| Rgb(base.0.map(|x| (x as f32 * f).round() as u8));
    let tint = |f: f32| {
        Rgb(base
            .0
            .map(|x| (x as f32 + (255.0 - x as f32) * f).round() as u8))
    };
    match round {
        0 => base,
        1 => shade(0.6),
        2 => tint(0.2),
        3 => shade(0.8),
        4 => tint(0.4),
        _ => shade(0.5),
    }
}

fn rgba(c: Rgb, a: u8) -> Rgba {
    Rgba([c.0[0], c.0[1], c.0[2], a])
}

/// Mid grey for gridlines and axis lines (Office's 15 % / 25 % text).
const GRID: Rgb = Rgb([0xD9, 0xD9, 0xD9]);
const AXIS: Rgb = Rgb([0xBF, 0xBF, 0xBF]);

/// A resolved line.
fn stroke_of(line: Option<&Line>, auto: Option<(Rgb, f32)>) -> Option<Stroke> {
    let (color, width, style) = match line {
        Some(l) => {
            let color = match &l.fill {
                Some(Fill::None) => return None,
                Some(Fill::Solid(c, a)) => rgba(*c, *a),
                Some(f) => rgba(f.main_color()?, l.style.alpha),
                None => rgba(auto?.0, l.style.alpha),
            };
            let width = l.width.or(auto.map(|a| a.1)).unwrap_or(1.0);
            (color, width, l.style.clone())
        }
        None => {
            let (c, w) = auto?;
            (rgba(c, 255), w, LineStyle::default())
        }
    };
    if width <= 0.0 {
        return None;
    }
    Some(Stroke {
        width,
        color,
        dash: dash_array(&style, width),
        cap: style.cap,
        join: style.join,
    })
}

fn fill_of(fill: Option<&Fill>, auto: Option<Rgb>) -> Fill {
    match fill {
        Some(f) => f.clone(),
        None => auto.map_or(Fill::None, |c| Fill::Solid(c, 255)),
    }
}

struct Out<'a> {
    marks: Vec<Mark>,
    d: &'a Defaults,
    m: &'a mut dyn Measure,
    base: Font,
    /// The chart's size.
    size: [f32; 2],
}

impl Out<'_> {
    fn font(&self, f: &Font, size: f32, bold: bool) -> TextFont {
        let f = f.over(&self.base);
        let face = f.face.unwrap_or_else(|| "+mn-lt".into());
        let face = if face.starts_with("+mj") {
            self.d.major.clone()
        } else if face.starts_with("+mn") {
            self.d.minor.clone()
        } else {
            face
        };
        TextFont {
            size_pt: f.size_pt.unwrap_or(size).clamp(1.0, 400.0),
            bold: f.bold.unwrap_or(bold),
            italic: f.italic.unwrap_or(false),
            color: f.color.map_or(rgba(self.d.text, 255), |(c, a)| rgba(c, a)),
            face,
        }
    }

    fn measure(&mut self, text: &str, font: &TextFont, wrap: Option<f32>) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, 0.0);
        }
        self.m.measure(text, font, wrap)
    }

    /// A filled box with its outline (the fill's gradient spans the box).
    fn boxed(&mut self, path: Path, rect: [f32; 4], fill: &Fill, line: Option<Stroke>) {
        let [x, y, w, h] = rect;
        let at = Affine::translate(x, y);
        let local = path.transformed(&Affine::translate(-x, -y));
        if let Some(paint) = fill_paint(fill, PathFill::Norm, w.max(1e-3), h.max(1e-3), &at) {
            self.marks.push(Mark::Prim {
                prim: Prim::Fill {
                    path: local.clone(),
                    paint,
                    rule: FillRule::NonZero,
                },
                at,
            });
        }
        if let Some(stroke) = line {
            self.marks.push(Mark::Prim {
                prim: Prim::Stroke {
                    path: local,
                    stroke,
                },
                at,
            });
        }
    }

    fn stroke(&mut self, path: Path, stroke: Stroke) {
        self.marks.push(Mark::Prim {
            prim: Prim::Stroke { path, stroke },
            at: Affine::IDENTITY,
        });
    }

    fn line(&mut self, a: P, b: P, stroke: &Stroke) {
        let mut path = Path::default();
        path.move_to(a);
        path.line_to(b);
        self.stroke(path, stroke.clone());
    }

    fn text(&mut self, label: Label) {
        if !label.text.is_empty() {
            self.marks.push(Mark::Text(label));
        }
    }

    /// A one-line label anchored at `at`: `ax`/`ay` 0 = left/top, 0.5 =
    /// centre, 1 = right/bottom of the label at `at`.
    fn label_at(&mut self, text: &str, font: &TextFont, at: P, ax: f32, ay: f32) -> [f32; 4] {
        let (w, h) = self.measure(text, font, None);
        let (x, y) = (at.x - w * ax, at.y - h * ay);
        let align = if ax < 0.25 {
            HAlign::Left
        } else if ax > 0.75 {
            HAlign::Right
        } else {
            HAlign::Center
        };
        self.text(Label {
            text: text.to_owned(),
            font: font.clone(),
            x,
            y,
            w,
            h,
            align,
            rot: 0.0,
        });
        [x, y, w, h]
    }
}

/// A rectangle `[x, y, w, h]`.
type Rect = [f32; 4];

/// The chart's marks in a `w` × `h` frame.
pub fn layout(chart: &Chart, w: f32, h: f32, d: &Defaults, m: &mut dyn Measure) -> Vec<Mark> {
    let mut o = Out {
        marks: Vec::new(),
        d,
        m,
        base: chart.font.clone(),
        size: [w, h],
    };
    // Chart area.
    let space_fill = fill_of(chart.space_look.fill.as_ref(), None);
    let space_line = stroke_of(chart.space_look.line.as_ref(), None);
    let outline = if chart.rounded {
        Path::round_rect(w, h, w.min(h) * 0.04)
    } else {
        Path::rect(0.0, 0.0, w, h)
    };
    o.boxed(outline, [0.0, 0.0, w, h], &space_fill, space_line);

    let pad = (w.min(h) * 0.03).clamp(3.0, 10.0);
    let mut area: Rect = [pad, pad, w - 2.0 * pad, h - 2.0 * pad];

    title(chart, &mut o, w, &mut area, pad);
    let entries = legend_entries(chart, d);
    if let Some(lg) = &chart.legend
        && !entries.is_empty()
    {
        legend(lg, &entries, &mut o, [w, h], &mut area, pad);
    }
    if area[2] < 4.0 || area[3] < 4.0 {
        return o.marks;
    }
    if chart
        .groups
        .iter()
        .any(|g| matches!(g.kind, Kind::Pie { .. }))
    {
        let plot = manual_rect(chart.plot_layout, w, h).unwrap_or(area);
        plot_back(chart, &mut o, plot);
        for g in chart
            .groups
            .iter()
            .filter(|g| matches!(g.kind, Kind::Pie { .. }))
        {
            pie(g, &mut o, plot);
        }
    } else {
        axes_chart(chart, &mut o, [w, h], area);
    }
    o.marks
}

fn manual_rect(l: Option<ManualLayout>, w: f32, h: f32) -> Option<Rect> {
    let l = l?;
    Some([l.x? * w, l.y? * h, l.w? * w, l.h? * h])
}

fn plot_back(chart: &Chart, o: &mut Out, r: Rect) {
    let fill = fill_of(chart.plot_look.fill.as_ref(), None);
    let line = stroke_of(chart.plot_look.line.as_ref(), None);
    o.boxed(Path::rect(r[0], r[1], r[2], r[3]), r, &fill, line);
}

/// The title's text: its own, or the only series' name when automatic.
fn title_text(chart: &Chart) -> Option<Vec<String>> {
    let only_name = || {
        let mut all = chart.all_series();
        let (_, s) = all.next()?;
        all.next().is_none().then(|| s.name.clone()).flatten()
    };
    match &chart.title {
        Some(t) => match &t.text {
            Some(text) => Some(text.clone()),
            None => Some(vec![
                only_name().unwrap_or_else(|| tr("Chart Title").into()),
            ]),
        },
        None if !chart.auto_title_deleted => only_name().map(|n| vec![n]),
        None => None,
    }
    .filter(|t| t.iter().any(|l| !l.trim().is_empty()))
}

fn title(chart: &Chart, o: &mut Out, w: f32, area: &mut Rect, pad: f32) {
    let Some(lines) = title_text(chart) else {
        return;
    };
    let t = chart.title.clone().unwrap_or_default();
    let base = o.base.size_pt.unwrap_or(18.0);
    let font = o.font(&t.font, base * 1.2, true);
    let text = lines.join("\n");
    let wrap = (w * 0.8).max(10.0);
    let (tw, th) = o.measure(&text, &font, Some(wrap));
    let x = (w - tw) / 2.0;
    o.text(Label {
        text,
        font,
        x,
        y: area[1],
        w: tw,
        h: th,
        align: HAlign::Center,
        rot: 0.0,
    });
    if !t.overlay {
        area[1] += th + pad;
        area[3] -= th + pad;
    }
}

/// A legend entry: its text and key.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub text: String,
    pub fill: Fill,
    pub line: Option<Stroke>,
    /// Lines and scatter series show a line (with their marker).
    pub line_key: bool,
    pub marker: Option<(Symbol, Fill, Option<Stroke>)>,
}

/// Legend entries: the categories of a pie (or of a single varied
/// series), else every series.
pub fn legend_entries(chart: &Chart, d: &Defaults) -> Vec<Entry> {
    let deleted = chart
        .legend
        .as_ref()
        .map(|l| l.deleted.clone())
        .unwrap_or_default();
    let mut out = Vec::new();
    let varied = chart.groups.len() == 1
        && chart.groups[0].series.len() == 1
        && (chart.groups[0].vary_colors
            || matches!(chart.groups[0].kind, Kind::Pie { hole: None, .. }));
    if varied
        || chart
            .groups
            .iter()
            .all(|g| matches!(g.kind, Kind::Pie { .. }))
    {
        let g = &chart.groups[0];
        if let Some(s) = g.series.first() {
            let n = s.vals.len().max(s.cats.len());
            for i in 0..n {
                let st = point_style(g, s, i, d);
                out.push(Entry {
                    text: cat_label(s, i, None),
                    fill: st.fill,
                    line: st.line,
                    line_key: false,
                    marker: None,
                });
            }
        }
    } else {
        for (g, s) in chart.all_series() {
            let st = series_style(g, s, d);
            let line_key = matches!(g.kind, Kind::Line { .. } | Kind::Scatter { .. });
            out.push(Entry {
                text: s
                    .name
                    .clone()
                    .unwrap_or_else(|| trf("Series{}", &[&(s.idx + 1)])),
                fill: st.fill,
                line: st.line,
                line_key,
                marker: line_key
                    .then(|| marker_style(g, s, None, d))
                    .flatten()
                    .map(|m| (m.0, m.2, m.3)),
            });
        }
    }
    out.into_iter()
        .enumerate()
        .filter(|(i, _)| !deleted.contains(i))
        .map(|(_, e)| e)
        .collect()
}

/// Where each legend entry goes: `(key box, text box)` per entry and the
/// legend's own box.
#[derive(Clone, Debug, PartialEq)]
pub struct LegendLayout {
    pub rect: Rect,
    pub items: Vec<(Rect, Rect)>,
}

/// Lay a legend out at `pos` inside `area` (the chart's free space).
pub fn legend_layout(
    pos: LegendPos,
    sizes: &[(f32, f32)],
    key: f32,
    area: Rect,
    gap: f32,
) -> LegendLayout {
    let [ax, ay, aw, ah] = area;
    let row_h = |i: usize| sizes[i].1.max(key);
    let item_w = |i: usize| key + gap * 0.6 + sizes[i].0;
    let mut items = Vec::new();
    match pos {
        LegendPos::R | LegendPos::L | LegendPos::Tr => {
            let wmax = (0..sizes.len()).map(item_w).fold(0.0, f32::max);
            let total: f32 =
                (0..sizes.len()).map(|i| row_h(i) + gap * 0.3).sum::<f32>() - gap * 0.3;
            let x = match pos {
                LegendPos::L => ax,
                _ => ax + aw - wmax,
            };
            let mut y = match pos {
                LegendPos::Tr => ay,
                _ => ay + (ah - total).max(0.0) / 2.0,
            };
            for (i, &(tw, th)) in sizes.iter().enumerate() {
                let rh = row_h(i);
                items.push((
                    [x, y + (rh - key) / 2.0, key, key],
                    [x + key + gap * 0.6, y + (rh - th) / 2.0, tw, th],
                ));
                y += rh + gap * 0.3;
            }
            LegendLayout {
                rect: [
                    x,
                    items.first().map_or(ay, |f| f.0[1].min(f.1[1])),
                    wmax,
                    total,
                ],
                items,
            }
        }
        LegendPos::T | LegendPos::B => {
            // Rows of entries, each row centred.
            let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
            let mut used = 0.0;
            for i in 0..sizes.len() {
                let wi = item_w(i);
                let row = rows.last_mut().expect("a row");
                if !row.is_empty() && used + gap + wi > aw {
                    rows.push(vec![i]);
                    used = wi;
                } else {
                    used += if row.is_empty() { wi } else { gap + wi };
                    row.push(i);
                }
            }
            let heights: Vec<f32> = rows
                .iter()
                .map(|r| r.iter().map(|&i| row_h(i)).fold(0.0, f32::max))
                .collect();
            let total: f32 = heights.iter().sum::<f32>() + gap * 0.3 * (rows.len() - 1) as f32;
            let mut y = match pos {
                LegendPos::T => ay,
                _ => ay + ah - total,
            };
            let mut wmax: f32 = 0.0;
            let mut x0 = ax + aw;
            for (row, rh) in rows.iter().zip(&heights) {
                let rw: f32 = row.iter().map(|&i| item_w(i)).sum::<f32>()
                    + gap * (row.len().saturating_sub(1)) as f32;
                wmax = wmax.max(rw);
                let mut x = ax + (aw - rw) / 2.0;
                x0 = x0.min(x);
                for &i in row {
                    items.push((
                        [x, y + (rh - key) / 2.0, key, key],
                        [
                            x + key + gap * 0.6,
                            y + (rh - sizes[i].1) / 2.0,
                            sizes[i].0,
                            sizes[i].1,
                        ],
                    ));
                    x += item_w(i) + gap;
                }
                y += rh + gap * 0.3;
            }
            LegendLayout {
                rect: [x0, y - total - gap * 0.3, wmax, total],
                items,
            }
        }
    }
}

fn legend(
    lg: &Legend,
    entries: &[Entry],
    o: &mut Out,
    [w, h]: [f32; 2],
    area: &mut Rect,
    pad: f32,
) {
    let base = o.base.size_pt.unwrap_or(18.0);
    let font = o.font(&lg.font, base, false);
    let max_w = match lg.pos {
        LegendPos::T | LegendPos::B => area[2],
        _ => area[2] * 0.4,
    };
    let sizes: Vec<(f32, f32)> = entries
        .iter()
        .map(|e| {
            let (tw, th) = o.measure(&e.text, &font, None);
            if tw > max_w {
                o.measure(&e.text, &font, Some(max_w))
            } else {
                (tw, th)
            }
        })
        .collect();
    let line_h = font.size_pt * PX_PER_PT * 1.2;
    let key = line_h * 0.55;
    let gap = line_h * 0.6;
    let place = match manual_rect(lg.layout, w, h) {
        Some(r) => r,
        None => *area,
    };
    let lay = legend_layout(lg.pos, &sizes, key, place, gap);
    // Room for the plot.
    if !lg.overlay && lg.layout.is_none() {
        let r = lay.rect;
        match lg.pos {
            LegendPos::R | LegendPos::Tr => area[2] = (r[0] - pad - area[0]).max(0.0),
            LegendPos::L => {
                let right = area[0] + area[2];
                area[0] = r[0] + r[2] + pad;
                area[2] = (right - area[0]).max(0.0);
            }
            LegendPos::T => {
                let bottom = area[1] + area[3];
                area[1] = r[1] + r[3] + pad;
                area[3] = (bottom - area[1]).max(0.0);
            }
            LegendPos::B => area[3] = (r[1] - pad - area[1]).max(0.0),
        }
    }
    let bg = fill_of(lg.look.fill.as_ref(), None);
    let border = stroke_of(lg.look.line.as_ref(), None);
    if !matches!(bg, Fill::None) || border.is_some() {
        let r = lay.rect;
        let m = gap * 0.3;
        let rr = [r[0] - m, r[1] - m, r[2] + 2.0 * m, r[3] + 2.0 * m];
        o.boxed(Path::rect(rr[0], rr[1], rr[2], rr[3]), rr, &bg, border);
    }
    for (e, (k, t)) in entries.iter().zip(&lay.items) {
        if e.line_key {
            let y = k[1] + k[3] / 2.0;
            let (x0, x1) = (k[0] - k[2] * 0.4, k[0] + k[2] * 1.4);
            if let Some(s) = &e.line {
                let mut s = s.clone();
                s.width = s.width.min(k[3] * 0.4);
                s.dash = None;
                o.line(p(x0, y), p(x1, y), &s);
            }
            if let Some((sym, fill, line)) = &e.marker {
                marker(
                    o,
                    *sym,
                    p(k[0] + k[2] / 2.0, y),
                    k[2] * 0.8,
                    fill,
                    line.clone(),
                );
            }
        } else {
            let mut line = e.line.clone();
            if let Some(l) = &mut line {
                l.width = l.width.min(2.0);
            }
            o.boxed(Path::rect(k[0], k[1], k[2], k[3]), *k, &e.fill, line);
        }
        o.text(Label {
            text: e.text.clone(),
            font: font.clone(),
            x: t[0],
            y: t[1],
            w: t[2],
            h: t[3],
            align: HAlign::Left,
            rot: 0.0,
        });
    }
}

// ── styles ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct Style {
    fill: Fill,
    line: Option<Stroke>,
}

fn is_line_kind(g: &Group) -> bool {
    matches!(g.kind, Kind::Line { .. } | Kind::Scatter { .. })
}

/// Default line width of line series: 2.25 pt.
const SERIES_LINE: f32 = 2.25 * PX_PER_PT;

fn series_style(g: &Group, s: &Series, d: &Defaults) -> Style {
    let auto = auto_color(d, s.idx);
    if is_line_kind(g) {
        let line = series_line(g, s, auto);
        Style {
            fill: Fill::Solid(auto, 255),
            line,
        }
    } else {
        Style {
            fill: fill_of(s.look.fill.as_ref(), Some(auto)),
            line: stroke_of(s.look.line.as_ref(), None),
        }
    }
}

/// The line of a line or scatter series (`None` = markers only).
fn series_line(g: &Group, s: &Series, auto: Rgb) -> Option<Stroke> {
    if let Kind::Scatter { style } = &g.kind
        && matches!(style.as_str(), "marker" | "none")
        && s.look.line.as_ref().is_none_or(|l| l.fill.is_none())
    {
        return None;
    }
    let mut st = stroke_of(s.look.line.as_ref(), Some((auto, SERIES_LINE)))?;
    if s.look
        .line
        .as_ref()
        .is_none_or(|l| l.style.cap == LineCap::Flat)
    {
        st.cap = LineCap::Round;
    }
    st.join = LineJoin::Round;
    Some(st)
}

/// A point's fill and outline (`c:dPt`, varied colours, the series').
fn point_style(g: &Group, s: &Series, i: usize, d: &Defaults) -> Style {
    let varied = g.vary_colors
        && (matches!(g.kind, Kind::Pie { .. }) || g.series.len() == 1)
        && !is_line_kind(g);
    let base = if varied {
        let auto = auto_color(d, i);
        Style {
            fill: Fill::Solid(auto, 255),
            line: stroke_of(s.look.line.as_ref(), None),
        }
    } else {
        series_style(g, s, d)
    };
    let Some(pt) = s.point(i) else {
        // Varied colours replace only the automatic fill.
        if varied && let Some(f) = &s.look.fill {
            return Style {
                fill: f.clone(),
                ..base
            };
        }
        return base;
    };
    Style {
        fill: pt.look.fill.clone().unwrap_or(base.fill),
        line: match &pt.look.line {
            Some(l) => stroke_of(
                Some(l),
                base.line.as_ref().map(|b| (rgb_of(b.color), b.width)),
            ),
            None => base.line,
        },
    }
}

fn rgb_of(c: Rgba) -> Rgb {
    Rgb([c.0[0], c.0[1], c.0[2]])
}

/// A segment between two unit-box points.
type Seg2 = ((f32, f32), (f32, f32));

const AUTO_SYMBOLS: [Symbol; 6] = [
    Symbol::Diamond,
    Symbol::Square,
    Symbol::Triangle,
    Symbol::X,
    Symbol::Star,
    Symbol::Circle,
];

/// A series' (or point's) marker: symbol, size px, fill and outline.
fn marker_style(
    g: &Group,
    s: &Series,
    point: Option<usize>,
    d: &Defaults,
) -> Option<(Symbol, f32, Fill, Option<Stroke>)> {
    let auto = auto_color(d, s.idx);
    let m = point
        .and_then(|i| s.point(i))
        .and_then(|p| p.marker.clone())
        .or_else(|| s.marker.clone());
    let shows = match &g.kind {
        Kind::Line { .. } => g.markers,
        Kind::Scatter { style } => !matches!(style.as_str(), "line" | "smooth" | "none"),
        _ => false,
    };
    let sym = match m.as_ref().map(|m| m.symbol) {
        Some(Symbol::None) => return None,
        Some(Symbol::Auto) | None if !shows => return None,
        Some(Symbol::Auto) | None => AUTO_SYMBOLS[s.idx % AUTO_SYMBOLS.len()],
        Some(sym) => sym,
    };
    let size = m.as_ref().map_or(5.0, |m| m.size) * PX_PER_PT;
    let look = m.map(|m| m.look).unwrap_or_default();
    let fill = fill_of(look.fill.as_ref(), Some(auto));
    let line = stroke_of(look.line.as_ref(), Some((auto, 0.75 * PX_PER_PT)));
    Some((sym, size, fill, line))
}

fn marker(o: &mut Out, sym: Symbol, c: P, size: f32, fill: &Fill, line: Option<Stroke>) {
    let r = size / 2.0;
    let mut path = Path::default();
    let poly = |pts: &[(f32, f32)]| {
        let mut pa = Path::default();
        for (i, (x, y)) in pts.iter().enumerate() {
            let q = p(c.x + x * r, c.y + y * r);
            if i == 0 { pa.move_to(q) } else { pa.line_to(q) }
        }
        pa.close();
        pa
    };
    let mut open = false;
    match sym {
        Symbol::Circle | Symbol::Auto => path = Path::ellipse(c.x, c.y, r, r),
        Symbol::Dot => path = Path::ellipse(c.x, c.y, r * 0.4, r * 0.4),
        Symbol::Square => path = Path::rect(c.x - r, c.y - r, size, size),
        Symbol::Diamond => path = poly(&[(0.0, -1.0), (1.0, 0.0), (0.0, 1.0), (-1.0, 0.0)]),
        Symbol::Triangle => path = poly(&[(0.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]),
        Symbol::Dash => path = Path::rect(c.x - r, c.y - r * 0.2, size, r * 0.4),
        Symbol::X | Symbol::Plus | Symbol::Star => {
            open = true;
            let segs: &[Seg2] = match sym {
                Symbol::X => &[((-1.0, -1.0), (1.0, 1.0)), ((-1.0, 1.0), (1.0, -1.0))],
                Symbol::Plus => &[((0.0, -1.0), (0.0, 1.0)), ((-1.0, 0.0), (1.0, 0.0))],
                _ => &[
                    ((-1.0, -1.0), (1.0, 1.0)),
                    ((-1.0, 1.0), (1.0, -1.0)),
                    ((0.0, -1.0), (0.0, 1.0)),
                ],
            };
            for ((x0, y0), (x1, y1)) in segs {
                path.move_to(p(c.x + x0 * r, c.y + y0 * r));
                path.line_to(p(c.x + x1 * r, c.y + y1 * r));
            }
        }
        Symbol::None => return,
    }
    if open {
        let st = line.or_else(|| {
            fill.main_color().map(|c| Stroke {
                width: 1.0,
                color: rgba(c, 255),
                dash: None,
                cap: LineCap::Flat,
                join: LineJoin::Miter,
            })
        });
        if let Some(mut st) = st {
            st.dash = None;
            o.stroke(path, st);
        }
        return;
    }
    let line = line.map(|mut l| {
        l.dash = None;
        l
    });
    o.boxed(path, [c.x - r, c.y - r, size, size], fill, line);
}

// ── data labels ────────────────────────────────────────────────────────────

/// The data label text of point `i` (`None` when it shows nothing).
pub fn label_text(
    l: &Labels,
    s: &Series,
    i: usize,
    value: f64,
    percent: Option<f64>,
) -> Option<String> {
    if !l.shown() {
        return None;
    }
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
        let code = l.num_fmt.as_deref().or(s.fmt.as_deref());
        parts.push(format_number(value, code));
    }
    if l.show_percent
        && let Some(pc) = percent
    {
        let code = l
            .num_fmt
            .as_deref()
            .filter(|c| c.contains('%'))
            .unwrap_or("0%");
        parts.push(format_number(pc, Some(code)));
    }
    let sep = l.separator.clone().unwrap_or_else(|| ", ".into());
    Some(parts.join(&sep)).filter(|t| !t.is_empty())
}

/// Category `i`'s label of a series (numbers with their format).
fn cat_label(s: &Series, i: usize, axis_fmt: Option<&str>) -> String {
    if let Some(c) = s.cats.get(i).filter(|c| !c.is_empty()) {
        return c.clone();
    }
    if let Some(Some(v)) = s.cat_nums.get(i) {
        return format_number(*v, axis_fmt.or(s.cat_fmt.as_deref()));
    }
    if !s.cats.is_empty() || !s.cat_nums.is_empty() {
        return String::new();
    }
    (i + 1).to_string()
}

fn series_labels(g: &Group, s: &Series, i: usize) -> Option<Labels> {
    let l = s.labels.as_ref().or(g.labels.as_ref())?;
    let pl = l.point(i);
    pl.shown().then_some(pl)
}

/// Where a data label goes after its manual offset.
fn moved(l: &Labels, at: P, size: [f32; 2]) -> P {
    match l.offset {
        Some((dx, dy)) => p(at.x + dx * size[0], at.y + dy * size[1]),
        None => at,
    }
}

fn label_font(o: &Out, l: &Labels) -> TextFont {
    let base = o.base.size_pt.unwrap_or(18.0);
    o.font(&l.font, base, false)
}

// ── pies ───────────────────────────────────────────────────────────────────

/// Slice angles of a pie: `(start, sweep)` in degrees clockwise from
/// 3 o'clock, for each value (zero sweep for missing / non-positive ones).
pub fn pie_angles(vals: &[Option<f64>], first_angle: f32) -> Vec<(f32, f32)> {
    let total: f64 = vals.iter().flatten().filter(|v| **v > 0.0).sum();
    let mut at = first_angle - 90.0;
    vals.iter()
        .map(|v| {
            let v = v.filter(|v| *v > 0.0).unwrap_or(0.0);
            let sweep = if total > 0.0 {
                (v / total * 360.0) as f32
            } else {
                0.0
            };
            let s = (at, sweep);
            at += sweep;
            s
        })
        .collect()
}

fn pie(g: &Group, o: &mut Out, plot: Rect) {
    let Kind::Pie { first_angle, hole } = g.kind else {
        return;
    };
    let series: Vec<&Series> = match hole {
        Some(_) => g.series.iter().collect(),
        None => g.series.iter().take(1).collect(),
    };
    if series.is_empty() {
        return;
    }
    let outside = series.iter().any(|s| {
        (0..s.vals.len())
            .any(|i| series_labels(g, s, i).is_some_and(|l| l.pos.as_deref() == Some("outEnd")))
    });
    let max_expl = series
        .iter()
        .flat_map(|s| {
            std::iter::once(s.explosion).chain(s.points.iter().filter_map(|p| p.explosion))
        })
        .fold(0.0f32, f32::max)
        .min(400.0)
        / 100.0;
    let c = p(plot[0] + plot[2] / 2.0, plot[1] + plot[3] / 2.0);
    let mut r = plot[2].min(plot[3]) / 2.0 / (1.0 + max_expl);
    if outside {
        r *= 0.78;
    }
    let inner = hole.map_or(0.0, |h| r * h / 100.0);
    let ring = (r - inner) / series.len() as f32;
    let mut labels = Vec::new();
    for (k, s) in series.iter().enumerate() {
        let (r0, r1) = if hole.is_some() {
            (inner + ring * k as f32, inner + ring * (k + 1) as f32)
        } else {
            (0.0, r)
        };
        let total: f64 = s.vals.iter().flatten().filter(|v| **v > 0.0).sum();
        for (i, (start, sweep)) in pie_angles(&s.vals, first_angle).into_iter().enumerate() {
            if sweep <= 0.0 {
                continue;
            }
            let st = point_style(g, s, i, o.d);
            let expl = s.point(i).and_then(|p| p.explosion).unwrap_or(s.explosion) / 100.0;
            let mid = (start + sweep / 2.0).to_radians();
            let dir = p(mid.cos(), mid.sin());
            let cc = c.plus(dir.scale(expl * r));
            let path = slice(cc, r0, r1, start, sweep);
            o.boxed(
                path,
                [cc.x - r1, cc.y - r1, 2.0 * r1, 2.0 * r1],
                &st.fill,
                st.line,
            );
            if let Some(l) = series_labels(g, s, i) {
                let v = s.vals[i].unwrap_or(0.0);
                let pc = (total > 0.0).then(|| v / total);
                if let Some(text) = label_text(&l, s, i, v, pc) {
                    labels.push((l, text, cc, dir, r0, r1));
                }
            }
        }
    }
    for (l, text, cc, dir, r0, r1) in labels {
        let font = label_font(o, &l);
        match l.pos.as_deref() {
            Some("outEnd") => {
                let at = cc.plus(dir.scale(r1 + 4.0));
                let ax = if dir.x > 0.2 {
                    0.0
                } else if dir.x < -0.2 {
                    1.0
                } else {
                    0.5
                };
                let ay = if dir.y > 0.2 {
                    0.0
                } else if dir.y < -0.2 {
                    1.0
                } else {
                    0.5
                };
                o.label_at(&text, &font, moved(&l, at, o.size), ax, ay);
            }
            Some("inEnd") => {
                let at = cc.plus(dir.scale(r1 - (r1 - r0) * 0.2));
                o.label_at(&text, &font, moved(&l, at, o.size), 0.5, 0.5);
            }
            Some("ctr") if r0 == 0.0 => {
                o.label_at(
                    &text,
                    &font,
                    moved(&l, cc.plus(dir.scale(r1 * 0.5)), o.size),
                    0.5,
                    0.5,
                );
            }
            _ => {
                let rr = if r0 > 0.0 { (r0 + r1) / 2.0 } else { r1 * 0.65 };
                o.label_at(
                    &text,
                    &font,
                    moved(&l, cc.plus(dir.scale(rr)), o.size),
                    0.5,
                    0.5,
                );
            }
        }
    }
}

/// A pie slice (`r0` = 0) or a ring segment, angles in degrees clockwise
/// from 3 o'clock.
fn slice(c: P, r0: f32, r1: f32, start: f32, sweep: f32) -> Path {
    let at = |r: f32, deg: f32| {
        let a = deg.to_radians();
        p(c.x + r * a.cos(), c.y + r * a.sin())
    };
    let mut path = Path::default();
    if sweep >= 359.99 {
        path = Path::ellipse(c.x, c.y, r1, r1);
        if r0 > 0.0 {
            let mut inner = Path::default();
            inner.move_to(at(r0, 0.0));
            inner.arc_to(r0, r0, 0.0, -360.0);
            inner.close();
            path.segs.extend(inner.segs);
        }
        return path;
    }
    if r0 > 0.0 {
        path.move_to(at(r0, start));
        path.line_to(at(r1, start));
        path.arc_to(r1, r1, start, sweep);
        path.line_to(at(r0, start + sweep));
        path.arc_to(r0, r0, start + sweep, -sweep);
    } else {
        path.move_to(c);
        path.line_to(at(r1, start));
        path.arc_to(r1, r1, start, sweep);
    }
    path.close();
    path
}

// ── axes ───────────────────────────────────────────────────────────────────

/// An axis as drawn.
#[derive(Clone, Debug)]
struct Ax<'a> {
    ax: &'a Axis,
    /// Runs top ↔ bottom.
    vertical: bool,
    /// Values (else categories).
    value: Option<Scale>,
    /// Data range of a value axis, and whether it's percent stacked.
    lo: f64,
    hi: f64,
    percent: bool,
    /// Categories (category axes).
    ncat: usize,
    labels: Vec<String>,
    /// Categories sit between ticks (else on them).
    between: bool,
    /// Format of the tick labels.
    fmt: Option<String>,
}

impl Ax<'_> {
    /// Fraction 0..1 along the axis (0 = left / bottom) of value `v` or
    /// of category position `u` (0..ncat).
    fn frac(&self, v: f64) -> f32 {
        let f = match &self.value {
            Some(s) => s.frac(v),
            None => {
                let n = self.ncat.max(1) as f64;
                if self.between {
                    v / n
                } else if n > 1.0 {
                    (v - 0.5) / (n - 1.0)
                } else {
                    0.5
                }
            }
        } as f32;
        if self.ax.reversed { 1.0 - f } else { f }
    }

    /// Pixel position of `v` in the plot rect.
    fn px(&self, v: f64, plot: Rect) -> f32 {
        let f = self.frac(v);
        if self.vertical {
            plot[1] + plot[3] * (1.0 - f)
        } else {
            plot[0] + plot[2] * f
        }
    }

    fn len(&self, plot: Rect) -> f32 {
        if self.vertical { plot[3] } else { plot[2] }
    }

    /// Where this axis crosses the other one (fraction along `other`).
    fn cross_frac(&self, other: &Ax) -> f32 {
        let f = match (self.ax.crosses.as_deref(), self.ax.crosses_at) {
            (_, Some(v)) => other.frac(v),
            (Some("max"), _) => {
                if other.ax.reversed {
                    0.0
                } else {
                    1.0
                }
            }
            (Some("min"), _) => {
                if other.ax.reversed {
                    1.0
                } else {
                    0.0
                }
            }
            _ => match &other.value {
                Some(s) => other.frac(0f64.clamp(s.min, s.max)),
                None => {
                    if other.ax.reversed {
                        1.0
                    } else {
                        0.0
                    }
                }
            },
        };
        f.clamp(0.0, 1.0)
    }

    /// Tick label texts and the values they sit at.
    fn ticks(&self) -> Vec<(f64, String)> {
        match &self.value {
            Some(s) => s
                .ticks()
                .into_iter()
                .map(|v| (v, format_number(v, self.fmt.as_deref())))
                .collect(),
            None => (0..self.ncat)
                .map(|i| {
                    (
                        i as f64 + 0.5,
                        self.labels.get(i).cloned().unwrap_or_default(),
                    )
                })
                .collect(),
        }
    }
}

/// Which side of the plot an axis' labels are on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    L,
    R,
    T,
    B,
}

/// Value range of series stacked as `grouping` asks: per category sums of
/// positives and negatives, or fractions of the absolute total.
pub fn stacked_values(series: &[&[Option<f64>]], grouping: Grouping) -> Vec<Vec<(f64, f64)>> {
    let n = series.iter().map(|s| s.len()).max().unwrap_or(0);
    let mut pos = vec![0.0f64; n];
    let mut neg = vec![0.0f64; n];
    let totals: Vec<f64> = (0..n)
        .map(|i| {
            series
                .iter()
                .filter_map(|s| s.get(i).copied().flatten())
                .map(f64::abs)
                .sum()
        })
        .collect();
    series
        .iter()
        .map(|s| {
            (0..n)
                .map(|i| {
                    let mut v = s.get(i).copied().flatten().unwrap_or(0.0);
                    if grouping == Grouping::PercentStacked {
                        v = if totals[i] > 0.0 { v / totals[i] } else { 0.0 };
                    }
                    if !grouping.stacked() {
                        return (0.0, v);
                    }
                    let acc = if v >= 0.0 { &mut pos[i] } else { &mut neg[i] };
                    let base = *acc;
                    *acc += v;
                    (base, *acc)
                })
                .collect()
        })
        .collect()
}

fn group_grouping(g: &Group) -> Grouping {
    match g.kind {
        Kind::Bar { grouping, .. } | Kind::Line { grouping } | Kind::Area { grouping } => grouping,
        _ => Grouping::Standard,
    }
}

/// The (category / x, value / y) axes of a group.
fn group_axes<'a>(chart: &'a Chart, g: &Group) -> Option<(&'a Axis, &'a Axis)> {
    let a = chart.axis(*g.ax_ids.first()?)?;
    let b = chart.axis(*g.ax_ids.get(1)?)?;
    Some((a, b))
}

fn axes_chart(chart: &Chart, o: &mut Out, [w, h]: [f32; 2], area: Rect) {
    let base = o.base.size_pt.unwrap_or(18.0);
    // Axes in use, with their orientation and data.
    let mut axes: Vec<Ax> = Vec::new();
    fn find<'a>(axes: &mut Vec<Ax<'a>>, a: &'a Axis) -> usize {
        if let Some(i) = axes.iter().position(|x| x.ax.id == a.id) {
            return i;
        }
        axes.push(Ax {
            ax: a,
            vertical: matches!(a.pos, AxPos::L | AxPos::R),
            value: None,
            lo: f64::INFINITY,
            hi: f64::NEG_INFINITY,
            percent: false,
            ncat: 0,
            labels: Vec::new(),
            between: true,
            fmt: None,
        });
        axes.len() - 1
    }
    let mut plan: Vec<(&Group, usize, usize)> = Vec::new();
    for g in &chart.groups {
        let Some((ca, va)) = group_axes(chart, g) else {
            continue;
        };
        let ci = find(&mut axes, ca);
        let vi = find(&mut axes, va);
        if ci == vi {
            continue;
        }
        plan.push((g, ci, vi));
        let horizontal_bars = matches!(
            g.kind,
            Kind::Bar {
                horizontal: true,
                ..
            }
        );
        axes[ci].vertical = horizontal_bars;
        axes[vi].vertical = !horizontal_bars;
        let grouping = group_grouping(g);
        if let Kind::Scatter { .. } = g.kind {
            for s in &g.series {
                for (i, y) in s.vals.iter().enumerate() {
                    let x =
                        s.xs.as_ref()
                            .and_then(|xs| xs.get(i).copied().flatten())
                            .unwrap_or(i as f64 + 1.0);
                    if let Some(y) = y {
                        let (a, b) = (&mut axes[ci], x);
                        a.lo = a.lo.min(b);
                        a.hi = a.hi.max(b);
                        let v = &mut axes[vi];
                        v.lo = v.lo.min(*y);
                        v.hi = v.hi.max(*y);
                    }
                }
                if axes[ci].fmt.is_none() {
                    axes[ci].fmt = s.cat_fmt.clone();
                }
                if axes[vi].fmt.is_none() {
                    axes[vi].fmt = s.fmt.clone();
                }
            }
        } else {
            let v = &mut axes[vi];
            if grouping.stacked() {
                let vals: Vec<&[Option<f64>]> =
                    g.series.iter().map(|s| s.vals.as_slice()).collect();
                for (a, b) in stacked_values(&vals, grouping).into_iter().flatten() {
                    v.lo = v.lo.min(a.min(b));
                    v.hi = v.hi.max(a.max(b));
                }
            } else {
                // Unstacked: zero only counts through the scale's zero rule.
                for x in g.series.iter().flat_map(|s| s.vals.iter().flatten()) {
                    v.lo = v.lo.min(*x);
                    v.hi = v.hi.max(*x);
                }
            }
            if grouping == Grouping::PercentStacked {
                axes[vi].percent = true;
            }
            let c = &mut axes[ci];
            for s in &g.series {
                c.ncat = c
                    .ncat
                    .max(s.vals.len())
                    .max(s.cats.len().max(s.cat_nums.len()));
            }
            if c.labels.is_empty()
                && let Some(s) = g
                    .series
                    .iter()
                    .find(|s| !s.cats.is_empty() || !s.cat_nums.is_empty())
                    .or(g.series.first())
            {
                let fmt =
                    c.ax.num_fmt
                        .as_ref()
                        .filter(|(code, linked)| !linked || is_date_code(code))
                        .map(|f| f.0.clone());
                c.labels = (0..c.ncat)
                    .map(|i| cat_label(s, i, fmt.as_deref()))
                    .collect();
            }
            // Bars always sit between ticks; others as the value axis says.
            let between =
                matches!(g.kind, Kind::Bar { .. }) || va.cross_between.as_deref() != Some("midCat");
            axes[ci].between = between;
            if axes[vi].fmt.is_none() {
                axes[vi].fmt = g.series.iter().find_map(|s| s.fmt.clone());
            }
        }
    }
    let is_value: Vec<bool> = axes
        .iter()
        .map(|a| {
            a.ax.kind == AxisKind::Val
                && chart.groups.iter().any(|g| {
                    group_axes(chart, g).is_some_and(|(c, v)| {
                        v.id == a.ax.id
                            || (c.id == a.ax.id && matches!(g.kind, Kind::Scatter { .. }))
                    })
                })
        })
        .collect();
    if plan.is_empty() {
        return;
    }
    // Axis formats: own code unless linked to the source.
    for (a, value) in axes.iter_mut().zip(&is_value) {
        if !value {
            continue;
        }
        if let Some((code, linked)) = &a.ax.num_fmt
            && (!linked || a.fmt.is_none())
        {
            a.fmt = Some(code.clone());
        }
        if a.percent && a.fmt.as_deref().is_none_or(|f| !f.contains('%')) {
            a.fmt = Some("0%".into());
        }
    }
    let fonts: Vec<TextFont> = axes
        .iter()
        .map(|a| o.font(&a.ax.font, base, false))
        .collect();
    let manual = chart
        .plot_layout
        .and_then(|l| manual_rect(Some(l), w, h).map(|r| (r, l.inner)));
    let outer = match manual {
        Some((r, false)) => r,
        _ => area,
    };
    // Two passes: scales need the plot size, label room needs the scales.
    let mut plot = outer;
    let mut cat_rot: Vec<f32> = vec![0.0; axes.len()];
    let mut cat_wrap: Vec<Option<f32>> = vec![None; axes.len()];
    let mut cat_step: Vec<usize> = vec![1; axes.len()];
    for _ in 0..2 {
        for (i, a) in axes.iter_mut().enumerate() {
            if !is_value[i] {
                continue;
            }
            let lh = fonts[i].size_pt * PX_PER_PT * 1.2;
            let n = intervals_for(a.len(plot), if a.vertical { lh } else { lh * 3.0 });
            let fixed = Fixed {
                min: a.ax.min,
                max: a.ax.max,
                unit: a.ax.major_unit,
            };
            a.value = Some(if a.percent {
                let lo = if a.lo < -1e-9 { -1.0 } else { 0.0 };
                let hi = if a.hi > 1e-9 || lo == 0.0 { 1.0 } else { 0.0 };
                auto_scale(
                    lo,
                    hi,
                    Fixed {
                        min: a.ax.min.or(Some(lo)),
                        max: a.ax.max.or(Some(hi)),
                        unit: a.ax.major_unit,
                    },
                    n,
                )
            } else {
                auto_scale(a.lo, a.hi, fixed, n)
            });
        }
        if let Some((r, true)) = manual {
            plot = r;
            break;
        }
        // Room for labels and titles on each side.
        let mut room = [0.0f32; 4]; // l, r, t, b
        for (i, a) in axes.iter().enumerate() {
            if a.ax.delete {
                continue;
            }
            let side = side_of(a);
            let mut need = 0.0f32;
            if a.ax.tick_lbl_pos != "none" {
                let lh = fonts[i].size_pt * PX_PER_PT * 1.2;
                let ticks = a.ticks();
                if a.vertical {
                    need = ticks
                        .iter()
                        .map(|(_, t)| o.measure(t, &fonts[i], None).0)
                        .fold(0.0, f32::max)
                        + lh * 0.4;
                    // The end labels are centred on the plot's edges.
                    if a.value.is_some() {
                        room[2] = room[2].max(lh / 2.0);
                        room[3] = room[3].max(lh / 2.0);
                    }
                } else if a.value.is_some() {
                    need = lh * 1.2;
                    let half = |t: Option<&(f64, String)>, o: &mut Out| {
                        t.map_or(0.0, |(_, t)| o.measure(t, &fonts[i], None).0 / 2.0)
                    };
                    room[0] = room[0].max(half(ticks.first(), o));
                    room[1] = room[1].max(half(ticks.last(), o));
                } else {
                    let (rot, wrap, step, hgt) = cat_fit(o, a, &fonts[i], plot, &ticks);
                    cat_rot[i] = rot;
                    cat_wrap[i] = wrap;
                    cat_step[i] = step;
                    need = hgt + lh * 0.3;
                }
            }
            if let Some(t) = &a.ax.title {
                let tf = o.font(&t.font, base, true);
                let text = t.text.clone().map_or("Axis Title".into(), |l| l.join("\n"));
                let (_, th) = o.measure(&text, &tf, None);
                need += th + 4.0;
            }
            let k = match side {
                Side::L => 0,
                Side::R => 1,
                Side::T => 2,
                Side::B => 3,
            };
            room[k] = room[k].max(need);
        }
        plot = [
            outer[0] + room[0],
            outer[1] + room[2],
            (outer[2] - room[0] - room[1]).max(4.0),
            (outer[3] - room[2] - room[3]).max(4.0),
        ];
    }
    plot_back(chart, o, plot);
    // Gridlines.
    for a in axes.iter() {
        for (grid, minor) in [(&a.ax.minor_grid, true), (&a.ax.major_grid, false)] {
            let Some(look) = grid else {
                continue;
            };
            let Some(st) = stroke_of(look.line.as_ref(), Some((GRID, 0.75 * PX_PER_PT))) else {
                continue;
            };
            let at: Vec<f64> = match (&a.value, minor) {
                (Some(s), false) => s.ticks(),
                (Some(s), true) => {
                    let u = s.unit / 5.0;
                    let n = ((s.max - s.min) / u).round().clamp(0.0, 2000.0) as usize;
                    (0..=n).map(|i| s.min + i as f64 * u).collect()
                }
                (None, _) => {
                    if a.between {
                        (0..=a.ncat).map(|i| i as f64).collect()
                    } else {
                        (0..a.ncat).map(|i| i as f64 + 0.5).collect()
                    }
                }
            };
            for v in at {
                let x = a.px(v, plot);
                if a.vertical {
                    o.line(p(plot[0], x), p(plot[0] + plot[2], x), &st);
                } else {
                    o.line(p(x, plot[1]), p(x, plot[1] + plot[3]), &st);
                }
            }
        }
    }
    // Series: areas, bars, then lines and scatter on top.
    let order = |g: &Group| match g.kind {
        Kind::Area { .. } => 0,
        Kind::Bar { .. } => 1,
        _ => 2,
    };
    let mut groups = plan.clone();
    groups.sort_by_key(|(g, ..)| order(g));
    let mut labels: Vec<(String, TextFont, P, f32, f32)> = Vec::new();
    for (g, ci, vi) in groups {
        let (ca, va) = (&axes[ci], &axes[vi]);
        match g.kind {
            Kind::Bar {
                gap,
                overlap,
                grouping,
                ..
            } => bars(g, ca, va, plot, gap, overlap, grouping, o, &mut labels),
            Kind::Line { grouping } | Kind::Area { grouping } => {
                lines(g, ca, va, plot, grouping, o, &mut labels)
            }
            Kind::Scatter { .. } => scatter(g, ca, va, plot, o, &mut labels),
            Kind::Pie { .. } => {}
        }
    }
    // Axis lines, ticks and labels.
    for (i, a) in axes.iter().enumerate() {
        if a.ax.delete {
            continue;
        }
        let Some(other) = axes
            .iter()
            .find(|x| x.ax.id == a.ax.cross_ax && x.vertical != a.vertical)
        else {
            continue;
        };
        let cf = a.cross_frac(other);
        let line_at = if a.vertical {
            plot[0] + plot[2] * cf
        } else {
            plot[1] + plot[3] * (1.0 - cf)
        };
        let side = side_of(a);
        let label_at = match a.ax.tick_lbl_pos.as_str() {
            "low" => match side {
                Side::L | Side::R => plot[0],
                _ => plot[1] + plot[3],
            },
            "high" => match side {
                Side::L | Side::R => plot[0] + plot[2],
                _ => plot[1],
            },
            _ => line_at,
        };
        let auto_line = (!is_value[i]).then_some((AXIS, 0.75 * PX_PER_PT));
        let st = stroke_of(a.ax.look.line.as_ref(), auto_line);
        if let Some(st) = &st {
            if a.vertical {
                o.line(p(line_at, plot[1]), p(line_at, plot[1] + plot[3]), st);
            } else {
                o.line(p(plot[0], line_at), p(plot[0] + plot[2], line_at), st);
            }
            // Tick marks.
            let tick = 4.0;
            let (t0, t1) = match a.ax.major_tick.as_str() {
                "none" => (0.0, 0.0),
                "in" => (-tick, 0.0),
                "cross" => (-tick, tick),
                _ => (0.0, tick),
            };
            if t0 != t1 {
                let dir = match side {
                    Side::L | Side::T => -1.0,
                    _ => 1.0,
                };
                let at: Vec<f64> = match &a.value {
                    Some(s) => s.ticks(),
                    None if a.between => (0..=a.ncat).map(|i| i as f64).collect(),
                    None => (0..a.ncat).map(|i| i as f64 + 0.5).collect(),
                };
                for v in at {
                    let x = a.px(v, plot);
                    if a.vertical {
                        o.line(p(line_at + t0 * dir, x), p(line_at + t1 * dir, x), st);
                    } else {
                        o.line(p(x, line_at + t0 * dir), p(x, line_at + t1 * dir), st);
                    }
                }
            }
        }
        let font = &fonts[i];
        let lh = font.size_pt * PX_PER_PT * 1.2;
        let gap = lh * 0.3;
        let mut extent = 0.0f32;
        if a.ax.tick_lbl_pos != "none" {
            let ticks = a.ticks();
            for (k, (v, text)) in ticks.iter().enumerate() {
                if k % cat_step[i].max(1) != 0 {
                    continue;
                }
                let x = a.px(*v, plot);
                if a.vertical {
                    let (ax, lx) = match side {
                        Side::R => (0.0, label_at + gap),
                        _ => (1.0, label_at - gap),
                    };
                    let r = o.label_at(text, font, p(lx, x), ax, 0.5);
                    extent = extent.max(r[2] + gap);
                } else {
                    let below = side != Side::T;
                    let y0 = if below {
                        label_at + gap
                    } else {
                        label_at - gap
                    };
                    let rot = cat_rot[i];
                    if rot != 0.0 {
                        let (tw, th) = o.measure(text, font, None);
                        let ang = rot.to_radians();
                        // The label's end (right for negative angles) at the tick.
                        let half = p(tw / 2.0 * ang.cos(), tw / 2.0 * ang.sin());
                        let drop = th / 2.0 * ang.cos().abs();
                        let c = p(x - half.x, y0 + drop - half.y);
                        let c = if below { c } else { p(c.x, y0 - (c.y - y0)) };
                        o.text(Label {
                            text: text.clone(),
                            font: font.clone(),
                            x: c.x - tw / 2.0,
                            y: c.y - th / 2.0,
                            w: tw,
                            h: th,
                            align: HAlign::Center,
                            rot,
                        });
                        extent = extent.max(tw * ang.sin().abs() + th * ang.cos().abs() + gap);
                    } else if let Some(wrap) = cat_wrap[i] {
                        let (tw, th) = o.measure(text, font, Some(wrap));
                        let y = if below { y0 } else { y0 - th };
                        o.text(Label {
                            text: text.clone(),
                            font: font.clone(),
                            x: x - tw / 2.0,
                            y,
                            w: tw,
                            h: th,
                            align: HAlign::Center,
                            rot: 0.0,
                        });
                        extent = extent.max(th + gap);
                    } else {
                        let r =
                            o.label_at(text, font, p(x, y0), 0.5, if below { 0.0 } else { 1.0 });
                        extent = extent.max(r[3] + gap);
                    }
                }
            }
        }
        if let Some(t) = &a.ax.title {
            let tf = o.font(&t.font, base, true);
            let text = t.text.clone().map_or("Axis Title".into(), |l| l.join("\n"));
            let (tw, th) = o.measure(&text, &tf, None);
            match side {
                Side::L | Side::R => {
                    let cy = plot[1] + plot[3] / 2.0;
                    let cx = if side == Side::L {
                        label_at - extent - 2.0 - th / 2.0
                    } else {
                        label_at + extent + 2.0 + th / 2.0
                    };
                    o.text(Label {
                        text,
                        font: tf,
                        x: cx - tw / 2.0,
                        y: cy - th / 2.0,
                        w: tw,
                        h: th,
                        align: HAlign::Center,
                        rot: if side == Side::L { -90.0 } else { 90.0 },
                    });
                }
                _ => {
                    let cx = plot[0] + plot[2] / 2.0;
                    let y = if side == Side::B {
                        label_at + extent + 2.0
                    } else {
                        label_at - extent - 2.0 - th
                    };
                    o.label_at(&text, &tf, p(cx, y), 0.5, 0.0);
                }
            }
        }
    }
    for (text, font, at, ax, ay) in labels {
        o.label_at(&text, &font, at, ax, ay);
    }
}

fn side_of(a: &Ax) -> Side {
    let crosses_max = a.ax.crosses.as_deref() == Some("max");
    match (a.vertical, a.ax.pos, crosses_max) {
        (true, AxPos::R, _) | (true, _, true) => Side::R,
        (true, _, _) => Side::L,
        (false, AxPos::T, _) | (false, _, true) => Side::T,
        (false, _, _) => Side::B,
    }
}

/// How a horizontal category axis' labels fit: rotation, wrap width,
/// label step and the height they take.
fn cat_fit(
    o: &mut Out,
    a: &Ax,
    font: &TextFont,
    plot: Rect,
    ticks: &[(f64, String)],
) -> (f32, Option<f32>, usize, f32) {
    let n = ticks.len().max(1);
    let band = plot[2] / n as f32;
    let lh = font.size_pt * PX_PER_PT * 1.2;
    let sizes: Vec<(f32, f32)> = ticks
        .iter()
        .map(|(_, t)| o.measure(t, font, None))
        .collect();
    let widest = sizes.iter().map(|s| s.0).fold(0.0, f32::max);
    let tallest = sizes.iter().map(|s| s.1).fold(lh, f32::max);
    let rotated = |rot: f32| {
        let ang = rot.to_radians();
        let step = ((lh / ang.sin().abs().max(0.2)) / band).ceil().max(1.0) as usize;
        let hgt = widest * ang.sin().abs() + tallest * ang.cos().abs();
        (rot, None, step, hgt)
    };
    if let Some(rot) = a.ax.rot.filter(|r| r.abs() > 0.5) {
        return rotated(rot);
    }
    if widest <= band * 0.95 {
        return (0.0, None, 1, tallest);
    }
    // Wrap at the band when every word fits and it stays short.
    let words_fit = ticks.iter().all(|(_, t)| {
        t.split_whitespace()
            .all(|w| o.measure(w, font, None).0 <= band * 0.95)
    });
    if words_fit && a.ax.rot.is_none() {
        let wrap = (band * 0.95).max(1.0);
        let hgt = ticks
            .iter()
            .map(|(_, t)| o.measure(t, font, Some(wrap)).1)
            .fold(0.0, f32::max);
        if hgt <= lh * 3.2 {
            return (0.0, Some(wrap), 1, hgt);
        }
    }
    if a.ax.rot.is_some() {
        // Explicitly upright: thin the labels out instead.
        let step = (widest / band).ceil().max(1.0) as usize;
        return (0.0, None, step, tallest);
    }
    rotated(-45.0)
}

#[allow(clippy::too_many_arguments)]
fn bars(
    g: &Group,
    ca: &Ax,
    va: &Ax,
    plot: Rect,
    gap: f32,
    overlap: f32,
    grouping: Grouping,
    o: &mut Out,
    labels: &mut Vec<(String, TextFont, P, f32, f32)>,
) {
    let n = g.series.len().max(1) as f32;
    let (ov, gp) = (overlap / 100.0, gap / 100.0);
    let slots = if grouping.stacked() { 1.0 } else { n };
    let bw = 1.0 / (slots - (slots - 1.0) * ov + gp);
    let vals: Vec<&[Option<f64>]> = g.series.iter().map(|s| s.vals.as_slice()).collect();
    let stacks = stacked_values(&vals, grouping);
    let s_scale = va.value.unwrap_or(Scale {
        min: 0.0,
        max: 1.0,
        unit: 1.0,
    });
    let clamp = |v: f64| v.clamp(s_scale.min, s_scale.max);
    // The bars' base: zero, or the axis edge nearest to it.
    let base_v = clamp(0.0);
    for (k, s) in g.series.iter().enumerate() {
        let slot = if grouping.stacked() { 0.0 } else { k as f32 };
        for (i, v) in s.vals.iter().enumerate() {
            let Some(v) = v else { continue };
            let (b0, b1) = stacks[k][i];
            let (b0, b1) = if grouping.stacked() {
                (b0, b1)
            } else {
                (base_v, b1)
            };
            let u0 = i as f32 + gp * bw / 2.0 + slot * bw * (1.0 - ov);
            let u1 = u0 + bw;
            let (c0, c1) = (ca.px(u0 as f64, plot), ca.px(u1 as f64, plot));
            let (v0, v1) = (va.px(clamp(b0), plot), va.px(clamp(b1), plot));
            let rect = if ca.vertical {
                [v0.min(v1), c0.min(c1), (v1 - v0).abs(), (c1 - c0).abs()]
            } else {
                [c0.min(c1), v0.min(v1), (c1 - c0).abs(), (v1 - v0).abs()]
            };
            let st = point_style(g, s, i, o.d);
            o.boxed(
                Path::rect(rect[0], rect[1], rect[2], rect[3]),
                rect,
                &st.fill,
                st.line,
            );
            if let Some(l) = series_labels(g, s, i)
                && let Some(text) = label_text(&l, s, i, *v, None)
            {
                let font = label_font(o, &l);
                let pos =
                    l.pos
                        .as_deref()
                        .unwrap_or(if grouping.stacked() { "ctr" } else { "outEnd" });
                // Along the value direction: from the base (v0) to the end (v1).
                let cm = (c0 + c1) / 2.0;
                let dir = if v1 >= v0 { 1.0 } else { -1.0 };
                let pad = 3.0;
                let (along, anchor) = match pos {
                    "inEnd" => (v1 - dir * pad, -1.0),
                    "inBase" => (v0 + dir * pad, 1.0),
                    "ctr" => ((v0 + v1) / 2.0, 0.0),
                    _ => (v1 + dir * pad, 1.0),
                };
                // anchor: 1 = label extends in +dir from `along`, -1 = in -dir.
                let k = anchor * dir;
                let (pt, ax, ay) = if ca.vertical {
                    (
                        p(along, cm),
                        if k > 0.0 {
                            0.0
                        } else if k < 0.0 {
                            1.0
                        } else {
                            0.5
                        },
                        0.5,
                    )
                } else {
                    (
                        p(cm, along),
                        0.5,
                        if k > 0.0 {
                            0.0
                        } else if k < 0.0 {
                            1.0
                        } else {
                            0.5
                        },
                    )
                };
                labels.push((text, font, moved(&l, pt, o.size), ax, ay));
            }
        }
    }
}

/// Smooth curve through points (Catmull-Rom as cubic Béziers).
fn polyline(pts: &[P], smooth: bool) -> Path {
    let mut path = Path::default();
    let Some(first) = pts.first() else {
        return path;
    };
    path.move_to(*first);
    if !smooth || pts.len() < 3 {
        for q in &pts[1..] {
            path.line_to(*q);
        }
        return path;
    }
    for i in 0..pts.len() - 1 {
        let p0 = if i == 0 { pts[0] } else { pts[i - 1] };
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts.get(i + 2).copied().unwrap_or(p2);
        let c1 = p1.plus(p2.minus(p0).scale(1.0 / 6.0));
        let c2 = p2.minus(p3.minus(p1).scale(1.0 / 6.0));
        path.cubic_to(c1, c2, p2);
    }
    path
}

fn lines(
    g: &Group,
    ca: &Ax,
    va: &Ax,
    plot: Rect,
    grouping: Grouping,
    o: &mut Out,
    labels: &mut Vec<(String, TextFont, P, f32, f32)>,
) {
    let area = matches!(g.kind, Kind::Area { .. });
    let vals: Vec<&[Option<f64>]> = g.series.iter().map(|s| s.vals.as_slice()).collect();
    let stacks = stacked_values(&vals, grouping);
    let sc = va.value.unwrap_or(Scale {
        min: 0.0,
        max: 1.0,
        unit: 1.0,
    });
    let clamp = |v: f64| v.clamp(sc.min, sc.max);
    let at = |i: usize, v: f64| {
        let c = ca.px(i as f64 + 0.5, plot);
        let y = va.px(clamp(v), plot);
        if ca.vertical { p(y, c) } else { p(c, y) }
    };
    let mut markers = Vec::new();
    for (k, s) in g.series.iter().enumerate() {
        let auto = auto_color(o.d, s.idx);
        let n = s.vals.len();
        if area {
            // Filled down to the previous series (stacked) or the base.
            let top: Vec<P> = (0..n).map(|i| at(i, stacks[k][i].1)).collect();
            let bottom: Vec<P> = (0..n)
                .rev()
                .map(|i| {
                    at(
                        i,
                        if grouping.stacked() {
                            stacks[k][i].0
                        } else {
                            0.0
                        },
                    )
                })
                .collect();
            if top.len() >= 2 {
                let mut path = polyline(&top, false);
                for q in &bottom {
                    path.line_to(*q);
                }
                path.close();
                let st = series_style(g, s, o.d);
                let b = path.bounds().unwrap_or(plot);
                let rect = [b[0], b[1], b[2] - b[0], b[3] - b[1]];
                o.boxed(path, rect, &st.fill, st.line);
            }
        } else if let Some(stroke) = series_line(g, s, auto) {
            // Gaps split the line.
            let mut run: Vec<P> = Vec::new();
            let ends = s.vals.iter().zip(&stacks[k]).enumerate();
            for (i, (v, st)) in ends
                .map(Some)
                .chain(std::iter::once(None))
                .map(|e| match e {
                    Some((i, (v, st))) => (i, (*v, st.1)),
                    None => (n, (None, 0.0)),
                })
            {
                match v {
                    Some(_) => run.push(at(i, st)),
                    None => {
                        if run.len() >= 2 {
                            o.stroke(polyline(&run, s.smooth), stroke.clone());
                        }
                        run.clear();
                    }
                }
            }
        }
        for (i, v) in s.vals.iter().enumerate() {
            let Some(v) = v else { continue };
            let q = at(i, stacks[k][i].1);
            if !area && let Some(m) = marker_style(g, s, Some(i), o.d) {
                markers.push((m, q));
            }
            if let Some(l) = series_labels(g, s, i)
                && let Some(text) = label_text(&l, s, i, *v, None)
            {
                let font = label_font(o, &l);
                let off = 5.0;
                let (pt, ax, ay) = match l.pos.as_deref() {
                    _ if area => (q, 0.5, 0.5),
                    Some("t") => (p(q.x, q.y - off), 0.5, 1.0),
                    Some("b") => (p(q.x, q.y + off), 0.5, 0.0),
                    Some("l") => (p(q.x - off, q.y), 1.0, 0.5),
                    Some("ctr") => (q, 0.5, 0.5),
                    _ => (p(q.x + off, q.y), 0.0, 0.5),
                };
                labels.push((text, font, moved(&l, pt, o.size), ax, ay));
            }
        }
    }
    for ((sym, size, fill, line), q) in markers {
        marker(o, sym, q, size, &fill, line);
    }
}

fn scatter(
    g: &Group,
    xa: &Ax,
    ya: &Ax,
    plot: Rect,
    o: &mut Out,
    labels: &mut Vec<(String, TextFont, P, f32, f32)>,
) {
    let (sx, sy) = match (xa.value, ya.value) {
        (Some(a), Some(b)) => (a, b),
        _ => return,
    };
    let smooth_style = matches!(&g.kind, Kind::Scatter { style } if style.starts_with("smooth"));
    let mut markers = Vec::new();
    for s in &g.series {
        let auto = auto_color(o.d, s.idx);
        let pts: Vec<Option<P>> = s
            .vals
            .iter()
            .enumerate()
            .map(|(i, y)| {
                let x =
                    s.xs.as_ref()
                        .and_then(|xs| xs.get(i).copied().flatten())
                        .unwrap_or(i as f64 + 1.0);
                y.map(|y| {
                    let (x, y) = (x.clamp(sx.min, sx.max), y.clamp(sy.min, sy.max));
                    p(xa.px(x, plot), ya.px(y, plot))
                })
            })
            .collect();
        if let Some(stroke) = series_line(g, s, auto) {
            let mut run = Vec::new();
            for q in pts.iter().chain(std::iter::once(&None)) {
                match q {
                    Some(q) => run.push(*q),
                    None => {
                        if run.len() >= 2 {
                            o.stroke(polyline(&run, s.smooth || smooth_style), stroke.clone());
                        }
                        run.clear();
                    }
                }
            }
        }
        for (i, q) in pts.iter().enumerate() {
            let Some(q) = q else { continue };
            if let Some(m) = marker_style(g, s, Some(i), o.d) {
                markers.push((m, *q));
            }
            if let Some(l) = series_labels(g, s, i)
                && let Some(text) = label_text(&l, s, i, s.vals[i].unwrap_or(0.0), None)
            {
                let font = label_font(o, &l);
                let off = 5.0;
                let (pt, ax, ay) = match l.pos.as_deref() {
                    Some("t") => (p(q.x, q.y - off), 0.5, 1.0),
                    Some("b") => (p(q.x, q.y + off), 0.5, 0.0),
                    Some("l") => (p(q.x - off, q.y), 1.0, 0.5),
                    Some("ctr") => (*q, 0.5, 0.5),
                    _ => (p(q.x + off, q.y), 0.0, 0.5),
                };
                labels.push((text, font, moved(&l, pt, o.size), ax, ay));
            }
        }
    }
    for ((sym, size, fill, line), q) in markers {
        marker(o, sym, q, size, &fill, line);
    }
}
