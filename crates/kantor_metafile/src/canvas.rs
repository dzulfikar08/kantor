//! What the metafile players draw into: device-space paths, pictures and
//! text, written out as one SVG document.

use std::fmt::Write as _;

pub(crate) type Pt = (f64, f64);

/// An affine map `p' = (a·x + c·y + e, b·x + d·y + f)` (GDI's XFORM and
/// SVG's `matrix(a b c d e f)` order).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Xf(pub [f64; 6]);

impl Xf {
    pub const ID: Xf = Xf([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn apply(&self, (x, y): Pt) -> Pt {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    /// `self` first, then `then`.
    pub fn then(&self, then: &Xf) -> Xf {
        let [a, b, c, d, e, f] = self.0;
        let [a2, b2, c2, d2, e2, f2] = then.0;
        Xf([
            a * a2 + b * c2,
            a * b2 + b * d2,
            c * a2 + d * c2,
            c * b2 + d * d2,
            e * a2 + f * c2 + e2,
            e * b2 + f * d2 + f2,
        ])
    }

    pub fn det(&self) -> f64 {
        self.0[0] * self.0[3] - self.0[1] * self.0[2]
    }

    /// How long a unit length becomes (the geometric mean of the axes).
    pub fn scale(&self) -> f64 {
        self.det().abs().sqrt()
    }

    pub fn x_len(&self) -> f64 {
        self.0[0].hypot(self.0[1])
    }

    pub fn y_len(&self) -> f64 {
        self.0[2].hypot(self.0[3])
    }

    pub fn translate(x: f64, y: f64) -> Xf {
        Xf([1.0, 0.0, 0.0, 1.0, x, y])
    }

    pub fn scaling(x: f64, y: f64) -> Xf {
        Xf([x, 0.0, 0.0, y, 0.0, 0.0])
    }

    pub fn is_finite(&self) -> bool {
        self.0.iter().all(|v| v.is_finite())
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Seg {
    M(Pt),
    L(Pt),
    C(Pt, Pt, Pt),
    Z,
}

/// A path in device space.
#[derive(Clone, Debug, Default)]
pub(crate) struct Path {
    pub segs: Vec<Seg>,
}

impl Path {
    pub fn move_to(&mut self, p: Pt) {
        self.segs.push(Seg::M(p));
    }

    pub fn line_to(&mut self, p: Pt) {
        if self.segs.is_empty() {
            self.segs.push(Seg::M(p));
        } else {
            self.segs.push(Seg::L(p));
        }
    }

    pub fn cubic(&mut self, a: Pt, b: Pt, c: Pt) {
        if self.segs.is_empty() {
            self.segs.push(Seg::M(a));
        }
        self.segs.push(Seg::C(a, b, c));
    }

    pub fn close(&mut self) {
        if !self.segs.is_empty() && !matches!(self.segs.last(), Some(Seg::Z)) {
            self.segs.push(Seg::Z);
        }
    }

    pub fn append(&mut self, other: &Path) {
        self.segs.extend_from_slice(&other.segs);
    }

    /// Points through `m`, as a closed or open polyline.
    pub fn poly(points: &[Pt], m: &Xf, closed: bool) -> Path {
        let mut p = Path::default();
        for (i, &pt) in points.iter().enumerate() {
            let q = m.apply(pt);
            if i == 0 {
                p.move_to(q);
            } else {
                p.line_to(q);
            }
        }
        if closed && !points.is_empty() {
            p.close();
        }
        p
    }

    /// GDI's poly-Bézier: a start point then three points per curve.
    pub fn bezier(points: &[Pt], m: &Xf, start: Option<Pt>) -> Path {
        let mut p = Path::default();
        let mut rest = points;
        match start {
            Some(s) => p.move_to(m.apply(s)),
            None => {
                let Some(&first) = points.first() else {
                    return p;
                };
                p.move_to(m.apply(first));
                rest = &points[1..];
            }
        }
        for c in rest.as_chunks::<3>().0 {
            p.cubic(m.apply(c[0]), m.apply(c[1]), m.apply(c[2]));
        }
        p
    }

    pub fn rect((l, t, r, b): (f64, f64, f64, f64), m: &Xf) -> Path {
        Path::poly(&[(l, t), (r, t), (r, b), (l, b)], m, true)
    }

    /// The ellipse inside the logical box.
    pub fn ellipse((l, t, r, b): (f64, f64, f64, f64), m: &Xf) -> Path {
        let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
        let (rx, ry) = ((r - l).abs() / 2.0, (b - t).abs() / 2.0);
        let mut p = Path::default();
        arc_points(
            &mut p,
            (cx, cy),
            (rx, ry),
            0.0,
            std::f64::consts::TAU,
            m,
            true,
        );
        p.close();
        p
    }

    pub fn round_rect((l, t, r, b): (f64, f64, f64, f64), (w, h): Pt, m: &Xf) -> Path {
        let (l, r) = (l.min(r), l.max(r));
        let (t, b) = (t.min(b), t.max(b));
        let rx = (w.abs() / 2.0).min((r - l) / 2.0);
        let ry = (h.abs() / 2.0).min((b - t) / 2.0);
        if rx <= 0.0 || ry <= 0.0 {
            return Path::rect((l, t, r, b), m);
        }
        use std::f64::consts::{FRAC_PI_2, PI};
        let mut p = Path::default();
        p.move_to(m.apply((l + rx, t)));
        p.line_to(m.apply((r - rx, t)));
        arc_points(
            &mut p,
            (r - rx, t + ry),
            (rx, ry),
            -FRAC_PI_2,
            FRAC_PI_2,
            m,
            false,
        );
        p.line_to(m.apply((r, b - ry)));
        arc_points(&mut p, (r - rx, b - ry), (rx, ry), 0.0, FRAC_PI_2, m, false);
        p.line_to(m.apply((l + rx, b)));
        arc_points(
            &mut p,
            (l + rx, b - ry),
            (rx, ry),
            FRAC_PI_2,
            FRAC_PI_2,
            m,
            false,
        );
        p.line_to(m.apply((l, t + ry)));
        arc_points(&mut p, (l + rx, t + ry), (rx, ry), PI, FRAC_PI_2, m, false);
        p.close();
        p
    }
}

/// An elliptical arc from parameter angle `start` turning `sweep` radians,
/// as cubic Béziers through `m`; `move_first` starts a new figure, else the
/// arc's start is joined with a line.
pub(crate) fn arc_points(
    p: &mut Path,
    (cx, cy): Pt,
    (rx, ry): Pt,
    start: f64,
    sweep: f64,
    m: &Xf,
    move_first: bool,
) {
    let at = |a: f64| (cx + rx * a.cos(), cy + ry * a.sin());
    let first = m.apply(at(start));
    if move_first {
        p.move_to(first);
    } else {
        p.line_to(first);
    }
    if sweep == 0.0 || !sweep.is_finite() {
        return;
    }
    let n = (sweep.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = sweep / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let mut a = start;
    for _ in 0..n {
        let b = a + step;
        let (p0, p3) = (at(a), at(b));
        let d0 = (-rx * a.sin(), ry * a.cos());
        let d3 = (-rx * b.sin(), ry * b.cos());
        let c1 = (p0.0 + k * d0.0, p0.1 + k * d0.1);
        let c2 = (p3.0 - k * d3.0, p3.1 - k * d3.1);
        p.segs.push(Seg::C(m.apply(c1), m.apply(c2), m.apply(p3)));
        a = b;
    }
}

/// GDI's arc: the ellipse in the box, from the ray through `s` to the ray
/// through `e`, counterclockwise as seen on the device unless `clockwise`.
/// Returns the arc and its centre (logical).
pub(crate) fn gdi_arc(
    p: &mut Path,
    (l, t, r, b): (f64, f64, f64, f64),
    s: Pt,
    e: Pt,
    clockwise: bool,
    m: &Xf,
    move_first: bool,
) -> Pt {
    let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
    let (rx, ry) = ((r - l).abs() / 2.0, (b - t).abs() / 2.0);
    let ang = |(x, y): Pt| {
        if rx == 0.0 || ry == 0.0 {
            0.0
        } else {
            ((y - cy) / ry).atan2((x - cx) / rx)
        }
    };
    let (a0, a1) = (ang(s), ang(e));
    // Visually counterclockwise on a y-down device is decreasing angle; a
    // map that flips one axis turns that around.
    let mut negative = !clockwise;
    if m.det() < 0.0 {
        negative = !negative;
    }
    let tau = std::f64::consts::TAU;
    let mut sweep = a1 - a0;
    if negative {
        while sweep >= 0.0 {
            sweep -= tau;
        }
        while sweep < -tau {
            sweep += tau;
        }
    } else {
        while sweep <= 0.0 {
            sweep += tau;
        }
        while sweep > tau {
            sweep -= tau;
        }
    }
    arc_points(p, (cx, cy), (rx, ry), a0, sweep, m, move_first);
    (cx, cy)
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Paint {
    Color([u8; 3], f64),
    /// GDI hatch style 0–5, line colour, background (opaque mode) or none.
    Hatch(u32, [u8; 3], Option<[u8; 3]>),
    /// A PNG tile, its size in device units.
    Tile(std::sync::Arc<Vec<u8>>, f64, f64),
    /// From `p0` to `p1` (device).
    Linear(Pt, Pt, [u8; 4], [u8; 4]),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Stroke {
    pub color: [u8; 3],
    pub alpha: f64,
    /// Device units.
    pub width: f64,
    /// Dash lengths in device units.
    pub dash: Vec<f64>,
    pub cap: &'static str,
    pub join: &'static str,
    pub miter: f64,
}

pub(crate) struct Text<'a> {
    /// Device position of the first character's baseline origin (after
    /// alignment), before turning.
    pub pos: Pt,
    pub xs: Option<Vec<f64>>,
    pub anchor: &'static str,
    /// Degrees, clockwise on the device, about `pivot`.
    pub rotate: f64,
    pub pivot: Pt,
    pub size: f64,
    pub family: &'a str,
    pub weight: i32,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: [u8; 3],
    pub alpha: f64,
    pub text: &'a str,
    /// Horizontal stretch (font width / normal width).
    pub stretch: f64,
}

/// The SVG being written. Coordinates are the metafile's device units.
pub(crate) struct Svg {
    body: String,
    defs: String,
    next: usize,
    /// The clip path elements are drawn inside (an id in `defs`).
    pub clip: Option<String>,
    /// Drawing operations written (paths, pictures, text).
    pub drawn: usize,
    pub has_text: bool,
    /// Device units in one pixel at 96 dpi (for hairlines and hatches).
    pub unit: f64,
}

fn rgb(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn num(out: &mut String, v: f64) {
    let v = if v.is_finite() { v } else { 0.0 };
    let r = (v * 1000.0).round() / 1000.0;
    if r == r.trunc() && r.abs() < 1e15 {
        let _ = write!(out, "{}", r as i64);
    } else {
        let _ = write!(out, "{r}");
    }
}

pub(crate) fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            // Control characters aren't allowed in XML.
            c if (c as u32) < 0x20 && c != '\t' => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

pub(crate) fn data_uri(bytes: &[u8]) -> Option<String> {
    use base64::Engine;
    let mime = crate::raster_mime(bytes)?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

impl Svg {
    pub fn new(unit: f64) -> Self {
        Svg {
            body: String::new(),
            defs: String::new(),
            next: 0,
            clip: None,
            drawn: 0,
            has_text: false,
            unit: if unit.is_finite() && unit > 0.0 {
                unit
            } else {
                1.0
            },
        }
    }

    fn id(&mut self, prefix: &str) -> String {
        self.next += 1;
        format!("{prefix}{}", self.next)
    }

    fn d(p: &Path) -> String {
        let mut d = String::new();
        for s in &p.segs {
            match *s {
                Seg::M((x, y)) => {
                    d.push('M');
                    num(&mut d, x);
                    d.push(' ');
                    num(&mut d, y);
                }
                Seg::L((x, y)) => {
                    d.push('L');
                    num(&mut d, x);
                    d.push(' ');
                    num(&mut d, y);
                }
                Seg::C(a, b, c) => {
                    d.push('C');
                    for (i, (x, y)) in [a, b, c].into_iter().enumerate() {
                        if i > 0 {
                            d.push(' ');
                        }
                        num(&mut d, x);
                        d.push(' ');
                        num(&mut d, y);
                    }
                }
                Seg::Z => d.push('Z'),
            }
        }
        d
    }

    fn clip_attr(&self) -> String {
        match &self.clip {
            Some(id) => format!(" clip-path=\"url(#{id})\""),
            None => String::new(),
        }
    }

    /// A clip path made of `path`, inside the current clip when
    /// `intersect`. Returns its id (to set as [`Svg::clip`]).
    pub fn clip_path(&mut self, path: &Path, evenodd: bool, intersect: bool) -> String {
        let id = self.id("c");
        let inner = if intersect {
            self.clip_attr()
        } else {
            String::new()
        };
        let rule = if evenodd { "evenodd" } else { "nonzero" };
        let _ = write!(
            self.defs,
            "<clipPath id=\"{id}\" clipPathUnits=\"userSpaceOnUse\"{inner}><path d=\"{}\" clip-rule=\"{rule}\"/></clipPath>",
            Self::d(path)
        );
        id
    }

    fn paint(&mut self, p: &Paint) -> String {
        match p {
            Paint::Color(c, a) => {
                if *a < 0.999 {
                    format!("{}\" fill-opacity=\"{:.3}", rgb(*c), a.clamp(0.0, 1.0))
                } else {
                    rgb(*c)
                }
            }
            Paint::Hatch(style, c, bg) => {
                let id = self.id("h");
                let s = 8.0 * self.unit;
                let w = self.unit;
                let col = rgb(*c);
                let line = |x1: f64, y1: f64, x2: f64, y2: f64| {
                    format!(
                        "<line x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\" stroke=\"{col}\" stroke-width=\"{w}\"/>"
                    )
                };
                let h = s / 2.0;
                let lines = match style {
                    0 => line(0.0, h, s, h),
                    1 => line(h, 0.0, h, s),
                    2 => format!("{}{}", line(0.0, 0.0, s, s), line(-s, 0.0, 0.0, s)),
                    3 => format!("{}{}", line(0.0, s, s, 0.0), line(s, s, 2.0 * s, 0.0)),
                    4 => format!("{}{}", line(0.0, h, s, h), line(h, 0.0, h, s)),
                    _ => format!("{}{}", line(0.0, 0.0, s, s), line(0.0, s, s, 0.0)),
                };
                let back = bg
                    .map(|b| format!("<rect width=\"{s}\" height=\"{s}\" fill=\"{}\"/>", rgb(b)))
                    .unwrap_or_default();
                let _ = write!(
                    self.defs,
                    "<pattern id=\"{id}\" patternUnits=\"userSpaceOnUse\" width=\"{s}\" height=\"{s}\">{back}{lines}</pattern>"
                );
                format!("url(#{id})")
            }
            Paint::Tile(png, w, h) => {
                let Some(uri) = data_uri(png) else {
                    return "#808080".into();
                };
                let id = self.id("t");
                let _ = write!(
                    self.defs,
                    "<pattern id=\"{id}\" patternUnits=\"userSpaceOnUse\" width=\"{w}\" height=\"{h}\"><image width=\"{w}\" height=\"{h}\" preserveAspectRatio=\"none\" href=\"{uri}\"/></pattern>"
                );
                format!("url(#{id})")
            }
            Paint::Linear(a, b, c0, c1) => {
                let id = self.id("g");
                let stop = |o: f64, c: &[u8; 4]| {
                    format!(
                        "<stop offset=\"{o}\" stop-color=\"{}\" stop-opacity=\"{:.3}\"/>",
                        rgb([c[0], c[1], c[2]]),
                        c[3] as f64 / 255.0
                    )
                };
                let _ = write!(
                    self.defs,
                    "<linearGradient id=\"{id}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">{}{}</linearGradient>",
                    a.0,
                    a.1,
                    b.0,
                    b.1,
                    stop(0.0, c0),
                    stop(1.0, c1)
                );
                format!("url(#{id})")
            }
        }
    }

    pub fn path(&mut self, p: &Path, fill: Option<&Paint>, evenodd: bool, stroke: Option<&Stroke>) {
        if p.segs.is_empty() || (fill.is_none() && stroke.is_none()) {
            return;
        }
        let fill = match fill {
            Some(f) => self.paint(f),
            None => "none".into(),
        };
        let mut s = format!("<path d=\"{}\" fill=\"{fill}\"", Self::d(p));
        if evenodd {
            s.push_str(" fill-rule=\"evenodd\"");
        }
        if let Some(st) = stroke {
            let w = if st.width > 0.0 { st.width } else { self.unit };
            let _ = write!(
                s,
                " stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"{}\" stroke-linejoin=\"{}\"",
                rgb(st.color),
                (w * 1000.0).round() / 1000.0,
                st.cap,
                st.join
            );
            if st.join == "miter" && st.miter > 0.0 {
                let _ = write!(s, " stroke-miterlimit=\"{}\"", st.miter.max(1.0));
            }
            if st.alpha < 0.999 {
                let _ = write!(s, " stroke-opacity=\"{:.3}\"", st.alpha.clamp(0.0, 1.0));
            }
            if !st.dash.is_empty() {
                let parts: Vec<String> = st
                    .dash
                    .iter()
                    .map(|d| format!("{}", (d.max(0.0) * 1000.0).round() / 1000.0))
                    .collect();
                let _ = write!(s, " stroke-dasharray=\"{}\"", parts.join(" "));
            }
        }
        s.push_str(&self.clip_attr());
        s.push_str("/>");
        self.body.push_str(&s);
        self.drawn += 1;
    }

    /// A raster picture (PNG, JPEG or GIF bytes) mapped from the unit square
    /// by `m`, blended with `blend` (an SVG `mix-blend-mode`).
    pub fn image(&mut self, bytes: &[u8], m: &Xf, blend: Option<&str>, opacity: f64) {
        if !m.is_finite() || m.det() == 0.0 {
            return;
        }
        let Some(uri) = data_uri(bytes) else {
            return;
        };
        let mut s = String::from("<g");
        s.push_str(&self.clip_attr());
        if let Some(b) = blend {
            let _ = write!(s, " style=\"mix-blend-mode:{b}\"");
        }
        if opacity < 0.999 {
            let _ = write!(s, " opacity=\"{:.3}\"", opacity.clamp(0.0, 1.0));
        }
        let [a, b, c, d, e, f] = m.0;
        let _ = write!(
            s,
            "><image width=\"1\" height=\"1\" preserveAspectRatio=\"none\" transform=\"matrix({a} {b} {c} {d} {e} {f})\" href=\"{uri}\"/></g>"
        );
        self.body.push_str(&s);
        self.drawn += 1;
    }

    pub fn text(&mut self, t: &Text) {
        if t.text.trim().is_empty() || !t.size.is_finite() || t.size <= 0.0 {
            return;
        }
        let stretched = (t.stretch - 1.0).abs() > 0.02 && t.stretch.is_finite() && t.stretch > 0.0;
        let mut s = String::from("<text");
        match &t.xs {
            Some(xs) if !xs.is_empty() => {
                // Positions are where the glyphs land: undo the stretch.
                let k = if stretched { t.stretch } else { 1.0 };
                let parts: Vec<String> = xs
                    .iter()
                    .map(|x| t.pos.0 + (x - t.pos.0) / k)
                    .map(|x| format!("{}", (x * 100.0).round() / 100.0))
                    .collect();
                let _ = write!(s, " x=\"{}\"", parts.join(" "));
            }
            _ => {
                let _ = write!(s, " x=\"{}\"", (t.pos.0 * 100.0).round() / 100.0);
            }
        }
        let _ = write!(
            s,
            " y=\"{}\" font-size=\"{}\" font-family=\"{}, Liberation Sans, Arial, sans-serif\" fill=\"{}\"",
            (t.pos.1 * 100.0).round() / 100.0,
            (t.size * 100.0).round() / 100.0,
            esc(&quote_family(t.family)),
            rgb(t.color)
        );
        if t.alpha < 0.999 {
            let _ = write!(s, " fill-opacity=\"{:.3}\"", t.alpha.clamp(0.0, 1.0));
        }
        if t.anchor != "start" {
            let _ = write!(s, " text-anchor=\"{}\"", t.anchor);
        }
        if t.weight >= 600 {
            s.push_str(" font-weight=\"bold\"");
        }
        if t.italic {
            s.push_str(" font-style=\"italic\"");
        }
        match (t.underline, t.strike) {
            (true, true) => s.push_str(" text-decoration=\"underline line-through\""),
            (true, false) => s.push_str(" text-decoration=\"underline\""),
            (false, true) => s.push_str(" text-decoration=\"line-through\""),
            _ => {}
        }
        let mut transform = String::new();
        if t.rotate.abs() > 1e-6 {
            let _ = write!(
                transform,
                "rotate({} {} {})",
                (t.rotate * 1000.0).round() / 1000.0,
                t.pivot.0,
                t.pivot.1
            );
        }
        if stretched {
            let (px, py) = t.pos;
            let _ = write!(
                transform,
                " translate({px} {py}) scale({} 1) translate({} {})",
                t.stretch, -px, -py
            );
        }
        if !transform.is_empty() {
            let _ = write!(s, " transform=\"{}\"", transform.trim());
        }
        s.push_str(" xml:space=\"preserve\"");
        s.push_str(&self.clip_attr());
        s.push('>');
        s.push_str(&esc(t.text));
        s.push_str("</text>");
        self.body.push_str(&s);
        self.drawn += 1;
        self.has_text = true;
    }

    /// The document, showing `view` (device units) at `w`×`h` px.
    pub fn finish(&self, view: [f64; 4], w: f64, h: f64) -> String {
        let [x, y, vw, vh] = view;
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{}\" height=\"{}\" viewBox=\"{} {} {} {}\" preserveAspectRatio=\"none\"><defs>{}</defs>{}</svg>",
            (w * 100.0).round() / 100.0,
            (h * 100.0).round() / 100.0,
            x,
            y,
            vw,
            vh,
            self.defs,
            self.body
        )
    }
}

fn quote_family(f: &str) -> String {
    let f = f.trim().trim_matches(char::from(0));
    if f.is_empty() {
        "Arial".into()
    } else {
        format!("'{}'", f.replace('\'', ""))
    }
}
