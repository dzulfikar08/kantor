//! Enhanced Metafiles ([MS-EMF]), with the EMF+ records they may carry.

use crate::Picture;
use crate::canvas::{Paint, Path, Pt, Xf};
use crate::dib;
use crate::emfplus::Plus;
use crate::gdi::{ArcKind, Brush, Font, Gdi, Mapping, Obj, Pen, stock};
use crate::rd::{colorref, f32_at, i16_at, i32_at, u8_at, u16_at, u32_at, utf16};

pub(crate) fn is_emf(b: &[u8]) -> bool {
    u32_at(b, 0) == 1 && u32_at(b, 40) == 0x464D_4520
}

fn rectl(b: &[u8], o: usize) -> (f64, f64, f64, f64) {
    (
        i32_at(b, o) as f64,
        i32_at(b, o + 4) as f64,
        i32_at(b, o + 8) as f64,
        i32_at(b, o + 12) as f64,
    )
}

pub(crate) fn play(b: &[u8]) -> Option<Picture> {
    if !is_emf(b) {
        return None;
    }
    let bounds = rectl(b, 8);
    let frame = rectl(b, 24);
    let dev = (i32_at(b, 72) as f64, i32_at(b, 76) as f64);
    let mm = (i32_at(b, 80) as f64, i32_at(b, 84) as f64);
    let ppm = if dev.0 > 0.0 && dev.1 > 0.0 && mm.0 > 0.0 && mm.1 > 0.0 {
        (dev.0 / mm.0, dev.1 / mm.1)
    } else {
        (96.0 / 25.4, 96.0 / 25.4)
    };
    let (fl, ft, fr, fb) = frame;
    let (view, w_px, h_px) = if fr > fl && fb > ft {
        let view = [
            fl / 100.0 * ppm.0,
            ft / 100.0 * ppm.1,
            (fr - fl) / 100.0 * ppm.0,
            (fb - ft) / 100.0 * ppm.1,
        ];
        (view, (fr - fl) / 2540.0 * 96.0, (fb - ft) / 2540.0 * 96.0)
    } else {
        let (l, t, r, bt) = bounds;
        if r < l || bt < t {
            return None;
        }
        let view = [l, t, r - l + 1.0, bt - t + 1.0];
        let s = 96.0 / (ppm.0 * 25.4);
        (view, view[2] * s, view[3] * s)
    };
    if view[2] <= 0.0 || view[3] <= 0.0 || w_px <= 0.0 || h_px <= 0.0 {
        return None;
    }
    let unit = view[2] / w_px;
    let mut g = Gdi::new(Mapping::Emf { px_per_mm: ppm }, unit);
    g.huge = [
        view[0] - view[2] * 100.0,
        view[1] - view[3] * 100.0,
        view[2] * 201.0,
        view[3] * 201.0,
    ];
    let mut plus = Plus::new(ppm);
    let mut at = 0usize;
    let mut guard = 0usize;
    while at + 8 <= b.len() {
        guard += 1;
        if guard > 4_000_000 {
            break;
        }
        let ty = u32_at(b, at);
        let size = u32_at(b, at + 4) as usize;
        if size < 8 || !size.is_multiple_of(4) {
            break;
        }
        let rec = &b[at..(at + size).min(b.len())];
        if ty == 14 {
            break;
        }
        if ty == 70 && u32_at(rec, 12) == 0x2B46_4D45 {
            // EMF+ records in a comment.
            let data_len = u32_at(rec, 8) as usize;
            let data = rec.get(16..(12 + data_len).min(rec.len())).unwrap_or(&[]);
            g.flush();
            plus.records(&mut g, data);
        } else if !plus.only || plus.gdi_enabled {
            if ty != 54 && ty != 27 {
                g.flush();
            }
            record(&mut g, ty, rec);
        }
        at += size;
    }
    g.flush();
    Some(Picture {
        svg: g.svg.finish(view, w_px, h_px),
        width: w_px,
        height: h_px,
        drawn: g.svg.drawn,
        has_text: g.svg.has_text,
    })
}

fn points32(r: &[u8], o: usize, n: usize) -> Vec<Pt> {
    (0..n.min(r.len() / 8 + 1))
        .map(|i| (i32_at(r, o + i * 8) as f64, i32_at(r, o + i * 8 + 4) as f64))
        .collect()
}

fn points16(r: &[u8], o: usize, n: usize) -> Vec<Pt> {
    (0..n.min(r.len() / 4 + 1))
        .map(|i| (i16_at(r, o + i * 4) as f64, i16_at(r, o + i * 4 + 2) as f64))
        .collect()
}

fn xform(r: &[u8], o: usize) -> Xf {
    Xf([
        f32_at(r, o),
        f32_at(r, o + 4),
        f32_at(r, o + 8),
        f32_at(r, o + 12),
        f32_at(r, o + 16),
        f32_at(r, o + 20),
    ])
}

fn bitmap_of(
    r: &[u8],
    off_bmi: usize,
    cb_bmi: usize,
    off_bits: usize,
    cb_bits: usize,
) -> Option<(&[u8], &[u8])> {
    let info = r.get(off_bmi..off_bmi.checked_add(cb_bmi)?)?;
    let bits = r.get(off_bits..off_bits.checked_add(cb_bits)?.min(r.len()))?;
    Some((info, bits))
}

/// Draw a DIB from a blit record; `src` is in the bitmap's pixels from its
/// top-left (BitBlt's source DC) or bottom-left (`dib_origin`).
#[allow(clippy::too_many_arguments)]
fn blit(
    g: &mut Gdi,
    info: &[u8],
    bits: &[u8],
    src: (i64, i64, i64, i64),
    dest: (f64, f64, f64, f64),
    rop: u32,
    dib_origin: bool,
    alpha: Option<(u8, bool)>,
) {
    let compression = u32_at(info, 16);
    if u32_at(info, 0) >= 40 && (compression == 4 || compression == 5) {
        g.picture_bytes(bits, dest);
        return;
    }
    let keep_alpha = matches!(alpha, Some((_, true)));
    let Some(mut img) = dib::decode(info, bits, keep_alpha) else {
        return;
    };
    if keep_alpha {
        // AC_SRC_ALPHA bitmaps are premultiplied.
        for p in &mut img.px {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 / a).min(255)) as u8;
                }
            }
        }
    }
    let (sx, sy, sw, sh) = src;
    let top = if dib_origin && dib::bottom_up(info) {
        img.h as i64 - sy - sh.abs()
    } else {
        sy
    };
    let part = if sw > 0 && sh != 0 && (sx, top, sw, sh.abs()) != (0, 0, img.w as i64, img.h as i64)
    {
        img.crop(sx, top, sw, sh.abs())
    } else {
        None
    };
    let img = part.unwrap_or(img);
    match alpha {
        Some((constant, _)) if constant < 255 => {
            let mut faded = img;
            for p in &mut faded.px {
                p[3] = (p[3] as u32 * constant as u32 / 255) as u8;
            }
            g.bitmap(dest, &faded, 0x00CC_0020);
        }
        _ => g.bitmap(dest, &img, rop),
    }
}

fn region_path(r: &[u8], o: usize) -> Path {
    // RGNDATAHEADER: dwSize, iType, nCount, nRgnSize, rcBound
    let count = u32_at(r, o + 8) as usize;
    let mut p = Path::default();
    for i in 0..count.min(100_000) {
        let rect = rectl(r, o + 32 + i * 16);
        p.append(&Path::rect(rect, &Xf::ID));
    }
    p
}

fn record(g: &mut Gdi, ty: u32, r: &[u8]) {
    match ty {
        // Polygon/line families, 32-bit points.
        2..=6 => {
            let n = u32_at(r, 24) as usize;
            let pts = points32(r, 28, n);
            poly(g, ty, &pts);
        }
        85..=89 => {
            let n = u32_at(r, 24) as usize;
            let pts = points16(r, 28, n);
            poly(g, ty - 83, &pts);
        }
        7 | 8 | 90 | 91 => {
            let n = u32_at(r, 24) as usize;
            let total = u32_at(r, 28) as usize;
            let wide = ty == 7 || ty == 8;
            let mut o = 32 + n * 4;
            let m = g.xf();
            let closed = ty == 8 || ty == 91;
            let mut path = Path::default();
            for i in 0..n.min(100_000) {
                let c = u32_at(r, 32 + i * 4) as usize;
                let c = c.min(total);
                let pts = if wide {
                    points32(r, o, c)
                } else {
                    points16(r, o, c)
                };
                path.append(&Path::poly(&pts, &m, closed));
                o += c * if wide { 8 } else { 4 };
            }
            if closed {
                g.shape(path, true, true);
            } else {
                g.open_figure(path, false);
            }
        }
        56 | 92 => {
            // PolyDraw: points then a type byte per point.
            let n = u32_at(r, 24) as usize;
            let (pts, types_at) = if ty == 56 {
                (points32(r, 28, n), 28 + n * 8)
            } else {
                (points16(r, 28, n), 28 + n * 4)
            };
            let m = g.xf();
            let mut path = Path::default();
            path.move_to(m.apply(g.dc.cur));
            let mut i = 0;
            while i < pts.len() {
                let t = u8_at(r, types_at + i);
                match t & !1 {
                    6 => path.move_to(m.apply(pts[i])),
                    4 if i + 2 < pts.len() => {
                        path.cubic(m.apply(pts[i]), m.apply(pts[i + 1]), m.apply(pts[i + 2]));
                        i += 2;
                    }
                    _ => path.line_to(m.apply(pts[i])),
                }
                if t & 1 != 0 {
                    path.close();
                }
                i += 1;
            }
            if let Some(&last) = pts.last() {
                g.dc.cur = last;
            }
            g.open_figure(path, true);
        }
        9 => g.dc.win_ext = Some((i32_at(r, 8) as f64, i32_at(r, 12) as f64)),
        10 => g.dc.win_org = (i32_at(r, 8) as f64, i32_at(r, 12) as f64),
        11 => g.dc.vp_ext = Some((i32_at(r, 8) as f64, i32_at(r, 12) as f64)),
        12 => g.dc.vp_org = (i32_at(r, 8) as f64, i32_at(r, 12) as f64),
        31 | 32 => {
            let (xn, xd, yn, yd) = (
                i32_at(r, 8) as f64,
                i32_at(r, 12) as f64,
                i32_at(r, 16) as f64,
                i32_at(r, 20) as f64,
            );
            let target = if ty == 31 {
                &mut g.dc.vp_ext
            } else {
                &mut g.dc.win_ext
            };
            if let Some((x, y)) = *target
                && xd != 0.0
                && yd != 0.0
            {
                *target = Some((x * xn / xd, y * yn / yd));
            }
        }
        17 => g.dc.map_mode = u32_at(r, 8),
        18 => g.dc.bk_mode = u32_at(r, 8),
        19 => g.dc.fill_mode = u32_at(r, 8),
        22 => g.dc.text_align = u32_at(r, 8),
        24 => g.dc.text_color = colorref(r, 8),
        25 => g.dc.bk_color = colorref(r, 8),
        27 => g.move_to((i32_at(r, 8) as f64, i32_at(r, 12) as f64)),
        54 => g.line_to((i32_at(r, 8) as f64, i32_at(r, 12) as f64)),
        29 => g.exclude_clip_rect(rectl(r, 8)),
        30 => g.intersect_clip_rect(rectl(r, 8)),
        33 => g.save(),
        34 => g.restore(i32_at(r, 8)),
        35 => {
            let m = xform(r, 8);
            if m.is_finite() {
                g.dc.world = m;
            }
        }
        36 => {
            let m = xform(r, 8);
            if !m.is_finite() {
                return;
            }
            match u32_at(r, 32) {
                1 => g.dc.world = Xf::ID,
                2 => g.dc.world = m.then(&g.dc.world),
                3 => g.dc.world = g.dc.world.then(&m),
                4 => g.dc.world = m,
                _ => {}
            }
        }
        37 => {
            let ih = u32_at(r, 8);
            if ih & 0x8000_0000 != 0 {
                g.select(stock(ih & 0x7FFF_FFFF));
            } else {
                g.select_index(ih as usize);
            }
        }
        38 => {
            let width = i32_at(r, 16) as f64;
            g.put(
                u32_at(r, 8) as usize,
                Obj::Pen(Pen {
                    style: u32_at(r, 12),
                    width: width.abs(),
                    color: colorref(r, 24),
                    geometric: true,
                    user_dash: Vec::new(),
                }),
            );
        }
        95 => {
            let style = u32_at(r, 28);
            let width = u32_at(r, 32) as f64;
            let brush_style = u32_at(r, 36);
            let n = u32_at(r, 48) as usize;
            let user_dash = (0..n.min(16))
                .map(|i| u32_at(r, 52 + i * 4) as f64)
                .collect();
            let pen = if brush_style == 1 {
                Pen::null()
            } else {
                Pen {
                    style,
                    width,
                    color: colorref(r, 40),
                    geometric: style & 0x0001_0000 != 0,
                    user_dash,
                }
            };
            g.put(u32_at(r, 8) as usize, Obj::Pen(pen));
        }
        39 => {
            let style = u32_at(r, 12);
            let color = colorref(r, 16);
            let brush = match style {
                1 => Brush::Null,
                2 => Brush::Hatch(u32_at(r, 20), color),
                _ => Brush::Solid(color),
            };
            g.put(u32_at(r, 8) as usize, Obj::Brush(brush));
        }
        93 | 94 => {
            let (off_bmi, cb_bmi, off_bits, cb_bits) = (
                u32_at(r, 16) as usize,
                u32_at(r, 20) as usize,
                u32_at(r, 24) as usize,
                u32_at(r, 28) as usize,
            );
            let brush = bitmap_of(r, off_bmi, cb_bmi, off_bits, cb_bits)
                .and_then(|(info, bits)| dib::decode(info, bits, false))
                .and_then(|img| {
                    // A monochrome brush uses the text and background colours.
                    let img = if ty == 93 {
                        let (fg, bg) = (g.dc.text_color, g.dc.bk_color);
                        dib::Rgba {
                            w: img.w,
                            h: img.h,
                            px: img
                                .px
                                .iter()
                                .map(|p| {
                                    let c = if p[0] as u32 + p[1] as u32 + p[2] as u32 > 382 {
                                        bg
                                    } else {
                                        fg
                                    };
                                    [c[0], c[1], c[2], 255]
                                })
                                .collect(),
                        }
                    } else {
                        img
                    };
                    Some(Brush::Tile(img.png()?.into(), img.w, img.h))
                })
                .unwrap_or(Brush::Solid([128, 128, 128]));
            g.put(u32_at(r, 8) as usize, Obj::Brush(brush));
        }
        82 => {
            let o = 12;
            g.put(
                u32_at(r, 8) as usize,
                Obj::Font(Font {
                    height: i32_at(r, o) as f64,
                    width: i32_at(r, o + 4) as f64,
                    escapement: i32_at(r, o + 8) as f64,
                    weight: i32_at(r, o + 16),
                    italic: u8_at(r, o + 20) != 0,
                    underline: u8_at(r, o + 21) != 0,
                    strike: u8_at(r, o + 22) != 0,
                    charset: u8_at(r, o + 23),
                    face: utf16(r, o + 28, 32),
                }),
            );
        }
        49 => g.put(u32_at(r, 8) as usize, Obj::Other),
        40 => g.delete(u32_at(r, 8) as usize),
        42 => {
            let p = Path::ellipse(rectl(r, 8), &g.xf());
            g.shape(p, true, true);
        }
        43 => {
            let p = Path::rect(rectl(r, 8), &g.xf());
            g.shape(p, true, true);
        }
        44 => {
            let p = Path::round_rect(
                rectl(r, 8),
                (i32_at(r, 24) as f64, i32_at(r, 28) as f64),
                &g.xf(),
            );
            g.shape(p, true, true);
        }
        45..=47 | 55 => {
            let rect = rectl(r, 8);
            let s = (i32_at(r, 24) as f64, i32_at(r, 28) as f64);
            let e = (i32_at(r, 32) as f64, i32_at(r, 36) as f64);
            let kind = match ty {
                45 => ArcKind::Arc,
                46 => ArcKind::Chord,
                47 => ArcKind::Pie,
                _ => ArcKind::ArcTo,
            };
            g.arc(rect, s, e, kind);
        }
        41 => {
            // AngleArc: a line to the arc's start, then the arc.
            let (cx, cy) = (i32_at(r, 8) as f64, i32_at(r, 12) as f64);
            let rad = u32_at(r, 16) as f64;
            let start = f32_at(r, 20).to_radians();
            let sweep = f32_at(r, 24).to_radians();
            let m = g.xf();
            let mut p = Path::default();
            p.move_to(m.apply(g.dc.cur));
            // Angles go counterclockwise with y up: negate for y down.
            crate::canvas::arc_points(&mut p, (cx, cy), (rad, rad), -start, -sweep, &m, false);
            let end = -(start + sweep);
            g.dc.cur = (cx + rad * end.cos(), cy + rad * end.sin());
            g.open_figure(p, true);
        }
        57 => g.dc.clockwise = u32_at(r, 8) == 2,
        58 => g.dc.miter = f32_at(r, 8).max(1.0),
        59 => g.begin_path(),
        60 => g.end_path(),
        61 => g.close_figure(),
        68 => g.abort_path(),
        62 => g.draw_done_path(true, false),
        63 => g.draw_done_path(true, true),
        64 => g.draw_done_path(false, true),
        65 | 66 => {
            // Flattening changes nothing drawn; widening is approximated by
            // keeping the outline.
            if let Some(p) = g.done_path.take() {
                g.done_path = Some(p);
            }
        }
        67 => g.select_clip_path(u32_at(r, 8)),
        75 => {
            let cb = u32_at(r, 8) as usize;
            let mode = u32_at(r, 12);
            if cb == 0 {
                if mode == 5 {
                    g.clip_reset();
                }
            } else {
                let p = region_path(r, 16);
                g.clip_to(&p, false, mode == 1);
            }
        }
        71 => {
            // FillRgn: bounds, cbRgnData, ihBrush, region (device units).
            let ih = u32_at(r, 28);
            let brush = if ih & 0x8000_0000 != 0 {
                stock(ih & 0x7FFF_FFFF)
            } else {
                g.objects.get(ih as usize).cloned().flatten()
            };
            if let Some(Obj::Brush(Brush::Solid(c))) = brush {
                let p = region_path(r, 32);
                g.fill_with(&p, &Paint::Color(c, 1.0));
            }
        }
        74 => {
            let p = region_path(r, 24);
            if let Brush::Solid(c) = g.dc.brush.clone() {
                g.fill_with(&p, &Paint::Color(c, 1.0));
            }
        }
        76 | 77 => {
            let dest = (
                i32_at(r, 24) as f64,
                i32_at(r, 28) as f64,
                i32_at(r, 32) as f64,
                i32_at(r, 36) as f64,
            );
            let rop = u32_at(r, 40);
            let (xs, ys) = (i32_at(r, 44) as i64, i32_at(r, 48) as i64);
            let (off_bmi, cb_bmi, off_bits, cb_bits) = (
                u32_at(r, 84) as usize,
                u32_at(r, 88) as usize,
                u32_at(r, 92) as usize,
                u32_at(r, 96) as usize,
            );
            let (cxs, cys) = if ty == 77 {
                (i32_at(r, 100) as i64, i32_at(r, 104) as i64)
            } else {
                (dest.2 as i64, dest.3 as i64)
            };
            match bitmap_of(r, off_bmi, cb_bmi, off_bits, cb_bits) {
                Some((info, bits)) if cb_bmi > 0 => {
                    blit(g, info, bits, (xs, ys, cxs, cys), dest, rop, false, None)
                }
                _ => g.pat_blt(dest, rop),
            }
        }
        81 => {
            let dest = (
                i32_at(r, 24) as f64,
                i32_at(r, 28) as f64,
                i32_at(r, 72) as f64,
                i32_at(r, 76) as f64,
            );
            let src = (
                i32_at(r, 32) as i64,
                i32_at(r, 36) as i64,
                i32_at(r, 40) as i64,
                i32_at(r, 44) as i64,
            );
            let rop = u32_at(r, 68);
            if let Some((info, bits)) = bitmap_of(
                r,
                u32_at(r, 48) as usize,
                u32_at(r, 52) as usize,
                u32_at(r, 56) as usize,
                u32_at(r, 60) as usize,
            ) {
                blit(g, info, bits, src, dest, rop, true, None);
            } else {
                g.pat_blt(dest, rop);
            }
        }
        80 => {
            let src = (
                i32_at(r, 32) as i64,
                i32_at(r, 36) as i64,
                i32_at(r, 40) as i64,
                i32_at(r, 44) as i64,
            );
            let dest = (
                i32_at(r, 24) as f64,
                i32_at(r, 28) as f64,
                src.2 as f64,
                src.3 as f64,
            );
            if let Some((info, bits)) = bitmap_of(
                r,
                u32_at(r, 48) as usize,
                u32_at(r, 52) as usize,
                u32_at(r, 56) as usize,
                u32_at(r, 60) as usize,
            ) {
                blit(g, info, bits, src, dest, 0x00CC_0020, true, None);
            }
        }
        114 => {
            let dest = (
                i32_at(r, 24) as f64,
                i32_at(r, 28) as f64,
                i32_at(r, 32) as f64,
                i32_at(r, 36) as f64,
            );
            let constant = u8_at(r, 42);
            let src_alpha = u8_at(r, 43) & 1 != 0;
            let src = (
                i32_at(r, 44) as i64,
                i32_at(r, 48) as i64,
                i32_at(r, 100) as i64,
                i32_at(r, 104) as i64,
            );
            if let Some((info, bits)) = bitmap_of(
                r,
                u32_at(r, 84) as usize,
                u32_at(r, 88) as usize,
                u32_at(r, 92) as usize,
                u32_at(r, 96) as usize,
            ) {
                blit(
                    g,
                    info,
                    bits,
                    src,
                    dest,
                    0x00CC_0020,
                    false,
                    Some((constant, src_alpha)),
                );
            }
        }
        116 => {
            // TransparentBlt: like StretchBlt; the transparent colour is
            // dropped.
            let dest = (
                i32_at(r, 24) as f64,
                i32_at(r, 28) as f64,
                i32_at(r, 32) as f64,
                i32_at(r, 36) as f64,
            );
            let key = colorref(r, 40);
            let src = (
                i32_at(r, 44) as i64,
                i32_at(r, 48) as i64,
                i32_at(r, 100) as i64,
                i32_at(r, 104) as i64,
            );
            if let Some((info, bits)) = bitmap_of(
                r,
                u32_at(r, 84) as usize,
                u32_at(r, 88) as usize,
                u32_at(r, 92) as usize,
                u32_at(r, 96) as usize,
            ) && let Some(mut img) = dib::decode(info, bits, false)
            {
                for p in &mut img.px {
                    if [p[0], p[1], p[2]] == key {
                        p[3] = 0;
                    }
                }
                let part = img.crop(src.0, src.1, src.2, src.3).unwrap_or(img);
                g.bitmap(dest, &part, 0x00CC_0020);
            }
        }
        83 | 84 => text(g, r, ty == 84),
        96 | 97 => {
            // PolyTextOut: several EMRTEXTs.
            let n = u32_at(r, 36) as usize;
            for i in 0..n.min(10_000) {
                text_at(g, r, 40 + i * 40, ty == 97);
            }
        }
        118 => gradient(g, r),
        _ => {}
    }
}

fn poly(g: &mut Gdi, ty: u32, pts: &[Pt]) {
    let m = g.xf();
    match ty {
        2 => {
            let p = Path::bezier(pts, &m, None);
            g.open_figure(p, false);
        }
        3 => {
            let p = Path::poly(pts, &m, true);
            g.shape(p, true, true);
        }
        4 => {
            let p = Path::poly(pts, &m, false);
            g.open_figure(p, false);
        }
        5 => {
            let p = Path::bezier(pts, &m, Some(g.dc.cur));
            if let Some(&last) = pts.last() {
                g.dc.cur = last;
            }
            g.open_figure(p, true);
        }
        6 => {
            let mut all = vec![g.dc.cur];
            all.extend_from_slice(pts);
            let p = Path::poly(&all, &m, false);
            if let Some(&last) = pts.last() {
                g.dc.cur = last;
            }
            g.open_figure(p, true);
        }
        _ => {}
    }
}

fn text(g: &mut Gdi, r: &[u8], wide: bool) {
    text_at(g, r, 36, wide);
}

/// An EMRTEXT at `o` (offsets in it are from the record's start).
fn text_at(g: &mut Gdi, r: &[u8], o: usize, wide: bool) {
    let at = (i32_at(r, o) as f64, i32_at(r, o + 4) as f64);
    let n = u32_at(r, o + 8) as usize;
    let off = u32_at(r, o + 12) as usize;
    let opts = u32_at(r, o + 16);
    let rect = rectl(r, o + 20);
    let off_dx = u32_at(r, o + 36) as usize;
    if n > 100_000 {
        return;
    }
    let s = if wide {
        let units: Vec<u16> = (0..n).map(|i| u16_at(r, off + i * 2)).collect();
        String::from_utf16_lossy(&units)
    } else {
        crate::rd::ansi(r.get(off..off + n).unwrap_or(&[]))
    };
    // ETO_PDY: a dx and a dy per character.
    let pdy = opts & 0x2000 != 0;
    let dx = if off_dx > 0 && r.len() >= off_dx + n * if pdy { 8 } else { 4 } {
        let raw: Vec<f64> = (0..n)
            .map(|i| i32_at(r, off_dx + i * if pdy { 8 } else { 4 }) as f64)
            .collect();
        // One advance per character (not per UTF-16 unit).
        if wide && s.chars().count() != n {
            let mut out = Vec::new();
            let units: Vec<u16> = (0..n).map(|i| u16_at(r, off + i * 2)).collect();
            let mut i = 0;
            while i < n {
                let surrogate = (0xD800..0xDC00).contains(&units[i]) && i + 1 < n;
                let adv = raw[i] + if surrogate { raw[i + 1] } else { 0.0 };
                out.push(adv);
                i += if surrogate { 2 } else { 1 };
            }
            Some(out)
        } else {
            Some(raw)
        }
    } else {
        None
    };
    let opaque = (opts & 0x0002 != 0 && rect.2 > rect.0 && rect.3 > rect.1).then_some(rect);
    let clipped = opts & 0x0004 != 0 && rect.2 > rect.0 && rect.3 > rect.1;
    let saved_clip = g.dc.clip.clone();
    if clipped {
        g.intersect_clip_rect(rect);
    }
    g.text(at, &s, dx, opaque);
    if clipped {
        g.dc.clip = saved_clip.clone();
        g.svg.clip = saved_clip;
    }
}

/// GradientFill with rectangles (horizontal or vertical); triangles are
/// filled with their vertices' average colour.
fn gradient(g: &mut Gdi, r: &[u8]) {
    let nv = u32_at(r, 24) as usize;
    let nt = u32_at(r, 28) as usize;
    let mode = u32_at(r, 32);
    if nv > 100_000 || nt > 100_000 {
        return;
    }
    let vert = |i: usize| {
        let o = 36 + i * 16;
        let c = |k: usize| (u16_at(r, o + 8 + k * 2) >> 8) as u8;
        (
            (i32_at(r, o) as f64, i32_at(r, o + 4) as f64),
            [c(0), c(1), c(2), 255],
        )
    };
    let m = g.xf();
    let o = 36 + nv * 16;
    for k in 0..nt {
        if mode == 2 {
            let (a, b, c) = (
                u32_at(r, o + k * 12) as usize,
                u32_at(r, o + k * 12 + 4) as usize,
                u32_at(r, o + k * 12 + 8) as usize,
            );
            if a.max(b).max(c) >= nv {
                continue;
            }
            let (pa, ca) = vert(a);
            let (pb, cb) = vert(b);
            let (pc, cc) = vert(c);
            let avg = |i: usize| ((ca[i] as u32 + cb[i] as u32 + cc[i] as u32) / 3) as u8;
            let p = Path::poly(&[pa, pb, pc], &m, true);
            g.fill_with(&p, &Paint::Color([avg(0), avg(1), avg(2)], 1.0));
        } else {
            let (a, b) = (
                u32_at(r, o + k * 8) as usize,
                u32_at(r, o + k * 8 + 4) as usize,
            );
            if a.max(b) >= nv {
                continue;
            }
            let (pa, ca) = vert(a);
            let (pb, cb) = vert(b);
            let p = Path::rect((pa.0, pa.1, pb.0, pb.1), &m);
            let (from, to) = if mode == 0 {
                ((pa.0, pa.1), (pb.0, pa.1))
            } else {
                ((pa.0, pa.1), (pa.0, pb.1))
            };
            let paint = Paint::Linear(m.apply(from), m.apply(to), ca, cb);
            g.fill_with(&p, &paint);
        }
    }
}
