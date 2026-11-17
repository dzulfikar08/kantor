//! QuickDraw PICT pictures (versions 1 and 2): lines, rectangles, round
//! rectangles, ovals, arcs, polygons, text, bitmaps and pixel maps
//! (packed or not) and QuickTime JPEG pictures, as Office writes them.
//! Regions are drawn as their bounding boxes; patterns as their average
//! colour.

use crate::Picture;
use crate::canvas::{Paint, Path, Pt, Stroke, Svg, Text, Xf, arc_points};
use crate::dib::Rgba;
use crate::rd::{be16, be32};

fn bi16(b: &[u8], o: usize) -> f64 {
    be16(b, o) as i16 as f64
}

/// Where the picture starts (after the optional 512-byte file header) and
/// whether it is version 2.
fn start(b: &[u8]) -> Option<(usize, bool)> {
    for base in [0usize, 512] {
        if b.len() < base + 12 {
            continue;
        }
        let v = &b[base + 10..];
        if v.starts_with(&[0x00, 0x11, 0x02, 0xFF]) {
            return Some((base, true));
        }
        if v.starts_with(&[0x11, 0x01]) {
            return Some((base, false));
        }
    }
    None
}

pub(crate) fn is_pict(b: &[u8]) -> bool {
    start(b).is_some()
}

struct State {
    fg: [u8; 3],
    bk: [u8; 3],
    pen: Pt,
    pen_pat: f64,
    fill_pat: f64,
    loc: Pt,
    origin: Pt,
    oval: Pt,
    last_rect: (f64, f64, f64, f64),
    last_rrect: (f64, f64, f64, f64),
    last_oval: (f64, f64, f64, f64),
    last_arc: (f64, f64, f64, f64),
    last_poly: Vec<Pt>,
    font: String,
    fonts: Vec<(u16, String)>,
    size: f64,
    face: u8,
    text_at: Pt,
}

fn old_color(v: u32) -> [u8; 3] {
    match v {
        30 => [255, 255, 255],
        205 => [221, 8, 6],
        341 => [0, 128, 17],
        409 => [0, 0, 212],
        273 => [2, 171, 234],
        137 => [242, 8, 132],
        69 => [252, 243, 5],
        _ => [0, 0, 0],
    }
}

fn rgb48(b: &[u8], o: usize) -> [u8; 3] {
    [
        (be16(b, o) >> 8) as u8,
        (be16(b, o + 2) >> 8) as u8,
        (be16(b, o + 4) >> 8) as u8,
    ]
}

/// How much of an 8×8 pattern is set (1.0 is the foreground colour).
fn density(b: &[u8], o: usize) -> f64 {
    let bits: u32 = (0..8)
        .map(|i| b.get(o + i).map_or(0, |v| v.count_ones()))
        .sum();
    bits as f64 / 64.0
}

fn mix(fg: [u8; 3], bk: [u8; 3], d: f64) -> [u8; 3] {
    let m = |a: u8, b: u8| (a as f64 * d + b as f64 * (1.0 - d)).round() as u8;
    [m(fg[0], bk[0]), m(fg[1], bk[1]), m(fg[2], bk[2])]
}

fn rect_at(b: &[u8], o: usize) -> (f64, f64, f64, f64) {
    // top, left, bottom, right → l, t, r, b
    (bi16(b, o + 2), bi16(b, o), bi16(b, o + 6), bi16(b, o + 4))
}

/// PackBits.
fn unpack(src: &[u8], want: usize, word: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(want);
    let mut i = 0;
    let unit = if word { 2 } else { 1 };
    while i < src.len() && out.len() < want {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let len = (n as usize + 1) * unit;
            out.extend_from_slice(src.get(i..i + len).unwrap_or(&src[i.min(src.len())..]));
            i += len;
        } else if n != -128 {
            let count = (1 - n as isize) as usize;
            let v = src.get(i..i + unit).unwrap_or(&[0, 0]).to_vec();
            for _ in 0..count {
                out.extend_from_slice(&v);
            }
            i += unit;
        }
    }
    out.truncate(want);
    out
}

pub(crate) fn play(b: &[u8]) -> Option<Picture> {
    let (base, v2) = start(b)?;
    let frame = rect_at(b, base + 2);
    let (fl, ft, fr, fb) = frame;
    if fr <= fl || fb <= ft {
        return None;
    }
    let (w_px, h_px) = ((fr - fl) * 96.0 / 72.0, (fb - ft) * 96.0 / 72.0);
    let mut svg = Svg::new(72.0 / 96.0);
    let mut st = State {
        fg: [0, 0, 0],
        bk: [255, 255, 255],
        pen: (1.0, 1.0),
        pen_pat: 1.0,
        fill_pat: 1.0,
        loc: (0.0, 0.0),
        origin: (0.0, 0.0),
        oval: (0.0, 0.0),
        last_rect: (0.0, 0.0, 0.0, 0.0),
        last_rrect: (0.0, 0.0, 0.0, 0.0),
        last_oval: (0.0, 0.0, 0.0, 0.0),
        last_arc: (0.0, 0.0, 0.0, 0.0),
        last_poly: Vec::new(),
        font: "Helvetica".into(),
        fonts: Vec::new(),
        size: 12.0,
        face: 0,
        text_at: (0.0, 0.0),
    };
    let mut at = base + 10;
    let mut guard = 0;
    while at < b.len() {
        guard += 1;
        if guard > 1_000_000 {
            break;
        }
        let op = if v2 {
            if at % 2 == 1 {
                at += 1;
            }
            let op = be16(b, at);
            at += 2;
            op
        } else {
            let op = b[at] as u16;
            at += 1;
            op
        };
        if op == 0x00FF {
            break;
        }
        let Some(len) = step(&mut svg, &mut st, b, at, op, v2) else {
            break;
        };
        at += len;
    }
    Some(Picture {
        svg: svg.finish([fl, ft, fr - fl, fb - ft], w_px, h_px),
        width: w_px,
        height: h_px,
        drawn: svg.drawn,
        has_text: svg.has_text,
    })
}

fn m(st: &State) -> Xf {
    Xf::translate(-st.origin.0, -st.origin.1)
}

/// Do one opcode (its data at `o`); returns its data length.
fn step(svg: &mut Svg, st: &mut State, b: &[u8], o: usize, op: u16, v2: bool) -> Option<usize> {
    let word = |n: usize| if v2 { n + (n & 1) } else { n };
    Some(match op {
        0x0000 | 0x001C | 0x001E => 0,
        0x0011 => {
            if v2 {
                2
            } else {
                1
            }
        }
        0x0C00 => 24,
        0x0001 => be16(b, o) as usize,
        0x0002 | 0x0009 | 0x000A => {
            match op {
                0x0009 => st.pen_pat = density(b, o),
                0x000A => st.fill_pat = density(b, o),
                _ => {}
            }
            8
        }
        0x0003 => {
            let id = be16(b, o);
            if let Some((_, name)) = st.fonts.iter().find(|(i, _)| *i == id) {
                st.font = name.clone();
            }
            2
        }
        0x0004 => {
            st.face = b.get(o).copied().unwrap_or(0);
            word(1)
        }
        0x0005 | 0x0008 | 0x0015 | 0x0016 => 2,
        0x000D => {
            st.size = be16(b, o).max(1) as f64;
            2
        }
        0x0006 => 4,
        0x0007 => {
            st.pen = (bi16(b, o + 2).max(0.0), bi16(b, o).max(0.0));
            4
        }
        0x000B => {
            st.oval = (bi16(b, o + 2), bi16(b, o));
            4
        }
        0x000C => {
            st.origin.0 += bi16(b, o + 2);
            st.origin.1 += bi16(b, o);
            4
        }
        0x000E => {
            st.fg = old_color(be32(b, o));
            4
        }
        0x000F => {
            st.bk = old_color(be32(b, o));
            4
        }
        0x0010 => 8,
        0x0012..=0x0014 => pix_pat_len(b, o)?,
        0x001A => {
            st.fg = rgb48(b, o);
            6
        }
        0x001B => {
            st.bk = rgb48(b, o);
            6
        }
        0x001D | 0x001F => 6,
        0x0017..=0x0019 => 0,
        0x0020 => {
            let from = (bi16(b, o + 2), bi16(b, o));
            let to = (bi16(b, o + 6), bi16(b, o + 4));
            line(svg, st, from, to);
            8
        }
        0x0021 => {
            let to = (bi16(b, o + 2), bi16(b, o));
            line(svg, st, st.loc, to);
            4
        }
        0x0022 => {
            let from = (bi16(b, o + 2), bi16(b, o));
            let dh = b.get(o + 4).copied().unwrap_or(0) as i8 as f64;
            let dv = b.get(o + 5).copied().unwrap_or(0) as i8 as f64;
            line(svg, st, from, (from.0 + dh, from.1 + dv));
            6
        }
        0x0023 => {
            let dh = b.get(o).copied().unwrap_or(0) as i8 as f64;
            let dv = b.get(o + 1).copied().unwrap_or(0) as i8 as f64;
            let from = st.loc;
            line(svg, st, from, (from.0 + dh, from.1 + dv));
            2
        }
        0x0028 => {
            st.text_at = (bi16(b, o + 2), bi16(b, o));
            let n = b.get(o + 4).copied().unwrap_or(0) as usize;
            text(svg, st, b.get(o + 5..o + 5 + n).unwrap_or(&[]));
            word(5 + n)
        }
        0x0029 | 0x002A => {
            let d = b.get(o).copied().unwrap_or(0) as f64;
            if op == 0x0029 {
                st.text_at.0 += d;
            } else {
                st.text_at.1 += d;
            }
            let n = b.get(o + 1).copied().unwrap_or(0) as usize;
            text(svg, st, b.get(o + 2..o + 2 + n).unwrap_or(&[]));
            word(2 + n)
        }
        0x002B => {
            st.text_at.0 += b.get(o).copied().unwrap_or(0) as f64;
            st.text_at.1 += b.get(o + 1).copied().unwrap_or(0) as f64;
            let n = b.get(o + 2).copied().unwrap_or(0) as usize;
            text(svg, st, b.get(o + 3..o + 3 + n).unwrap_or(&[]));
            word(3 + n)
        }
        0x002C => {
            let len = be16(b, o) as usize;
            let id = be16(b, o + 2);
            let n = b.get(o + 4).copied().unwrap_or(0) as usize;
            let name = crate::rd::ansi(b.get(o + 5..o + 5 + n).unwrap_or(&[]));
            st.fonts.push((id, name.clone()));
            st.font = name;
            word(2 + len)
        }
        0x0024..=0x0027
        | 0x002D..=0x002F
        | 0x0075..=0x0077
        | 0x0085..=0x0087
        | 0x0092..=0x0097
        | 0x009C..=0x009F
        | 0x00A2..=0x00AF => word(2 + be16(b, o) as usize),
        0x0030..=0x0034 => {
            st.last_rect = rect_at(b, o);
            shape(svg, st, op & 7, Shape::Rect(st.last_rect));
            8
        }
        0x0038..=0x003C => {
            shape(svg, st, op & 7, Shape::Rect(st.last_rect));
            0
        }
        0x0035..=0x0037 | 0x0045..=0x0047 | 0x0055..=0x0057 => 8,
        0x003D..=0x003F | 0x004D..=0x004F | 0x005D..=0x005F | 0x007D..=0x007F | 0x008D..=0x008F => {
            0
        }
        0x0040..=0x0044 => {
            st.last_rrect = rect_at(b, o);
            shape(svg, st, op & 7, Shape::RRect(st.last_rrect));
            8
        }
        0x0048..=0x004C => {
            shape(svg, st, op & 7, Shape::RRect(st.last_rrect));
            0
        }
        0x0050..=0x0054 => {
            st.last_oval = rect_at(b, o);
            shape(svg, st, op & 7, Shape::Oval(st.last_oval));
            8
        }
        0x0058..=0x005C => {
            shape(svg, st, op & 7, Shape::Oval(st.last_oval));
            0
        }
        0x0060..=0x0064 => {
            st.last_arc = rect_at(b, o);
            let (a0, a1) = (bi16(b, o + 8), bi16(b, o + 10));
            shape(svg, st, op & 7, Shape::Arc(st.last_arc, a0, a1));
            12
        }
        0x0068..=0x006C => {
            let (a0, a1) = (bi16(b, o), bi16(b, o + 2));
            shape(svg, st, op & 7, Shape::Arc(st.last_arc, a0, a1));
            4
        }
        0x0065..=0x0067 => 12,
        0x006D..=0x006F => 4,
        0x0070..=0x0074 => {
            let size = be16(b, o) as usize;
            let n = size.saturating_sub(10) / 4;
            st.last_poly = (0..n)
                .map(|i| (bi16(b, o + 10 + i * 4 + 2), bi16(b, o + 10 + i * 4)))
                .collect();
            shape(svg, st, op & 7, Shape::Poly(st.last_poly.clone()));
            word(size)
        }
        0x0078..=0x007C => {
            shape(svg, st, op & 7, Shape::Poly(st.last_poly.clone()));
            0
        }
        0x0080..=0x0084 => {
            let size = be16(b, o) as usize;
            st.last_rect = rect_at(b, o + 2);
            shape(svg, st, op & 7, Shape::Rect(st.last_rect));
            word(size)
        }
        0x0088..=0x008C => 0,
        0x0090 | 0x0091 | 0x0098 | 0x0099 | 0x009A | 0x009B => bits(svg, st, b, o, op)?,
        0x00A0 => 2,
        0x00A1 => word(4 + be16(b, o + 2) as usize),
        0x00B0..=0x00CF => 0,
        0x00D0..=0x00FE => 4 + be32(b, o) as usize,
        0x8000..=0x80FF => 0,
        0x8200 | 0x8201 => {
            let size = be32(b, o) as usize;
            if op == 0x8200 {
                quicktime(svg, st, b.get(o + 4..o + 4 + size).unwrap_or(&[]));
            }
            word(4 + size)
        }
        0x8100..=0xFFFF => 4 + be32(b, o) as usize,
        0x0100..=0x7FFF => (op as usize >> 8) * 2,
        _ => return None,
    })
}

fn pix_pat_len(b: &[u8], o: usize) -> Option<usize> {
    // PixPat: type, pat1Data(8), then for type 1 a PixMap, colour table and
    // packed data; Office rarely writes them: stop if we can't tell.
    if be16(b, o) == 1 { None } else { Some(10) }
}

fn line(svg: &mut Svg, st: &mut State, from: Pt, to: Pt) {
    st.loc = to;
    if st.pen.0 <= 0.0 || st.pen.1 <= 0.0 {
        return;
    }
    let mx = m(st);
    // QuickDraw's pen hangs below and to the right of the line.
    let (hx, hy) = (st.pen.0 / 2.0, st.pen.1 / 2.0);
    let mut p = Path::default();
    p.move_to(mx.apply((from.0 + hx, from.1 + hy)));
    p.line_to(mx.apply((to.0 + hx, to.1 + hy)));
    let stroke = Stroke {
        color: mix(st.fg, st.bk, st.pen_pat),
        alpha: 1.0,
        width: st.pen.0.max(st.pen.1),
        dash: Vec::new(),
        cap: "square",
        join: "miter",
        miter: 4.0,
    };
    svg.path(&p, None, false, Some(&stroke));
}

enum Shape {
    Rect((f64, f64, f64, f64)),
    RRect((f64, f64, f64, f64)),
    Oval((f64, f64, f64, f64)),
    Arc((f64, f64, f64, f64), f64, f64),
    Poly(Vec<Pt>),
}

/// verb: 0 frame, 1 paint, 2 erase, 3 invert, 4 fill.
fn shape(svg: &mut Svg, st: &State, verb: u16, s: Shape) {
    if verb == 3 {
        return;
    }
    let mx = m(st);
    let inset = if verb == 0 {
        (st.pen.0 / 2.0, st.pen.1 / 2.0)
    } else {
        (0.0, 0.0)
    };
    let shrink =
        |(l, t, r, b): (f64, f64, f64, f64)| (l + inset.0, t + inset.1, r - inset.0, b - inset.1);
    let path = match &s {
        Shape::Rect(r) => Path::rect(shrink(*r), &mx),
        Shape::RRect(r) => Path::round_rect(shrink(*r), st.oval, &mx),
        Shape::Oval(r) => Path::ellipse(shrink(*r), &mx),
        Shape::Arc(r, a0, a1) => {
            let (l, t, rr, bb) = shrink(*r);
            let (cx, cy) = ((l + rr) / 2.0, (t + bb) / 2.0);
            // QuickDraw angles: clockwise from 12 o'clock.
            let start = (a0 - 90.0).to_radians();
            let sweep = a1.to_radians();
            let mut p = Path::default();
            if verb != 0 {
                p.move_to(mx.apply((cx, cy)));
            }
            arc_points(
                &mut p,
                (cx, cy),
                ((rr - l) / 2.0, (bb - t) / 2.0),
                start,
                sweep,
                &mx,
                verb == 0,
            );
            if verb != 0 {
                p.close();
            }
            p
        }
        Shape::Poly(pts) => Path::poly(pts, &mx, verb != 0),
    };
    match verb {
        0 => {
            if st.pen.0 > 0.0 && st.pen.1 > 0.0 {
                let stroke = Stroke {
                    color: mix(st.fg, st.bk, st.pen_pat),
                    alpha: 1.0,
                    width: st.pen.0.max(st.pen.1),
                    dash: Vec::new(),
                    cap: "butt",
                    join: "miter",
                    miter: 4.0,
                };
                svg.path(&path, None, false, Some(&stroke));
            }
        }
        1 => svg.path(
            &path,
            Some(&Paint::Color(mix(st.fg, st.bk, st.pen_pat), 1.0)),
            false,
            None,
        ),
        2 => svg.path(&path, Some(&Paint::Color(st.bk, 1.0)), false, None),
        _ => svg.path(
            &path,
            Some(&Paint::Color(mix(st.fg, st.bk, st.fill_pat), 1.0)),
            false,
            None,
        ),
    }
}

fn text(svg: &mut Svg, st: &State, bytes: &[u8]) {
    let s = mac_roman(bytes);
    let p = m(st).apply(st.text_at);
    svg.text(&Text {
        pos: p,
        xs: None,
        anchor: "start",
        rotate: 0.0,
        pivot: p,
        size: st.size,
        family: &st.font,
        weight: if st.face & 1 != 0 { 700 } else { 400 },
        italic: st.face & 2 != 0,
        underline: st.face & 4 != 0,
        strike: false,
        color: st.fg,
        alpha: 1.0,
        text: &s,
        stretch: 1.0,
    });
}

fn mac_roman(b: &[u8]) -> String {
    const HIGH: &str = "ÄÅÇÉÑÖÜáàâäãåçéèêëíìîïñóòôöõúùûü†°¢£§•¶ß®©™´¨≠ÆØ∞±≤≥¥µ∂∑∏π∫ªºΩæø¿¡¬√ƒ≈∆«»… ÀÃÕŒœ–—“”‘’÷◊ÿŸ⁄€‹›ﬁﬂ‡·‚„‰ÂÊÁËÈÍÎÏÌÓÔ\u{F8FF}ÒÚÛÙıˆ˜¯˘˙˚¸˝˛ˇ";
    b.iter()
        .map(|&c| match c {
            0x0D => ' ',
            0x80..=0xFF => HIGH.chars().nth((c - 0x80) as usize).unwrap_or('?'),
            c => c as char,
        })
        .collect()
}

/// BitsRect/BitsRgn/PackBitsRect/PackBitsRgn/DirectBitsRect/DirectBitsRgn.
fn bits(svg: &mut Svg, st: &State, b: &[u8], o: usize, op: u16) -> Option<usize> {
    let direct = op == 0x009A || op == 0x009B;
    let mut at = o;
    if direct {
        at += 4; // baseAddr
    }
    let row_bytes_raw = be16(b, at);
    let pixmap = row_bytes_raw & 0x8000 != 0 || direct;
    let row_bytes = (row_bytes_raw & 0x3FFF) as usize;
    let bounds = rect_at(b, at + 2);
    at += 10;
    let (mut pack_type, mut pixel_size, mut cmp_count) = (0u16, 1u16, 1u16);
    let mut palette: Vec<[u8; 3]> = Vec::new();
    if pixmap {
        pack_type = be16(b, at + 2);
        pixel_size = be16(b, at + 18);
        cmp_count = be16(b, at + 20);
        at += 36;
        if !direct {
            let size = be16(b, at + 6) as usize + 1;
            for i in 0..size.min(256) {
                let e = at + 8 + i * 8;
                let idx = be16(b, e) as usize;
                let c = rgb48(b, e + 2);
                let pos = if be16(b, at + 4) & 0x8000 != 0 {
                    i
                } else {
                    idx
                };
                if palette.len() <= pos {
                    palette.resize(pos + 1, [0, 0, 0]);
                }
                palette[pos] = c;
            }
            at += 8 + size * 8;
        }
    } else {
        palette = vec![st.bk, st.fg];
    }
    let src = rect_at(b, at);
    let dst = rect_at(b, at + 8);
    at += 18; // src, dst, mode
    if op & 1 == 1 {
        at += be16(b, at) as usize;
    }
    let (bl, bt, br, bb) = bounds;
    let w = (br - bl).max(0.0) as usize;
    let h = (bb - bt).max(0.0) as usize;
    if w == 0 || h == 0 || w * h > 64 * 1024 * 1024 {
        return None;
    }
    let row_bytes = if row_bytes == 0 {
        w * pixel_size as usize / 8
    } else {
        row_bytes
    };
    let mut px = Vec::with_capacity(w * h);
    for _ in 0..h {
        let (row, used) =
            if row_bytes < 8 || (pixmap && (pack_type == 1 || (direct && pack_type == 2))) {
                let len = if direct && pack_type == 2 {
                    w * 3
                } else {
                    row_bytes
                };
                (b.get(at..at + len)?.to_vec(), len)
            } else {
                let (count, hdr) = if row_bytes > 250 {
                    (be16(b, at) as usize, 2)
                } else {
                    (b.get(at).copied()? as usize, 1)
                };
                let data = b.get(at + hdr..at + hdr + count)?;
                let want = if direct && pack_type == 4 {
                    w * cmp_count as usize
                } else {
                    row_bytes
                };
                (
                    unpack(data, want, pixel_size == 16 && pack_type != 4),
                    hdr + count,
                )
            };
        at += used;
        for x in 0..w {
            let c: [u8; 4] = match (direct, pixel_size) {
                (true, 32) | (true, 24) if pack_type == 4 => {
                    let plane = |k: usize| row.get(k * w + x).copied().unwrap_or(0);
                    if cmp_count >= 4 {
                        [plane(1), plane(2), plane(3), 255]
                    } else {
                        [plane(0), plane(1), plane(2), 255]
                    }
                }
                (true, 32) => {
                    let o = x * 4;
                    [
                        row.get(o + 1).copied().unwrap_or(0),
                        row.get(o + 2).copied().unwrap_or(0),
                        row.get(o + 3).copied().unwrap_or(0),
                        255,
                    ]
                }
                (true, 24) => {
                    let o = x * 3;
                    [
                        row.get(o).copied().unwrap_or(0),
                        row.get(o + 1).copied().unwrap_or(0),
                        row.get(o + 2).copied().unwrap_or(0),
                        255,
                    ]
                }
                (_, 16) => {
                    let v = ((row.get(x * 2).copied().unwrap_or(0) as u16) << 8)
                        | row.get(x * 2 + 1).copied().unwrap_or(0) as u16;
                    let f = |s: u16| ((((v >> s) & 31) as u32) * 255 / 31) as u8;
                    [f(10), f(5), f(0), 255]
                }
                (_, bpp @ (1 | 2 | 4 | 8)) => {
                    let bit = x * bpp as usize;
                    let byte = row.get(bit / 8).copied().unwrap_or(0);
                    let shift = 8 - bpp as usize - bit % 8;
                    let i = ((byte >> shift) as usize) & ((1 << bpp) - 1);
                    let c = palette.get(i).copied().unwrap_or(if i == 0 {
                        [255, 255, 255]
                    } else {
                        [0, 0, 0]
                    });
                    [c[0], c[1], c[2], 255]
                }
                _ => [0, 0, 0, 255],
            };
            px.push(c);
        }
    }
    let img = Rgba {
        w: w as u32,
        h: h as u32,
        px,
    };
    let (sl, stt, sr, sb) = src;
    let part = img
        .crop(
            (sl - bl) as i64,
            (stt - bt) as i64,
            (sr - sl) as i64,
            (sb - stt) as i64,
        )
        .unwrap_or(img);
    if let Some(png) = part.png() {
        let mx = m(st);
        let (dl, dt, dr, db) = dst;
        let p0 = mx.apply((dl, dt));
        let unit = Xf([dr - dl, 0.0, 0.0, db - dt, p0.0, p0.1]);
        svg.image(&png, &unit, None, 1.0);
    }
    Some(at - o)
}

/// A QuickTime-compressed picture: JPEG data put on its source rectangle.
fn quicktime(svg: &mut Svg, st: &State, d: &[u8]) {
    // version(2) matrix(36) matteSize(4) matteRect(8) mode(2) srcRect(8)
    let fixed = |o: usize| be32(d, o) as i32 as f64 / 65536.0;
    let (a, dd) = (fixed(2), fixed(2 + 16));
    let (tx, ty) = (fixed(2 + 24), fixed(2 + 28));
    let src = rect_at(d, 52);
    let Some(soi) = d.windows(3).position(|w| w == [0xFF, 0xD8, 0xFF]) else {
        return;
    };
    let jpeg = &d[soi..];
    let (l, t, r, b) = src;
    let (a, dd) = (
        if a == 0.0 { 1.0 } else { a },
        if dd == 0.0 { 1.0 } else { dd },
    );
    let mx = m(st);
    let p0 = mx.apply((l * a + tx, t * dd + ty));
    let unit = Xf([(r - l) * a, 0.0, 0.0, (b - t) * dd, p0.0, p0.1]);
    svg.image(jpeg, &unit, None, 1.0);
}
