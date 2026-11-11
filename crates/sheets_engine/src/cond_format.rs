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
//! worked out once per workbook state. Any edit, undo or recalculation is a
//! new state ([`Workbook::revision`]) and starts from scratch.
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
    /// The bar is for a negative value (it grows to the left of the axis).
    pub negative: bool,
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

/// Conditional formatting state for one workbook state.
#[derive(Default)]
pub(crate) struct CondState {
    key: (u64, u64),
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
    Period(PeriodType),
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
        if self.cond.key != key {
            self.cond = CondState {
                key,
                ..CondState::default()
            };
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
                    dxf_id,
                    stop_if_true,
                    ..
                } => (Kind::Period(time_period), dxf_of(dxf_id), stop_if_true),
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
                Kind::Period(p) => match v {
                    Value::Number(n) => self
                        .period(p)
                        .is_some_and(|(lo, hi)| (lo..=hi).contains(&n.floor())),
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
                    ..
                } => match v {
                    Value::Number(n) => {
                        let agg = self.aggregate(sheet, index, rule);
                        if bar.is_none() {
                            bar = Some(data_bar(
                                n,
                                agg.stops[0],
                                agg.stops[1],
                                *color,
                                *negative,
                                *gradient,
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

/// The bar for `v` on a scale from `low` to `high`. With negative and
/// positive values the axis sits where zero is; otherwise bars start at the
/// left edge (all negative: at the right edge, growing left).
fn data_bar(v: f64, low: f64, high: f64, color: Rgb, negative: Rgb, gradient: bool) -> DataBar {
    let span = high - low;
    let frac = |x: f64| -> f32 {
        if span.abs() < f64::EPSILON {
            1.0
        } else {
            ((x - low) / span).clamp(0.0, 1.0) as f32
        }
    };
    let (from, to, axis) = if low < 0.0 && high > 0.0 {
        let zero = frac(0.0);
        let at = frac(v);
        (zero.min(at), zero.max(at), Some(zero))
    } else if high <= 0.0 && low < 0.0 {
        // All negative: bars grow leftwards from the right edge.
        let at = if span.abs() < f64::EPSILON {
            0.0
        } else {
            ((v - low) / span).clamp(0.0, 1.0) as f32
        };
        (at, 1.0, None)
    } else {
        (0.0, frac(v), None)
    };
    DataBar {
        color: if v < 0.0 { negative } else { color },
        gradient,
        from,
        to,
        axis,
        negative: v < 0.0,
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
