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
use crate::slicer::{
    CacheSettings, CacheSource, Cross, SlicerCache, SlicerDef, TimelineCache, TimelineDef,
};

/// A slicer made in Kantor and its cache.
#[derive(Debug, Clone)]
pub(crate) struct NewSlicer {
    pub def: SlicerDef,
    pub cache: SlicerCache,
}

/// A timeline made in Kantor (Insert Timeline) and its cache.
#[derive(Debug, Clone)]
pub(crate) struct NewTimeline {
    pub def: TimelineDef,
    pub cache: TimelineCache,
}

/// A timeline's settings, as Excel's Timeline Settings dialog and the
/// Timeline tab show them.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineSettings {
    /// The timeline's name (read-only here).
    pub name: String,
    /// The date field it filters (read-only).
    pub source_name: String,
    /// The cache's name, used in formulas (read-only).
    pub cache_name: String,
    pub caption: String,
    pub show_header: bool,
    pub show_selection_label: bool,
    pub show_time_level: bool,
    pub show_scrollbar: bool,
    /// `TimeSlicerStyleLight1`...
    pub style: String,
}

/// The built-in timeline styles, as Excel's gallery lists them.
pub const TIMELINE_STYLES: [&str; 12] = [
    "TimeSlicerStyleLight1",
    "TimeSlicerStyleLight2",
    "TimeSlicerStyleLight3",
    "TimeSlicerStyleLight4",
    "TimeSlicerStyleLight5",
    "TimeSlicerStyleLight6",
    "TimeSlicerStyleDark1",
    "TimeSlicerStyleDark2",
    "TimeSlicerStyleDark3",
    "TimeSlicerStyleDark4",
    "TimeSlicerStyleDark5",
    "TimeSlicerStyleDark6",
];

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
/// A timeline's default size in cells (about Excel's 3.3" × 1.5").
const TIMELINE_COLS: u32 = 6;
const TIMELINE_ROWS: u32 = 7;

const NS_XDR: &str = "http://schemas.openxmlformats.org/drawingml/2006/spreadsheetDrawing";
const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const NS_SLE: &str = "http://schemas.microsoft.com/office/drawing/2010/slicer";
const NS_TSLE: &str = "http://schemas.microsoft.com/office/drawing/2012/timeslicer";
const NS_A15: &str = "http://schemas.microsoft.com/office/drawing/2012/main";

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

/// The anchor's object for a timeline, as Excel writes it: the timeline
/// frame for Excel 2013+ (`a15`) and its fallback shape.
pub(crate) fn timeline_object(name: &str) -> String {
    let name = esc(name);
    format!(
        "<mc:AlternateContent xmlns:mc=\"{NS_MC}\"><mc:Choice xmlns:a15=\"{NS_A15}\" Requires=\"a15\"><xdr:graphicFrame xmlns:xdr=\"{NS_XDR}\" xmlns:a=\"{NS_A}\" macro=\"\"><xdr:nvGraphicFramePr><xdr:cNvPr id=\"2\" name=\"{name}\"/><xdr:cNvGraphicFramePr/></xdr:nvGraphicFramePr><xdr:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"0\" cy=\"0\"/></xdr:xfrm><a:graphic><a:graphicData uri=\"{NS_TSLE}\"><tsle:timeslicer xmlns:tsle=\"{NS_TSLE}\" name=\"{name}\"/></a:graphicData></a:graphic></xdr:graphicFrame></mc:Choice><mc:Fallback><xdr:sp xmlns:xdr=\"{NS_XDR}\" xmlns:a=\"{NS_A}\" macro=\"\" textlink=\"\"><xdr:nvSpPr><xdr:cNvPr id=\"3\" name=\"\"/><xdr:cNvSpPr><a:spLocks noTextEdit=\"1\"/></xdr:cNvSpPr></xdr:nvSpPr><xdr:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"3429000\" cy=\"1333500\"/></a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom><a:solidFill><a:prstClr val=\"white\"/></a:solidFill><a:ln w=\"1\"><a:solidFill><a:prstClr val=\"green\"/></a:solidFill></a:ln></xdr:spPr><xdr:txBody><a:bodyPr vertOverflow=\"clip\" horzOverflow=\"clip\"/><a:lstStyle/><a:p><a:r><a:rPr lang=\"en-US\" sz=\"1100\"/><a:t>Timeline: Works in Excel 2013 or higher. Do not move or resize.</a:t></a:r></a:p></xdr:txBody></xdr:sp></mc:Fallback></mc:AlternateContent>"
    )
}

/// A two-cell anchor `cols` × `rows` cells from `(row, col)`, moved in so
/// it fits on the sheet.
fn object_anchor((row, col): (u32, u32), cols: u32, rows: u32) -> Anchor {
    let col = col.min(crate::MAX_COLS - 1 - cols);
    let row = row.min(crate::MAX_ROWS - 1 - rows);
    Anchor::TwoCell {
        from: Marker {
            col,
            col_off: 0,
            row,
            row_off: 0,
        },
        to: Marker {
            col: col + cols,
            col_off: 0,
            row: row + rows,
            row_off: 0,
        },
        edit_as: EditAs::OneCell,
    }
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

    /// The timelines made in Kantor present now (their drawing not
    /// deleted, their sheet still there).
    pub(crate) fn new_timelines(&self) -> Vec<Arc<NewTimeline>> {
        let edits = self.edits();
        edits
            .charts
            .iter()
            .filter(|c| self.sheet_index_of(c.sheet_id).is_some())
            .filter_map(|c| c.shape.as_ref()?.timeline.clone())
            .collect()
    }

    /// Every slicer cache, timeline cache and defined name (lower case):
    /// the names a new cache must not take.
    fn cache_names_taken(&self) -> Vec<String> {
        let file = &self.pivots.slicers;
        file.caches
            .iter()
            .map(|c| c.name.to_lowercase())
            .chain(file.timeline_caches.iter().map(|c| c.name.to_lowercase()))
            .chain(
                self.new_slicers()
                    .iter()
                    .map(|n| n.cache.name.to_lowercase()),
            )
            .chain(
                self.new_timelines()
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
            .collect()
    }

    /// Every slicer's and timeline's name (lower case).
    fn object_names_taken(&self) -> Vec<String> {
        let file = &self.pivots.slicers;
        file.slicers
            .iter()
            .map(|s| s.name.to_lowercase())
            .chain(file.timelines.iter().map(|t| t.name.to_lowercase()))
            .chain(self.new_slicers().iter().map(|n| n.def.name.to_lowercase()))
            .chain(
                self.new_timelines()
                    .iter()
                    .map(|n| n.def.name.to_lowercase()),
            )
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
        let mut slicer_names = self.object_names_taken();
        let mut cache_names = self.cache_names_taken();
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
            .map(|k| object_anchor((row + k, col + k), SLICER_COLS, SLICER_ROWS))
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
                        timeline: None,
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
    /// it, a column apart, level with its top, and right of objects
    /// already there.
    pub fn slicer_place(&self, source: &SlicerSource) -> Option<(u32, (u32, u32))> {
        self.object_place(source, SLICER_COLS, SLICER_ROWS)
    }

    /// Where Insert Timeline puts timelines for pivot table `name` on
    /// sheet `sheet` (as [`Self::slicer_place`] does).
    pub fn timeline_place(&self, sheet: u32, name: &str) -> Option<(u32, (u32, u32))> {
        let source = SlicerSource::Pivot {
            sheet,
            name: name.to_owned(),
        };
        self.object_place(&source, TIMELINE_COLS, TIMELINE_ROWS)
    }

    fn object_place(
        &self,
        source: &SlicerSource,
        cols: u32,
        rows: u32,
    ) -> Option<(u32, (u32, u32))> {
        let (sheet, (row, mut col)) = match source {
            SlicerSource::Pivot { sheet, name } => {
                let p = self
                    .pivot_tables(*sheet)
                    .into_iter()
                    .find(|p| p.name.eq_ignore_ascii_case(name))?;
                (*sheet, (p.bounds.top, p.bounds.right + 2))
            }
            SlicerSource::Table(name) => {
                (0..self.sheet_names().len() as u32).find_map(|sheet| {
                    self.tables(sheet)
                        .into_iter()
                        .find(|t| t.name.eq_ignore_ascii_case(name))
                        .map(|t| (sheet, (t.area.top, t.area.right + 2)))
                })?
            }
        };
        let taken: Vec<(u32, u32, u32, u32)> = self
            .drawings(sheet)
            .iter()
            .filter(|d| !d.hidden)
            .filter_map(|d| match d.anchor {
                Anchor::TwoCell { from, to, .. } => Some((from.row, from.col, to.row, to.col)),
                Anchor::OneCell { from, .. } => Some((from.row, from.col, from.row, from.col)),
                Anchor::Absolute { .. } => None,
            })
            .collect();
        for _ in 0..taken.len() {
            let hit = taken.iter().find(|(r0, c0, r1, c1)| {
                *c0 < col + cols && *c1 >= col && *r0 < row + rows && *r1 >= row
            });
            match hit {
                Some((_, _, _, c1)) => col = c1 + 1,
                None => break,
            }
        }
        Some((sheet, (row, col)))
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

    /// Whether drawing `key` is a timeline (its name).
    pub fn timeline_of(&self, key: DrawingKey) -> Option<String> {
        let sheet = self.sheet_index_of(key.sheet_id)?;
        self.drawings(sheet)
            .into_iter()
            .find(|d| d.key == key)
            .and_then(|d| match &*d.kind {
                DrawingKind::Timeline { name } => Some(name.clone()),
                _ => None,
            })
    }

    // ------------------------------------------------------- timelines

    /// The fields Insert Timeline offers for pivot table `name` on sheet
    /// `sheet`: those holding dates (every value in the source a date), as
    /// Excel lists them.
    pub fn timeline_fields(&self, sheet: u32, name: &str) -> Result<Vec<String>> {
        let (def, cache) = self.pivot_named(sheet, name)?;
        if let Some(why) = cache.unsupported.clone().or(def.unsupported.clone()) {
            bail!("Kantor can't add a timeline to this PivotTable: {why}");
        }
        use crate::pivot::Val;
        let dates = |vals: &mut dyn Iterator<Item = &Val>| {
            let mut any = false;
            for v in vals {
                match v {
                    Val::Date(_) => any = true,
                    Val::Missing => {}
                    _ => return false,
                }
            }
            any
        };
        let source = self.read_source(&cache).ok().map(|(_, _, d)| d);
        let out: Vec<String> = cache
            .fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.database && f.formula.is_none())
            .filter(|(i, f)| {
                if let Some(d) = &source
                    && let Some(k) = d.names.iter().position(|n| n.eq_ignore_ascii_case(&f.name))
                {
                    return dates(&mut d.rows.iter().map(|r| &r[k]));
                }
                if !f.items.is_empty() {
                    return dates(&mut f.items.iter());
                }
                cache
                    .records
                    .as_ref()
                    .is_some_and(|r| dates(&mut r.iter().filter_map(|r| r.get(*i))))
            })
            .map(|(_, f)| f.name.clone())
            .collect();
        if out.is_empty() {
            bail!("The PivotTable has no field of dates for a timeline to filter.");
        }
        Ok(out)
    }

    /// Insert Timeline: one timeline for each of the date `fields` of
    /// pivot table `pivot` on sheet `sheet`, cascading from cell
    /// `(row, col)` as Excel does. Each gets its own cache
    /// (`NativeTimeline_Date`, `NativeTimeline_Date1`...) spanning the
    /// field's years, shown by months and with no dates selected. One undo
    /// step. Returns their drawings' keys.
    pub fn insert_timelines(
        &mut self,
        sheet: u32,
        pivot: &str,
        fields: &[String],
        (row, col): (u32, u32),
    ) -> Result<Vec<DrawingKey>> {
        if fields.is_empty() {
            bail!("pick at least one field");
        }
        let offered = self.timeline_fields(sheet, pivot)?;
        let (def, _) = self.pivot_named(sheet, pivot)?;
        let pivots = vec![(def.sheet_id, def.name.clone())];
        let mut names = self.object_names_taken();
        let mut cache_names = self.cache_names_taken();
        let sheet_id = self.sheet_id(sheet);
        let mut made: Vec<NewTimeline> = Vec::new();
        for f in fields {
            let field = offered
                .iter()
                .find(|o| o.eq_ignore_ascii_case(f))
                .cloned()
                .ok_or_else(|| anyhow!("there is no field of dates {f}"))?;
            let name = unique(&field, " ", |n| names.contains(&n.to_lowercase()));
            names.push(name.to_lowercase());
            let cache_name = unique(
                &cache_base(&field).replacen("Slicer_", "NativeTimeline_", 1),
                "",
                |n| cache_names.contains(&n.to_lowercase()),
            );
            cache_names.push(cache_name.to_lowercase());
            let mut cache = TimelineCache {
                name: cache_name.clone(),
                source_name: field.clone(),
                pivots: pivots.clone(),
                selection: None,
                bounds: None,
            };
            // The years its dates span (Excel's bounds: the end is the day
            // after the last).
            let (lo, hi) = self
                .timeline_bounds(&cache)
                .ok_or_else(|| anyhow!("the field {field} has no dates"))?;
            let year = |d: f64| crate::pivot_calc::ymd(d).0;
            let jan1 = |y: i64| {
                crate::pivot::serial_of_iso(&format!("{y:04}-01-01")).unwrap_or(lo.floor())
            };
            cache.bounds = Some((jan1(year(lo)), jan1(year(hi) + 1)));
            made.push(NewTimeline {
                def: TimelineDef {
                    sheet_id,
                    name,
                    cache: cache_name,
                    caption: field.clone(),
                    level: 2,
                    show_header: true,
                    show_selection_label: true,
                    show_time_level: true,
                    show_scrollbar: true,
                    scroll: Some(jan1(year(lo))),
                    style: None,
                },
                cache,
            });
        }
        let anchors: Vec<Anchor> = (0..made.len() as u32)
            .map(|k| object_anchor((row + k, col + k), TIMELINE_COLS, TIMELINE_ROWS))
            .collect();
        let mut keys = Vec::new();
        self.own_edit(|e, layout| {
            for (n, anchor) in made.into_iter().zip(anchors) {
                let id = e.charts.iter().map(|c| c.id).max().map_or(1, |m| m + 1);
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
                        xml: timeline_object(&name).into(),
                        name,
                        slicer: None,
                        timeline: Some(Arc::new(n)),
                    })),
                });
                keys.push(DrawingKey {
                    sheet_id,
                    index: NEW_CHART_BASE + id as usize,
                });
            }
        });
        Ok(keys)
    }

    /// Timeline `name`'s settings now.
    pub fn timeline_settings(&self, sheet_id: u32, name: &str) -> Option<TimelineSettings> {
        let t = self.timeline_def(sheet_id, name)?;
        let c = self.timeline_cache(&t.cache);
        Some(TimelineSettings {
            name: t.name.clone(),
            source_name: c.map(|c| c.source_name).unwrap_or_default(),
            cache_name: t.cache.clone(),
            caption: t.caption.clone(),
            show_header: t.show_header,
            show_selection_label: t.show_selection_label,
            show_time_level: t.show_time_level,
            show_scrollbar: t.show_scrollbar,
            style: t
                .style
                .clone()
                .unwrap_or_else(|| "TimeSlicerStyleLight1".to_owned()),
        })
    }

    /// Change timeline `name`'s settings to `s` (its name, field and cache
    /// stay). One undo step.
    pub fn set_timeline_settings(
        &mut self,
        sheet_id: u32,
        name: &str,
        s: &TimelineSettings,
    ) -> Result<()> {
        let mut t = self
            .timeline_def(sheet_id, name)
            .ok_or_else(|| anyhow!("there is no timeline {name}"))?;
        if self.timeline_settings(sheet_id, name).as_ref() == Some(s) {
            return Ok(());
        }
        t.caption = s.caption.trim().to_owned();
        t.show_header = s.show_header;
        t.show_selection_label = s.show_selection_label;
        t.show_time_level = s.show_time_level;
        t.show_scrollbar = s.show_scrollbar;
        t.style =
            Some(s.style.clone()).filter(|st| st != "TimeSlicerStyleLight1" || t.style.is_some());
        let key = (sheet_id, t.name.to_lowercase());
        self.own_edit(|e, _| {
            e.timeline_defs.insert(key, t);
        });
        Ok(())
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
        let new_t = self.new_timelines();
        if file.caches.is_empty()
            && file.timeline_caches.is_empty()
            && new.is_empty()
            && new_t.is_empty()
        {
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
        // Timelines: the file's not deleted (on sheets still there), then
        // Kantor's.
        let mut tdefs: Vec<crate::slicer::TimelineDef> = Vec::new();
        for d in &file.timelines {
            let Some(sheet) = self.sheet_index_of(d.sheet_id) else {
                continue;
            };
            let opened = self.drawings.opened_timelines(d.sheet_id);
            let shown = self.drawings(sheet).iter().any(|x| {
                matches!(&*x.kind, DrawingKind::Timeline { name } if name.eq_ignore_ascii_case(&d.name))
            });
            if opened.contains(&d.name.to_lowercase()) && !shown {
                continue;
            }
            tdefs.push(
                self.timeline_def(d.sheet_id, &d.name)
                    .unwrap_or_else(|| d.clone()),
            );
        }
        for n in &new_t {
            tdefs.push(
                self.timeline_def(n.def.sheet_id, &n.def.name)
                    .unwrap_or_else(|| n.def.clone()),
            );
        }
        plan.timelines = tdefs
            .into_iter()
            .map(|t| TimelineOut {
                sheet_id: t.sheet_id,
                name: t.name,
                cache: t.cache,
                caption: t.caption,
                level: t.level,
                scroll: t.scroll,
                show_header: t.show_header,
                show_selection_label: t.show_selection_label,
                show_time_level: t.show_time_level,
                show_scrollbar: t.show_scrollbar,
                style: t.style,
            })
            .collect();
        let caches = file
            .timeline_caches
            .iter()
            .cloned()
            .chain(new_t.iter().map(|n| n.cache.clone()));
        for c in caches {
            let conn = self.timeline_conn(&c);
            let filter = conn.as_ref().ok().and_then(|(all, field)| {
                let state = self.pivot_state();
                let def = self.pivot_def(&state, all[0]);
                def.filters
                    .iter()
                    .position(|f| f.fld == *field && f.kind == "dateBetween")
                    .map(|k| TimelineFilter {
                        id: k + 1,
                        tab: def.sheet_id,
                        pivot: def.name.clone(),
                    })
            });
            plan.timeline_caches.push(TimelineCacheOut {
                selection: self.timeline_selection(&c),
                known: conn.is_ok(),
                name: c.name,
                source_name: c.source_name,
                pivots: c.pivots,
                bounds: c.bounds,
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
