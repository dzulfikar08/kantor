//! DrawingML shape geometry (ECMA-376 Part 1 §20.1.9, §19.5): shape
//! guides, adjust values, paths and the text rectangle, evaluated into
//! neutral outlines any renderer can draw.
//!
//! - [`Geometry`]: what a shape's `a:prstGeom` / `a:custGeom` says.
//! - [`ShapeDef`]: a parsed definition: the 187 presets of
//!   `presetShapeDefinitions.xml` ([`preset`]) or a `a:custGeom`.
//! - [`outline`]: the shape's [`SubPath`]s and text rectangle at a size.
//!
//! Coordinates are shape-local, origin at the top left, in the units of
//! the `w` and `h` given (pass EMU; the presets are scale-free, but custom
//! geometry may hold absolute lengths in EMU). Flips and rotation are the
//! renderer's (apply them to the outline as to the box). `arcTo` is turned
//! into cubic Béziers.

mod formula;
mod presets;

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, TAU};
use std::sync::Arc;

use docs_engine::Rgb;
use roxmltree::Node;

pub use formula::{Arg, Formula, Op};
pub use presets::{preset, preset_names};

/// A shape's geometry as written in the file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Geometry {
    /// `a:prstGeom`: a preset by name and its `a:avLst` overrides.
    Preset { name: String, adjust: Vec<Guide> },
    /// `a:custGeom`.
    Custom(Arc<ShapeDef>),
}

impl Geometry {
    pub fn preset(name: &str) -> Self {
        Self::Preset {
            name: name.to_owned(),
            adjust: Vec::new(),
        }
    }

    /// The preset name (`None` for custom geometry).
    pub fn preset_name(&self) -> Option<&str> {
        match self {
            Self::Preset { name, .. } => Some(name),
            Self::Custom(_) => None,
        }
    }

    /// Whether this draws exactly as the basic shapes a renderer may
    /// paint itself (`rect`, `ellipse`, `line`, `straightConnector1`, and
    /// `roundRect` without adjustments).
    pub fn is_basic(&self) -> bool {
        match self {
            Self::Preset { name, adjust } => {
                matches!(
                    name.as_str(),
                    "rect" | "ellipse" | "line" | "straightConnector1"
                ) || (name == "roundRect" && adjust.is_empty())
            }
            Self::Custom(_) => false,
        }
    }

    /// Parse `a:prstGeom` or `a:custGeom`.
    pub fn from_xml(n: Node) -> Option<Self> {
        match n.tag_name().name() {
            "prstGeom" => Some(Self::Preset {
                name: n.attribute("prst")?.to_owned(),
                adjust: child(n, "avLst").map(guides).unwrap_or_default(),
            }),
            "custGeom" => Some(Self::Custom(Arc::new(ShapeDef::from_cust_geom(n)))),
            _ => None,
        }
    }
}

/// A shape guide: `<gd name="…" fmla="…"/>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Guide {
    pub name: String,
    pub fmla: Formula,
}

impl Guide {
    /// `None` if the formula doesn't parse.
    pub fn new(name: &str, fmla: &str) -> Option<Self> {
        Some(Self {
            name: name.to_owned(),
            fmla: Formula::parse(fmla)?,
        })
    }
}

/// How a path is filled (`path/@fill`). The `lighten`/`darken` modes
/// shade the shape's fill ([`FillMode::shade`]); they're used for the 3D
/// faces of shapes such as `can`, `cube` and `bevel`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FillMode {
    None,
    #[default]
    Norm,
    Lighten,
    LightenLess,
    Darken,
    DarkenLess,
}

impl FillMode {
    pub fn parse(s: &str) -> Self {
        match s {
            "none" => Self::None,
            "lighten" => Self::Lighten,
            "lightenLess" => Self::LightenLess,
            "darken" => Self::Darken,
            "darkenLess" => Self::DarkenLess,
            _ => Self::Norm,
        }
    }

    pub fn is_filled(self) -> bool {
        self != Self::None
    }

    /// The fill colour of a path in this mode, from the shape's fill
    /// (as LibreOffice and PowerPoint do: darken = 60 %, darkenLess = 80 %
    /// of each channel; lighten / lightenLess = 40 % / 20 % towards white).
    pub fn shade(self, c: Rgb) -> Rgb {
        let k = |f: fn(f64) -> f64| Rgb(c.0.map(|v| f(v as f64).round().clamp(0.0, 255.0) as u8));
        match self {
            Self::None | Self::Norm => c,
            Self::Darken => k(|v| v * 0.6),
            Self::DarkenLess => k(|v| v * 0.8),
            Self::Lighten => k(|v| v + (255.0 - v) * 0.4),
            Self::LightenLess => k(|v| v + (255.0 - v) * 0.2),
        }
    }
}

/// A path command as defined (arguments are guide names or literals).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PathDefCmd {
    MoveTo(Arg, Arg),
    LineTo(Arg, Arg),
    /// `wR hR stAng swAng`.
    ArcTo([Arg; 4]),
    QuadTo([Arg; 4]),
    CubicTo([Arg; 6]),
    Close,
}

/// `a:path` as defined.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PathDef {
    /// Path coordinate space (`0` = the shape's own size).
    pub w: i64,
    pub h: i64,
    pub fill: FillMode,
    pub stroke: bool,
    pub extrusion_ok: bool,
    pub cmds: Vec<PathDefCmd>,
}

/// A geometry definition: a preset or a `a:custGeom`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ShapeDef {
    /// Adjust values with their defaults (`avLst`).
    pub av: Vec<Guide>,
    /// Shape guides (`gdLst`), evaluated in order after `av`.
    pub gd: Vec<Guide>,
    /// Text rectangle `l t r b` (`None` = the whole shape).
    pub rect: Option<[Arg; 4]>,
    pub paths: Vec<PathDef>,
}

impl ShapeDef {
    /// Parse a `a:custGeom` (unknown children are ignored; handles and
    /// connection sites aren't needed for display).
    pub fn from_cust_geom(n: Node) -> Self {
        let rect = child(n, "rect")
            .map(|r| ["l", "t", "r", "b"].map(|k| Arg::parse(r.attribute(k).unwrap_or("0"))));
        let paths = child(n, "pathLst")
            .map(|l| {
                l.children()
                    .filter(|p| p.is_element() && p.tag_name().name() == "path")
                    .map(path_def)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            av: child(n, "avLst").map(guides).unwrap_or_default(),
            gd: child(n, "gdLst").map(guides).unwrap_or_default(),
            rect,
            paths,
        }
    }
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn guides(list: Node) -> Vec<Guide> {
    list.children()
        .filter(|g| g.is_element() && g.tag_name().name() == "gd")
        .filter_map(|g| Guide::new(g.attribute("name")?, g.attribute("fmla")?))
        .collect()
}

fn path_def(p: Node) -> PathDef {
    let flag = |k| !matches!(p.attribute(k), Some("0" | "false"));
    let num = |k| p.attribute(k).and_then(|v| v.parse().ok()).unwrap_or(0);
    let pts = |c: Node| -> Vec<Arg> {
        c.children()
            .filter(|p| p.is_element() && p.tag_name().name() == "pt")
            .flat_map(|p| {
                [
                    Arg::parse(p.attribute("x").unwrap_or("0")),
                    Arg::parse(p.attribute("y").unwrap_or("0")),
                ]
            })
            .collect()
    };
    let mut cmds = Vec::new();
    for c in p.children().filter(|c| c.is_element()) {
        let a = pts(c);
        let cmd = match (c.tag_name().name(), a.len()) {
            ("moveTo", 2..) => PathDefCmd::MoveTo(a[0].clone(), a[1].clone()),
            ("lnTo", 2..) => PathDefCmd::LineTo(a[0].clone(), a[1].clone()),
            ("quadBezTo", 4..) => PathDefCmd::QuadTo(arr(&a)),
            ("cubicBezTo", 6..) => PathDefCmd::CubicTo(arr(&a)),
            ("arcTo", _) => PathDefCmd::ArcTo(
                ["wR", "hR", "stAng", "swAng"].map(|k| Arg::parse(c.attribute(k).unwrap_or("0"))),
            ),
            ("close", _) => PathDefCmd::Close,
            _ => continue,
        };
        cmds.push(cmd);
    }
    PathDef {
        w: num("w"),
        h: num("h"),
        fill: FillMode::parse(p.attribute("fill").unwrap_or("norm")),
        stroke: flag("stroke"),
        extrusion_ok: flag("extrusionOk"),
        cmds,
    }
}

fn arr<const N: usize>(a: &[Arg]) -> [Arg; N] {
    std::array::from_fn(|i| a[i].clone())
}

/// A point in shape-local coordinates.
pub type Pt = (f64, f64);

/// An evaluated path command.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCmd {
    MoveTo(Pt),
    LineTo(Pt),
    /// Control point, end point.
    QuadTo(Pt, Pt),
    /// Two control points, end point.
    CubicTo(Pt, Pt, Pt),
    Close,
}

/// One `a:path`, evaluated. A sub-path may hold several contours (one per
/// `MoveTo`); holes (`donut`, `frame`) run the other way, so they come out
/// with either the even-odd or the non-zero rule.
#[derive(Clone, Debug, PartialEq)]
pub struct SubPath {
    pub cmds: Vec<PathCmd>,
    pub fill: FillMode,
    /// Whether the outline is drawn along this path.
    pub stroke: bool,
    pub extrusion_ok: bool,
}

impl SubPath {
    /// Contours as polylines, curves flattened to within `tol` (same
    /// units). Each contour is closed when its path closed it.
    pub fn flatten(&self, tol: f64) -> Vec<(Vec<Pt>, bool)> {
        let tol = tol.max(1e-9);
        let mut out: Vec<(Vec<Pt>, bool)> = Vec::new();
        let mut cur: Vec<Pt> = Vec::new();
        let mut at = (0.0, 0.0);
        let flush = |cur: &mut Vec<Pt>, out: &mut Vec<(Vec<Pt>, bool)>, closed: bool| {
            if cur.len() > 1 {
                out.push((std::mem::take(cur), closed));
            }
            cur.clear();
        };
        for c in &self.cmds {
            match *c {
                PathCmd::MoveTo(p) => {
                    flush(&mut cur, &mut out, false);
                    cur.push(p);
                    at = p;
                }
                PathCmd::LineTo(p) => {
                    if cur.is_empty() {
                        cur.push(at);
                    }
                    cur.push(p);
                    at = p;
                }
                PathCmd::QuadTo(c1, p) => {
                    // Degree elevation.
                    let lerp = |a: Pt, b: Pt| {
                        (a.0 + (b.0 - a.0) * 2.0 / 3.0, a.1 + (b.1 - a.1) * 2.0 / 3.0)
                    };
                    if cur.is_empty() {
                        cur.push(at);
                    }
                    cubic_points(&mut cur, at, lerp(at, c1), lerp(p, c1), p, tol);
                    at = p;
                }
                PathCmd::CubicTo(c1, c2, p) => {
                    if cur.is_empty() {
                        cur.push(at);
                    }
                    cubic_points(&mut cur, at, c1, c2, p, tol);
                    at = p;
                }
                PathCmd::Close => {
                    if let Some(&first) = cur.first() {
                        at = first;
                    }
                    flush(&mut cur, &mut out, true);
                }
            }
        }
        flush(&mut cur, &mut out, false);
        out
    }
}

fn cubic_points(out: &mut Vec<Pt>, p0: Pt, p1: Pt, p2: Pt, p3: Pt, tol: f64) {
    // Control polygon length bounds the curve; segments of ~tol*8 keep the
    // chord error well within tol for these shapes.
    let d = |a: Pt, b: Pt| ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let len = d(p0, p1) + d(p1, p2) + d(p2, p3);
    let n = ((len / (tol * 8.0)).sqrt().ceil() as usize * 2).clamp(1, 256);
    for i in 1..=n {
        let t = i as f64 / n as f64;
        let u = 1.0 - t;
        let (a, b, c, e) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push((
            a * p0.0 + b * p1.0 + c * p2.0 + e * p3.0,
            a * p0.1 + b * p1.1 + c * p2.1 + e * p3.1,
        ));
    }
}

/// A rectangle in shape-local coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub l: f64,
    pub t: f64,
    pub r: f64,
    pub b: f64,
}

/// A geometry evaluated at a size.
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub paths: Vec<SubPath>,
    /// Where text goes (`rect`), before the body's insets.
    pub text_rect: Rect,
}

impl Outline {
    /// Bounding box of the flattened paths (`None` if there are none).
    pub fn bounds(&self) -> Option<Rect> {
        let mut b: Option<Rect> = None;
        for p in &self.paths {
            for (pts, _) in p.flatten(1.0) {
                for (x, y) in pts {
                    let r = b.get_or_insert(Rect {
                        l: x,
                        t: y,
                        r: x,
                        b: y,
                    });
                    r.l = r.l.min(x);
                    r.t = r.t.min(y);
                    r.r = r.r.max(x);
                    r.b = r.b.max(y);
                }
            }
        }
        b
    }
}

/// The outline of `geometry` at `w` × `h` (shape-local, same units).
/// Unknown presets draw as a rectangle.
pub fn outline(geometry: &Geometry, w: f64, h: f64) -> Outline {
    match geometry {
        Geometry::Preset { name, adjust } => match preset(name) {
            Some(def) => evaluate(&def, adjust, w, h),
            None => evaluate(&rect_def(), &[], w, h),
        },
        Geometry::Custom(def) => evaluate(def, &[], w, h),
    }
}

fn rect_def() -> Arc<ShapeDef> {
    preset("rect").expect("rect preset")
}

/// Guide values during evaluation.
pub(crate) struct Env<'a> {
    w: f64,
    h: f64,
    vars: HashMap<&'a str, f64>,
}

impl<'a> Env<'a> {
    pub(crate) fn new(w: f64, h: f64) -> Self {
        Self {
            w,
            h,
            vars: HashMap::new(),
        }
    }

    pub(crate) fn get(&self, a: &Arg) -> f64 {
        match a {
            Arg::Lit(v) => *v as f64,
            Arg::Ref(n) => self
                .vars
                .get(n.as_str())
                .copied()
                .or_else(|| builtin(n, self.w, self.h))
                .unwrap_or(0.0),
        }
    }

    pub(crate) fn define(&mut self, g: &'a Guide) {
        let v = g.fmla.eval(|a| self.get(a));
        self.vars.insert(&g.name, v);
    }
}

/// Built-in guides (§20.1.9.11): `w h l t r b hc vc ss ls`, `wd<n>`,
/// `hd<n>`, `ssd<n>` (a size over n) and `cd<n>` / `<k>cd<n>` (k/n of a
/// full turn, in 60000ths of a degree).
fn builtin(n: &str, w: f64, h: f64) -> Option<f64> {
    let ss = w.min(h);
    let div = |rest: &str, of: f64| {
        rest.parse::<f64>()
            .ok()
            .filter(|d| *d != 0.0)
            .map(|d| of / d)
    };
    Some(match n {
        "w" | "r" => w,
        "h" | "b" => h,
        "l" | "t" => 0.0,
        "hc" => w / 2.0,
        "vc" => h / 2.0,
        "ss" => ss,
        "ls" => w.max(h),
        _ => {
            if let Some(r) = n.strip_prefix("ssd") {
                return div(r, ss);
            }
            if let Some(r) = n.strip_prefix("wd") {
                return div(r, w);
            }
            if let Some(r) = n.strip_prefix("hd") {
                return div(r, h);
            }
            let (k, d) = n.split_once("cd")?;
            let k = if k.is_empty() { 1.0 } else { k.parse().ok()? };
            return div(d, 21_600_000.0 * k);
        }
    })
}

/// Evaluate a definition at `w` × `h`, `adjust` overriding the defaults
/// of its adjust values by name.
pub fn evaluate(def: &ShapeDef, adjust: &[Guide], w: f64, h: f64) -> Outline {
    let mut env = Env::new(w, h);
    for g in &def.av {
        let g = adjust.iter().find(|a| a.name == g.name).unwrap_or(g);
        env.define(g);
    }
    // Overrides of guides the definition doesn't declare still count.
    for a in adjust {
        if !def.av.iter().any(|g| g.name == a.name) {
            env.define(a);
        }
    }
    for g in &def.gd {
        env.define(g);
    }
    let text_rect = match &def.rect {
        Some([l, t, r, b]) => Rect {
            l: env.get(l),
            t: env.get(t),
            r: env.get(r),
            b: env.get(b),
        },
        None => Rect {
            l: 0.0,
            t: 0.0,
            r: w,
            b: h,
        },
    };
    let paths = def.paths.iter().map(|p| eval_path(p, &env)).collect();
    Outline { paths, text_rect }
}

fn eval_path(p: &PathDef, env: &Env) -> SubPath {
    // Path units → shape units.
    let sx = if p.w > 0 { env.w / p.w as f64 } else { 1.0 };
    let sy = if p.h > 0 { env.h / p.h as f64 } else { 1.0 };
    let pt = |x: &Arg, y: &Arg| (env.get(x), env.get(y));
    let s = |(x, y): Pt| (x * sx, y * sy);
    let mut cmds = Vec::with_capacity(p.cmds.len());
    // Current point and contour start, in path units.
    let mut at = (0.0, 0.0);
    let mut start = at;
    for c in &p.cmds {
        match c {
            PathDefCmd::MoveTo(x, y) => {
                at = pt(x, y);
                start = at;
                cmds.push(PathCmd::MoveTo(s(at)));
            }
            PathDefCmd::LineTo(x, y) => {
                at = pt(x, y);
                cmds.push(PathCmd::LineTo(s(at)));
            }
            PathDefCmd::QuadTo([x1, y1, x2, y2]) => {
                let c1 = pt(x1, y1);
                at = pt(x2, y2);
                cmds.push(PathCmd::QuadTo(s(c1), s(at)));
            }
            PathDefCmd::CubicTo([x1, y1, x2, y2, x3, y3]) => {
                let (c1, c2) = (pt(x1, y1), pt(x2, y2));
                at = pt(x3, y3);
                cmds.push(PathCmd::CubicTo(s(c1), s(c2), s(at)));
            }
            PathDefCmd::ArcTo([wr, hr, st, sw]) => {
                let (wr, hr) = (env.get(wr), env.get(hr));
                let (st, sw) = (env.get(st), env.get(sw));
                at = arc_to(at, wr, hr, st, sw, &mut |c1, c2, p| {
                    cmds.push(PathCmd::CubicTo(s(c1), s(c2), s(p)));
                });
            }
            PathDefCmd::Close => {
                cmds.push(PathCmd::Close);
                at = start;
            }
        }
    }
    SubPath {
        cmds,
        fill: p.fill,
        stroke: p.stroke,
        extrusion_ok: p.extrusion_ok,
    }
}

/// Angles in 60000ths of a degree.
const ANGLE_UNIT: f64 = 60_000.0;

/// The parametric angle of the point at visual angle `a` (radians) on an
/// ellipse with radii `rx`, `ry`, continuous in `a` (DrawingML's arc
/// angles are visual: the angle of the point seen from the centre).
fn parametric(a: f64, rx: f64, ry: f64) -> f64 {
    let t = (rx * a.sin()).atan2(ry * a.cos());
    t + TAU * ((a - t) / TAU).round()
}

/// `arcTo` from `from`: an arc of the ellipse with radii `wr`, `hr`
/// through `from` at angle `st`, sweeping `sw` (60000ths of a degree,
/// clockwise in y-down space), as cubics of at most 90° each. Returns the
/// end point.
pub(crate) fn arc_to(
    from: Pt,
    wr: f64,
    hr: f64,
    st: f64,
    sw: f64,
    emit: &mut dyn FnMut(Pt, Pt, Pt),
) -> Pt {
    let (a0, a1) = (
        (st / ANGLE_UNIT).to_radians(),
        ((st + sw) / ANGLE_UNIT).to_radians(),
    );
    let (t0, t1) = (parametric(a0, wr, hr), parametric(a1, wr, hr));
    let c = (from.0 - wr * t0.cos(), from.1 - hr * t0.sin());
    let on = |t: f64| (c.0 + wr * t.cos(), c.1 + hr * t.sin());
    let span = t1 - t0;
    if span == 0.0 {
        return from;
    }
    let n = (span.abs() / FRAC_PI_2 - 1e-9).ceil().max(1.0) as usize;
    let d = span / n as f64;
    let k = 4.0 / 3.0 * (d / 4.0).tan();
    for i in 0..n {
        let (ta, tb) = (t0 + d * i as f64, t0 + d * (i + 1) as f64);
        let (pa, pb) = (on(ta), on(tb));
        let c1 = (pa.0 - k * wr * ta.sin(), pa.1 + k * hr * ta.cos());
        let c2 = (pb.0 + k * wr * tb.sin(), pb.1 - k * hr * tb.cos());
        emit(c1, c2, pb);
    }
    on(t1)
}

#[cfg(test)]
mod tests;
