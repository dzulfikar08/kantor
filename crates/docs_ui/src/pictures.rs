//! Pictures for the screen (the body's, floating objects', headers' and
//! footers' and text boxes'), decoded on a background thread at about the
//! size they're shown (a photo is often several times larger than its
//! box): the page draws nothing in their place until they're ready, a few
//! pages ahead are decoded before they scroll into view, and those not
//! shown for a while are let go (a long document can have hundreds).
//! Printing and PDF export read the document's own bytes, at full size.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::DocsView;

/// Decoded pictures kept at most (more are let go, least recently shown
/// first, once not shown for [`KEEP`]): a screen and the pages decoded
/// ahead seldom have more than a dozen.
const MAX: usize = 24;
const KEEP: Duration = Duration::from_secs(4);
/// Pictures decoded at once (each needs its full-size pixels for a
/// moment: many at once while scrolling fast swelled the heap).
const RUNNING: usize = 2;
/// A waiting picture not asked for again within this long has scrolled
/// out of view: it isn't decoded (asked again, it waits again).
/// (Frames ask for what they show every frame; a view drawing no frames
/// draws one when a decode finishes, which asks again.)
const STALE: Duration = Duration::from_millis(250);

/// Pictures with at most this many pixels are never made smaller (1 MB
/// decoded).
const SMALL: u64 = 512 * 512;

/// A picture waiting to be decoded.
struct Job {
    key: String,
    bytes: Arc<[u8]>,
    side: u32,
    asked: Instant,
}

/// A decoded picture: its image, the longest side it was decoded for
/// (`u32::MAX` when it's at its own size) and when it was last shown.
struct Ready {
    img: Arc<RenderImage>,
    side: u32,
    used: Instant,
}

#[derive(Default)]
pub(crate) struct Pictures {
    ready: HashMap<String, Ready>,
    /// Being decoded or waiting to be.
    loading: HashSet<String>,
    /// Waiting, the most recently asked for decoded first.
    queue: Vec<Job>,
    running: usize,
    /// Images replaced by sharper ones, given back to the window on the
    /// next trim.
    retired: Vec<Arc<RenderImage>>,
    /// Keys whose images were replaced: their turned and cut copies (in
    /// `turned_images`, keyed `key` or `key@…`) are made again.
    replaced: Vec<String>,
    /// When the cache was last reported (`KANTOR_PERF`).
    reported: Option<Instant>,
    /// A frame is asked for once the pictures over [`MAX`] may be let go
    /// (a view that stopped scrolling draws no frames, and only a frame
    /// can give images back to the window).
    wake: bool,
}

impl Pictures {
    /// Waiting picture `key` was asked for again (it's still in view).
    fn asked(&mut self, key: &str) {
        if let Some(j) = self.queue.iter_mut().find(|j| j.key == key) {
            j.asked = Instant::now();
        }
    }
}

/// Bytes of a decoded image.
fn image_bytes(img: &RenderImage) -> usize {
    let s = img.size(0);
    s.width.0.max(0) as usize * s.height.0.max(0) as usize * 4
}

/// `bytes` (PNG or JPEG) decoded with no side longer than `side` px (as
/// GPUI wants it: BGRA).
fn decode(bytes: &[u8], side: u32) -> Option<(Arc<RenderImage>, u32)> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    // Small ones (logos, icons) at their own size: little to save, and
    // fine detail (thin lines) would look different.
    let small = u64::from(w) * u64::from(h) <= SMALL;
    let scaled = w.max(h) > side && side > 0 && !small;
    let rgba = if scaled {
        let scale = side as f32 / w.max(h) as f32;
        let (tw, th) = (
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
        );
        // Straight from the decoded pixels (no full-size RGBA copy).
        image::imageops::thumbnail(&img, tw, th)
    } else {
        img.to_rgba8()
    };
    let mut rgba = rgba;
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    let img = Arc::new(RenderImage::new(vec![image::Frame::new(rgba)]));
    Some((img, if scaled { side } else { u32::MAX }))
}

impl DocsView {
    /// Picture `rel_id` of the body for a box `box_px` px at its largest:
    /// decoded (`None` while it's being decoded, or when it isn't a PNG or
    /// JPEG: the caller draws those its own way).
    pub(crate) fn screen_picture_of(
        &mut self,
        rel_id: &str,
        box_px: f32,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        let bytes = self.doc.image_bytes(rel_id);
        self.screen_picture_from(rel_id, box_px, bytes, cx)
    }

    /// The picture cached as `key` (the body's by relationship id, other
    /// stories' with a prefix) for a box `box_px` px at its largest, made
    /// from `bytes` when it isn't decoded yet: `None` while it's being
    /// decoded, or when it isn't a PNG or JPEG. Shown larger than it was
    /// decoded for, it's decoded again (the old one shown meanwhile).
    pub(crate) fn screen_picture_from(
        &mut self,
        key: &str,
        box_px: f32,
        bytes: Option<Arc<[u8]>>,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        // Sharp at 200 % (zoom or a HiDPI screen).
        let side = (box_px * 2.0).ceil().max(64.0) as u32;
        let p = &mut self.pictures;
        let shown = match p.ready.get_mut(key) {
            Some(r) => {
                r.used = Instant::now();
                let (img, sharp) = (r.img.clone(), r.side >= side);
                if sharp {
                    return Some(img);
                }
                if p.loading.contains(key) {
                    p.asked(key);
                    return Some(img);
                }
                // Shown larger than it was decoded for: this one until
                // the sharper one is ready.
                Some(img)
            }
            None if p.loading.contains(key) => {
                p.asked(key);
                return None;
            }
            None => None,
        };
        let Some(bytes) = bytes.filter(|b| {
            matches!(
                docs_engine::ImageFormat::sniff(b),
                docs_engine::ImageFormat::Jpeg | docs_engine::ImageFormat::Png
            )
        }) else {
            return shown;
        };
        let key = key.to_owned();
        self.pictures.loading.insert(key.clone());
        self.pictures.queue.push(Job {
            key,
            bytes,
            side,
            asked: Instant::now(),
        });
        self.run_decodes(cx);
        shown
    }

    /// Start decoding waiting pictures, up to [`RUNNING`] at once.
    fn run_decodes(&mut self, cx: &mut Context<Self>) {
        let p = &mut self.pictures;
        let loading = &mut p.loading;
        p.queue.retain(|j| {
            let keep = j.asked.elapsed() < STALE;
            if !keep {
                loading.remove(&j.key);
            }
            keep
        });
        while p.running < RUNNING {
            let Some(next) = (0..p.queue.len()).max_by_key(|&i| p.queue[i].asked) else {
                break;
            };
            let Job {
                key, bytes, side, ..
            } = p.queue.swap_remove(next);
            p.running += 1;
            let task = cx.background_spawn(async move { decode(&bytes, side) });
            cx.spawn(async move |this, cx| {
                let img = task.await;
                let _ = this.update(cx, |this, cx| {
                    let p = &mut this.pictures;
                    p.running -= 1;
                    p.loading.remove(&key);
                    if let Some((img, side)) = img {
                        let new = Ready {
                            img,
                            side,
                            used: Instant::now(),
                        };
                        if let Some(old) = p.ready.insert(key.clone(), new) {
                            p.retired.push(old.img);
                            p.replaced.push(key);
                        }
                    }
                    // Also when it failed: pictures still in view that
                    // waited too long are asked for again by the frame.
                    cx.notify();
                    this.run_decodes(cx);
                });
            })
            .detach();
        }
    }

    /// Whether picture `key` is decoded by [`Self::screen_picture_from`]
    /// (or being decoded): the old way isn't needed for it.
    pub(crate) fn picture_decoding(&self, key: &str) -> bool {
        self.pictures.ready.contains_key(key) || self.pictures.loading.contains(key)
    }

    /// Let go of decoded pictures not shown for a while (beyond the most
    /// recent [`MAX`]), and of those replaced by sharper ones.
    pub(crate) fn trim_pictures(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = &mut self.pictures;
        for img in p.retired.drain(..) {
            window.drop_image(img).ok();
        }
        if !p.replaced.is_empty() {
            let replaced = std::mem::take(&mut p.replaced);
            let turned = &mut self.turned_images;
            let stale: Vec<String> = turned
                .keys()
                .filter(|k| {
                    replaced.iter().any(|r| {
                        k.strip_prefix(r.as_str())
                            .is_some_and(|rest| rest.is_empty() || rest.starts_with('@'))
                    })
                })
                .cloned()
                .collect();
            for k in stale {
                if let Some(img) = turned.remove(&k) {
                    window.drop_image(img).ok();
                }
            }
        }
        let p = &mut self.pictures;
        if p.ready.len() <= MAX {
            return;
        }
        let mut by_age: Vec<(Instant, String)> =
            p.ready.iter().map(|(k, r)| (r.used, k.clone())).collect();
        by_age.sort();
        let extra = p.ready.len() - MAX;
        let mut dropped = false;
        for (t, k) in by_age.into_iter().take(extra) {
            if t.elapsed() < KEEP {
                break;
            }
            if let Some(r) = p.ready.remove(&k) {
                window.drop_image(r.img).ok();
                dropped = true;
            }
        }
        if dropped {
            // The decodes ran on the background threads' heaps, which keep
            // the freed pages otherwise.
            kantor_mem::release_free_memory_soon(Duration::from_secs(2));
        }
        if p.ready.len() > MAX && !p.wake {
            p.wake = true;
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(KEEP + Duration::from_millis(500))
                    .await;
                let _ = this.update(cx, |this, cx| {
                    this.pictures.wake = false;
                    cx.notify();
                });
            })
            .detach();
        }
    }

    /// With `KANTOR_PERF`: the picture caches' sizes and the resident
    /// memory, every few seconds while frames are drawn.
    pub(crate) fn report_pictures(&mut self) {
        if self.perf.is_none() {
            return;
        }
        let p = &mut self.pictures;
        if p.reported
            .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        p.reported = Some(Instant::now());
        let mb = |b: usize| b as f64 / (1024.0 * 1024.0);
        let pics: usize = p.ready.values().map(|r| image_bytes(&r.img)).sum();
        let turned: usize = self.turned_images.values().map(|i| image_bytes(i)).sum();
        eprintln!(
            "[docs] mem pictures {} ({:.1} MB, {} decoding), turned {} ({:.1} MB), gpui images {}, turned texts {}, rss {} kB",
            p.ready.len(),
            mb(pics),
            p.loading.len(),
            self.turned_images.len(),
            mb(turned),
            self.images.len(),
            self.turned_texts.len(),
            crate::perf::rss_kb()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[std::prelude::v1::test]
    fn decodes_at_the_size_shown() {
        let (img, side) = decode(&png(1200, 600), 100).unwrap();
        let size = img.size(0);
        assert_eq!((size.width.0, size.height.0, side), (100, 50, 100));
        // Small ones stay as they are.
        let (img, side) = decode(&png(400, 200), 100).unwrap();
        assert_eq!((img.size(0).width.0, side), (400, u32::MAX));
        // BGRA, as GPUI wants it.
        assert_eq!(&img.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
        // Smaller than asked for: its own size, never decoded again.
        let (img, side) = decode(&png(400, 200), 1000).unwrap();
        assert_eq!((img.size(0).width.0, side), (400, u32::MAX));
    }
}
