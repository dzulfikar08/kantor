//! Metafile pictures (WMF, EMF, PICT, BMP) drawn on screen without
//! stalling the UI: they are rendered to PNG on a background thread
//! (the first one with text also loads the system fonts there), and the
//! windows redraw when a picture is ready. Until then callers draw a
//! placeholder.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

use gpui_kit::App;

/// A metafile picture as PNG, if it's ready.
#[derive(Clone, Debug, PartialEq)]
pub enum MetafilePng {
    /// Rendered: the PNG, or `None` when the picture can't be drawn.
    Ready(Option<Arc<Vec<u8>>>),
    /// Rendering in the background: draw a placeholder for now.
    Loading,
}

/// Which picture, at which size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Key {
    hash: u64,
    len: usize,
    max_side: u32,
}

impl Key {
    fn of(bytes: &[u8], max_side: u32) -> Self {
        let mut h = DefaultHasher::new();
        bytes.hash(&mut h);
        Self {
            hash: h.finish(),
            len: bytes.len(),
            max_side,
        }
    }
}

/// Renders started and finished, by picture. A finished one is handed to
/// the first caller that asks for it (who keeps its decoded image), so
/// the map stays small; one that can't be drawn is remembered.
#[derive(Default)]
struct Renders(HashMap<Key, Option<Option<Arc<Vec<u8>>>>>);

/// Finished pictures nobody came back for are dropped past this many.
const MAX_FINISHED: usize = 32;

impl Renders {
    /// `Some(state)` for a picture already started; `None` when the
    /// caller should start it (it's marked started).
    fn poll(&mut self, key: Key) -> Option<MetafilePng> {
        match self.0.get(&key) {
            Some(None) => Some(MetafilePng::Loading),
            // Pictures that can't be drawn stay known (they're small).
            Some(Some(None)) => Some(MetafilePng::Ready(None)),
            Some(Some(_)) => {
                let png = self.0.remove(&key).flatten().flatten();
                Some(MetafilePng::Ready(png))
            }
            None => {
                self.0.insert(key, None);
                None
            }
        }
    }

    fn finish(&mut self, key: Key, png: Option<Arc<Vec<u8>>>) {
        let finished = self
            .0
            .values()
            .filter(|v| matches!(v, Some(Some(_))))
            .count();
        if finished >= MAX_FINISHED {
            self.0.retain(|_, v| !matches!(v, Some(Some(_))));
        }
        self.0.insert(key, Some(png));
    }
}

fn renders() -> &'static Mutex<Renders> {
    static RENDERS: OnceLock<Mutex<Renders>> = OnceLock::new();
    RENDERS.get_or_init(Mutex::default)
}

/// The picture in `bytes` (anything `kantor_metafile` reads) as PNG, its
/// longer side at most `max_side`: [`MetafilePng::Loading`] the first
/// time (rendering starts in the background and every window redraws
/// when it's done), then [`MetafilePng::Ready`] once (keep the result;
/// `Ready(None)` for a picture that can't be drawn comes back every time).
pub fn metafile_png(bytes: &[u8], max_side: u32, cx: &App) -> MetafilePng {
    if kantor_metafile::kind(bytes).is_none() {
        return MetafilePng::Ready(None);
    }
    let key = Key::of(bytes, max_side);
    if let Some(state) = renders()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .poll(key)
    {
        return state;
    }
    let owned = bytes.to_vec();
    let rendering = cx
        .background_executor()
        .spawn(async move { kantor_metafile::to_png(&owned, max_side).map(Arc::new) });
    cx.spawn(async move |cx| {
        let png = rendering.await;
        renders()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finish(key, png);
        cx.update(|cx| cx.refresh_windows());
    })
    .detach();
    MetafilePng::Loading
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn a_render_is_started_once_and_handed_out_once() {
        let mut r = Renders::default();
        let key = Key::of(b"emf", 2048);
        assert_eq!(r.poll(key), None, "the first caller starts it");
        assert_eq!(r.poll(key), Some(MetafilePng::Loading));
        let png = Arc::new(vec![1, 2, 3]);
        r.finish(key, Some(png.clone()));
        assert_eq!(r.poll(key), Some(MetafilePng::Ready(Some(png))));
        assert_eq!(r.poll(key), None, "handed out: a later caller starts again");
        r.finish(key, None);
        assert_eq!(r.poll(key), Some(MetafilePng::Ready(None)));
        assert_eq!(
            r.poll(key),
            Some(MetafilePng::Ready(None)),
            "not started again"
        );
    }

    #[std::prelude::v1::test]
    fn unclaimed_pictures_are_dropped() {
        let mut r = Renders::default();
        let pending = Key::of(b"pending", 1);
        assert_eq!(r.poll(pending), None);
        for i in 0..(MAX_FINISHED as u32 + 5) {
            r.finish(Key::of(b"x", i), Some(Arc::new(vec![0])));
        }
        assert!(r.0.len() <= MAX_FINISHED + 1);
        assert_eq!(r.poll(pending), Some(MetafilePng::Loading), "kept");
    }

    #[std::prelude::v1::test]
    fn different_sizes_are_different_pictures() {
        assert_ne!(Key::of(b"a", 1), Key::of(b"a", 2));
        assert_ne!(Key::of(b"a", 1), Key::of(b"b", 1));
    }
}
