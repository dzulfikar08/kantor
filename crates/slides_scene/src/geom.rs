//! Renderer-neutral geometry: points, affine transforms and paths of
//! lines, quadratic and cubic curves (arcs become cubics), and the outline
//! of a shape ([`shape_outline`]).

use slides_engine::{Geom, Shape, ShapeKind};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct P {
    pub x: f32,
    pub y: f32,
}

pub const fn p(x: f32, y: f32) -> P {
    P { x, y }
}

impl P {
    pub fn minus(self, o: P) -> P {
        p(self.x - o.x, self.y - o.y)
    }

    pub fn plus(self, o: P) -> P {
        p(self.x + o.x, self.y + o.y)
    }

    pub fn scale(self, k: f32) -> P {
        p(self.x * k, self.y * k)
    }

    pub fn len(self) -> f32 {
        self.x.hypot(self.y)
    }

    pub fn dot(self, o: P) -> f32 {
        self.x * o.x + self.y * o.y
    }

    /// Unit vector (zero stays zero).
    pub fn unit(self) -> P {
        let l = self.len();
        if l > 1e-9 { self.scale(1.0 / l) } else { self }
    }
}

/// `x' = a·x + c·y + e`, `y' = b·x + d·y + f` (PDF / krilla order).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub e: f32,
    pub f: f32,
}

impl Default for Affine {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Affine {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub fn translate(x: f32, y: f32) -> Self {
        Self {
            e: x,
            f: y,
            ..Self::IDENTITY
        }
    }

    pub fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            d: sy,
            ..Self::IDENTITY
        }
    }

    /// Clockwise on screen (y down) by `rad`.
    pub fn rotate(rad: f32) -> Self {
        let (s, c) = rad.sin_cos();
        Self {
            a: c,
            b: s,
            c: -s,
            d: c,
            e: 0.0,
            f: 0.0,
        }
    }

    /// `self`, then `next`.
    pub fn then(self, n: Affine) -> Affine {
        Affine {
            a: n.a * self.a + n.c * self.b,
            b: n.b * self.a + n.d * self.b,
            c: n.a * self.c + n.c * self.d,
            d: n.b * self.c + n.d * self.d,
            e: n.a * self.e + n.c * self.f + n.e,
            f: n.b * self.e + n.d * self.f + n.f,
        }
    }

    pub fn apply(&self, q: P) -> P {
        p(
            self.a * q.x + self.c * q.y + self.e,
            self.b * q.x + self.d * q.y + self.f,
        )
    }

    /// The linear part only (directions).
    pub fn apply_vec(&self, v: P) -> P {
        p(self.a * v.x + self.c * v.y, self.b * v.x + self.d * v.y)
    }

    pub fn invert(&self) -> Option<Affine> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Affine {
            a,
            b,
            c,
            d,
            e: -(a * self.e + c * self.f),
            f: -(b * self.e + d * self.f),
        })
    }

    /// No rotation or flip (only translation and positive scaling).
    pub fn is_upright(&self) -> bool {
        self.b.abs() < 1e-6 && self.c.abs() < 1e-6 && self.a > 0.0 && self.d > 0.0
    }

    /// For a rotation, optionally after a horizontal flip: the clockwise
    /// angle (radians) and whether it flips.
    pub fn angle_flip(&self) -> (f32, bool) {
        let det = self.a * self.d - self.b * self.c;
        if det < 0.0 {
            // M = R · diag(-1, 1): its first column is −(cos, sin).
            ((-self.b).atan2(-self.a), true)
        } else {
            (self.b.atan2(self.a), false)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(P),
    Line(P),
    /// Control point, end.
    Quad(P, P),
    /// Two control points, end.
    Cubic(P, P, P),
    Close,
}

/// A path of subpaths (each starts with [`Seg::Move`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub segs: Vec<Seg>,
}

/// How a subpath is filled (DrawingML `path/@fill`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum PathFill {
    None,
    #[default]
    Norm,
    Lighten,
    LightenLess,
    Darken,
    DarkenLess,
}

/// One drawn path of a shape's outline.
#[derive(Clone, Debug, PartialEq)]
pub struct SubPath {
    pub path: Path,
    pub fill: PathFill,
    pub stroke: bool,
}

impl Path {
    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    fn current(&self) -> P {
        let mut start = P::default();
        let mut cur = P::default();
        for s in &self.segs {
            match *s {
                Seg::Move(q) => {
                    start = q;
                    cur = q;
                }
                Seg::Line(q) | Seg::Quad(_, q) | Seg::Cubic(_, _, q) => cur = q,
                Seg::Close => cur = start,
            }
        }
        cur
    }

    pub fn move_to(&mut self, q: P) {
        self.segs.push(Seg::Move(q));
    }

    pub fn line_to(&mut self, q: P) {
        self.segs.push(Seg::Line(q));
    }

    pub fn quad_to(&mut self, c: P, q: P) {
        self.segs.push(Seg::Quad(c, q));
    }

    pub fn cubic_to(&mut self, c1: P, c2: P, q: P) {
        self.segs.push(Seg::Cubic(c1, c2, q));
    }

    pub fn close(&mut self) {
        self.segs.push(Seg::Close);
    }

    /// DrawingML `a:arcTo` from the current point: an arc of the ellipse
    /// with radii `wr`, `hr` starting at angle `st` and sweeping `sw`
    /// (degrees, clockwise; angles are visual, as in the file format).
    pub fn arc_to(&mut self, wr: f32, hr: f32, st: f32, sw: f32) {
        let cur = self.current();
        if wr.abs() < 1e-6 || hr.abs() < 1e-6 {
            // Degenerate: a straight line to where the arc would end.
            let (t1, t2) = (st.to_radians(), (st + sw).to_radians());
            let to = cur.plus(p(wr * (t2.cos() - t1.cos()), hr * (t2.sin() - t1.sin())));
            self.line_to(to);
            return;
        }
        // Visual angle → ellipse parameter.
        let param = |deg: f32| {
            let a = deg.to_radians();
            (wr * a.sin()).atan2(hr * a.cos())
        };
        let t1 = param(st);
        let mut t2 = param(st + sw);
        let sweep = sw.to_radians();
        // Keep the sweep's direction and size (whole turns included).
        let turns = (sweep / std::f32::consts::TAU).trunc();
        t2 += turns * std::f32::consts::TAU;
        if sweep > 0.0 && t2 <= t1 + 1e-6 && sweep.abs() > 1e-6 {
            t2 += std::f32::consts::TAU;
        } else if sweep < 0.0 && t2 >= t1 - 1e-6 {
            t2 -= std::f32::consts::TAU;
        }
        if sweep.abs() < 1e-6 {
            return;
        }
        let c = cur.minus(p(wr * t1.cos(), hr * t1.sin()));
        ellipse_arc(self, c, wr, hr, t1, t2);
    }

    pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Path {
        let mut pa = Path::default();
        pa.move_to(p(x, y));
        pa.line_to(p(x + w, y));
        pa.line_to(p(x + w, y + h));
        pa.line_to(p(x, y + h));
        pa.close();
        pa
    }

    pub fn ellipse(cx: f32, cy: f32, rx: f32, ry: f32) -> Path {
        let mut pa = Path::default();
        pa.move_to(p(cx + rx, cy));
        ellipse_arc(&mut pa, p(cx, cy), rx, ry, 0.0, std::f32::consts::TAU);
        pa.close();
        pa
    }

    /// A rectangle with corners of radius `r`.
    pub fn round_rect(w: f32, h: f32, r: f32) -> Path {
        let r = r.clamp(0.0, w.min(h) / 2.0);
        if r <= 0.0 {
            return Path::rect(0.0, 0.0, w, h);
        }
        let mut pa = Path::default();
        pa.move_to(p(0.0, r));
        pa.arc_to(r, r, 180.0, 90.0);
        pa.line_to(p(w - r, 0.0));
        pa.arc_to(r, r, 270.0, 90.0);
        pa.line_to(p(w, h - r));
        pa.arc_to(r, r, 0.0, 90.0);
        pa.line_to(p(r, h));
        pa.arc_to(r, r, 90.0, 90.0);
        pa.close();
        pa
    }

    pub fn transformed(&self, t: &Affine) -> Path {
        Path {
            segs: self
                .segs
                .iter()
                .map(|s| match *s {
                    Seg::Move(q) => Seg::Move(t.apply(q)),
                    Seg::Line(q) => Seg::Line(t.apply(q)),
                    Seg::Quad(c, q) => Seg::Quad(t.apply(c), t.apply(q)),
                    Seg::Cubic(a, b, q) => Seg::Cubic(t.apply(a), t.apply(b), t.apply(q)),
                    Seg::Close => Seg::Close,
                })
                .collect(),
        }
    }

    /// Polylines within `tol` of the curves, with whether each is closed.
    pub fn flatten(&self, tol: f32) -> Vec<(Vec<P>, bool)> {
        let mut out: Vec<(Vec<P>, bool)> = Vec::new();
        let mut cur: Vec<P> = Vec::new();
        let tol = tol.max(1e-3);
        let flush = |cur: &mut Vec<P>, out: &mut Vec<(Vec<P>, bool)>, closed: bool| {
            if cur.len() > 1 {
                out.push((std::mem::take(cur), closed));
            } else {
                cur.clear();
            }
        };
        for s in &self.segs {
            match *s {
                Seg::Move(q) => {
                    flush(&mut cur, &mut out, false);
                    cur.push(q);
                }
                Seg::Line(q) => cur.push(q),
                Seg::Quad(c, q) => {
                    let a = *cur.last().unwrap_or(&q);
                    let n = steps(a.minus(c).len() + c.minus(q).len(), tol);
                    for i in 1..=n {
                        let t = i as f32 / n as f32;
                        let u = 1.0 - t;
                        cur.push(
                            a.scale(u * u)
                                .plus(c.scale(2.0 * u * t))
                                .plus(q.scale(t * t)),
                        );
                    }
                }
                Seg::Cubic(c1, c2, q) => {
                    let a = *cur.last().unwrap_or(&q);
                    let n = steps(
                        a.minus(c1).len() + c1.minus(c2).len() + c2.minus(q).len(),
                        tol,
                    );
                    for i in 1..=n {
                        let t = i as f32 / n as f32;
                        let u = 1.0 - t;
                        cur.push(
                            a.scale(u * u * u)
                                .plus(c1.scale(3.0 * u * u * t))
                                .plus(c2.scale(3.0 * u * t * t))
                                .plus(q.scale(t * t * t)),
                        );
                    }
                }
                Seg::Close => {
                    let start = cur.first().copied();
                    flush(&mut cur, &mut out, true);
                    // Drawing may continue from the start point.
                    if let Some(s) = start {
                        cur.push(s);
                    }
                }
            }
        }
        flush(&mut cur, &mut out, false);
        out
    }

    /// Box around the points and control points: `[x0, y0, x1, y1]`.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let mut b: Option<[f32; 4]> = None;
        let mut add = |q: P| {
            let r = b.get_or_insert([q.x, q.y, q.x, q.y]);
            r[0] = r[0].min(q.x);
            r[1] = r[1].min(q.y);
            r[2] = r[2].max(q.x);
            r[3] = r[3].max(q.y);
        };
        for s in &self.segs {
            match *s {
                Seg::Move(q) | Seg::Line(q) => add(q),
                Seg::Quad(c, q) => {
                    add(c);
                    add(q);
                }
                Seg::Cubic(a, c, q) => {
                    add(a);
                    add(c);
                    add(q);
                }
                Seg::Close => {}
            }
        }
        b
    }

    /// Whether every subpath is closed.
    pub fn is_closed(&self) -> bool {
        let mut open = false;
        let mut any = false;
        for s in &self.segs {
            match s {
                Seg::Move(_) => {
                    if open {
                        return false;
                    }
                    open = true;
                    any = true;
                }
                Seg::Close => open = false,
                _ => {}
            }
        }
        any && !open
    }

    /// Start of the path and the direction pointing out of it (backwards
    /// along the first segment), for a head arrow.
    pub fn start_end(&self) -> Option<(P, P)> {
        let mut it = self.segs.iter();
        let Some(Seg::Move(a)) = it.next() else {
            return None;
        };
        for s in it {
            let next = match *s {
                Seg::Line(q) | Seg::Quad(q, _) | Seg::Cubic(q, _, _) => q,
                _ => return None,
            };
            let d = a.minus(next);
            if d.len() > 1e-4 {
                return Some((*a, d.unit()));
            }
            // A zero-length first control: try the segment's end.
            let end = match *s {
                Seg::Quad(_, q) | Seg::Cubic(_, _, q) => q,
                _ => continue,
            };
            let d = a.minus(end);
            if d.len() > 1e-4 {
                return Some((*a, d.unit()));
            }
        }
        None
    }

    /// End of the path and the direction pointing out of it, for a tail
    /// arrow. `None` if the path ends closed.
    pub fn finish_end(&self) -> Option<(P, P)> {
        let n = self.segs.len();
        let last = self.segs.last()?;
        let (end, before) = match *last {
            Seg::Line(q) => (q, self.point_before(n - 1)?),
            Seg::Quad(c, q) => (
                q,
                if c.minus(q).len() > 1e-4 {
                    c
                } else {
                    self.point_before(n - 1)?
                },
            ),
            Seg::Cubic(_, c2, q) => (
                q,
                if c2.minus(q).len() > 1e-4 {
                    c2
                } else {
                    self.point_before(n - 1)?
                },
            ),
            _ => return None,
        };
        let d = end.minus(before);
        (d.len() > 1e-4).then(|| (end, d.unit()))
    }

    /// The end point of segment `i - 1`.
    fn point_before(&self, i: usize) -> Option<P> {
        match *self.segs.get(i.checked_sub(1)?)? {
            Seg::Move(q) | Seg::Line(q) | Seg::Quad(_, q) | Seg::Cubic(_, _, q) => Some(q),
            Seg::Close => None,
        }
    }

    /// Pull the path's first point (`at_start`) or last point towards the
    /// inside by `by`, when that segment is a straight line (so a line
    /// doesn't poke through an arrowhead's tip).
    pub fn shorten(&mut self, at_start: bool, by: f32) {
        let n = self.segs.len();
        if n < 2 || by <= 0.0 {
            return;
        }
        if at_start {
            if let (Seg::Move(a), Seg::Line(b)) = (self.segs[0], self.segs[1]) {
                let d = b.minus(a);
                let l = d.len();
                if l > by * 1.5 {
                    self.segs[0] = Seg::Move(a.plus(d.scale(by / l)));
                }
            }
        } else if let Seg::Line(b) = self.segs[n - 1]
            && let Some(a) = self.point_before(n - 1)
        {
            let d = a.minus(b);
            let l = d.len();
            if l > by * 1.5 {
                self.segs[n - 1] = Seg::Line(b.plus(d.scale(by / l)));
            }
        }
    }
}

fn steps(len: f32, tol: f32) -> usize {
    ((len / tol).sqrt().ceil() as usize).clamp(1, 256)
}

/// Cubic pieces of an elliptical arc from parameter `t1` to `t2` about
/// `c`; the current point must be at `t1`.
fn ellipse_arc(pa: &mut Path, c: P, rx: f32, ry: f32, t1: f32, t2: f32) {
    let total = t2 - t1;
    let n = (total.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = total / n as f32;
    let at = |t: f32| p(c.x + rx * t.cos(), c.y + ry * t.sin());
    let tangent = |t: f32| p(-rx * t.sin(), ry * t.cos());
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    for i in 0..n {
        let a = t1 + step * i as f32;
        let b = a + step;
        let (p0, p3) = (at(a), at(b));
        pa.cubic_to(
            p0.plus(tangent(a).scale(k)),
            p3.minus(tangent(b).scale(k)),
            p3,
        );
    }
}

/// The outline of a shape in its own box (`w` × `h`, origin top left,
/// before rotation and flips).
///
/// This is the one place that knows shape geometry. It draws rectangles,
/// rounded rectangles, ellipses, lines and the common connectors itself;
/// every other preset is drawn as its box. The preset-geometry module
/// (all DrawingML presets and `a:custGeom`) plugs in here.
pub fn shape_outline(shape: &Shape, w: f32, h: f32) -> Vec<SubPath> {
    let geom = match &shape.kind {
        ShapeKind::Shape { geom, .. } => geom,
        _ => &Geom::Rect,
    };
    let filled = |path| {
        vec![SubPath {
            path,
            fill: PathFill::Norm,
            stroke: true,
        }]
    };
    let open = |path| {
        vec![SubPath {
            path,
            fill: PathFill::None,
            stroke: true,
        }]
    };
    match geom {
        Geom::Rect => filled(Path::rect(0.0, 0.0, w, h)),
        // PowerPoint's default corner: 16 667 / 100 000 of the short side.
        Geom::RoundRect => filled(Path::round_rect(w, h, w.min(h) * 0.166_67)),
        Geom::Ellipse => filled(Path::ellipse(w / 2.0, h / 2.0, w / 2.0, h / 2.0)),
        Geom::Line => {
            let mut pa = Path::default();
            pa.move_to(p(0.0, 0.0));
            pa.line_to(p(w, h));
            open(pa)
        }
        Geom::Other(name) => {
            let mut pa = Path::default();
            if name.starts_with("bentConnector2") {
                pa.move_to(p(0.0, 0.0));
                pa.line_to(p(w, 0.0));
                pa.line_to(p(w, h));
                open(pa)
            } else if name.starts_with("bentConnector") {
                // Default adjustment: the bend half way.
                pa.move_to(p(0.0, 0.0));
                pa.line_to(p(w / 2.0, 0.0));
                pa.line_to(p(w / 2.0, h));
                pa.line_to(p(w, h));
                open(pa)
            } else if name.starts_with("curvedConnector") {
                pa.move_to(p(0.0, 0.0));
                pa.cubic_to(p(w / 2.0, 0.0), p(w / 2.0, h), p(w, h));
                open(pa)
            } else {
                filled(Path::rect(0.0, 0.0, w, h))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: P, b: P) -> bool {
        (a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3
    }

    #[test]
    fn transforms_compose_and_invert() {
        let t = Affine::translate(-5.0, -5.0)
            .then(Affine::rotate(90f32.to_radians()))
            .then(Affine::translate(5.0, 5.0));
        // A quarter turn clockwise about (5, 5): (10, 5) → (5, 10).
        assert!(close(t.apply(p(10.0, 5.0)), p(5.0, 10.0)));
        let back = t.invert().unwrap().apply(p(5.0, 10.0));
        assert!(close(back, p(10.0, 5.0)));
        let (a, flip) = t.angle_flip();
        assert!((a.to_degrees() - 90.0).abs() < 1e-3 && !flip);
        let f = Affine::scale(-1.0, 1.0).then(Affine::rotate(30f32.to_radians()));
        let (a, flip) = f.angle_flip();
        assert!((a.to_degrees() - 30.0).abs() < 1e-3 && flip);
    }

    #[test]
    fn arcs_land_on_the_ellipse() {
        // Quarter arc from the left of a circle to its top, clockwise.
        let mut pa = Path::default();
        pa.move_to(p(0.0, 10.0));
        pa.arc_to(10.0, 10.0, 180.0, 90.0);
        assert!(close(pa.current(), p(10.0, 0.0)), "{:?}", pa.current());
        let flat = pa.flatten(0.01);
        for q in &flat[0].0 {
            let r = q.minus(p(10.0, 10.0)).len();
            assert!((r - 10.0).abs() < 0.02, "{r}");
        }
        // A full ellipse closes where it started.
        let e = Path::ellipse(0.0, 0.0, 20.0, 10.0);
        assert!(close(e.current(), p(20.0, 0.0)));
        assert!(e.is_closed());
        let b = e.bounds().unwrap();
        assert!((b[2] - 20.0).abs() < 0.1 && (b[3] - 10.0 * 1.0).abs() < 0.5);
    }

    #[test]
    fn path_ends_point_outwards() {
        let mut pa = Path::default();
        pa.move_to(p(0.0, 0.0));
        pa.line_to(p(10.0, 0.0));
        pa.line_to(p(10.0, 20.0));
        let (s, ds) = pa.start_end().unwrap();
        assert!(close(s, p(0.0, 0.0)) && close(ds, p(-1.0, 0.0)));
        let (e, de) = pa.finish_end().unwrap();
        assert!(close(e, p(10.0, 20.0)) && close(de, p(0.0, 1.0)));
        pa.shorten(false, 5.0);
        assert_eq!(pa.segs[2], Seg::Line(p(10.0, 15.0)));
        assert!(Path::rect(0.0, 0.0, 1.0, 1.0).finish_end().is_none());
    }
}
