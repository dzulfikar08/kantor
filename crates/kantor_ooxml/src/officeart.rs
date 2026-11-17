//! OfficeArt ([MS-ODRAW]) shape properties as DrawingML, shared by the
//! importers of Word 97-2003 (`.doc`) and Excel 97-2003 (`.xls`) drawings:
//! property tables, colours, preset shape types, arrowheads and freeform
//! paths (vertices, segments and guide formulas, LibreOffice's arcs
//! included).

use std::collections::HashMap;

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        b.get(o..o.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        b.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// A shape's properties, as [`read_fopt`] reads them.
struct S<'a> {
    props: &'a HashMap<u16, u32>,
    complex: &'a HashMap<u16, Vec<u8>>,
}

impl S<'_> {
    fn prop(&self, id: u16) -> Option<u32> {
        self.props.get(&id).copied()
    }
}

/// Read the properties of an `OfficeArtFOPT` (or secondary / tertiary
/// FOPT) record body holding `n` of them (the record's instance): simple
/// ones into `props`, complex ones' data into `complex` (first wins).
pub fn read_fopt(
    n: usize,
    body: &[u8],
    props: &mut HashMap<u16, u32>,
    complex: &mut HashMap<u16, Vec<u8>>,
) {
    let mut complex_at = n * 6;
    for k in 0..n {
        let (Some(opid), Some(op)) = (u16_at(body, k * 6), u32_at(body, k * 6 + 2)) else {
            break;
        };
        let id = opid & 0x3FFF;
        if opid & 0x8000 != 0 {
            // Arrays (`IMsoArray`) whose length leaves out their 6-byte
            // header, as Word and LibreOffice write some.
            let mut len = op as usize;
            if let (Some(n), Some(cb)) = (u16_at(body, complex_at), u16_at(body, complex_at + 4)) {
                let cb = if cb == 0xFFF0 { 4 } else { cb as usize };
                if len > 0 && n as usize * cb == len {
                    len += 6;
                }
            }
            if let Some(data) = body.get(complex_at..complex_at.saturating_add(len)) {
                complex.entry(id).or_insert_with(|| data.to_vec());
            }
            complex_at = complex_at.saturating_add(len);
        } else {
            props.entry(id).or_insert(op);
        }
    }
}

/// An `OfficeArtCOLORREF` as `0xRRGGBB` (`None` for scheme or system
/// colours).
pub fn color(v: u32) -> Option<u32> {
    if v >> 24 & 0x18 != 0 {
        return None;
    }
    let (r, g, b) = (v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF);
    Some(r << 16 | g << 8 | b)
}

/// The DrawingML preset of a shape type.
pub fn preset(typ: u16) -> Option<&'static str> {
    Some(match typ {
        1 | 202 | 75 | 109 => "rect",
        2 => "roundRect",
        3 => "ellipse",
        4 | 110 => "diamond",
        5 => "triangle",
        6 => "rtTriangle",
        7 => "parallelogram",
        8 => "trapezoid",
        9 => "hexagon",
        10 => "octagon",
        11 => "plus",
        12 => "star5",
        13 => "rightArrow",
        15 => "homePlate",
        16 => "cube",
        20 => "line",
        21 => "plaque",
        22 => "can",
        23 => "donut",
        32 => "straightConnector1",
        55 => "chevron",
        56 => "pentagon",
        61 => "wedgeRectCallout",
        62 => "wedgeRoundRectCallout",
        63 => "wedgeEllipseCallout",
        66 => "leftArrow",
        67 => "downArrow",
        68 => "upArrow",
        69 => "leftRightArrow",
        70 => "upDownArrow",
        74 => "heart",
        96 => "smileyFace",
        116 => "flowChartTerminator",
        183 => "sun",
        184 => "moon",
        187 => "star4",
        _ => return None,
    })
}

/// A freeform's path (`pVertices`, `pSegmentInfo`, `geoRight/Bottom`) as
/// `a:custGeom`.

/// A freeform shape's path (`pVertices`, `pSegmentInfo`, `pGuides` and the
/// geometry box) as `a:custGeom`; `None` when it can't be followed.
pub fn cust_geom(props: &HashMap<u16, u32>, complex: &HashMap<u16, Vec<u8>>) -> Option<String> {
    cust_geom_of(&S { props, complex })
}

fn cust_geom_of(s: &S) -> Option<String> {
    let verts = s.complex.get(&0x145)?;
    let (n, cb) = (u16_at(verts, 0)? as usize, u16_at(verts, 4)?);
    let geo = |id: u16, d: i64| s.prop(id).map_or(d, |v| i64::from(v as i32));
    let (left, top) = (geo(0x140, 0), geo(0x141, 0));
    let guides = guides(s, [left, top, geo(0x142, 21600), geo(0x143, 21600)]);
    // Values with the high bit set name a guide formula's result.
    let guide = |k: usize| guides.get(k).copied();
    let mut pts = Vec::with_capacity(n);
    for k in 0..n {
        let p = match cb {
            0xFFF0 | 4 => {
                let (x, y) = (u16_at(verts, 6 + k * 4)?, u16_at(verts, 8 + k * 4)?);
                let v = |c: u16| {
                    if c & 0xFF00 == 0x8000 {
                        guide((c & 0xFF) as usize)
                    } else {
                        Some(i64::from(c as i16))
                    }
                };
                (v(x)?, v(y)?)
            }
            8 => {
                let (x, y) = (u32_at(verts, 6 + k * 8)?, u32_at(verts, 10 + k * 8)?);
                let v = |c: u32| {
                    if c & 0xFFFF_0000 == 0x8000_0000 {
                        guide((c & 0xFFFF) as usize)
                    } else {
                        Some(i64::from(c as i32))
                    }
                };
                (v(x)?, v(y)?)
            }
            _ => return None,
        };
        pts.push(p);
    }
    if pts.is_empty() {
        return None;
    }
    let (mut right, mut bottom) = (geo(0x142, 21600), geo(0x143, 21600));
    if right <= left {
        right = pts.iter().map(|p| p.0).max().unwrap_or(1).max(left + 1);
    }
    if bottom <= top {
        bottom = pts.iter().map(|p| p.1).max().unwrap_or(1).max(top + 1);
    }
    let pt = |p: (i64, i64)| format!("<a:pt x=\"{}\" y=\"{}\"/>", p.0 - left, p.1 - top);
    let segs: Vec<u16> = s
        .complex
        .get(&0x146)
        .map(|b| {
            let n = u16_at(b, 0).unwrap_or(0) as usize;
            (0..n).filter_map(|k| u16_at(b, 6 + k * 2)).collect()
        })
        .unwrap_or_default();
    // A second try reads LibreOffice's arcs (see below) when the segments
    // leave vertices over.
    let mut arcs_left = 0usize;
    let mut tried = false;
    let (path, no_fill) = loop {
        let mut path = String::new();
        let mut it = pts.iter().copied().peekable();
        let mut no_fill = false;
        let mut cur = (0i64, 0i64);
        let cubic = |path: &mut String, a: (f64, f64), b: (f64, f64), c: (f64, f64)| {
            let r = |p: (f64, f64)| (p.0.round() as i64, p.1.round() as i64);
            path.push_str(&format!(
                "<a:cubicBezTo>{}{}{}</a:cubicBezTo>",
                pt(r(a)),
                pt(r(b)),
                pt(r(c))
            ));
        };
        if segs.is_empty() {
            let first = it.next()?;
            path.push_str(&format!("<a:moveTo>{}</a:moveTo>", pt(first)));
            for p in it.by_ref() {
                path.push_str(&format!("<a:lnTo>{}</a:lnTo>", pt(p)));
            }
        } else {
            for &seg in &segs {
                let kind = seg >> 13;
                let count = (seg & 0x1FFF) as usize;
                match kind {
                    0 => {
                        for _ in 0..count.max(1) {
                            let Some(p) = it.next() else { break };
                            // LibreOffice writes its arcs (ODF's arcangleto) as
                            // a "line" with two vertices: the radii, then the
                            // start angle and the sweep in degrees.
                            if arcs_left > 0
                                && p.0 >= 0
                                && p.1 >= 0
                                && let Some(&q) = it.peek()
                                && q.0.abs() <= 360
                                && q.1.abs() <= 360
                            {
                                it.next();
                                arcs_left -= 1;
                                let (rx, ry) = (p.0 as f64, p.1 as f64);
                                let start = (q.0 as f64).to_radians();
                                let sweep = (q.1 as f64).to_radians();
                                let cx = cur.0 as f64 - rx * start.cos();
                                let cy = cur.1 as f64 - ry * start.sin();
                                let at = |t: f64| (cx + rx * t.cos(), cy + ry * t.sin());
                                let n = (sweep.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0)
                                    as usize;
                                let step = sweep / n as f64;
                                let k = 4.0 / 3.0 * (step / 4.0).tan();
                                let mut t = start;
                                for _ in 0..n {
                                    let u = t + step;
                                    let (p0, p3) = (at(t), at(u));
                                    cubic(
                                        &mut path,
                                        (p0.0 - k * rx * t.sin(), p0.1 + k * ry * t.cos()),
                                        (p3.0 + k * rx * u.sin(), p3.1 - k * ry * u.cos()),
                                        p3,
                                    );
                                    t = u;
                                }
                                let end = at(t);
                                cur = (end.0.round() as i64, end.1.round() as i64);
                                continue;
                            }
                            path.push_str(&format!("<a:lnTo>{}</a:lnTo>", pt(p)));
                            cur = p;
                        }
                    }
                    1 => {
                        for _ in 0..count.max(1) {
                            let (Some(a), Some(b), Some(c)) = (it.next(), it.next(), it.next())
                            else {
                                break;
                            };
                            path.push_str(&format!(
                                "<a:cubicBezTo>{}{}{}</a:cubicBezTo>",
                                pt(a),
                                pt(b),
                                pt(c)
                            ));
                            cur = c;
                        }
                    }
                    2 => {
                        if let Some(p) = it.next() {
                            path.push_str(&format!("<a:moveTo>{}</a:moveTo>", pt(p)));
                            cur = p;
                        }
                    }
                    3 => path.push_str("<a:close/>"),
                    4 => break,
                    5 => {
                        // Escapes: a code and how many times it repeats.
                        let code = (seg >> 8) & 0x1F;
                        let n = (seg & 0xFF) as usize;
                        match code {
                            // ArcTo, Arc, ClockwiseArcTo, ClockwiseArc: the
                            // ellipse's box (two corners), then points on the
                            // rays where the arc starts and ends.
                            3..=6 => {
                                for _ in 0..n.max(1) {
                                    let (Some(a), Some(b), Some(st), Some(en)) =
                                        (it.next(), it.next(), it.next(), it.next())
                                    else {
                                        break;
                                    };
                                    let line_to = matches!(code, 3 | 5);
                                    let clockwise = matches!(code, 5 | 6);
                                    cur = arc(
                                        &mut path, &pt, &cubic, a, b, st, en, clockwise, line_to,
                                    );
                                }
                            }
                            // EllipticalQuadrantX/Y: quarter ellipses to each
                            // point, starting along x (or y), then alternating.
                            7 | 8 => {
                                let mut x_first = code == 7;
                                for _ in 0..n.max(1) {
                                    let Some(p) = it.next() else { break };
                                    let (c, e) =
                                        ((cur.0 as f64, cur.1 as f64), (p.0 as f64, p.1 as f64));
                                    const K: f64 = 0.552_284_75;
                                    let (c1, c2) = if x_first {
                                        ((c.0 + K * (e.0 - c.0), c.1), (e.0, e.1 + K * (c.1 - e.1)))
                                    } else {
                                        ((c.0, c.1 + K * (e.1 - c.1)), (e.0 + K * (c.0 - e.0), e.1))
                                    };
                                    cubic(&mut path, c1, c2, e);
                                    cur = p;
                                    x_first = !x_first;
                                }
                            }
                            // QuadraticBezier: a control point and an end.
                            9 => {
                                for _ in 0..n.max(1) {
                                    let (Some(a), Some(b)) = (it.next(), it.next()) else {
                                        break;
                                    };
                                    path.push_str(&format!(
                                        "<a:quadBezTo>{}{}</a:quadBezTo>",
                                        pt(a),
                                        pt(b)
                                    ));
                                    cur = b;
                                }
                            }
                            0xA => no_fill = true,
                            // NoLine, AutoLine, AutoCurve, CornerLine and the
                            // like change no geometry.
                            0xB..=0x14 => {}
                            _ => {
                                for _ in 0..n {
                                    it.next();
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        let rest = it.count();
        if path.starts_with("<a:moveTo>") && (segs.is_empty() || rest == 0) {
            break (path, no_fill);
        }
        // Vertices the segments don't account for: the path can't be
        // trusted, unless they are LibreOffice's arcs.
        if tried || segs.is_empty() || rest == 0 {
            return None;
        }
        tried = true;
        arcs_left = rest;
    };
    let fill = if no_fill { " fill=\"none\"" } else { "" };
    // A path that reaches past its geometry box (LibreOffice's ellipses
    // are written at twice their box) is fitted by its own extent.
    let (mut w, mut h) = (right - left, bottom - top);
    for part in path.split("<a:pt x=\"").skip(1) {
        let mut nums = part.split('"').filter_map(|v| v.parse::<i64>().ok());
        if let (Some(x), Some(y)) = (nums.next(), nums.next()) {
            w = w.max(x);
            h = h.max(y);
        }
    }
    Some(format!(
        "<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l=\"0\" t=\"0\" r=\"r\" b=\"b\"/><a:pathLst><a:path w=\"{w}\" h=\"{h}\"{fill}>{path}</a:path></a:pathLst></a:custGeom>"
    ))
}

/// An escape arc ([MS-ODRAW] msopathEscapeArcTo and friends) as cubic
/// Béziers: the ellipse in the box `a`–`b`, from the ray through `st` to
/// the ray through `en`, counterclockwise on the page unless `clockwise`;
/// starts with a line (`line_to`) or a move. Returns where it ends.
#[allow(clippy::too_many_arguments)]
fn arc(
    path: &mut String,
    pt: &dyn Fn((i64, i64)) -> String,
    cubic: &dyn Fn(&mut String, (f64, f64), (f64, f64), (f64, f64)),
    a: (i64, i64),
    b: (i64, i64),
    st: (i64, i64),
    en: (i64, i64),
    clockwise: bool,
    line_to: bool,
) -> (i64, i64) {
    let (cx, cy) = ((a.0 + b.0) as f64 / 2.0, (a.1 + b.1) as f64 / 2.0);
    let (rx, ry) = (
        ((b.0 - a.0) as f64 / 2.0).abs().max(1e-9),
        ((b.1 - a.1) as f64 / 2.0).abs().max(1e-9),
    );
    let ang = |p: (i64, i64)| ((p.1 as f64 - cy) / ry).atan2((p.0 as f64 - cx) / rx);
    let (a0, a1) = (ang(st), ang(en));
    let tau = std::f64::consts::TAU;
    // y grows downwards: counterclockwise on the page is a falling angle.
    let mut sweep = a1 - a0;
    if clockwise {
        while sweep <= 0.0 {
            sweep += tau;
        }
    } else {
        while sweep >= 0.0 {
            sweep -= tau;
        }
    }
    let at = |t: f64| (cx + rx * t.cos(), cy + ry * t.sin());
    let r = |p: (f64, f64)| (p.0.round() as i64, p.1.round() as i64);
    let start = r(at(a0));
    let tag = if line_to { "lnTo" } else { "moveTo" };
    path.push_str(&format!("<a:{tag}>{}</a:{tag}>", pt(start)));
    let n = (sweep.abs() / std::f64::consts::FRAC_PI_2).ceil().max(1.0) as usize;
    let step = sweep / n as f64;
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let mut t = a0;
    for _ in 0..n {
        let u = t + step;
        let (p0, p3) = (at(t), at(u));
        let (d0, d3) = ((-rx * t.sin(), ry * t.cos()), (-rx * u.sin(), ry * u.cos()));
        cubic(
            path,
            (p0.0 + k * d0.0, p0.1 + k * d0.1),
            (p3.0 - k * d3.0, p3.1 - k * d3.1),
            p3,
        );
        t = u;
    }
    r(at(t))
}

/// A line end of `lineStartArrowhead` / `lineEndArrowhead`.
/// The results of a shape's guide formulas (`pGuides`, [MS-ODRAW]
/// 2.2.59 SG): each from up to three parameters, which are numbers or
/// (flagged) the geometry's edges, adjust values, earlier guides or the
/// shape's size.
fn guides(s: &S, [l, t, r, b]: [i64; 4]) -> Vec<i64> {
    let Some(g) = s.complex.get(&0x156) else {
        return Vec::new();
    };
    let n = u16_at(g, 0).unwrap_or(0) as usize;
    let cb = u16_at(g, 4).unwrap_or(8);
    let cb = if cb == 0xFFF0 { 4 } else { cb as usize };
    if cb != 8 {
        return Vec::new();
    }
    let mut out: Vec<i64> = Vec::with_capacity(n);
    for k in 0..n {
        let at = 6 + k * 8;
        let (Some(op), Some(p1), Some(p2), Some(p3)) = (
            u16_at(g, at),
            u16_at(g, at + 2),
            u16_at(g, at + 4),
            u16_at(g, at + 6),
        ) else {
            break;
        };
        let value = |raw: u16, calc: bool| -> f64 {
            if !calc {
                return f64::from(raw as i16);
            }
            match raw {
                0x0140 => l as f64,
                0x0141 => t as f64,
                0x0142 => r as f64,
                0x0143 => b as f64,
                0x0147..=0x0150 => s.prop(raw).map_or(0.0, |v| f64::from(v as i32)),
                0x0400..=0x047F => out.get((raw - 0x0400) as usize).copied().unwrap_or(0) as f64,
                0x04F8 | 0x04FC => (r - l) as f64,
                0x04F9 | 0x04FD => (b - t) as f64,
                0x04FE => (r - l) as f64 / 2.0,
                0x04FF => (b - t) as f64 / 2.0,
                _ => 0.0,
            }
        };
        let a = value(p1, op & 0x2000 != 0);
        let bb = value(p2, op & 0x4000 != 0);
        let c = value(p3, op & 0x8000 != 0);
        let deg = |v: f64| (v / 65536.0).to_radians();
        let v = match op & 0x1FFF {
            0 => a + bb - c,
            1 if c != 0.0 => a * bb / c,
            1 => 0.0,
            2 => (a + bb) / 2.0,
            3 => a.abs(),
            4 => a.min(bb),
            5 => a.max(bb),
            6 => {
                if a > 0.0 {
                    bb
                } else {
                    c
                }
            }
            7 => (a * a + bb * bb + c * c).sqrt(),
            8 => bb.atan2(a).to_degrees() * 65536.0,
            9 => a * deg(bb).sin(),
            10 => a * deg(bb).cos(),
            11 => a * c.atan2(bb).cos(),
            12 => a * c.atan2(bb).sin(),
            13 => a.max(0.0).sqrt(),
            14 => a + bb * 65536.0 - c * 65536.0,
            15 if bb != 0.0 => c * (1.0 - (a / bb).powi(2)).max(0.0).sqrt(),
            16 => a * deg(bb).tan(),
            _ => 0.0,
        };
        out.push(if v.is_finite() { v.round() as i64 } else { 0 });
    }
    out
}

pub fn arrowhead(v: u32) -> Option<&'static str> {
    Some(match v {
        1 => "triangle",
        2 => "stealth",
        3 => "diamond",
        4 => "oval",
        5 => "arrow",
        _ => return None,
    })
}
