//! The body's pictures for the screen, decoded on a background thread at
//! about the size they're shown (a photo is often several times larger
//! than its box): the page draws nothing in their place until they're
//! ready, a few pages ahead are decoded before they scroll into view, and
//! those not shown for a while are let go (a long document can have
//! hundreds).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;

use crate::DocsView;

/// Decoded pictures kept at most (more are let go, least recently shown
/// first, once not shown for [`KEEP`]).
const MAX: usize = 48;
const KEEP: Duration = Duration::from_secs(8);

#[derive(Default)]
pub(crate) struct Pictures {
    ready: HashMap<String, (Arc<RenderImage>, Instant)>,
    loading: HashSet<String>,
}

/// `bytes` (PNG or JPEG) decoded with no side longer than `side` px (as
/// GPUI wants it: BGRA).
fn decode(bytes: &[u8], side: u32) -> Option<Arc<RenderImage>> {
    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width(), img.height());
    let rgba = if w.max(h) > side && side > 0 {
        let scale = side as f32 / w.max(h) as f32;
        let (tw, th) = (
            ((w as f32 * scale).round() as u32).max(1),
            ((h as f32 * scale).round() as u32).max(1),
        );
        image::imageops::thumbnail(&img.to_rgba8(), tw, th)
    } else {
        img.to_rgba8()
    };
    let mut rgba = rgba;
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(rgba)])))
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
        if let Some((img, used)) = self.pictures.ready.get_mut(rel_id) {
            *used = Instant::now();
            return Some(img.clone());
        }
        if self.pictures.loading.contains(rel_id) {
            return None;
        }
        let bytes = self.doc.image_bytes(rel_id)?;
        if !matches!(
            docs_engine::ImageFormat::sniff(&bytes),
            docs_engine::ImageFormat::Jpeg | docs_engine::ImageFormat::Png
        ) {
            return None;
        }
        // Sharp at 200 % (zoom or a HiDPI screen).
        let side = (box_px * 2.0).ceil().max(64.0) as u32;
        let key = rel_id.to_owned();
        self.pictures.loading.insert(key.clone());
        let task = cx.background_spawn(async move { decode(&bytes, side) });
        cx.spawn(async move |this, cx| {
            let img = task.await;
            let _ = this.update(cx, |this, cx| {
                this.pictures.loading.remove(&key);
                if let Some(img) = img {
                    this.pictures.ready.insert(key, (img, Instant::now()));
                    cx.notify();
                }
            });
        })
        .detach();
        None
    }

    /// Whether picture `rel_id` is decoded by [`Self::screen_picture_of`]
    /// (or being decoded): the old way isn't needed for it.
    pub(crate) fn picture_decoding(&self, rel_id: &str) -> bool {
        self.pictures.ready.contains_key(rel_id) || self.pictures.loading.contains(rel_id)
    }

    /// Let go of decoded pictures not shown for a while (beyond the most
    /// recent [`MAX`]).
    pub(crate) fn trim_pictures(&mut self, window: &mut Window) {
        let p = &mut self.pictures;
        if p.ready.len() <= MAX {
            return;
        }
        let mut by_age: Vec<(Instant, String)> =
            p.ready.iter().map(|(k, (_, t))| (*t, k.clone())).collect();
        by_age.sort();
        let extra = p.ready.len() - MAX;
        for (t, k) in by_age.into_iter().take(extra) {
            if t.elapsed() < KEEP {
                break;
            }
            if let Some((img, _)) = p.ready.remove(&k) {
                window.drop_image(img).ok();
            }
        }
    }
}
