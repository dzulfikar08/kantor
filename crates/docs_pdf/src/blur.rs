//! Blurred shadows: an outline filled on the CPU (anti-aliased) and
//! blurred (three box blurs, close to a Gaussian), as a PNG the screen
//! and the PDF both draw where the shadow goes.

use std::io::Write as _;
use std::sync::Arc;

use crate::floats::Seg;

/// Most pixels on a side of a blurred shadow.
const MAX_SIDE: f32 = 1200.0;
/// Pixels per page px (crisper than 1 at zoom, small enough to be quick).
const SCALE: f32 = 1.5;

/// A shadow picture: where it goes on the page and its PNG.
#[derive(Clone, Debug, PartialEq)]
pub struct Blurred {
    pub rect: [f32; 4],
    pub png: Arc<[u8]>,
    /// Tells pictures apart (screen cache).
    pub key: u64,
}

/// `outlines` (page px) filled with `rgba` and blurred by `radius` px.
pub fn blurred(outlines: &[Vec<Seg>], rgba: [u8; 4], radius: f32) -> Option<Blurred> {
    let polys: Vec<Vec<[f32; 2]>> = outlines.iter().flat_map(|s| flatten(s)).collect();
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in polys.iter().flatten() {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    let pad = radius * 1.5 + 1.0;
    let (x0, y0, x1, y1) = (x0 - pad, y0 - pad, x1 + pad, y1 + pad);
    let k = SCALE.min(MAX_SIDE / (x1 - x0).max(y1 - y0));
    let (w, h) = (
        ((x1 - x0) * k).ceil().max(1.0) as usize,
        ((y1 - y0) * k).ceil().max(1.0) as usize,
    );
    let mut cov = vec![0f32; w * h];
    let scaled: Vec<Vec<[f32; 2]>> = polys
        .iter()
        .map(|p| {
            p.iter()
                .map(|q| [(q[0] - x0) * k, (q[1] - y0) * k])
                .collect()
        })
        .collect();
    fill(&scaled, w, h, &mut cov);
    // Three box blurs of this half-width approximate a Gaussian whose
    // spread matches the radius.
    let r = ((radius * k) / 2.0).round() as usize;
    if r > 0 {
        for _ in 0..3 {
            box_blur(&mut cov, w, h, r);
        }
    }
    let mut px = Vec::with_capacity(w * h * 4);
    for c in &cov {
        let a = (c.clamp(0.0, 1.0) * rgba[3] as f32).round() as u8;
        px.extend_from_slice(&[rgba[0], rgba[1], rgba[2], a]);
    }
    let png = encode_png(&px, w as u32, h as u32)?;
    let key = {
        use std::hash::{Hash, Hasher};
        let mut s = std::collections::hash_map::DefaultHasher::new();
        png.hash(&mut s);
        s.finish()
    };
    Some(Blurred {
        rect: [x0, y0, w as f32 / k, h as f32 / k],
        png: png.into(),
        key,
    })
}

/// The outline as polygons (curves as short lines).
fn flatten(segs: &[Seg]) -> Vec<Vec<[f32; 2]>> {
    let mut out: Vec<Vec<[f32; 2]>> = Vec::new();
    let mut cur: Vec<[f32; 2]> = Vec::new();
    let mut last = [0.0, 0.0];
    for s in segs {
        match *s {
            Seg::Move(p) => {
                if cur.len() >= 3 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
                cur.push(p);
                last = p;
            }
            Seg::Line(p) => {
                cur.push(p);
                last = p;
            }
            Seg::Quad(c, p) => {
                for i in 1..=8 {
                    let t = i as f32 / 8.0;
                    let u = 1.0 - t;
                    cur.push([
                        u * u * last[0] + 2.0 * u * t * c[0] + t * t * p[0],
                        u * u * last[1] + 2.0 * u * t * c[1] + t * t * p[1],
                    ]);
                }
                last = p;
            }
            Seg::Cubic(c1, c2, p) => {
                for i in 1..=12 {
                    let t = i as f32 / 12.0;
                    let u = 1.0 - t;
                    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                    cur.push([
                        a * last[0] + b * c1[0] + c * c2[0] + d * p[0],
                        a * last[1] + b * c1[1] + c * c2[1] + d * p[1],
                    ]);
                }
                last = p;
            }
            Seg::Close => {
                if cur.len() >= 3 {
                    out.push(std::mem::take(&mut cur));
                }
                cur.clear();
            }
        }
    }
    if cur.len() >= 3 {
        out.push(cur);
    }
    out
}

/// Non-zero fill with 4 × 4 samples per pixel.
fn fill(polys: &[Vec<[f32; 2]>], w: usize, h: usize, cov: &mut [f32]) {
    const SUB: usize = 4;
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for row in 0..h * SUB {
        let y = (row as f32 + 0.5) / SUB as f32;
        xs.clear();
        for p in polys {
            for i in 0..p.len() {
                let (a, b) = (p[i], p[(i + 1) % p.len()]);
                if (a[1] <= y) != (b[1] <= y) {
                    let x = a[0] + (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]);
                    xs.push((x, if b[1] > a[1] { 1 } else { -1 }));
                }
            }
        }
        xs.sort_by(|p, q| p.0.total_cmp(&q.0));
        let mut wind = 0;
        let line = &mut cov[(row / SUB) * w..(row / SUB + 1) * w];
        for k in 0..xs.len() {
            let before = wind;
            wind += xs[k].1;
            if before == 0 && wind != 0 {
                let start = xs[k].0;
                let end = xs[k + 1..]
                    .iter()
                    .scan(wind, |wd, e| {
                        *wd += e.1;
                        Some((*wd, e.0))
                    })
                    .find(|(wd, _)| *wd == 0)
                    .map_or(w as f32, |(_, x)| x);
                span(line, start * SUB as f32, end * SUB as f32, SUB);
            }
        }
    }
}

/// Add one sub-row's coverage from `a` to `b` (in sub-pixels).
fn span(line: &mut [f32], a: f32, b: f32, sub: usize) {
    let w = line.len();
    let (a, b) = (a.max(0.0), b.min((w * sub) as f32));
    if a >= b {
        return;
    }
    let unit = 1.0 / (sub * sub) as f32;
    let mut x = a.floor() as usize;
    while (x as f32) < b && x < w * sub {
        let lo = (x as f32).max(a);
        let hi = ((x + 1) as f32).min(b);
        line[x / sub] += (hi - lo) * unit;
        x += 1;
    }
}

/// One box blur pass (horizontal then vertical) of half-width `r`.
fn box_blur(px: &mut [f32], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0f32; px.len()];
    let n = (2 * r + 1) as f32;
    for y in 0..h {
        let row = &px[y * w..(y + 1) * w];
        let mut acc = 0.0;
        for i in 0..=r.min(w - 1) {
            acc += row[i];
        }
        for x in 0..w {
            tmp[y * w + x] = acc / n;
            if x + r + 1 < w {
                acc += row[x + r + 1];
            }
            if x >= r {
                acc -= row[x - r];
            }
        }
    }
    for x in 0..w {
        let mut acc = 0.0;
        for i in 0..=r.min(h - 1) {
            acc += tmp[i * w + x];
        }
        for y in 0..h {
            px[y * w + x] = acc / n;
            if y + r + 1 < h {
                acc += tmp[(y + r + 1) * w + x];
            }
            if y >= r {
                acc -= tmp[(y - r) * w + x];
            }
        }
    }
}

/// RGBA pixels as a PNG.
fn encode_png(px: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut raw = Vec::with_capacity(px.len() + h as usize);
    for row in px.chunks_exact(w as usize * 4) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(&raw).ok()?;
    let data = z.finish().ok()?;
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], body: &[u8]| {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut c = flate2::Crc::new();
        c.update(kind);
        c.update(body);
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        out.extend_from_slice(&c.sum().to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &data);
    chunk(b"IEND", &[]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_blurs_softly_at_its_edges() {
        let sq = vec![
            Seg::Move([10.0, 10.0]),
            Seg::Line([110.0, 10.0]),
            Seg::Line([110.0, 110.0]),
            Seg::Line([10.0, 110.0]),
            Seg::Close,
        ];
        let b = blurred(&[sq], [0, 0, 0, 200], 8.0).unwrap();
        assert!(b.rect[0] < 10.0 && b.rect[0] + b.rect[2] > 110.0);
        let img = image_px(&b.png);
        let (w, h, px) = img;
        let alpha = |x: f32, y: f32| {
            let i = (((y - b.rect[1]) / b.rect[3] * h as f32) as usize * w as usize
                + ((x - b.rect[0]) / b.rect[2] * w as f32) as usize)
                * 4;
            px[i + 3]
        };
        // Solid in the middle, half at the edge, fading outside.
        assert!(alpha(60.0, 60.0) >= 195, "{}", alpha(60.0, 60.0));
        let edge = alpha(10.0, 60.0);
        assert!((70..=130).contains(&edge), "{edge}");
        assert!(alpha(3.0, 60.0) < 40);
    }

    /// Decode our own PNG (no filter, one IDAT).
    fn image_px(png: &[u8]) -> (u32, u32, Vec<u8>) {
        use std::io::Read as _;
        let w = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let h = u32::from_be_bytes(png[20..24].try_into().unwrap());
        let len = u32::from_be_bytes(png[33..37].try_into().unwrap()) as usize;
        let mut raw = Vec::new();
        flate2::read::ZlibDecoder::new(&png[41..41 + len])
            .read_to_end(&mut raw)
            .unwrap();
        let px = raw
            .chunks_exact(w as usize * 4 + 1)
            .flat_map(|r| r[1..].to_vec())
            .collect();
        (w, h, px)
    }
}
