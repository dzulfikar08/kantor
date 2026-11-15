//! Turned text on screen. GPUI paints glyphs upright only (no transform
//! on `paint_glyph`), so text in a turned shape, and vertical text
//! (`vert`, `vert270`, `eaVert`), is drawn on the CPU instead: each glyph's
//! outline (swash) is turned with the shape and filled (zeno) into a
//! raster at the screen's resolution, which GPUI paints as an image.
//!
//! Shared by Slides (turned shapes, vertical text) and Docs (turned text
//! boxes). Rasters live with their owner (Slides: the prepared slide, one
//! per slide revision), a few zoom levels each (the canvas, thumbnails
//! and the slide show paint the same slide at different sizes). Zoom
//! levels are rounded up to steps of an eighth of a doubling, so zooming
//! re-renders now and then, not every frame; rasters that leave a cache
//! are handed back by [`take_evicted`] to be freed from GPUI's sprite
//! atlas.

use std::cell::RefCell;
use std::sync::Arc;

use docs_layout::parley::FontData;
use gpui_kit::RenderImage;
use slides_scene::{Affine, P, Rgba as SRgba, Text, p as pt};
use swash::scale::ScaleContext;
use swash::zeno::{Mask, Transform};

/// Zoom levels kept per text (canvas, thumbnail, slide show).
const LEVELS: usize = 3;
/// Largest side of a raster, device px.
const MAX_SIDE: f32 = 4096.0;

thread_local! {
    /// Rasters dropped from a cache, to free from the sprite atlas.
    static EVICTED: RefCell<Vec<Arc<RenderImage>>> = const { RefCell::new(Vec::new()) };
}

/// Rasters to free from the sprite atlas (see the module docs).
pub fn take_evicted() -> Vec<Arc<RenderImage>> {
    EVICTED.with(|e| std::mem::take(&mut *e.borrow_mut()))
}

/// A glyph run in the text's own (unturned) frame, slide px.
struct LRun {
    font: FontData,
    size: f32,
    color: SRgba,
    coords: Vec<i16>,
    /// Glyph id and pen position.
    glyphs: Vec<(u32, f32, f32)>,
    underlines: Vec<[P; 4]>,
}

/// Turned text ready to raster (see the module docs).
pub struct TurnedText {
    runs: Vec<LRun>,
    /// Own frame → slide px.
    t: Affine,
    /// Where the raster lands, `[x, y, w, h]` slide px.
    pub bounds: [f32; 4],
    cache: RefCell<Vec<(i32, Arc<RenderImage>)>>,
}

impl Drop for TurnedText {
    fn drop(&mut self) {
        let old: Vec<_> = self.cache.get_mut().drain(..).map(|(_, i)| i).collect();
        if !old.is_empty() {
            EVICTED.with(|e| e.borrow_mut().extend(old));
        }
    }
}

impl TurnedText {
    /// `text` drawn through `t` (not upright); `None` if it shows nothing.
    pub fn new(text: &Text, t: &Affine) -> Option<Self> {
        let o = text.origin;
        let mut lo = (f32::MAX, f32::MAX);
        let mut hi = (f32::MIN, f32::MIN);
        let runs: Vec<LRun> = text
            .runs
            .iter()
            .map(|r| {
                let run = &r.run;
                let s = run.font_size;
                let glyphs: Vec<(u32, f32, f32)> = run
                    .glyphs
                    .iter()
                    .map(|g| {
                        let (x, y) = (o.x + g.x, o.y + g.y);
                        let w = g.advance.max(s * 0.6);
                        lo = (lo.0.min(x - s * 0.3), lo.1.min(y - s * 1.2));
                        hi = (hi.0.max(x + w + s * 0.3), hi.1.max(y + s * 0.5));
                        (g.id, x, y)
                    })
                    .collect();
                let underlines = run
                    .underline
                    .map(|(off, thick)| {
                        let y = o.y + run.baseline + off;
                        let (x0, x1) = (o.x + run.origin_x, o.x + run.origin_x + run.advance);
                        let th = thick.max(0.75);
                        lo = (lo.0.min(x0), lo.1.min(y));
                        hi = (hi.0.max(x1), hi.1.max(y + th));
                        [pt(x0, y), pt(x1, y), pt(x1, y + th), pt(x0, y + th)]
                    })
                    .into_iter()
                    .collect();
                LRun {
                    font: run.font.clone(),
                    size: s,
                    color: run.color,
                    coords: run.normalized_coords.clone(),
                    glyphs,
                    underlines,
                }
            })
            .collect();
        if lo.0 > hi.0 || lo.1 > hi.1 {
            return None;
        }
        let corners = [(lo.0, lo.1), (hi.0, lo.1), (hi.0, hi.1), (lo.0, hi.1)]
            .map(|(x, y)| t.apply(pt(x, y)));
        let x0 = corners.iter().map(|c| c.x).fold(f32::MAX, f32::min) - 1.0;
        let y0 = corners.iter().map(|c| c.y).fold(f32::MAX, f32::min) - 1.0;
        let x1 = corners.iter().map(|c| c.x).fold(f32::MIN, f32::max) + 1.0;
        let y1 = corners.iter().map(|c| c.y).fold(f32::MIN, f32::max) + 1.0;
        Some(Self {
            runs,
            t: *t,
            bounds: [x0, y0, x1 - x0, y1 - y0],
            cache: RefCell::default(),
        })
    }

    /// The raster for `scale` device px per slide px (rounded up to a
    /// cached zoom level).
    pub fn image(&self, scale: f32) -> Option<Arc<RenderImage>> {
        let level = (scale.max(1e-3).log2() * 8.0).ceil() as i32;
        let mut cache = self.cache.borrow_mut();
        if let Some(k) = cache.iter().position(|(l, _)| *l == level) {
            let hit = cache.remove(k);
            let img = hit.1.clone();
            cache.push(hit);
            return Some(img);
        }
        let k = 2f32.powf(level as f32 / 8.0);
        let (px_, w, h) = self.raster(k)?;
        let buf = image::RgbaImage::from_raw(w, h, px_)?;
        let img = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
        if cache.len() >= LEVELS {
            let (_, old) = cache.remove(0);
            EVICTED.with(|e| e.borrow_mut().push(old));
        }
        cache.push((level, img.clone()));
        Some(img)
    }

    /// BGRA pixels (straight alpha, as GPUI's images) at `k` device px per
    /// slide px, and their size.
    pub fn raster(&self, k: f32) -> Option<(Vec<u8>, u32, u32)> {
        let [bx, by, bw, bh] = self.bounds;
        let k = k.min(MAX_SIDE / bw.max(bh).max(1e-3));
        let (w, h) = (
            (bw * k).ceil().max(1.0) as u32,
            (bh * k).ceil().max(1.0) as u32,
        );
        let mut out = vec![0u8; (w * h * 4) as usize];
        let device = self
            .t
            .then(Affine::translate(-bx, -by))
            .then(Affine::scale(k, k));
        let mut ctx = ScaleContext::new();
        for run in &self.runs {
            let color = run.color.0;
            if let Some(font) =
                swash::FontRef::from_index(run.font.data.as_ref(), run.font.index as usize)
            {
                let mut scaler = ctx
                    .builder(font)
                    .size(run.size)
                    .hint(false)
                    .normalized_coords(run.coords.iter())
                    .build();
                for &(id, gx, gy) in &run.glyphs {
                    let Some(outline) = scaler.scale_outline(id as u16) else {
                        continue;
                    };
                    // Outline units are y-up at the pen position.
                    let m = Affine {
                        a: 1.0,
                        b: 0.0,
                        c: 0.0,
                        d: -1.0,
                        e: gx,
                        f: gy,
                    }
                    .then(device);
                    let (mask, at) = Mask::new(outline.path())
                        .transform(Some(zeno_of(&m)))
                        .render();
                    blend(
                        &mut out,
                        (w, h),
                        &mask,
                        (at.left, at.top, at.width, at.height),
                        color,
                    );
                }
            }
            for u in &run.underlines {
                let q: Vec<P> = u.iter().map(|c| device.apply(*c)).collect();
                let path = format!(
                    "M {} {} L {} {} L {} {} L {} {} Z",
                    q[0].x, q[0].y, q[1].x, q[1].y, q[2].x, q[2].y, q[3].x, q[3].y
                );
                let (mask, at) = Mask::new(path.as_str()).render();
                blend(
                    &mut out,
                    (w, h),
                    &mask,
                    (at.left, at.top, at.width, at.height),
                    color,
                );
            }
        }
        Some((out, w, h))
    }
}

fn zeno_of(m: &Affine) -> Transform {
    Transform {
        xx: m.a,
        xy: m.b,
        yx: m.c,
        yy: m.d,
        x: m.e,
        y: m.f,
    }
}

/// Paint `color` through coverage `mask` (placed at `left, top`) over
/// BGRA pixels `out` (straight alpha).
fn blend(
    out: &mut [u8],
    (w, h): (u32, u32),
    mask: &[u8],
    (left, top, mw, mh): (i32, i32, u32, u32),
    [r, g, b, a]: [u8; 4],
) {
    let a = a as f32 / 255.0;
    for j in 0..mh as i32 {
        let y = top + j;
        if y < 0 || y >= h as i32 {
            continue;
        }
        for i in 0..mw as i32 {
            let x = left + i;
            if x < 0 || x >= w as i32 {
                continue;
            }
            let cov = mask[(j as u32 * mw + i as u32) as usize] as f32 / 255.0 * a;
            if cov <= 0.0 {
                continue;
            }
            let d = ((y as u32 * w + x as u32) * 4) as usize;
            let da = out[d + 3] as f32 / 255.0;
            let oa = cov + da * (1.0 - cov);
            let mix = |src: u8, dst: u8| {
                ((src as f32 * cov + dst as f32 * da * (1.0 - cov)) / oa).round() as u8
            };
            out[d] = mix(b, out[d]);
            out[d + 1] = mix(g, out[d + 1]);
            out[d + 2] = mix(r, out[d + 2]);
            out[d + 3] = (oa * 255.0).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn coverage_blends_in_straight_alpha() {
        let mut out = vec![0u8; 2 * 4];
        blend(
            &mut out,
            (2, 1),
            &[255, 128],
            (0, 0, 2, 1),
            [255, 0, 0, 255],
        );
        // BGRA: full red, then half-covered red (colour kept, alpha halved).
        assert_eq!(&out[..4], &[0, 0, 255, 255]);
        assert_eq!(&out[4..7], &[0, 0, 255]);
        assert!((out[7] as i32 - 128).abs() <= 1);
        // Clipped placements are ignored outside the raster.
        blend(
            &mut out,
            (2, 1),
            &[255; 4],
            (-1, -1, 2, 2),
            [0, 0, 255, 255],
        );
        assert_eq!(&out[..4], &[255, 0, 0, 255]);
    }

    /// The fixture's "Rotated text" slide: 4 turned boxes, 3 vertical
    /// ones and 40 small turned ones, rastered at a 1080p canvas scale.
    #[std::prelude::v1::test]
    fn turned_text_on_a_busy_slide_rasters_quickly() {
        use slides_scene::{Builder, Options, Prim};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/pptx/frames.pptx");
        let pres = slides_engine::Presentation::open(&path).unwrap();
        let mut engine = docs_layout::Engine::new();
        let scene = Builder::new().slide(&mut engine, &pres, 2, &Options::default());
        let turned: Vec<TurnedText> = scene
            .items
            .iter()
            .filter_map(|it| match &it.prim {
                Prim::Text(t) if !it.transform.is_upright() => TurnedText::new(t, &it.transform),
                _ => None,
            })
            .collect();
        // The eaVert box is CJK, which has no glyphs where no CJK font is
        // installed (CI).
        let n = turned.len();
        assert!(
            n == 46 || n == 47,
            "every turned and vertical text box: {n}"
        );
        // A 1920-px-wide window shows the 1280-px slide at about 1.2×.
        let start = std::time::Instant::now();
        let mut inked = 0;
        for t in &turned {
            let (px_, w, h) = t.raster(1.2).unwrap();
            assert!(w > 0 && h > 0);
            inked += usize::from(px_.chunks(4).any(|p| p[3] > 0));
        }
        let took = start.elapsed();
        assert_eq!(inked, n, "every raster shows its glyphs");
        eprintln!("{n} turned texts rastered in {took:?}");
        // Debug build; release is several times faster.
        assert!(took.as_millis() < 2000, "{took:?}");
    }

    /// A 90° turn: text that reads down lands in a tall, narrow raster.
    #[std::prelude::v1::test]
    fn a_quarter_turn_makes_a_tall_raster() {
        use slides_scene::{Builder, Options, Prim};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/pptx/frames.pptx");
        let pres = slides_engine::Presentation::open(&path).unwrap();
        let mut engine = docs_layout::Engine::new();
        let scene = Builder::new().slide(&mut engine, &pres, 2, &Options::default());
        let vert = pres
            .slide(2)
            .shapes
            .iter()
            .position(|s| s.name == "Vert")
            .unwrap();
        let t = scene
            .items
            .iter()
            .find_map(|it| match &it.prim {
                Prim::Text(t) if it.layer == Some(slides_engine::Layer::Slide(vert)) => {
                    TurnedText::new(t, &it.transform)
                }
                _ => None,
            })
            .unwrap();
        let [_, _, w, h] = t.bounds;
        assert!(h > 2.0 * w, "{w} × {h}");
        // Cached per zoom level: the same level is one raster.
        let a = t.image(1.01).unwrap();
        let b = t.image(1.05).unwrap();
        assert!(Arc::ptr_eq(&a, &b));
        let _ = t.image(2.0).unwrap();
        let _ = t.image(0.5).unwrap();
        let _ = t.image(4.0).unwrap();
        assert_eq!(t.cache.borrow().len(), LEVELS);
        assert!(!take_evicted().is_empty());
    }
}
