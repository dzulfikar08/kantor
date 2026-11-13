//! Insert Chart: charts made in Kantor from a range of cells.
//!
//! A new chart lives in the history state's [`crate::edits::Edits`] (its
//! [`ChartSpec`] and anchor, in the rows and columns of the state it was
//! placed in, so it follows later moves). [`Workbook::drawings`] lists it
//! with the file's objects, as a [`crate::drawing::DrawingKind::Chart`] whose
//! part is generated here ([`chart_xml`]: `c:chartSpace` with `c:f`
//! references and cached values, read live by the renderer like a file's
//! chart). Moving, resizing and deleting go through the usual
//! [`Workbook::place_drawing`] / [`Workbook::delete_drawing`]; the Chart
//! panel changes the spec ([`Workbook::set_chart`]). A save adds the chart
//! part and a drawing part (or an anchor in the sheet's existing drawing),
//! see `preserve::write_charts`.

use std::sync::Arc;

use anyhow::{Result, anyhow, bail};

pub use crate::chart_style::SeriesColor;
use crate::chart_style::{self, NewSeries};
use crate::drawing::{Anchor, Drawing, DrawingKey, DrawingKind, EditAs, Marker, RefValue};
use crate::shift::Layout;
use crate::{CellRange, MAX_COLS, MAX_ROWS, Workbook};

/// Keys of charts made in Kantor start here (objects of the file are
/// numbered from 0 in their drawing).
pub const NEW_CHART_BASE: usize = 1 << 30;
/// Part names of charts made in Kantor, until saved.
pub const NEW_CHART_PART: &str = "kantor-new-chart/";

/// The chart types Insert Chart offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartType {
    Column,
    StackedColumn,
    Bar,
    StackedBar,
    Line,
    StackedLine,
    Pie,
    Area,
    StackedArea,
    Scatter,
    /// A chart from a file whose type isn't one of these (combo, 3-D,
    /// doughnut, 100% stacked...): see [`ChartInfo::kind_label`].
    Other,
}

impl ChartType {
    pub const ALL: [ChartType; 10] = [
        ChartType::Column,
        ChartType::StackedColumn,
        ChartType::Bar,
        ChartType::StackedBar,
        ChartType::Line,
        ChartType::StackedLine,
        ChartType::Pie,
        ChartType::Area,
        ChartType::StackedArea,
        ChartType::Scatter,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ChartType::Column => "Column",
            ChartType::StackedColumn => "Stacked Column",
            ChartType::Bar => "Bar",
            ChartType::StackedBar => "Stacked Bar",
            ChartType::Line => "Line",
            ChartType::StackedLine => "Stacked Line",
            ChartType::Pie => "Pie",
            ChartType::Area => "Area",
            ChartType::StackedArea => "Stacked Area",
            ChartType::Scatter => "Scatter",
            ChartType::Other => "Other",
        }
    }

    fn stacked(self) -> bool {
        matches!(
            self,
            ChartType::StackedColumn
                | ChartType::StackedBar
                | ChartType::StackedLine
                | ChartType::StackedArea
        )
    }
}

/// Where the legend goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegendPos {
    None,
    Right,
    Top,
    Bottom,
    Left,
}

impl LegendPos {
    pub const ALL: [LegendPos; 5] = [
        LegendPos::Right,
        LegendPos::Bottom,
        LegendPos::Top,
        LegendPos::Left,
        LegendPos::None,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LegendPos::None => "None",
            LegendPos::Right => "Right",
            LegendPos::Top => "Top",
            LegendPos::Bottom => "Bottom",
            LegendPos::Left => "Left",
        }
    }

    fn code(self) -> Option<&'static str> {
        match self {
            LegendPos::None => None,
            LegendPos::Right => Some("r"),
            LegendPos::Top => Some("t"),
            LegendPos::Bottom => Some("b"),
            LegendPos::Left => Some("l"),
        }
    }
}

/// What a chart shows, as the Chart panel edits it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartSpec {
    pub kind: ChartType,
    /// Empty: no title.
    pub title: String,
    pub legend: LegendPos,
    /// Series are the data's rows (else its columns).
    pub by_rows: bool,
    /// The data, in the rows and columns of the state the spec was set in.
    pub data: CellRange,
    /// The data's sheet (`sheetId`); `None`: the chart's own sheet.
    pub data_sheet: Option<u32>,
    /// Titles of the horizontal and vertical axes (empty: none).
    pub x_title: String,
    pub y_title: String,
    /// Each series' colour, in series order (missing: automatic).
    pub colors: Vec<SeriesColor>,
}

/// A chart for the Chart panel: its spec and what can't be changed.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartInfo {
    pub spec: ChartSpec,
    /// It came with the file (its part is changed in place).
    pub from_file: bool,
    /// The type's name (for [`ChartType::Other`]: "Combo", "3-D Column"...).
    pub kind_label: String,
    /// Its type and data range can't be changed in Kantor (several plot
    /// types, 3-D, radar...).
    pub fixed: bool,
    /// The data range isn't one block Kantor can read back (series on
    /// several sheets, literal values...): changing it replaces the series.
    pub data_unknown: bool,
    /// It has axes (titles apply).
    pub has_axes: bool,
    /// Series names, in order (for the colours).
    pub series: Vec<String>,
}

/// A chart made in Kantor.
#[derive(Debug, Clone)]
pub(crate) struct NewChart {
    pub id: u32,
    pub sheet_id: u32,
    pub spec: ChartSpec,
    pub anchor: Anchor,
    /// The layout `spec.data` and `anchor` are in.
    pub layout: Layout,
}

/// One series: name cell, category cells, value cells (0-based ranges).
struct Series {
    name: Option<CellRange>,
    cats: Option<CellRange>,
    vals: CellRange,
}

fn single(r: u32, c: u32) -> CellRange {
    CellRange::from_corners((r, c), (r, c))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn abs_ref(sheet: &str, r: &CellRange) -> String {
    let q = crate::print::quote_sheet(sheet);
    let a = format!("${}${}", crate::column_name(r.left), r.top + 1);
    if r.is_single() {
        format!("{q}!{a}")
    } else {
        format!("{q}!{a}:${}${}", crate::column_name(r.right), r.bottom + 1)
    }
}

impl Workbook {
    /// Whether the first row / first column of `data` holds labels (text
    /// or blanks where the rest has numbers).
    fn chart_labels(&self, sheet: u32, data: CellRange) -> (bool, bool) {
        let is_label = |r: u32, c: u32| self.raw_number(sheet, r, c).is_none();
        let row_labels = data.bottom > data.top
            && (data.left..=data.right).all(|c| is_label(data.top, c))
            && (data.left..=data.right).any(|c| !self.display_value(sheet, data.top, c).is_empty());
        let col_labels = data.right > data.left
            && (data.top + u32::from(row_labels)..=data.bottom).all(|r| is_label(r, data.left));
        (row_labels, col_labels)
    }

    /// The series of `spec` on `sheet`.
    fn chart_series(&self, sheet: u32, spec: &ChartSpec) -> Vec<Series> {
        let d = spec.data;
        let (row_labels, col_labels) = self.chart_labels(sheet, d);
        let top = d.top + u32::from(row_labels);
        let left = d.left + u32::from(col_labels);
        let mut out = Vec::new();
        if spec.kind == ChartType::Scatter {
            // First column (or row) are the X values.
            if spec.by_rows {
                let xs = (top..=d.bottom)
                    .next()
                    .map(|r| CellRange::from_corners((r, left), (r, d.right)));
                for r in top + 1..=d.bottom {
                    out.push(Series {
                        name: col_labels.then(|| single(r, d.left)),
                        cats: xs,
                        vals: CellRange::from_corners((r, left), (r, d.right)),
                    });
                }
            } else {
                let xs = Some(CellRange::from_corners((top, left), (d.bottom, left)));
                for c in left + 1..=d.right {
                    out.push(Series {
                        name: row_labels.then(|| single(d.top, c)),
                        cats: xs,
                        vals: CellRange::from_corners((top, c), (d.bottom, c)),
                    });
                }
            }
            if !out.is_empty() {
                return out;
            }
        }
        if spec.by_rows {
            let cats = row_labels.then(|| CellRange::from_corners((d.top, left), (d.top, d.right)));
            for r in top..=d.bottom {
                out.push(Series {
                    name: col_labels.then(|| single(r, d.left)),
                    cats,
                    vals: CellRange::from_corners((r, left), (r, d.right)),
                });
            }
        } else {
            let cats =
                col_labels.then(|| CellRange::from_corners((top, d.left), (d.bottom, d.left)));
            for c in left..=d.right {
                out.push(Series {
                    name: row_labels.then(|| single(d.top, c)),
                    cats,
                    vals: CellRange::from_corners((top, c), (d.bottom, c)),
                });
            }
        }
        out
    }

    /// Insert Chart: a chart of `kind` from `data` on `sheet` (the
    /// selection, else the data around the active cell), placed beside the
    /// data. One undo step. Returns its key.
    pub fn insert_chart(
        &mut self,
        sheet: u32,
        data: CellRange,
        kind: ChartType,
    ) -> Result<DrawingKey> {
        if data.is_whole_columns() || data.is_whole_rows() {
            bail!("Select the chart's cells, not whole rows or columns");
        }
        // Empty rows and columns at the edges don't make series.
        let mut data = data;
        let filled_row = |wb: &Self, r: u32, d: &CellRange| {
            (d.left..=d.right).any(|c| wb.is_filled(sheet, r, c))
        };
        let filled_col = |wb: &Self, c: u32, d: &CellRange| {
            (d.top..=d.bottom).any(|r| wb.is_filled(sheet, r, c))
        };
        while data.bottom > data.top && !filled_row(self, data.bottom, &data) {
            data.bottom -= 1;
        }
        while data.right > data.left && !filled_col(self, data.right, &data) {
            data.right -= 1;
        }
        while data.top < data.bottom && !filled_row(self, data.top, &data) {
            data.top += 1;
        }
        while data.left < data.right && !filled_col(self, data.left, &data) {
            data.left += 1;
        }
        if (data.bottom - data.top + 1) as u64 * (data.right - data.left + 1) as u64 > 50_000 {
            bail!("That's too many cells for a chart");
        }
        let has_number = (data.top..=data.bottom)
            .any(|r| (data.left..=data.right).any(|c| self.raw_number(sheet, r, c).is_some()));
        if !has_number {
            bail!("The selected cells have no numbers to chart");
        }
        let (row_labels, col_labels) = self.chart_labels(sheet, data);
        let rows = data.bottom - data.top + 1 - u32::from(row_labels);
        let cols = data.right - data.left + 1 - u32::from(col_labels);
        // Excel: series in columns when there are at least as many rows.
        let by_rows = cols > rows;
        let spec = ChartSpec {
            kind,
            title: String::new(),
            legend: LegendPos::Right,
            by_rows,
            data,
            data_sheet: None,
            x_title: String::new(),
            y_title: String::new(),
            colors: Vec::new(),
        };
        // Beside the data: two columns to its right, 8 columns × 15 rows.
        let col = (data.right + 2).min(MAX_COLS - 9);
        let row = data.top.min(MAX_ROWS - 16);
        let anchor = Anchor::TwoCell {
            from: Marker {
                col,
                col_off: 0,
                row,
                row_off: 0,
            },
            to: Marker {
                col: col + 8,
                col_off: 0,
                row: row + 15,
                row_off: 0,
            },
            edit_as: EditAs::OneCell,
        };
        let sheet_id = self.sheet_id(sheet);
        let mut id = 0;
        self.own_edit(|e, layout| {
            id = e.charts.iter().map(|c| c.id).max().map_or(1, |m| m + 1);
            e.charts.push(NewChart {
                id,
                sheet_id,
                spec,
                anchor,
                layout: layout.clone(),
            });
        });
        Ok(DrawingKey {
            sheet_id,
            index: NEW_CHART_BASE + id as usize,
        })
    }

    /// The spec of chart `key` (made in Kantor) where its data is now.
    pub fn chart_spec(&self, key: DrawingKey) -> Option<ChartSpec> {
        let edits = self.edits();
        let c = edits
            .charts
            .iter()
            .find(|c| key.index == NEW_CHART_BASE + c.id as usize && c.sheet_id == key.sheet_id)?;
        Some(self.chart_now(c).0)
    }

    /// What the Chart panel shows for chart `key`: made in Kantor, or one
    /// that came with the file (`None`: not a chart Kantor can change).
    pub fn chart_info(&self, key: DrawingKey) -> Option<ChartInfo> {
        if key.index < NEW_CHART_BASE {
            return self.file_chart_info(key);
        }
        let spec = self.chart_spec(key)?;
        let sheet = self.sheet_index_of(key.sheet_id)?;
        let style = chart_style::read(&self.chart_xml(sheet, &spec)).ok()?;
        Some(ChartInfo {
            from_file: false,
            kind_label: spec.kind.label().to_owned(),
            fixed: false,
            data_unknown: false,
            has_axes: spec.kind != ChartType::Pie,
            series: style.names,
            spec,
        })
    }

    pub(crate) fn sheet_index_of(&self, sheet_id: u32) -> Option<u32> {
        (0..self.sheet_names().len() as u32).find(|&s| self.sheet_id(s) == sheet_id)
    }

    /// Whether `spec`'s data has a number to chart.
    pub(crate) fn check_chart_data(&self, sheet: u32, spec: &ChartSpec) -> Result<()> {
        let data_sheet = self.data_sheet_index(sheet, spec);
        let d = spec.data;
        if (d.bottom - d.top + 1) as u64 * (d.right - d.left + 1) as u64 > 50_000 {
            bail!("That's too many cells for a chart");
        }
        let has_number = (d.top..=d.bottom)
            .any(|r| (d.left..=d.right).any(|c| self.raw_number(data_sheet, r, c).is_some()));
        if !has_number {
            bail!("The data range has no numbers to chart");
        }
        Ok(())
    }

    /// Change chart `key` (made in Kantor or from the file). One undo step.
    pub fn set_chart(&mut self, key: DrawingKey, spec: ChartSpec) -> Result<()> {
        if key.index < NEW_CHART_BASE {
            return self.set_file_chart(key, spec);
        }
        if self.chart_spec(key).as_ref() == Some(&spec) {
            return Ok(());
        }
        if spec.kind == ChartType::Other {
            bail!("Pick a chart type");
        }
        let sheet = self
            .sheet_index_of(key.sheet_id)
            .ok_or_else(|| anyhow!("no sheet"))?;
        self.check_chart_data(sheet, &spec)?;
        let (_, anchor_now) = {
            let edits = self.edits();
            let c = edits
                .charts
                .iter()
                .find(|c| key.index == NEW_CHART_BASE + c.id as usize)
                .ok_or_else(|| anyhow!("this chart can't be changed in Kantor"))?;
            self.chart_now(c)
        };
        self.own_edit(|e, layout| {
            if let Some(c) = e
                .charts
                .iter_mut()
                .find(|c| key.index == NEW_CHART_BASE + c.id as usize)
            {
                c.spec = spec;
                c.anchor = anchor_now;
                c.layout = layout.clone();
            }
        });
        Ok(())
    }

    /// `c`'s spec and anchor in the current rows and columns.
    fn chart_now(&self, c: &NewChart) -> (ChartSpec, Anchor) {
        let moves = c.layout.moves_to(&self.history.layout());
        let mut spec = c.spec.clone();
        let data_id = spec.data_sheet.unwrap_or(c.sheet_id);
        if moves.touches(data_id) {
            let d = spec.data;
            if let Some(a) = moves.area(
                data_id,
                crate::shift::Area {
                    top: d.top,
                    left: d.left,
                    bottom: d.bottom,
                    right: d.right,
                },
            ) {
                spec.data = CellRange::from_corners((a.top, a.left), (a.bottom, a.right));
            }
        }
        (spec, c.anchor.moved(&moves, c.sheet_id))
    }

    /// Move, resize or delete a chart made in Kantor (see
    /// [`Workbook::place_drawing`]).
    pub(crate) fn new_chart_edit(&mut self, key: DrawingKey, anchor: Option<Anchor>) -> Result<()> {
        let found = self
            .edits()
            .charts
            .iter()
            .any(|c| key.index == NEW_CHART_BASE + c.id as usize && c.sheet_id == key.sheet_id);
        if !found {
            bail!("this object can't be edited");
        }
        let current = {
            let edits = self.edits();
            let c = edits
                .charts
                .iter()
                .find(|c| key.index == NEW_CHART_BASE + c.id as usize)
                .cloned();
            c.map(|c| self.chart_now(&c).0)
        };
        self.own_edit(|e, layout| match anchor {
            None => e
                .charts
                .retain(|c| key.index != NEW_CHART_BASE + c.id as usize),
            Some(a) => {
                if let Some(c) = e
                    .charts
                    .iter_mut()
                    .find(|c| key.index == NEW_CHART_BASE + c.id as usize)
                {
                    c.anchor = a;
                    if let Some(spec) = current.clone() {
                        c.spec = spec;
                    }
                    c.layout = layout.clone();
                }
            }
        });
        Ok(())
    }

    /// The charts made in Kantor on `sheet_id`, as drawings.
    pub(crate) fn new_chart_drawings(&self, sheet_id: u32) -> Vec<Drawing> {
        let edits = self.edits();
        let Some(sheet) =
            (0..self.sheet_names().len() as u32).find(|&s| self.sheet_id(s) == sheet_id)
        else {
            return Vec::new();
        };
        edits
            .charts
            .iter()
            .filter(|c| c.sheet_id == sheet_id)
            .map(|c| {
                let (spec, anchor) = self.chart_now(c);
                let xml = self.chart_xml(sheet, &spec);
                let part = format!("{NEW_CHART_PART}{}/{:x}", c.id, fxhash(&xml));
                Drawing {
                    key: DrawingKey {
                        sheet_id,
                        index: NEW_CHART_BASE + c.id as usize,
                    },
                    name: format!("Chart {}", c.id),
                    descr: String::new(),
                    anchor,
                    kind: Arc::new(DrawingKind::Chart {
                        part: part.into(),
                        xml: xml.into(),
                        current_refs: true,
                    }),
                    editable: true,
                    hidden: false,
                    prints: true,
                }
            })
            .collect()
    }

    /// The sheet index of `spec`'s data (`sheet`: the chart's own).
    pub(crate) fn data_sheet_index(&self, sheet: u32, spec: &ChartSpec) -> u32 {
        spec.data_sheet
            .and_then(|id| (0..self.sheet_names().len() as u32).find(|&s| self.sheet_id(s) == id))
            .unwrap_or(sheet)
    }

    /// The series of `spec` (data on sheet `data_sheet`) as `c:tx`, `c:cat`
    /// and `c:val` content: references with the values they hold now.
    pub(crate) fn series_xml(&self, data_sheet: u32, spec: &ChartSpec) -> Vec<NewSeries> {
        let name = self
            .sheet_names()
            .get(data_sheet as usize)
            .cloned()
            .unwrap_or_default();
        let series = self.chart_series(data_sheet, spec);
        let reference = |r: &CellRange| self.chart_reference_now(&abs_ref(&name, r));
        let str_ref = |r: &CellRange| -> String {
            let data = reference(r).unwrap_or_default();
            let mut x = format!(
                "<c:strRef><c:f>{}</c:f><c:strCache><c:ptCount val=\"{}\"/>",
                esc(&abs_ref(&name, r)),
                data.shown.len()
            );
            for (i, s) in data.shown.iter().enumerate() {
                x.push_str(&format!("<c:pt idx=\"{i}\"><c:v>{}</c:v></c:pt>", esc(s)));
            }
            x.push_str("</c:strCache></c:strRef>");
            x
        };
        let num_ref = |r: &CellRange| -> String {
            let data = reference(r).unwrap_or_default();
            let fmt = data.format.clone().unwrap_or_else(|| "General".into());
            let mut x = format!(
                "<c:numRef><c:f>{}</c:f><c:numCache><c:formatCode>{}</c:formatCode><c:ptCount val=\"{}\"/>",
                esc(&abs_ref(&name, r)),
                esc(&fmt),
                data.values.len()
            );
            for (i, v) in data.values.iter().enumerate() {
                if let RefValue::Number(n) = v {
                    x.push_str(&format!("<c:pt idx=\"{i}\"><c:v>{n}</c:v></c:pt>"));
                }
            }
            x.push_str("</c:numCache></c:numRef>");
            x
        };
        let cats_numeric = |r: &CellRange| {
            reference(r).is_some_and(|d| {
                d.values.iter().any(|v| matches!(v, RefValue::Number(_)))
                    && d.values.iter().all(|v| !matches!(v, RefValue::Text(_)))
            })
        };
        let scatter = spec.kind == ChartType::Scatter;
        series
            .iter()
            .map(|s| NewSeries {
                tx: s.name.as_ref().map(str_ref),
                cat: s.cats.as_ref().map(|c| {
                    if scatter || cats_numeric(c) {
                        num_ref(c)
                    } else {
                        str_ref(c)
                    }
                }),
                val: num_ref(&s.vals),
            })
            .collect()
    }

    /// The `c:chartSpace` part for `spec` on `sheet`, with its references
    /// and the values they hold now.
    pub(crate) fn chart_xml(&self, sheet: u32, spec: &ChartSpec) -> String {
        let data_sheet = self.data_sheet_index(sheet, spec);
        let series = self.series_xml(data_sheet, spec);
        let kind = spec.kind;
        let mut sers = String::new();
        let take = if kind == ChartType::Pie {
            1
        } else {
            usize::MAX
        };
        for (i, s) in series.iter().take(take).enumerate() {
            sers.push_str(&format!(
                "<c:ser><c:idx val=\"{i}\"/><c:order val=\"{i}\"/>"
            ));
            if let Some(n) = &s.tx {
                sers.push_str(&format!("<c:tx>{n}</c:tx>"));
            }
            match kind {
                ChartType::Scatter => {
                    sers.push_str(
                        "<c:spPr><a:ln w=\"19050\" cap=\"rnd\"><a:noFill/><a:round/></a:ln></c:spPr><c:marker><c:symbol val=\"circle\"/><c:size val=\"5\"/></c:marker>",
                    );
                    if let Some(x) = &s.cat {
                        sers.push_str(&format!("<c:xVal>{x}</c:xVal>"));
                    }
                    sers.push_str(&format!("<c:yVal>{}</c:yVal><c:smooth val=\"0\"/>", s.val));
                }
                _ => {
                    match kind {
                        ChartType::Column
                        | ChartType::StackedColumn
                        | ChartType::Bar
                        | ChartType::StackedBar => {
                            sers.push_str("<c:invertIfNegative val=\"0\"/>");
                        }
                        ChartType::Line | ChartType::StackedLine => {
                            sers.push_str("<c:marker><c:symbol val=\"none\"/></c:marker>");
                        }
                        _ => {}
                    }
                    if let Some(c) = &s.cat {
                        sers.push_str(&format!("<c:cat>{c}</c:cat>"));
                    }
                    sers.push_str(&format!("<c:val>{}</c:val>", s.val));
                    if matches!(kind, ChartType::Line | ChartType::StackedLine) {
                        sers.push_str("<c:smooth val=\"0\"/>");
                    }
                }
            }
            sers.push_str("</c:ser>");
        }
        let grouping = if kind.stacked() {
            "stacked"
        } else {
            "standard"
        };
        let axes_ids = "<c:axId val=\"500000001\"/><c:axId val=\"500000002\"/>";
        let plot = match kind {
            ChartType::Column
            | ChartType::StackedColumn
            | ChartType::Bar
            | ChartType::StackedBar => {
                let dir = if matches!(kind, ChartType::Bar | ChartType::StackedBar) {
                    "bar"
                } else {
                    "col"
                };
                let (g, overlap) = if kind.stacked() {
                    ("stacked", "<c:overlap val=\"100\"/>")
                } else {
                    ("clustered", "")
                };
                format!(
                    "<c:barChart><c:barDir val=\"{dir}\"/><c:grouping val=\"{g}\"/><c:varyColors val=\"0\"/>{sers}<c:gapWidth val=\"150\"/>{overlap}{axes_ids}</c:barChart>"
                )
            }
            ChartType::Line | ChartType::StackedLine => format!(
                "<c:lineChart><c:grouping val=\"{grouping}\"/><c:varyColors val=\"0\"/>{sers}<c:marker val=\"1\"/>{axes_ids}</c:lineChart>"
            ),
            ChartType::Area | ChartType::StackedArea => format!(
                "<c:areaChart><c:grouping val=\"{grouping}\"/><c:varyColors val=\"0\"/>{sers}{axes_ids}</c:areaChart>"
            ),
            ChartType::Pie => format!(
                "<c:pieChart><c:varyColors val=\"1\"/>{sers}<c:firstSliceAng val=\"0\"/></c:pieChart>"
            ),
            ChartType::Scatter => format!(
                "<c:scatterChart><c:scatterStyle val=\"lineMarker\"/><c:varyColors val=\"0\"/>{sers}{axes_ids}</c:scatterChart>"
            ),
            ChartType::Other => unreachable!("Other is never set on a chart made in Kantor"),
        };
        let horizontal = matches!(kind, ChartType::Bar | ChartType::StackedBar);
        let (cat_pos, val_pos) = if horizontal { ("l", "b") } else { ("b", "l") };
        let axis_tail = "<c:majorTickMark val=\"out\"/><c:minorTickMark val=\"none\"/><c:tickLblPos val=\"nextTo\"/>";
        let axes = match kind {
            ChartType::Pie => String::new(),
            ChartType::Scatter => format!(
                "<c:valAx><c:axId val=\"500000001\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"b\"/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>{axis_tail}<c:crossAx val=\"500000002\"/><c:crosses val=\"autoZero\"/><c:crossBetween val=\"midCat\"/></c:valAx>\
<c:valAx><c:axId val=\"500000002\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"l\"/><c:majorGridlines/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>{axis_tail}<c:crossAx val=\"500000001\"/><c:crosses val=\"autoZero\"/><c:crossBetween val=\"midCat\"/></c:valAx>"
            ),
            _ => format!(
                "<c:catAx><c:axId val=\"500000001\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"{cat_pos}\"/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>{axis_tail}<c:crossAx val=\"500000002\"/><c:crosses val=\"autoZero\"/><c:auto val=\"1\"/><c:lblAlgn val=\"ctr\"/><c:lblOffset val=\"100\"/><c:noMultiLvlLbl val=\"0\"/></c:catAx>\
<c:valAx><c:axId val=\"500000002\"/><c:scaling><c:orientation val=\"minMax\"/></c:scaling><c:delete val=\"0\"/><c:axPos val=\"{val_pos}\"/><c:majorGridlines/><c:numFmt formatCode=\"General\" sourceLinked=\"1\"/>{axis_tail}<c:crossAx val=\"500000001\"/><c:crosses val=\"autoZero\"/><c:crossBetween val=\"between\"/></c:valAx>"
            ),
        };
        let title = if spec.title.trim().is_empty() {
            "<c:autoTitleDeleted val=\"1\"/>".to_owned()
        } else {
            format!(
                "<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr/></a:pPr><a:r><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val=\"0\"/></c:title><c:autoTitleDeleted val=\"0\"/>",
                esc(spec.title.trim())
            )
        };
        let legend = match spec.legend.code() {
            Some(p) => {
                format!("<c:legend><c:legendPos val=\"{p}\"/><c:overlay val=\"0\"/></c:legend>")
            }
            None => String::new(),
        };
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<c:chartSpace xmlns:c=\"http://schemas.openxmlformats.org/drawingml/2006/chart\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\">\
<c:roundedCorners val=\"0\"/><c:chart>{title}<c:plotArea><c:layout/>{plot}{axes}</c:plotArea>{legend}<c:plotVisOnly val=\"1\"/><c:dispBlanksAs val=\"gap\"/></c:chart></c:chartSpace>"
        );
        styled(xml, spec)
    }

    /// Charts made in Kantor that a save adds (or updates or removes where
    /// an earlier save of this session put them).
    pub(crate) fn chart_patches(&self, out: &mut crate::preserve::Patches) {
        let edits = self.edits();
        for (id, s) in &self.saved_charts {
            if !edits.charts.iter().any(|c| c.id == *id) {
                out.removed_charts.push(s.clone());
            }
        }
        for c in &edits.charts {
            let Some(sheet) =
                (0..self.sheet_names().len() as u32).find(|&s| self.sheet_id(s) == c.sheet_id)
            else {
                continue;
            };
            let (spec, anchor) = self.chart_now(c);
            out.charts.push(crate::preserve::ChartSave {
                id: c.id,
                saved: self.saved_charts.get(&c.id).cloned(),
                sheet_id: c.sheet_id,
                name: format!("Chart {}", c.id),
                anchor,
                xml: self.chart_xml(sheet, &spec),
            });
        }
    }
}

/// `xml` with `spec`'s axis titles and series colours (a chart made in
/// Kantor: the generated part has neither).
fn styled(xml: String, spec: &ChartSpec) -> String {
    let mut xml = xml;
    if spec.kind != ChartType::Pie {
        for (horizontal, text) in [(true, &spec.x_title), (false, &spec.y_title)] {
            if let Ok(x) = chart_style::set_axis_title(&xml, horizontal, text) {
                xml = x;
            }
        }
    }
    for (i, c) in spec.colors.iter().enumerate() {
        if let SeriesColor::Rgb(_) = c
            && let Ok(x) = chart_style::set_color(&xml, i, *c)
        {
            xml = x;
        }
    }
    xml
}

pub(crate) fn fxhash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references() {
        assert_eq!(
            abs_ref("Data", &CellRange::from_corners((1, 1), (12, 1))),
            "Data!$B$2:$B$13"
        );
        assert_eq!(abs_ref("My Data", &single(0, 0)), "'My Data'!$A$1");
    }
}
