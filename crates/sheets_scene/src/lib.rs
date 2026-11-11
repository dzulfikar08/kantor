//! Kantor Sheets drawings as renderer-neutral display lists (no GPUI).
//!
//! A worksheet's charts and pictures ([`sheets_engine::drawing`]) become
//! [`slides_scene::Item`]s, the display list Slides paints on screen and
//! `slides_pdf` writes with krilla, so a printed sheet can draw them the
//! same way:
//!
//! - **Charts** are parsed and laid out by `slides_scene::chart` (the
//!   Slides renderer), with the workbook theme's colours and fonts. Each
//!   series' cached values are replaced by the cells it refers to now
//!   ([`live_chart`]), so a chart follows edits, recalculation and inserted
//!   rows; number formats come from the cells (through `kantor_numfmt`,
//!   like the grid's).
//! - **Pictures** are one [`slides_scene::Picture`] with their cropping.
//! - Anything else (shapes, text boxes, chart types Kantor can't draw) is
//!   a [`Prim::Unsupported`] box, which the screen shows as a placeholder.
//!
//! Items are in the object's own px (`0..w`, `0..h`); [`translated`]
//! places them. [`DrawingScenes`] caches built charts per object, size and
//! data: scrolling never rebuilds one, an edit rebuilds only the charts
//! whose cells changed.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use docs_layout::Engine;
use sheets_engine::Workbook;
use sheets_engine::drawing::{Drawing, DrawingKey, DrawingKind, RefData, RefValue};
use slides_engine::theme::{ClrMap, ColorCtx, Theme};
use slides_scene::chart::{self, Chart, Defaults};
use slides_scene::{Affine, Item, Picture, Prim};

/// The chart look of a workbook: its theme's colours and fonts.
pub struct ChartTheme {
    theme: Theme,
    map: ClrMap,
    defaults: Defaults,
}

impl ChartTheme {
    /// From `xl/theme/theme1.xml` (Office defaults without one).
    pub fn new(theme_xml: Option<&str>) -> Self {
        let theme = theme_xml.map(Theme::parse).unwrap_or_default();
        let map = ClrMap::default();
        let defaults = if theme_xml.is_some() {
            chart::defaults(&ColorCtx {
                theme: &theme,
                map: &map,
                ph: None,
            })
        } else {
            Defaults::default()
        };
        Self {
            theme,
            map,
            defaults,
        }
    }

    pub fn of(book: &Workbook) -> Self {
        Self::new(book.theme_xml().as_deref())
    }

    fn cx(&self) -> ColorCtx<'_> {
        ColorCtx {
            theme: &self.theme,
            map: &self.map,
            ph: None,
        }
    }

    /// A chart part, parsed with these colours.
    pub fn parse(&self, xml: &str) -> Option<Chart> {
        Chart::parse(xml, &self.cx())
    }
}

fn numbers(r: &RefData) -> Vec<Option<f64>> {
    r.values
        .iter()
        .map(|v| match v {
            RefValue::Number(n) => Some(*n),
            _ => None,
        })
        .collect()
}

fn hash_ref(r: &Option<RefData>, h: &mut DefaultHasher) {
    match r {
        None => 0u8.hash(h),
        Some(r) => {
            1u8.hash(h);
            for (v, s) in r.values.iter().zip(&r.shown) {
                match v {
                    RefValue::Number(n) => n.to_bits().hash(h),
                    RefValue::Text(t) => t.hash(h),
                    RefValue::Empty => 2u8.hash(h),
                }
                s.hash(h);
            }
            r.format.hash(h);
        }
    }
}

/// `base` with every series reading the cells it refers to now, and a
/// hash of what was read (equal hashes: the same chart). References that
/// can't be read (other workbooks, names, deleted ranges) keep the values
/// cached in the part.
pub fn live_chart(base: &Chart, book: &Workbook) -> (Chart, u64) {
    let mut chart = base.clone();
    let mut h = DefaultHasher::new();
    let read = |f: &Option<String>, h: &mut DefaultHasher| {
        let r = f.as_deref().and_then(|f| book.chart_reference(f));
        hash_ref(&r, h);
        r
    };
    for g in &mut chart.groups {
        for s in &mut g.series {
            if let Some(r) = read(&s.refs.val, &mut h) {
                s.vals = numbers(&r);
                if r.format.is_some() {
                    s.fmt = r.format.clone();
                }
            }
            if let Some(r) = read(&s.refs.cat, &mut h) {
                let any_num = r.values.iter().any(|v| matches!(v, RefValue::Number(_)));
                let all_num = any_num && r.values.iter().all(|v| !matches!(v, RefValue::Text(_)));
                // Several levels of labels read as one long list: keep the
                // cached innermost level.
                let fits = r.values.len() == s.vals.len() || s.cats.len() + s.cat_nums.len() == 0;
                if s.xs.is_some() {
                    s.xs = Some(if all_num {
                        numbers(&r)
                    } else {
                        (1..=s.vals.len()).map(|i| Some(i as f64)).collect()
                    });
                }
                if fits {
                    if all_num && (s.xs.is_some() || !s.cat_nums.is_empty()) {
                        s.cat_nums = numbers(&r);
                        s.cats.clear();
                        if r.format.is_some() {
                            s.cat_fmt = r.format.clone();
                        }
                    } else {
                        s.cats = r.shown.clone();
                        s.cat_nums.clear();
                        s.cat_fmt = None;
                    }
                }
            }
            if let Some(r) = read(&s.refs.name, &mut h) {
                let name: Vec<&str> = r
                    .shown
                    .iter()
                    .map(String::as_str)
                    .filter(|t| !t.is_empty())
                    .collect();
                s.name = Some(name.join(" "));
            }
        }
    }
    (chart, h.finish())
}

/// `items` moved by `(x, y)`.
pub fn translated(items: &[Item], x: f32, y: f32) -> Vec<Item> {
    let t = Affine::translate(x, y);
    items
        .iter()
        .map(|i| Item {
            transform: i.transform.then(t),
            ..i.clone()
        })
        .collect()
}

fn placeholder(w: f32, h: f32) -> Vec<Item> {
    vec![Item {
        layer: None,
        transform: Affine::translate(0.0, 0.0),
        prim: Prim::Unsupported { w, h },
    }]
}

/// The items of a picture drawn into `w` × `h`.
pub fn picture_items(
    part: &Arc<str>,
    data: &Arc<Vec<u8>>,
    crop: [f32; 4],
    w: f32,
    h: f32,
) -> Vec<Item> {
    vec![Item {
        layer: None,
        transform: Affine::translate(0.0, 0.0),
        prim: Prim::Picture(Picture {
            part: part.clone(),
            data: data.clone(),
            rect: [0.0, 0.0, w, h],
            crop,
            alpha: 1.0,
            clip: None,
        }),
    }]
}

/// Size key: quarter pixels.
fn q(v: f32) -> u32 {
    (v.max(0.0) * 4.0).round() as u32
}

type BuiltKey = (DrawingKey, u32, u32);

/// A chart's live data for one workbook state.
struct Live {
    revision: (u64, u64),
    hash: u64,
    chart: Arc<Chart>,
}

/// Built drawings of one workbook, cached (see the crate docs).
#[derive(Default)]
pub struct DrawingScenes {
    theme: Option<Arc<ChartTheme>>,
    /// Parsed chart parts, by part name.
    parsed: HashMap<Arc<str>, Option<Arc<Chart>>>,
    live: HashMap<DrawingKey, Live>,
    built: HashMap<BuiltKey, (u64, Arc<Vec<Item>>)>,
    /// Charts built since [`DrawingScenes::take_builds`] (for tests and the
    /// performance log).
    builds: usize,
}

/// Most built sizes kept (a resize drag makes many).
const MAX_BUILT: usize = 256;

impl DrawingScenes {
    /// Forget everything (another workbook, or a changed theme).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Charts built since the last call.
    pub fn take_builds(&mut self) -> usize {
        std::mem::take(&mut self.builds)
    }

    fn theme(&mut self, book: &Workbook) -> Arc<ChartTheme> {
        self.theme
            .get_or_insert_with(|| Arc::new(ChartTheme::of(book)))
            .clone()
    }

    /// The chart of `d` with its live data (`None`: not a chart Kantor
    /// can draw), and the data's hash.
    fn live(&mut self, book: &Workbook, d: &Drawing) -> Option<(Arc<Chart>, u64)> {
        let DrawingKind::Chart { part, xml } = &*d.kind else {
            return None;
        };
        let revision = book.revision();
        if let Some(l) = self.live.get(&d.key)
            && l.revision == revision
        {
            return Some((l.chart.clone(), l.hash));
        }
        let theme = self.theme(book);
        let base = self
            .parsed
            .entry(part.clone())
            .or_insert_with(|| theme.parse(xml).filter(Chart::drawable).map(Arc::new))
            .clone()?;
        let (chart, hash) = live_chart(&base, book);
        // Same data as before: keep the old chart so its builds stay valid.
        let chart = match self.live.get(&d.key) {
            Some(l) if l.hash == hash => l.chart.clone(),
            _ => Arc::new(chart),
        };
        self.live.insert(
            d.key,
            Live {
                revision,
                hash,
                chart: chart.clone(),
            },
        );
        Some((chart, hash))
    }

    /// The items of drawing `d` drawn into `w` × `h` px (its own
    /// coordinates), cached.
    pub fn items(
        &mut self,
        engine: &mut Engine,
        book: &Workbook,
        d: &Drawing,
        w: f32,
        h: f32,
    ) -> Arc<Vec<Item>> {
        match &*d.kind {
            DrawingKind::Picture { part, data, crop } => {
                return Arc::new(picture_items(part, data, *crop, w, h));
            }
            DrawingKind::Other(_) => return Arc::new(placeholder(w, h)),
            DrawingKind::Chart { .. } => {}
        }
        let Some((chart, hash)) = self.live(book, d) else {
            return Arc::new(placeholder(w, h));
        };
        let key = (d.key, q(w), q(h));
        if let Some((h0, items)) = self.built.get(&key)
            && *h0 == hash
        {
            return items.clone();
        }
        let theme = self.theme(book);
        let items = Arc::new(
            chart::items(engine, &chart, &theme.defaults, w, h)
                .unwrap_or_else(|| placeholder(w, h)),
        );
        self.builds += 1;
        if self.built.len() >= MAX_BUILT {
            self.built.clear();
        }
        self.built.insert(key, (hash, items.clone()));
        items
    }
}

#[cfg(test)]
mod tests;
