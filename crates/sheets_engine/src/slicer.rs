//! Slicers and timelines (Excel 2010 pivot table slicers, Excel 2013 table
//! slicers and timelines): read from the file, shown as drawings over the
//! grid (see `sheets_scene`), and used as Excel uses them.
//!
//! * A **pivot table slicer** filters a field of the pivot tables its cache
//!   lists (`x14:slicerCacheDefinition` → `pivotTables`). Its selection *is*
//!   that field's hidden items in those pivot tables, as in Excel: clicking
//!   an item lays them out again with the items left out hidden (also for a
//!   field that is on no axis, which then filters the records). A save
//!   writes the cache's `items` back (`s` selected, `nd` no data).
//! * A **table slicer** (`x15:tableSlicerCache`) filters a column of its
//!   table: its selection is the table's AutoFilter on that column, which
//!   the table saves.
//! * A **timeline** (`x15:timelineCacheDefinition`) is drawn with its
//!   periods and the selection the file saved, read-only: Kantor can't
//!   change its date filter yet.
//!
//! Items with no data (filtered out by the other fields or columns) are
//! greyed and, by default, listed last, as Excel's "Visually indicate items
//! with no data" and "Show items with no data last" do.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Result, anyhow, bail};
use roxmltree::{Document, Node};

use crate::Workbook;
use crate::filter::{FilterKind, FilterScope};
use crate::format::Rgb;
use crate::pivot::{Cache, Def, FieldItem, Rebuild, Val};

// ---------------------------------------------------------------- model

/// What a slicer cache filters.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CacheSource {
    /// Pivot tables, by `(sheetId, name)`; the file's items (`x`, selected,
    /// no data).
    Pivot {
        pivots: Vec<(u32, String)>,
        items: Vec<(usize, bool, bool)>,
    },
    /// A column of a table (`column`: the table column's `id`).
    Table { table: String, column: u32 },
    /// An OLAP slicer or one Kantor can't read.
    Other,
}

/// Where items with no data go (`crossFilter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cross {
    /// Greyed and listed last (Excel's default).
    Last,
    /// Greyed, in their place.
    InPlace,
    /// Not marked.
    None,
}

#[derive(Debug, Clone)]
pub(crate) struct SlicerCache {
    pub part: String,
    pub xml: Arc<String>,
    pub name: String,
    pub source_name: String,
    pub source: CacheSource,
    pub descending: bool,
    pub cross: Cross,
    /// Excel 2013 "Hide items with no data".
    pub hide_no_data: bool,
}

/// A slicer cache's settings (Slicer Settings' sorting and items with no
/// data).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct CacheSettings {
    pub descending: bool,
    pub cross: Cross,
    pub hide_no_data: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SlicerDef {
    pub sheet_id: u32,
    pub name: String,
    pub cache: String,
    pub caption: String,
    pub columns: u32,
    /// Button height, EMU.
    pub row_height: i64,
    pub show_caption: bool,
    pub style: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct TimelineCache {
    pub name: String,
    pub source_name: String,
    pub pivots: Vec<(u32, String)>,
    /// The selected dates (serials), if filtered.
    pub selection: Option<(f64, f64)>,
    /// The dates the timeline spans.
    pub bounds: Option<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub(crate) struct TimelineDef {
    pub sheet_id: u32,
    pub name: String,
    pub cache: String,
    pub caption: String,
    /// 0 years, 1 quarters, 2 months, 3 days.
    pub level: u32,
    pub show_header: bool,
    pub show_selection_label: bool,
    pub show_time_level: bool,
    /// The first period shown.
    pub scroll: Option<f64>,
    pub style: Option<String>,
}

/// The slicers and timelines of the opened file.
#[derive(Debug, Clone, Default)]
pub(crate) struct Slicers {
    pub caches: Vec<SlicerCache>,
    pub slicers: Vec<SlicerDef>,
    pub timeline_caches: Vec<TimelineCache>,
    pub timelines: Vec<TimelineDef>,
}

fn local<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn locals<'a, 'i>(n: Node<'a, 'i>, name: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn deep<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.descendants()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn flag(n: Node, name: &str, default: bool) -> bool {
    match n.attribute(name) {
        Some("1" | "true") => true,
        Some("0" | "false") => false,
        _ => default,
    }
}

fn pivot_list(root: Node) -> Vec<(u32, String)> {
    local(root, "pivotTables")
        .map(|l| {
            locals(l, "pivotTable")
                .filter_map(|p| {
                    Some((
                        p.attribute("tabId")?.parse().ok()?,
                        p.attribute("name")?.to_owned(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn cross_of(s: Option<&str>) -> Cross {
    match s {
        Some("none") => Cross::None,
        Some("showItemsWithNoData") => Cross::InPlace,
        _ => Cross::Last,
    }
}

fn date_of(n: Option<Node>, attr: &str) -> Option<f64> {
    crate::pivot::serial_of_iso(n?.attribute(attr)?)
}

impl Slicers {
    pub(crate) fn read(bytes: &[u8]) -> Self {
        let src = match crate::preserve::slicer_sources(bytes) {
            Ok(s) => s,
            Err(e) => {
                log::warn!("slicers not read: {e}");
                return Self::default();
            }
        };
        let mut out = Slicers::default();
        for (part, xml) in &src.caches {
            let Ok(doc) = Document::parse(xml) else {
                continue;
            };
            let root = doc.root_element();
            let name = root.attribute("name").unwrap_or_default().to_owned();
            let source_name = root.attribute("sourceName").unwrap_or_default().to_owned();
            match root.tag_name().name() {
                "slicerCacheDefinition" => {
                    let tabular = local(root, "data").and_then(|d| local(d, "tabular"));
                    let table = deep(root, "tableSlicerCache");
                    let (source, sort, cross) = match (tabular, table) {
                        (Some(t), _) => {
                            let items = local(t, "items")
                                .map(|l| {
                                    locals(l, "i")
                                        .filter_map(|i| {
                                            Some((
                                                i.attribute("x")?.parse().ok()?,
                                                flag(i, "s", false),
                                                flag(i, "nd", false),
                                            ))
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            (
                                CacheSource::Pivot {
                                    pivots: pivot_list(root),
                                    items,
                                },
                                t.attribute("sortOrder"),
                                t.attribute("crossFilter"),
                            )
                        }
                        (None, Some(t)) => {
                            let id: Option<u32> =
                                t.attribute("tableId").and_then(|v| v.parse().ok());
                            let table = id.and_then(|id| {
                                src.tables.iter().find(|(i, _)| *i == id).map(|(_, n)| n)
                            });
                            let column = t.attribute("column").and_then(|v| v.parse().ok());
                            match (table, column) {
                                (Some(table), Some(column)) => (
                                    CacheSource::Table {
                                        table: table.clone(),
                                        column,
                                    },
                                    t.attribute("sortOrder"),
                                    t.attribute("crossFilter"),
                                ),
                                _ => (CacheSource::Other, None, None),
                            }
                        }
                        _ => (CacheSource::Other, None, None),
                    };
                    out.caches.push(SlicerCache {
                        part: part.clone(),
                        xml: Arc::new(xml.clone()),
                        name,
                        source_name,
                        source,
                        descending: sort == Some("descending"),
                        cross: cross_of(cross),
                        hide_no_data: deep(root, "slicerCacheHideItemsWithNoData").is_some(),
                    });
                }
                "timelineCacheDefinition" => {
                    let state = local(root, "state");
                    let selection = state.and_then(|s| local(s, "selection"));
                    let bounds = state.and_then(|s| local(s, "bounds"));
                    let pair =
                        |n: Option<Node>| Some((date_of(n, "startDate")?, date_of(n, "endDate")?));
                    out.timeline_caches.push(TimelineCache {
                        name,
                        source_name,
                        pivots: pivot_list(root),
                        selection: pair(selection),
                        bounds: pair(bounds),
                    });
                }
                _ => {}
            }
        }
        for (sheet_id, _, xml) in &src.sheets {
            let Ok(doc) = Document::parse(xml) else {
                continue;
            };
            let root = doc.root_element();
            for s in locals(root, "slicer") {
                let name = s.attribute("name").unwrap_or_default().to_owned();
                out.slicers.push(SlicerDef {
                    sheet_id: *sheet_id,
                    cache: s.attribute("cache").unwrap_or_default().to_owned(),
                    caption: s.attribute("caption").unwrap_or(&name).to_owned(),
                    name,
                    columns: s
                        .attribute("columnCount")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1u32)
                        .max(1),
                    row_height: s
                        .attribute("rowHeight")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(241_300),
                    show_caption: flag(s, "showCaption", true),
                    style: s.attribute("style").map(str::to_owned),
                });
            }
            for t in locals(root, "timeline") {
                let name = t.attribute("name").unwrap_or_default().to_owned();
                out.timelines.push(TimelineDef {
                    sheet_id: *sheet_id,
                    cache: t.attribute("cache").unwrap_or_default().to_owned(),
                    caption: t.attribute("caption").unwrap_or(&name).to_owned(),
                    name,
                    level: t
                        .attribute("level")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(2u32)
                        .min(3),
                    show_header: flag(t, "showHeader", true),
                    show_selection_label: flag(t, "showSelectionLabel", true),
                    show_time_level: flag(t, "showTimeLevel", true),
                    scroll: t
                        .attribute("scrollPosition")
                        .and_then(crate::pivot::serial_of_iso),
                    style: t.attribute("style").map(str::to_owned),
                });
            }
        }
        out
    }

    fn cache(&self, name: &str) -> Option<&SlicerCache> {
        self.caches
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }
}

// ---------------------------------------------------------------- views

/// A slicer's colours (its style, coloured by the workbook's theme).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlicerColors {
    pub background: Rgb,
    pub border: Rgb,
    pub header: Rgb,
    pub selected: Rgb,
    pub selected_text: Rgb,
    pub unselected: Rgb,
    pub unselected_text: Rgb,
    pub no_data: Rgb,
    pub no_data_text: Rgb,
    pub button_border: Rgb,
}

/// A slicer button.
#[derive(Debug, Clone, PartialEq)]
pub struct SlicerItem {
    pub label: String,
    pub selected: bool,
    /// Filtered out by the other fields or columns (greyed).
    pub no_data: bool,
}

/// A slicer as it shows now.
#[derive(Debug, Clone, PartialEq)]
pub struct SlicerView {
    pub name: String,
    pub caption: String,
    pub show_caption: bool,
    pub columns: u32,
    /// Button height in px.
    pub button_height: f32,
    /// In the order shown.
    pub items: Vec<SlicerItem>,
    /// Some item isn't selected (Clear Filter is on).
    pub filtered: bool,
    /// Why clicking it can't filter, if it can't.
    pub problem: Option<String>,
    pub colors: SlicerColors,
}

/// A timeline period.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelinePeriod {
    pub label: String,
    /// Its first day and the first day of the next period (serials).
    pub start: f64,
    pub end: f64,
    /// The year (or year and quarter) above it, on the first period of a
    /// new one.
    pub group: Option<String>,
    pub selected: bool,
}

/// A timeline as it shows now.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineView {
    pub name: String,
    pub caption: String,
    pub show_header: bool,
    /// `All Periods` or the selected dates (`Jan - Mar 2026`).
    pub label: Option<String>,
    /// `MONTHS`, `YEARS`...
    pub level: Option<String>,
    /// 0 years, 1 quarters, 2 months, 3 days.
    pub level_index: u32,
    /// From the first one scrolled to.
    pub periods: Vec<TimelinePeriod>,
    /// How many periods are before the first one shown, and how many
    /// there are in all (for the scroll bar).
    pub before: usize,
    pub total: usize,
    /// Some dates are selected (Clear Filter is on).
    pub filtered: bool,
    /// Why it can't filter, if it can't.
    pub problem: Option<String>,
    pub colors: SlicerColors,
}

/// The time levels' names, as Excel shows them.
pub const LEVELS: [&str; 4] = ["YEARS", "QUARTERS", "MONTHS", "DAYS"];

/// Where a slicer's parts go in a `w` × `h` px box (`[x, y, w, h]`).
#[derive(Debug, Clone, PartialEq)]
pub struct SlicerLayout {
    pub header: Option<[f32; 4]>,
    /// Multi-Select and Clear Filter, in the header.
    pub multi: Option<[f32; 4]>,
    pub clear: Option<[f32; 4]>,
    /// One per item that fits, in [`SlicerView::items`] order.
    pub buttons: Vec<[f32; 4]>,
}

const PAD: f32 = 7.0;
const GAP: f32 = 4.0;
const HEADER: f32 = 26.0;
const ICON: f32 = 20.0;

/// Where the parts of `v` go in a `w` × `h` px slicer.
pub fn slicer_layout(v: &SlicerView, w: f32, h: f32) -> SlicerLayout {
    let mut top = PAD;
    let (mut header, mut multi, mut clear) = (None, None, None);
    if v.show_caption {
        header = Some([PAD, PAD, (w - 2.0 * PAD).max(0.0), HEADER]);
        let y = PAD + (HEADER - ICON) / 2.0;
        clear = Some([w - PAD - ICON, y, ICON, ICON]);
        multi = Some([w - PAD - 2.0 * ICON - GAP, y, ICON, ICON]);
        top += HEADER + GAP;
    }
    let cols = v.columns.max(1) as usize;
    let bw = ((w - 2.0 * PAD - GAP * (cols - 1) as f32) / cols as f32).max(1.0);
    let bh = v.button_height.max(14.0);
    let mut buttons = Vec::new();
    for k in 0..v.items.len() {
        let (row, col) = (k / cols, k % cols);
        let y = top + row as f32 * (bh + GAP);
        if y + bh > h - PAD + 0.5 {
            break;
        }
        buttons.push([PAD + col as f32 * (bw + GAP), y, bw, bh]);
    }
    SlicerLayout {
        header,
        multi,
        clear,
        buttons,
    }
}

/// Where a timeline's parts go: its header, label line and period cells.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineLayout {
    pub header: Option<[f32; 4]>,
    /// Clear Filter, in the header.
    pub clear: Option<[f32; 4]>,
    pub label: [f32; 4],
    pub level: [f32; 4],
    /// The group (year) line above the periods.
    pub groups: [f32; 4],
    /// One per period that fits.
    pub periods: Vec<[f32; 4]>,
    /// The scroll bar under the periods and its thumb, when not all of
    /// them fit.
    pub scroll: Option<([f32; 4], [f32; 4])>,
}

/// Width of a timeline period cell (px).
const PERIOD: f32 = 42.0;
/// Height of a timeline's scroll bar (px).
const SCROLL: f32 = 7.0;

pub fn timeline_layout(v: &TimelineView, w: f32, h: f32) -> TimelineLayout {
    let mut top = PAD;
    let (mut header, mut clear) = (None, None);
    if v.show_header {
        header = Some([PAD, PAD, (w - 2.0 * PAD).max(0.0), HEADER]);
        clear = Some([w - PAD - ICON, PAD + (HEADER - ICON) / 2.0, ICON, ICON]);
        top += HEADER;
    }
    let label = [PAD, top, (w - 2.0 * PAD) * 0.6, 20.0];
    let level = [
        PAD + (w - 2.0 * PAD) * 0.6,
        top,
        (w - 2.0 * PAD) * 0.4,
        20.0,
    ];
    top += 22.0;
    let groups = [PAD, top, (w - 2.0 * PAD).max(0.0), 18.0];
    top += 18.0;
    let n = ((w - 2.0 * PAD) / PERIOD).floor().max(0.0) as usize;
    let scrolls = v.total > n.min(v.periods.len()) && n > 0;
    let bottom = if scrolls { SCROLL + 4.0 } else { 0.0 };
    let ph = (h - top - PAD - bottom).clamp(10.0, 26.0);
    let periods = (0..v.periods.len().min(n))
        .map(|k| [PAD + k as f32 * PERIOD, top, PERIOD - 2.0, ph])
        .collect();
    let scroll = scrolls.then(|| {
        let bar = [PAD, top + ph + 4.0, (w - 2.0 * PAD).max(0.0), SCROLL];
        let total = v.total.max(1) as f32;
        let tw = (bar[2] * n as f32 / total).clamp(12.0_f32.min(bar[2]), bar[2]);
        let tx = bar[0] + (bar[2] - tw) * (v.before as f32 / (total - n as f32).max(1.0)).min(1.0);
        (bar, [tx, bar[1], tw, SCROLL])
    });
    TimelineLayout {
        header,
        clear,
        label,
        level,
        groups,
        periods,
        scroll,
    }
}

/// Colours of slicer style `name` (`SlicerStyleLight1`... `Dark6`,
/// `Other1`-`2`; timelines' `TimeSlicerStyle…` the same way).
fn colors(name: Option<&str>, t: &ironcalc_base::types::Theme) -> SlicerColors {
    let name = name
        .unwrap_or("SlicerStyleLight1")
        .trim_start_matches("TimeSlicerStyle")
        .trim_start_matches("SlicerStyle");
    let (family, n) = if let Some(n) = name.strip_prefix("Light") {
        ("light", n)
    } else if let Some(n) = name.strip_prefix("Dark") {
        ("dark", n)
    } else if let Some(n) = name.strip_prefix("Other") {
        ("other", n)
    } else {
        ("light", "1")
    };
    let n: i32 = n.parse().unwrap_or(1).clamp(1, 6);
    let accent = |tint: f64| crate::table::theme(t, 3 + n, tint);
    let white = 0xFFFFFF;
    let grey = |tint: f64| crate::table::theme(t, 1, tint);
    match family {
        "dark" => SlicerColors {
            background: white,
            border: accent(0.0),
            header: grey(0.0),
            selected: accent(0.0),
            selected_text: white,
            unselected: accent(0.6),
            unselected_text: grey(0.0),
            no_data: 0xF2F2F2,
            no_data_text: 0xA6A6A6,
            button_border: accent(0.0),
        },
        "other" => SlicerColors {
            background: white,
            border: 0xBFBFBF,
            header: grey(0.0),
            selected: if n == 1 { 0xD9D9D9 } else { 0x595959 },
            selected_text: if n == 1 { 0x000000 } else { white },
            unselected: white,
            unselected_text: grey(0.0),
            no_data: white,
            no_data_text: 0xA6A6A6,
            button_border: 0xBFBFBF,
        },
        _ => SlicerColors {
            background: white,
            border: 0xBFBFBF,
            header: grey(0.0),
            selected: accent(0.6),
            selected_text: grey(0.0),
            unselected: white,
            unselected_text: grey(0.0),
            no_data: white,
            no_data_text: 0xA6A6A6,
            button_border: 0xBFBFBF,
        },
    }
}

/// An item of a pivot slicer: its shared item (`x` in the cache field's
/// item list, `None` when the cache lists none), value and state.
#[derive(Debug, Clone)]
pub(crate) struct PivotItem {
    pub x: Option<usize>,
    pub value: Val,
    pub selected: bool,
    pub no_data: bool,
}

/// A pivot slicer's connection: the pivot table it shows the state of (the
/// first one present), its cache and the field.
pub(crate) struct Conn {
    def: Arc<Def>,
    cache: Arc<Cache>,
    field: usize,
    /// Every connected pivot table present.
    all: Vec<usize>,
}

impl Workbook {
    /// Slicer `name` on the sheet with `sheet_id` (the file's or one made
    /// in Kantor), with its settings now.
    pub(crate) fn slicer_def(&self, sheet_id: u32, name: &str) -> Option<SlicerDef> {
        let edits = self.edits();
        if let Some(d) = edits.slicer_defs.get(&(sheet_id, name.to_lowercase())) {
            return Some(d.clone());
        }
        self.pivots
            .slicers
            .slicers
            .iter()
            .find(|s| s.sheet_id == sheet_id && s.name.eq_ignore_ascii_case(name))
            .cloned()
            .or_else(|| {
                self.new_slicers()
                    .into_iter()
                    .find(|n| n.def.sheet_id == sheet_id && n.def.name.eq_ignore_ascii_case(name))
                    .map(|n| n.def.clone())
            })
    }

    /// Slicer cache `name` (the file's or one made in Kantor), with its
    /// settings now.
    pub(crate) fn slicer_cache(&self, name: &str) -> Option<SlicerCache> {
        let mut c = self.pivots.slicers.cache(name).cloned().or_else(|| {
            self.new_slicers()
                .into_iter()
                .find(|n| n.cache.name.eq_ignore_ascii_case(name))
                .map(|n| n.cache.clone())
        })?;
        if let Some(set) = self.edits().slicer_caches.get(&name.to_lowercase()) {
            c.descending = set.descending;
            c.cross = set.cross;
            c.hide_no_data = set.hide_no_data;
        }
        Some(c)
    }

    /// The pivot tables of `pivots` (`(sheetId, name)`) present now.
    fn connected(&self, pivots: &[(u32, String)]) -> Vec<usize> {
        let state = self.pivot_state();
        (0..self.pivots.defs.len())
            .filter(|&i| self.pivot_present(&state, i))
            .filter(|&i| {
                let d = self.pivot_def(&state, i);
                pivots
                    .iter()
                    .any(|(tab, n)| *tab == d.sheet_id && n.eq_ignore_ascii_case(&d.name))
            })
            .collect()
    }

    fn conn(&self, c: &SlicerCache) -> Result<Conn> {
        let CacheSource::Pivot { pivots, .. } = &c.source else {
            bail!("not a PivotTable slicer");
        };
        let all = self.connected(pivots);
        let first = *all
            .first()
            .ok_or_else(|| anyhow!("its PivotTable is not in the workbook"))?;
        let state = self.pivot_state();
        let def = self.pivot_def(&state, first);
        let cache = self.pivot_cache(&state, def.cache);
        let field = cache
            .fields
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(&c.source_name))
            .ok_or_else(|| anyhow!("its field {} is not in the PivotTable", c.source_name))?;
        Ok(Conn {
            def,
            cache,
            field,
            all,
        })
    }

    /// A pivot slicer's items: the field's items with the selection of the
    /// first connected pivot table and the items the other fields' filters
    /// leave without data.
    pub(crate) fn pivot_items(&self, c: &SlicerCache) -> Result<(Conn, Vec<PivotItem>)> {
        let conn = self.conn(c)?;
        let (def, cache, f) = (&conn.def, &conn.cache, conn.field);
        let list = cache.fields[f].item_list().to_vec();
        // Records now: refreshed ones, else the source's.
        let records: Option<Vec<Vec<Val>>> = match &cache.records {
            Some(r) => Some((**r).clone()),
            None => self.read_source(cache).ok().map(|(_, _, d)| {
                let cols: Vec<Option<usize>> = cache
                    .fields
                    .iter()
                    .map(|cf| {
                        d.names
                            .iter()
                            .position(|n| cf.database && n.eq_ignore_ascii_case(&cf.name))
                    })
                    .collect();
                d.rows
                    .iter()
                    .map(|r| {
                        cols.iter()
                            .map(|c| c.map_or(Val::Missing, |k| r[k].clone()))
                            .collect()
                    })
                    .collect()
            }),
        };
        // Hidden keys of each field (by the field's items).
        let hidden_keys = |g: usize| -> HashSet<String> {
            let Some(fd) = def.fields.get(g) else {
                return HashSet::new();
            };
            let items = cache.fields[g].item_list();
            fd.items
                .iter()
                .filter(|it| it.hidden)
                .filter_map(|it| items.get(it.x?))
                .map(Val::key)
                .collect()
        };
        let own_hidden = hidden_keys(f);
        let mut values: Vec<(Option<usize>, Val)> = if list.is_empty() {
            // No shared items: the distinct values of the records.
            let mut seen = HashSet::new();
            records
                .iter()
                .flatten()
                .filter_map(|r| r.get(f).cloned())
                .filter(|v| seen.insert(v.key()))
                .map(|v| (None, v))
                .collect()
        } else {
            list.iter()
                .cloned()
                .enumerate()
                .map(|(x, v)| (Some(x), v))
                .collect()
        };
        // Calculated items are not slicer items.
        values.retain(|(x, _)| x.is_none_or(|x| !cache.fields[f].is_calc(x)));
        // Values with data under the other fields' filters.
        let with_data: Option<HashSet<String>> = (cache.fields[f].database)
            .then_some(())
            .and(records.as_ref())
            .map(|recs| {
                let others: Vec<(usize, HashSet<String>)> = (0..cache.fields.len())
                    .filter(|&g| g != f && cache.fields[g].database)
                    .map(|g| (g, hidden_keys(g)))
                    .filter(|(_, h)| !h.is_empty())
                    .collect();
                recs.iter()
                    .filter(|r| {
                        others
                            .iter()
                            .all(|(g, h)| r.get(*g).is_none_or(|v| !h.contains(&v.key())))
                    })
                    .filter_map(|r| r.get(f).map(Val::key))
                    .collect()
            });
        let mut items: Vec<PivotItem> = values
            .into_iter()
            .map(|(x, value)| PivotItem {
                x,
                selected: !own_hidden.contains(&value.key()),
                no_data: with_data
                    .as_ref()
                    .is_some_and(|d| !d.contains(&value.key())),
                value,
            })
            .collect();
        let grouped = cache.fields[f]
            .group
            .as_ref()
            .is_some_and(|g| g.groups_items());
        if !grouped {
            items.sort_by(|a, b| {
                let o = a
                    .value
                    .sort_key()
                    .partial_cmp(&b.value.sort_key())
                    .unwrap_or(std::cmp::Ordering::Equal);
                if c.descending { o.reverse() } else { o }
            });
        }
        if c.cross == Cross::None {
            for it in items.iter_mut() {
                it.no_data = false;
            }
        }
        if c.cross == Cross::Last {
            items.sort_by_key(|it| it.no_data);
        }
        if c.hide_no_data {
            items.retain(|it| !it.no_data || !it.selected);
        }
        Ok((conn, items))
    }

    /// A table slicer's table, sheet and absolute column.
    fn table_column(&self, table: &str, column: u32) -> Result<(u32, String, u32)> {
        let model = self.model.get_model();
        let t = ironcalc_base::kantor_tables::find_table_pub(&model.workbook.tables, table)
            .ok_or_else(|| anyhow!("its table {table} is not in the workbook"))?;
        let k = t
            .columns
            .iter()
            .position(|c| c.id == column)
            .unwrap_or(column.saturating_sub(1) as usize);
        let (_, left, _, _) = ironcalc_base::kantor_tables::table_area(t)
            .ok_or_else(|| anyhow!("the table {table} has no range"))?;
        let sheet = self
            .sheet_index_by_name(&t.sheet_name)
            .ok_or_else(|| anyhow!("the table {table}'s sheet is gone"))?;
        Ok((sheet, t.name.clone(), left as u32 - 1 + k as u32))
    }

    /// A table slicer's items: the column's values, selected when its
    /// filter shows them, no data when the other columns' filters leave
    /// them out.
    fn table_items(&self, c: &SlicerCache, table: &str, column: u32) -> Result<Vec<SlicerItem>> {
        let (sheet, name, col) = self.table_column(table, column)?;
        let scope = FilterScope::Table(name);
        let f = self
            .filter(sheet, &scope)
            .ok_or_else(|| anyhow!("the table {table} has no filter"))?;
        let through = self.filter_values(sheet, &scope, col);
        let mut items: Vec<SlicerItem> = through
            .iter()
            .map(|v| SlicerItem {
                label: if v.text.is_empty() {
                    "(blank)".into()
                } else {
                    v.text.clone()
                },
                selected: v.checked,
                no_data: false,
            })
            .collect();
        // Values the other columns leave out.
        let own_filtered = f.columns.iter().any(|k| k.col == col - f.area.left);
        let shown: HashSet<String> = through.iter().map(|v| v.text.to_lowercase()).collect();
        let mut seen = HashSet::new();
        let mut missing = Vec::new();
        for row in f.area.top + 1..=f.area.bottom {
            let text = self.display_value(sheet, row, col);
            let key = text.to_lowercase();
            if !shown.contains(&key) && seen.insert(key) {
                missing.push(text);
            }
        }
        missing.sort_by_key(|s| s.to_lowercase());
        let checked_value = |text: &str| -> bool {
            match f
                .columns
                .iter()
                .find(|k| k.col == col - f.area.left)
                .map(|k| &k.kind)
            {
                Some(FilterKind::Values { values, blanks, .. }) => {
                    if text.is_empty() {
                        *blanks
                    } else {
                        values.iter().any(|v| v.eq_ignore_ascii_case(text))
                    }
                }
                _ => !own_filtered,
            }
        };
        items.extend(missing.into_iter().map(|text| SlicerItem {
            selected: checked_value(&text),
            label: if text.is_empty() {
                "(blank)".into()
            } else {
                text
            },
            no_data: true,
        }));
        if c.descending {
            items.reverse();
            if c.cross == Cross::Last {
                items.sort_by_key(|it| it.no_data);
            }
        }
        if c.cross == Cross::None {
            for it in items.iter_mut() {
                it.no_data = false;
            }
        }
        if c.hide_no_data {
            items.retain(|it| !it.no_data);
        }
        Ok(items)
    }

    /// Slicer `name` on the sheet with `sheet_id`, as it shows now.
    pub fn slicer_view(&self, sheet_id: u32, name: &str) -> Option<SlicerView> {
        let key = (sheet_id, name.to_lowercase());
        if let Some(v) = self.cached(|c| c.slicers.get(&key).cloned()) {
            return v;
        }
        let v = self.slicer_view_now(sheet_id, name);
        self.cached(|c| c.slicers.insert(key, v.clone()));
        v
    }

    fn slicer_view_now(&self, sheet_id: u32, name: &str) -> Option<SlicerView> {
        let s = self.slicer_def(sheet_id, name)?;
        let theme = &self.model.get_model().workbook.theme;
        let c = self.slicer_cache(&s.cache);
        let (items, problem) = match c.as_ref().map(|c| (c, &c.source)) {
            Some((c, CacheSource::Pivot { .. })) => match self.pivot_items(c) {
                Ok((conn, items)) => {
                    let cache_problem = conn
                        .cache
                        .unsupported
                        .clone()
                        .or_else(|| conn.def.unsupported.clone());
                    (
                        items
                            .into_iter()
                            .map(|it| SlicerItem {
                                label: if it.value == Val::Missing {
                                    "(blank)".into()
                                } else {
                                    it.value.label()
                                },
                                selected: it.selected,
                                no_data: it.no_data,
                            })
                            .collect(),
                        cache_problem,
                    )
                }
                Err(e) => (file_items(c), Some(e.to_string())),
            },
            Some((c, CacheSource::Table { table, column })) => {
                match self.table_items(c, table, *column) {
                    Ok(items) => (items, None),
                    Err(e) => (Vec::new(), Some(e.to_string())),
                }
            }
            Some(_) => (Vec::new(), Some("Kantor can't use OLAP slicers".to_owned())),
            None => (Vec::new(), Some("its slicer cache is missing".to_owned())),
        };
        Some(SlicerView {
            name: s.name.clone(),
            caption: s.caption.clone(),
            show_caption: s.show_caption,
            columns: s.columns,
            button_height: (s.row_height as f32 / 9525.0).max(14.0),
            filtered: items.iter().any(|i| !i.selected),
            items,
            problem,
            colors: colors(s.style.as_deref(), theme),
        })
    }

    /// The names of the slicers on the sheet with `sheet_id` (deleted ones
    /// left out).
    pub fn slicer_names(&self, sheet_id: u32) -> Vec<String> {
        let Some(sheet) = self.sheet_index_of(sheet_id) else {
            return Vec::new();
        };
        self.drawings(sheet)
            .iter()
            .filter_map(|d| match &*d.kind {
                crate::drawing::DrawingKind::Slicer { name } => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    /// Select the items of slicer `name` (`selected`, in
    /// [`SlicerView::items`] order): its pivot tables or table are filtered
    /// to them, as in Excel. All selected clears the filter. One undo step.
    pub fn slicer_select(&mut self, sheet_id: u32, name: &str, selected: &[bool]) -> Result<()> {
        let view = self
            .slicer_view(sheet_id, name)
            .ok_or_else(|| anyhow!("there is no slicer {name}"))?;
        if let Some(why) = &view.problem {
            bail!("can't filter with the slicer {name}: {why}");
        }
        if selected.len() != view.items.len() {
            bail!("the slicer {name} changed; try again");
        }
        if !selected.iter().any(|s| *s) {
            bail!("select at least one item of {}", view.caption);
        }
        let s = self
            .slicer_def(sheet_id, name)
            .ok_or_else(|| anyhow!("there is no slicer {name}"))?;
        let c = self
            .slicer_cache(&s.cache)
            .ok_or_else(|| anyhow!("its slicer cache is missing"))?;
        let all = selected.iter().all(|s| *s);
        match &c.source {
            CacheSource::Pivot { .. } => {
                let (conn, items) = self.pivot_items(&c)?;
                let hidden: HashSet<String> = items
                    .iter()
                    .zip(selected)
                    .filter(|(_, s)| !**s)
                    .map(|(it, _)| it.value.key())
                    .collect();
                self.filter_pivot_field(&conn, &c.source_name, hidden)
            }
            CacheSource::Table { table, column } => {
                let (sheet, tname, col) = self.table_column(table, *column)?;
                let kind = (!all).then(|| {
                    let picked: Vec<&SlicerItem> = view
                        .items
                        .iter()
                        .zip(selected)
                        .filter(|(_, s)| **s)
                        .map(|(it, _)| it)
                        .collect();
                    FilterKind::Values {
                        blanks: picked.iter().any(|it| it.label == "(blank)"),
                        values: picked
                            .iter()
                            .filter(|it| it.label != "(blank)")
                            .map(|it| it.label.clone())
                            .collect(),
                        dates: Vec::new(),
                    }
                });
                self.set_column_filter(sheet, &FilterScope::Table(tname), col, kind)?;
                Ok(())
            }
            CacheSource::Other => bail!("Kantor can't use OLAP slicers"),
        }
    }

    /// Hide items `hidden` (by key) of field `field` in the pivot tables of
    /// `conn` (the others on their caches are laid out again as they are).
    fn filter_pivot_field(
        &mut self,
        conn: &Conn,
        field: &str,
        hidden: HashSet<String>,
    ) -> Result<()> {
        let state = self.pivot_state();
        let targets: Vec<(u32, String)> = conn
            .all
            .iter()
            .map(|&i| {
                let d = self.pivot_def(&state, i);
                (d.sheet_id, d.name.to_lowercase())
            })
            .collect();
        let mut caches: Vec<usize> = conn
            .all
            .iter()
            .map(|&i| self.pivot_def(&state, i).cache)
            .collect();
        caches.sort_unstable();
        caches.dedup();
        let name = field.to_owned();
        let adjust = move |d: &mut Def, c: &Cache| {
            if !targets
                .iter()
                .any(|(s, n)| *s == d.sheet_id && *n == d.name.to_lowercase())
            {
                return;
            }
            let Some(f) = c
                .fields
                .iter()
                .position(|cf| cf.name.eq_ignore_ascii_case(&name))
            else {
                return;
            };
            let list = c.fields[f].item_list().to_vec();
            let Some(fd) = d.fields.get_mut(f) else {
                return;
            };
            if !fd.items.iter().any(|it| it.x.is_some()) {
                // A field on no axis yet: its items, to hide some.
                let mut items: Vec<FieldItem> = (0..list.len())
                    .map(|x| FieldItem {
                        x: Some(x),
                        hidden: false,
                        t: None,
                        expanded: true,
                        n: None,
                        calc: false,
                    })
                    .collect();
                if fd.default_subtotal {
                    items.push(FieldItem {
                        x: None,
                        hidden: false,
                        t: Some("default".into()),
                        expanded: true,
                        n: None,
                        calc: false,
                    });
                }
                fd.items = items;
            }
            for it in fd.items.iter_mut() {
                if let Some(x) = it.x {
                    it.hidden = list.get(x).is_some_and(|v| hidden.contains(&v.key()));
                }
            }
        };
        self.rebuild_pivots(
            &caches,
            Rebuild {
                adjust: Some(Box::new(adjust)),
                axis: vec![field.to_owned()],
                ..Default::default()
            },
        )?;
        Ok(())
    }

    /// A timeline's definition now (its level and scroll position as set
    /// in Kantor).
    pub(crate) fn timeline_def(&self, sheet_id: u32, name: &str) -> Option<TimelineDef> {
        let mut t = self
            .pivots
            .slicers
            .timelines
            .iter()
            .find(|t| t.sheet_id == sheet_id && t.name.eq_ignore_ascii_case(name))?
            .clone();
        if let Some((level, scroll)) = self.edits().timelines.get(&(sheet_id, name.to_lowercase()))
        {
            t.level = *level;
            t.scroll = *scroll;
        }
        Some(t)
    }

    fn timeline_cache(&self, name: &str) -> Option<&TimelineCache> {
        self.pivots
            .slicers
            .timeline_caches
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name))
    }

    /// A timeline's pivot tables present now (the first one shows its
    /// state) and its field's index in their cache.
    pub(crate) fn timeline_conn(&self, c: &TimelineCache) -> Result<(Vec<usize>, usize)> {
        let all = self.connected(&c.pivots);
        let first = *all
            .first()
            .ok_or_else(|| anyhow!("its PivotTable is not in the workbook"))?;
        let state = self.pivot_state();
        let def = self.pivot_def(&state, first);
        let cache = self.pivot_cache(&state, def.cache);
        let field = cache
            .fields
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(&c.source_name))
            .ok_or_else(|| anyhow!("its field {} is not in the PivotTable", c.source_name))?;
        if let Some(why) = cache
            .unsupported
            .clone()
            .or_else(|| def.unsupported.clone())
        {
            bail!(why);
        }
        Ok((all, field))
    }

    /// The dates a timeline selects now (first and last day), `None` for
    /// all: its pivot table's date filter on the field, else what the file
    /// saved.
    pub(crate) fn timeline_selection(&self, c: &TimelineCache) -> Option<(f64, f64)> {
        let Ok((all, field)) = self.timeline_conn(c) else {
            return c.selection;
        };
        let state = self.pivot_state();
        let def = self.pivot_def(&state, all[0]);
        let f = def
            .filters
            .iter()
            .find(|f| f.fld == field && f.kind == "dateBetween")?;
        let a = crate::pivot_calc::date_serial(&f.v1)?;
        let b = crate::pivot_calc::date_serial(&f.v2)?;
        Some((a.min(b).floor(), a.max(b).floor()))
    }

    /// The first and last day a timeline spans: its cache's bounds, else
    /// its field's dates in the pivot table's source.
    fn timeline_bounds(&self, c: &TimelineCache) -> Option<(f64, f64)> {
        // The cache's end date is the day after the last.
        if let Some((a, b)) = c.bounds {
            return Some((a, (b - 1.0).max(a)));
        }
        let all = self.connected(&c.pivots);
        let state = self.pivot_state();
        let def = self.pivot_def(&state, *all.first()?);
        let cache = self.pivot_cache(&state, def.cache);
        let f = cache
            .fields
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(&c.source_name))?;
        let dates: Vec<f64> = match &cache.records {
            Some(r) => r
                .iter()
                .filter_map(|r| match r.get(f) {
                    Some(Val::Date(d) | Val::Num(d)) => Some(*d),
                    _ => None,
                })
                .collect(),
            None => {
                let (_, _, d) = self.read_source(&cache).ok()?;
                let k = d
                    .names
                    .iter()
                    .position(|n| n.eq_ignore_ascii_case(&c.source_name))?;
                d.rows.iter().filter_map(|r| r[k].number()).collect()
            }
        };
        let lo = dates.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = dates.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        lo.is_finite().then(|| (lo.floor(), hi.floor()))
    }

    /// Timeline `name` on the sheet with `sheet_id`, as it shows now.
    pub fn timeline_view(&self, sheet_id: u32, name: &str) -> Option<TimelineView> {
        let t = self.timeline_def(sheet_id, name)?;
        let theme = &self.model.get_model().workbook.theme;
        let c = self.timeline_cache(&t.cache);
        let bounds = c.and_then(|c| self.timeline_bounds(c));
        let selection = c.and_then(|c| self.timeline_selection(c));
        let problem = match c {
            Some(c) => self.timeline_conn(c).err().map(|e| e.to_string()),
            None => Some("its timeline cache is missing".to_owned()),
        };
        let periods = match bounds {
            Some((lo, hi)) => periods(t.level, t.scroll.unwrap_or(lo).max(lo), hi, selection),
            None => Vec::new(),
        };
        let before = bounds.map_or(0, |(lo, _)| {
            let first = t.scroll.unwrap_or(lo).max(lo);
            periods_between(t.level, lo, first)
        });
        let total = bounds.map_or(0, |(lo, hi)| periods_between(t.level, lo, hi) + 1);
        let label = t.show_selection_label.then(|| match selection {
            Some((a, b)) => range_label(t.level, a, b),
            None => "All Periods".to_owned(),
        });
        let level = t
            .show_time_level
            .then(|| LEVELS[t.level.min(3) as usize].to_owned());
        Some(TimelineView {
            name: t.name.clone(),
            caption: t.caption.clone(),
            show_header: t.show_header,
            label,
            level,
            level_index: t.level.min(3),
            periods,
            before,
            total,
            filtered: selection.is_some(),
            problem,
            colors: colors(t.style.as_deref(), theme),
        })
    }

    /// Filter timeline `name`'s pivot tables to the dates `range` (first
    /// and last day, any order; `None`: Clear Filter), as Excel does: a
    /// "between" date filter on the field, named after the timeline. One
    /// undo step.
    pub fn timeline_select(
        &mut self,
        sheet_id: u32,
        name: &str,
        range: Option<(f64, f64)>,
    ) -> Result<()> {
        let t = self
            .timeline_def(sheet_id, name)
            .ok_or_else(|| anyhow!("there is no timeline {name}"))?;
        let c = self
            .timeline_cache(&t.cache)
            .cloned()
            .ok_or_else(|| anyhow!("its timeline cache is missing"))?;
        let (all, _) = self
            .timeline_conn(&c)
            .map_err(|e| anyhow!("can't filter with the timeline {}: {e}", t.caption))?;
        let range = range.map(|(a, b)| (a.min(b).floor(), a.max(b).floor()));
        if self.timeline_selection(&c) == range {
            return Ok(());
        }
        let state = self.pivot_state();
        let targets: Vec<(u32, String)> = all
            .iter()
            .map(|&i| {
                let d = self.pivot_def(&state, i);
                (d.sheet_id, d.name.to_lowercase())
            })
            .collect();
        let mut caches: Vec<usize> = all
            .iter()
            .map(|&i| self.pivot_def(&state, i).cache)
            .collect();
        caches.sort_unstable();
        caches.dedup();
        let field = c.source_name.clone();
        let filter_name = c.name.clone();
        let adjust = move |d: &mut Def, cache: &Cache| {
            if !targets
                .iter()
                .any(|(s, n)| *s == d.sheet_id && *n == d.name.to_lowercase())
            {
                return;
            }
            let Some(f) = cache
                .fields
                .iter()
                .position(|cf| cf.name.eq_ignore_ascii_case(&field))
            else {
                return;
            };
            // A field keeps one date filter: the timeline's replaces it.
            d.filters.retain(|x| {
                x.fld != f
                    || !matches!(
                        crate::pivot_calc::filter_family(&x.kind),
                        Some(crate::pivot_calc::FilterFamily::Date)
                    )
            });
            if let Some((a, b)) = range {
                d.filters.push(crate::pivot_calc::PivotFilter {
                    fld: f,
                    kind: "dateBetween".into(),
                    measure: None,
                    v1: crate::pivot::iso_of_serial(a),
                    v2: crate::pivot::iso_of_serial(b),
                    top: true,
                    name: Some(filter_name.clone()),
                });
            }
        };
        self.rebuild_pivots(
            &caches,
            Rebuild {
                adjust: Some(Box::new(adjust)),
                ..Default::default()
            },
        )?;
        Ok(())
    }

    /// Show timeline `name` by years (0), quarters (1), months (2) or days
    /// (3), scrolled to its selection (else its first date). One undo step.
    pub fn timeline_set_level(&mut self, sheet_id: u32, name: &str, level: u32) -> Result<()> {
        let t = self
            .timeline_def(sheet_id, name)
            .ok_or_else(|| anyhow!("there is no timeline {name}"))?;
        let level = level.min(3);
        if level == t.level {
            return Ok(());
        }
        let first = self
            .timeline_cache(&t.cache)
            .and_then(|c| self.timeline_selection(c).or(self.timeline_bounds(c)))
            .map(|(a, _)| period_bounds(level, a).0);
        let key = (sheet_id, t.name.to_lowercase());
        self.own_edit(|e, _| {
            e.timelines.insert(key, (level, first));
        });
        Ok(())
    }

    /// Scroll timeline `name` by `by` periods (back when negative), within
    /// its dates. Not an undo step (Excel's scroll bar isn't one).
    pub fn timeline_scroll(&mut self, sheet_id: u32, name: &str, by: i32) -> Result<()> {
        let t = self
            .timeline_def(sheet_id, name)
            .ok_or_else(|| anyhow!("there is no timeline {name}"))?;
        let Some((lo, hi)) = self
            .timeline_cache(&t.cache)
            .and_then(|c| self.timeline_bounds(c))
        else {
            return Ok(());
        };
        let mut at = period_bounds(t.level, t.scroll.unwrap_or(lo).max(lo)).0;
        for _ in 0..by.unsigned_abs() {
            let next = if by > 0 {
                period_bounds(t.level, at).1
            } else {
                period_bounds(t.level, at - 1.0).0
            };
            if next < period_bounds(t.level, lo).0 || next > hi {
                break;
            }
            at = next;
        }
        let key = (sheet_id, t.name.to_lowercase());
        let level = t.level;
        self.change_edits(|e, _| {
            e.timelines.insert(key, (level, Some(at)));
        });
        Ok(())
    }

    /// Slicer caches whose pivot slicer items to write at a save, by part
    /// (only after a pivot edit: until then the file's are carried).
    pub(crate) fn slicer_saves(&self, out: &mut HashMap<String, Vec<u8>>) {
        for c in &self.pivots.slicers.caches {
            let CacheSource::Pivot { items: file, .. } = &c.source else {
                continue;
            };
            let Ok((_, items)) = self.pivot_items(c) else {
                continue;
            };
            let mut by_x: Vec<(usize, bool, bool)> = items
                .iter()
                .filter_map(|it| Some((it.x?, it.selected, it.no_data)))
                .collect();
            if by_x.is_empty() {
                continue;
            }
            by_x.sort_by_key(|(x, _, _)| *x);
            // The selection the file has: carried as it is.
            let mut was: Vec<(usize, bool)> = file.iter().map(|(x, s, _)| (*x, *s)).collect();
            was.sort_unstable();
            let now: Vec<(usize, bool)> = by_x.iter().map(|(x, s, _)| (*x, *s)).collect();
            if was == now {
                continue;
            }
            if let Some(xml) = with_items(&c.xml, &by_x) {
                out.insert(c.part.clone(), xml.into_bytes());
            }
        }
    }
}

impl Workbook {
    /// After a save: the saved package (`bytes`, the next save's original)
    /// has the slicer caches as they were written.
    pub(crate) fn slicers_saved(&mut self, bytes: &[u8]) {
        if self.pivots.slicers.caches.is_empty() {
            return;
        }
        let saved = Slicers::read(bytes);
        for c in self.pivots.slicers.caches.iter_mut() {
            if let Some(n) = saved
                .caches
                .iter()
                .find(|n| n.part.eq_ignore_ascii_case(&c.part) || n.name == c.name)
            {
                c.part = n.part.clone();
                c.xml = n.xml.clone();
                c.source = n.source.clone();
            }
        }
    }
}

/// The items the file saved for a slicer whose pivot table Kantor can't
/// read: numbered, as selected.
fn file_items(c: &SlicerCache) -> Vec<SlicerItem> {
    match &c.source {
        CacheSource::Pivot { items, .. } => items
            .iter()
            .map(|(x, s, nd)| SlicerItem {
                label: format!("{}", x + 1),
                selected: *s,
                no_data: *nd,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `xml` (a slicer cache) with its `<tabular>` items `items` (`x`,
/// selected, no data).
pub(crate) fn with_items(xml: &str, items: &[(usize, bool, bool)]) -> Option<String> {
    let doc = Document::parse(xml).ok()?;
    let tabular = deep(doc.root_element(), "tabular")?;
    let prefix = {
        let start = tabular.range().start + 1;
        let tag = &xml[start..];
        let end = tag.find(|c: char| c.is_whitespace() || c == '>' || c == '/')?;
        match tag[..end].split_once(':') {
            Some((p, _)) => format!("{p}:"),
            None => String::new(),
        }
    };
    let mut list = format!("<{prefix}items count=\"{}\">", items.len());
    for (x, s, nd) in items {
        list.push_str(&format!("<{prefix}i x=\"{x}\""));
        if *s {
            list.push_str(" s=\"1\"");
        }
        if *nd {
            list.push_str(" nd=\"1\"");
        }
        list.push_str("/>");
    }
    list.push_str(&format!("</{prefix}items>"));
    let mut out = xml.to_owned();
    match local(tabular, "items") {
        Some(old) => out.replace_range(old.range(), &list),
        None => {
            let r = tabular.range();
            let text = &xml[r.clone()];
            if let Some(open) = text.strip_suffix("/>") {
                out.replace_range(r, &format!("{open}>{list}</{prefix}tabular>"));
            } else {
                let at = r.start + text.find('>')? + 1;
                out.insert_str(at, &list);
            }
        }
    }
    Some(out)
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The start of the period holding serial `d` at `level`, and the next.
fn period_bounds(level: u32, d: f64) -> (f64, f64) {
    let (y, m, day) = crate::pivot_calc::ymd(d);
    let serial = |y: i64, m: i64, d: i64| {
        crate::pivot::serial_of_iso(&format!("{y:04}-{m:02}-{d:02}")).unwrap_or(0.0)
    };
    match level {
        0 => (serial(y, 1, 1), serial(y + 1, 1, 1)),
        1 => {
            let q = (m - 1) / 3;
            let start = serial(y, q * 3 + 1, 1);
            let (ny, nm) = if q == 3 { (y + 1, 1) } else { (y, q * 3 + 4) };
            (start, serial(ny, nm, 1))
        }
        2 => {
            let (ny, nm) = if m == 12 { (y + 1, 1) } else { (y, m + 1) };
            (serial(y, m, 1), serial(ny, nm, 1))
        }
        _ => (serial(y, m, day), serial(y, m, day) + 1.0),
    }
}

/// A period's label and its group label (`Jan` in `2026`).
fn period_label(level: u32, d: f64) -> (String, String) {
    let (y, m, day) = crate::pivot_calc::ymd(d);
    match level {
        0 => (y.to_string(), String::new()),
        1 => (format!("Q{}", (m - 1) / 3 + 1), y.to_string()),
        2 => (MONTHS[(m - 1) as usize].to_owned(), y.to_string()),
        _ => (day.to_string(), format!("{} {y}", MONTHS[(m - 1) as usize])),
    }
}

/// The periods from the one holding `from` to the one holding `to`.
fn periods(level: u32, from: f64, to: f64, selection: Option<(f64, f64)>) -> Vec<TimelinePeriod> {
    let mut out = Vec::new();
    let mut at = period_bounds(level, from).0;
    let mut last_group = String::new();
    while at <= to && out.len() < 400 {
        let (start, next) = period_bounds(level, at);
        let (label, group) = period_label(level, start);
        out.push(TimelinePeriod {
            label,
            start,
            end: next,
            group: (group != last_group && !group.is_empty()).then(|| group.clone()),
            selected: selection.is_some_and(|(a, b)| start <= b.floor() && next > a.floor()),
        });
        last_group = group;
        at = next;
    }
    out
}

/// How many periods start after the one holding `from`, up to the one
/// holding `to`.
fn periods_between(level: u32, from: f64, to: f64) -> usize {
    let mut at = period_bounds(level, from).0;
    let mut n = 0;
    while n < 100_000 {
        let next = period_bounds(level, at).1;
        if next > to {
            break;
        }
        at = next;
        n += 1;
    }
    n
}

/// `Jan - Mar 2026`, `2025 - 2026`, `Q1 - Q2 2026`...
fn range_label(level: u32, a: f64, b: f64) -> String {
    let (la, ga) = period_label(level, a);
    let (lb, gb) = period_label(level, b);
    let one = |l: String, g: String| {
        if g.is_empty() { l } else { format!("{l} {g}") }
    };
    if (la.clone(), ga.clone()) == (lb.clone(), gb.clone()) {
        return one(la, ga);
    }
    if ga == gb && !ga.is_empty() {
        return format!("{la} - {lb} {ga}");
    }
    format!("{} - {}", one(la, ga), one(lb, gb))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(n: usize, columns: u32) -> SlicerView {
        SlicerView {
            name: "Region".into(),
            caption: "Region".into(),
            show_caption: true,
            columns,
            button_height: 25.0,
            items: (0..n)
                .map(|k| SlicerItem {
                    label: format!("{k}"),
                    selected: true,
                    no_data: false,
                })
                .collect(),
            filtered: false,
            problem: None,
            colors: colors(None, &Default::default()),
        }
    }

    #[test]
    fn buttons_fill_columns_and_stop_at_the_bottom() {
        let l = slicer_layout(&view(5, 2), 200.0, 100.0);
        assert!(l.header.is_some() && l.clear.is_some());
        // Two rows of two fit under the header (rows at 37 and 66, 25 high).
        assert_eq!(l.buttons.len(), 4);
        assert_eq!(l.buttons[1][1], l.buttons[0][1]);
        assert!(l.buttons[1][0] > l.buttons[0][0]);
    }

    #[test]
    fn items_are_rewritten_in_the_cache() {
        let xml = r#"<slicerCacheDefinition xmlns="http://schemas.microsoft.com/office/spreadsheetml/2009/9/main" name="S" sourceName="Region"><data><tabular pivotCacheId="1"><items count="2"><i x="0" s="1"/><i x="1" s="1"/></items></tabular></data></slicerCacheDefinition>"#;
        let out = with_items(xml, &[(0, true, false), (1, false, true)]).unwrap();
        assert!(out.contains(r#"<items count="2"><i x="0" s="1"/><i x="1" nd="1"/></items>"#));
        let empty = r#"<x14:slicerCacheDefinition xmlns:x14="urn:x"><x14:data><x14:tabular pivotCacheId="1"/></x14:data></x14:slicerCacheDefinition>"#;
        let out = with_items(empty, &[(0, true, false)]).unwrap();
        assert!(out.contains(r#"<x14:tabular pivotCacheId="1"><x14:items count="1"><x14:i x="0" s="1"/></x14:items></x14:tabular>"#), "{out}");
    }

    #[test]
    fn timeline_periods_and_labels() {
        let s = |d: &str| crate::pivot::serial_of_iso(d).unwrap();
        let p = periods(
            2,
            s("2025-12-05"),
            s("2026-03-30"),
            Some((s("2026-01-01"), s("2026-02-28"))),
        );
        let labels: Vec<_> = p.iter().map(|p| p.label.as_str()).collect();
        assert_eq!(labels, ["Dec", "Jan", "Feb", "Mar"]);
        assert_eq!(p[0].group.as_deref(), Some("2025"));
        assert_eq!(p[1].group.as_deref(), Some("2026"));
        assert_eq!(p[2].group, None);
        assert_eq!(
            p.iter().map(|p| p.selected).collect::<Vec<_>>(),
            [false, true, true, false]
        );
        assert_eq!(
            range_label(2, s("2026-01-01"), s("2026-02-28")),
            "Jan - Feb 2026"
        );
        assert_eq!(
            range_label(0, s("2025-01-01"), s("2026-02-28")),
            "2025 - 2026"
        );
    }
}
