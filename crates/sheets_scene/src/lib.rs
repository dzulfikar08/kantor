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
//! - **Shapes**, text boxes, connectors and groups are read and drawn by
//!   Slides' shape code ([`shape_items`]): preset and custom geometry,
//!   solid and gradient fills, outlines with dashes and arrowheads, and
//!   their text (Excel's 11 pt default, the theme's fonts and colours).
//! - Anything else (chart types Kantor can't draw, OLE objects) is a
//!   [`Prim::Unsupported`] box, which the screen shows as a placeholder.
//! - **IMAGE formulas** put a picture in their cell ([`cell_image_items`])
//!   with Excel's four sizing modes.
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
use sheets_engine::drawing::{Drawing, DrawingKey, DrawingKind, RefData, RefValue, ShapeMedia};
use sheets_engine::web::CellImage;
use slides_engine::theme::{ClrMap, ColorCtx, ListStyle, Theme};
use slides_engine::{EMU_PER_PX, Fill, Rgb, Xfrm};
use slides_scene::chart::model::Line;
use slides_scene::chart::{self, Chart, Defaults};
use slides_scene::{Affine, Builder, Item, Picture, Prim};
pub mod slicer;

/// The chart look of a workbook: its theme's colours and fonts.
pub struct ChartTheme {
    theme: Theme,
    map: ClrMap,
    defaults: Defaults,
    /// For shapes: the workbook's theme, or Office's when it has none.
    shape_theme: Theme,
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
            // Shapes refer to theme colours and styles: Office's when the
            // workbook has no theme.
            shape_theme: match theme_xml {
                Some(x) => Theme::parse(x),
                None => Theme::parse(&slides_engine::sheet_shape::office_theme()),
            },
            theme,
            map,
            defaults,
        }
    }

    pub fn of(book: &Workbook) -> Self {
        Self::new(book.theme_xml().as_deref())
    }

    /// What the theme gives charts (for [`chart_items`]).
    pub fn defaults(&self) -> &Defaults {
        &self.defaults
    }

    fn cx(&self) -> ColorCtx<'_> {
        ColorCtx {
            theme: &self.theme,
            map: &self.map,
            ph: None,
        }
    }

    /// A chart part, parsed with these colours, with Excel's defaults for
    /// what it leaves out ([`excel_defaults`]).
    pub fn parse(&self, xml: &str) -> Option<Chart> {
        let mut chart = Chart::parse(xml, &self.cx())?;
        excel_defaults(&mut chart, &self.defaults);
        Some(chart)
    }
}

/// What Excel uses where a chart part says nothing (the renderer's own
/// defaults are PowerPoint's): 10 pt text with a 14 pt title, and a chart
/// area filled with the background colour inside a light grey border.
pub fn excel_defaults(chart: &mut Chart, d: &Defaults) {
    chart.font.size_pt.get_or_insert(10.0);
    if let Some(t) = chart.title.as_mut() {
        t.font.size_pt.get_or_insert(14.0);
    }
    if chart.space_look.fill.is_none() {
        chart.space_look.fill = Some(Fill::Solid(d.background, 255));
    }
    if chart.space_look.line.is_none() {
        chart.space_look.line = Some(Line {
            fill: Some(Fill::Solid(Rgb([0xD9, 0xD9, 0xD9]), 255)),
            width: Some(1.0),
            style: Default::default(),
        });
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
/// `current`: the references are in the current rows and columns (a chart
/// made in Kantor), not the opened file's.
pub fn live_chart(base: &Chart, book: &Workbook, current: bool) -> (Chart, u64) {
    let mut chart = base.clone();
    let mut h = DefaultHasher::new();
    let read = |f: &Option<String>, h: &mut DefaultHasher| {
        let r = f.as_deref().and_then(|f| {
            if current {
                book.chart_reference_now(f)
            } else {
                book.chart_reference(f)
            }
        });
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
            if let Some(r) = read(&s.refs.size, &mut h) {
                s.sizes = Some(numbers(&r));
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

/// The charts inside a group, by their `r:id`, parsed, with the cells'
/// values now when `book` is given (`current`: their references are in
/// the current rows and columns, a group made in Kantor), else the values
/// cached in their parts; and a hash of what was read.
pub type GroupCharts = Vec<(String, Arc<Chart>)>;

/// The charts among a group's `media` (see [`GroupCharts`]).
pub fn group_charts(
    theme: &ChartTheme,
    media: &ShapeMedia,
    book: Option<&Workbook>,
    current: bool,
) -> (GroupCharts, u64) {
    let mut h = DefaultHasher::new();
    let mut out = Vec::new();
    for (id, ext, data) in media.iter() {
        if ext != sheets_engine::drawing::GROUP_CHART {
            continue;
        }
        let Some(base) = std::str::from_utf8(data)
            .ok()
            .and_then(|x| theme.parse(x))
            .filter(Chart::drawable)
        else {
            continue;
        };
        let chart = match book {
            Some(b) => {
                let (c, k) = live_chart(&base, b, current);
                k.hash(&mut h);
                c
            }
            None => base,
        };
        id.hash(&mut h);
        out.push((id.clone(), Arc::new(chart)));
    }
    (out, h.finish())
}

/// The items of a shape, text box, connector or group (its element's XML,
/// see `DrawingKind::Shape`) drawn into `w` × `h` px; a placeholder box
/// when it can't be read. Its own turn and flips apply inside the box.
/// Charts inside a group show the values cached in their parts.
pub fn shape_items(
    builder: &Builder,
    engine: &mut Engine,
    theme: &ChartTheme,
    xml: &str,
    media: &ShapeMedia,
    w: f32,
    h: f32,
) -> Vec<Item> {
    let charts = group_charts(theme, media, None, false).0;
    shape_items_on(builder, engine, theme, xml, media, &charts, w, h, None)
}

/// Relative luminance of a colour (0 black, 1 white).
fn luminance(Rgb([r, g, b]): Rgb) -> f32 {
    let ch = |c: u8| {
        let c = c as f32 / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
}

/// Whether a shape lets the grid show through behind its text (no fill).
fn unfilled(shape: &slides_engine::Shape) -> bool {
    shape.look.fill.is_none() && matches!(shape.look.fill_as_read, None | Some(Fill::None))
}

/// [`shape_items`] on a dark grid: `dark_text` is the theme's text
/// colour. As cells' Automatic text is, a shape's text in the theme's
/// default text colour (dark) is drawn in it when the shape has no fill
/// and the dark text would vanish into the grid; text with a colour of
/// its own keeps it.
#[allow(clippy::too_many_arguments)]
pub fn shape_items_on(
    builder: &Builder,
    engine: &mut Engine,
    theme: &ChartTheme,
    xml: &str,
    media: &ShapeMedia,
    charts: &[(String, Arc<Chart>)],
    w: f32,
    h: f32,
    dark_text: Option<Rgb>,
) -> Vec<Item> {
    let colors = ColorCtx {
        theme: &theme.shape_theme,
        map: &theme.map,
        ph: None,
    };
    // Part names a picture cache can key on: the id, the bytes' hash and
    // the extension (which says how to decode it).
    // Charts inside a group: named by their id.
    let names: Vec<(String, String)> = media
        .iter()
        .map(|(id, ext, data)| {
            if ext == sheets_engine::drawing::GROUP_CHART {
                return (id.clone(), format!("{CHART_PART}{id}"));
            }
            let mut h = DefaultHasher::new();
            data.hash(&mut h);
            (id.clone(), format!("{id}-{:x}.{ext}", h.finish()))
        })
        .collect();
    let ids: Vec<(&str, &str)> = names
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let Some(mut shape) =
        slides_engine::sheet_shape::parse_with_media(&format!("<r>{xml}</r>"), colors, &ids)
    else {
        return placeholder(w, h);
    };
    if let Some(fg) = dark_text
        && unfilled(&shape)
        && shape.look.text_color.is_none_or(|c| luminance(c) < 0.2)
    {
        shape.look.text_color = Some(fg);
    }
    let emu = |v: f32| (v * EMU_PER_PX).round() as i64;
    let mut xfrm = Xfrm::new(0, 0, emu(w), emu(h));
    if let Some(own) = shape.xfrm {
        xfrm.rot = own.rot;
        xfrm.flip_h = own.flip_h;
        xfrm.flip_v = own.flip_v;
    }
    // Excel's shape text: 11 pt in the theme's body font.
    let mut base = ListStyle::default();
    for l in &mut base.0 {
        l.size = Some(1100);
    }
    let minor = theme.shape_theme.minor_font.clone();
    let text = |s: &slides_engine::Shape, x: &Xfrm| {
        slides_engine::render::free_shape_text(s, &base, minor.as_deref(), x)
    };
    let mut out = Vec::new();
    let pictures = |part: &str| {
        let i = names.iter().position(|(_, n)| n == part)?;
        Some(Arc::new(media[i].2.to_vec()))
    };
    let defaults = &theme.defaults;
    let group_chart = |engine: &mut Engine, part: &str, w: f32, h: f32| {
        let id = part.strip_prefix(CHART_PART)?;
        let (_, chart) = charts.iter().find(|(i, _)| i == id)?;
        Some(chart_items(engine, chart, defaults, w, h))
    };
    builder.free_shape_with(
        engine,
        &shape,
        &xfrm,
        &text,
        &pictures,
        &group_chart,
        &mut out,
    );
    out
}

/// The part name a chart inside a group is known by while drawn.
const CHART_PART: &str = "kantor-group-chart:";

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

/// Where an IMAGE formula's picture goes in its `w` × `h` cell (px), as
/// `[x, y, w, h]` of the whole picture, centred: sizing 0 fits it keeping
/// its aspect ratio, 1 fills the cell, 2 keeps the picture's own size, 3 is
/// the formula's width and height in pixels (one of them alone keeps the
/// aspect ratio). Sizes 2 and 3 may overflow the cell.
pub fn cell_image_rect(image: &CellImage, w: f32, h: f32) -> [f32; 4] {
    let pw = image.pixel_width.max(1) as f32;
    let ph = image.pixel_height.max(1) as f32;
    let (iw, ih) = match image.sizing {
        1 => return [0.0, 0.0, w, h],
        2 => (pw, ph),
        3 => match (image.width, image.height) {
            (Some(iw), Some(ih)) => (iw as f32, ih as f32),
            (Some(iw), None) => (iw as f32, ph * iw as f32 / pw),
            (None, Some(ih)) => (pw * ih as f32 / ph, ih as f32),
            (None, None) => (pw, ph),
        },
        _ => {
            let k = (w / pw).min(h / ph);
            (pw * k, ph * k)
        }
    };
    [(w - iw) / 2.0, (h - ih) / 2.0, iw, ih]
}

/// The items of an IMAGE formula's picture in its `w` × `h` cell, cut to
/// the cell; none when nothing of it shows.
pub fn cell_image_items(image: &CellImage, w: f32, h: f32) -> Vec<Item> {
    let [x, y, iw, ih] = cell_image_rect(image, w, h);
    if iw <= 0.0 || ih <= 0.0 {
        return Vec::new();
    }
    let (x0, y0) = (x.max(0.0), y.max(0.0));
    let (x1, y1) = ((x + iw).min(w), (y + ih).min(h));
    if x1 <= x0 || y1 <= y0 {
        return Vec::new();
    }
    let crop = [
        (x0 - x) / iw,
        (y0 - y) / ih,
        (x + iw - x1) / iw,
        (y + ih - y1) / ih,
    ];
    vec![Item {
        layer: None,
        transform: Affine::translate(0.0, 0.0),
        prim: Prim::Picture(Picture {
            part: Arc::from(format!("image:{}", image.source)),
            data: image.data.clone(),
            rect: [x0, y0, x1 - x0, y1 - y0],
            crop,
            alpha: 1.0,
            clip: None,
        }),
    }]
}

/// The items of a chart (with its live data, see [`live_chart`]) drawn
/// into `w` × `h` px; a placeholder box when it can't be laid out.
pub fn chart_items(engine: &mut Engine, chart: &Chart, d: &Defaults, w: f32, h: f32) -> Vec<Item> {
    chart::items(engine, chart, d, w, h).unwrap_or_else(|| placeholder(w, h))
}

/// Size key: quarter pixels.
fn q(v: f32) -> u32 {
    (v.max(0.0) * 4.0).round() as u32
}

type BuiltKey = (DrawingKey, u32, u32);

/// A chart's live data for one workbook state.
struct Live {
    revision: (u64, u64),
    /// The chart part it was built from (a chart made in Kantor gets a new
    /// part name when it changes).
    part: Arc<str>,
    hash: u64,
    chart: Arc<Chart>,
}

/// Built drawings of one workbook, cached (see the crate docs).
#[derive(Default)]
pub struct DrawingScenes {
    theme: Option<Arc<ChartTheme>>,
    /// Shapes' text layouts.
    shapes: Builder,
    /// Parsed chart parts, by part name.
    parsed: HashMap<Arc<str>, Option<Arc<Chart>>>,
    live: HashMap<DrawingKey, Live>,
    built: HashMap<BuiltKey, (u64, Arc<Vec<Item>>)>,
    /// Charts built since [`DrawingScenes::take_builds`] (for tests and the
    /// performance log).
    builds: usize,
    /// The dark theme's text colour (see [`shape_items_on`]).
    dark_text: Option<Rgb>,
    /// Slicers whose Multi-Select is on (drawn pressed).
    multi: std::collections::HashSet<DrawingKey>,
    /// A timeline being dragged across and the dates it would select.
    timeline_preview: Option<(DrawingKey, (f64, f64))>,
}

/// Most built sizes kept (a resize drag makes many).
const MAX_BUILT: usize = 256;

impl DrawingScenes {
    /// Forget everything (another workbook, or a changed theme).
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Draw shapes for a dark grid whose text colour is `fg` (`None`: a
    /// light one, as printed). Shapes built for the other look are dropped.
    pub fn set_dark_text(&mut self, fg: Option<Rgb>) {
        if self.dark_text != fg {
            self.dark_text = fg;
            self.built.clear();
        }
    }

    /// Slicers whose Multi-Select button is on.
    pub fn set_multi(&mut self, keys: std::collections::HashSet<DrawingKey>) {
        self.multi = keys;
    }

    /// The timeline whose periods are being dragged across, and the
    /// dates that would select (drawn selected until the button is up).
    pub fn set_timeline_preview(&mut self, preview: Option<(DrawingKey, (f64, f64))>) {
        self.timeline_preview = preview;
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
        let DrawingKind::Chart {
            part,
            xml,
            current_refs,
        } = &*d.kind
        else {
            return None;
        };
        let revision = book.revision();
        if let Some(l) = self.live.get(&d.key)
            && l.revision == revision
            && l.part == *part
        {
            return Some((l.chart.clone(), l.hash));
        }
        let theme = self.theme(book);
        let base = self
            .parsed
            .entry(part.clone())
            .or_insert_with(|| theme.parse(xml).filter(Chart::drawable).map(Arc::new))
            .clone()?;
        let (chart, hash) = live_chart(&base, book, *current_refs);
        // A changed chart made in Kantor has a new part: build it again even
        // with the same data.
        let hash = {
            let mut h = DefaultHasher::new();
            hash.hash(&mut h);
            part.hash(&mut h);
            h.finish()
        };
        // Same data as before: keep the old chart so its builds stay valid.
        let chart = match self.live.get(&d.key) {
            Some(l) if l.hash == hash && l.part == *part => l.chart.clone(),
            _ => Arc::new(chart),
        };
        self.live.insert(
            d.key,
            Live {
                revision,
                part: part.clone(),
                hash,
                chart: chart.clone(),
            },
        );
        Some((chart, hash))
    }

    /// A shape's items, cached by its XML and size.
    fn shape_cached(
        &mut self,
        engine: &mut Engine,
        book: &Workbook,
        key: DrawingKey,
        xml: &str,
        w: f32,
        h: f32,
    ) -> Arc<Vec<Item>> {
        let hash = {
            let mut s = DefaultHasher::new();
            xml.hash(&mut s);
            s.finish()
        };
        let k = (key, q(w), q(h));
        if let Some((h0, items)) = self.built.get(&k)
            && *h0 == hash
        {
            return items.clone();
        }
        let theme = self.theme(book);
        let items = Arc::new(shape_items_on(
            &self.shapes,
            engine,
            &theme,
            xml,
            &ShapeMedia::default(),
            &[],
            w,
            h,
            self.dark_text,
        ));
        if self.built.len() >= MAX_BUILT {
            self.built.clear();
        }
        self.built.insert(k, (hash, items.clone()));
        items
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
            DrawingKind::Slicer { .. } | DrawingKind::Timeline { .. } => {
                let xml = match &*d.kind {
                    DrawingKind::Slicer { name } => book
                        .slicer_view(d.key.sheet_id, name)
                        .map(|v| slicer::slicer_xml(&v, w, h, self.multi.contains(&d.key))),
                    DrawingKind::Timeline { name } => book
                        .timeline_view(d.key.sheet_id, name)
                        .map(|v| match self.timeline_preview {
                            Some((k, range)) if k == d.key => v.with_selection(range),
                            _ => v,
                        })
                        .map(|v| slicer::timeline_xml(&v, w, h)),
                    _ => None,
                };
                let Some(xml) = xml else {
                    return Arc::new(placeholder(w, h));
                };
                return self.shape_cached(engine, book, d.key, &xml, w, h);
            }
            DrawingKind::Shape { xml, media } => {
                let theme = self.theme(book);
                let current = d.key.index >= sheets_engine::NEW_CHART_BASE;
                let (charts, read) = group_charts(&theme, media, Some(book), current);
                let hash = {
                    let mut h = DefaultHasher::new();
                    xml.hash(&mut h);
                    media.len().hash(&mut h);
                    for (id, _, data) in media.iter() {
                        id.hash(&mut h);
                        data.len().hash(&mut h);
                    }
                    read.hash(&mut h);
                    h.finish()
                };
                let key = (d.key, q(w), q(h));
                if let Some((h0, items)) = self.built.get(&key)
                    && *h0 == hash
                {
                    return items.clone();
                }
                let items = Arc::new(shape_items_on(
                    &self.shapes,
                    engine,
                    &theme,
                    xml,
                    media,
                    &charts,
                    w,
                    h,
                    self.dark_text,
                ));
                if self.built.len() >= MAX_BUILT {
                    self.built.clear();
                }
                self.built.insert(key, (hash, items.clone()));
                return items;
            }
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
        let items = Arc::new(chart_items(engine, &chart, &theme.defaults, w, h));
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
