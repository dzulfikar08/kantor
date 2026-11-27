//! Charts: `Worksheet.ChartObjects`, `ChartObject`, `Chart` (type, source
//! data, title, legend, axes, series, `Export`, `Location`), `Series` and
//! `SeriesCollection`, `Workbook.Charts` (chart sheets), `Shapes.AddChart2`
//! / `AddChart` and `ActiveChart`.
//!
//! Objects keep the chart's [`DrawingKey`] (`sheetId` and its place, stable
//! while the macro runs) and look the chart up on every access. Changes go
//! through the Chart panel's edit ([`Workbook::set_chart`]) or, for series
//! changed one by one, `Workbook::script_set_series`; both are undo steps
//! the run folds into one, and both are saved like edits made by hand.
//!
//! Approximations (documented in `docs/scripting.md`):
//! - `xlLineMarkers` / `xlLineMarkersStacked` make line charts without
//!   markers (Kantor's line charts have none); `ChartType` then reads
//!   `xlLine` / `xlLineStacked`.
//! - `ChartTitle`, `Legend` and `AxisTitle` can be used when the chart has
//!   none: setting their text or position adds them (Excel fails).
//! - `MinimumScale` / `MaximumScale` / `MajorUnit` of an automatic axis
//!   read Excel's rule worked out from the data (zero included, about 5 %
//!   headroom, a 1-2-5 unit), which can differ from what Excel shows.
//! - `ActiveChart` is the chart last added, selected or activated by the
//!   macro; selecting cells doesn't clear it.

use std::any::Any;
use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use kantor_script::{Arg, Array, Dispatch, ScriptError, Value};
use sheets_engine::drawing::{
    Anchor, Drawing, DrawingKey, DrawingKind, EditAs, Marker, RefValue, SheetGeometry,
    chart_sheet_key_id, chart_sheet_of_key,
};
use sheets_engine::script_charts::{ScriptSeries, SeriesData};
use sheets_engine::{
    CellRange, ChartInfo, ChartPlace, ChartSpec, ChartType, LegendPos, MAX_COLS, MAX_ROWS,
    MovedChart, Tab, Workbook,
};

use crate::range::range_of;
use crate::session::Session;
use crate::sheet::SheetObj;
use crate::values::*;

// ------------------------------------------------------------------ constants

pub const XL_COLUMN_CLUSTERED: i64 = 51;
pub const XL_CATEGORY: i64 = 1;
pub const XL_VALUE: i64 = 2;
pub const XL_SERIES_AXIS: i64 = 3;
pub const XL_PRIMARY: i64 = 1;
pub const XL_SECONDARY: i64 = 2;
pub const XL_ROWS: i64 = 1;
pub const XL_COLUMNS: i64 = 2;
pub const XL_LEGEND_POSITION_BOTTOM: i64 = -4107;
pub const XL_LEGEND_POSITION_TOP: i64 = -4160;
pub const XL_LEGEND_POSITION_LEFT: i64 = -4131;
pub const XL_LEGEND_POSITION_RIGHT: i64 = -4152;
pub const XL_LEGEND_POSITION_CORNER: i64 = 2;
pub const XL_LOCATION_AS_NEW_SHEET: i64 = 1;
pub const XL_LOCATION_AS_OBJECT: i64 = 2;
pub const XL_SCALE_LINEAR: i64 = -4132;
pub const XL_SCALE_LOGARITHMIC: i64 = -4133;
pub const XL_MOVE_AND_SIZE: i64 = 1;
pub const XL_MOVE: i64 = 2;
pub const XL_FREE_FLOATING: i64 = 3;
/// What `ChartType` reads for a chart of several types.
pub const XL_COMBINATION: i64 = -4111;

/// The chart constants macros use, by VBA name (Excel's `XlChartType`,
/// `XlAxisType`, `XlAxisGroup`, `XlRowCol`, `XlLegendPosition`,
/// `XlChartLocation`, `XlScaleType`, `XlPlacement`), to register.
pub const CHART_CONSTANTS: &[(&str, i64)] = &[
    ("xlColumnClustered", 51),
    ("xlColumnStacked", 52),
    ("xlColumnStacked100", 53),
    ("xl3DColumnClustered", 54),
    ("xl3DColumnStacked", 55),
    ("xl3DColumnStacked100", 56),
    ("xlBarClustered", 57),
    ("xlBarStacked", 58),
    ("xlBarStacked100", 59),
    ("xl3DBarClustered", 60),
    ("xl3DBarStacked", 61),
    ("xl3DBarStacked100", 62),
    ("xlLineStacked", 63),
    ("xlLineStacked100", 64),
    ("xlLineMarkers", 65),
    ("xlLineMarkersStacked", 66),
    ("xlLineMarkersStacked100", 67),
    ("xlPieOfPie", 68),
    ("xlPieExploded", 69),
    ("xl3DPieExploded", 70),
    ("xlBarOfPie", 71),
    ("xlXYScatterSmooth", 72),
    ("xlXYScatterSmoothNoMarkers", 73),
    ("xlXYScatterLines", 74),
    ("xlXYScatterLinesNoMarkers", 75),
    ("xlAreaStacked", 76),
    ("xlAreaStacked100", 77),
    ("xl3DAreaStacked", 78),
    ("xl3DAreaStacked100", 79),
    ("xlDoughnutExploded", 80),
    ("xlRadarMarkers", 81),
    ("xlRadarFilled", 82),
    ("xlSurface", 83),
    ("xlSurfaceWireframe", 84),
    ("xlSurfaceTopView", 85),
    ("xlSurfaceTopViewWireframe", 86),
    ("xlBubble", 15),
    ("xlBubble3DEffect", 87),
    ("xlStockHLC", 88),
    ("xlStockOHLC", 89),
    ("xlStockVHLC", 90),
    ("xlStockVOHLC", 91),
    ("xlLine", 4),
    ("xlPie", 5),
    ("xlArea", 1),
    ("xlXYScatter", -4169),
    ("xlDoughnut", -4120),
    ("xlRadar", -4151),
    ("xl3DArea", -4098),
    ("xl3DColumn", -4100),
    ("xl3DLine", -4101),
    ("xl3DPie", -4102),
    ("xlTreemap", 117),
    ("xlHistogram", 118),
    ("xlWaterfall", 119),
    ("xlSunburst", 120),
    ("xlBoxwhisker", 121),
    ("xlPareto", 122),
    ("xlFunnel", 123),
    ("xlCategory", XL_CATEGORY),
    ("xlValue", XL_VALUE),
    ("xlSeriesAxis", XL_SERIES_AXIS),
    ("xlPrimary", XL_PRIMARY),
    ("xlSecondary", XL_SECONDARY),
    ("xlRows", XL_ROWS),
    ("xlColumns", XL_COLUMNS),
    ("xlLegendPositionBottom", XL_LEGEND_POSITION_BOTTOM),
    ("xlLegendPositionTop", XL_LEGEND_POSITION_TOP),
    ("xlLegendPositionLeft", XL_LEGEND_POSITION_LEFT),
    ("xlLegendPositionRight", XL_LEGEND_POSITION_RIGHT),
    ("xlLegendPositionCorner", XL_LEGEND_POSITION_CORNER),
    ("xlLegendPositionCustom", -4161),
    ("xlLocationAsNewSheet", XL_LOCATION_AS_NEW_SHEET),
    ("xlLocationAsObject", XL_LOCATION_AS_OBJECT),
    ("xlLocationAutomatic", 3),
    ("xlScaleLinear", XL_SCALE_LINEAR),
    ("xlScaleLogarithmic", XL_SCALE_LOGARITHMIC),
    ("xlMoveAndSize", XL_MOVE_AND_SIZE),
    ("xlMove", XL_MOVE),
    ("xlFreeFloating", XL_FREE_FLOATING),
];

// ------------------------------------------------------------------ types

/// Kantor's chart type (and 3-D form) for an `XlChartType`.
fn kind_of(t: i64) -> Result<(ChartType, bool), ScriptError> {
    Ok(match t {
        51 => (ChartType::Column, false),
        52 => (ChartType::StackedColumn, false),
        54 => (ChartType::Column, true),
        55 => (ChartType::StackedColumn, true),
        57 => (ChartType::Bar, false),
        58 => (ChartType::StackedBar, false),
        60 => (ChartType::Bar, true),
        61 => (ChartType::StackedBar, true),
        4 | 65 => (ChartType::Line, false),
        63 | 66 => (ChartType::StackedLine, false),
        -4101 => (ChartType::Line, true),
        5 => (ChartType::Pie, false),
        -4102 => (ChartType::Pie, true),
        1 => (ChartType::Area, false),
        76 => (ChartType::StackedArea, false),
        -4098 => (ChartType::Area, true),
        78 => (ChartType::StackedArea, true),
        -4169 => (ChartType::Scatter, false),
        117 => (ChartType::Treemap, false),
        118 => (ChartType::Histogram, false),
        119 => (ChartType::Waterfall, false),
        120 => (ChartType::Sunburst, false),
        121 => (ChartType::BoxWhisker, false),
        122 => (ChartType::Pareto, false),
        123 => (ChartType::Funnel, false),
        _ => {
            let name = CHART_CONSTANTS
                .iter()
                .take_while(|(n, _)| *n != "xlCategory")
                .find(|(_, v)| *v == t)
                .map_or_else(|| format!("type {t}"), |(n, _)| (*n).to_owned());
            return Err(ScriptError::unsupported(&format!("A chart of type {name}")));
        }
    })
}

/// The `XlChartType` of a chart.
fn type_code(info: &ChartInfo) -> i64 {
    let s = &info.spec;
    match (s.kind, s.three_d) {
        (ChartType::Column, _) if info.kind_label == "3-D Column" => -4100,
        (ChartType::Column, false) => 51,
        (ChartType::Column, true) => 54,
        (ChartType::StackedColumn, false) => 52,
        (ChartType::StackedColumn, true) => 55,
        (ChartType::Bar, false) => 57,
        (ChartType::Bar, true) => 60,
        (ChartType::StackedBar, false) => 58,
        (ChartType::StackedBar, true) => 61,
        (ChartType::Line, false) => 4,
        (ChartType::Line, true) => -4101,
        (ChartType::StackedLine, _) => 63,
        (ChartType::Pie, false) => 5,
        (ChartType::Pie, true) => -4102,
        (ChartType::Area, false) => 1,
        (ChartType::Area, true) => -4098,
        (ChartType::StackedArea, false) => 76,
        (ChartType::StackedArea, true) => 78,
        (ChartType::Scatter, _) => -4169,
        (ChartType::Combo, _) => XL_COMBINATION,
        (ChartType::Waterfall, _) => 119,
        (ChartType::Funnel, _) => 123,
        (ChartType::Treemap, _) => 117,
        (ChartType::Sunburst, _) => 120,
        (ChartType::Histogram, _) => 118,
        (ChartType::Pareto, _) => 122,
        (ChartType::BoxWhisker, _) => 121,
        (ChartType::Other, _) => match info.kind_label.as_str() {
            "100% Stacked Column" => 53,
            "100% Stacked Bar" => 59,
            "100% Stacked Line" => 64,
            "100% Stacked Area" => 77,
            "Doughnut" => -4120,
            "Radar" => -4151,
            "Bubble" => 15,
            "Stock" => 88,
            "Surface" => 83,
            "Pie of Pie" => 68,
            "3-D Column" => -4100,
            "3-D Line" => -4101,
            "3-D Area" => -4098,
            _ => XL_COMBINATION,
        },
    }
}

fn legend_code(p: LegendPos) -> i64 {
    match p {
        LegendPos::Bottom => XL_LEGEND_POSITION_BOTTOM,
        LegendPos::Top => XL_LEGEND_POSITION_TOP,
        LegendPos::Left => XL_LEGEND_POSITION_LEFT,
        LegendPos::Right | LegendPos::None => XL_LEGEND_POSITION_RIGHT,
    }
}

fn legend_pos(code: i64) -> Result<LegendPos, ScriptError> {
    Ok(match code {
        XL_LEGEND_POSITION_BOTTOM => LegendPos::Bottom,
        XL_LEGEND_POSITION_TOP => LegendPos::Top,
        XL_LEGEND_POSITION_LEFT => LegendPos::Left,
        XL_LEGEND_POSITION_RIGHT => LegendPos::Right,
        XL_LEGEND_POSITION_CORNER => {
            return Err(ScriptError::unsupported(
                "A legend in the corner of a chart",
            ));
        }
        -4161 => return Err(ScriptError::unsupported("A legend placed by hand")),
        _ => return Err(ScriptError::runtime(5)),
    })
}

// ------------------------------------------------------------------ helpers

fn gone() -> ScriptError {
    ScriptError::new(424, "Object required (the chart was deleted)")
}

fn failed(member: &str, class: &str, e: impl std::fmt::Display) -> ScriptError {
    err1004(format!(
        "Method '{member}' of object '{class}' failed ({e})"
    ))
}

fn unable(prop: &str, class: &str, e: impl std::fmt::Display) -> ScriptError {
    err1004(format!(
        "Unable to set the {prop} property of the {class} class ({e})"
    ))
}

fn chart_value(s: &Rc<Session>, key: DrawingKey) -> Value {
    obj(Rc::new(ChartObj { s: s.clone(), key }))
}

fn chart_info(s: &Rc<Session>, key: DrawingKey) -> Result<ChartInfo, ScriptError> {
    s.read()?.chart_info(key).ok_or_else(gone)
}

/// Change the chart's spec as the Chart panel does (one undo step).
fn edit_spec(
    s: &Rc<Session>,
    key: DrawingKey,
    prop: &str,
    f: impl FnOnce(&mut ChartSpec, &ChartInfo) -> Result<(), ScriptError>,
) -> Result<(), ScriptError> {
    let info = chart_info(s, key)?;
    let mut spec = info.spec.clone();
    f(&mut spec, &info)?;
    if spec == info.spec {
        return Ok(());
    }
    s.write()?
        .set_chart(key, spec)
        .map_err(|e| unable(prop, "Chart", e))
}

fn is_chart(d: &Drawing) -> bool {
    matches!(*d.kind, DrawingKind::Chart { .. })
}

/// The charts on worksheet `idx`, bottom to top.
fn charts_on(book: &Workbook, idx: u32) -> Vec<Drawing> {
    book.drawings(idx).into_iter().filter(is_chart).collect()
}

/// The chart of chart sheet `k`.
fn chart_sheet_key(book: &Workbook, k: usize) -> Option<DrawingKey> {
    book.chart_sheet_drawings(k)
        .into_iter()
        .find(is_chart)
        .map(|d| d.key)
}

/// The chart object `key` on its worksheet (index) now.
fn find_drawing(s: &Rc<Session>, key: DrawingKey) -> Result<(u32, Drawing), ScriptError> {
    if chart_sheet_of_key(key.sheet_id).is_some() {
        return Err(ScriptError::no_member("Chart", "ChartObject"));
    }
    let idx = s.sheet_index(key.sheet_id)?;
    let d = s
        .read()?
        .drawings(idx)
        .into_iter()
        .find(|d| d.key == key)
        .ok_or_else(gone)?;
    Ok((idx, d))
}

/// A worksheet in points (Excel's `Left` / `Top` / `Width` / `Height`).
struct Points<'a> {
    book: &'a Workbook,
    sheet: u32,
}

fn points(book: &Workbook, sheet: u32) -> Points<'_> {
    Points { book, sheet }
}

impl SheetGeometry for Points<'_> {
    fn col_left(&self, col: u32) -> f64 {
        (0..col.min(MAX_COLS)).map(|c| self.col_width(c)).sum()
    }
    fn col_width(&self, col: u32) -> f64 {
        self.book.column_width(self.sheet, col) * 0.75
    }
    fn row_top(&self, row: u32) -> f64 {
        (0..row.min(MAX_ROWS)).map(|r| self.row_height(r)).sum()
    }
    fn row_height(&self, row: u32) -> f64 {
        self.book.row_points(self.sheet, row)
    }
    fn col_at(&self, x: f64) -> u32 {
        let mut at = 0.0;
        for c in 0..MAX_COLS {
            at += self.col_width(c);
            if x < at {
                return c;
            }
        }
        MAX_COLS - 1
    }
    fn row_at(&self, y: f64) -> u32 {
        let mut at = 0.0;
        for r in 0..MAX_ROWS {
            at += self.row_height(r);
            if y < at {
                return r;
            }
        }
        MAX_ROWS - 1
    }
    fn scale(&self) -> (f64, f64) {
        (0.75, 0.75)
    }
}

/// A two-cell anchor at `[left, top, width, height]` points.
fn anchor_at(book: &Workbook, sheet: u32, rect: [f64; 4], edit_as: EditAs) -> Anchor {
    let g = points(book, sheet);
    let [x, y, w, h] = rect;
    let (x, y) = (x.max(0.0), y.max(0.0));
    Anchor::TwoCell {
        from: Marker::at(x, y, &g),
        to: Marker::at(x + w.max(1.0), y + h.max(1.0), &g),
        edit_as,
    }
}

/// The cells a new chart charts: the selection on `sheet`, or the data
/// around its active cell (as Excel's Insert Chart), when it has numbers.
fn selection_data(s: &Rc<Session>, sheet: u32) -> Result<Option<(u32, CellRange)>, ScriptError> {
    let sel = s.selection(sheet);
    let idx = s.sheet_index(sheet)?;
    let book = s.read()?;
    let area = if sel.range.is_single() {
        book.current_region(idx, sel.active.0, sel.active.1)
    } else {
        sel.range
    };
    Ok(book.script_chart_data(idx, area).map(|d| (idx, d)))
}

thread_local! {
    /// `ActiveChart`: the run's session and the chart.
    static ACTIVE: RefCell<Option<(Weak<Session>, DrawingKey)>> = const { RefCell::new(None) };
}

/// A new run on session `s` (the engine reuses it): no `ActiveChart` yet.
pub(crate) fn end_run(s: &Session) {
    ACTIVE.with(|a| {
        let mut a = a.borrow_mut();
        if a.as_ref().is_some_and(|(w, _)| std::ptr::eq(w.as_ptr(), s)) {
            *a = None;
        }
    });
}

fn set_active(s: &Rc<Session>, key: DrawingKey) {
    ACTIVE.with(|a| *a.borrow_mut() = Some((Rc::downgrade(s), key)));
}

/// `ActiveChart`: the chart the macro last added, selected or activated
/// (`Nothing` when there is none, or it was deleted).
pub fn active_chart(s: &Rc<Session>) -> Value {
    let key = ACTIVE.with(|a| match &*a.borrow() {
        Some((w, k)) if w.upgrade().is_some_and(|x| Rc::ptr_eq(&x, s)) => Some(*k),
        _ => None,
    });
    match key {
        Some(k) if s.read().is_ok_and(|b| b.chart_info(k).is_some()) => chart_value(s, k),
        _ => Value::NOTHING,
    }
}

// ------------------------------------------------------------------ entry points

/// `Worksheet.ChartObjects` / `ChartObjects(index)` of worksheet `sheet`.
pub fn chart_objects(s: &Rc<Session>, sheet: u32, args: &[Arg]) -> Result<Value, ScriptError> {
    let c = ChartObjectsObj {
        s: s.clone(),
        sheet,
    };
    if args.is_empty() {
        return Ok(obj(Rc::new(c)));
    }
    c.get("", args)
}

/// `Workbook.Charts` (the chart sheets).
pub fn charts_collection(s: &Rc<Session>) -> Value {
    obj(Rc::new(ChartsObj { s: s.clone() }))
}

/// `Charts` / `Charts(index)`.
pub fn charts(s: &Rc<Session>, args: &[Arg]) -> Result<Value, ScriptError> {
    if args.is_empty() {
        return Ok(charts_collection(s));
    }
    ChartsObj { s: s.clone() }.get("", args)
}

/// The `Chart` of chart object `name` on worksheet `sheet` (for
/// `Shapes("Chart 1").Chart`).
pub fn chart_by_name(s: &Rc<Session>, sheet: u32, name: &str) -> Result<Value, ScriptError> {
    let idx = s.sheet_index(sheet)?;
    let key = charts_on(&*s.read()?, idx)
        .into_iter()
        .find(|d| d.name.eq_ignore_ascii_case(name))
        .map(|d| d.key)
        .ok_or_else(|| {
            err1004(format!(
                "The item with the specified name wasn't found ({name})"
            ))
        })?;
    Ok(chart_value(s, key))
}

/// `Shapes.AddChart2(Style, XlChartType, Left, Top, Width, Height,
/// NewLayout)` and `Shapes.AddChart(XlChartType, Left, Top, Width,
/// Height)` on worksheet `sheet`: a first positional argument of -1 or 200
/// and above (a chart style), or a named `Style` / `NewLayout`, is the
/// `AddChart2` form; `Style` and `NewLayout` are accepted and change
/// nothing. As in Excel, the chart shows the selection (or the data around
/// the active cell) when that has numbers, else it is empty; it is 360 ×
/// 216 points by default, beside its data or at the active cell. It becomes
/// the `ActiveChart`. Returns the `ChartObject`.
pub fn add_chart2(s: &Rc<Session>, sheet: u32, args: &[Arg]) -> Result<Value, ScriptError> {
    let named = |n: &str| {
        args.iter()
            .any(|a| a.name.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(n)))
    };
    let first = args.iter().find(|a| a.name.is_none()).map(|a| &a.value);
    let style_form = named("Style")
        || named("NewLayout")
        || match first {
            Some(v) if v.is_missing() => true,
            Some(v) => to_i64(v).is_ok_and(|n| n == -1 || n >= 200),
            None => false,
        };
    let off = usize::from(style_form);
    let t = match arg_i64(args, off, "XlChartType")? {
        None | Some(-1) => XL_COLUMN_CLUSTERED,
        Some(t) => t,
    };
    let (kind, three_d) = kind_of(t)?;
    let num = |i: usize, n: &str| arg(args, i, n).map(to_f64).transpose();
    let (left, top) = (num(off + 1, "Left")?, num(off + 2, "Top")?);
    let width = num(off + 3, "Width")?.unwrap_or(360.0);
    let height = num(off + 4, "Height")?.unwrap_or(216.0);
    let data = selection_data(s, sheet)?;
    let idx = s.sheet_index(sheet)?;
    let key = {
        let mut book = s.write()?;
        let (x, y) = {
            let g = points(&book, idx);
            let (row, col) = match data {
                Some((_, d)) => (d.top, (d.right + 2).min(MAX_COLS - 1)),
                None => s.selection(sheet).active,
            };
            (
                left.unwrap_or_else(|| g.col_left(col)),
                top.unwrap_or_else(|| g.row_top(row)),
            )
        };
        let anchor = anchor_at(&book, idx, [x, y, width, height], EditAs::TwoCell);
        book.script_add_chart(
            idx,
            anchor,
            kind,
            three_d,
            data.map(|(ds, d)| (ds, d, None)),
        )
        .map_err(|e| failed("AddChart2", "Shapes", e))?
    };
    set_active(s, key);
    Ok(obj(Rc::new(ChartObjectObj { s: s.clone(), key })))
}

// ------------------------------------------------------------------ ChartObjects

pub struct ChartObjectsObj {
    s: Rc<Session>,
    sheet: u32,
}

impl ChartObjectsObj {
    fn keys(&self) -> Result<Vec<Drawing>, ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        Ok(charts_on(&*self.s.read()?, idx))
    }

    fn item(&self, key: &Value) -> Result<Value, ScriptError> {
        let all = self.keys()?;
        let d = match scalar(key)? {
            Value::String(n) => all
                .into_iter()
                .find(|d| d.name.eq_ignore_ascii_case(&n))
                .ok_or_else(|| ScriptError::runtime(9))?,
            Value::Array(_) => {
                return Err(ScriptError::unsupported("Several chart objects at once"));
            }
            v => {
                let i = to_i64(&v)?;
                if i < 1 || i as usize > all.len() {
                    return Err(ScriptError::runtime(9));
                }
                all.into_iter()
                    .nth(i as usize - 1)
                    .ok_or_else(|| ScriptError::runtime(9))?
            }
        };
        Ok(obj(Rc::new(ChartObjectObj {
            s: self.s.clone(),
            key: d.key,
        })))
    }
}

impl Dispatch for ChartObjectsObj {
    fn type_name(&self) -> String {
        "ChartObjects".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" => self.item(need(args, 0, "Index")?),
            "count" => Ok(Value::Long(self.keys()?.len() as i32)),
            "add" => {
                let f = |i: usize, n: &str| to_f64(need(args, i, n)?);
                let rect = [f(0, "Left")?, f(1, "Top")?, f(2, "Width")?, f(3, "Height")?];
                let idx = self.s.sheet_index(self.sheet)?;
                let key = {
                    let mut book = self.s.write()?;
                    let anchor = anchor_at(&book, idx, rect, EditAs::TwoCell);
                    book.script_add_chart(idx, anchor, ChartType::Column, false, None)
                        .map_err(|e| failed("Add", "ChartObjects", e))?
                };
                Ok(obj(Rc::new(ChartObjectObj {
                    s: self.s.clone(),
                    key,
                })))
            }
            "delete" => {
                for d in self.keys()? {
                    self.s
                        .write()?
                        .delete_drawing(d.key)
                        .map_err(|e| failed("Delete", "ChartObjects", e))?;
                }
                Ok(Value::Empty)
            }
            "select" | "activate" => Ok(Value::Empty),
            "parent" => Ok(obj(SheetObj::new(&self.s, self.sheet))),
            "application" => Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            "copy" | "cut" | "duplicate" | "copypicture" | "bringtofront" | "sendtoback"
            | "group" | "shaperange" => Err(ScriptError::unsupported(&format!(
                "ChartObjects.{name} from a macro"
            ))),
            _ => Err(ScriptError::no_member("ChartObjects", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(self
            .keys()?
            .into_iter()
            .map(|d| {
                obj(Rc::new(ChartObjectObj {
                    s: self.s.clone(),
                    key: d.key,
                }))
            })
            .collect())
    }
}

// ------------------------------------------------------------------ ChartObject

/// A chart on a worksheet (its frame).
pub struct ChartObjectObj {
    pub s: Rc<Session>,
    pub key: DrawingKey,
}

impl ChartObjectObj {
    fn rect(&self) -> Result<[f64; 4], ScriptError> {
        let (idx, d) = find_drawing(&self.s, self.key)?;
        let book = self.s.read()?;
        Ok(d.anchor.rect(&points(&book, idx)))
    }

    /// Place the frame at `rect` (points), with `edit_as`.
    fn place(&self, rect: [f64; 4], edit_as: Option<EditAs>) -> Result<(), ScriptError> {
        let (idx, d) = find_drawing(&self.s, self.key)?;
        let mut book = self.s.write()?;
        let anchor = match edit_as {
            Some(e) => anchor_at(&book, idx, rect, e),
            None => {
                let g = points(&book, idx);
                d.anchor.with_rect(rect, &g)
            }
        };
        book.place_drawing(self.key, anchor)
            .map_err(|e| unable("Left", "ChartObject", e))
    }

    fn placement(&self) -> Result<i64, ScriptError> {
        let (_, d) = find_drawing(&self.s, self.key)?;
        Ok(match d.anchor {
            Anchor::TwoCell {
                edit_as: EditAs::TwoCell,
                ..
            } => XL_MOVE_AND_SIZE,
            Anchor::TwoCell {
                edit_as: EditAs::OneCell,
                ..
            }
            | Anchor::OneCell { .. } => XL_MOVE,
            _ => XL_FREE_FLOATING,
        })
    }

    /// The cell under a corner of the frame.
    fn corner_cell(&self, bottom_right: bool) -> Result<Value, ScriptError> {
        let (idx, d) = find_drawing(&self.s, self.key)?;
        let book = self.s.read()?;
        let g = points(&book, idx);
        let [x, y, w, h] = d.anchor.rect(&g);
        let (x, y) = if bottom_right {
            ((x + w - 0.01).max(x), (y + h - 0.01).max(y))
        } else {
            (x, y)
        };
        let (r, c) = (g.row_at(y), g.col_at(x));
        drop(book);
        Ok(obj(crate::range::RangeObj::new(
            &self.s,
            self.key.sheet_id,
            CellRange::from_corners((r, c), (r, c)),
        )))
    }
}

impl Dispatch for ChartObjectObj {
    fn type_name(&self) -> String {
        "ChartObject".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "" | "name" => Ok(Value::String(find_drawing(&self.s, self.key)?.1.name)),
            "chart" => {
                find_drawing(&self.s, self.key)?;
                Ok(chart_value(&self.s, self.key))
            }
            "left" | "top" | "width" | "height" => {
                let r = self.rect()?;
                let i = ["left", "top", "width", "height"]
                    .iter()
                    .position(|p| *p == lower)
                    .unwrap_or(0);
                Ok(Value::Double(r[i]))
            }
            "placement" => Ok(Value::Long(self.placement()? as i32)),
            "topleftcell" => self.corner_cell(false),
            "bottomrightcell" => self.corner_cell(true),
            "visible" => Ok(Value::Boolean(!find_drawing(&self.s, self.key)?.1.hidden)),
            "index" => {
                let idx = self.s.sheet_index(self.key.sheet_id)?;
                let all = charts_on(&*self.s.read()?, idx);
                let i = all
                    .iter()
                    .position(|d| d.key == self.key)
                    .ok_or_else(gone)?;
                Ok(Value::Long(i as i32 + 1))
            }
            "delete" => {
                find_drawing(&self.s, self.key)?;
                self.s
                    .write()?
                    .delete_drawing(self.key)
                    .map_err(|e| failed("Delete", "ChartObject", e))?;
                Ok(Value::Empty)
            }
            "select" | "activate" => {
                find_drawing(&self.s, self.key)?;
                set_active(&self.s, self.key);
                Ok(Value::Empty)
            }
            "parent" => Ok(obj(SheetObj::new(&self.s, self.key.sheet_id))),
            "application" => Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            "copy" | "cut" | "duplicate" | "copypicture" | "bringtofront" | "sendtoback"
            | "border" | "interior" | "shadow" | "roundedcorners" | "locked" | "printobject"
            | "onaction" | "shaperange" => Err(ScriptError::unsupported(&format!(
                "ChartObject.{name} from a macro"
            ))),
            _ => Err(ScriptError::no_member("ChartObject", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "name" => {
                let n = to_str(&value)?;
                if find_drawing(&self.s, self.key)?.1.name == n {
                    return Ok(());
                }
                Err(ScriptError::unsupported("Renaming a chart object"))
            }
            "left" | "top" | "width" | "height" => {
                let v = to_f64(&value)?;
                let mut r = self.rect()?;
                let i = ["left", "top", "width", "height"]
                    .iter()
                    .position(|p| *p == lower)
                    .unwrap_or(0);
                if i >= 2 && v < 0.0 {
                    return Err(unable(name, "ChartObject", "a size can't be negative"));
                }
                r[i] = v;
                self.place(r, None)
            }
            "placement" => {
                let e = match to_i64(&value)? {
                    XL_MOVE_AND_SIZE => EditAs::TwoCell,
                    XL_MOVE => EditAs::OneCell,
                    XL_FREE_FLOATING => EditAs::Absolute,
                    _ => return Err(ScriptError::runtime(5)),
                };
                let r = self.rect()?;
                self.place(r, Some(e))
            }
            "visible" => {
                if to_bool(&value)? {
                    return Ok(());
                }
                Err(ScriptError::unsupported("Hiding a chart object"))
            }
            "locked" | "printobject" | "roundedcorners" | "shadow" | "onaction" => Err(
                ScriptError::unsupported(&format!("ChartObject.{name} from a macro")),
            ),
            _ => Err(ScriptError::no_member("ChartObject", name)),
        }
    }
}

// ------------------------------------------------------------------ Chart

/// A chart: on a worksheet (`key.sheet_id` is its `sheetId`) or a chart
/// sheet's (see [`chart_sheet_key_id`]).
pub struct ChartObj {
    pub s: Rc<Session>,
    pub key: DrawingKey,
}

impl ChartObj {
    fn sheet_k(&self) -> Option<usize> {
        chart_sheet_of_key(self.key.sheet_id)
    }

    /// The worksheet references without a sheet are on: the chart's own,
    /// or for a chart sheet the active one.
    fn home(&self) -> u32 {
        match self.sheet_k() {
            Some(_) => self.s.active.get(),
            None => self.key.sheet_id,
        }
    }

    fn info(&self) -> Result<ChartInfo, ScriptError> {
        chart_info(&self.s, self.key)
    }

    fn edit(
        &self,
        prop: &str,
        f: impl FnOnce(&mut ChartSpec, &ChartInfo) -> Result<(), ScriptError>,
    ) -> Result<(), ScriptError> {
        edit_spec(&self.s, self.key, prop, f)
    }

    fn name(&self) -> Result<String, ScriptError> {
        match self.sheet_k() {
            Some(k) => self.s.read()?.chart_sheet_name(k).ok_or_else(gone),
            None => {
                let (idx, d) = find_drawing(&self.s, self.key)?;
                let sheet = self.s.read()?.sheet_names()[idx as usize].clone();
                Ok(format!("{sheet} {}", d.name))
            }
        }
    }

    fn set_source(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let src = need(args, 0, "Source")?;
        let r = match range_of(src) {
            Some(r) => r,
            None => crate::sheet::resolve(&self.s, self.home(), &to_str(src)?)?,
        };
        if r.areas.len() > 1 {
            return Err(ScriptError::unsupported(
                "A chart source of several separate ranges",
            ));
        }
        let ds = self.s.sheet_index(r.sheet)?;
        let (data, auto_rows) = {
            let book = self.s.read()?;
            let data = book.script_chart_data(ds, r.first()).ok_or_else(|| {
                failed(
                    "SetSourceData",
                    "_Chart",
                    "the range has no numbers to chart",
                )
            })?;
            (data, book.script_plot_by_rows(ds, data))
        };
        let by_rows = match arg_i64(args, 1, "PlotBy")? {
            None => auto_rows,
            Some(XL_ROWS) => true,
            Some(XL_COLUMNS) => false,
            Some(_) => return Err(ScriptError::runtime(5)),
        };
        let home = self.sheet_k().is_none().then_some(self.key.sheet_id);
        let sheet = r.sheet;
        let info = self.info()?;
        let mut spec = info.spec.clone();
        spec.data = data;
        spec.by_rows = by_rows;
        spec.data_sheet = (home != Some(sheet)).then_some(sheet);
        self.s
            .write()?
            .set_chart(self.key, spec)
            .map_err(|e| failed("SetSourceData", "_Chart", e))?;
        Ok(Value::Empty)
    }

    fn axes(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let Some(t) = arg_i64(args, 0, "Type")? else {
            return Err(ScriptError::unsupported(
                "The Axes collection (use Axes(xlCategory) or Axes(xlValue))",
            ));
        };
        if arg_i64(args, 1, "AxisGroup")?.unwrap_or(XL_PRIMARY) == XL_SECONDARY {
            return Err(ScriptError::unsupported("Secondary axes from a macro"));
        }
        axis_side(&self.info()?, t)?;
        Ok(obj(Rc::new(AxisObj {
            s: self.s.clone(),
            key: self.key,
            axis: t,
        })))
    }

    fn location(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let to = match to_i64(need(args, 0, "Where")?)? {
            XL_LOCATION_AS_NEW_SHEET => {
                let name = match arg_str(args, 1, "Name")? {
                    Some(n) => n,
                    None => self.s.read()?.next_chart_sheet_name(),
                };
                ChartPlace::NewSheet(name)
            }
            XL_LOCATION_AS_OBJECT => {
                let name = arg_str(args, 1, "Name")?.ok_or_else(|| ScriptError::runtime(449))?;
                let id = crate::sheet::sheet_by_name(&self.s, &name)
                    .ok_or_else(|| ScriptError::runtime(9))?;
                ChartPlace::ObjectIn(self.s.sheet_index(id)?)
            }
            _ => return Err(ScriptError::runtime(5)),
        };
        let moved = self
            .s
            .write()?
            .move_chart(self.key, to)
            .map_err(|e| failed("Location", "_Chart", e))?;
        let key = match moved {
            MovedChart::Object { key, .. } => key,
            MovedChart::ChartSheet(k) => chart_sheet_key(&*self.s.read()?, k).ok_or_else(gone)?,
        };
        Ok(chart_value(&self.s, key))
    }

    /// The chart's size in points, for `Export`.
    fn size(&self) -> Result<(f64, f64), ScriptError> {
        if let Some(k) = self.sheet_k() {
            let book = self.s.read()?;
            let d = book
                .chart_sheet_drawings(k)
                .into_iter()
                .find(|d| d.key == self.key)
                .ok_or_else(gone)?;
            return Ok(match d.anchor {
                Anchor::Absolute { cx, cy, .. } | Anchor::OneCell { cx, cy, .. }
                    if cx > 0 && cy > 0 =>
                {
                    (cx as f64 / 12_700.0, cy as f64 / 12_700.0)
                }
                _ => (720.0, 450.0),
            });
        }
        let (idx, d) = find_drawing(&self.s, self.key)?;
        let book = self.s.read()?;
        let [_, _, w, h] = d.anchor.rect(&points(&book, idx));
        Ok((w, h))
    }

    fn export(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let file = to_str(need(args, 0, "Filename")?)?;
        match arg_str(args, 1, "FilterName")? {
            Some(f) if !f.eq_ignore_ascii_case("png") => {
                return Err(ScriptError::unsupported(&format!(
                    "Exporting a chart as {f} (PNG is)"
                )));
            }
            Some(_) => {}
            None => {
                let lower = file.to_lowercase();
                if [".gif", ".jpg", ".jpeg", ".bmp", ".tif", ".tiff", ".emf"]
                    .iter()
                    .any(|e| lower.ends_with(e))
                {
                    return Err(ScriptError::unsupported(
                        "Exporting a chart in a format other than PNG",
                    ));
                }
            }
        }
        let (w, h) = self.size()?;
        let path = crate::paths::checked(&self.s, &file, true)?;
        let png = {
            let book = self.s.read()?;
            let xml = book.script_chart_part(self.key).ok_or_else(gone)?;
            render_png(&book, self.key, &xml, w as f32, h as f32)?
        };
        std::fs::write(&path, png)
            .map_err(|e| failed("Export", "_Chart", format!("{}: {e}", path.display())))?;
        Ok(Value::Boolean(true))
    }

    fn delete(&self) -> Result<Value, ScriptError> {
        let mut book = self.s.write()?;
        let done = match self.sheet_k() {
            Some(k) => book.delete_chart_sheet(k),
            None => book.delete_drawing(self.key),
        };
        done.map_err(|e| failed("Delete", "_Chart", e))?;
        Ok(Value::Empty)
    }
}

impl Dispatch for ChartObj {
    fn type_name(&self) -> String {
        "Chart".into()
    }

    fn as_any(&self) -> Option<&dyn Any> {
        Some(self)
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "" | "name" => Ok(Value::String(self.name()?)),
            "charttype" => Ok(Value::Long(type_code(&self.info()?) as i32)),
            "setsourcedata" => self.set_source(args),
            "hastitle" => Ok(Value::Boolean(!self.info()?.spec.title.is_empty())),
            "charttitle" => {
                self.info()?;
                Ok(obj(Rc::new(TitleObj {
                    s: self.s.clone(),
                    key: self.key,
                    axis: None,
                })))
            }
            "haslegend" => Ok(Value::Boolean(self.info()?.spec.legend != LegendPos::None)),
            "legend" => {
                self.info()?;
                Ok(obj(Rc::new(LegendObj {
                    s: self.s.clone(),
                    key: self.key,
                })))
            }
            "seriescollection" | "fullseriescollection" => {
                let c = SeriesCollectionObj {
                    s: self.s.clone(),
                    key: self.key,
                };
                if args.is_empty() {
                    self.info()?;
                    return Ok(obj(Rc::new(c)));
                }
                c.get("", args)
            }
            "axes" => self.axes(args),
            "hasaxis" => Ok(Value::Boolean(self.info()?.has_axes)),
            "plotby" => Ok(Value::Long(if self.info()?.spec.by_rows {
                XL_ROWS as i32
            } else {
                XL_COLUMNS as i32
            })),
            "hasdatatable" => Ok(Value::Boolean(false)),
            "export" => self.export(args),
            "location" => self.location(args),
            "parent" => Ok(match self.sheet_k() {
                Some(_) => obj(Rc::new(crate::app::WorkbookObj::new(&self.s))),
                None => {
                    find_drawing(&self.s, self.key)?;
                    obj(Rc::new(ChartObjectObj {
                        s: self.s.clone(),
                        key: self.key,
                    }))
                }
            }),
            "application" => Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            "activate" | "select" => {
                self.info()?;
                set_active(&self.s, self.key);
                Ok(Value::Empty)
            }
            "refresh" => Ok(Value::Empty),
            "delete" => self.delete(),
            "visible" => match self.sheet_k() {
                Some(k) => {
                    let hidden = self
                        .s
                        .read()?
                        .hidden_chart_sheets()
                        .iter()
                        .any(|(h, _)| *h == k);
                    Ok(Value::Long(if hidden {
                        crate::consts::XL_SHEET_HIDDEN as i32
                    } else {
                        crate::consts::XL_SHEET_VISIBLE as i32
                    }))
                }
                None => Err(ScriptError::no_member("Chart", name)),
            },
            "index" => match self.sheet_k() {
                Some(k) => {
                    let book = self.s.read()?;
                    let tabs = book.tabs();
                    let i = tabs
                        .iter()
                        .position(|t| *t == Tab::Chart(k))
                        .ok_or_else(gone)?;
                    Ok(Value::Long(i as i32 + 1))
                }
                None => Err(ScriptError::no_member("Chart", name)),
            },
            "chartarea"
            | "plotarea"
            | "chartstyle"
            | "applylayout"
            | "applycharttemplate"
            | "cleartomatchstyle"
            | "chartgroups"
            | "displayblanksas"
            | "datatable"
            | "walls"
            | "floor"
            | "backwall"
            | "sidewall"
            | "elevation"
            | "rotation"
            | "perspective"
            | "copypicture"
            | "copy"
            | "chartwizard"
            | "applydatalabels"
            | "shapes"
            | "printout"
            | "printpreview"
            | "exportasfixedformat"
            | "barshape"
            | "chartcolor"
            | "showdatalabelsovermaximum"
            | "autoscaling"
            | "rightangleaxes"
            | "hasaxisgroup"
            | "pivotlayout" => Err(ScriptError::unsupported(&format!(
                "Chart.{name} from a macro"
            ))),
            _ => Err(ScriptError::no_member("Chart", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "charttype" => {
                let (kind, three_d) = kind_of(to_i64(&value)?)?;
                self.edit("ChartType", |spec, _| {
                    spec.kind = kind;
                    spec.three_d = three_d;
                    Ok(())
                })
            }
            "hastitle" => {
                let on = to_bool(&value)?;
                self.edit("HasTitle", |spec, info| {
                    if !on {
                        spec.title.clear();
                    } else if spec.title.is_empty() {
                        spec.title = match info.series.as_slice() {
                            [one] if !one.is_empty() => one.clone(),
                            _ => "Chart Title".to_owned(),
                        };
                    }
                    Ok(())
                })
            }
            "haslegend" => {
                let on = to_bool(&value)?;
                self.edit("HasLegend", |spec, _| {
                    if !on {
                        spec.legend = LegendPos::None;
                    } else if spec.legend == LegendPos::None {
                        spec.legend = LegendPos::Right;
                    }
                    Ok(())
                })
            }
            "plotby" => {
                let by_rows = match to_i64(&value)? {
                    XL_ROWS => true,
                    XL_COLUMNS => false,
                    _ => return Err(ScriptError::runtime(5)),
                };
                self.edit("PlotBy", |spec, info| {
                    if info.data_unknown && spec.by_rows != by_rows {
                        return Err(unable(
                            "PlotBy",
                            "Chart",
                            "its series aren't one block of cells",
                        ));
                    }
                    spec.by_rows = by_rows;
                    Ok(())
                })
            }
            "name" => match self.sheet_k() {
                Some(k) => {
                    let n = to_str(&value)?;
                    self.s
                        .write()?
                        .rename_chart_sheet(k, &n)
                        .map_err(|e| unable("Name", "Chart", e))
                }
                None => Err(ScriptError::unsupported("Renaming an embedded chart")),
            },
            "visible" => match self.sheet_k() {
                Some(k) => {
                    let hidden = match scalar(&value)? {
                        Value::Boolean(b) => !b,
                        v => to_i64(&v)? != crate::consts::XL_SHEET_VISIBLE,
                    };
                    self.s
                        .write()?
                        .set_chart_sheet_hidden(k, hidden)
                        .map_err(|e| unable("Visible", "Chart", e))
                }
                None => Err(ScriptError::no_member("Chart", name)),
            },
            "hasdatatable" | "hasaxis" | "chartstyle" | "displayblanksas" | "barshape"
            | "elevation" | "rotation" | "perspective" | "rightangleaxes" | "autoscaling"
            | "chartcolor" => Err(ScriptError::unsupported(&format!(
                "Setting Chart.{name} from a macro"
            ))),
            _ => Err(ScriptError::no_member("Chart", name)),
        }
    }
}

// ------------------------------------------------------------------ Export

/// Chart part `xml` drawn `w` × `h` points (at 96 dpi, as Excel exports)
/// as a PNG file: the PDF export's own drawing, rasterized as the print
/// preview does.
fn render_png(
    book: &Workbook,
    key: DrawingKey,
    xml: &str,
    w: f32,
    h: f32,
) -> Result<Vec<u8>, ScriptError> {
    let (w, h) = (w.clamp(1.0, 4000.0), h.clamp(1.0, 4000.0));
    let theme = sheets_scene::ChartTheme::of(book);
    let base = theme
        .parse(xml)
        .filter(|c| c.drawable())
        .ok_or_else(|| failed("Export", "_Chart", "Kantor can't draw this chart"))?;
    let (chart, hash) = sheets_scene::live_chart(&base, book, true);
    let page = sheets_pdf::PageContent {
        width: w,
        height: h,
        origin: [0.0, 0.0],
        scale: 1.0,
        block: [w, h],
        overlays: vec![sheets_pdf::Overlay {
            rect: [0.0, 0.0, w, h],
            clip: [0.0, 0.0, w, h],
            kind: sheets_pdf::OverlayKind::Chart {
                key,
                hash,
                chart: Arc::new(chart),
                defaults: Arc::new(theme.defaults().clone()),
            },
        }],
        ..Default::default()
    };
    let raster = sheets_pdf::preview::Rasterizer::new()
        .render(&page, 4.0 / 3.0)
        .map_err(|e| failed("Export", "_Chart", e))?;
    Ok(png_rgb(raster.width, raster.height, &raster.rgba))
}

fn crc32(parts: &[&[u8]]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for p in parts {
        for &b in *p {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
        }
    }
    !c
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[&kind[..], data]).to_be_bytes());
}

/// An opaque RGBA picture as an 8-bit RGB PNG (stored, uncompressed
/// deflate blocks: no compression library needed).
fn png_rgb(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let row = width as usize * 4;
    let mut raw = Vec::with_capacity((width as usize * 3 + 1) * height as usize);
    for line in rgba.chunks_exact(row.max(1)).take(height as usize) {
        raw.push(0);
        for px in line.as_chunks::<4>().0 {
            raw.extend_from_slice(&px[..3]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65_535).collect();
    if blocks.is_empty() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    for (i, b) in blocks.iter().enumerate() {
        z.push(u8::from(i + 1 == blocks.len()));
        let n = b.len() as u16;
        z.extend_from_slice(&n.to_le_bytes());
        z.extend_from_slice(&(!n).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &z);
    png_chunk(&mut out, b"IEND", &[]);
    out
}

// ------------------------------------------------------------------ titles, legend

/// `ChartTitle` (`axis: None`) or an axis' `AxisTitle`.
pub struct TitleObj {
    s: Rc<Session>,
    key: DrawingKey,
    axis: Option<i64>,
}

impl TitleObj {
    fn text(&self) -> Result<String, ScriptError> {
        let info = chart_info(&self.s, self.key)?;
        Ok(match self.axis {
            None => info.spec.title,
            Some(t) => {
                if axis_side(&info, t)? {
                    info.spec.x_title
                } else {
                    info.spec.y_title
                }
            }
        })
    }

    fn set_text(&self, text: String) -> Result<(), ScriptError> {
        let axis = self.axis;
        edit_spec(&self.s, self.key, "Text", |spec, info| {
            match axis {
                None => spec.title = text,
                Some(t) => {
                    if axis_side(info, t)? {
                        spec.x_title = text;
                    } else {
                        spec.y_title = text;
                    }
                }
            }
            Ok(())
        })
    }

    fn class(&self) -> &'static str {
        if self.axis.is_some() {
            "AxisTitle"
        } else {
            "ChartTitle"
        }
    }
}

impl Dispatch for TitleObj {
    fn type_name(&self) -> String {
        self.class().into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "text" | "caption" => Ok(Value::String(self.text()?)),
            "delete" => {
                self.set_text(String::new())?;
                Ok(Value::Empty)
            }
            "select" => Ok(Value::Empty),
            "parent" => Ok(chart_value(&self.s, self.key)),
            "font"
            | "format"
            | "characters"
            | "left"
            | "top"
            | "orientation"
            | "horizontalalignment"
            | "verticalalignment"
            | "formula"
            | "interior"
            | "border"
            | "shadow"
            | "position"
            | "includeinlayout"
            | "autoscalefont" => Err(ScriptError::unsupported(&format!(
                "{}.{name} from a macro",
                self.class()
            ))),
            _ => Err(ScriptError::no_member(self.class(), name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "text" | "caption" => self.set_text(to_str(&value)?),
            "font"
            | "left"
            | "top"
            | "orientation"
            | "horizontalalignment"
            | "verticalalignment"
            | "formula"
            | "shadow"
            | "position"
            | "includeinlayout"
            | "autoscalefont" => Err(ScriptError::unsupported(&format!(
                "Setting {}.{name} from a macro",
                self.class()
            ))),
            _ => Err(ScriptError::no_member(self.class(), name)),
        }
    }
}

pub struct LegendObj {
    s: Rc<Session>,
    key: DrawingKey,
}

impl Dispatch for LegendObj {
    fn type_name(&self) -> String {
        "Legend".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "position" => Ok(Value::Long(
                legend_code(chart_info(&self.s, self.key)?.spec.legend) as i32,
            )),
            "delete" => {
                edit_spec(&self.s, self.key, "HasLegend", |spec, _| {
                    spec.legend = LegendPos::None;
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            "select" => Ok(Value::Empty),
            "parent" => Ok(chart_value(&self.s, self.key)),
            "font" | "format" | "left" | "top" | "width" | "height" | "includeinlayout"
            | "legendentries" | "interior" | "border" | "shadow" | "clear" => Err(
                ScriptError::unsupported(&format!("Legend.{name} from a macro")),
            ),
            _ => Err(ScriptError::no_member("Legend", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "position" => {
                let p = legend_pos(to_i64(&value)?)?;
                edit_spec(&self.s, self.key, "Position", |spec, _| {
                    spec.legend = p;
                    Ok(())
                })
            }
            "includeinlayout" | "left" | "top" | "width" | "height" | "shadow" => Err(
                ScriptError::unsupported(&format!("Setting Legend.{name} from a macro")),
            ),
            _ => Err(ScriptError::no_member("Legend", name)),
        }
    }
}

// ------------------------------------------------------------------ axes

/// Whether axis `t` (`xlCategory` / `xlValue`) of a chart is its
/// horizontal one (else the vertical one).
fn axis_side(info: &ChartInfo, t: i64) -> Result<bool, ScriptError> {
    if !info.has_axes {
        return Err(failed("Axes", "_Chart", "this chart has no axes"));
    }
    // Bars: categories down the side, values across.
    let bars = info.x_value && !info.y_value;
    match t {
        XL_CATEGORY => Ok(!bars),
        XL_VALUE => Ok(bars),
        XL_SERIES_AXIS => Err(ScriptError::unsupported("The series axis of a 3-D chart")),
        _ => Err(ScriptError::runtime(5)),
    }
}

/// The smallest of 1, 2, 5 × 10ⁿ at least `raw`.
fn nice_unit(raw: f64) -> f64 {
    if raw <= 0.0 || !raw.is_finite() {
        return 1.0;
    }
    let base = 10f64.powf(raw.log10().floor());
    let f = raw / base;
    let n = if f <= 1.0 + 1e-9 {
        1.0
    } else if f <= 2.0 + 1e-9 {
        2.0
    } else if f <= 5.0 + 1e-9 {
        5.0
    } else {
        10.0
    };
    n * base
}

/// Excel's automatic bounds and major unit for values `lo..=hi`
/// (approximately: zero included, about 5 % headroom, a 1-2-5 unit).
fn auto_scale(lo: f64, hi: f64) -> (f64, f64, f64) {
    let lo = lo.min(0.0);
    let hi = hi.max(0.0);
    if hi <= lo {
        return (0.0, 1.0, 0.2);
    }
    let top = if hi > 0.0 { hi + (hi - lo) * 0.05 } else { hi };
    let bottom = if lo < 0.0 { lo - (hi - lo) * 0.05 } else { lo };
    let unit = nice_unit((top - bottom) / 10.0);
    (
        (bottom / unit).floor() * unit,
        (top / unit).ceil() * unit,
        unit,
    )
}

/// The numbers a series shows.
fn numbers_of(book: &Workbook, d: &Option<SeriesData>) -> Vec<f64> {
    match d {
        Some(SeriesData::Ref(f)) => book
            .chart_reference_now(f)
            .map(|r| {
                r.values
                    .iter()
                    .filter_map(|v| match v {
                        RefValue::Number(n) => Some(*n),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        Some(SeriesData::Numbers(n)) => n.clone(),
        _ => Vec::new(),
    }
}

pub struct AxisObj {
    s: Rc<Session>,
    key: DrawingKey,
    /// `xlCategory` or `xlValue`.
    axis: i64,
}

#[derive(Clone, Copy, PartialEq)]
enum Bound {
    Min,
    Max,
    Major,
    Minor,
}

impl AxisObj {
    /// Whether the axis is the horizontal one, and whether it shows values.
    fn side(&self) -> Result<(bool, bool, ChartInfo), ScriptError> {
        let info = chart_info(&self.s, self.key)?;
        let h = axis_side(&info, self.axis)?;
        let value = if h { info.x_value } else { info.y_value };
        Ok((h, value, info))
    }

    fn options(info: &ChartInfo, h: bool) -> &sheets_engine::AxisOptions {
        if h {
            &info.spec.x_axis
        } else {
            &info.spec.y_axis
        }
    }

    fn edit_options(
        &self,
        prop: &str,
        f: impl FnOnce(&mut sheets_engine::AxisOptions),
    ) -> Result<(), ScriptError> {
        let axis = self.axis;
        edit_spec(&self.s, self.key, prop, |spec, info| {
            let o = if axis_side(info, axis)? {
                &mut spec.x_axis
            } else {
                &mut spec.y_axis
            };
            f(o);
            Ok(())
        })
    }

    fn check_bound(&self, b: Bound, prop: &str) -> Result<(bool, ChartInfo), ScriptError> {
        let (h, value, info) = self.side()?;
        let date = Self::options(&info, h).axis_type == sheets_engine::AxisType::Date;
        let ok = value || (date && matches!(b, Bound::Major | Bound::Minor));
        if !ok {
            return Err(err1004(format!(
                "Unable to get the {prop} property of the Axis class (a category axis has no {prop})"
            )));
        }
        Ok((h, info))
    }

    fn bound(&self, b: Bound, prop: &str) -> Result<Value, ScriptError> {
        let (h, info) = self.check_bound(b, prop)?;
        let o = Self::options(&info, h);
        let set = match b {
            Bound::Min => o.min,
            Bound::Max => o.max,
            Bound::Major => o.major,
            Bound::Minor => o.minor,
        };
        if let Some(v) = set {
            return Ok(Value::Double(v));
        }
        let book = self.s.read()?;
        let list = book
            .script_series(self.key)
            .map_err(|e| failed(prop, "Axis", e))?;
        // A scatter chart's horizontal axis shows the X values.
        let xs = info.x_value && info.y_value && h;
        let all: Vec<f64> = list
            .iter()
            .flat_map(|s| numbers_of(&book, if xs { &s.cats } else { &s.vals }))
            .collect();
        let lo = all.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = all.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let (min, max, unit) = if all.is_empty() {
            (0.0, 1.0, 0.2)
        } else {
            auto_scale(lo, hi)
        };
        Ok(Value::Double(match b {
            Bound::Min => o.min.unwrap_or(min),
            Bound::Max => o.max.unwrap_or(max),
            Bound::Major => unit,
            Bound::Minor => unit / 5.0,
        }))
    }

    fn set_bound(&self, b: Bound, prop: &str, v: Option<f64>) -> Result<(), ScriptError> {
        self.check_bound(b, prop)?;
        if let Some(x) = v
            && matches!(b, Bound::Major | Bound::Minor)
            && x <= 0.0
        {
            return Err(unable(prop, "Axis", "a unit must be above zero"));
        }
        self.edit_options(prop, |o| match b {
            Bound::Min => o.min = v,
            Bound::Max => o.max = v,
            Bound::Major => o.major = v,
            Bound::Minor => o.minor = v,
        })
    }
}

fn bound_of(lower: &str) -> Option<(Bound, bool)> {
    Some(match lower {
        "minimumscale" => (Bound::Min, false),
        "maximumscale" => (Bound::Max, false),
        "majorunit" => (Bound::Major, false),
        "minorunit" => (Bound::Minor, false),
        "minimumscaleisauto" => (Bound::Min, true),
        "maximumscaleisauto" => (Bound::Max, true),
        "majorunitisauto" => (Bound::Major, true),
        "minorunitisauto" => (Bound::Minor, true),
        _ => return None,
    })
}

impl Dispatch for AxisObj {
    fn type_name(&self) -> String {
        "Axis".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        if let Some((b, auto)) = bound_of(&lower) {
            if auto {
                let (h, info) = self.check_bound(b, name)?;
                let o = Self::options(&info, h);
                let set = match b {
                    Bound::Min => o.min,
                    Bound::Max => o.max,
                    Bound::Major => o.major,
                    Bound::Minor => o.minor,
                };
                return Ok(Value::Boolean(set.is_none()));
            }
            return self.bound(b, name);
        }
        match lower.as_str() {
            "type" => Ok(Value::Long(self.axis as i32)),
            "axisgroup" => Ok(Value::Long(XL_PRIMARY as i32)),
            "hastitle" => {
                let (h, _, info) = self.side()?;
                let t = if h {
                    info.spec.x_title
                } else {
                    info.spec.y_title
                };
                Ok(Value::Boolean(!t.is_empty()))
            }
            "axistitle" => {
                self.side()?;
                Ok(obj(Rc::new(TitleObj {
                    s: self.s.clone(),
                    key: self.key,
                    axis: Some(self.axis),
                })))
            }
            "reverseplotorder" => {
                let (h, _, info) = self.side()?;
                Ok(Value::Boolean(Self::options(&info, h).reverse))
            }
            "scaletype" => {
                let (h, _, info) = self.side()?;
                Ok(Value::Long(if Self::options(&info, h).log_base.is_some() {
                    XL_SCALE_LOGARITHMIC as i32
                } else {
                    XL_SCALE_LINEAR as i32
                }))
            }
            "logbase" => {
                let (h, _, info) = self.side()?;
                Ok(Value::Double(
                    Self::options(&info, h).log_base.unwrap_or(10.0),
                ))
            }
            "ticklabels" => {
                self.side()?;
                Ok(obj(Rc::new(TickLabelsObj {
                    s: self.s.clone(),
                    key: self.key,
                    axis: self.axis,
                })))
            }
            "select" => Ok(Value::Empty),
            "parent" => Ok(chart_value(&self.s, self.key)),
            "hasmajorgridlines"
            | "hasminorgridlines"
            | "majorgridlines"
            | "minorgridlines"
            | "displayunit"
            | "displayunitcustom"
            | "hasdisplayunitlabel"
            | "crosses"
            | "crossesat"
            | "format"
            | "border"
            | "majortickmark"
            | "minortickmark"
            | "ticklabelposition"
            | "categorytype"
            | "baseunit"
            | "majorunitscale"
            | "minorunitscale"
            | "categorynames"
            | "delete"
            | "left"
            | "top"
            | "width"
            | "height" => Err(ScriptError::unsupported(&format!(
                "Axis.{name} from a macro"
            ))),
            _ => Err(ScriptError::no_member("Axis", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        if let Some((b, auto)) = bound_of(&lower) {
            if auto {
                if to_bool(&value)? {
                    return self.set_bound(b, name, None);
                }
                // Fixing the bound at what shows now.
                let now = to_f64(&self.bound(b, name)?)?;
                return self.set_bound(b, name, Some(now));
            }
            return self.set_bound(b, name, Some(to_f64(&value)?));
        }
        match lower.as_str() {
            "hastitle" => {
                let on = to_bool(&value)?;
                let axis = self.axis;
                edit_spec(&self.s, self.key, "HasTitle", |spec, info| {
                    let t = if axis_side(info, axis)? {
                        &mut spec.x_title
                    } else {
                        &mut spec.y_title
                    };
                    if !on {
                        t.clear();
                    } else if t.is_empty() {
                        *t = "Axis Title".to_owned();
                    }
                    Ok(())
                })
            }
            "reverseplotorder" => {
                let on = to_bool(&value)?;
                self.edit_options(name, |o| o.reverse = on)
            }
            "scaletype" => {
                let log = match to_i64(&value)? {
                    XL_SCALE_LOGARITHMIC => true,
                    XL_SCALE_LINEAR => false,
                    _ => return Err(ScriptError::runtime(5)),
                };
                let (_, value_axis, _) = self.side()?;
                if !value_axis {
                    return Err(unable(name, "Axis", "a category axis has no scale"));
                }
                self.edit_options(name, |o| {
                    o.log_base = if log { o.log_base.or(Some(10.0)) } else { None };
                })
            }
            "logbase" => {
                let b = to_f64(&value)?;
                if !(2.0..=1000.0).contains(&b) {
                    return Err(unable(name, "Axis", "the base must be 2 to 1000"));
                }
                self.edit_options(name, |o| o.log_base = Some(b))
            }
            "hasmajorgridlines" | "hasminorgridlines" | "displayunit" | "crosses" | "crossesat"
            | "majortickmark" | "minortickmark" | "ticklabelposition" | "categorytype"
            | "baseunit" | "majorunitscale" | "minorunitscale" | "categorynames" => Err(
                ScriptError::unsupported(&format!("Setting Axis.{name} from a macro")),
            ),
            _ => Err(ScriptError::no_member("Axis", name)),
        }
    }
}

/// An axis' `TickLabels` (its number format).
pub struct TickLabelsObj {
    s: Rc<Session>,
    key: DrawingKey,
    axis: i64,
}

impl Dispatch for TickLabelsObj {
    fn type_name(&self) -> String {
        "TickLabels".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let info = chart_info(&self.s, self.key)?;
        let h = axis_side(&info, self.axis)?;
        let o = AxisObj::options(&info, h);
        match name.to_ascii_lowercase().as_str() {
            "numberformat" => Ok(Value::String(
                o.num_fmt.clone().unwrap_or_else(|| "General".to_owned()),
            )),
            "numberformatlinked" => Ok(Value::Boolean(o.num_fmt.is_none())),
            "parent" => Ok(obj(Rc::new(AxisObj {
                s: self.s.clone(),
                key: self.key,
                axis: self.axis,
            }))),
            "font" | "orientation" | "offset" | "alignment" | "numberformatlocal" => Err(
                ScriptError::unsupported(&format!("TickLabels.{name} from a macro")),
            ),
            _ => Err(ScriptError::no_member("TickLabels", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let fmt = match name.to_ascii_lowercase().as_str() {
            "numberformat" => Some(to_str(&value)?),
            "numberformatlinked" => {
                if !to_bool(&value)? {
                    return Ok(());
                }
                None
            }
            "font" | "orientation" | "offset" | "numberformatlocal" => {
                return Err(ScriptError::unsupported(&format!(
                    "Setting TickLabels.{name} from a macro"
                )));
            }
            _ => return Err(ScriptError::no_member("TickLabels", name)),
        };
        let axis = AxisObj {
            s: self.s.clone(),
            key: self.key,
            axis: self.axis,
        };
        axis.edit_options("NumberFormat", |o| o.num_fmt = fmt)
    }
}

// ------------------------------------------------------------------ series

fn series_of(s: &Rc<Session>, key: DrawingKey) -> Result<Vec<ScriptSeries>, ScriptError> {
    s.read()?
        .script_series(key)
        .map_err(|e| failed("SeriesCollection", "_Chart", e))
}

fn put_series(
    s: &Rc<Session>,
    key: DrawingKey,
    list: &[ScriptSeries],
    prop: &str,
) -> Result<(), ScriptError> {
    s.write()?
        .script_set_series(key, list)
        .map_err(|e| unable(prop, "Series", e))
}

/// The name a series shows (`Series2` when it has none).
fn series_name(list: &[ScriptSeries], i: usize) -> String {
    let s = &list[i];
    if !s.shown_name.is_empty() {
        return s.shown_name.clone();
    }
    match &s.name {
        Some(SeriesData::Text(t)) if !t.is_empty() => t.clone(),
        _ => format!("Series{}", i + 1),
    }
}

/// A reference typed as text (`=Sheet1!$B$2:$B$5`, `B2:B5` on the chart's
/// sheet, a defined name), as a chart reference.
fn ref_text(s: &Rc<Session>, home: u32, text: &str) -> Result<String, ScriptError> {
    let t = text.trim().trim_start_matches('=').trim();
    match crate::sheet::resolve(s, home, t) {
        Ok(r) if r.areas.len() == 1 => {
            let idx = s.sheet_index(r.sheet)?;
            Ok(s.read()?.script_chart_ref(idx, r.first()))
        }
        _ if t.contains('!') => Ok(t.to_owned()),
        _ => Err(err1004(format!("{text} isn't a range a chart can show"))),
    }
}

/// A literal array as a formula writes it (`{1,2,3}`, `{"a","b"}`).
fn literal(text: &str) -> SeriesData {
    let inner = text.trim().trim_start_matches('{').trim_end_matches('}');
    let items: Vec<String> = split_top(inner, &[',', ';'])
        .into_iter()
        .map(|p| p.trim().to_owned())
        .collect();
    let numbers: Option<Vec<f64>> = items.iter().map(|p| p.parse().ok()).collect();
    match numbers {
        Some(n) => SeriesData::Numbers(n),
        None => SeriesData::Texts(items.iter().map(|p| unquote(p)).collect()),
    }
}

fn unquote(p: &str) -> String {
    let p = p.trim();
    match p.strip_prefix('"').and_then(|x| x.strip_suffix('"')) {
        Some(inner) => inner.replace("\"\"", "\""),
        None => p.to_owned(),
    }
}

/// `text` split at `seps` outside quotes, braces and brackets.
fn split_top<'a>(text: &'a str, seps: &[char]) -> Vec<&'a str> {
    let mut out = Vec::new();
    let (mut start, mut depth, mut quote, mut squote) = (0, 0i32, false, false);
    for (i, c) in text.char_indices() {
        match c {
            '"' if !squote => quote = !quote,
            '\'' if !quote => squote = !squote,
            '(' | '{' if !quote && !squote => depth += 1,
            ')' | '}' if !quote && !squote => depth -= 1,
            c if seps.contains(&c) && depth == 0 && !quote && !squote => {
                out.push(&text[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&text[start..]);
    out
}

/// What a macro assigns to `Values` / `XValues`: a range, a reference as
/// text, an array, a literal array as text, or one value.
fn series_data(
    s: &Rc<Session>,
    home: u32,
    v: &Value,
    numbers: bool,
) -> Result<SeriesData, ScriptError> {
    if let Some(r) = range_of(v) {
        if r.areas.len() > 1 {
            return Err(ScriptError::unsupported(
                "A series from several separate ranges",
            ));
        }
        let idx = s.sheet_index(r.sheet)?;
        return Ok(SeriesData::Ref(s.read()?.script_chart_ref(idx, r.first())));
    }
    let v = scalar(v)?;
    let items: Vec<Value> = match &v {
        Value::Array(a) => a.values().to_vec(),
        Value::String(t) if t.trim_start().starts_with('=') => {
            return Ok(SeriesData::Ref(ref_text(s, home, t)?));
        }
        Value::String(t) if t.trim_start().starts_with('{') => {
            let d = literal(t);
            if numbers && matches!(d, SeriesData::Texts(_)) {
                return Err(ScriptError::runtime(13));
            }
            return Ok(d);
        }
        other => vec![other.clone()],
    };
    let nums: Result<Vec<f64>, ScriptError> = items
        .iter()
        .map(|x| match x {
            Value::String(_) | Value::Empty | Value::Null | Value::Error(_) => {
                Err(ScriptError::runtime(13))
            }
            x => to_f64(x),
        })
        .collect();
    match nums {
        Ok(n) => Ok(SeriesData::Numbers(n)),
        Err(e) if numbers => Err(e),
        Err(_) => Ok(SeriesData::Texts(
            items.iter().map(to_str).collect::<Result<_, _>>()?,
        )),
    }
}

/// One argument of `=SERIES(...)` as a formula writes it.
fn formula_part(d: &Option<SeriesData>) -> String {
    match d {
        None => String::new(),
        Some(SeriesData::Ref(f)) => f.clone(),
        Some(SeriesData::Text(t)) => format!("\"{}\"", t.replace('"', "\"\"")),
        Some(SeriesData::Numbers(n)) => format!(
            "{{{}}}",
            n.iter()
                .map(|x| number_text(*x))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Some(SeriesData::Texts(t)) => format!(
            "{{{}}}",
            t.iter()
                .map(|x| format!("\"{}\"", x.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

/// One argument of `=SERIES(...)` read back (`None`: left empty).
fn parse_part(
    s: &Rc<Session>,
    home: u32,
    p: &str,
    name: bool,
) -> Result<Option<SeriesData>, ScriptError> {
    let p = p.trim();
    if p.is_empty() {
        return Ok(None);
    }
    if p.starts_with('"') {
        let t = unquote(p);
        return Ok(Some(if name {
            SeriesData::Text(t)
        } else {
            SeriesData::Texts(vec![t])
        }));
    }
    if p.starts_with('{') {
        return Ok(Some(literal(p)));
    }
    Ok(Some(SeriesData::Ref(ref_text(s, home, p)?)))
}

pub struct SeriesCollectionObj {
    s: Rc<Session>,
    key: DrawingKey,
}

impl SeriesCollectionObj {
    fn series(&self, i: usize) -> Value {
        obj(Rc::new(SeriesObj {
            s: self.s.clone(),
            key: self.key,
            i,
        }))
    }
}

impl Dispatch for SeriesCollectionObj {
    fn type_name(&self) -> String {
        "SeriesCollection".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" => {
                let list = series_of(&self.s, self.key)?;
                let i = match scalar(need(args, 0, "Index")?)? {
                    Value::String(n) => (0..list.len())
                        .find(|i| series_name(&list, *i).eq_ignore_ascii_case(&n))
                        .ok_or_else(|| ScriptError::runtime(9))?,
                    v => {
                        let i = to_i64(&v)?;
                        if i < 1 || i as usize > list.len() {
                            return Err(ScriptError::runtime(9));
                        }
                        i as usize - 1
                    }
                };
                Ok(self.series(i))
            }
            "count" => Ok(Value::Long(series_of(&self.s, self.key)?.len() as i32)),
            "newseries" => {
                let mut list = series_of(&self.s, self.key)?;
                list.push(ScriptSeries::default());
                put_series(&self.s, self.key, &list, "NewSeries")?;
                Ok(self.series(list.len() - 1))
            }
            "parent" => Ok(chart_value(&self.s, self.key)),
            "add" | "extend" | "paste" => Err(ScriptError::unsupported(&format!(
                "SeriesCollection.{name} from a macro (use NewSeries or SetSourceData)"
            ))),
            _ => Err(ScriptError::no_member("SeriesCollection", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        let n = series_of(&self.s, self.key)?.len();
        Ok((0..n).map(|i| self.series(i)).collect())
    }
}

/// Series `i` (0-based, in the chart part's order) of a chart.
pub struct SeriesObj {
    s: Rc<Session>,
    key: DrawingKey,
    i: usize,
}

impl SeriesObj {
    fn list(&self) -> Result<Vec<ScriptSeries>, ScriptError> {
        let list = series_of(&self.s, self.key)?;
        if self.i >= list.len() {
            return Err(ScriptError::new(
                424,
                "Object required (the series was deleted)",
            ));
        }
        Ok(list)
    }

    fn home(&self) -> u32 {
        match chart_sheet_of_key(self.key.sheet_id) {
            Some(_) => self.s.active.get(),
            None => self.key.sheet_id,
        }
    }

    fn change(
        &self,
        prop: &str,
        f: impl FnOnce(&mut Vec<ScriptSeries>) -> Result<(), ScriptError>,
    ) -> Result<(), ScriptError> {
        let mut list = self.list()?;
        f(&mut list)?;
        put_series(&self.s, self.key, &list, prop)
    }

    /// What `d` holds, as an array (`text`: texts as they are, else only
    /// numbers).
    fn values(&self, d: &Option<SeriesData>, text: bool) -> Result<Vec<Value>, ScriptError> {
        let book = self.s.read()?;
        Ok(match d {
            None => Vec::new(),
            Some(SeriesData::Ref(f)) => book
                .chart_reference_now(f)
                .map(|r| {
                    r.values
                        .iter()
                        .zip(&r.shown)
                        .map(|(v, shown)| match v {
                            RefValue::Number(n) => Value::Double(*n),
                            RefValue::Text(_) if text => Value::String(shown.clone()),
                            _ => Value::Empty,
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Some(SeriesData::Numbers(n)) => n.iter().map(|x| Value::Double(*x)).collect(),
            Some(SeriesData::Texts(t)) => t.iter().map(|x| Value::String(x.clone())).collect(),
            Some(SeriesData::Text(t)) => vec![Value::String(t.clone())],
        })
    }
}

impl Dispatch for SeriesObj {
    fn type_name(&self) -> String {
        "Series".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "name" => {
                let list = self.list()?;
                Ok(Value::String(series_name(&list, self.i)))
            }
            "values" => {
                let list = self.list()?;
                let v = self.values(&list[self.i].vals, false)?;
                Ok(Value::array(Array::from_vec(1, v)))
            }
            "xvalues" => {
                let list = self.list()?;
                let s = &list[self.i];
                let v = if s.cats.is_some() {
                    self.values(&s.cats, true)?
                } else {
                    // No categories: Excel numbers the points.
                    let n = self.values(&s.vals, false)?.len();
                    (1..=n).map(|k| Value::Double(k as f64)).collect()
                };
                Ok(Value::array(Array::from_vec(1, v)))
            }
            "formula" => {
                let list = self.list()?;
                let s = &list[self.i];
                Ok(Value::String(format!(
                    "=SERIES({},{},{},{})",
                    formula_part(&s.name),
                    formula_part(&s.cats),
                    formula_part(&s.vals),
                    self.i + 1
                )))
            }
            "plotorder" => {
                self.list()?;
                Ok(Value::Long(self.i as i32 + 1))
            }
            "axisgroup" => {
                self.list()?;
                Ok(Value::Long(XL_PRIMARY as i32))
            }
            "charttype" => Ok(Value::Long(
                type_code(&chart_info(&self.s, self.key)?) as i32
            )),
            "hasdatalabels" => {
                self.list()?;
                let info = chart_info(&self.s, self.key)?;
                Ok(Value::Boolean(
                    info.spec.labels.get(self.i).is_some_and(|l| l.value),
                ))
            }
            "delete" => {
                let i = self.i;
                self.change("Delete", |list| {
                    list.remove(i);
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            "select" => Ok(Value::Empty),
            "parent" => Ok(chart_value(&self.s, self.key)),
            "formular1c1"
            | "formulalocal"
            | "format"
            | "interior"
            | "border"
            | "points"
            | "datalabels"
            | "applydatalabels"
            | "markerstyle"
            | "markersize"
            | "markerbackgroundcolor"
            | "markerforegroundcolor"
            | "smooth"
            | "trendlines"
            | "errorbar"
            | "explosion"
            | "haslegendkey"
            | "invertifnegative"
            | "type"
            | "bubblesizes"
            | "barshape"
            | "shadow" => Err(ScriptError::unsupported(&format!(
                "Series.{name} from a macro"
            ))),
            _ => Err(ScriptError::no_member("Series", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        let i = self.i;
        let home = self.home();
        match lower.as_str() {
            "" | "name" => {
                let n = to_str(&value)?;
                let d = if n.trim_start().starts_with('=') {
                    SeriesData::Ref(ref_text(&self.s, home, &n)?)
                } else {
                    SeriesData::Text(n)
                };
                self.change("Name", |list| {
                    list[i].name = Some(d);
                    list[i].shown_name.clear();
                    Ok(())
                })
            }
            "values" => {
                let d = series_data(&self.s, home, &value, true)?;
                self.change("Values", |list| {
                    list[i].vals = Some(d);
                    Ok(())
                })
            }
            "xvalues" => {
                let d = series_data(&self.s, home, &value, false)?;
                self.change("XValues", |list| {
                    list[i].cats = Some(d);
                    Ok(())
                })
            }
            "formula" => {
                let f = to_str(&value)?;
                let body = f.trim().trim_start_matches('=').trim();
                let inner = body
                    .get(..7)
                    .filter(|p| p.eq_ignore_ascii_case("SERIES("))
                    .and_then(|_| body[7..].strip_suffix(')'))
                    .ok_or_else(|| unable("Formula", "Series", "it isn't =SERIES(...)"))?;
                let parts = split_top(inner, &[',']);
                if parts.len() < 3 || parts.len() > 5 {
                    return Err(unable(
                        "Formula",
                        "Series",
                        "=SERIES takes 3 or 4 arguments",
                    ));
                }
                let name = parse_part(&self.s, home, parts[0], true)?;
                let cats = parse_part(&self.s, home, parts[1], false)?;
                let vals = parse_part(&self.s, home, parts[2], false)?;
                if matches!(vals, Some(SeriesData::Texts(_) | SeriesData::Text(_))) {
                    return Err(ScriptError::runtime(13));
                }
                let order = match parts.get(3).map(|p| p.trim()) {
                    Some(p) if !p.is_empty() => Some(
                        p.parse::<usize>()
                            .map_err(|_| unable("Formula", "Series", "bad plot order"))?,
                    ),
                    _ => None,
                };
                self.change("Formula", |list| {
                    list[i] = ScriptSeries {
                        name,
                        shown_name: String::new(),
                        cats,
                        vals,
                    };
                    if let Some(o) = order {
                        let to = o.clamp(1, list.len()) - 1;
                        let s = list.remove(i);
                        list.insert(to, s);
                    }
                    Ok(())
                })
            }
            "plotorder" => {
                let o = to_i64(&value)?;
                self.change("PlotOrder", |list| {
                    if o < 1 || o as usize > list.len() {
                        return Err(unable("PlotOrder", "Series", "no such position"));
                    }
                    let s = list.remove(i);
                    list.insert(o as usize - 1, s);
                    Ok(())
                })
            }
            "hasdatalabels" => {
                let on = to_bool(&value)?;
                self.list()?;
                edit_spec(&self.s, self.key, "HasDataLabels", |spec, _| {
                    if spec.labels.len() <= i {
                        spec.labels.resize(i + 1, Default::default());
                    }
                    if on {
                        spec.labels[i].value = true;
                    } else {
                        spec.labels[i] = Default::default();
                    }
                    Ok(())
                })
            }
            "formular1c1"
            | "formulalocal"
            | "charttype"
            | "axisgroup"
            | "markerstyle"
            | "markersize"
            | "markerbackgroundcolor"
            | "markerforegroundcolor"
            | "smooth"
            | "explosion"
            | "haslegendkey"
            | "invertifnegative"
            | "bubblesizes"
            | "barshape"
            | "shadow"
            | "type" => Err(ScriptError::unsupported(&format!(
                "Setting Series.{name} from a macro"
            ))),
            _ => Err(ScriptError::no_member("Series", name)),
        }
    }
}

// ------------------------------------------------------------------ Charts (chart sheets)

pub struct ChartsObj {
    s: Rc<Session>,
}

impl ChartsObj {
    fn ks(&self) -> Result<Vec<usize>, ScriptError> {
        Ok(self.s.read()?.script_chart_sheets())
    }

    fn chart(&self, k: usize) -> Result<Value, ScriptError> {
        let key = chart_sheet_key(&*self.s.read()?, k).unwrap_or(DrawingKey {
            sheet_id: chart_sheet_key_id(k),
            index: 0,
        });
        Ok(chart_value(&self.s, key))
    }

    fn item(&self, key: &Value) -> Result<Value, ScriptError> {
        let ks = self.ks()?;
        let k = match scalar(key)? {
            Value::String(n) => {
                let book = self.s.read()?;
                ks.into_iter()
                    .find(|k| {
                        book.chart_sheet_name(*k)
                            .is_some_and(|x| x.eq_ignore_ascii_case(&n))
                    })
                    .ok_or_else(|| ScriptError::runtime(9))?
            }
            Value::Array(_) => {
                return Err(ScriptError::unsupported("Selecting several chart sheets"));
            }
            v => {
                let i = to_i64(&v)?;
                if i < 1 || i as usize > ks.len() {
                    return Err(ScriptError::runtime(9));
                }
                ks[i as usize - 1]
            }
        };
        self.chart(k)
    }

    /// The tab a `Before:=` / `After:=` sheet or chart stands for.
    fn tab_of(&self, v: &Value) -> Result<Tab, ScriptError> {
        let o = v.as_object().ok_or_else(|| ScriptError::runtime(424))?;
        let any = o.as_any().ok_or_else(|| ScriptError::runtime(13))?;
        if let Some(w) = any.downcast_ref::<SheetObj>() {
            return Ok(Tab::Sheet(self.s.sheet_index(w.id)?));
        }
        if let Some(c) = any.downcast_ref::<ChartObj>()
            && let Some(k) = c.sheet_k()
        {
            return Ok(Tab::Chart(k));
        }
        Err(ScriptError::runtime(13))
    }

    fn add(&self, args: &[Arg]) -> Result<Value, ScriptError> {
        let before = if let Some(b) = arg(args, 0, "Before") {
            Some(self.tab_of(b)?)
        } else if let Some(a) = arg(args, 1, "After") {
            let t = self.tab_of(a)?;
            let tabs = self.s.read()?.tabs();
            tabs.iter()
                .position(|x| *x == t)
                .and_then(|p| tabs.get(p + 1).copied())
        } else {
            let active = self.s.sheet_index(self.s.active.get())?;
            Some(Tab::Sheet(active))
        };
        let count = arg_i64(args, 2, "Count")?.unwrap_or(1).max(1);
        let data = selection_data(&self.s, self.s.active.get())?;
        let mut last = None;
        for _ in 0..count {
            let k = self
                .s
                .write()?
                .script_add_chart_sheet(
                    None,
                    before,
                    ChartType::Column,
                    false,
                    data.map(|(ds, d)| (ds, d, None)),
                )
                .map_err(|e| failed("Add", "Sheets", e))?;
            last = Some(k);
        }
        let k = last.ok_or_else(|| ScriptError::runtime(5))?;
        let key = chart_sheet_key(&*self.s.read()?, k).ok_or_else(gone)?;
        set_active(&self.s, key);
        Ok(chart_value(&self.s, key))
    }
}

impl Dispatch for ChartsObj {
    fn type_name(&self) -> String {
        "Sheets".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "item" => self.item(need(args, 0, "Index")?),
            "count" => Ok(Value::Long(self.ks()?.len() as i32)),
            "add" | "add2" => self.add(args),
            "delete" => {
                for k in self.ks()? {
                    self.s
                        .write()?
                        .delete_chart_sheet(k)
                        .map_err(|e| failed("Delete", "Sheets", e))?;
                }
                Ok(Value::Empty)
            }
            "select" => Ok(Value::Empty),
            "parent" => Ok(obj(Rc::new(crate::app::WorkbookObj::new(&self.s)))),
            "application" => Ok(obj(Rc::new(crate::app::Application::new(&self.s)))),
            "copy" | "move" | "printout" | "printpreview" | "visible" => Err(
                ScriptError::unsupported(&format!("Charts.{name} from a macro")),
            ),
            _ => Err(ScriptError::no_member("Sheets", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        self.ks()?.into_iter().map(|k| self.chart(k)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_is_well_formed() {
        let png = png_rgb(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        // IHDR: 13 bytes, its CRC as the PNG spec's example tools compute.
        assert_eq!(&png[8..16], b"\x00\x00\x00\x0dIHDR");
        assert_eq!(crc32(&[&b"IEND"[..]]), 0xAE42_6082);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert!(png.ends_with(b"IEND\xae\x42\x60\x82"));
    }

    #[test]
    fn literals_and_scales() {
        assert_eq!(
            literal("{1,2.5,3}"),
            SeriesData::Numbers(vec![1.0, 2.5, 3.0])
        );
        assert_eq!(
            literal("{\"a\",\"b,c\"}"),
            SeriesData::Texts(vec!["a".into(), "b,c".into()])
        );
        assert_eq!(
            split_top("\"x\",(A1,B1),Sheet1!$B$2:$B$5,1", &[',']),
            vec!["\"x\"", "(A1,B1)", "Sheet1!$B$2:$B$5", "1"]
        );
        assert_eq!(auto_scale(10.0, 170.0), (0.0, 180.0, 20.0));
        assert_eq!(nice_unit(0.3), 0.5);
        assert_eq!(kind_of(-4169).unwrap(), (ChartType::Scatter, false));
        assert_eq!(kind_of(-4120).unwrap_err().number, 445);
    }
}
