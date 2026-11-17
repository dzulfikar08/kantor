//! Device-independent bitmaps (the pixels inside metafiles and `.bmp`
//! files) decoded to RGBA and written as PNG.

use crate::rd::{u16_at, u32_at};

/// Decoded pixels, top row first.
pub(crate) struct Rgba {
    pub w: u32,
    pub h: u32,
    pub px: Vec<[u8; 4]>,
}

impl Rgba {
    /// The part from (`x`, `y`), `w`×`h`, clamped to the picture.
    pub fn crop(&self, x: i64, y: i64, w: i64, h: i64) -> Option<Rgba> {
        let x0 = x.clamp(0, self.w as i64);
        let y0 = y.clamp(0, self.h as i64);
        let x1 = (x + w).clamp(0, self.w as i64);
        let y1 = (y + h).clamp(0, self.h as i64);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        if (x0, y0, x1, y1) == (0, 0, self.w as i64, self.h as i64) {
            return Some(Rgba {
                w: self.w,
                h: self.h,
                px: self.px.clone(),
            });
        }
        let mut px = Vec::with_capacity(((x1 - x0) * (y1 - y0)) as usize);
        for row in y0..y1 {
            let start = (row * self.w as i64 + x0) as usize;
            px.extend_from_slice(&self.px[start..start + (x1 - x0) as usize]);
        }
        Some(Rgba {
            w: (x1 - x0) as u32,
            h: (y1 - y0) as u32,
            px,
        })
    }

    pub fn png(&self) -> Option<Vec<u8>> {
        let mut map = resvg::tiny_skia::Pixmap::new(self.w, self.h)?;
        for (dst, s) in map.pixels_mut().iter_mut().zip(&self.px) {
            *dst = resvg::tiny_skia::ColorU8::from_rgba(s[0], s[1], s[2], s[3]).premultiply();
        }
        map.encode_png().ok()
    }
}

/// Largest bitmap decoded (pixels), to keep a damaged header from
/// allocating gigabytes.
const MAX_PIXELS: u64 = 64 * 1024 * 1024;

/// A DIB: `info` is the BITMAPINFO (header and colour table), `bits` the
/// pixel rows. `alpha` keeps the fourth byte of 32-bit pixels (else they
/// are opaque, as GDI draws them).
pub(crate) fn decode(info: &[u8], bits: &[u8], alpha: bool) -> Option<Rgba> {
    let hsize = u32_at(info, 0) as usize;
    let (w, h, bpp, compression, used, masks_at) = if hsize == 12 {
        (
            u16_at(info, 4) as i64,
            u16_at(info, 6) as i16 as i64,
            u16_at(info, 10) as u32,
            0,
            0,
            0,
        )
    } else if hsize >= 40 {
        (
            u32_at(info, 4) as i32 as i64,
            u32_at(info, 8) as i32 as i64,
            u16_at(info, 14) as u32,
            u32_at(info, 16),
            u32_at(info, 32) as usize,
            if hsize >= 52 { 40 } else { hsize },
        )
    } else {
        return None;
    };
    if w <= 0 || h == 0 {
        return None;
    }
    let (w, top_down, h) = (w as u32, h < 0, h.unsigned_abs() as u32);
    if w as u64 * h as u64 > MAX_PIXELS {
        return None;
    }
    // BI_JPEG / BI_PNG: the bits are a whole picture file.
    if compression == 4 || compression == 5 {
        return None;
    }
    let entry = if hsize == 12 { 3 } else { 4 };
    let ncolors = if bpp <= 8 {
        if used > 0 { used.min(256) } else { 1 << bpp }
    } else {
        0
    };
    let masks_len = if compression == 3 && hsize == 40 {
        12
    } else {
        0
    };
    let table_at = hsize + masks_len;
    let palette: Vec<[u8; 4]> = (0..ncolors)
        .map(|i| {
            let o = table_at + i * entry;
            match info.get(o..o + 3) {
                Some(c) => [c[2], c[1], c[0], 255],
                None => [0, 0, 0, 255],
            }
        })
        .collect();
    let masks = if compression == 3 || compression == 6 {
        Some((
            u32_at(info, masks_at),
            u32_at(info, masks_at + 4),
            u32_at(info, masks_at + 8),
            if hsize >= 56 || compression == 6 {
                u32_at(info, masks_at + 12)
            } else {
                0
            },
        ))
    } else {
        None
    };
    let mut px = vec![[0u8, 0, 0, 255]; (w * h) as usize];
    let mut put = |x: u32, y: u32, c: [u8; 4]| {
        if x < w && y < h {
            let row = if top_down { y } else { h - 1 - y };
            px[(row * w + x) as usize] = c;
        }
    };
    let pal = |i: usize| palette.get(i).copied().unwrap_or([0, 0, 0, 255]);
    match compression {
        1 | 2 => rle(bits, h, compression == 2, &mut |x, y, i| put(x, y, pal(i))),
        _ => {
            let stride = (w as usize * bpp as usize).div_ceil(32) * 4;
            for y in 0..h {
                let row = match bits.get(y as usize * stride..) {
                    Some(r) => r,
                    None => break,
                };
                for x in 0..w {
                    let c = match bpp {
                        1 | 2 | 4 | 8 => {
                            let bit = x as usize * bpp as usize;
                            let Some(&byte) = row.get(bit / 8) else { break };
                            let shift = 8 - bpp as usize - (bit % 8);
                            pal(((byte >> shift) & ((1u16 << bpp) - 1) as u8) as usize)
                        }
                        16 => {
                            let v = u16_at(row, x as usize * 2) as u32;
                            match masks {
                                Some((r, g, b, _)) => [mask(v, r), mask(v, g), mask(v, b), 255],
                                None => [
                                    (((v >> 10) & 31) * 255 / 31) as u8,
                                    (((v >> 5) & 31) * 255 / 31) as u8,
                                    ((v & 31) * 255 / 31) as u8,
                                    255,
                                ],
                            }
                        }
                        24 => match row.get(x as usize * 3..x as usize * 3 + 3) {
                            Some(c) => [c[2], c[1], c[0], 255],
                            None => break,
                        },
                        32 => {
                            let v = u32_at(row, x as usize * 4);
                            match masks {
                                Some((r, g, b, a)) => [
                                    mask(v, r),
                                    mask(v, g),
                                    mask(v, b),
                                    if alpha && a != 0 { mask(v, a) } else { 255 },
                                ],
                                None => [
                                    (v >> 16) as u8,
                                    (v >> 8) as u8,
                                    v as u8,
                                    if alpha { (v >> 24) as u8 } else { 255 },
                                ],
                            }
                        }
                        _ => return None,
                    };
                    put(x, y, c);
                }
            }
        }
    }
    Some(Rgba { w, h, px })
}

fn mask(v: u32, m: u32) -> u8 {
    if m == 0 {
        return 0;
    }
    let shift = m.trailing_zeros();
    let max = m >> shift;
    (((v & m) >> shift) * 255 / max.max(1)) as u8
}

/// Run-length encoded rows (BI_RLE8, BI_RLE4), bottom row first.
fn rle(bits: &[u8], h: u32, four: bool, put: &mut dyn FnMut(u32, u32, usize)) {
    let (mut x, mut y, mut i) = (0u32, 0u32, 0usize);
    while i + 1 < bits.len() && y < h {
        let (n, v) = (bits[i], bits[i + 1]);
        i += 2;
        if n > 0 {
            for k in 0..n {
                let idx = if four {
                    if k % 2 == 0 { v >> 4 } else { v & 15 }
                } else {
                    v
                };
                put(x, y, idx as usize);
                x += 1;
            }
            continue;
        }
        match v {
            0 => {
                x = 0;
                y += 1;
            }
            1 => break,
            2 => {
                let (dx, dy) = (
                    bits.get(i).copied().unwrap_or(0),
                    bits.get(i + 1).copied().unwrap_or(0),
                );
                i += 2;
                x += dx as u32;
                y += dy as u32;
            }
            n => {
                let count = n as usize;
                let bytes = if four { count.div_ceil(2) } else { count };
                for k in 0..count {
                    let idx = if four {
                        let b = bits.get(i + k / 2).copied().unwrap_or(0);
                        if k % 2 == 0 { b >> 4 } else { b & 15 }
                    } else {
                        bits.get(i + k).copied().unwrap_or(0)
                    };
                    put(x, y, idx as usize);
                    x += 1;
                }
                i += bytes + (bytes & 1);
            }
        }
    }
}

/// A packed DIB (BITMAPINFO immediately followed by the bits), as in WMF
/// records, `.doc`/`.xls` DIB blips and the clipboard.
pub(crate) fn decode_packed(dib: &[u8], alpha: bool) -> Option<Rgba> {
    let (info_len, compression) = info_len(dib)?;
    if compression == 4 || compression == 5 {
        return None;
    }
    decode(dib, dib.get(info_len..)?, alpha)
}

/// The BITMAPINFO's length (header, masks, colour table) and compression.
pub(crate) fn info_len(dib: &[u8]) -> Option<(usize, u32)> {
    let hsize = u32_at(dib, 0) as usize;
    if hsize == 12 {
        let bpp = u16_at(dib, 10) as usize;
        let n = if bpp <= 8 { 1 << bpp } else { 0 };
        return Some((12 + 3 * n, 0));
    }
    if hsize < 40 {
        return None;
    }
    let bpp = u16_at(dib, 14) as usize;
    let compression = u32_at(dib, 16);
    let used = u32_at(dib, 32) as usize;
    let n = if bpp <= 8 {
        if used > 0 { used.min(256) } else { 1 << bpp }
    } else {
        // Colour tables are allowed (and ignored) for deeper pixels.
        used.min(256)
    };
    let masks = if compression == 3 && hsize == 40 {
        12
    } else {
        0
    };
    Some((hsize + masks + n * 4, compression))
}

/// The picture inside a BI_PNG/BI_JPEG DIB.
pub(crate) fn embedded(dib: &[u8]) -> Option<&[u8]> {
    let (len, c) = info_len(dib)?;
    if c == 4 || c == 5 {
        dib.get(len..)
    } else {
        None
    }
}

/// Rows stored bottom first (a positive height).
pub(crate) fn bottom_up(info: &[u8]) -> bool {
    if u32_at(info, 0) == 12 {
        (u16_at(info, 6) as i16) > 0
    } else {
        (u32_at(info, 8) as i32) > 0
    }
}
