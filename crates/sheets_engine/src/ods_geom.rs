//! Curved geometry of OpenDocument shapes as DrawingML: elliptical arcs of
//! `svg:d` paths (SVG's `A`), and the `draw:enhanced-path` of a
//! `draw:custom-shape` that has no preset Kantor knows (ODF 1.3 19.145:
//! lines, cubic and quadratic curves, the arcs `A`/`B`/`W`/`V`, the angle
//! ellipses `T`/`U`, the quadrants `X`/`Y`, with parameters that are
//! numbers, `$n` modifiers, `?name` equations (`draw:equation`) or the
//! named constants). Arcs become cubic Béziers of at most 90° each.

use std::collections::HashMap;
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use roxmltree::Node;

const NS_DRAW: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
const NS_SVG: &str = "urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0";

/// One cubic Bézier piece: two control points and the end.
pub(crate) type Cubic = [(f64, f64); 3];

/// The arc of an ellipse centred on `c` with radii `rx`, `ry`, turned by
/// `phi`, from parameter angle `t0` through `dt` (radians, y down: the
/// point at `t` is `c + R(phi)·(rx cos t, ry sin t)`), as cubic pieces.
pub(crate) fn ellipse_arc(
    c: (f64, f64),
    rx: f64,
    ry: f64,
    phi: f64,
    t0: f64,
    dt: f64,
) -> Vec<Cubic> {
    let n = ((dt.abs() / FRAC_PI_2).ceil() as usize).clamp(1, 8);
    let step = dt / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let (sp, cp) = phi.sin_cos();
    let at = |t: f64| {
        let (s, co) = t.sin_cos();
        (
            c.0 + rx * cp * co - ry * sp * s,
            c.1 + rx * sp * co + ry * cp * s,
        )
    };
    let d = |t: f64| {
        let (s, co) = t.sin_cos();
        (-rx * cp * s - ry * sp * co, -rx * sp * s + ry * cp * co)
    };
    (0..n)
        .map(|i| {
            let (a, b) = (t0 + step * i as f64, t0 + step * (i + 1) as f64);
            let (p0, p3) = (at(a), at(b));
            let (d0, d3) = (d(a), d(b));
            [
                (p0.0 + k * d0.0, p0.1 + k * d0.1),
                (p3.0 - k * d3.0, p3.1 - k * d3.1),
                p3,
            ]
        })
        .collect()
}

/// SVG's elliptical arc (`A rx ry rotation large-arc sweep x y`) from
/// `from` to `to` as cubic pieces (SVG 1.1 F.6.5); `None` for a straight
/// line (a zero radius).
pub(crate) fn svg_arc(
    from: (f64, f64),
    rx: f64,
    ry: f64,
    rotation: f64,
    large: bool,
    sweep: bool,
    to: (f64, f64),
) -> Option<Vec<Cubic>> {
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx < 1e-9 || ry < 1e-9 || from == to {
        return None;
    }
    let phi = rotation.to_radians();
    let (sp, cp) = phi.sin_cos();
    let (hx, hy) = ((from.0 - to.0) / 2.0, (from.1 - to.1) / 2.0);
    let x1 = cp * hx + sp * hy;
    let y1 = -sp * hx + cp * hy;
    let lambda = x1 * x1 / (rx * rx) + y1 * y1 / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut sq = (num / den).max(0.0).sqrt();
    if large == sweep {
        sq = -sq;
    }
    let (cx1, cy1) = (sq * rx * y1 / ry, -sq * ry * x1 / rx);
    let c = (
        cp * cx1 - sp * cy1 + (from.0 + to.0) / 2.0,
        sp * cx1 + cp * cy1 + (from.1 + to.1) / 2.0,
    );
    let angle = |ux: f64, uy: f64| uy.atan2(ux);
    let t0 = angle((x1 - cx1) / rx, (y1 - cy1) / ry);
    let t1 = angle((-x1 - cx1) / rx, (-y1 - cy1) / ry);
    let mut dt = t1 - t0;
    if sweep && dt < 0.0 {
        dt += TAU;
    } else if !sweep && dt > 0.0 {
        dt -= TAU;
    }
    Some(ellipse_arc(c, rx, ry, phi, t0, dt))
}

fn pt(p: (f64, f64), o: (f64, f64)) -> String {
    format!(
        "<a:pt x=\"{}\" y=\"{}\"/>",
        (p.0 - o.0).round() as i64,
        (p.1 - o.1).round() as i64
    )
}

/// DrawingML cubic pieces relative to the origin `o`.
pub(crate) fn cubics_xml(pieces: &[Cubic], o: (f64, f64)) -> String {
    pieces
        .iter()
        .map(|[a, b, c]| {
            format!(
                "<a:cubicBezTo>{}{}{}</a:cubicBezTo>",
                pt(*a, o),
                pt(*b, o),
                pt(*c, o)
            )
        })
        .collect()
}

/// The values a custom shape's formulas see.
struct Env<'a> {
    view: [f64; 4],
    modifiers: Vec<f64>,
    equations: HashMap<&'a str, &'a str>,
    cache: HashMap<String, f64>,
    /// Equations being evaluated (a loop gives 0).
    busy: Vec<String>,
}

impl Env<'_> {
    fn constant(&self, name: &str) -> Option<f64> {
        let [x, y, w, h] = self.view;
        Some(match name {
            "left" => x,
            "top" => y,
            "right" => x + w,
            "bottom" => y + h,
            "width" | "logwidth" => w,
            "height" | "logheight" => h,
            "xstretch" | "ystretch" => 0.0,
            "hasstroke" | "hasfill" => 1.0,
            "pi" => PI,
            _ => return None,
        })
    }

    fn equation(&mut self, name: &str) -> f64 {
        if let Some(v) = self.cache.get(name) {
            return *v;
        }
        let Some(formula) = self.equations.get(name).copied() else {
            return 0.0;
        };
        if self.busy.iter().any(|b| b == name) {
            return 0.0;
        }
        self.busy.push(name.to_owned());
        let v = Formula {
            s: formula.as_bytes(),
            at: 0,
        }
        .expr(self)
        .unwrap_or(0.0);
        self.busy.pop();
        let v = if v.is_finite() { v } else { 0.0 };
        self.cache.insert(name.to_owned(), v);
        v
    }

    /// A path parameter or formula operand: a number, `$n`, `?name` or a
    /// constant.
    fn value(&mut self, token: &str) -> Option<f64> {
        if let Some(n) = token.strip_prefix('$') {
            return Some(
                n.parse::<usize>()
                    .ok()
                    .and_then(|i| self.modifiers.get(i).copied())
                    .unwrap_or(0.0),
            );
        }
        if let Some(name) = token.strip_prefix('?') {
            return Some(self.equation(name));
        }
        token.parse().ok().or_else(|| self.constant(token))
    }
}

/// A `draw:formula`: `+ - * /`, parentheses, unary minus, the functions
/// `abs sqrt sin cos tan atan atan2 min max if`, numbers, `$n`, `?name`
/// and constants. Angles are radians.
struct Formula<'s> {
    s: &'s [u8],
    at: usize,
}

impl Formula<'_> {
    fn skip_space(&mut self) {
        while self.s.get(self.at).is_some_and(u8::is_ascii_whitespace) {
            self.at += 1;
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        self.skip_space();
        if self.s.get(self.at) == Some(&c) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self, env: &mut Env) -> Option<f64> {
        let mut v = self.term(env)?;
        loop {
            if self.eat(b'+') {
                v += self.term(env)?;
            } else if self.eat(b'-') {
                v -= self.term(env)?;
            } else {
                return Some(v);
            }
        }
    }

    fn term(&mut self, env: &mut Env) -> Option<f64> {
        let mut v = self.factor(env)?;
        loop {
            if self.eat(b'*') {
                v *= self.factor(env)?;
            } else if self.eat(b'/') {
                let d = self.factor(env)?;
                v = if d == 0.0 { 0.0 } else { v / d };
            } else {
                return Some(v);
            }
        }
    }

    fn factor(&mut self, env: &mut Env) -> Option<f64> {
        if self.eat(b'-') {
            return Some(-self.factor(env)?);
        }
        if self.eat(b'+') {
            return self.factor(env);
        }
        if self.eat(b'(') {
            let v = self.expr(env)?;
            self.eat(b')');
            return Some(v);
        }
        self.skip_space();
        let start = self.at;
        while self
            .s
            .get(self.at)
            .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'$' | b'?' | b'_'))
        {
            self.at += 1;
        }
        let word = std::str::from_utf8(&self.s[start..self.at]).ok()?;
        if word.is_empty() {
            return None;
        }
        if self.eat(b'(') {
            let mut args = vec![self.expr(env)?];
            while self.eat(b',') {
                args.push(self.expr(env)?);
            }
            self.eat(b')');
            let a = |i: usize| args.get(i).copied().unwrap_or(0.0);
            return Some(match word {
                "abs" => a(0).abs(),
                "sqrt" => a(0).max(0.0).sqrt(),
                "sin" => a(0).sin(),
                "cos" => a(0).cos(),
                "tan" => a(0).tan(),
                "atan" => a(0).atan(),
                "atan2" => a(0).atan2(a(1)),
                "min" => a(0).min(a(1)),
                "max" => a(0).max(a(1)),
                "if" => {
                    if a(0) > 0.0 {
                        a(1)
                    } else {
                        a(2)
                    }
                }
                _ => return None,
            });
        }
        env.value(word)
    }
}

/// The commands and parameters of a `draw:enhanced-path`.
fn tokens(path: &str) -> Vec<Result<char, String>> {
    const COMMANDS: &str = "MLCZNFSTUAWBVXYQ";
    let mut out = Vec::new();
    let b = path.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() || c == b',' {
            i += 1;
            continue;
        }
        let start = i;
        if c.is_ascii_alphabetic() {
            while i < b.len() && b[i].is_ascii_alphabetic() {
                i += 1;
            }
            let word = &path[start..i];
            // A run of command letters ("ZN"), else a constant.
            if word.chars().all(|ch| COMMANDS.contains(ch)) {
                out.extend(word.chars().map(Ok));
            } else {
                out.push(Err(word.to_owned()));
            }
            continue;
        }
        i += 1;
        if c == b'$' || c == b'?' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
        } else if c.is_ascii_digit() || matches!(c, b'-' | b'+' | b'.') {
            while i < b.len() {
                let d = b[i];
                let exponent = matches!(d, b'e' | b'E')
                    && b.get(i + 1)
                        .is_some_and(|n| n.is_ascii_digit() || matches!(n, b'-' | b'+'));
                if d.is_ascii_digit() || d == b'.' {
                    i += 1;
                } else if exponent {
                    i += 2;
                } else {
                    break;
                }
            }
        } else {
            continue;
        }
        out.push(Err(path[start..i].to_owned()));
    }
    out
}

/// A sub-path being built.
#[derive(Default)]
struct Sub {
    body: String,
    fill: bool,
    stroke: bool,
}

/// The `a:custGeom` of a `draw:enhanced-geometry` element's
/// `draw:enhanced-path`, in its view box; `None` when there is no path or
/// it has commands Kantor doesn't read.
pub(crate) fn enhanced_geom(g: Node) -> Option<String> {
    let path = g.attribute((NS_DRAW, "enhanced-path"))?;
    let view = g
        .attribute((NS_SVG, "viewBox"))
        .map(|v| {
            v.split(|c: char| c.is_whitespace() || c == ',')
                .filter_map(|t| t.parse::<f64>().ok())
                .collect::<Vec<_>>()
        })
        .filter(|v| v.len() == 4)
        .map_or([0.0, 0.0, 21600.0, 21600.0], |v| {
            [v[0], v[1], v[2].max(1.0), v[3].max(1.0)]
        });
    let mut env = Env {
        view,
        modifiers: g
            .attribute((NS_DRAW, "modifiers"))
            .unwrap_or_default()
            .split_whitespace()
            .filter_map(|v| v.parse().ok())
            .collect(),
        equations: g
            .children()
            .filter(|c| c.has_tag_name((NS_DRAW, "equation")))
            .filter_map(|c| {
                Some((
                    c.attribute((NS_DRAW, "name"))?,
                    c.attribute((NS_DRAW, "formula"))?,
                ))
            })
            .collect(),
        cache: HashMap::new(),
        busy: Vec::new(),
    };
    let o = (view[0], view[1]);
    let toks = tokens(path);
    let mut subs: Vec<Sub> = Vec::new();
    let mut cur = Sub {
        fill: true,
        stroke: true,
        ..Sub::default()
    };
    let mut p = (0.0, 0.0);
    let mut start = (0.0, 0.0);
    let mut cmd = 'M';
    let mut k = 0;
    // X/Y alternate between their two tangents.
    let mut quadrant_x = true;
    while k < toks.len() {
        if let Ok(c) = toks[k] {
            cmd = c;
            k += 1;
            quadrant_x = cmd == 'X';
            match cmd {
                'Z' => {
                    cur.body.push_str("<a:close/>");
                    p = start;
                    continue;
                }
                'N' => {
                    if !cur.body.is_empty() {
                        subs.push(std::mem::replace(
                            &mut cur,
                            Sub {
                                fill: true,
                                stroke: true,
                                ..Sub::default()
                            },
                        ));
                    }
                    continue;
                }
                'F' => {
                    cur.fill = false;
                    continue;
                }
                'S' => {
                    cur.stroke = false;
                    continue;
                }
                _ => {}
            }
        }
        let arity = match cmd {
            'M' | 'L' | 'X' | 'Y' => 2,
            'Q' => 4,
            'C' | 'T' | 'U' => 6,
            'A' | 'B' | 'W' | 'V' => 8,
            _ => return None,
        };
        let mut v = [0.0f64; 8];
        let mut read = 0;
        for slot in v.iter_mut().take(arity) {
            let Some(Err(t)) = toks.get(k) else {
                break;
            };
            *slot = env.value(t)?;
            k += 1;
            read += 1;
        }
        // Too few parameters: the path ends there.
        if read < arity {
            break;
        }
        let at = |i: usize| (v[i], v[i + 1]);
        let line = |cur: &mut Sub, q: (f64, f64)| {
            cur.body.push_str(&format!("<a:lnTo>{}</a:lnTo>", pt(q, o)));
        };
        let move_to = |cur: &mut Sub, q: (f64, f64)| {
            cur.body
                .push_str(&format!("<a:moveTo>{}</a:moveTo>", pt(q, o)));
        };
        match cmd {
            'M' => {
                p = at(0);
                start = p;
                move_to(&mut cur, p);
                cmd = 'L';
            }
            'L' => {
                p = at(0);
                line(&mut cur, p);
            }
            'C' => {
                cur.body.push_str(&format!(
                    "<a:cubicBezTo>{}{}{}</a:cubicBezTo>",
                    pt(at(0), o),
                    pt(at(2), o),
                    pt(at(4), o)
                ));
                p = at(4);
            }
            'Q' => {
                cur.body.push_str(&format!(
                    "<a:quadBezTo>{}{}</a:quadBezTo>",
                    pt(at(0), o),
                    pt(at(2), o)
                ));
                p = at(2);
            }
            // Angle ellipses: centre, radii, start and end angle in degrees
            // (counter-clockwise on screen).
            'T' | 'U' => {
                let (c, (rx, ry)) = (at(0), at(2));
                let (a0, mut a1) = (v[4].to_radians(), v[5].to_radians());
                while a1 <= a0 {
                    a1 += TAU;
                }
                let s = (c.0 + rx * a0.cos(), c.1 - ry * a0.sin());
                if cmd == 'U' || cur.body.is_empty() {
                    move_to(&mut cur, s);
                    start = s;
                } else {
                    line(&mut cur, s);
                }
                let pieces = ellipse_arc(c, rx, ry, 0.0, -a0, -(a1 - a0));
                cur.body.push_str(&cubics_xml(&pieces, o));
                p = pieces.last().map_or(s, |q| q[2]);
            }
            // Arcs on the ellipse in a rectangle, from the angle of one
            // point to the angle of another (A, B counter-clockwise; W, V
            // clockwise; B and V start a new sub-path).
            _ if arity == 8 => {
                let (r1, r2) = (at(0), at(2));
                let c = ((r1.0 + r2.0) / 2.0, (r1.1 + r2.1) / 2.0);
                let (rx, ry) = (((r2.0 - r1.0) / 2.0).abs(), ((r2.1 - r1.1) / 2.0).abs());
                if rx < 1e-9 || ry < 1e-9 {
                    p = at(6);
                    line(&mut cur, p);
                    continue;
                }
                // Parameter angles (y down) of the two points.
                let angle = |q: (f64, f64)| ((q.1 - c.1) / ry).atan2((q.0 - c.0) / rx);
                let (t0, t1) = (angle(at(4)), angle(at(6)));
                let clockwise = matches!(cmd, 'W' | 'V');
                let mut dt = t1 - t0;
                if clockwise {
                    if dt <= 0.0 {
                        dt += TAU;
                    }
                } else if dt >= 0.0 {
                    dt -= TAU;
                }
                let s = (c.0 + rx * t0.cos(), c.1 + ry * t0.sin());
                if matches!(cmd, 'B' | 'V') || cur.body.is_empty() {
                    move_to(&mut cur, s);
                    start = s;
                } else {
                    line(&mut cur, s);
                }
                let pieces = ellipse_arc(c, rx, ry, 0.0, t0, dt);
                cur.body.push_str(&cubics_xml(&pieces, o));
                p = pieces.last().map_or(s, |q| q[2]);
            }
            // Quarter ellipses to a point, first along x (X) or y (Y), the
            // next one along the other axis.
            _ => {
                let e = at(0);
                const K: f64 = 0.552_284_749_8;
                let (c1, c2) = if quadrant_x {
                    ((p.0 + K * (e.0 - p.0), p.1), (e.0, e.1 - K * (e.1 - p.1)))
                } else {
                    ((p.0, p.1 + K * (e.1 - p.1)), (e.0 - K * (e.0 - p.0), e.1))
                };
                cur.body.push_str(&format!(
                    "<a:cubicBezTo>{}{}{}</a:cubicBezTo>",
                    pt(c1, o),
                    pt(c2, o),
                    pt(e, o)
                ));
                p = e;
                quadrant_x = !quadrant_x;
            }
        }
    }
    if !cur.body.is_empty() {
        subs.push(cur);
    }
    if subs.is_empty() {
        return None;
    }
    let (w, h) = (
        view[2].round().max(1.0) as i64,
        view[3].round().max(1.0) as i64,
    );
    let paths: String = subs
        .iter()
        .map(|s| {
            format!(
                "<a:path w=\"{w}\" h=\"{h}\"{}{}>{}</a:path>",
                if s.fill { "" } else { " fill=\"none\"" },
                if s.stroke { "" } else { " stroke=\"0\"" },
                s.body
            )
        })
        .collect();
    Some(format!(
        "<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l=\"0\" t=\"0\" r=\"r\" b=\"b\"/><a:pathLst>{paths}</a:pathLst></a:custGeom>"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: (f64, f64), b: (f64, f64)) -> bool {
        (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6
    }

    #[test]
    fn svg_arc_half_circle() {
        // From (0,0) to (20,0), radius 10, sweep: the lower half (y down).
        let pieces = svg_arc((0.0, 0.0), 10.0, 10.0, 0.0, false, true, (20.0, 0.0)).unwrap();
        assert_eq!(pieces.len(), 2);
        assert!(close(pieces[0][2], (10.0, -10.0)) || close(pieces[0][2], (10.0, 10.0)));
        assert!(close(pieces[1][2], (20.0, 0.0)));
        // A zero radius is a line.
        assert!(svg_arc((0.0, 0.0), 0.0, 5.0, 0.0, false, true, (1.0, 1.0)).is_none());
    }

    fn geom(attrs: &str, children: &str) -> Option<String> {
        let xml = format!(
            "<draw:enhanced-geometry xmlns:draw=\"{NS_DRAW}\" xmlns:svg=\"{NS_SVG}\" {attrs}>{children}</draw:enhanced-geometry>"
        );
        let doc = roxmltree::Document::parse(&xml).unwrap();
        enhanced_geom(doc.root_element())
    }

    #[test]
    fn enhanced_paths_with_arcs_equations_and_flags() {
        // A pie: centre, then a counter-clockwise arc, closed.
        let g = geom(
            "svg:viewBox=\"0 0 100 100\" draw:enhanced-path=\"M 50 50 L 100 50 A 0 0 100 100 100 50 50 0 Z N\"",
            "",
        )
        .unwrap();
        assert!(
            g.contains("<a:cubicBezTo>") && g.contains("<a:close/>"),
            "{g}"
        );
        // Counter-clockwise on screen from 0° to 90°: through the top.
        assert!(g.contains("<a:pt x=\"50\" y=\"0\"/></a:cubicBezTo>"), "{g}");
        // Equations, modifiers and constants; F and S per sub-path.
        let g = geom(
            "draw:modifiers=\"5400\" draw:enhanced-path=\"M ?f0 0 L right bottom F N M 0 0 Q $0 0 width height S N\"",
            "<draw:equation draw:name=\"f0\" draw:formula=\"$0 *2 - (10+10)\"/>",
        )
        .unwrap();
        assert!(g.contains("<a:moveTo><a:pt x=\"10780\" y=\"0\"/>"), "{g}");
        assert!(g.contains("<a:lnTo><a:pt x=\"21600\" y=\"21600\"/>"), "{g}");
        assert!(
            g.contains("fill=\"none\"") && g.contains("stroke=\"0\""),
            "{g}"
        );
        assert!(g.contains("<a:quadBezTo>"), "{g}");
        // Angle ellipse: a full circle of radius 10 around (10,10).
        let g = geom(
            "svg:viewBox=\"0 0 20 20\" draw:enhanced-path=\"U 10 10 10 10 0 360 Z\"",
            "",
        )
        .unwrap();
        assert_eq!(g.matches("<a:cubicBezTo>").count(), 4, "{g}");
        // Unknown commands: no geometry.
        assert!(geom("draw:enhanced-path=\"M 0 0 G 1 2\"", "").is_none());
    }
}
