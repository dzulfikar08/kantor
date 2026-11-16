//! Making and changing slicers in Kantor, as Excel does: Insert Slicer
//! (from a pivot table or a table: one slicer per field picked, each with
//! its own cache), Slicer Settings (caption, header, sorting, items with no
//! data), slicer styles and columns. Deleting, moving and resizing are the
//! drawing layer's ([`Workbook::delete_drawing`], [`Workbook::place_drawing`]).
//!
//! A slicer made in Kantor is a drawing made in Kantor (`NewChart` with a
//! `NewShape` holding the anchor's `mc:AlternateContent`, as Excel writes
//! it) that carries its slicer and cache: undoing the insert or deleting
//! the drawing removes both. Settings live in the history state's
//! [`crate::edits::Edits`] for the file's slicers and Kantor's alike.
//!
//! A save ([`Workbook::slicer_plan`]) lists every slicer and timeline as it
//! should be; `preserve::slicer_parts` makes the package agree: it adds
//! the parts of new slicers and caches (with their workbook and sheet
//! extensions and `#N/A` names), removes those of deleted ones and writes
//! changed settings, timeline levels and timeline selections in place.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

use crate::Workbook;
use crate::chart_edit::{NEW_CHART_BASE, NewChart, NewShape};
use crate::drawing::{Anchor, DrawingKey, DrawingKind, EditAs, Marker};
use crate::pivot::Rebuild;
use crate::slicer::{CacheSettings, CacheSource, Cross, SlicerCache, SlicerDef};

/// A slicer made in Kantor and its cache.
#[derive(Debug, Clone)]
pub(crate) struct NewSlicer {
    pub def: SlicerDef,
    pub cache: SlicerCache,
}

/// What a new slicer filters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlicerSource {
    /// A pivot table, by its sheet (index) and name.
    Pivot { sheet: u32, name: String },
    /// A table, by name.
    Table(String),
}

/// A slicer's settings, as Excel's Slicer Settings dialog and the Slicer
/// tab show them.
#[derive(Debug, Clone, PartialEq)]
pub struct SlicerSettings {
    /// The slicer's name (read-only here).
    pub name: String,
    /// The field or column it filters (read-only).
    pub source_name: String,
    /// The cache's name, used in formulas (read-only).
    pub cache_name: String,
    pub caption: String,
    /// Display header.
    pub show_caption: bool,
    /// Descending (Z to A).
    pub descending: bool,
    pub hide_no_data: bool,
    /// Visually indicate items with no data.
    pub mark_no_data: bool,
    /// Show items with no data last.
    pub no_data_last: bool,
    /// `SlicerStyleLight1`...
    pub style: String,
    pub columns: u32,
    /// Button height, px.
    pub button_height: f32,
}

/// The built-in slicer styles, as Excel's gallery lists them.
pub const SLICER_STYLES: [&str; 14] = [
    "SlicerStyleLight1",
    "SlicerStyleLight2",
    "SlicerStyleLight3",
    "SlicerStyleLight4",
    "SlicerStyleLight5",
    "SlicerStyleLight6",
    "SlicerStyleOther1",
    "SlicerStyleOther2",
    "SlicerStyleDark1",
    "SlicerStyleDark2",
    "SlicerStyleDark3",
    "SlicerStyleDark4",
    "SlicerStyleDark5",
    "SlicerStyleDark6",
];

/// A slicer's default size in cells (about Excel's 1.8" × 2.6").
const SLICER_COLS: u32 = 3;
const SLICER_ROWS: u32 = 13;

const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const NS_SLE: &str = "http://schemas.microsoft.com/office/drawing/2010/slicer";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The anchor's object for a slicer, as Excel writes it: the slicer frame
/// for Excel 2010+ (`a14`, or `sle15` for a table slicer) and its fallback
/// shape for older versions.
pub(crate) fn slicer_object(name: &str, table: bool) -> String {
    let (req, ns) = if table {
        (
            "sle15",
            "http://schemas.microsoft.com/office/drawing/2012/slicer",
        )
    } else {
        (
            "a14",
            "http://schemas.microsoft.com/office/drawing/2010/main",
        )
    };
    let name = esc(name);
    format!(
        "<mc:AlternateContent xmlns:mc=\"{NS_MC}\"><mc:Choice xmlns:{req}=\"{ns}\" Requires=\"{req}\"><xdr:graphicFrame xmlns:xdr=\"{NS_XDR}\" xmlns:a=\"{NS_A}\" macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"2\" name=\"{name}\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></xdr:xfrm><a:graphic><a:graphicData uri=\"{NS_SLE}\"><sle:slicer xmlns:sle=\"{NS_SLE}\" name=\"{name}\"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp xmlns:xdr=\"{NS_XDR}\" xmlns:a=\"{NS_A}\" macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"3\" name=\"\"/><xdr:cNvSpPr><a:spLocks noTextEdit=\"1\"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"1828800\" cy=\"2381250\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:solidFill><a:prstClr val=\"white\"/></a:solidFill><a:ln w=\"1\"><a:solidFill><a:prstClr val=\"green\"/></a:solidFill></a:ln></xdr:spPr><xdr:txBody><a:bodyPr vertOverflow=\"clip\" horzOverflow=\"clip\"/><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\" sz=\"1100\"/><a:t>This shape represents a slicer. Slicers are supported in Excel 2010 or later.</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent>"
    )
}

/// `base`, or `base` with the first number from 1 that makes it unused
/// (`sep` between them).
fn unique(base: &str, sep: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    (1..)
        .map(|n| format!("{base}{sep}{n}"))
        .find(|n| !taken(n))
        .expect("a free name")
}

/// A slicer cache's name for field `field`: `Slicer_` and the field's
/// letters, digits and underscores (Excel's rule for the name formulas
/// use).
fn cache_base(field: &str) -> String {
    let mut s = String::from("Slicer_");
    for c in field.chars() {
        s.push(if c.is_alphanumeric() || c == '_' {
            c
        } else {
            '_'
        });
    }
    s
}

impl Workbook {
    /// The slicers made in Kantor present now (their drawing not deleted,
    /// their sheet still there).
    pub(crate) fn new_slicers(&self) -> Vec<Arc<NewSlicer>> {
        let edits = self.edits();
        edits
            .charts
            .iter()
            .filter(|c| self.sheet_index_of(c.sheet_id).is_some())
            .filter_map(|c| c.shape.as_ref()?.slicer.clone())
            .collect()
    }

    /// The fields Insert Slicer offers for `source`: a pivot table's fields
    /// (calculated ones left out, as Excel does), a table's columns.
    pub fn slicer_fields(&self, source: &SlicerSource) -> Result<Vec<String>> {
        match source {
            SlicerSource::Pivot { sheet, name } => {
                let (def, cache) = self.pivot_named(*sheet, name)?;
                if let Some(why) = cache.unsupported.clone().or(def.unsupported.clone()) {
                    bail!("Kantor can't add a slicer to this PivotTable: {why}");
                }
                Ok(cache
                    .fields
                    .iter()
                    .filter(|f| f.formula.is_none())
                    .map(|f| f.name.clone())
                    .collect())
            }
            SlicerSource::Table(name) => {
                let model = self.model.get_model();
                let t = ironcalc_base::kantor_tables::find_table_pub(&model.workbook.tables, name)
                    .ok_or_else(|| anyhow!("there is no table {name}"))?;
                Ok(t.columns.iter().map(|c| c.name.clone()).collect())
            }
        }
    }

    /// Pivot table `name` on `sheet` now: its definition and cache.
    fn pivot_named(
        &self,
        sheet: u32,
        name: &str,
    ) -> Result<(Arc<crate::pivot::Def>, Arc<crate::pivot::Cache>)> {
        let state = self.pivot_state();
        let sheet_id = self.sheet_id(sheet);
        let i = (0..self.pivots.defs.len())
            .filter(|&i| self.pivot_present(&state, i))
            .find(|&i| {
                let d = self.pivot_def(&state, i);
                d.sheet_id == sheet_id && d.name.eq_ignore_ascii_case(name)
            })
            .ok_or_else(|| anyhow!("there is no PivotTable {name}"))?;
        let def = self.pivot_def(&state, i);
        let cache = self.pivot_cache(&state, def.cache);
        Ok((def, cache))
    }

    /// Insert Slicer: one slicer for each of `fields` of `source`, on the
    /// sheet `sheet`, cascading from cell `(row, col)` as Excel does. Each
    /// gets its own cache (`Slicer_Region`, `Slicer_Region1`...) with every
    /// item selected (a pivot table's current filter on the field stays).
    /// One undo step. Returns their drawings' keys.
    pub fn insert_slicers(
        &mut self,
        sheet: u32,
        source: &SlicerSource,
        fields: &[String],
        (row, col): (u32, u32),
    ) -> Result<Vec<DrawingKey>> {
        if fields.is_empty() {
            bail!("pick at least one field");
        }
        let offered = self.slicer_fields(source)?;
        let fields: Vec<String> = fields
            .iter()
            .map(|f| {
                offered
                    .iter()
                    .find(|o| o.eq_ignore_ascii_case(f))
                    .cloned()
                    .ok_or_else(|| anyhow!("there is no field {f}"))
            })
            .collect::<Result<_>>()?;
        // Names: slicers unique in the workbook, caches unique among caches
        // and defined names.
        let mut slicer_names: Vec<String> = self
            .pivots
            .slicers
            .slicers
            .iter()
            .map(|s| s.name.to_lowercase())
            .chain(self.new_slicers().iter().map(|n| n.def.name.to_lowercase()))
            .collect();
        let mut cache_names: Vec<String> = self
            .pivots
            .slicers
            .caches
            .iter()
            .map(|c| c.name.to_lowercase())
            .chain(
                self.pivots
                    .slicers
                    .timeline_caches
                    .iter()
                    .map(|c| c.name.to_lowercase()),
            )
            .chain(
                self.new_slicers()
                    .iter()
                    .map(|n| n.cache.name.to_lowercase()),
            )
            .chain(
                self.model
                    .get_model()
                    .workbook
                    .defined_names
                    .iter()
                    .map(|d| d.name.to_lowercase()),
            )
            .collect();
        let sheet_id = self.sheet_id(sheet);
        let mut made: Vec<NewSlicer> = Vec::new();
        for field in &fields {
            let name = unique(field, " ", |n| slicer_names.contains(&n.to_lowercase()));
            slicer_names.push(name.to_lowercase());
            let cache_name = unique(&cache_base(field), "", |n| {
                cache_names.contains(&n.to_lowercase())
            });
            cache_names.push(cache_name.to_lowercase());
            let source = match source {
                SlicerSource::Pivot { sheet, name } => CacheSource::Pivot {
                    pivots: vec![(self.sheet_id(*sheet), name.clone())],
                    items: Vec::new(),
                },
                SlicerSource::Table(t) => {
                    let model = self.model.get_model();
                    let table =
                        ironcalc_base::kantor_tables::find_table_pub(&model.workbook.tables, t)
                            .ok_or_else(|| anyhow!("there is no table {t}"))?;
                    let column = table
                        .columns
                        .iter()
                        .find(|c| c.name.eq_ignore_ascii_case(field))
                        .map(|c| c.id)
                        .ok_or_else(|| anyhow!("there is no column {field}"))?;
                    CacheSource::Table {
                        table: table.name.clone(),
                        column,
                    }
                }
            };
            made.push(NewSlicer {
                def: SlicerDef {
                    sheet_id,
                    name: name.clone(),
                    cache: cache_name.clone(),
                    caption: field.clone(),
                    columns: 1,
                    row_height: 241_300,
                    show_caption: true,
                    style: None,
                },
                cache: SlicerCache {
                    part: String::new(),
                    xml: Arc::new(String::new()),
                    name: cache_name,
                    source_name: field.clone(),
                    source,
                    descending: false,
                    cross: Cross::Last,
                    hide_no_data: false,
                },
            });
        }
        // A pivot field needs its shared items (items to select).
        let pivot = match source {
            SlicerSource::Pivot { sheet, name } => {
                let state = self.pivot_state();
                let sid = self.sheet_id(*sheet);
                let i = (0..self.pivots.defs.len())
                    .filter(|&i| self.pivot_present(&state, i))
                    .find(|&i| {
                        let d = self.pivot_def(&state, i);
                        d.sheet_id == sid && d.name.eq_ignore_ascii_case(name)
                    })
                    .ok_or_else(|| anyhow!("there is no PivotTable {name}"))?;
                Some(self.pivot_def(&state, i).cache)
            }
            SlicerSource::Table(_) => None,
        };
        let anchors: Vec<Anchor> = (0..made.len() as u32)
            .map(|k| {
                let col = (col + k).min(crate::MAX_COLS - 1 - SLICER_COLS);
                let row = (row + k).min(crate::MAX_ROWS - 1 - SLICER_ROWS);
                Anchor::TwoCell {
                    from: Marker {
                        col,
                        col_off: 0,
                        row,
                        row_off: 0,
                    },
                    to: Marker {
                        col: col + SLICER_COLS,
                        col_off: 0,
                        row: row + SLICER_ROWS,
                        row_off: 0,
                    },
                    edit_as: EditAs::OneCell,
                }
            })
            .collect();
        let mut keys = Vec::new();
        let push = |e: &mut crate::edits::Edits,
                    layout: &crate::shift::Layout,
                    keys: &mut Vec<DrawingKey>| {
            for (n, anchor) in made.into_iter().zip(anchors) {
                let id = e.charts.iter().map(|c| c.id).max().map_or(1, |m| m + 1);
                let table = matches!(n.cache.source, CacheSource::Table { .. });
                let name = n.def.name.clone();
                e.charts.push(NewChart {
                    id,
                    sheet_id,
                    spec: crate::ChartSpec::placeholder(),
                    anchor,
                    layout: layout.clone(),
                    xml: None,
                    picture: None,
                    shape: Some(Arc::new(NewShape {
                        xml: slicer_object(&name, table).into(),
                        name,
                        slicer: Some(Arc::new(n)),
                    })),
                });
                keys.push(DrawingKey {
                    sheet_id,
                    index: NEW_CHART_BASE + id as usize,
                });
            }
        };
        match pivot {
            Some(cache) => {
                self.rebuild_pivots(
                    &[cache],
                    Rebuild {
                        axis: fields.clone(),
                        ..Default::default()
                    },
                )?;
                self.change_edits(|e, layout| push(e, layout, &mut keys));
            }
            None => self.own_edit(|e, layout| push(e, layout, &mut keys)),
        }
        Ok(keys)
    }

    /// Where Insert Slicer puts slicers for `source` on its sheet: right of
    /// it, a column apart, level with its top.
    pub fn slicer_place(&self, source: &SlicerSource) -> Option<(u32, (u32, u32))> {
        match source {
            SlicerSource::Pivot { sheet, name } => {
                let p = self
                    .pivot_tables(*sheet)
                    .into_iter()
                    .find(|p| p.name.eq_ignore_ascii_case(name))?;
                Some((*sheet, (p.bounds.top, p.bounds.right + 2)))
            }
            SlicerSource::Table(name) => {
                for sheet in 0..self.sheet_names().len() as u32 {
                    if let Some(t) = self
                        .tables(sheet)
                        .into_iter()
                        .find(|t| t.name.eq_ignore_ascii_case(name))
                    {
                        return Some((sheet, (t.area.top, t.area.right + 2)));
                    }
                }
                None
            }
        }
    }

    /// Slicer `name`'s settings now.
    pub fn slicer_settings(&self, sheet_id: u32, name: &str) -> Option<SlicerSettings> {
        let d = self.slicer_def(sheet_id, name)?;
        let c = self.slicer_cache(&d.cache)?;
        Some(SlicerSettings {
            name: d.name.clone(),
            source_name: c.source_name.clone(),
            cache_name: c.name.clone(),
            caption: d.caption.clone(),
            show_caption: d.show_caption,
            descending: c.descending,
            hide_no_data: c.hide_no_data,
            mark_no_data: c.cross != Cross::None,
            no_data_last: c.cross == Cross::Last,
            style: d
                .style
                .clone()
                .unwrap_or_else(|| "SlicerStyleLight1".to_owned()),
            columns: d.columns,
            button_height: d.row_height as f32 / crate::drawing::EMU_PER_PX as f32,
        })
    }

    /// Change slicer `name`'s settings to `s` (its name, field and cache
    /// stay): the caption, header, style, columns and button height are the
    /// slicer's, sorting and items with no data its cache's (every slicer
    /// of the cache shows them). One undo step.
    pub fn set_slicer_settings(
        &mut self,
        sheet_id: u32,
        name: &str,
        s: &SlicerSettings,
    ) -> Result<()> {
        let mut d = self
            .slicer_def(sheet_id, name)
            .ok_or_else(|| anyhow!("there is no slicer {name}"))?;
        let c = self
            .slicer_cache(&d.cache)
            .ok_or_else(|| anyhow!("its slicer cache is missing"))?;
        if s.columns == 0 || s.columns > 20_000 {
            bail!("the number of columns must be between 1 and 20000");
        }
        if !(14.0..=2000.0).contains(&s.button_height) {
            bail!("the button height must be between 14 and 2000 px");
        }
        let caption = s.caption.trim();
        d.caption = caption.to_owned();
        d.show_caption = s.show_caption;
        d.style = Some(s.style.clone()).filter(|st| st != "SlicerStyleLight1" || d.style.is_some());
        d.columns = s.columns;
        d.row_height = (s.button_height as f64 * crate::drawing::EMU_PER_PX).round() as i64;
        let cache = CacheSettings {
            descending: s.descending,
            cross: match (s.mark_no_data, s.no_data_last) {
                (false, _) => Cross::None,
                (true, true) => Cross::Last,
                (true, false) => Cross::InPlace,
            },
            hide_no_data: s.hide_no_data,
        };
        let same_def = self.slicer_settings(sheet_id, name).as_ref() == Some(s);
        if same_def {
            return Ok(());
        }
        let key = (sheet_id, d.name.to_lowercase());
        let cache_key = c.name.to_lowercase();
        self.own_edit(|e, _| {
            e.slicer_defs.insert(key, d);
            e.slicer_caches.insert(cache_key, cache);
        });
        Ok(())
    }

    /// Whether drawing `key` is a slicer (its name) — for the Slicer tab.
    pub fn slicer_of(&self, key: DrawingKey) -> Option<String> {
        let sheet = self.sheet_index_of(key.sheet_id)?;
        self.drawings(sheet)
            .into_iter()
            .find(|d| d.key == key)
            .and_then(|d| match &*d.kind {
                DrawingKind::Slicer { name } => Some(name.clone()),
                _ => None,
            })
    }
}

impl Workbook {
    /// What a save makes the slicer and timeline parts say (see
    /// `preserve::slicer_parts`).
    pub(crate) fn slicer_plan(&self) -> crate::preserve::SlicerPlan {
        use crate::preserve::{
            CacheKind, CacheOut, SlicerOut, SlicerPlan, TimelineCacheOut, TimelineFilter,
            TimelineOut,
        };
        let file = &self.pivots.slicers;
        let new = self.new_slicers();
        if file.caches.is_empty() && file.timeline_caches.is_empty() && new.is_empty() {
            return SlicerPlan::default();
        }
        let mut plan = SlicerPlan {
            active: true,
            ..Default::default()
        };
        // Slicers: the file's not deleted (on sheets still there), then
        // Kantor's.
        let mut defs: Vec<SlicerDef> = Vec::new();
        for d in &file.slicers {
            let Some(sheet) = self.sheet_index_of(d.sheet_id) else {
                continue;
            };
            let opened = self.drawings.opened_slicers(d.sheet_id);
            let shown = self.drawings(sheet).iter().any(|x| {
                matches!(&*x.kind, DrawingKind::Slicer { name } if name.eq_ignore_ascii_case(&d.name))
            });
            if opened.contains(&d.name.to_lowercase()) && !shown {
                continue;
            }
            defs.push(
                self.slicer_def(d.sheet_id, &d.name)
                    .unwrap_or_else(|| d.clone()),
            );
        }
        for n in &new {
            defs.push(
                self.slicer_def(n.def.sheet_id, &n.def.name)
                    .unwrap_or_else(|| n.def.clone()),
            );
        }
        plan.slicers = defs
            .iter()
            .map(|d| SlicerOut {
                sheet_id: d.sheet_id,
                name: d.name.clone(),
                cache: d.cache.clone(),
                caption: d.caption.clone(),
                columns: d.columns,
                row_height: d.row_height,
                show_caption: d.show_caption,
                style: d.style.clone(),
            })
            .collect();
        // Caches: every one (unused ones are removed by the save).
        let names: Vec<String> = file
            .caches
            .iter()
            .map(|c| c.name.clone())
            .chain(new.iter().map(|n| n.cache.name.clone()))
            .collect();
        for name in names {
            let Some(c) = self.slicer_cache(&name) else {
                continue;
            };
            let is_new = new.iter().any(|n| n.cache.name.eq_ignore_ascii_case(&name));
            let kind = match &c.source {
                CacheSource::Pivot { pivots, .. } => CacheKind::Pivot {
                    pivots: pivots.clone(),
                    items: if is_new {
                        self.new_cache_items(&c)
                    } else {
                        Vec::new()
                    },
                },
                CacheSource::Table { table, column } => CacheKind::Table {
                    table: table.clone(),
                    column: *column,
                },
                CacheSource::Other => {
                    // Kept as the file has it.
                    CacheKind::Pivot {
                        pivots: Vec::new(),
                        items: Vec::new(),
                    }
                }
            };
            let other = matches!(c.source, CacheSource::Other);
            plan.caches.push(CacheOut {
                name: c.name.clone(),
                source_name: c.source_name.clone(),
                kind,
                descending: c.descending,
                cross: match c.cross {
                    Cross::Last => None,
                    Cross::InPlace => Some("showItemsWithNoData"),
                    Cross::None => Some("none"),
                },
                hide_no_data: c.hide_no_data,
                new: is_new && !other,
            });
        }
        // Timelines.
        for ((sheet_id, name), (level, scroll)) in &self.edits().timelines {
            if self.sheet_index_of(*sheet_id).is_none() {
                continue;
            }
            plan.timelines.push(TimelineOut {
                name: name.clone(),
                level: *level,
                scroll: *scroll,
            });
        }
        for c in &file.timeline_caches {
            let Ok((all, field)) = self.timeline_conn(c) else {
                continue;
            };
            let state = self.pivot_state();
            let def = self.pivot_def(&state, all[0]);
            let filter = def
                .filters
                .iter()
                .position(|f| f.fld == field && f.kind == "dateBetween")
                .map(|k| TimelineFilter {
                    id: k + 1,
                    tab: def.sheet_id,
                    pivot: def.name.clone(),
                });
            plan.timeline_caches.push(TimelineCacheOut {
                name: c.name.clone(),
                selection: self.timeline_selection(c),
                filter,
            });
        }
        plan
    }

    /// A pivot slicer cache's items to save (`x`, selected, no data).
    fn new_cache_items(&self, c: &SlicerCache) -> Vec<(usize, bool, bool)> {
        let Ok((_, items)) = self.pivot_items(c) else {
            return Vec::new();
        };
        let mut out: Vec<(usize, bool, bool)> = items
            .iter()
            .filter_map(|it| Some((it.x?, it.selected, it.no_data)))
            .collect();
        out.sort_by_key(|(x, _, _)| *x);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_unique() {
        assert_eq!(cache_base("Unit price"), "Slicer_Unit_price");
        let taken = ["region", "region 1"];
        assert_eq!(
            unique("Region", " ", |n| taken
                .contains(&n.to_lowercase().as_str())),
            "Region 2"
        );
        let object = slicer_object("A & B", true);
        assert!(roxmltree::Document::parse(&object).is_ok());
        assert!(object.contains("Requires=\"sle15\""));
    }
}
