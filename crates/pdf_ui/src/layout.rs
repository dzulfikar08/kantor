//! Where pages go: one column, top to bottom, centred, in logical pixels.
//! Also the page-image cache, bounded in bytes.

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::RenderImage;
use pdf_engine::Turn;

/// Space around the column of pages and between pages, px.
pub(crate) const MARGIN: f32 = 16.0;
pub(crate) const GAP: f32 = 12.0;
/// Screen pixels per point at 100 % (96 dpi): Actual Size is true size.
pub(crate) const PX_PER_PT: f32 = 96.0 / 72.0;
pub(crate) const ZOOM_MIN: f32 = 0.1;
pub(crate) const ZOOM_MAX: f32 = 8.0;
/// The zoom steps of Ctrl+plus / Ctrl+minus and the zoom menu (as in
/// Acrobat).
pub(crate) const STEPS: &[f32] = &[
    0.1, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0,
];

/// How the zoom is chosen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Zoom {
    FitWidth,
    FitPage,
    /// 1.0 = Actual Size.
    Percent(f32),
}

/// The next step up or down from `z`.
pub(crate) fn step(z: f32, up: bool) -> f32 {
    if up {
        STEPS
            .iter()
            .copied()
            .find(|s| *s > z + 1e-3)
            .unwrap_or(ZOOM_MAX)
    } else {
        STEPS
            .iter()
            .rev()
            .copied()
            .find(|s| *s < z - 1e-3)
            .unwrap_or(ZOOM_MIN)
    }
}

/// The column of pages at one zoom.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Column {
    /// Each page's top, px from the top of the column.
    pub tops: Vec<f32>,
    /// Each page's size on screen (turned), px.
    pub sizes: Vec<(f32, f32)>,
    /// The widest page plus margins, and the full height, px.
    pub width: f32,
    pub height: f32,
}

impl Column {
    /// `pages` (points, unturned) at `zoom`, turned by `turn`.
    pub fn new(pages: &[(f32, f32)], zoom: f32, turn: Turn) -> Self {
        let k = zoom * PX_PER_PT;
        let mut tops = Vec::with_capacity(pages.len());
        let mut sizes = Vec::with_capacity(pages.len());
        let mut y = MARGIN;
        let mut widest: f32 = 0.0;
        for &(w, h) in pages {
            let (w, h) = turn.size(w * k, h * k);
            tops.push(y);
            sizes.push((w, h));
            widest = widest.max(w);
            y += h + GAP;
        }
        Self {
            tops,
            sizes,
            width: widest + 2.0 * MARGIN,
            height: y - GAP + MARGIN,
        }
    }

    pub fn len(&self) -> usize {
        self.tops.len()
    }

    /// The page at `y` (the one above when `y` is in a gap).
    pub fn page_at(&self, y: f32) -> usize {
        self.tops
            .partition_point(|&t| t <= y)
            .saturating_sub(1)
            .min(self.len().saturating_sub(1))
    }

    /// Pages that show between `y0` and `y1`.
    pub fn visible(&self, y0: f32, y1: f32) -> Range<usize> {
        if self.tops.is_empty() {
            return 0..0;
        }
        let first = self.page_at(y0);
        let first = if self.tops[first] + self.sizes[first].1 < y0 {
            (first + 1).min(self.len() - 1)
        } else {
            first
        };
        let last = self.page_at(y1);
        first..last + 1
    }

    /// A page's left edge in a view `view_w` wide: centred, or at the
    /// margin when the column is wider than the view.
    pub fn left(&self, page: usize, view_w: f32) -> f32 {
        let w = self.sizes.get(page).map_or(0.0, |s| s.0);
        let content = self.width.max(view_w);
        (content - w) / 2.0
    }
}

/// The zoom that fits the widest page's width in `view_w`.
pub(crate) fn fit_width(pages: &[(f32, f32)], turn: Turn, view_w: f32) -> f32 {
    let widest = pages
        .iter()
        .map(|&(w, h)| turn.size(w, h).0)
        .fold(1.0_f32, f32::max);
    ((view_w - 2.0 * MARGIN) / (widest * PX_PER_PT)).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// The zoom that fits page `page` whole in the view.
pub(crate) fn fit_page(pages: &[(f32, f32)], page: usize, turn: Turn, view: (f32, f32)) -> f32 {
    let (w, h) = pages.get(page).copied().unwrap_or((612.0, 792.0));
    let (w, h) = turn.size(w, h);
    let kw = (view.0 - 2.0 * MARGIN) / (w * PX_PER_PT);
    let kh = (view.1 - 2.0 * MARGIN) / (h * PX_PER_PT);
    kw.min(kh).clamp(ZOOM_MIN, ZOOM_MAX)
}

/// A drawn page or thumbnail.
struct Entry {
    page: usize,
    turn: Turn,
    scale: f32,
    image: Arc<RenderImage>,
    bytes: usize,
    used: u64,
}

/// Page images, bounded in bytes: the least recently shown go first (but
/// never one on screen). Evicted images wait in [`Self::take_evicted`] for
/// the window to free their texture.
pub(crate) struct ImageCache {
    entries: Vec<Entry>,
    total: usize,
    budget: usize,
    tick: u64,
    evicted: Vec<Arc<RenderImage>>,
}

impl ImageCache {
    pub fn new(budget: usize) -> Self {
        Self {
            entries: Vec::new(),
            total: 0,
            budget,
            tick: 0,
            evicted: Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.total
    }

    /// The best image of `page`: at `scale` if there is one (`true`), else
    /// the closest scale, to show stretched until the right one comes.
    pub fn get(&mut self, page: usize, turn: Turn, scale: f32) -> Option<(Arc<RenderImage>, bool)> {
        self.tick += 1;
        let tick = self.tick;
        let e = self
            .entries
            .iter_mut()
            .filter(|e| e.page == page && e.turn == turn)
            .min_by(|a, b| {
                let da = (a.scale / scale).ln().abs();
                let db = (b.scale / scale).ln().abs();
                da.total_cmp(&db)
            })?;
        e.used = tick;
        let exact = (e.scale / scale - 1.0).abs() < 0.02;
        Some((e.image.clone(), exact))
    }

    /// Whether `page` has any image (thumbnails: any scale will do).
    pub fn has(&self, page: usize, turn: Turn) -> bool {
        self.entries
            .iter()
            .any(|e| e.page == page && e.turn == turn)
    }

    /// Add an image; older images of the same page go, then the least
    /// recently used ones outside `keep` until the cache fits its budget.
    pub fn insert(
        &mut self,
        page: usize,
        turn: Turn,
        scale: f32,
        image: Arc<RenderImage>,
        bytes: usize,
        keep: Range<usize>,
    ) {
        self.tick += 1;
        let mut i = 0;
        while i < self.entries.len() {
            let e = &self.entries[i];
            if e.page == page {
                let e = self.entries.swap_remove(i);
                self.total -= e.bytes;
                self.evicted.push(e.image);
            } else {
                i += 1;
            }
        }
        self.entries.push(Entry {
            page,
            turn,
            scale,
            image,
            bytes,
            used: self.tick,
        });
        self.total += bytes;
        while self.total > self.budget {
            let victim = self
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| !keep.contains(&e.page))
                .min_by_key(|(_, e)| e.used)
                .map(|(i, _)| i);
            let Some(i) = victim else {
                break;
            };
            let e = self.entries.swap_remove(i);
            self.total -= e.bytes;
            self.evicted.push(e.image);
        }
    }

    #[cfg(test)]
    pub fn clear(&mut self) {
        self.evicted.extend(self.entries.drain(..).map(|e| e.image));
        self.total = 0;
    }

    pub fn take_evicted(&mut self) -> Vec<Arc<RenderImage>> {
        std::mem::take(&mut self.evicted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img() -> Arc<RenderImage> {
        let buf = image::RgbaImage::new(1, 1);
        Arc::new(RenderImage::new(vec![image::Frame::new(buf)]))
    }

    #[std::prelude::v1::test]
    fn column_places_pages() {
        let pages = [(612.0, 792.0), (792.0, 612.0), (612.0, 792.0)];
        let c = Column::new(&pages, 1.0, Turn(0));
        assert_eq!(c.tops[0], MARGIN);
        assert_eq!(c.sizes[0], (816.0, 1056.0));
        assert_eq!(c.tops[1], MARGIN + 1056.0 + GAP);
        assert_eq!(c.width, 1056.0 + 2.0 * MARGIN);
        assert_eq!(c.page_at(0.0), 0);
        assert_eq!(c.page_at(c.tops[1] + 1.0), 1);
        assert_eq!(c.page_at(1e9), 2);
        assert_eq!(c.visible(0.0, 100.0), 0..1);
        assert_eq!(c.visible(1000.0, 1200.0), 0..2);
        // Turned: widths and heights swap.
        let t = Column::new(&pages, 1.0, Turn(1));
        assert_eq!(t.sizes[0], (1056.0, 816.0));
        // Centred in a wide view.
        assert_eq!(c.left(0, 2000.0), (2000.0 - 816.0) / 2.0);
    }

    #[std::prelude::v1::test]
    fn fits_and_steps() {
        let pages = [(612.0, 792.0)];
        let z = fit_width(&pages, Turn(0), 816.0 + 2.0 * MARGIN);
        assert!((z - 1.0).abs() < 1e-4);
        let z = fit_page(&pages, 0, Turn(0), (2000.0, 1056.0 + 2.0 * MARGIN));
        assert!((z - 1.0).abs() < 1e-4);
        assert_eq!(step(1.0, true), 1.25);
        assert_eq!(step(1.0, false), 0.75);
        assert_eq!(step(0.8, false), 0.75);
        assert_eq!(step(8.0, true), ZOOM_MAX);
    }

    #[std::prelude::v1::test]
    fn cache_stays_in_budget_but_keeps_visible_pages() {
        let mut c = ImageCache::new(250);
        for p in 0..5 {
            c.insert(p, Turn(0), 1.0, img(), 100, p..p + 1);
        }
        assert!(c.bytes() <= 250);
        assert!(c.has(4, Turn(0)) && c.has(3, Turn(0)));
        assert!(!c.has(0, Turn(0)));
        assert_eq!(c.take_evicted().len(), 3);
        // A page on screen is kept even over budget.
        c.insert(9, Turn(0), 1.0, img(), 300, 3..10);
        assert!(c.has(9, Turn(0)) && c.has(3, Turn(0)));
        // A new scale replaces the old image of that page; the nearest
        // one is offered until then.
        let (_, exact) = c.get(9, Turn(0), 2.0).unwrap();
        assert!(!exact);
        c.insert(9, Turn(0), 2.0, img(), 10, 9..10);
        assert!(c.get(9, Turn(0), 2.0).unwrap().1);
        c.clear();
        assert_eq!(c.bytes(), 0);
    }
}
