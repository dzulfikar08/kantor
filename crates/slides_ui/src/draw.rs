//! Painting slides with GPUI from their renderer-neutral scenes
//! ([`slides_scene::SlideScene`], the same display list the PDF export
//! writes). [`Painter::painted`] turns a scene into GPUI-ready primitives
//! in slide pixels, cached per slide revision; [`paint_slide`] maps them to
//! the screen at any scale, so the canvas, the thumbnails and presenter
//! mode share one renderer.
//!
//! Where GPUI falls short of the scene (the PDF has none of these limits):
//! - Glyphs and pictures can't be turned at paint time. Text in a turned
//!   shape is placed along the turn with upright glyphs; turned, flipped,
//!   translucent or cropped-and-turned pictures are pre-turned rasters
//!   (nearest pixel, cached).
//! - Gradients: GPUI paths take two-stop linear gradients only. A linear
//!   gradient with more stops is cut into one band per stop pair (the
//!   outline clipped to each band on the CPU), each band painted with its
//!   own two-stop gradient, so the result is exact. Radial and path
//!   gradients are drawn as 32 nested ellipses (solid colour steps), so
//!   translucent radial gradients come out a little denser.
//! - Picture fills of non-rectangular shapes fill the shape's box.
//!
//! Slide content is painted in the file's own colours; only chrome
//! (selection, handles, placeholders for objects Kantor can't draw) uses
//! theme tokens.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use docs_layout::parley::FontData;
use docs_ui::FontBridge;
use gpui_kit::*;
use lyon_tessellation::{LineCap as LCap, LineJoin as LJoin};
use slides_engine::{EMU_PER_PX, LineCap, LineJoin, Presentation, RenderText, Rgb, Slide, Xfrm};
use slides_scene::{
    Affine, Builder, FillRule as SFillRule, LaidText, Options, P, Paint, Prim, Rgba as SRgba, Seg,
    SlideScene, p as pt,
};

/// Largest side of a turned picture's raster, px.
const TURNED_MAX: f32 = 1600.0;
/// Steps of a radial gradient on screen.
const RADIAL_STEPS: usize = 32;

/// A picture `src` (`sw` × `sh` 4-byte pixels) stretched to a `w` × `h`
/// box after cutting `crop` (fractions left, top, right, bottom) off its
/// edges, flipped, then turned clockwise by `angle` radians, resampled
/// (nearest pixel) into the box around the turned one, with its opacity
/// multiplied by `alpha`. Corners outside the picture are transparent.
/// Returns the pixels and their size.
pub(crate) fn turn_pixels(
    src: &[u8],
    (sw, sh): (u32, u32),
    (w, h): (f32, f32),
    angle: f32,
    (flip_h, flip_v): (bool, bool),
    crop: [f32; 4],
    alpha: f32,
) -> (Vec<u8>, u32, u32) {
    let k = (TURNED_MAX.min(sw.max(sh) as f32) / w.max(h).max(1e-3)).max(1e-3);
    let (w, h) = ((w * k).max(1.0), (h * k).max(1.0));
    let (sin, cos) = angle.sin_cos();
    // Less a hair, so 90° turns don't gain a pixel from rounding.
    let bw = (w * cos.abs() + h * sin.abs() - 1e-3).ceil().max(1.0) as u32;
    let bh = (w * sin.abs() + h * cos.abs() - 1e-3).ceil().max(1.0) as u32;
    let [cl, ct, cr, cb] = crop;
    let (kw, kh) = (1.0 - cl - cr, 1.0 - ct - cb);
    let mut out = vec![0u8; (bw * bh * 4) as usize];
    for j in 0..bh {
        for i in 0..bw {
            let u = i as f32 + 0.5 - bw as f32 / 2.0;
            let v = j as f32 + 0.5 - bh as f32 / 2.0;
            // Back into the unturned box.
            let mut lx = u * cos + v * sin + w / 2.0;
            let mut ly = -u * sin + v * cos + h / 2.0;
            if lx < 0.0 || ly < 0.0 || lx >= w || ly >= h {
                continue;
            }
            if flip_h {
                lx = w - lx;
            }
            if flip_v {
                ly = h - ly;
            }
            let fx = cl + lx / w * kw;
            let fy = ct + ly / h * kh;
            if !(0.0..1.0).contains(&fx) || !(0.0..1.0).contains(&fy) {
                continue;
            }
            let sx = ((fx * sw as f32) as u32).min(sw - 1);
            let sy = ((fy * sh as f32) as u32).min(sh - 1);
            let s = ((sy * sw + sx) * 4) as usize;
            let d = ((j * bw + i) * 4) as usize;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
            if alpha < 1.0 {
                out[d + 3] = (out[d + 3] as f32 * alpha).round() as u8;
            }
        }
    }
    (out, bw, bh)
}

pub(crate) fn hsla(c: Rgb) -> Hsla {
    rgba_hsla(SRgba([c.0[0], c.0[1], c.0[2], 255]))
}

fn rgba_hsla(c: SRgba) -> Hsla {
    let [r, g, b, a] = c.0;
    gpui_kit::Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a: a as f32 / 255.,
    }
    .into()
}

/// A glyph run ready to paint (slide px).
pub(crate) struct GRun {
    font: FontData,
    /// Text the run shows (picks the GPUI face).
    sample: String,
    size: f32,
    color: Hsla,
    glyphs: Vec<(u32, P)>,
    underlines: Vec<[P; 4]>,
}

/// A primitive ready for GPUI, in slide px.
pub(crate) enum GPrim {
    Fill {
        segs: Vec<Seg>,
        even_odd: bool,
        bg: Background,
    },
    Stroke {
        segs: Vec<Seg>,
        width: f32,
        color: Hsla,
        dash: Option<Vec<f32>>,
        cap: LineCap,
        join: LineJoin,
    },
    Image {
        /// What shows (`[x, y, w, h]`) and where the whole raster lands.
        bounds: [f32; 4],
        image: [f32; 4],
        img: Arc<RenderImage>,
    },
    Text(Vec<GRun>),
    /// An object Kantor can't draw: a chrome box.
    Other([P; 4]),
    /// The editor's dashed outline of an empty placeholder; `upright` is
    /// its box when not turned.
    Placeholder([P; 4], Option<[f32; 4]>),
}

/// A slide ready to paint.
pub(crate) struct Painted {
    pub prims: Vec<GPrim>,
    pub background: Rgb,
}

type TurnKey = (Arc<str>, i32, bool, i64, [i32; 4], u8);
type SceneKey = (usize, bool, Option<usize>);
/// A cached slide: the revision it was built from, and the result.
type SceneEntry = (Arc<Slide>, Rc<Painted>);

/// Caches shared by every surface of one view.
pub(crate) struct Painter {
    pub engine: Rc<RefCell<docs_layout::Engine>>,
    pub bridge: Rc<RefCell<FontBridge>>,
    builder: Builder,
    images: RefCell<HashMap<Arc<str>, Option<Arc<RenderImage>>>>,
    /// Turned / flipped / faded pictures.
    turned: RefCell<HashMap<TurnKey, Arc<RenderImage>>>,
    /// Rasters dropped from `turned`, to free from the sprite atlas.
    evicted: RefCell<Vec<Arc<RenderImage>>>,
    /// Painted slides by slide revision: the `Arc<Slide>` is kept, so its
    /// address can't be reused while cached and an edit (copy on write)
    /// always gets a new one.
    scenes: RefCell<HashMap<SceneKey, SceneEntry>>,
}

impl Painter {
    pub fn new(engine: Rc<RefCell<docs_layout::Engine>>, bridge: Rc<RefCell<FontBridge>>) -> Self {
        Self {
            engine,
            bridge,
            builder: Builder::new(),
            images: RefCell::default(),
            turned: RefCell::default(),
            evicted: RefCell::default(),
            scenes: RefCell::default(),
        }
    }

    /// Free turned-picture rasters that left the cache.
    pub fn drop_evicted(&self, window: &mut Window) {
        for img in self.evicted.borrow_mut().drain(..) {
            let _ = window.drop_image(img);
        }
    }

    /// Text placement of a resolved body inside `xfrm`.
    pub fn place(&self, t: &RenderText, xfrm: &Xfrm) -> LaidText {
        self.builder.place(&mut self.engine.borrow_mut(), t, xfrm)
    }

    /// The scene of slide `slide` (as the PDF export gets it).
    pub fn scene(&self, pres: &Presentation, slide: usize, opts: &Options) -> SlideScene {
        self.builder
            .slide(&mut self.engine.borrow_mut(), pres, slide, opts)
    }

    /// Slide `slide` ready to paint; cached unless a shape is being dragged.
    pub fn painted(
        &self,
        pres: &Presentation,
        slide: usize,
        opts: Options,
        cx: &App,
    ) -> Rc<Painted> {
        let arc = pres.slides()[slide].clone();
        let key = (Arc::as_ptr(&arc) as usize, opts.prompts, opts.editing);
        if opts.preview.is_none()
            && let Some((_, p)) = self.scenes.borrow().get(&key)
        {
            return p.clone();
        }
        let scene = self.scene(pres, slide, &opts);
        let painted = Rc::new(self.prepare(&scene, cx));
        if opts.preview.is_none() {
            let mut cache = self.scenes.borrow_mut();
            if cache.len() > 400 {
                cache.clear();
            }
            cache.insert(key, (arc, painted.clone()));
        }
        painted
    }

    fn image(&self, part: &Arc<str>, data: &Arc<Vec<u8>>, cx: &App) -> Option<Arc<RenderImage>> {
        if let Some(i) = self.images.borrow().get(part) {
            return i.clone();
        }
        let format = match part.rsplit('.').next()?.to_ascii_lowercase().as_str() {
            "png" => Some(ImageFormat::Png),
            "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
            "gif" => Some(ImageFormat::Gif),
            "bmp" => Some(ImageFormat::Bmp),
            "svg" => Some(ImageFormat::Svg),
            "webp" => Some(ImageFormat::Webp),
            "tif" | "tiff" => Some(ImageFormat::Tiff),
            _ => None,
        };
        let decoded = format.and_then(|format| {
            Image::from_bytes(format, data.as_ref().clone())
                .to_image_data(cx.svg_renderer())
                .map_err(|e| log::warn!("slides: can't decode {part}: {e:#}"))
                .ok()
        });
        self.images
            .borrow_mut()
            .insert(part.clone(), decoded.clone());
        decoded
    }

    /// A picture as drawn into its turned box's bounding box.
    fn turned_image(
        &self,
        pic: &slides_scene::Picture,
        img: &Arc<RenderImage>,
        t: &Affine,
    ) -> Arc<RenderImage> {
        let (angle, flip) = t.angle_flip();
        let deg = (angle.to_degrees().round() as i32).rem_euclid(360);
        let [_, _, w, h] = pic.rect;
        let aspect = (w as f64 / h.max(1e-3) as f64 * 1000.0) as i64;
        let crop = pic.crop.map(|c| (c * 10_000.0) as i32);
        let alpha = (pic.alpha * 255.0).round() as u8;
        let key = (pic.part.clone(), deg, flip, aspect, crop, alpha);
        if let Some(t) = self.turned.borrow().get(&key) {
            return t.clone();
        }
        let size = img.size(0);
        let (sw, sh) = (size.width.0.max(1) as u32, size.height.0.max(1) as u32);
        let Some(bytes) = img
            .as_bytes(0)
            .filter(|b| b.len() >= (sw * sh * 4) as usize)
        else {
            return img.clone();
        };
        let (px_, bw, bh) = turn_pixels(
            bytes,
            (sw, sh),
            (w, h),
            (deg as f32).to_radians(),
            (flip, false),
            pic.crop,
            pic.alpha,
        );
        let Some(buf) = image::RgbaImage::from_raw(bw, bh, px_) else {
            return img.clone();
        };
        let turned = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
        let mut cache = self.turned.borrow_mut();
        if cache.len() >= 24 {
            self.evicted
                .borrow_mut()
                .extend(cache.drain().map(|(_, v)| v));
        }
        cache.insert(key, turned.clone());
        turned
    }

    /// A scene as GPUI primitives.
    fn prepare(&self, scene: &SlideScene, cx: &App) -> Painted {
        let mut prims = Vec::with_capacity(scene.items.len());
        for item in &scene.items {
            let t = &item.transform;
            match &item.prim {
                Prim::Fill { path, paint, rule } => {
                    let segs = path.transformed(t).segs;
                    let even_odd = *rule == SFillRule::EvenOdd;
                    match paint {
                        Paint::Solid(c) => prims.push(GPrim::Fill {
                            segs,
                            even_odd,
                            bg: rgba_hsla(*c).into(),
                        }),
                        _ => gradient_bands(&segs, paint, t, even_odd, &mut prims),
                    }
                }
                Prim::Stroke { path, stroke } => prims.push(GPrim::Stroke {
                    segs: path.transformed(t).segs,
                    width: stroke.width,
                    color: rgba_hsla(stroke.color),
                    dash: stroke.dash.clone(),
                    cap: stroke.cap,
                    join: stroke.join,
                }),
                Prim::Picture(pic) => {
                    let Some(img) = self.image(&pic.part, &pic.data, cx) else {
                        let [x, y, w, h] = pic.rect;
                        prims.push(GPrim::Other(corners(t, x, y, w, h)));
                        continue;
                    };
                    let [x, y, w, h] = pic.rect;
                    if t.is_upright() && pic.alpha >= 0.999 {
                        let a = t.apply(pt(x, y));
                        let b = t.apply(pt(x + w, y + h));
                        let [ix, iy, iw, ih] = pic.image_rect();
                        let ia = t.apply(pt(ix, iy));
                        let ib = t.apply(pt(ix + iw, iy + ih));
                        prims.push(GPrim::Image {
                            bounds: [a.x, a.y, b.x - a.x, b.y - a.y],
                            image: [ia.x, ia.y, ib.x - ia.x, ib.y - ia.y],
                            img,
                        });
                    } else {
                        let c = corners(t, x, y, w, h);
                        let bb = bbox(&c);
                        prims.push(GPrim::Image {
                            bounds: bb,
                            image: bb,
                            img: self.turned_image(pic, &img, t),
                        });
                    }
                }
                Prim::Text(text) => {
                    let runs = text
                        .runs
                        .iter()
                        .map(|r| {
                            let run = &r.run;
                            let at =
                                |x: f32, y: f32| t.apply(pt(text.origin.x + x, text.origin.y + y));
                            let underlines = run
                                .underline
                                .map(|(off, thick)| {
                                    let y = run.baseline + off;
                                    let (x0, x1) = (run.origin_x, run.origin_x + run.advance);
                                    let th = thick.max(0.75);
                                    vec![at(x0, y), at(x1, y), at(x1, y + th), at(x0, y + th)]
                                        .try_into()
                                        .expect("four corners")
                                })
                                .into_iter()
                                .collect();
                            GRun {
                                font: run.font.clone(),
                                sample: r.sample().to_owned(),
                                size: run.font_size,
                                color: rgba_hsla(run.color),
                                glyphs: run.glyphs.iter().map(|g| (g.id, at(g.x, g.y))).collect(),
                                underlines,
                            }
                        })
                        .collect();
                    prims.push(GPrim::Text(runs));
                }
                Prim::Unsupported { w, h } => {
                    prims.push(GPrim::Other(corners(t, 0.0, 0.0, *w, *h)))
                }
                Prim::PlaceholderOutline { w, h } => {
                    let c = corners(t, 0.0, 0.0, *w, *h);
                    let upright = t.is_upright().then(|| bbox(&c));
                    prims.push(GPrim::Placeholder(c, upright));
                }
            }
        }
        Painted {
            prims,
            background: scene.background,
        }
    }
}

fn corners(t: &Affine, x: f32, y: f32, w: f32, h: f32) -> [P; 4] {
    [
        t.apply(pt(x, y)),
        t.apply(pt(x + w, y)),
        t.apply(pt(x + w, y + h)),
        t.apply(pt(x, y + h)),
    ]
}

fn bbox(c: &[P]) -> [f32; 4] {
    let x0 = c.iter().map(|q| q.x).fold(f32::MAX, f32::min);
    let y0 = c.iter().map(|q| q.y).fold(f32::MAX, f32::min);
    let x1 = c.iter().map(|q| q.x).fold(f32::MIN, f32::max);
    let y1 = c.iter().map(|q| q.y).fold(f32::MIN, f32::max);
    [x0, y0, x1 - x0, y1 - y0]
}

/// Keep the part of polygon `poly` where `f(q) >= 0` (`f` linear).
fn clip_poly(poly: &[P], f: impl Fn(P) -> f32) -> Vec<P> {
    let mut out = Vec::with_capacity(poly.len() + 4);
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        let (fa, fb) = (f(a), f(b));
        if fa >= 0.0 {
            out.push(a);
        }
        if (fa >= 0.0) != (fb >= 0.0) {
            let k = fa / (fa - fb);
            out.push(a.plus(b.minus(a).scale(k)));
        }
    }
    out
}

fn poly_segs(polys: &[Vec<P>]) -> Vec<Seg> {
    let mut segs = Vec::new();
    for poly in polys.iter().filter(|p| p.len() >= 3) {
        segs.push(Seg::Move(poly[0]));
        segs.extend(poly[1..].iter().map(|&q| Seg::Line(q)));
        segs.push(Seg::Close);
    }
    segs
}

/// A GPUI two-stop linear gradient over a region with bounding box
/// `[x, y, w, h]` whose colour runs along unit vector `u`, `c0` where
/// the projection on `u` is `s0`, `c1` at `s1`. GPUI measures its
/// gradients against the painted path's box in its own way (see
/// `gradient_color` in GPUI's shaders); this picks the angle and stop
/// percentages that land the stops exactly.
fn band_background(b: [f32; 4], u: P, (s0, c0): (f32, Hsla), (s1, c1): (f32, Hsla)) -> Background {
    let (angle, p0, p1) = band_params(b, u, s0, s1);
    linear_gradient(angle, linear_color_stop(c0, p0), linear_color_stop(c1, p1))
}

/// GPUI angle and stop percentages for [`band_background`].
fn band_params([x, y, w, h]: [f32; 4], u: P, s0: f32, s1: f32) -> (f32, f32, f32) {
    let (w, h) = (w.max(1e-3), h.max(1e-3));
    let (kx, ky) = if w > h { (1.0, h / w) } else { (w / h, 1.0) };
    let r = (u.y / ky).atan2(u.x / kx);
    let dir = pt(r.cos() * kx, r.sin() * ky);
    let size = if dir.x.abs() > dir.y.abs() { w } else { h };
    let sc = pt(x + w / 2.0, y + h / 2.0).dot(u);
    let pct = |s: f32| (s - sc + size / 2.0) / size;
    (r.to_degrees() + 90.0, pct(s0), pct(s1))
}

/// A gradient fill as GPUI primitives (see the module docs).
fn gradient_bands(segs: &[Seg], paint: &Paint, t: &Affine, even_odd: bool, out: &mut Vec<GPrim>) {
    let path = slides_scene::Path {
        segs: segs.to_vec(),
    };
    let polys: Vec<Vec<P>> = path.flatten(0.25).into_iter().map(|(p, _)| p).collect();
    match paint {
        Paint::Solid(_) => {}
        Paint::Linear { from, to, stops } => {
            let (a, b) = (t.apply(*from), t.apply(*to));
            let len = b.minus(a).len().max(1e-4);
            let u = b.minus(a).scale(1.0 / len);
            let s_of = |o: f32| a.dot(u) + o * len;
            for k in 0..stops.len() - 1 {
                let (lo, hi) = (s_of(stops[k].offset), s_of(stops[k + 1].offset));
                let first = k == 0;
                let last = k + 2 == stops.len();
                let band: Vec<Vec<P>> = polys
                    .iter()
                    .map(|poly| {
                        let mut q = poly.clone();
                        if !first {
                            q = clip_poly(&q, |v| v.dot(u) - lo);
                        }
                        if !last {
                            q = clip_poly(&q, |v| hi - v.dot(u));
                        }
                        q
                    })
                    .collect();
                let segs = poly_segs(&band);
                let all: Vec<P> = band.concat();
                if segs.is_empty() || all.is_empty() {
                    continue;
                }
                let bg = band_background(
                    bbox(&all),
                    u,
                    (lo, rgba_hsla(stops[k].color)),
                    (hi.max(lo + 1e-3), rgba_hsla(stops[k + 1].color)),
                );
                out.push(GPrim::Fill { segs, even_odd, bg });
            }
        }
        Paint::Radial {
            center,
            rx,
            ry,
            stops,
        } => {
            let color_at = |o: f32| -> Hsla {
                let (mut lo, mut hi) = (stops[0], stops[stops.len() - 1]);
                for w in stops.windows(2) {
                    if o >= w[0].offset && o <= w[1].offset {
                        (lo, hi) = (w[0], w[1]);
                        break;
                    }
                }
                let k = if o <= lo.offset {
                    0.0
                } else if o >= hi.offset || hi.offset <= lo.offset {
                    1.0
                } else {
                    (o - lo.offset) / (hi.offset - lo.offset)
                };
                let [r0, g0, b0, a0] = lo.color.0;
                let [r1, g1, b1, a1] = hi.color.0;
                let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * k).round() as u8;
                rgba_hsla(SRgba([m(r0, r1), m(g0, g1), m(b0, b1), m(a0, a1)]))
            };
            // Outermost: the last colour everywhere beyond radius 1.
            out.push(GPrim::Fill {
                segs: segs.to_vec(),
                even_odd,
                bg: color_at(1.0).into(),
            });
            for i in 1..RADIAL_STEPS {
                let r = 1.0 - i as f32 / RADIAL_STEPS as f32;
                // An ellipse polygon (in the shape's frame, then on the slide).
                let ell: Vec<P> = (0..48)
                    .map(|j| {
                        let a = j as f32 / 48.0 * std::f32::consts::TAU;
                        t.apply(pt(center.x + rx * r * a.cos(), center.y + ry * r * a.sin()))
                    })
                    .collect();
                // Clip by each edge (the ellipse is convex either way round).
                let c = t.apply(*center);
                let clipped: Vec<Vec<P>> = polys
                    .iter()
                    .map(|poly| {
                        let mut q = poly.clone();
                        for e in 0..ell.len() {
                            let (a, b) = (ell[e], ell[(e + 1) % ell.len()]);
                            let n = pt(-(b.y - a.y), b.x - a.x);
                            let side = if c.minus(a).dot(n) >= 0.0 { 1.0 } else { -1.0 };
                            q = clip_poly(&q, |v| v.minus(a).dot(n) * side);
                            if q.is_empty() {
                                break;
                            }
                        }
                        q
                    })
                    .collect();
                let segs = poly_segs(&clipped);
                if segs.is_empty() {
                    continue;
                }
                out.push(GPrim::Fill {
                    segs,
                    even_odd,
                    bg: color_at(r - 0.5 / RADIAL_STEPS as f32).into(),
                });
            }
        }
    }
}

/// Slide → screen mapping: `origin` is the slide's top left on screen,
/// `scale` screen px per EMU.
#[derive(Clone, Copy, Debug)]
pub(crate) struct View {
    pub origin: Point<Pixels>,
    pub scale: f32,
}

impl View {
    pub fn point(self, x: f32, y: f32) -> Point<Pixels> {
        point(
            self.origin.x + px(x * self.scale),
            self.origin.y + px(y * self.scale),
        )
    }

    pub fn rect(self, x: &Xfrm) -> Bounds<Pixels> {
        Bounds::from_corners(
            self.point(x.x as f32, x.y as f32),
            self.point((x.x + x.cx) as f32, (x.y + x.cy) as f32),
        )
    }

    /// A screen point in EMU.
    pub fn to_emu(self, p: Point<Pixels>) -> (i64, i64) {
        (
            (f32::from(p.x - self.origin.x) / self.scale) as i64,
            (f32::from(p.y - self.origin.y) / self.scale) as i64,
        )
    }

    /// Screen px per slide px.
    pub fn px_scale(self) -> f32 {
        self.scale * EMU_PER_PX
    }

    /// A point in slide px on screen.
    fn slide_px(self, q: P) -> Point<Pixels> {
        let k = self.px_scale();
        point(self.origin.x + px(q.x * k), self.origin.y + px(q.y * k))
    }

    fn slide_rect(self, [x, y, w, h]: [f32; 4]) -> Bounds<Pixels> {
        let k = self.px_scale();
        Bounds::new(self.slide_px(pt(x, y)), size(px(w * k), px(h * k)))
    }
}

/// A shape's rotation on screen: about the centre of its box.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Turn {
    pub c: Point<Pixels>,
    sin: f32,
    cos: f32,
}

impl Turn {
    /// `None` for an unrotated shape.
    pub fn of(x: &Xfrm, view: View) -> Option<Self> {
        (x.rot != 0).then(|| {
            let (sin, cos) = (x.angle() as f32).sin_cos();
            Turn {
                c: view.rect(x).center(),
                sin,
                cos,
            }
        })
    }

    pub fn apply(&self, p: Point<Pixels>) -> Point<Pixels> {
        let (dx, dy) = (f32::from(p.x - self.c.x), f32::from(p.y - self.c.y));
        point(
            self.c.x + px(dx * self.cos - dy * self.sin),
            self.c.y + px(dx * self.sin + dy * self.cos),
        )
    }

    /// The inverse of [`Turn::apply`].
    pub fn unapply(&self, p: Point<Pixels>) -> Point<Pixels> {
        let (dx, dy) = (f32::from(p.x - self.c.x), f32::from(p.y - self.c.y));
        point(
            self.c.x + px(dx * self.cos + dy * self.sin),
            self.c.y + px(-dx * self.sin + dy * self.cos),
        )
    }
}

pub(crate) fn turned(t: Option<Turn>, p: Point<Pixels>) -> Point<Pixels> {
    t.map_or(p, |t| t.apply(p))
}

fn rect_points(b: Bounds<Pixels>) -> Vec<Point<Pixels>> {
    vec![b.origin, b.top_right(), b.bottom_right(), b.bottom_left()]
}

/// Fill a polygon (screen space).
pub(crate) fn fill_polygon(window: &mut Window, pts: &[Point<Pixels>], color: Hsla) {
    let mut p = PathBuilder::fill();
    p.add_polygon(pts, true);
    if let Ok(path) = p.build() {
        window.paint_path(path, color);
    }
}

/// Stroke a closed polygon (screen space).
pub(crate) fn stroke_polygon(window: &mut Window, pts: &[Point<Pixels>], color: Hsla, w: Pixels) {
    let mut p = PathBuilder::stroke(w);
    p.add_polygon(pts, true);
    if let Ok(path) = p.build() {
        window.paint_path(path, color);
    }
}

/// The corners of a box turned with its shape.
pub(crate) fn turned_rect(b: Bounds<Pixels>, t: Option<Turn>) -> Vec<Point<Pixels>> {
    rect_points(b).into_iter().map(|p| turned(t, p)).collect()
}

/// How `paint_slide` draws: `other` is the colour for objects Kantor
/// can't draw yet (a chrome token); `editor` adds the editor-only dashed
/// outline around empty placeholders (not in thumbnails or presenting).
#[derive(Clone, Copy)]
pub(crate) struct Surface {
    pub other: Hsla,
    pub editor: bool,
}

/// Dashed outline of an empty placeholder: grey that reads on the slide's
/// own background (dark on light slides, light on dark ones).
pub(crate) fn placeholder_outline(background: Rgb) -> Hsla {
    let [r, g, b] = background.0;
    let lum = (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0;
    let l = if lum > 0.5 { 0.0 } else { 1.0 };
    Hsla {
        h: 0.,
        s: 0.,
        l,
        a: 0.38,
    }
}

fn build_segs(pb: &mut PathBuilder, segs: &[Seg], view: View) {
    for s in segs {
        match *s {
            Seg::Move(q) => pb.move_to(view.slide_px(q)),
            Seg::Line(q) => pb.line_to(view.slide_px(q)),
            Seg::Quad(c, q) => pb.curve_to(view.slide_px(q), view.slide_px(c)),
            Seg::Cubic(a, b, q) => {
                pb.cubic_bezier_to(view.slide_px(q), view.slide_px(a), view.slide_px(b))
            }
            Seg::Close => pb.close(),
        }
    }
}

/// Paint a prepared slide with `view` into `slide_bounds`.
pub(crate) fn paint_slide(
    painted: &Painted,
    slide_bounds: Bounds<Pixels>,
    view: View,
    surface: Surface,
    bridge: &RefCell<FontBridge>,
    window: &mut Window,
) {
    let other = surface.other;
    let k = view.px_scale();
    window.paint_quad(fill(slide_bounds, hsla(painted.background)));
    let screen = |c: &[P; 4]| c.iter().map(|&q| view.slide_px(q)).collect::<Vec<_>>();
    window.with_content_mask(
        Some(ContentMask {
            bounds: slide_bounds,
        }),
        |window| {
            for prim in &painted.prims {
                match prim {
                    GPrim::Fill { segs, even_odd, bg } => {
                        let opts = if *even_odd {
                            FillOptions::even_odd()
                        } else {
                            FillOptions::non_zero()
                        };
                        let mut pb = PathBuilder::fill().with_style(PathStyle::Fill(opts));
                        build_segs(&mut pb, segs, view);
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, *bg);
                        }
                    }
                    GPrim::Stroke {
                        segs,
                        width,
                        color,
                        dash,
                        cap,
                        join,
                    } => {
                        let w = (width * k).max(1.0);
                        let lcap = match cap {
                            LineCap::Flat => LCap::Butt,
                            LineCap::Round => LCap::Round,
                            LineCap::Square => LCap::Square,
                        };
                        let opts = StrokeOptions::default()
                            .with_line_width(w)
                            .with_line_cap(lcap)
                            .with_line_join(match join {
                                LineJoin::Round => LJoin::Round,
                                LineJoin::Bevel => LJoin::Bevel,
                                LineJoin::Miter => LJoin::MiterClip,
                            });
                        let mut pb = PathBuilder::stroke(px(w)).with_style(PathStyle::Stroke(opts));
                        if let Some(d) = dash {
                            let d: Vec<Pixels> = d.iter().map(|x| px((x * k).max(0.5))).collect();
                            pb = pb.dash_array(&d);
                        }
                        build_segs(&mut pb, segs, view);
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, *color);
                        }
                    }
                    GPrim::Image { bounds, image, img } => {
                        let _ = window.paint_image(
                            view.slide_rect(*bounds),
                            view.slide_rect(*image),
                            Corners::default(),
                            img.clone(),
                            0,
                            false,
                        );
                    }
                    GPrim::Text(runs) => paint_runs(runs, view, bridge, window),
                    GPrim::Other(c) => {
                        let pts = screen(c);
                        fill_polygon(window, &pts, other.opacity(0.12));
                        stroke_polygon(window, &pts, other.opacity(0.6), px(1.));
                    }
                    GPrim::Placeholder(c, upright) => {
                        if !surface.editor {
                            continue;
                        }
                        let color = placeholder_outline(painted.background);
                        match upright {
                            Some(b) => window.paint_quad(quad(
                                view.slide_rect(*b),
                                Corners::default(),
                                transparent_black(),
                                Edges::all(px(1.)),
                                color,
                                BorderStyle::Dashed,
                            )),
                            None => stroke_polygon(window, &screen(c), color, px(1.)),
                        }
                    }
                }
            }
        },
    );
}

pub(crate) fn text_origin(t: &LaidText, xfrm: &Xfrm, view: View) -> Point<Pixels> {
    view.point(
        xfrm.x as f32 + t.dx * EMU_PER_PX,
        xfrm.y as f32 + t.dy * EMU_PER_PX,
    )
}

/// Glyphs upright at their (possibly turned) places: GPUI can't turn glyphs.
fn paint_runs(runs: &[GRun], view: View, bridge: &RefCell<FontBridge>, window: &mut Window) {
    let k = view.px_scale();
    let ts = window.text_system().clone();
    let mut bridge = bridge.borrow_mut();
    for run in runs {
        let Some(font_id) = bridge.font_id(&run.font, &run.sample, &ts) else {
            continue;
        };
        let size = px(run.size * k);
        for (id, at) in &run.glyphs {
            let _ = window.paint_glyph(view.slide_px(*at), font_id, GlyphId(*id), size, run.color);
        }
        for u in &run.underlines {
            let pts: Vec<Point<Pixels>> = u.iter().map(|&q| view.slide_px(q)).collect();
            fill_polygon(window, &pts, run.color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2 × 1 picture (red, blue) turned 90° clockwise is 1 × 2 with red
    /// on top; flipped horizontally it's (blue, red).
    #[std::prelude::v1::test]
    fn pictures_turn_and_flip() {
        let red = [255, 0, 0, 255];
        let blue = [0, 0, 255, 255];
        let src: Vec<u8> = [red, blue].concat();
        let none = [0.0; 4];
        let (px_, w, h) = turn_pixels(
            &src,
            (2, 1),
            (2.0, 1.0),
            90f32.to_radians(),
            (false, false),
            none,
            1.0,
        );
        assert_eq!((w, h), (1, 2));
        assert_eq!(&px_[0..4], &red);
        assert_eq!(&px_[4..8], &blue);
        let (px_, w, h) = turn_pixels(&src, (2, 1), (2.0, 1.0), 0.0, (true, false), none, 1.0);
        assert_eq!((w, h), (2, 1));
        assert_eq!(&px_[0..4], &blue);
        // 45°: the corners of the bounding box are transparent.
        let (px_, w, _) = turn_pixels(
            &src,
            (2, 1),
            (20.0, 20.0),
            45f32.to_radians(),
            (false, false),
            none,
            1.0,
        );
        assert_eq!(&px_[0..4], &[0, 0, 0, 0]);
        assert!(w > 2);
        // Cropping the left half shows only blue; alpha fades it.
        let (px_, _, _) = turn_pixels(
            &src,
            (2, 1),
            (2.0, 1.0),
            0.0,
            (false, false),
            [0.5, 0.0, 0.0, 0.0],
            0.5,
        );
        assert_eq!(&px_[0..4], &[0, 0, 255, 128]);
        assert_eq!(&px_[4..8], &[0, 0, 255, 128]);
    }

    #[std::prelude::v1::test]
    fn turn_round_trips() {
        let x = Xfrm::new(0, 0, 1000, 500).with_degrees(30.0);
        let view = View {
            origin: point(px(10.), px(10.)),
            scale: 0.1,
        };
        let t = Turn::of(&x, view).unwrap();
        let p = point(px(17.), px(3.));
        let back = t.unapply(t.apply(p));
        assert!((f32::from(back.x) - 17.).abs() < 1e-3 && (f32::from(back.y) - 3.).abs() < 1e-3);
    }

    /// Mirrors GPUI's `gradient_color` shader: position 0..1 of a point.
    fn gpui_t(bg_angle: f32, b: [f32; 4], q: P) -> f32 {
        let [x, y, w, h] = b;
        let r = (bg_angle - 90.0).to_radians();
        let mut d = pt(r.cos(), r.sin());
        if w > h {
            d.y *= h / w;
        } else {
            d.x *= w / h;
        }
        let c = pt(x + w / 2.0, y + h / 2.0);
        let t = q.minus(c).dot(d) / d.len();
        if d.x.abs() > d.y.abs() {
            (t + w / 2.0) / w
        } else {
            (t + h / 2.0) / h
        }
    }

    /// Band gradients put their stops where asked, for any direction and
    /// box shape.
    #[std::prelude::v1::test]
    fn band_gradients_land_their_stops() {
        for (deg, b) in [
            (0.0f32, [0.0, 0.0, 200.0, 100.0]),
            (30.0, [10.0, 20.0, 50.0, 300.0]),
            (135.0, [-5.0, 0.0, 80.0, 80.0]),
            (260.0, [0.0, 0.0, 400.0, 30.0]),
        ] {
            let u = pt(deg.to_radians().cos(), deg.to_radians().sin());
            let c = pt(b[0] + b[2] / 2.0, b[1] + b[3] / 2.0);
            let (s0, s1) = (c.dot(u) - 7.0, c.dot(u) + 11.0);
            let (angle, p0, p1) = band_params(b, u, s0, s1);
            let pcts = [p0, p1];
            let dbg = format!("{angle} {pcts:?}");
            // Points on the stop lines: centre moved along u.
            let q0 = c.plus(u.scale(-7.0)).plus(pt(-u.y, u.x).scale(3.0));
            let q1 = c.plus(u.scale(11.0));
            assert!((gpui_t(angle, b, q0) - pcts[0]).abs() < 1e-3, "{deg} {dbg}");
            assert!((gpui_t(angle, b, q1) - pcts[1]).abs() < 1e-3, "{deg} {dbg}");
        }
    }

    #[std::prelude::v1::test]
    fn polygons_clip_to_half_planes() {
        let sq = vec![pt(0.0, 0.0), pt(10.0, 0.0), pt(10.0, 10.0), pt(0.0, 10.0)];
        let left = clip_poly(&sq, |q| 4.0 - q.x);
        let b = bbox(&left);
        assert_eq!(b, [0.0, 0.0, 4.0, 10.0]);
        assert!(clip_poly(&sq, |q| q.x - 20.0).is_empty());
    }
}
