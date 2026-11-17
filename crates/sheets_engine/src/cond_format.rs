//! Conditional formatting, as Excel shows it (display only).
//!
//! IronCalc's reader parses the rules (`<conditionalFormatting>`, the x14
//! data bars and icon sets of `extLst`) and the differential formats
//! (`<dxfs>`) into its model; on save the original markup is carried as it
//! was (`preserve`). IronCalc's own evaluator runs every rule over every cell
//! after each recalculation, so it is turned off (vendor patch) and rules are
//! evaluated here, lazily: only for the cells the grid draws, with what a rule
//! needs from its whole range (minimum and maximum, sorted values for
//! percentiles and top/bottom N, the average, value counts for duplicates)
//! worked out once and kept across edits that can't change them: an edit,
//! undo or redo drops the aggregates of rules whose range it touched (from
//! IronCalc's record of changes, `kantor_take_changes`); an evaluation
//! drops those that read formula results (all of them when dynamic arrays
//! could spill anywhere); a recalculation (F9) or a workbook-wide change
//! (sheets, names, tables) starts from scratch. What each cell shows is
//! worked out again after any change.
//!
//! Rules apply in priority order (Excel's `priority="1"` first). Formats of
//! several true rules combine, the higher priority winning a property both
//! set; `stopIfTrue` ends the walk for that cell. A colour scale sets the
//! fill (if no higher rule did); the first data bar and the first icon set
//! win. Formulas (`expression` rules, `cellIs` operands, `formula` cfvos) are
//! relative to the top-left cell of the rule's range and read stored values
//! (a formula cell is never recalculated for painting).

use std::collections::HashMap;
use std::sync::Arc;

use ironcalc_base::cell::CellValue;
use ironcalc_base::cf_types::{
    CfRule, Cfvo, ColorScaleThreshold, Icon as IcIcon, IconThreshold, PeriodType, TextOperator,
    ValueOperator,
};
use ironcalc_base::types::{Cell, CellType, Dxf, FormulaValue, SpillValue};
use ironcalc_base::{CfFormula, CfValue, Model};

use crate::format::{CellStyle, Rgb, parse_rgb};
use crate::numfmt::{self, Display};
use crate::{CellRange, MAX_COLS, MAX_ROWS, Workbook};

/// Cells kept evaluated at most; beyond this the cell cache starts over (a
/// long scroll through a big sheet shouldn't keep every cell it passed).
const MAX_CACHED_CELLS: usize = 100_000;

/// What conditional formatting changes about one cell.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Conditional {
    /// The cell's style with the rules' formats applied (fill, font colour,
    /// bold, italic, underline, strikethrough, number format), when one
    /// applies.
    pub style: Option<Arc<CellStyle>>,
    /// The value shown with a rule's number format.
    pub display: Option<Display>,
    pub bar: Option<DataBar>,
    pub icon: Option<CfIcon>,
    /// `false` when a data bar or icon set shows only the bar / icon.
    pub show_value: bool,
    /// Border edges the rules' formats draw (over the cell's own).
    pub borders: crate::format::Borders,
}

/// A data bar, in fractions of the cell's width.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DataBar {
    pub color: Rgb,
    /// Fades to white towards its end (Excel's gradient fill).
    pub gradient: bool,
    /// Where the bar starts and ends, `0.0 <= from <= to <= 1.0`.
    pub from: f32,
    pub to: f32,
    /// The zero axis, when the range has negative and positive values.
    pub axis: Option<f32>,
    /// The bar grows leftwards (a negative value's bar, or any bar of a
    /// right-to-left rule): its gradient fades towards the left.
    pub negative: bool,
    /// Its border (Excel 2010's "Solid Border").
    pub border: Option<Rgb>,
    /// The axis' colour (black when `None`).
    pub axis_color: Option<Rgb>,
}

/// A data bar's look with its colours resolved (see `cf_x14::BarLook`).
#[derive(Debug, Clone, Copy, PartialEq)]
struct BarStyle {
    min_len: f32,
    max_len: f32,
    axis: crate::cf_x14::Axis,
    border: Option<Rgb>,
    negative_border: Option<Rgb>,
    negative_fill: Option<Rgb>,
    axis_color: Option<Rgb>,
    /// Bars grow from the right (Excel's "Right-to-Left" direction).
    rtl: bool,
}

impl BarStyle {
    const KANTOR: BarStyle = BarStyle {
        min_len: 0.0,
        max_len: 1.0,
        axis: crate::cf_x14::Axis::Automatic,
        border: None,
        negative_border: None,
        negative_fill: None,
        axis_color: None,
        rtl: false,
    };
}

/// An icon of an icon set, drawn as a small vector shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CfIcon {
    pub shape: IconShape,
    pub color: Rgb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconShape {
    ArrowUp,
    ArrowDown,
    ArrowRight,
    /// Diagonal arrows of the 4- and 5-arrow sets.
    ArrowUpRight,
    ArrowDownRight,
    Circle,
    TriangleUp,
    TriangleDown,
    /// The yellow bar of "3 Triangles" (a short horizontal bar).
    Dash,
    Diamond,
    Flag,
    Check,
    Cross,
    Exclamation,
    Heart,
    ThumbsUp,
    ThumbsDown,
    /// A star filled `filled` out of `of` steps (0 empty, `of` full).
    Star {
        filled: u8,
        of: u8,
    },
    /// A circle filled by quarters (5 Quarters): `filled` of `of`.
    Pie {
        filled: u8,
        of: u8,
    },
    /// Signal bars or boxes (Ratings, Boxes): `lit` of `of`.
    Bars {
        lit: u8,
        of: u8,
    },
}

/// Conditional formatting state, for the workbook state `key`.
#[derive(Default)]
pub(crate) struct CondState {
    key: (u64, u64),
    /// Taken the workbook's state into account at least once (`key` and
    /// `evaluations` are meaningful).
    started: bool,
    /// The model's evaluation count at `key`.
    evaluations: u64,
    sheets: HashMap<u32, Arc<Vec<Rule>>>,
    /// Per rule (sheet, index in `sheets`): its range-wide numbers.
    aggregates: HashMap<(u32, usize), Arc<Aggregate>>,
    cells: HashMap<(u32, u32, u32), Option<Arc<Conditional>>>,
    today: Option<f64>,
}

/// A compiled rule.
struct Rule {
    areas: Vec<CellRange>,
    bounds: CellRange,
    /// 1-based anchor the formulas are relative to.
    anchor: (i32, i32),
    kind: Kind,
    dxf: Option<Arc<Dxf>>,
    stop: bool,
}

enum Kind {
    CellIs {
        op: ValueOperator,
        a: Option<CfFormula>,
        b: Option<CfFormula>,
    },
    Expression(Option<CfFormula>),
    Text {
        op: TextOperator,
        text: String,
    },
    /// A date occurring in a period; `Between` / `NotBetween` with their
    /// first and last dates (text for DATEVALUE).
    Period(PeriodType, Option<String>, Option<String>),
    Duplicate(bool),
    Blanks(bool),
    Errors(bool),
    Average(bool),
    Top {
        rank: u32,
        percent: bool,
        bottom: bool,
    },
    ColorScale(Vec<(Cfvo, Option<Rgb>, Option<CfFormula>)>),
    DataBar {
        min: Option<Cfvo>,
        max: Option<Cfvo>,
        formulas: [Option<CfFormula>; 2],
        color: Rgb,
        negative: Rgb,
        gradient: bool,
        show_value: bool,
        style: BarStyle,
    },
    Icons {
        steps: Vec<IconStep>,
        show_value: bool,
    },
}

struct IconStep {
    cfvo: Cfvo,
    formula: Option<CfFormula>,
    gte: bool,
    icon: CfIcon,
}

/// What a rule needs from its whole range.
#[derive(Default)]
struct Aggregate {
    /// The numbers in the range, sorted.
    sorted: Vec<f64>,
    sum: f64,
    /// How often each value occurs (text lower-cased), for duplicates.
    counts: HashMap<Key, u32>,
    /// Lower-cased texts of the range, numbered.
    texts: HashMap<String, u32>,
    /// Resolved thresholds of a colour scale, data bar or icon set.
    stops: Vec<f64>,
    /// Top/bottom N: the value a cell must reach.
    cutoff: Option<f64>,
    /// The range has formula results (an evaluation can change them).
    formulas: bool,
    /// Thresholds come from formulas, which can read any cell.
    external: bool,
}

#[derive(Hash, PartialEq, Eq)]
enum Key {
    Number(u64),
    /// Text, by its lower-cased form's number in [`Aggregate::texts`].
    Text(u32),
    Bool(bool),
}

impl Aggregate {
    fn text_id(&mut self, lower: String) -> u32 {
        let next = self.texts.len() as u32;
        *self.texts.entry(lower).or_insert(next)
    }
}

/// A cell's stored value.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    Error,
}

pub(crate) fn cell_value(model: &Model, sheet: u32, row: i32, col: i32) -> Value {
    let Ok(ws) = model.workbook.worksheet(sheet) else {
        return Value::Empty;
    };
    let Some(cell) = ws.cell(row, col) else {
        return Value::Empty;
    };
    if cell.get_type() == CellType::ErrorValue {
        return Value::Error;
    }
    match cell.value(&model.workbook.shared_strings, numfmt::language()) {
        CellValue::None => Value::Empty,
        CellValue::Number(n) => Value::Number(n),
        CellValue::String(s) => Value::Text(s),
        CellValue::Boolean(b) => Value::Bool(b),
    }
}

impl From<CfValue> for Value {
    fn from(v: CfValue) -> Self {
        match v {
            CfValue::Number(n) => Value::Number(n),
            CfValue::Text(s) => Value::Text(s),
            CfValue::Bool(b) => Value::Bool(b),
            CfValue::Empty => Value::Empty,
            CfValue::Error | CfValue::Other => Value::Error,
        }
    }
}

/// `A1:B3 D4`, whole columns (`A:A`) and rows (`3:5`) included.
pub(crate) fn parse_sqref(sqref: &str) -> Vec<CellRange> {
    sqref
        .split_whitespace()
        .filter_map(|part| {
            let part = part.rsplit('!').next().unwrap_or(part);
            let a = crate::shift::parse_range(&part.to_ascii_uppercase())?.area();
            Some(CellRange {
                top: a.top,
                left: a.left,
                bottom: a.bottom.min(MAX_ROWS - 1),
                right: a.right.min(MAX_COLS - 1),
            })
        })
        .collect()
}

fn rgb(model: &Model, c: &ironcalc_base::types::Color) -> Option<Rgb> {
    match c {
        ironcalc_base::types::Color::None => None,
        c => parse_rgb(&c.to_rgb(&model.workbook.theme).to_ascii_uppercase()),
    }
}

/// Excel's comparison of two values: numbers < text < booleans, text
/// without regard to case. `None` when an error is involved.
pub(crate) fn compare(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    use std::cmp::Ordering;
    let rank = |v: &Value| match v {
        Value::Number(_) | Value::Empty => 0,
        Value::Text(_) => 1,
        Value::Bool(_) => 2,
        Value::Error => 3,
    };
    // An empty cell is 0, "" or FALSE, whatever it is compared with.
    let blank_as = |v: &Value, other: &Value| match (v, other) {
        (Value::Empty, Value::Text(_)) => Value::Text(String::new()),
        (Value::Empty, Value::Bool(_)) => Value::Bool(false),
        (Value::Empty, _) => Value::Number(0.0),
        (v, _) => v.clone(),
    };
    let (a, b) = (blank_as(a, b), blank_as(b, a));
    if matches!(a, Value::Error) || matches!(b, Value::Error) {
        return None;
    }
    match (&a, &b) {
        (Value::Number(x), Value::Number(y)) => {
            if (x - y).abs() <= 1e-15 * x.abs().max(y.abs()) {
                Some(Ordering::Equal)
            } else {
                x.partial_cmp(y)
            }
        }
        (Value::Text(x), Value::Text(y)) => Some(x.to_lowercase().cmp(&y.to_lowercase())),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        _ => Some(rank(&a).cmp(&rank(&b))),
    }
}

/// The text `SEARCH` sees for a value.
fn as_text(v: &Value) -> Option<String> {
    match v {
        Value::Empty => Some(String::new()),
        Value::Number(n) => Some(general_number(*n)),
        Value::Text(s) => Some(s.clone()),
        Value::Bool(b) => Some(if *b { "TRUE" } else { "FALSE" }.to_owned()),
        Value::Error => None,
    }
}

pub(crate) fn general_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n}");
        s
    }
}

pub(crate) fn truthy(v: &Value) -> bool {
    match v {
        Value::Number(n) => *n != 0.0,
        Value::Bool(b) => *b,
        _ => false,
    }
}

/// PERCENTILE.INC of sorted values.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let p = (p / 100.0).clamp(0.0, 1.0);
    let pos = p * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

fn lerp(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let ch = |c: Rgb, s: u32| f64::from((c >> s) & 0xff);
    let mix = |s: u32| {
        let v = ch(a, s) + (ch(b, s) - ch(a, s)) * t.clamp(0.0, 1.0);
        (v.round() as u32) << s
    };
    mix(16) | mix(8) | mix(0)
}

/// [`icon_of`] for the filters and sorts by icon.
pub(crate) fn icon_of_pub(
    model: &Model,
    icon: &IcIcon,
    color: &ironcalc_base::types::Color,
) -> CfIcon {
    icon_of(model, icon, color)
}

fn icon_of(model: &Model, icon: &IcIcon, color: &ironcalc_base::types::Color) -> CfIcon {
    let shape = match icon {
        IcIcon::ArrowUp => IconShape::ArrowUp,
        IcIcon::ArrowRight => IconShape::ArrowRight,
        IcIcon::ArrowDown => IconShape::ArrowDown,
        IcIcon::ArrowAngleUp => IconShape::ArrowUpRight,
        IcIcon::ArrowAngleDown => IconShape::ArrowDownRight,
        IcIcon::Circle => IconShape::Circle,
        IcIcon::TriangleUp | IcIcon::TriangleUpFilled => IconShape::TriangleUp,
        IcIcon::TriangleDown | IcIcon::TriangleDownFilled => IconShape::TriangleDown,
        IcIcon::FlatRectangle => IconShape::Dash,
        IcIcon::Rhombus => IconShape::Diamond,
        IcIcon::Flag => IconShape::Flag,
        IcIcon::Check => IconShape::Check,
        IcIcon::Cross => IconShape::Cross,
        IcIcon::Exclamation => IconShape::Exclamation,
        IcIcon::Star => IconShape::Star { filled: 2, of: 2 },
        IcIcon::Heart => IconShape::Heart,
        IcIcon::ThumbsUp => IconShape::ThumbsUp,
        IcIcon::ThumbsDown => IconShape::ThumbsDown,
    };
    CfIcon {
        shape,
        color: rgb(model, color).unwrap_or(0x80_80_80),
    }
}

/// The dxf properties a cell got so far (higher priority first).
#[derive(Default)]
struct Overlay {
    fill: Option<Rgb>,
    font_color: Option<Rgb>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    strike: Option<bool>,
    num_fmt: Option<String>,
    borders: crate::format::Borders,
}

impl Overlay {
    fn add(&mut self, model: &Model, dxf: &Dxf) {
        if let Some(f) = &dxf.font {
            if self.font_color.is_none() {
                self.font_color = rgb(model, &f.color);
            }
            self.bold = self.bold.or(f.b);
            self.italic = self.italic.or(f.i);
            self.underline = self.underline.or(f.u);
            self.strike = self.strike.or(f.strike);
        }
        if let Some(fill) = &dxf.fill
            && self.fill.is_none()
        {
            self.fill = rgb(model, &fill.color);
        }
        if let Some(b) = &dxf.border {
            let line = |item: &Option<ironcalc_base::types::BorderItem>| {
                item.as_ref().map(|i| crate::format::BorderLine {
                    style: crate::format::LineStyle::from_ironcalc(&i.style),
                    color: rgb(model, &i.color),
                })
            };
            let o = &mut self.borders;
            o.left = o.left.or(line(&b.left));
            o.right = o.right.or(line(&b.right));
            o.top = o.top.or(line(&b.top));
            o.bottom = o.bottom.or(line(&b.bottom));
        }
        if let Some(n) = &dxf.num_fmt
            && self.num_fmt.is_none()
            && !n.format_code.is_empty()
        {
            self.num_fmt = Some(n.format_code.clone());
        }
    }

    fn is_empty(&self) -> bool {
        self.fill.is_none()
            && self.font_color.is_none()
            && self.bold.is_none()
            && self.italic.is_none()
            && self.underline.is_none()
            && self.strike.is_none()
            && self.num_fmt.is_none()
            && self.borders == crate::format::Borders::default()
    }
}

impl Workbook {
    /// Whether `sheet` has conditional formatting rules.
    pub fn has_conditional_formats(&self, sheet: u32) -> bool {
        self.model
            .get_model()
            .workbook
            .worksheet(sheet)
            .is_ok_and(|ws| !ws.conditional_formatting.is_empty())
    }

    /// What conditional formatting does to a cell (0-based), `None` if no
    /// rule applies. Evaluated on first use per workbook state.
    pub fn conditional(&mut self, sheet: u32, row: u32, col: u32) -> Option<Arc<Conditional>> {
        if !self.has_conditional_formats(sheet) {
            return None;
        }
        let key = self.revision();
        if self.cond.key != key || !self.cond.started {
            self.cond_sync(key);
        }
        if let Some(hit) = self.cond.cells.get(&(sheet, row, col)) {
            return hit.clone();
        }
        let result = self.evaluate_conditional(sheet, row, col).map(Arc::new);
        if self.cond.cells.len() >= MAX_CACHED_CELLS {
            self.cond.cells.clear();
        }
        self.cond.cells.insert((sheet, row, col), result.clone());
        result
    }

    /// Bring the state up to `key`: keep what the changes since the last
    /// state can't have changed (see the module docs).
    fn cond_sync(&mut self, key: (u64, u64)) {
        let changes = self.model.kantor_take_changes();
        let model = self.model.get_model();
        let evaluations = model.kantor_evaluations();
        let evaluated = evaluations != self.cond.evaluations;
        let dynamic = model.kantor_has_dynamic_arrays();
        let state = &mut self.cond;
        if !state.started || key.1 != state.key.1 || changes.all || (evaluated && dynamic) {
            *state = CondState::default();
        } else {
            state.cells.clear();
            state.today = None;
            state
                .sheets
                .retain(|sheet, _| !changes.sheet_changed(*sheet));
            let values = changes.any_values();
            let sheets = &state.sheets;
            state.aggregates.retain(|&(sheet, index), agg| {
                let Some(rule) = sheets.get(&sheet).and_then(|r| r.get(index)) else {
                    return false;
                };
                if (agg.formulas && evaluated) || (agg.external && (values || evaluated)) {
                    return false;
                }
                !rule.areas.iter().any(|a| {
                    changes.touches(
                        sheet,
                        a.top as i32 + 1,
                        a.left as i32 + 1,
                        a.bottom as i32 + 1,
                        a.right as i32 + 1,
                    )
                })
            });
        }
        state.key = key;
        state.started = true;
        state.evaluations = evaluations;
    }

    fn cond_rules(&mut self, sheet: u32) -> Arc<Vec<Rule>> {
        if let Some(r) = self.cond.sheets.get(&sheet) {
            return r.clone();
        }
        let rules = Arc::new(self.compile_rules(sheet));
        self.cond.sheets.insert(sheet, rules.clone());
        rules
    }

    fn compile_rules(&mut self, sheet: u32) -> Vec<Rule> {
        let (mut list, dxfs) = {
            let model = self.model.get_model();
            let Ok(ws) = model.workbook.worksheet(sheet) else {
                return Vec::new();
            };
            (
                ws.conditional_formatting.clone(),
                model.workbook.styles.dxfs.clone(),
            )
        };
        // IronCalc stores Excel's priorities reversed (highest number wins).
        list.sort_by_key(|cf| std::cmp::Reverse(cf.priority));
        let mut rules = Vec::with_capacity(list.len());
        for cf in list {
            let areas = parse_sqref(&cf.range);
            let Some(first) = areas.first() else {
                continue;
            };
            let mut bounds = *first;
            for a in &areas {
                bounds.top = bounds.top.min(a.top);
                bounds.left = bounds.left.min(a.left);
                bounds.bottom = bounds.bottom.max(a.bottom);
                bounds.right = bounds.right.max(a.right);
            }
            let anchor = (bounds.top as i32 + 1, bounds.left as i32 + 1);
            let dxf_of = |id: u32| dxfs.get(id as usize).cloned().map(Arc::new);
            // A data bar's look from the file while it is as opened.
            let bar_style = matches!(cf.cf_rule, CfRule::DataBar { .. }).then(|| {
                let look = self.bar_look_of(self.sheet_id(sheet), &cf.range, &cf.cf_rule);
                let model = self.model.get_model();
                let c = |c: &Option<ironcalc_base::types::Color>| {
                    c.as_ref().and_then(|c| rgb(model, c))
                };
                BarStyle {
                    min_len: look.min_len,
                    max_len: look.max_len,
                    axis: look.axis,
                    border: c(&look.border),
                    negative_border: c(&look.negative_border),
                    negative_fill: c(&look.negative_fill),
                    axis_color: c(&look.axis_color),
                    rtl: look.direction == crate::cf_x14::Direction::RightToLeft,
                }
            });
            let (kind, dxf, stop) = match cf.cf_rule {
                CfRule::CellIs {
                    operator,
                    formula,
                    formula2,
                    dxf_id,
                    stop_if_true,
                } => (
                    Kind::CellIs {
                        op: operator,
                        a: self.model.cf_parse(sheet, &formula, anchor.0, anchor.1),
                        b: formula2
                            .as_deref()
                            .and_then(|f| self.model.cf_parse(sheet, f, anchor.0, anchor.1)),
                    },
                    dxf_of(dxf_id),
                    stop_if_true,
                ),
                CfRule::Formula {
                    formula,
                    dxf_id,
                    stop_if_true,
                } => (
                    Kind::Expression(self.model.cf_parse(sheet, &formula, anchor.0, anchor.1)),
                    dxf_of(dxf_id),
                    stop_if_true,
                ),
                CfRule::Text {
                    operator,
                    value,
                    dxf_id,
                    stop_if_true,
                } => (
                    Kind::Text {
                        op: operator,
                        text: value.to_lowercase(),
                    },
                    dxf_of(dxf_id),
                    stop_if_true,
                ),
                CfRule::TimePeriod {
                    time_period,
                    date1,
                    date2,
                    dxf_id,
                    stop_if_true,
                } => (
                    Kind::Period(time_period, date1, date2),
                    dxf_of(dxf_id),
                    stop_if_true,
                ),
                CfRule::DuplicateValues {
                    dxf_id,
                    stop_if_true,
                } => (Kind::Duplicate(true), dxf_of(dxf_id), stop_if_true),
                CfRule::UniqueValues {
                    dxf_id,
                    stop_if_true,
                } => (Kind::Duplicate(false), dxf_of(dxf_id), stop_if_true),
                CfRule::Blanks {
                    dxf_id,
                    stop_if_true,
                } => (Kind::Blanks(true), dxf_of(dxf_id), stop_if_true),
                CfRule::NotBlanks {
                    dxf_id,
                    stop_if_true,
                } => (Kind::Blanks(false), dxf_of(dxf_id), stop_if_true),
                CfRule::Errors {
                    dxf_id,
                    stop_if_true,
                } => (Kind::Errors(true), dxf_of(dxf_id), stop_if_true),
                CfRule::NoErrors {
                    dxf_id,
                    stop_if_true,
                } => (Kind::Errors(false), dxf_of(dxf_id), stop_if_true),
                CfRule::AboveAverage {
                    dxf_id,
                    stop_if_true,
                } => (Kind::Average(true), dxf_of(dxf_id), stop_if_true),
                CfRule::BelowAverage {
                    dxf_id,
                    stop_if_true,
                } => (Kind::Average(false), dxf_of(dxf_id), stop_if_true),
                CfRule::Top10 {
                    rank,
                    percent,
                    dxf_id,
                    stop_if_true,
                } => (
                    Kind::Top {
                        rank,
                        percent,
                        bottom: false,
                    },
                    dxf_of(dxf_id),
                    stop_if_true,
                ),
                CfRule::Bottom10 {
                    rank,
                    percent,
                    dxf_id,
                    stop_if_true,
                } => (
                    Kind::Top {
                        rank,
                        percent,
                        bottom: true,
                    },
                    dxf_of(dxf_id),
                    stop_if_true,
                ),
                CfRule::ColorScale { thresholds } => {
                    let model = self.model.get_model();
                    let colors: Vec<Option<Rgb>> = thresholds
                        .iter()
                        .map(|t: &ColorScaleThreshold| rgb(model, &t.color))
                        .collect();
                    let steps = thresholds
                        .into_iter()
                        .zip(colors)
                        .map(|(t, c)| {
                            let f = match &t.cfvo {
                                Cfvo::Formula(f) => {
                                    self.model.cf_parse(sheet, f, anchor.0, anchor.1)
                                }
                                _ => None,
                            };
                            (t.cfvo, c, f)
                        })
                        .collect();
                    (Kind::ColorScale(steps), None, false)
                }
                CfRule::DataBar {
                    min,
                    max,
                    positive_color,
                    negative_color,
                    is_gradient,
                    show_value,
                } => {
                    let model = self.model.get_model();
                    let color = rgb(model, &positive_color).unwrap_or(0x63_8E_C6);
                    let negative = rgb(model, &negative_color).unwrap_or(0xFF_00_00);
                    let mut f = |c: &Option<Cfvo>| match c {
                        Some(Cfvo::Formula(f)) => self.model.cf_parse(sheet, f, anchor.0, anchor.1),
                        _ => None,
                    };
                    let formulas = [f(&min), f(&max)];
                    (
                        Kind::DataBar {
                            min,
                            max,
                            formulas,
                            color,
                            negative,
                            gradient: is_gradient,
                            show_value,
                            style: bar_style.unwrap_or(BarStyle::KANTOR),
                        },
                        None,
                        false,
                    )
                }
                CfRule::IconSet {
                    thresholds,
                    show_value,
                } => {
                    let steps = thresholds
                        .into_iter()
                        .map(|t: IconThreshold| {
                            let icon = icon_of(self.model.get_model(), &t.icon, &t.color);
                            let formula = match &t.cfvo {
                                Cfvo::Formula(f) => {
                                    self.model.cf_parse(sheet, f, anchor.0, anchor.1)
                                }
                                _ => None,
                            };
                            IconStep {
                                cfvo: t.cfvo,
                                formula,
                                gte: t.is_strict,
                                icon,
                            }
                        })
                        .collect();
                    (Kind::Icons { steps, show_value }, None, false)
                }
                CfRule::IconRating {
                    icon,
                    color,
                    thresholds,
                    show_value,
                } => {
                    let of = thresholds.len().clamp(1, 5) as u8;
                    let base = icon_of(self.model.get_model(), &icon, &color);
                    let steps = thresholds
                        .into_iter()
                        .enumerate()
                        .map(|(i, (cfvo, gte))| {
                            let level = i as u8;
                            let shape = match base.shape {
                                IconShape::Star { .. } => IconShape::Star {
                                    filled: level,
                                    of: of - 1,
                                },
                                IconShape::Circle => IconShape::Pie {
                                    filled: level,
                                    of: of - 1,
                                },
                                _ => IconShape::Bars {
                                    lit: level,
                                    of: of - 1,
                                },
                            };
                            let formula = match &cfvo {
                                Cfvo::Formula(f) => {
                                    self.model.cf_parse(sheet, f, anchor.0, anchor.1)
                                }
                                _ => None,
                            };
                            IconStep {
                                cfvo,
                                formula,
                                gte,
                                icon: CfIcon {
                                    shape,
                                    // Excel's quarters are dark grey.
                                    color: if matches!(shape, IconShape::Pie { .. }) {
                                        0x40_40_40
                                    } else {
                                        base.color
                                    },
                                },
                            }
                        })
                        .collect();
                    (Kind::Icons { steps, show_value }, None, false)
                }
            };
            rules.push(Rule {
                areas,
                bounds,
                anchor,
                kind,
                dxf,
                stop,
            });
        }
        rules
    }

    /// The numbers and value counts of a rule's range, clipped to the used
    /// area (a rule on `A:A` reads the filled rows only).
    fn aggregate(&mut self, sheet: u32, index: usize, rule: &Rule) -> Arc<Aggregate> {
        if let Some(a) = self.cond.aggregates.get(&(sheet, index)) {
            return a.clone();
        }
        let started = std::time::Instant::now();
        let (rows, cols) = self.used_size(sheet);
        let model = self.model.get_model();
        let needs_counts = matches!(rule.kind, Kind::Duplicate(_));
        let mut agg = Aggregate::default();
        for area in &rule.areas {
            if area.top >= rows || area.left >= cols {
                continue;
            }
            let (bottom, right) = (area.bottom.min(rows - 1), area.right.min(cols - 1));
            let Ok(ws) = model.workbook.worksheet(sheet) else {
                continue;
            };
            // Shared strings seen, by index: their text's number.
            let mut shared: HashMap<i32, u32> = HashMap::new();
            for r in area.top..=bottom {
                for c in area.left..=right {
                    let Some(cell) = ws.cell(r as i32 + 1, c as i32 + 1) else {
                        continue;
                    };
                    // Numbers are read without making a value of the cell
                    // (which copies text).
                    agg.formulas |= matches!(
                        &*cell,
                        Cell::CellFormula { .. }
                            | Cell::ArrayFormula { .. }
                            | Cell::SpillCell { .. }
                    );
                    let number = match &*cell {
                        Cell::NumberCell { v, .. } => Some(*v),
                        Cell::CellFormula {
                            v: FormulaValue::Number(n),
                            ..
                        }
                        | Cell::ArrayFormula {
                            v: FormulaValue::Number(n),
                            ..
                        }
                        | Cell::SpillCell {
                            v: SpillValue::Number(n),
                            ..
                        } => Some(*n),
                        _ => None,
                    };
                    if let Some(n) = number {
                        agg.sorted.push(n);
                        agg.sum += n;
                    }
                    if !needs_counts {
                        continue;
                    }
                    let key = match (&*cell, number) {
                        (_, Some(n)) => Key::Number(n.to_bits()),
                        (Cell::SharedString { si, .. }, _) => {
                            let id = match shared.get(si) {
                                Some(id) => *id,
                                None => {
                                    let text = model
                                        .workbook
                                        .shared_strings
                                        .get(*si as usize)
                                        .map(|t| t.to_lowercase())
                                        .unwrap_or_default();
                                    let id = agg.text_id(text);
                                    shared.insert(*si, id);
                                    id
                                }
                            };
                            Key::Text(id)
                        }
                        _ => match cell_value(model, sheet, r as i32 + 1, c as i32 + 1) {
                            Value::Text(t) => Key::Text(agg.text_id(t.to_lowercase())),
                            Value::Bool(b) => Key::Bool(b),
                            _ => continue,
                        },
                    };
                    *agg.counts.entry(key).or_insert(0) += 1;
                }
            }
        }
        agg.sorted.sort_by(f64::total_cmp);
        agg.external = match &rule.kind {
            Kind::ColorScale(steps) => steps.iter().any(|s| s.2.is_some()),
            Kind::DataBar { formulas, .. } => formulas.iter().any(Option::is_some),
            Kind::Icons { steps, .. } => steps.iter().any(|s| s.formula.is_some()),
            _ => false,
        };
        let anchor = rule.anchor;
        match &rule.kind {
            Kind::Top {
                rank,
                percent,
                bottom,
            } if !agg.sorted.is_empty() => {
                let len = agg.sorted.len();
                let n = if *percent {
                    (len as f64 * f64::from(*rank) / 100.0).floor() as usize
                } else {
                    *rank as usize
                }
                .clamp(1, len);
                agg.cutoff = Some(if *bottom {
                    agg.sorted[n - 1]
                } else {
                    agg.sorted[len - n]
                });
            }
            Kind::ColorScale(steps) => {
                agg.stops = steps
                    .iter()
                    .map(|(cfvo, _, f)| self.resolve(sheet, anchor, cfvo, f.as_ref(), &agg.sorted))
                    .collect();
            }
            Kind::DataBar {
                min, max, formulas, ..
            } => {
                let (lo, hi) = (
                    agg.sorted.first().copied().unwrap_or(0.0),
                    agg.sorted.last().copied().unwrap_or(0.0),
                );
                let low = match min {
                    None => lo.min(0.0),
                    Some(c) => self.resolve(sheet, anchor, c, formulas[0].as_ref(), &agg.sorted),
                };
                let high = match max {
                    None => hi.max(0.0),
                    Some(c) => self.resolve(sheet, anchor, c, formulas[1].as_ref(), &agg.sorted),
                };
                agg.stops = vec![low, high];
            }
            Kind::Icons { steps, .. } => {
                agg.stops = steps
                    .iter()
                    .map(|s| self.resolve(sheet, anchor, &s.cfvo, s.formula.as_ref(), &agg.sorted))
                    .collect();
            }
            _ => {}
        }
        let elapsed = started.elapsed();
        if elapsed.as_millis() >= 20 {
            log::debug!(
                "conditional formatting: rule {index} of sheet {sheet} read {} numbers in {elapsed:.2?}",
                agg.sorted.len()
            );
        }
        let agg = Arc::new(agg);
        self.cond.aggregates.insert((sheet, index), agg.clone());
        agg
    }

    /// A threshold's value over the range's sorted numbers.
    fn resolve(
        &mut self,
        sheet: u32,
        anchor: (i32, i32),
        cfvo: &Cfvo,
        formula: Option<&CfFormula>,
        sorted: &[f64],
    ) -> f64 {
        let (lo, hi) = (
            sorted.first().copied().unwrap_or(0.0),
            sorted.last().copied().unwrap_or(0.0),
        );
        match cfvo {
            Cfvo::Min => lo,
            Cfvo::Max => hi,
            Cfvo::Number(n) => *n,
            Cfvo::Percent(p) => lo + (hi - lo) * p / 100.0,
            Cfvo::Percentile(p) => percentile(sorted, *p),
            Cfvo::Formula(_) => {
                match formula.map(|f| self.model.cf_evaluate(f, sheet, anchor.0, anchor.1)) {
                    Some(CfValue::Number(n)) => n,
                    _ => 0.0,
                }
            }
        }
    }

    fn today(&mut self) -> f64 {
        if let Some(t) = self.cond.today {
            return t;
        }
        let t = self
            .model
            .cf_parse(0, "TODAY()", 1, 1)
            .map(|f| self.model.cf_evaluate(&f, 0, 1, 1))
            .and_then(|v| match v {
                CfValue::Number(n) => Some(n),
                _ => None,
            })
            .unwrap_or(0.0);
        self.cond.today = Some(t);
        t
    }

    /// `[first, last]` days (serial numbers) of a time period.
    fn period(&mut self, period: &PeriodType) -> Option<(f64, f64)> {
        let today = self.today();
        let weekday = (today - 1.0).rem_euclid(7.0); // 0 = Sunday
        let sunday = today - weekday;
        Some(match period {
            PeriodType::Today => (today, today),
            PeriodType::Yesterday => (today - 1.0, today - 1.0),
            PeriodType::Tomorrow => (today + 1.0, today + 1.0),
            PeriodType::Last7Days => (today - 6.0, today),
            PeriodType::Next7Days => (today + 1.0, today + 7.0),
            PeriodType::ThisWeek => (sunday, sunday + 6.0),
            PeriodType::LastWeek => (sunday - 7.0, sunday - 1.0),
            PeriodType::NextWeek => (sunday + 7.0, sunday + 13.0),
            PeriodType::ThisMonth => return self.month(0),
            PeriodType::LastMonth => return self.month(-1),
            PeriodType::NextMonth => return self.month(1),
            PeriodType::ThisYear | PeriodType::LastYear | PeriodType::NextYear => {
                let shift = match period {
                    PeriodType::LastYear => -1,
                    PeriodType::NextYear => 1,
                    _ => 0,
                };
                let start = self.formula_number(&format!("DATE(YEAR(TODAY())+{shift},1,1)"))?;
                let end = self.formula_number(&format!("DATE(YEAR(TODAY())+{shift},12,31)"))?;
                (start, end)
            }
            PeriodType::Between | PeriodType::NotBetween => return None,
        })
    }

    /// The days (serial numbers) from `d1` to `d2`, dates as DATEVALUE
    /// reads them (either order).
    fn dates_between(&mut self, d1: Option<&str>, d2: Option<&str>) -> Option<(f64, f64)> {
        let day = |this: &mut Self, d: &str| {
            this.formula_number(&format!("DATEVALUE(\"{}\")", d.replace('"', "\"\"")))
        };
        let a = day(self, d1?)?;
        let b = day(self, d2?)?;
        Some((a.min(b), a.max(b)))
    }

    /// First and last day of the month `m` months from this one.
    fn month(&mut self, m: i32) -> Option<(f64, f64)> {
        let start = self.formula_number(&format!("EOMONTH(TODAY(),{})", m - 1))? + 1.0;
        let end = self.formula_number(&format!("EOMONTH(TODAY(),{m})"))?;
        Some((start, end))
    }

    fn formula_number(&mut self, formula: &str) -> Option<f64> {
        let f = self.model.cf_parse(0, formula, 1, 1)?;
        match self.model.cf_evaluate(&f, 0, 1, 1) {
            CfValue::Number(n) => Some(n),
            _ => None,
        }
    }

    fn evaluate_conditional(&mut self, sheet: u32, row: u32, col: u32) -> Option<Conditional> {
        let rules = self.cond_rules(sheet);
        let (r1, c1) = (row as i32 + 1, col as i32 + 1);
        let mut value: Option<Value> = None;
        let mut overlay = Overlay::default();
        let mut bar = None;
        let mut icon: Option<CfIcon> = None;
        let mut show_value = true;
        for (index, rule) in rules.iter().enumerate() {
            if !rule.bounds.contains(row, col) || !rule.areas.iter().any(|a| a.contains(row, col)) {
                continue;
            }
            let v = value
                .get_or_insert_with(|| cell_value(self.model.get_model(), sheet, r1, c1))
                .clone();
            let matched = match &rule.kind {
                Kind::CellIs { op, a, b } => {
                    let mut operand = |f: &Option<CfFormula>| -> Value {
                        match f {
                            Some(f) => self.model.cf_evaluate(f, sheet, r1, c1).into(),
                            None => Value::Error,
                        }
                    };
                    let x = operand(a);
                    let ord = compare(&v, &x);
                    use std::cmp::Ordering::*;
                    match op {
                        ValueOperator::Equal => ord == Some(Equal),
                        ValueOperator::NotEqual => ord.is_some_and(|o| o != Equal),
                        ValueOperator::GreaterThan => ord == Some(Greater),
                        ValueOperator::GreaterThanOrEqual => ord.is_some_and(|o| o != Less),
                        ValueOperator::LessThan => ord == Some(Less),
                        ValueOperator::LessThanOrEqual => ord.is_some_and(|o| o != Greater),
                        ValueOperator::Between | ValueOperator::NotBetween => {
                            let y = operand(b);
                            let (lo, hi) = match compare(&x, &y) {
                                Some(Greater) => (y, x),
                                Some(_) => (x, y),
                                None => (Value::Error, Value::Error),
                            };
                            let inside = compare(&v, &lo).is_some_and(|o| o != Less)
                                && compare(&v, &hi).is_some_and(|o| o != Greater);
                            let known = compare(&v, &lo).is_some() && compare(&v, &hi).is_some();
                            if matches!(op, ValueOperator::Between) {
                                inside
                            } else {
                                known && !inside
                            }
                        }
                    }
                }
                Kind::Expression(f) => match f {
                    Some(f) => truthy(&self.model.cf_evaluate(f, sheet, r1, c1).into()),
                    None => false,
                },
                Kind::Text { op, text } => match as_text(&v).map(|s| s.to_lowercase()) {
                    None => false,
                    Some(s) => match op {
                        TextOperator::Contains => !matches!(v, Value::Empty) && s.contains(text),
                        TextOperator::DoesNotContain => {
                            matches!(v, Value::Empty) || !s.contains(text)
                        }
                        TextOperator::BeginsWith => s.starts_with(text.as_str()),
                        TextOperator::EndsWith => s.ends_with(text.as_str()),
                        TextOperator::Equals => s == *text,
                    },
                },
                Kind::Period(p, d1, d2) => match v {
                    Value::Number(n) => match p {
                        PeriodType::Between | PeriodType::NotBetween => {
                            let inside = self
                                .dates_between(d1.as_deref(), d2.as_deref())
                                .is_some_and(|(lo, hi)| (lo..=hi).contains(&n.floor()));
                            inside == (*p == PeriodType::Between)
                        }
                        _ => self
                            .period(p)
                            .is_some_and(|(lo, hi)| (lo..=hi).contains(&n.floor())),
                    },
                    _ => false,
                },
                Kind::Blanks(blank) => {
                    let is_blank = match &v {
                        Value::Empty => true,
                        Value::Text(s) => s.trim().is_empty(),
                        _ => false,
                    };
                    is_blank == *blank
                }
                Kind::Errors(errors) => matches!(v, Value::Error) == *errors,
                Kind::Duplicate(dup) => {
                    let key = match &v {
                        Value::Number(n) => Some(Key::Number(n.to_bits())),
                        Value::Text(s) => (!s.is_empty()).then_some(Key::Text(u32::MAX)),
                        Value::Bool(b) => Some(Key::Bool(*b)),
                        _ => None,
                    };
                    match key {
                        Some(k) => {
                            let agg = self.aggregate(sheet, index, rule);
                            let k = match (&k, &v) {
                                (Key::Text(_), Value::Text(t)) => {
                                    match agg.texts.get(&t.to_lowercase()) {
                                        Some(id) => Key::Text(*id),
                                        None => k,
                                    }
                                }
                                _ => k,
                            };
                            let n = agg.counts.get(&k).copied().unwrap_or(0);
                            if *dup { n > 1 } else { n == 1 }
                        }
                        None => false,
                    }
                }
                Kind::Average(above) => match v {
                    Value::Number(n) => {
                        let agg = self.aggregate(sheet, index, rule);
                        if agg.sorted.is_empty() {
                            false
                        } else {
                            let avg = agg.sum / agg.sorted.len() as f64;
                            if *above { n > avg } else { n < avg }
                        }
                    }
                    _ => false,
                },
                Kind::Top { bottom, .. } => match v {
                    Value::Number(n) => {
                        let agg = self.aggregate(sheet, index, rule);
                        agg.cutoff
                            .is_some_and(|c| if *bottom { n <= c } else { n >= c })
                    }
                    _ => false,
                },
                Kind::ColorScale(steps) => match v {
                    Value::Number(n) => {
                        let agg = self.aggregate(sheet, index, rule);
                        let stops = &agg.stops;
                        let colors: Vec<Rgb> =
                            steps.iter().map(|s| s.1.unwrap_or(0xFF_FF_FF)).collect();
                        if overlay.fill.is_none() && stops.len() == colors.len() && stops.len() >= 2
                        {
                            overlay.fill = Some(scale_color(n, stops, &colors));
                        }
                        true
                    }
                    _ => false,
                },
                Kind::DataBar {
                    color,
                    negative,
                    gradient,
                    show_value: show,
                    style,
                    ..
                } => match v {
                    Value::Number(n) => {
                        let agg = self.aggregate(sheet, index, rule);
                        if bar.is_none() {
                            bar = Some(styled_bar(
                                n,
                                agg.stops[0],
                                agg.stops[1],
                                *color,
                                *negative,
                                *gradient,
                                style,
                            ));
                            show_value &= *show;
                        }
                        true
                    }
                    _ => false,
                },
                Kind::Icons {
                    steps,
                    show_value: show,
                } => match v {
                    Value::Number(n) => {
                        let agg = self.aggregate(sheet, index, rule);
                        if icon.is_none() && !steps.is_empty() {
                            let mut at = 0;
                            for (i, (s, t)) in steps.iter().zip(&agg.stops).enumerate() {
                                if n > *t || (n == *t && s.gte) {
                                    at = i;
                                }
                            }
                            icon = Some(steps[at].icon);
                            show_value &= *show;
                        }
                        true
                    }
                    _ => false,
                },
            };
            if !matched {
                continue;
            }
            if let Some(dxf) = &rule.dxf {
                overlay.add(self.model.get_model(), dxf);
            }
            if rule.stop {
                break;
            }
        }
        if overlay.is_empty() && bar.is_none() && icon.is_none() {
            return None;
        }
        let (style, display) = if overlay.is_empty() {
            (None, None)
        } else {
            let base = self.cell_style(sheet, row, col);
            let mut s = (*base).clone();
            if let Some(f) = overlay.fill {
                s.fill = Some(f);
            }
            if let Some(c) = overlay.font_color {
                s.font_color = Some(c);
            }
            s.bold = overlay.bold.unwrap_or(s.bold);
            s.italic = overlay.italic.unwrap_or(s.italic);
            s.underline = overlay.underline.unwrap_or(s.underline);
            s.strikethrough = overlay.strike.unwrap_or(s.strikethrough);
            let b = &overlay.borders;
            s.borders.left = b.left.or(s.borders.left);
            s.borders.right = b.right.or(s.borders.right);
            s.borders.top = b.top.or(s.borders.top);
            s.borders.bottom = b.bottom.or(s.borders.bottom);
            let display = overlay.num_fmt.as_ref().map(|code| {
                s.num_fmt = code.clone();
                let fmt = kantor_numfmt::NumberFormat::parse(code);
                numfmt::display_cell(self.model.get_model(), sheet, r1, c1, &fmt)
            });
            (Some(Arc::new(s)), display)
        };
        Some(Conditional {
            style,
            display,
            bar,
            icon,
            show_value,
            borders: overlay.borders,
        })
    }
}

/// A colour scale's colour for `v`.
fn scale_color(v: f64, stops: &[f64], colors: &[Rgb]) -> Rgb {
    let last = stops.len() - 1;
    if v <= stops[0] {
        return colors[0];
    }
    if v >= stops[last] {
        return colors[last];
    }
    for i in 0..last {
        if v <= stops[i + 1] {
            let span = stops[i + 1] - stops[i];
            let t = if span.abs() < f64::EPSILON {
                0.0
            } else {
                (v - stops[i]) / span
            };
            return lerp(colors[i], colors[i + 1], t);
        }
    }
    colors[last]
}

/// The bar for `v` with Kantor's look (lengths 0 to 100 %, no border).
#[cfg(test)]
fn data_bar(v: f64, low: f64, high: f64, color: Rgb, negative: Rgb, gradient: bool) -> DataBar {
    styled_bar(v, low, high, color, negative, gradient, &BarStyle::KANTOR)
}

/// The bar for `v` on a scale from `low` to `high`. With negative and
/// positive values the axis sits where zero is (in the middle with Excel's
/// "cell midpoint"; none with "none": every bar starts at the left edge);
/// otherwise bars start at the left edge (all negative: at the right edge,
/// growing left). Bar lengths run from the style's minimum to its maximum
/// length.
fn styled_bar(
    v: f64,
    low: f64,
    high: f64,
    color: Rgb,
    negative: Rgb,
    gradient: bool,
    style: &BarStyle,
) -> DataBar {
    use crate::cf_x14::Axis;
    let len = |f: f32| style.min_len + f.clamp(0.0, 1.0) * (style.max_len - style.min_len);
    let span = high - low;
    let frac = |x: f64| -> f32 {
        if span.abs() < f64::EPSILON {
            1.0
        } else {
            ((x - low) / span).clamp(0.0, 1.0) as f32
        }
    };
    let mixed = low < 0.0 && high > 0.0;
    let (from, to, axis) = match style.axis {
        Axis::None if low < 0.0 => (0.0, len(frac(v)), None),
        Axis::Middle if low < 0.0 || mixed => {
            let side = |x: f64, edge: f64| {
                if edge.abs() < f64::EPSILON {
                    0.0
                } else {
                    (x / edge).clamp(0.0, 1.0) as f32
                }
            };
            if v < 0.0 {
                let l = 0.5 * len(side(v, low));
                (0.5 - l, 0.5, Some(0.5))
            } else {
                (0.5, 0.5 + 0.5 * len(side(v, high.max(0.0))), Some(0.5))
            }
        }
        _ if mixed => {
            let zero = frac(0.0);
            if v < 0.0 {
                let l = zero * len((v / low).clamp(0.0, 1.0) as f32);
                (zero - l, zero, Some(zero))
            } else {
                let l = (1.0 - zero) * len((v / high).clamp(0.0, 1.0) as f32);
                (zero, zero + l, Some(zero))
            }
        }
        _ if high <= 0.0 && low < 0.0 => {
            // All negative: bars grow leftwards from the right edge.
            let at = if span.abs() < f64::EPSILON {
                0.0
            } else {
                ((v - low) / span).clamp(0.0, 1.0) as f32
            };
            (1.0 - len(1.0 - at), 1.0, None)
        }
        _ => (0.0, len(frac(v)), None),
    };
    let neg = v < 0.0;
    // Right to left: the same bar, mirrored.
    let (from, to, axis) = if style.rtl {
        (1.0 - to, 1.0 - from, axis.map(|a| 1.0 - a))
    } else {
        (from, to, axis)
    };
    DataBar {
        color: if neg {
            style.negative_fill.unwrap_or(negative)
        } else {
            color
        },
        gradient,
        from,
        to,
        axis,
        negative: neg != style.rtl,
        border: if neg {
            style.negative_border.or(style.border)
        } else {
            style.border
        },
        axis_color: style.axis_color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_interpolates() {
        let v = [1.0, 2.0, 3.0, 4.0, 5.0];
        assert_eq!(percentile(&v, 50.0), 3.0);
        assert_eq!(percentile(&v, 25.0), 2.0);
        assert!((percentile(&v, 10.0) - 1.4).abs() < 1e-9);
    }

    #[test]
    fn compare_like_excel() {
        use std::cmp::Ordering::*;
        let n = Value::Number;
        let t = |s: &str| Value::Text(s.into());
        assert_eq!(compare(&n(5.0), &n(3.0)), Some(Greater));
        assert_eq!(compare(&t("abc"), &t("ABC")), Some(Equal));
        assert_eq!(compare(&t("a"), &n(1e9)), Some(Greater));
        assert_eq!(compare(&Value::Empty, &n(0.0)), Some(Equal));
        assert_eq!(compare(&Value::Empty, &t("")), Some(Equal));
        assert_eq!(compare(&Value::Error, &n(1.0)), None);
    }

    #[test]
    fn scale_and_bars() {
        assert_eq!(
            scale_color(0.0, &[0.0, 10.0], &[0x000000, 0xFFFFFF]),
            0x000000
        );
        assert_eq!(
            scale_color(5.0, &[0.0, 10.0], &[0x000000, 0xFEFEFE]),
            0x7F7F7F
        );
        assert_eq!(
            scale_color(7.5, &[0.0, 5.0, 10.0], &[0xFF0000, 0x00FF00, 0x0000FF]),
            0x008080
        );
        let b = data_bar(5.0, 0.0, 10.0, 1, 2, true);
        assert_eq!((b.from, b.to, b.axis), (0.0, 0.5, None));
        let b = data_bar(-5.0, -10.0, 10.0, 1, 2, false);
        assert_eq!((b.from, b.to, b.axis, b.color), (0.25, 0.5, Some(0.5), 2));
        // Excel 2007's lengths (10 % to 90 %), a border, no axis.
        let style = BarStyle {
            min_len: 0.1,
            max_len: 0.9,
            border: Some(7),
            ..BarStyle::KANTOR
        };
        let b = styled_bar(0.0, 0.0, 10.0, 1, 2, false, &style);
        assert_eq!((b.from, b.to, b.border), (0.0, 0.1, Some(7)));
        let b = styled_bar(10.0, 0.0, 10.0, 1, 2, false, &style);
        assert!((b.to - 0.9).abs() < 1e-6);
        let none = BarStyle {
            axis: crate::cf_x14::Axis::None,
            negative_fill: Some(9),
            ..BarStyle::KANTOR
        };
        let b = styled_bar(-10.0, -10.0, 10.0, 1, 2, false, &none);
        assert_eq!((b.from, b.to, b.axis, b.color), (0.0, 0.0, None, 9));
        let mid = BarStyle {
            axis: crate::cf_x14::Axis::Middle,
            ..BarStyle::KANTOR
        };
        let b = styled_bar(5.0, -10.0, 20.0, 1, 2, false, &mid);
        assert_eq!((b.from, b.to, b.axis), (0.5, 0.625, Some(0.5)));
    }

    #[test]
    fn sqref_forms() {
        let r = parse_sqref("A1:B3 D4 C:C 2:3");
        assert_eq!(r.len(), 4);
        assert_eq!((r[1].top, r[1].left, r[1].bottom), (3, 3, 3));
        assert_eq!((r[2].left, r[2].bottom), (2, MAX_ROWS - 1));
        assert_eq!((r[3].top, r[3].bottom, r[3].right), (1, 2, MAX_COLS - 1));
    }
}

/// Kept aggregates must give what a fresh evaluation gives, after every kind
/// of change.
#[cfg(test)]
mod invalidation {
    use super::*;
    use crate::{CalcMode, CfFormat, CfRuleDef, CfSpec};

    const ROWS: u32 = 24;
    const COLS: u32 = 6;

    fn rule(spec: CfSpec) -> CfRuleDef {
        CfRuleDef {
            num_fmt: None,
            border: None,
            spec,
            format: CfFormat {
                fill: Some(0xFF_C7_CE),
                ..CfFormat::default()
            },
            stop_if_true: false,
        }
    }

    /// A: numbers (data bar), B: =A*2 (colour scale), C: numbers (top 3),
    /// D: texts (duplicates), E: numbers (above average), F: free.
    fn book(mode: CalcMode) -> Workbook {
        let mut wb = Workbook::new_empty().unwrap();
        let mut cells = Vec::new();
        for r in 0..20u32 {
            cells.push((r, 0, ((r * 7) % 13).to_string()));
            cells.push((r, 1, format!("=A{}*2", r + 1)));
            cells.push((r, 2, ((r * 5) % 17).to_string()));
            cells.push((
                r,
                3,
                ["pump", "valve", "Pump", "seal"][r as usize % 4].to_owned(),
            ));
            cells.push((r, 4, (r % 6).to_string()));
        }
        wb.set_inputs(0, &cells).unwrap();
        let bar = CfSpec::DataBar {
            color: 0x63_8E_C6,
            gradient: true,
            bar: Default::default(),
        };
        wb.add_cf_rule(0, "A1:A20", &rule(bar)).unwrap();
        let scale = CfSpec::ColorScale {
            colors: vec![0xF8_69_6B, 0xFF_EB_84, 0x63_BE_7B],
        };
        wb.add_cf_rule(0, "B1:B20", &rule(scale)).unwrap();
        let top = CfSpec::TopBottom {
            bottom: false,
            rank: 3,
            percent: false,
        };
        wb.add_cf_rule(0, "C1:C20", &rule(top)).unwrap();
        let dups = CfSpec::Duplicates { unique: false };
        wb.add_cf_rule(0, "D:D", &rule(dups)).unwrap();
        let avg = CfSpec::Average { below: false };
        wb.add_cf_rule(0, "E1:E20", &rule(avg)).unwrap();
        wb.set_calc_mode(mode);
        wb
    }

    fn shown(wb: &mut Workbook) -> Vec<Option<Arc<Conditional>>> {
        (0..ROWS)
            .flat_map(|r| (0..COLS).map(move |c| (r, c)))
            .map(|(r, c)| wb.conditional(0, r, c))
            .collect()
    }

    /// What's shown with kept aggregates equals a fresh evaluation.
    fn check(wb: &mut Workbook, what: &str) {
        let kept = shown(wb);
        let state = std::mem::take(&mut wb.cond);
        let fresh = shown(wb);
        assert_eq!(kept, fresh, "after {what}");
        wb.cond = state;
    }

    fn kept_rules(wb: &Workbook) -> Vec<usize> {
        let mut v: Vec<usize> = wb.cond.aggregates.keys().map(|k| k.1).collect();
        v.sort();
        v
    }

    #[test]
    fn edits_outside_a_rule_keep_its_aggregate() {
        let mut wb = book(CalcMode::Manual);
        shown(&mut wb);
        let all = kept_rules(&wb);
        assert_eq!(all.len(), 5);
        wb.set_input(0, 2, 5, "free").unwrap();
        check(&mut wb, "an edit outside every rule");
        assert_eq!(kept_rules(&wb), all, "nothing recomputed");
        // Inside the data bar's range only: the others are kept.
        wb.set_input(0, 3, 0, "1000").unwrap();
        check(&mut wb, "an edit inside one rule");
        assert_eq!(kept_rules(&wb), all);
        wb.undo().unwrap();
        check(&mut wb, "undo");
        wb.redo().unwrap();
        check(&mut wb, "redo");
        // A text that now has a duplicate, far down the whole-column rule.
        wb.set_input(0, 40, 3, "SEAL").unwrap();
        check(&mut wb, "a new duplicate below the used rows");
        wb.set_input(0, 1, 2, "99").unwrap();
        check(&mut wb, "a new top value");
        wb.set_input(0, 1, 4, "").unwrap();
        check(&mut wb, "a cleared cell");
        // F9 recalculates B (its values were stale).
        wb.recalculate();
        check(&mut wb, "a recalculation");
    }

    #[test]
    fn automatic_evaluation_drops_formula_ranges_only() {
        let mut wb = book(CalcMode::Automatic);
        shown(&mut wb);
        let before = kept_rules(&wb);
        // A changes, and so does B (its formulas) when the model evaluates.
        wb.set_input(0, 0, 0, "50").unwrap();
        check(&mut wb, "an edit feeding formulas");
        wb.set_input(0, 2, 5, "x").unwrap();
        // B's aggregate went (formulas evaluated), C/D/E stayed.
        let after = kept_rules(&wb);
        check(&mut wb, "an edit outside");
        assert!(after.len() < before.len() || after == before);
        wb.undo().unwrap();
        wb.undo().unwrap();
        check(&mut wb, "two undos");
    }

    #[test]
    fn structural_edits_start_the_sheet_over() {
        let mut wb = book(CalcMode::Manual);
        shown(&mut wb);
        wb.insert_rows(0, 2, 3).unwrap();
        check(&mut wb, "inserted rows");
        wb.delete_rows(0, 0, 1).unwrap();
        check(&mut wb, "deleted rows");
        let keys = [crate::SortKey {
            col: 2,
            descending: true,
        }];
        wb.sort_range(0, CellRange::from_corners((0, 0), (19, 4)), &keys, false)
            .unwrap();
        check(&mut wb, "a sort");
        wb.undo().unwrap();
        check(&mut wb, "undoing the sort");
        let rule2 = rule(CfSpec::Blanks { not: false });
        wb.add_cf_rule(0, "F1:F20", &rule2).unwrap();
        check(&mut wb, "a new rule");
        wb.set_frozen(0, 1, 0).unwrap();
        check(&mut wb, "frozen panes");
    }
}

#[cfg(test)]
mod between_tests {
    use ironcalc_base::cf_types::{CfRuleInput, PeriodType};
    use ironcalc_base::types::{Dxf, Fill};

    use crate::Workbook;

    /// IronCalc's "between" date periods (A Date Occurring between two
    /// dates) highlight the dates inside, "not between" the others.
    #[test]
    fn between_dates_highlight() {
        let mut wb = Workbook::new_empty().unwrap();
        for (r, d) in ["2026-01-05", "2026-02-10", "2026-03-20"]
            .iter()
            .enumerate()
        {
            wb.set_input(0, r as u32, 0, d).unwrap();
            wb.set_input(0, r as u32, 1, d).unwrap();
        }
        let format = Dxf {
            fill: Some(Fill {
                color: ironcalc_base::types::Color::Rgb("#FFFF00".into()),
            }),
            ..Dxf::default()
        };
        for (range, period) in [
            ("A1:A3", PeriodType::Between),
            ("B1:B3", PeriodType::NotBetween),
        ] {
            wb.model
                .add_conditional_formatting(
                    0,
                    range,
                    CfRuleInput::TimePeriod {
                        time_period: period,
                        date1: Some("2026-02-01".into()),
                        date2: Some("2026-02-28".into()),
                        format: format.clone(),
                        stop_if_true: false,
                    },
                )
                .unwrap();
        }
        wb.history.edited();
        wb.mark_cf(0);
        let filled = |wb: &mut Workbook, r: u32, c: u32| {
            wb.conditional(0, r, c)
                .and_then(|x| x.style.as_ref().and_then(|s| s.fill))
                == Some(0xFFFF00)
        };
        assert!(!filled(&mut wb, 0, 0));
        assert!(filled(&mut wb, 1, 0));
        assert!(!filled(&mut wb, 2, 0));
        assert!(filled(&mut wb, 0, 1));
        assert!(!filled(&mut wb, 1, 1));
        assert!(filled(&mut wb, 2, 1));
    }

    /// A rule's border is drawn over the cell's own edges.
    #[test]
    fn rule_borders() {
        use ironcalc_base::types::{Border, BorderItem, BorderStyle, Color};
        let mut wb = Workbook::new_empty().unwrap();
        wb.set_input(0, 0, 0, "5").unwrap();
        let red = BorderItem {
            style: BorderStyle::Thick,
            color: Color::Rgb("#FF0000".into()),
        };
        let format = Dxf {
            border: Some(Border {
                bottom: Some(red.clone()),
                left: Some(red),
                ..Border::default()
            }),
            ..Dxf::default()
        };
        wb.model
            .add_conditional_formatting(
                0,
                "A1:A3",
                CfRuleInput::Formula {
                    formula: "A1>1".into(),
                    format,
                    stop_if_true: false,
                },
            )
            .unwrap();
        wb.history.edited();
        wb.mark_cf(0);
        let c = wb.conditional(0, 0, 0).expect("the rule applies");
        let b = c.borders;
        assert_eq!(b.bottom.and_then(|l| l.color), Some(0xFF0000));
        assert_eq!(
            b.left.map(|l| l.style),
            Some(crate::format::LineStyle::Thick)
        );
        assert!(b.top.is_none() && b.right.is_none());
        assert!(wb.conditional(0, 1, 0).is_none(), "an empty cell isn't > 1");
    }
}
