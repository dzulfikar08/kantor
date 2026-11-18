//! Anti-aliased coverage of a path on a pixel grid, for renderers that
//! can't clip pictures to a path themselves (GPUI paints images only into
//! rectangles): the picture is cut to its shape on the CPU by multiplying
//! its alpha by the coverage.

use crate::geom::{P, Path};

/// Rows sampled per pixel row (columns are covered exactly).
const SUB: usize = 4;

/// How much of each pixel of a `w` × `h` grid `path` covers (0..=255),
/// row by row, with the non-zero winding rule. `path` is in pixels (its
/// subpaths taken as closed).
pub fn coverage(path: &Path, (w, h): (u32, u32)) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut acc = vec![0f32; w * h];
    let edges: Vec<(P, P)> = path
        .flatten(0.2)
        .into_iter()
        .flat_map(|(poly, _)| {
            let n = poly.len();
            (0..n).map(move |i| (poly[i], poly[(i + 1) % n]))
        })
        .filter(|(a, b)| a.y != b.y)
        .collect();
    let mut xs: Vec<(f32, i32)> = Vec::new();
    for row in 0..h {
        let line = &mut acc[row * w..(row + 1) * w];
        for s in 0..SUB {
            let y = row as f32 + (s as f32 + 0.5) / SUB as f32;
            xs.clear();
            for &(a, b) in &edges {
                let (lo, hi, dir) = if a.y < b.y { (a, b, 1) } else { (b, a, -1) };
                if y < lo.y || y >= hi.y {
                    continue;
                }
                xs.push((lo.x + (y - lo.y) / (hi.y - lo.y) * (hi.x - lo.x), dir));
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut wind = 0;
            for k in 0..xs.len().saturating_sub(1) {
                wind += xs[k].1;
                if wind != 0 {
                    span(line, xs[k].0, xs[k + 1].0);
                }
            }
        }
    }
    acc.into_iter()
        .map(|c| (c / SUB as f32 * 255.0).round().clamp(0.0, 255.0) as u8)
        .collect()
}

/// Adds the part of each pixel of `line` that `x0..x1` covers.
fn span(line: &mut [f32], x0: f32, x1: f32) {
    let w = line.len() as f32;
    let (x0, x1) = (x0.clamp(0.0, w), x1.clamp(0.0, w));
    if x1 <= x0 {
        return;
    }
    let (first, last) = (x0.floor() as usize, (x1.ceil() as usize).min(line.len()));
    for (i, c) in line.iter_mut().enumerate().take(last).skip(first) {
        let i = i as f32;
        *c += x1.min(i + 1.0) - x0.max(i);
    }
}

/// Cuts a `w` × `h` raster of 4-byte pixels (alpha last) to `path` (in
/// pixels): each pixel's alpha times the path's coverage.
pub fn cut_to(pixels: &mut [u8], (w, h): (u32, u32), path: &Path) {
    let cover = coverage(path, (w, h));
    for (px, c) in pixels.chunks_exact_mut(4).zip(cover) {
        if c < 255 {
            px[3] = ((px[3] as u16 * c as u16 + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_square_covers_its_pixels_and_halves_at_its_edges() {
        let c = coverage(&Path::rect(1.0, 1.0, 2.5, 2.0), (4, 4));
        assert_eq!(&c[4..8], &[0, 255, 255, 128]);
        assert_eq!(&c[8..12], &[0, 255, 255, 128]);
        assert_eq!(&c[0..4], &[0; 4]);
        assert_eq!(&c[12..16], &[0; 4]);
    }

    #[test]
    fn an_ellipse_leaves_the_corners_out() {
        let c = coverage(&Path::ellipse(50.0, 50.0, 50.0, 50.0), (100, 100));
        assert_eq!(c[0], 0, "corner");
        assert_eq!(c[99 * 100 + 99], 0, "corner");
        assert_eq!(c[50 * 100 + 50], 255, "centre");
        assert!(c[50 * 100] > 200, "left edge, mid height");
        assert_eq!(c[50 * 100 + 3], 255, "inside the left edge");
        let total: f32 = c.iter().map(|&v| v as f32 / 255.0).sum();
        let area = std::f32::consts::PI * 50.0 * 50.0;
        assert!((total - area).abs() < area * 0.01, "{total} vs {area}");
    }

    #[test]
    fn a_hole_drawn_the_other_way_stays_empty() {
        let mut p = Path::rect(0.0, 0.0, 10.0, 10.0);
        // Counter-clockwise inner square.
        p.move_to(crate::geom::p(3.0, 3.0));
        p.line_to(crate::geom::p(3.0, 7.0));
        p.line_to(crate::geom::p(7.0, 7.0));
        p.line_to(crate::geom::p(7.0, 3.0));
        p.close();
        let c = coverage(&p, (10, 10));
        assert_eq!(c[5 * 10 + 5], 0);
        assert_eq!(c[5 * 10 + 1], 255);
    }

    #[test]
    fn cutting_scales_alpha() {
        let mut px = vec![9, 9, 9, 200, 9, 9, 9, 200];
        cut_to(&mut px, (2, 1), &Path::rect(0.0, 0.0, 1.5, 1.0));
        assert_eq!(px[3], 200);
        assert_eq!(px[7], 100);
    }
}
