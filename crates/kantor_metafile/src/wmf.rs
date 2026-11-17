//! Windows Metafiles ([MS-WMF]).

use crate::Picture;
use crate::canvas::{Path, Pt};
use crate::dib;
use crate::gdi::{ArcKind, Brush, Font, Gdi, Mapping, Obj, Pen};
use crate::rd::colorref as colorref_at;
use crate::rd::{ansi, i16_at, u8_at, u16_at, u32_at};

const PLACEABLE: u32 = 0x9AC6_CDD7;

/// Where the records start, and the placeable header's frame (logical
/// units) and units per inch.
/// A placeable header: the frame and units per inch.
type Placeable = ([f64; 4], f64);

fn header(b: &[u8]) -> Option<(usize, Option<Placeable>)> {
    let (start, placeable) = if u32_at(b, 0) == PLACEABLE {
        let l = i16_at(b, 6) as f64;
        let t = i16_at(b, 8) as f64;
        let r = i16_at(b, 10) as f64;
        let bt = i16_at(b, 12) as f64;
        let inch = u16_at(b, 14) as f64;
        let frame = [l.min(r), t.min(bt), (r - l).abs(), (bt - t).abs()];
        (22, Some((frame, if inch > 0.0 { inch } else { 1440.0 })))
    } else {
        (0, None)
    };
    let ty = u16_at(b, start);
    let hsize = u16_at(b, start + 2);
    if !(ty == 1 || ty == 2) || hsize != 9 {
        return None;
    }
    Some((start + 18, placeable))
}

pub(crate) fn is_wmf(b: &[u8]) -> bool {
    header(b).is_some()
}

/// For files without a placeable header: the first window the picture
/// sets up is its frame.
fn first_window(b: &[u8], mut at: usize) -> Option<[f64; 4]> {
    let (mut org, mut ext) = (None, None);
    while at + 6 <= b.len() && (org.is_none() || ext.is_none()) {
        let size = u32_at(b, at) as usize * 2;
        let func = u16_at(b, at + 4);
        if size < 6 || func == 0 {
            break;
        }
        let p = at + 6;
        match func {
            0x020B if org.is_none() => org = Some((i16_at(b, p + 2) as f64, i16_at(b, p) as f64)),
            0x020C if ext.is_none() => ext = Some((i16_at(b, p + 2) as f64, i16_at(b, p) as f64)),
            _ => {}
        }
        at += size;
    }
    let (w, h): Pt = ext?;
    let (ox, oy) = org.unwrap_or((0.0, 0.0));
    Some([ox.min(ox + w), oy.min(oy + h), w.abs(), h.abs()])
}

pub(crate) fn play(b: &[u8]) -> Option<Picture> {
    let (start, placeable) = header(b)?;
    let (frame, inch) = match placeable {
        Some((f, inch)) if f[2] > 0.0 && f[3] > 0.0 => (f, inch),
        _ => (first_window(b, start)?, 1440.0),
    };
    if frame[2] <= 0.0 || frame[3] <= 0.0 {
        return None;
    }
    // Natural size: placeable files give units per inch; others are
    // assumed to be in twips (MM_ANISOTROPIC files usually are).
    let mut w_px = frame[2] / inch * 96.0;
    let mut h_px = frame[3] / inch * 96.0;
    if placeable.is_none() && (w_px < 8.0 || h_px < 8.0 || w_px > 20_000.0) {
        let s = 400.0 / frame[2].max(frame[3]);
        w_px = frame[2] * s;
        h_px = frame[3] * s;
    }
    let unit = frame[2] / w_px.max(1e-9);
    let mut g = Gdi::new(Mapping::Wmf { frame }, unit);
    g.huge = [
        frame[0] - frame[2] * 100.0,
        frame[1] - frame[3] * 100.0,
        frame[2] * 201.0,
        frame[3] * 201.0,
    ];
    // WMF's defaults: a white brush, black pen, transparent text.
    let mut at = start;
    let mut guard = 0usize;
    while at + 6 <= b.len() {
        guard += 1;
        if guard > 2_000_000 {
            break;
        }
        let size = u32_at(b, at) as usize * 2;
        let func = u16_at(b, at + 4);
        if func == 0 {
            break;
        }
        if size < 6 {
            break;
        }
        let rec = &b[at..(at + size).min(b.len())];
        if func != 0x0213 && func != 0x0214 {
            g.flush();
        }
        record(&mut g, func, rec);
        at += size;
    }
    g.flush();
    Some(Picture {
        svg: g.svg.finish(frame, w_px, h_px),
        width: w_px,
        height: h_px,
        drawn: g.svg.drawn,
        has_text: g.svg.has_text,
    })
}

fn rect_rev(p: &[u8], o: usize) -> (f64, f64, f64, f64) {
    // bottom, right, top, left
    let b = i16_at(p, o) as f64;
    let r = i16_at(p, o + 2) as f64;
    let t = i16_at(p, o + 4) as f64;
    let l = i16_at(p, o + 6) as f64;
    (l, t, r, b)
}

fn points(p: &[u8], o: usize, n: usize) -> Vec<Pt> {
    (0..n)
        .map(|i| (i16_at(p, o + i * 4) as f64, i16_at(p, o + i * 4 + 2) as f64))
        .collect()
}

fn record(g: &mut Gdi, func: u16, rec: &[u8]) {
    let p = rec.get(6..).unwrap_or(&[]);
    match func {
        0x0201 => g.dc.bk_color = colorref_at(p, 0),
        0x0102 => g.dc.bk_mode = u16_at(p, 0) as u32,
        0x0103 => g.dc.map_mode = u16_at(p, 0) as u32,
        0x0106 => g.dc.fill_mode = u16_at(p, 0) as u32,
        0x0209 => g.dc.text_color = colorref_at(p, 0),
        0x012E => g.dc.text_align = u16_at(p, 0) as u32,
        0x020B => g.dc.win_org = (i16_at(p, 2) as f64, i16_at(p, 0) as f64),
        0x020C => g.dc.win_ext = Some((i16_at(p, 2) as f64, i16_at(p, 0) as f64)),
        0x020F => {
            g.dc.win_org.0 += i16_at(p, 2) as f64;
            g.dc.win_org.1 += i16_at(p, 0) as f64;
        }
        0x0410 => {
            // ScaleWindowExt: yDenom, yNum, xDenom, xNum
            if let Some((x, y)) = g.dc.win_ext {
                let yd = i16_at(p, 0) as f64;
                let yn = i16_at(p, 2) as f64;
                let xd = i16_at(p, 4) as f64;
                let xn = i16_at(p, 6) as f64;
                if xd != 0.0 && yd != 0.0 {
                    g.dc.win_ext = Some((x * xn / xd, y * yn / yd));
                }
            }
        }
        0x020D => g.dc.vp_org = (i16_at(p, 2) as f64, i16_at(p, 0) as f64),
        0x020E => g.dc.vp_ext = Some((i16_at(p, 2) as f64, i16_at(p, 0) as f64)),
        0x0214 => g.move_to((i16_at(p, 2) as f64, i16_at(p, 0) as f64)),
        0x0213 => g.line_to((i16_at(p, 2) as f64, i16_at(p, 0) as f64)),
        0x0415 => g.exclude_clip_rect(rect_rev(p, 0)),
        0x0416 => g.intersect_clip_rect(rect_rev(p, 0)),
        0x012C => g.clip_reset(),
        0x0418 => {
            let r = rect_rev(p, 0);
            let path = Path::ellipse(r, &g.xf());
            g.shape(path, true, true);
        }
        0x041B => {
            let r = rect_rev(p, 0);
            let path = Path::rect(r, &g.xf());
            g.shape(path, true, true);
        }
        0x061C => {
            let h = i16_at(p, 0) as f64;
            let w = i16_at(p, 2) as f64;
            let r = rect_rev(p, 4);
            let path = Path::round_rect(r, (w, h), &g.xf());
            g.shape(path, true, true);
        }
        0x0817 | 0x081A | 0x0830 => {
            let e = (i16_at(p, 2) as f64, i16_at(p, 0) as f64);
            let s = (i16_at(p, 6) as f64, i16_at(p, 4) as f64);
            let r = rect_rev(p, 8);
            let kind = match func {
                0x0817 => ArcKind::Arc,
                0x081A => ArcKind::Pie,
                _ => ArcKind::Chord,
            };
            g.arc(r, s, e, kind);
        }
        0x0324 | 0x0325 => {
            let n = u16_at(p, 0) as usize;
            let pts = points(p, 2, n);
            let path = Path::poly(&pts, &g.xf(), func == 0x0324);
            if func == 0x0324 {
                g.shape(path, true, true);
            } else {
                g.open_figure(path, false);
            }
        }
        0x0538 => {
            let n = u16_at(p, 0) as usize;
            let mut o = 2 + n * 2;
            let mut path = Path::default();
            let m = g.xf();
            for i in 0..n {
                let c = u16_at(p, 2 + i * 2) as usize;
                path.append(&Path::poly(&points(p, o, c), &m, true));
                o += c * 4;
            }
            g.shape(path, true, true);
        }
        0x0521 => {
            let n = u16_at(p, 0) as usize;
            let s = p.get(2..2 + n).unwrap_or(&[]);
            let o = 2 + n + (n & 1);
            let y = i16_at(p, o) as f64;
            let x = i16_at(p, o + 2) as f64;
            g.text((x, y), &ansi(s), None, None);
        }
        0x0A32 => {
            let y = i16_at(p, 0) as f64;
            let x = i16_at(p, 2) as f64;
            let n = u16_at(p, 4) as usize;
            let opts = u16_at(p, 6);
            let mut o = 8;
            let mut rect = None;
            if opts & 0x0006 != 0 && p.len() >= o + 8 {
                let l = i16_at(p, o) as f64;
                let t = i16_at(p, o + 2) as f64;
                let r = i16_at(p, o + 4) as f64;
                let bt = i16_at(p, o + 6) as f64;
                if opts & 0x0002 != 0 {
                    rect = Some((l, t, r, bt));
                }
                o += 8;
            }
            let s = p.get(o..o + n).unwrap_or(&[]);
            o += n + (n & 1);
            let dx = if p.len() >= o + n * 2 && n > 0 {
                Some((0..n).map(|i| i16_at(p, o + i * 2) as f64).collect())
            } else {
                None
            };
            g.text((x, y), &ansi(s), dx, rect);
        }
        0x02FA => {
            let style = u16_at(p, 0) as u32;
            let width = i16_at(p, 2) as f64;
            g.add_lowest(Obj::Pen(Pen {
                style,
                width: width.abs(),
                color: colorref_at(p, 6),
                geometric: true,
                user_dash: Vec::new(),
            }));
        }
        0x02FC => {
            let style = u16_at(p, 0);
            let color = colorref_at(p, 2);
            let hatch = u16_at(p, 6) as u32;
            let brush = match style {
                1 => Brush::Null,
                2 => Brush::Hatch(hatch, color),
                _ => Brush::Solid(color),
            };
            g.add_lowest(Obj::Brush(brush));
        }
        0x0142 => {
            // DIBCreatePatternBrush: style, usage, DIB
            let brush = dib::decode_packed(p.get(4..).unwrap_or(&[]), false)
                .and_then(|img| Some(Brush::Tile(img.png()?.into(), img.w, img.h)))
                .unwrap_or(Brush::Solid([128, 128, 128]));
            g.add_lowest(Obj::Brush(brush));
        }
        0x02FB => {
            let face_bytes = p.get(18..50).unwrap_or(&[]);
            let end = face_bytes
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(face_bytes.len());
            g.add_lowest(Obj::Font(Font {
                height: i16_at(p, 0) as f64,
                width: i16_at(p, 2) as f64,
                escapement: i16_at(p, 4) as f64,
                weight: i16_at(p, 8) as i32,
                italic: u8_at(p, 10) != 0,
                underline: u8_at(p, 11) != 0,
                strike: u8_at(p, 12) != 0,
                charset: u8_at(p, 13),
                face: ansi(&face_bytes[..end]),
            }));
        }
        0x00F7 | 0x01F9 | 0x06FF => g.add_lowest(Obj::Other),
        0x012D => g.select_index(u16_at(p, 0) as usize),
        0x01F0 => g.delete(u16_at(p, 0) as usize),
        0x001E => g.save(),
        0x0127 => g.restore(i16_at(p, 0) as i32),
        0x061D => {
            let rop = u32_at(p, 0);
            let h = i16_at(p, 4) as f64;
            let w = i16_at(p, 6) as f64;
            let y = i16_at(p, 8) as f64;
            let x = i16_at(p, 10) as f64;
            g.pat_blt((x, y, w, h), rop);
        }
        0x0F43 => {
            // StretchDIB: rop, usage, srcH, srcW, ySrc, xSrc, dstH, dstW, yDst, xDst, DIB
            let rop = u32_at(p, 0);
            let sh = i16_at(p, 6) as i64;
            let sw = i16_at(p, 8) as i64;
            let sy = i16_at(p, 10) as i64;
            let sx = i16_at(p, 12) as i64;
            let dh = i16_at(p, 14) as f64;
            let dw = i16_at(p, 16) as f64;
            let dy = i16_at(p, 18) as f64;
            let dx = i16_at(p, 20) as f64;
            blit(
                g,
                p.get(22..).unwrap_or(&[]),
                (sx, sy, sw, sh),
                (dx, dy, dw, dh),
                rop,
                true,
            );
        }
        0x0B41 | 0x0940 => {
            let rop = u32_at(p, 0);
            let stretch = func == 0x0B41;
            let words = rec.len() / 2;
            // Without a bitmap the record is a pattern fill.
            let no_bitmap = words == (func as usize >> 8) + 3;
            let (src, dest, bits_at) = if stretch {
                let sh = i16_at(p, 4) as i64;
                let sw = i16_at(p, 6) as i64;
                let sy = i16_at(p, 8) as i64;
                let sx = i16_at(p, 10) as i64;
                let o = if no_bitmap { 14 } else { 12 };
                let dh = i16_at(p, o) as f64;
                let dw = i16_at(p, o + 2) as f64;
                let dy = i16_at(p, o + 4) as f64;
                let dx = i16_at(p, o + 6) as f64;
                ((sx, sy, sw, sh), (dx, dy, dw, dh), o + 8)
            } else {
                let sy = i16_at(p, 4) as i64;
                let sx = i16_at(p, 6) as i64;
                let o = if no_bitmap { 10 } else { 8 };
                let h = i16_at(p, o) as f64;
                let w = i16_at(p, o + 2) as f64;
                let dy = i16_at(p, o + 4) as f64;
                let dx = i16_at(p, o + 6) as f64;
                (
                    (sx, sy, w.abs() as i64, h.abs() as i64),
                    (dx, dy, w, h),
                    o + 8,
                )
            };
            if no_bitmap {
                g.pat_blt(dest, rop);
            } else {
                blit(g, p.get(bits_at..).unwrap_or(&[]), src, dest, rop, stretch);
            }
        }
        0x0D33 => {
            // SetDIBitsToDevice: usage, scans, startScan, yDib, xDib, h, w, yDest, xDest, DIB
            let h = i16_at(p, 10) as f64;
            let w = i16_at(p, 12) as f64;
            let dy = i16_at(p, 14) as f64;
            let dx = i16_at(p, 16) as f64;
            let ys = i16_at(p, 6) as i64;
            let xs = i16_at(p, 8) as i64;
            blit(
                g,
                p.get(18..).unwrap_or(&[]),
                (xs, ys, w as i64, h as i64),
                (dx, dy, w, h),
                0x00CC_0020,
                false,
            );
        }
        0x0228 => {
            // FillRegion: region, brush — regions aren't kept.
        }
        _ => {}
    }
}

/// A DIB from a blit record onto `dest`; `src` is in the bitmap's pixels
/// (origin at the bottom-left for bottom-up bitmaps).
fn blit(
    g: &mut Gdi,
    packed: &[u8],
    src: (i64, i64, i64, i64),
    dest: (f64, f64, f64, f64),
    rop: u32,
    _stretch: bool,
) {
    if let Some(pic) = dib::embedded(packed) {
        g.picture_bytes(pic, dest);
        return;
    }
    let Some(img) = dib::decode_packed(packed, false) else {
        return;
    };
    let (sx, sy, sw, sh) = src;
    let bottom_up = dib::bottom_up(packed);
    let top = if bottom_up {
        img.h as i64 - sy - sh.abs()
    } else {
        sy
    };
    let part = if sw > 0 && sh != 0 {
        img.crop(sx, top, sw, sh.abs())
    } else {
        None
    };
    g.bitmap(dest, part.as_ref().unwrap_or(&img), rop);
}
