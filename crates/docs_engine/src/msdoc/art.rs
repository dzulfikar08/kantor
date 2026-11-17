//! Floating objects of a `.doc` ([MS-DOC] 2.9.253 PlcfSpa, [MS-ODRAW]).
//!
//! Each floating object has an anchor character (`0x08`) in the text and
//! an `SPA` in `PlcSpaMom` at that CP: the shape id, its rectangle in twips
//! from what `bx`/`by` say (margin, page, column / paragraph), and its
//! wrapping. The shape itself is in the OfficeArt drawing (`DggInfo` in the
//! table stream): its type, properties (fill, line, picture, crop, text
//! insets, name) and, for a text box, the id of its text in the text box
//! story. Pictures are in the BLIP store, either in the `FBSE` itself or in
//! the WordDocument stream at `foDelay`.
//!
//! Preset shape types become DrawingML presets; freeform shapes (which
//! LibreOffice writes for all its shapes) become a `custGeom` path from
//! their vertices and segments, vertices that name guide formulas worked
//! out ([`guides`]).

use std::collections::HashMap;

use super::rd::{i32_at, u16_at, u32_at};

/// An `SPA`: where a floating object is anchored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Spa {
    pub spid: u32,
    /// Left, top, right, bottom in twips.
    pub rect: [i32; 4],
    /// 0 margin, 1 page, 2 column (`bx`); 0 margin, 1 page, 2 paragraph (`by`).
    pub bx: u8,
    pub by: u8,
    /// 0 around, 1 top and bottom, 2 square, 3 none, 4 tight, 5 through.
    pub wr: u8,
    /// 0 both sides, 1 left, 2 right, 3 largest.
    pub wrk: u8,
    pub below: bool,
}

/// `PlcSpaMom` (or `PlcSpaHdr`): SPAs by CP.
pub(crate) fn spas(plc: &[u8]) -> HashMap<u32, Spa> {
    let mut out = HashMap::new();
    if plc.len() < 4 {
        return out;
    }
    let n = (plc.len() - 4) / 30;
    for k in 0..n {
        let at = (n + 1) * 4 + k * 26;
        let (Some(cp), Some(spid), Some(flags)) =
            (u32_at(plc, k * 4), u32_at(plc, at), u16_at(plc, at + 20))
        else {
            break;
        };
        let r = |o: usize| i32_at(plc, at + o).unwrap_or(0);
        out.insert(
            cp,
            Spa {
                spid,
                rect: [r(4), r(8), r(12), r(16)],
                bx: ((flags >> 1) & 3) as u8,
                by: ((flags >> 3) & 3) as u8,
                wr: ((flags >> 5) & 15) as u8,
                wrk: ((flags >> 9) & 15) as u8,
                below: flags & 0x4000 != 0,
            },
        );
    }
    out
}

/// A shape of the drawing.
#[derive(Clone, Debug, Default)]
pub(crate) struct Shape {
    /// `msospt` (the FSP record's instance).
    pub typ: u16,
    pub flags: u32,
    /// Simple properties by id.
    pub props: HashMap<u16, u32>,
    /// Complex properties' data by id.
    pub complex: HashMap<u16, Vec<u8>>,
    /// `OfficeArtClientTextbox`: the text box's id.
    pub txid: Option<u32>,
    /// Drawing order in its group.
    pub order: usize,
    /// A group's shapes (ids, back to front).
    pub children: Vec<u32>,
    /// A group's own coordinates (`OfficeArtFSPGR`): left, top, right,
    /// bottom.
    pub group_rect: Option<[i32; 4]>,
    /// A shape's place in its group's coordinates (`OfficeArtChildAnchor`).
    pub child_anchor: Option<[i32; 4]>,
}

impl Shape {
    pub fn group(&self) -> bool {
        self.flags & 0x1 != 0
    }
    pub fn flip(&self) -> (bool, bool) {
        (self.flags & 0x40 != 0, self.flags & 0x80 != 0)
    }
    pub fn prop(&self, id: u16) -> Option<u32> {
        self.props.get(&id).copied()
    }
    /// A boolean of a property set (`fUse…` bit 16 above the value bit).
    pub fn flag(&self, id: u16, bit: u32) -> Option<bool> {
        let v = self.prop(id)?;
        (v & (1 << (bit + 16)) != 0).then_some(v & (1 << bit) != 0)
    }
    /// A UTF-16 string property (name, description).
    pub fn text(&self, id: u16) -> Option<String> {
        let b = self.complex.get(&id)?;
        let units: Vec<u16> = b
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .take_while(|&u| u != 0)
            .collect();
        let s = String::from_utf16_lossy(&units);
        (!s.trim().is_empty()).then_some(s)
    }
}

/// A picture of the BLIP store.
#[derive(Clone, Debug)]
pub(crate) enum Blip {
    Image(&'static str, Vec<u8>),
    /// Metafiles, DIB, TIFF.
    Other,
}

/// One OfficeArt record: version, instance, type, body.
struct Rec<'a> {
    ver: u16,
    inst: u16,
    typ: u16,
    body: &'a [u8],
}

fn records(mut b: &[u8]) -> Vec<Rec<'_>> {
    let mut out = Vec::new();
    while b.len() >= 8 {
        let (Some(vi), Some(typ), Some(len)) = (u16_at(b, 0), u16_at(b, 2), u32_at(b, 4)) else {
            break;
        };
        let end = 8usize.saturating_add(len as usize).min(b.len());
        out.push(Rec {
            ver: vi & 0xF,
            inst: vi >> 4,
            typ,
            body: &b[8..end],
        });
        b = &b[end..];
    }
    out
}

/// The drawing of the main document (`dgglbl` 0) and the BLIP store.
#[derive(Default)]
pub(crate) struct Drawing {
    pub shapes: HashMap<u32, Shape>,
    pub blips: Vec<Blip>,
}

/// Reads `DggInfo`; `word` is the WordDocument stream (`foDelay` blips).
pub(crate) fn drawing(dgg: &[u8], word: &[u8]) -> Drawing {
    let mut d = Drawing::default();
    let mut b = dgg;
    // OfficeArtDggContainer, then (dgglbl, OfficeArtDgContainer) pairs.
    let mut first = true;
    while b.len() >= 8 {
        if !first {
            let label = b[0];
            b = &b[1..];
            let Some(len) = u32_at(b, 4) else { break };
            let end = 8usize.saturating_add(len as usize).min(b.len());
            // dgglbl 0: the main text's drawing, 1: the headers' (shape
            // ids are unique across both).
            if label <= 1 && u16_at(b, 2) == Some(0xF002) {
                let mut order = 0;
                let _ = shapes(&b[8..end], &mut d.shapes, &mut order);
            }
            b = &b[end..];
            continue;
        }
        first = false;
        let Some(len) = u32_at(b, 4) else { break };
        let end = 8usize.saturating_add(len as usize).min(b.len());
        if u16_at(b, 2) == Some(0xF000) {
            for store in records(&b[8..end]).into_iter().filter(|r| r.typ == 0xF001) {
                for fbse in records(store.body) {
                    d.blips.push(if fbse.typ == 0xF007 {
                        fbse_blip(fbse.body, word)
                    } else {
                        Blip::Other
                    });
                }
            }
        }
        b = &b[end..];
    }
    d
}

fn fbse_blip(body: &[u8], word: &[u8]) -> Blip {
    let name = body.get(33).copied().unwrap_or(0) as usize;
    let rec = if body.len() > 36 + name + 8 {
        body.get(36 + name..)
    } else {
        let size = u32_at(body, 20).unwrap_or(0) as usize;
        u32_at(body, 28).and_then(|at| {
            let at = at as usize;
            word.get(at..at.checked_add(size)?.min(word.len()))
        })
    };
    match rec.and_then(super::blip_record) {
        Some((ext, bytes)) => Blip::Image(ext, bytes),
        None => Blip::Other,
    }
}

/// The shapes of a drawing container, by id (groups and their children);
/// returns the ids of the shapes and groups directly in `b`.
fn shapes(b: &[u8], out: &mut HashMap<u32, Shape>, order: &mut usize) -> Vec<u32> {
    let rect = |body: &[u8]| -> Option<[i32; 4]> {
        Some([
            i32_at(body, 0)?,
            i32_at(body, 4)?,
            i32_at(body, 8)?,
            i32_at(body, 12)?,
        ])
    };
    let mut ids = Vec::new();
    for r in records(b) {
        match r.typ {
            // Group container: its first shape is the group itself.
            0xF003 => {
                let inner = shapes(r.body, out, order);
                if let Some((&group, rest)) = inner.split_first() {
                    if let Some(g) = out.get_mut(&group) {
                        g.children = rest.to_vec();
                    }
                    ids.push(group);
                }
            }
            0xF004 => {
                let mut s = Shape {
                    order: *order,
                    ..Shape::default()
                };
                *order += 1;
                let mut spid = None;
                for c in records(r.body) {
                    match c.typ {
                        0xF00A => {
                            s.typ = c.inst;
                            spid = u32_at(c.body, 0);
                            s.flags = u32_at(c.body, 4).unwrap_or(0);
                        }
                        0xF009 => s.group_rect = rect(c.body),
                        0xF00F => s.child_anchor = rect(c.body),
                        0xF00B | 0xF121 | 0xF122 => read_props(&c, &mut s),
                        0xF00D => s.txid = u32_at(c.body, 0),
                        _ => {}
                    }
                }
                // Text linked by property (`lTxid`) rather than by record.
                if s.txid.is_none() {
                    s.txid = s.prop(0x80);
                }
                if let Some(id) = spid {
                    out.insert(id, s);
                    ids.push(id);
                }
            }
            _ if r.ver == 0xF => ids.extend(shapes(r.body, out, order)),
            _ => {}
        }
    }
    ids
}

fn read_props(r: &Rec, s: &mut Shape) {
    let n = r.inst as usize;
    let mut complex_at = n * 6;
    for k in 0..n {
        let (Some(opid), Some(op)) = (u16_at(r.body, k * 6), u32_at(r.body, k * 6 + 2)) else {
            break;
        };
        let id = opid & 0x3FFF;
        if opid & 0x8000 != 0 {
            // Arrays (`IMsoArray`) whose length leaves out their 6-byte
            // header, as Word and LibreOffice write some.
            let mut len = op as usize;
            if let (Some(n), Some(cb)) =
                (u16_at(r.body, complex_at), u16_at(r.body, complex_at + 4))
            {
                let cb = if cb == 0xFFF0 { 4 } else { cb as usize };
                if len > 0 && n as usize * cb == len {
                    len += 6;
                }
            }
            if let Some(data) = r.body.get(complex_at..complex_at.saturating_add(len)) {
                s.complex.entry(id).or_insert_with(|| data.to_vec());
            }
            complex_at = complex_at.saturating_add(len);
        } else {
            s.props.entry(id).or_insert(op);
        }
    }
}

/// An `OfficeArtCOLORREF` as `0xRRGGBB` (`None` for scheme or system
/// colours).
pub(crate) fn color(v: u32) -> Option<u32> {
    if v >> 24 & 0x18 != 0 {
        return None;
    }
    let (r, g, b) = (v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF);
    Some(r << 16 | g << 8 | b)
}

/// The DrawingML preset of a shape type.
pub(crate) fn preset(typ: u16) -> Option<&'static str> {
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
pub(crate) fn cust_geom(s: &Shape) -> Option<String> {
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
    Some(format!(
        "<a:custGeom><a:avLst/><a:gdLst/><a:ahLst/><a:cxnLst/><a:rect l=\"0\" t=\"0\" r=\"r\" b=\"b\"/><a:pathLst><a:path w=\"{}\" h=\"{}\"{fill}>{path}</a:path></a:pathLst></a:custGeom>",
        right - left,
        bottom - top
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
fn guides(s: &Shape, [l, t, r, b]: [i64; 4]) -> Vec<i64> {
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

pub(crate) fn arrowhead(v: u32) -> Option<&'static str> {
    Some(match v {
        1 => "triangle",
        2 => "stealth",
        3 => "diamond",
        4 => "oval",
        5 => "arrow",
        _ => return None,
    })
}

/// `PlcftxbxTxt`: each text box's CP range in the text box story (the
/// last entry is a guard) and the shape id it belongs to.
pub(crate) fn text_boxes(plc: &[u8]) -> Vec<(u32, u32, u32)> {
    let mut out = Vec::new();
    if plc.len() < 4 {
        return out;
    }
    let n = (plc.len() - 4) / 26;
    for k in 0..n {
        let (Some(a), Some(b)) = (u32_at(plc, k * 4), u32_at(plc, k * 4 + 4)) else {
            break;
        };
        let lid = u32_at(plc, (n + 1) * 4 + k * 22 + 14).unwrap_or(0);
        if b > a {
            out.push((a, b, lid));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_flags() {
        assert_eq!(color(0x00FF_EBDD), Some(0xDDEBFF));
        assert_eq!(color(0x0800_0001), None);
        let mut s = Shape::default();
        s.props.insert(0x1FF, 0x80008);
        assert_eq!(s.flag(0x1FF, 3), Some(true));
        s.props.insert(0x1BF, 0x100000);
        assert_eq!(s.flag(0x1BF, 4), Some(false));
    }

    #[test]
    fn freeform_path() {
        let mut s = Shape::default();
        // Three i16 points: (0,0), (100,0), (100,50); segments move, 2 lines, close, end.
        let mut v = vec![3, 0, 3, 0, 0xF0, 0xFF];
        for (x, y) in [(0i16, 0i16), (100, 0), (100, 50)] {
            v.extend(x.to_le_bytes());
            v.extend(y.to_le_bytes());
        }
        s.complex.insert(0x145, v);
        let mut g = vec![4, 0, 4, 0, 2, 0];
        for seg in [0x4000u16, 0x0002, 0x6001, 0x8000] {
            g.extend(seg.to_le_bytes());
        }
        s.complex.insert(0x146, g);
        s.props.insert(0x142, 0);
        s.props.insert(0x143, 0);
        let x = cust_geom(&s).unwrap();
        assert!(x.contains("<a:path w=\"100\" h=\"50\">"), "{x}");
        assert!(
            x.contains("<a:moveTo><a:pt x=\"0\" y=\"0\"/></a:moveTo><a:lnTo>"),
            "{x}"
        );
        assert!(x.contains("<a:close/>"), "{x}");
    }
}
