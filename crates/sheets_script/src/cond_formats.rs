//! `Range.FormatConditions`: conditional formatting rules, through the
//! engine's rule edits (the ones Kantor's rule manager makes), so each
//! change undoes with the run and is saved like a rule made by hand.
//!
//! A `FormatCondition` keeps the rule's storage index in the sheet (the
//! engine's key for a rule): it stays right when other rules are added or
//! reordered, not when a rule stored before it is deleted.
//!
//! Like Excel, `FormatConditions.Add` puts the new rule last in priority
//! (recorded macros then call `SetFirstPriority`); `Item(i)` counts the
//! rules touching the range in priority order.

use std::collections::HashSet;
use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::{
    CellRange, CfBarAxis, CfBarDirection, CfBarOptions, CfCompare, CfFormat, CfIconRef, CfIconStep,
    CfPeriod, CfRuleDef, CfRuleInfo, CfSpec, CfTextOp, CfValue, CfValueKind, LineStyle, Workbook,
    default_icon_steps, sqref_of,
};

use crate::addr;
use crate::consts::*;
use crate::format::{line_style, line_style_of, palette, weight_of};
use crate::range::{RangeObj, range_of};
use crate::session::Session;
use crate::values::*;

// XlFormatConditionType
pub const XL_CELL_VALUE: i64 = 1;
pub const XL_EXPRESSION: i64 = 2;
pub const XL_COLOR_SCALE: i64 = 3;
pub const XL_DATABAR: i64 = 4;
pub const XL_TOP10: i64 = 5;
pub const XL_ICON_SETS: i64 = 6;
pub const XL_UNIQUE_VALUES: i64 = 8;
pub const XL_TEXT_STRING: i64 = 9;
pub const XL_BLANKS_CONDITION: i64 = 10;
pub const XL_TIME_PERIOD: i64 = 11;
pub const XL_ABOVE_AVERAGE_CONDITION: i64 = 12;
pub const XL_NO_BLANKS_CONDITION: i64 = 13;
pub const XL_ERRORS_CONDITION: i64 = 16;
pub const XL_NO_ERRORS_CONDITION: i64 = 17;

// XlFormatConditionOperator
pub const XL_BETWEEN: i64 = 1;
pub const XL_NOT_BETWEEN: i64 = 2;
pub const XL_EQUAL: i64 = 3;
pub const XL_NOT_EQUAL: i64 = 4;
pub const XL_GREATER: i64 = 5;
pub const XL_LESS: i64 = 6;
pub const XL_GREATER_EQUAL: i64 = 7;
pub const XL_LESS_EQUAL: i64 = 8;

// XlContainsOperator
pub const XL_CONTAINS: i64 = 0;
pub const XL_DOES_NOT_CONTAIN: i64 = 1;
pub const XL_BEGINS_WITH: i64 = 2;
pub const XL_ENDS_WITH: i64 = 3;

// XlTimePeriods
pub const XL_TODAY: i64 = 0;
pub const XL_YESTERDAY: i64 = 1;
pub const XL_LAST7_DAYS: i64 = 2;
pub const XL_THIS_WEEK: i64 = 3;
pub const XL_LAST_WEEK: i64 = 4;
pub const XL_LAST_MONTH: i64 = 5;
pub const XL_TOMORROW: i64 = 6;
pub const XL_NEXT_WEEK: i64 = 7;
pub const XL_NEXT_MONTH: i64 = 8;
pub const XL_THIS_MONTH: i64 = 9;

// XlConditionValueTypes
pub const XL_CONDITION_VALUE_NONE: i64 = -1;
pub const XL_CONDITION_VALUE_NUMBER: i64 = 0;
pub const XL_CONDITION_VALUE_LOWEST_VALUE: i64 = 1;
pub const XL_CONDITION_VALUE_HIGHEST_VALUE: i64 = 2;
pub const XL_CONDITION_VALUE_PERCENT: i64 = 3;
pub const XL_CONDITION_VALUE_FORMULA: i64 = 4;
pub const XL_CONDITION_VALUE_PERCENTILE: i64 = 5;
pub const XL_CONDITION_VALUE_AUTOMATIC_MIN: i64 = 6;
pub const XL_CONDITION_VALUE_AUTOMATIC_MAX: i64 = 7;

// XlTopBottom, XlAboveBelow, XlDupeUnique
pub const XL_TOP10_TOP: i64 = 1;
pub const XL_TOP10_BOTTOM: i64 = 0;
pub const XL_ABOVE_AVERAGE: i64 = 0;
pub const XL_BELOW_AVERAGE: i64 = 1;
pub const XL_EQUAL_ABOVE_AVERAGE: i64 = 2;
pub const XL_EQUAL_BELOW_AVERAGE: i64 = 3;
pub const XL_ABOVE_STD_DEV: i64 = 4;
pub const XL_BELOW_STD_DEV: i64 = 5;
pub const XL_UNIQUE: i64 = 0;
pub const XL_DUPLICATE: i64 = 1;

// Data bars: XlDataBarFillType, XlReadingOrder (direction),
// XlDataBarAxisPosition, XlDataBarBorderType
pub const XL_DATA_BAR_FILL_SOLID: i64 = 0;
pub const XL_DATA_BAR_FILL_GRADIENT: i64 = 1;
pub const XL_CONTEXT: i64 = -5002;
pub const XL_LTR: i64 = -5003;
pub const XL_RTL: i64 = -5004;
pub const XL_DATA_BAR_AXIS_AUTOMATIC: i64 = 0;
pub const XL_DATA_BAR_AXIS_MIDPOINT: i64 = 1;
pub const XL_DATA_BAR_AXIS_NONE: i64 = 2;
pub const XL_DATA_BAR_BORDER_NONE: i64 = 0;
pub const XL_DATA_BAR_BORDER_SOLID: i64 = 1;

// XlIconSet (`ActiveWorkbook.IconSets(xl3Arrows)`), by Excel's set names.
pub const XL_3_ARROWS: i64 = 1;
pub const XL_3_ARROWS_GRAY: i64 = 2;
pub const XL_3_FLAGS: i64 = 3;
pub const XL_3_TRAFFIC_LIGHTS1: i64 = 4;
pub const XL_3_TRAFFIC_LIGHTS2: i64 = 5;
pub const XL_3_SIGNS: i64 = 6;
pub const XL_3_SYMBOLS: i64 = 7;
pub const XL_3_SYMBOLS2: i64 = 8;
pub const XL_4_ARROWS: i64 = 9;
pub const XL_4_ARROWS_GRAY: i64 = 10;
pub const XL_4_RED_TO_BLACK: i64 = 11;
pub const XL_4_CRV: i64 = 12;
pub const XL_4_TRAFFIC_LIGHTS: i64 = 13;
pub const XL_5_ARROWS: i64 = 14;
pub const XL_5_ARROWS_GRAY: i64 = 15;
pub const XL_5_CRV: i64 = 16;
pub const XL_5_QUARTERS: i64 = 17;
pub const XL_3_STARS: i64 = 18;
pub const XL_3_TRIANGLES: i64 = 19;
pub const XL_5_BOXES: i64 = 20;

/// Excel's icon set names by `XlIconSet` value (1-based).
const ICON_SETS: [&str; 20] = [
    "3Arrows",
    "3ArrowsGray",
    "3Flags",
    "3TrafficLights1",
    "3TrafficLights2",
    "3Signs",
    "3Symbols",
    "3Symbols2",
    "4Arrows",
    "4ArrowsGray",
    "4RedToBlack",
    "4Rating",
    "4TrafficLights",
    "5Arrows",
    "5ArrowsGray",
    "5Rating",
    "5Quarters",
    "3Stars",
    "3Triangles",
    "5Boxes",
];

fn icon_set_name(id: i64) -> Result<&'static str, ScriptError> {
    if id == XL_5_BOXES {
        return Err(ScriptError::unsupported("The 5 Boxes icon set"));
    }
    usize::try_from(id - 1)
        .ok()
        .and_then(|i| ICON_SETS.get(i).copied())
        .ok_or_else(|| err1004("Unable to set the IconSet property of the IconSetCondition class"))
}

fn icon_set_id(name: &str) -> i64 {
    ICON_SETS
        .iter()
        .position(|n| n.eq_ignore_ascii_case(name))
        .map_or(XL_3_TRAFFIC_LIGHTS1, |i| i as i64 + 1)
}

/// Excel's default colours for a new colour scale (lowest, [midpoint,]
/// highest) and data bar.
const SCALE3: [u32; 3] = [0xF8696B, 0xFFEB84, 0x63BE7B];
const SCALE2: [u32; 2] = [0xF8696B, 0x63BE7B];
const BAR_COLOR: u32 = 0x638EC6;

/// `Range.FormatConditions` (or `FormatConditions(i)` with an index).
pub fn format_conditions(r: &RangeObj, args: &[Arg]) -> Result<Value, ScriptError> {
    let fc = Rc::new(FormatConditionsObj {
        s: r.s.clone(),
        sheet: r.sheet,
        areas: r.areas.clone(),
    });
    if arg(args, 0, "Index").is_some() {
        return fc.get("Item", args);
    }
    Ok(obj(fc))
}

/// `Workbook.IconSets` (or `IconSets(xl3Arrows)`): Excel's icon sets by
/// `XlIconSet` value.
pub fn icon_sets(args: &[Arg]) -> Result<Value, ScriptError> {
    if arg(args, 0, "Index").is_some() {
        return IconSetsObj.get("Item", args);
    }
    Ok(obj(Rc::new(IconSetsObj)))
}

pub struct FormatConditionsObj {
    pub s: Rc<Session>,
    pub sheet: u32,
    pub areas: Vec<CellRange>,
}

fn compare_of(op: i64) -> Result<CfCompare, ScriptError> {
    Ok(match op {
        XL_BETWEEN => CfCompare::Between,
        XL_NOT_BETWEEN => CfCompare::NotBetween,
        XL_EQUAL => CfCompare::EqualTo,
        XL_NOT_EQUAL => CfCompare::NotEqualTo,
        XL_GREATER => CfCompare::GreaterThan,
        XL_LESS => CfCompare::LessThan,
        XL_GREATER_EQUAL => CfCompare::GreaterOrEqual,
        XL_LESS_EQUAL => CfCompare::LessOrEqual,
        _ => return Err(ScriptError::runtime(5)),
    })
}

fn operator_of(c: CfCompare) -> i64 {
    match c {
        CfCompare::Between => XL_BETWEEN,
        CfCompare::NotBetween => XL_NOT_BETWEEN,
        CfCompare::EqualTo => XL_EQUAL,
        CfCompare::NotEqualTo => XL_NOT_EQUAL,
        CfCompare::GreaterThan => XL_GREATER,
        CfCompare::LessThan => XL_LESS,
        CfCompare::GreaterOrEqual => XL_GREATER_EQUAL,
        CfCompare::LessOrEqual => XL_LESS_EQUAL,
    }
}

fn text_op_of(op: i64) -> Result<CfTextOp, ScriptError> {
    Ok(match op {
        XL_CONTAINS => CfTextOp::Contains,
        XL_DOES_NOT_CONTAIN => CfTextOp::DoesNotContain,
        XL_BEGINS_WITH => CfTextOp::BeginsWith,
        XL_ENDS_WITH => CfTextOp::EndsWith,
        _ => return Err(ScriptError::runtime(5)),
    })
}

fn text_operator_of(op: CfTextOp) -> i64 {
    match op {
        CfTextOp::Contains | CfTextOp::Equals => XL_CONTAINS,
        CfTextOp::DoesNotContain => XL_DOES_NOT_CONTAIN,
        CfTextOp::BeginsWith => XL_BEGINS_WITH,
        CfTextOp::EndsWith => XL_ENDS_WITH,
    }
}

const PERIODS: [(i64, CfPeriod); 10] = [
    (XL_TODAY, CfPeriod::Today),
    (XL_YESTERDAY, CfPeriod::Yesterday),
    (XL_LAST7_DAYS, CfPeriod::Last7Days),
    (XL_THIS_WEEK, CfPeriod::ThisWeek),
    (XL_LAST_WEEK, CfPeriod::LastWeek),
    (XL_LAST_MONTH, CfPeriod::LastMonth),
    (XL_TOMORROW, CfPeriod::Tomorrow),
    (XL_NEXT_WEEK, CfPeriod::NextWeek),
    (XL_NEXT_MONTH, CfPeriod::NextMonth),
    (XL_THIS_MONTH, CfPeriod::ThisMonth),
];

fn period_of(n: i64) -> Result<CfPeriod, ScriptError> {
    PERIODS
        .iter()
        .find(|(k, _)| *k == n)
        .map(|(_, p)| *p)
        .ok_or_else(|| ScriptError::runtime(5))
}

fn date_operator_of(p: CfPeriod) -> i64 {
    PERIODS
        .iter()
        .find(|(_, q)| *q == p)
        .map_or(XL_TODAY, |(k, _)| *k)
}

/// A `Formula1` argument as typed: text as it is (`"=$C2>100"`, `"200"`),
/// a number as its text.
fn formula_text(v: &Value) -> Result<String, ScriptError> {
    Ok(match scalar(v)? {
        Value::String(s) => s,
        other => match other.as_f64() {
            Some(n) if !matches!(other, Value::Boolean(_)) => number_text(n),
            _ => to_str(&other)?,
        },
    })
}

/// A compared value as Excel's `Formula1` shows it: `=200`, `="East"`,
/// `=$B$1`.
fn formula_of_value(v: &str) -> String {
    let t = v.trim();
    if t.starts_with('=') {
        t.to_owned()
    } else if t.parse::<f64>().is_ok() {
        format!("={t}")
    } else {
        format!("=\"{}\"", t.replace('"', "\"\""))
    }
}

/// What `FormatConditions.Add` (or `Modify`) makes of its arguments
/// (`Type`, `Operator`, `Formula1`, `Formula2`, `String`, `TextOperator`,
/// `DateOperator`).
fn spec_of(kind: i64, args: &[Arg]) -> Result<CfSpec, ScriptError> {
    let formula = |i: usize, name: &str| -> Result<Option<String>, ScriptError> {
        arg(args, i, name).map(formula_text).transpose()
    };
    Ok(match kind {
        XL_CELL_VALUE => {
            let op = compare_of(arg_i64(args, 1, "Operator")?.unwrap_or(XL_BETWEEN))?;
            let value = formula(2, "Formula1")?.ok_or_else(|| ScriptError::runtime(449))?;
            let value2 = if op.two_values() {
                formula(3, "Formula2")?.ok_or_else(|| ScriptError::runtime(449))?
            } else {
                String::new()
            };
            CfSpec::CellValue { op, value, value2 }
        }
        XL_EXPRESSION => {
            let f = formula(2, "Formula1")?.ok_or_else(|| ScriptError::runtime(449))?;
            CfSpec::Formula {
                formula: format!("={}", f.trim().trim_start_matches('=')),
            }
        }
        XL_TEXT_STRING => CfSpec::Text {
            op: text_op_of(arg_i64(args, 5, "TextOperator")?.unwrap_or(XL_CONTAINS))?,
            text: arg_str(args, 4, "String")?.ok_or_else(|| ScriptError::runtime(449))?,
        },
        XL_BLANKS_CONDITION => CfSpec::Blanks { not: false },
        XL_NO_BLANKS_CONDITION => CfSpec::Blanks { not: true },
        XL_TIME_PERIOD => CfSpec::Dates {
            period: period_of(arg_i64(args, 6, "DateOperator")?.unwrap_or(XL_TODAY))?,
        },
        XL_UNIQUE_VALUES => CfSpec::Duplicates { unique: true },
        XL_TOP10 => CfSpec::TopBottom {
            bottom: false,
            rank: 10,
            percent: false,
        },
        XL_ABOVE_AVERAGE_CONDITION => CfSpec::Average {
            below: false,
            equal: false,
            std_dev: 0,
        },
        XL_COLOR_SCALE => CfSpec::ColorScale {
            colors: SCALE3.to_vec(),
        },
        XL_DATABAR => CfSpec::DataBar {
            color: BAR_COLOR,
            gradient: true,
            bar: CfBarOptions::default(),
        },
        XL_ICON_SETS => CfSpec::IconSet {
            set: "3TrafficLights1".into(),
            reverse: false,
            show_value: true,
            steps: Vec::new(),
        },
        XL_ERRORS_CONDITION | XL_NO_ERRORS_CONDITION => {
            return Err(ScriptError::unsupported(
                "Conditional formats for errors (xlErrorsCondition, xlNoErrorsCondition)",
            ));
        }
        _ => return Err(ScriptError::runtime(5)),
    })
}

/// `XlFormatConditionType` of a rule.
fn type_of(info: &CfRuleInfo) -> i64 {
    match info.def.as_ref().map(|d| &d.spec) {
        Some(CfSpec::CellValue { .. }) => XL_CELL_VALUE,
        Some(CfSpec::Formula { .. }) => XL_EXPRESSION,
        Some(CfSpec::ColorScale { .. }) => XL_COLOR_SCALE,
        Some(CfSpec::DataBar { .. }) => XL_DATABAR,
        Some(CfSpec::TopBottom { .. }) => XL_TOP10,
        Some(CfSpec::IconSet { .. }) => XL_ICON_SETS,
        Some(CfSpec::Duplicates { .. }) => XL_UNIQUE_VALUES,
        Some(CfSpec::Text { .. }) => XL_TEXT_STRING,
        Some(CfSpec::Blanks { not: false }) => XL_BLANKS_CONDITION,
        Some(CfSpec::Blanks { not: true }) => XL_NO_BLANKS_CONDITION,
        Some(CfSpec::Dates { .. }) => XL_TIME_PERIOD,
        Some(CfSpec::Average { .. }) => XL_ABOVE_AVERAGE_CONDITION,
        None => {
            let s = info.summary.as_str();
            if s.starts_with("No errors") {
                XL_NO_ERRORS_CONDITION
            } else if s.starts_with("Errors") {
                XL_ERRORS_CONDITION
            } else if s.starts_with("Text") {
                XL_TEXT_STRING
            } else if s.starts_with("Date") {
                XL_TIME_PERIOD
            } else if s.ends_with("color scale") {
                XL_COLOR_SCALE
            } else if s.starts_with("Icon") || s.starts_with("Rating") {
                XL_ICON_SETS
            } else {
                XL_EXPRESSION
            }
        }
    }
}

/// The object type a rule is in Excel's object model.
fn type_name_of(spec: Option<&CfSpec>) -> &'static str {
    match spec {
        Some(CfSpec::ColorScale { .. }) => "ColorScale",
        Some(CfSpec::DataBar { .. }) => "Databar",
        Some(CfSpec::IconSet { .. }) => "IconSetCondition",
        Some(CfSpec::TopBottom { .. }) => "Top10",
        Some(CfSpec::Average { .. }) => "AboveAverage",
        Some(CfSpec::Duplicates { .. }) => "UniqueValues",
        _ => "FormatCondition",
    }
}

impl FormatConditionsObj {
    /// The rules touching the range, in priority order.
    fn rules(&self) -> Result<Vec<CfRuleInfo>, ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let book = self.s.read()?;
        let mut touching = HashSet::new();
        for a in &self.areas {
            touching.extend(book.cf_rules(idx, Some(*a)).iter().map(|r| r.index));
        }
        Ok(book
            .cf_rules(idx, None)
            .into_iter()
            .filter(|r| touching.contains(&r.index))
            .collect())
    }

    fn item(&self, index: usize) -> Value {
        obj(Rc::new(FormatConditionObj {
            s: self.s.clone(),
            sheet: self.sheet,
            index,
        }))
    }

    /// Add a rule on the range, last in priority (as Excel's `Add`).
    fn add(&self, spec: CfSpec) -> Result<Value, ScriptError> {
        let def = CfRuleDef {
            spec,
            format: CfFormat::default(),
            stop_if_true: false,
            num_fmt: None,
            border: None,
        };
        let idx = self.s.sheet_index(self.sheet)?;
        let range = sqref_of(&self.areas);
        let mut book = self.s.write()?;
        let index = book
            .batch(|b| {
                b.add_cf_rule(idx, &range, &def)?;
                let all = b.cf_rules(idx, None);
                // The engine adds it first and stores it last.
                let index = all.iter().map(|r| r.index).max().unwrap_or(0);
                for _ in 1..all.len() {
                    b.move_cf_rule(idx, index, false)?;
                }
                Ok(index)
            })
            .map_err(engine)?;
        drop(book);
        Ok(self.item(index))
    }
}

impl Dispatch for FormatConditionsObj {
    fn type_name(&self) -> String {
        "FormatConditions".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "count" => Ok(Value::Long(self.rules()?.len() as i32)),
            "" | "item" => {
                let i = to_i64(need(args, 0, "Index")?)?;
                let rules = self.rules()?;
                usize::try_from(i - 1)
                    .ok()
                    .and_then(|k| rules.get(k))
                    .map(|r| self.item(r.index))
                    .ok_or_else(|| ScriptError::runtime(9))
            }
            "add" => {
                let kind = to_i64(need(args, 0, "Type")?)?;
                self.add(spec_of(kind, args)?)
            }
            "addcolorscale" => {
                let n = arg_i64(args, 0, "ColorScaleType")?.unwrap_or(3);
                let colors = match n {
                    2 => SCALE2.to_vec(),
                    3 => SCALE3.to_vec(),
                    _ => return Err(ScriptError::runtime(5)),
                };
                self.add(CfSpec::ColorScale { colors })
            }
            "adddatabar" => self.add(spec_of(XL_DATABAR, &[])?),
            "addiconsetcondition" => self.add(spec_of(XL_ICON_SETS, &[])?),
            "addtop10" => self.add(spec_of(XL_TOP10, &[])?),
            "addaboveaverage" => self.add(spec_of(XL_ABOVE_AVERAGE_CONDITION, &[])?),
            "adduniquevalues" => self.add(spec_of(XL_UNIQUE_VALUES, &[])?),
            "delete" => {
                let idx = self.s.sheet_index(self.sheet)?;
                let mut book = self.s.write()?;
                let areas = self.areas.clone();
                book.batch(|b| {
                    for a in areas {
                        b.clear_cf_rules(idx, Some(a))?;
                    }
                    Ok(())
                })
                .map_err(engine)?;
                Ok(Value::Empty)
            }
            _ => Err(ScriptError::no_member("FormatConditions", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok(self.rules()?.iter().map(|r| self.item(r.index)).collect())
    }
}

/// One rule (`FormatCondition`, `ColorScale`, `Databar`,
/// `IconSetCondition`, `Top10`, `AboveAverage`, `UniqueValues`).
#[derive(Clone)]
pub struct FormatConditionObj {
    pub s: Rc<Session>,
    pub sheet: u32,
    /// The rule's storage index in the sheet.
    pub index: usize,
}

/// A value measured as `kind` given to a rule that had none: what Excel
/// starts from.
fn default_value(kind: CfValueKind) -> &'static str {
    match kind {
        CfValueKind::Percent | CfValueKind::Percentile => "50",
        CfValueKind::Formula => "=0",
        _ => "0",
    }
}

fn value_kind(t: i64) -> Result<CfValueKind, ScriptError> {
    Ok(match t {
        XL_CONDITION_VALUE_NUMBER => CfValueKind::Number,
        XL_CONDITION_VALUE_LOWEST_VALUE => CfValueKind::Lowest,
        XL_CONDITION_VALUE_HIGHEST_VALUE => CfValueKind::Highest,
        XL_CONDITION_VALUE_PERCENT => CfValueKind::Percent,
        XL_CONDITION_VALUE_FORMULA => CfValueKind::Formula,
        XL_CONDITION_VALUE_PERCENTILE => CfValueKind::Percentile,
        XL_CONDITION_VALUE_AUTOMATIC_MIN
        | XL_CONDITION_VALUE_AUTOMATIC_MAX
        | XL_CONDITION_VALUE_NONE => CfValueKind::Automatic,
        _ => return Err(ScriptError::runtime(5)),
    })
}

fn value_type(k: CfValueKind, max: bool) -> i64 {
    match k {
        CfValueKind::Automatic if max => XL_CONDITION_VALUE_AUTOMATIC_MAX,
        CfValueKind::Automatic => XL_CONDITION_VALUE_AUTOMATIC_MIN,
        CfValueKind::Lowest => XL_CONDITION_VALUE_LOWEST_VALUE,
        CfValueKind::Highest => XL_CONDITION_VALUE_HIGHEST_VALUE,
        CfValueKind::Number => XL_CONDITION_VALUE_NUMBER,
        CfValueKind::Percent => XL_CONDITION_VALUE_PERCENT,
        CfValueKind::Percentile => XL_CONDITION_VALUE_PERCENTILE,
        CfValueKind::Formula => XL_CONDITION_VALUE_FORMULA,
    }
}

/// A point's `Type` changed: the value stays when it fits.
fn retype(v: &mut CfValue, kind: CfValueKind) {
    v.kind = kind;
    if !kind.has_value() {
        v.value.clear();
    } else if v.value.trim().is_empty()
        || (kind != CfValueKind::Formula && v.value.parse::<f64>().is_err())
    {
        v.value = default_value(kind).to_owned();
    }
}

/// A point's `Value`: a number, or a formula for the `Formula` type.
fn set_point_value(v: &mut CfValue, value: &Value) -> Result<(), ScriptError> {
    v.value = if v.kind == CfValueKind::Formula {
        let f = formula_text(value)?;
        format!("={}", f.trim().trim_start_matches('='))
    } else {
        number_text(to_f64(value)?)
    };
    Ok(())
}

fn point_value(v: &CfValue) -> Value {
    if !v.kind.has_value() {
        return Value::Empty;
    }
    match v.value.trim().parse::<f64>() {
        Ok(n) => Value::Double(n),
        Err(_) => Value::String(v.value.clone()),
    }
}

fn color_value(c: Option<u32>) -> Value {
    c.map_or(Value::Null, |c| Value::Long(rgb_to_bgr(c)))
}

/// `ColorIndex` given to a rule's colour (`None`: none, automatic).
fn color_from_index(i: i64) -> Result<Option<u32>, ScriptError> {
    if i == XL_COLOR_INDEX_NONE || i == XL_COLOR_INDEX_AUTOMATIC {
        Ok(None)
    } else {
        palette(i).map(Some)
    }
}

impl FormatConditionObj {
    fn idx(&self) -> Result<u32, ScriptError> {
        self.s.sheet_index(self.sheet)
    }

    fn info(&self) -> Result<CfRuleInfo, ScriptError> {
        let idx = self.idx()?;
        let book = self.s.read()?;
        book.cf_rules(idx, None)
            .into_iter()
            .find(|r| r.index == self.index)
            .ok_or_else(|| err1004("The conditional format was deleted"))
    }

    fn def(&self) -> Result<(CfRuleInfo, CfRuleDef), ScriptError> {
        let info = self.info()?;
        let def = info
            .def
            .clone()
            .ok_or_else(|| ScriptError::unsupported("Changing this kind of conditional format"))?;
        Ok((info, def))
    }

    /// Change the rule (one edit).
    fn update(
        &self,
        change: impl FnOnce(&mut CfRuleDef) -> Result<(), ScriptError>,
    ) -> Result<(), ScriptError> {
        let (info, mut def) = self.def()?;
        change(&mut def)?;
        let idx = self.idx()?;
        self.s
            .write()?
            .update_cf_rule(idx, self.index, &info.range, &def)
            .map_err(engine)
    }

    /// Change a data bar's settings.
    fn update_bar(
        &self,
        change: impl FnOnce(&mut u32, &mut bool, &mut CfBarOptions) -> Result<(), ScriptError>,
    ) -> Result<(), ScriptError> {
        self.update(|d| match &mut d.spec {
            CfSpec::DataBar {
                color,
                gradient,
                bar,
            } => change(color, gradient, bar),
            _ => Err(ScriptError::runtime(438)),
        })
    }

    /// Change an icon set (its steps filled in when it uses the defaults).
    fn update_icons(
        &self,
        change: impl FnOnce(
            &mut String,
            &mut bool,
            &mut bool,
            &mut Vec<CfIconStep>,
        ) -> Result<(), ScriptError>,
    ) -> Result<(), ScriptError> {
        self.update(|d| match &mut d.spec {
            CfSpec::IconSet {
                set,
                reverse,
                show_value,
                steps,
            } => {
                if steps.is_empty() {
                    *steps = default_icon_steps(set);
                }
                change(set, reverse, show_value, steps)
            }
            _ => Err(ScriptError::runtime(438)),
        })
    }

    /// An icon set's steps, lowest first.
    fn icon_steps(&self) -> Result<Vec<CfIconStep>, ScriptError> {
        match self.def()?.1.spec {
            CfSpec::IconSet { set, steps, .. } if steps.is_empty() => Ok(default_icon_steps(&set)),
            CfSpec::IconSet { steps, .. } => Ok(steps),
            _ => Err(ScriptError::runtime(438)),
        }
    }

    fn scale_points(&self) -> Result<Vec<(CfValue, u32)>, ScriptError> {
        let idx = self.idx()?;
        self.s
            .read()?
            .cf_color_scale(idx, self.index)
            .ok_or_else(|| ScriptError::runtime(438))
    }

    fn set_scale_points(&self, points: &[(CfValue, u32)]) -> Result<(), ScriptError> {
        let idx = self.idx()?;
        self.s
            .write()?
            .set_cf_color_scale(idx, self.index, points)
            .map_err(engine)
    }

    /// Move to place `target` (0-based) in the sheet's priority order.
    fn move_to(&self, target: usize) -> Result<(), ScriptError> {
        let idx = self.idx()?;
        let mut book = self.s.write()?;
        let all = book.cf_rules(idx, None);
        let at = all
            .iter()
            .position(|r| r.index == self.index)
            .ok_or_else(|| err1004("The conditional format was deleted"))?;
        let target = target.min(all.len().saturating_sub(1));
        if at == target {
            return Ok(());
        }
        let up = target < at;
        let steps = at.abs_diff(target);
        let index = self.index;
        book.batch(|b| {
            for _ in 0..steps {
                b.move_cf_rule(idx, index, up)?;
            }
            Ok(())
        })
        .map_err(engine)
    }

    fn sub(&self) -> FormatConditionObj {
        self.clone()
    }
}

impl Dispatch for FormatConditionObj {
    fn type_name(&self) -> String {
        let info = self.info().ok();
        type_name_of(info.as_ref().and_then(|i| i.def.as_ref()).map(|d| &d.spec)).into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        let info = self.info()?;
        let tname = type_name_of(info.def.as_ref().map(|d| &d.spec));
        let no_member =
            || -> Result<Value, ScriptError> { Err(ScriptError::no_member(tname, name)) };
        let spec = info.def.as_ref().map(|d| d.spec.clone());
        let formats = spec.as_ref().is_some_and(CfSpec::has_format);
        // Members of every rule.
        match lower.as_str() {
            "type" => return Ok(Value::Long(type_of(&info) as i32)),
            "priority" => {
                let idx = self.idx()?;
                let all = self.s.read()?.cf_rules(idx, None);
                let at = all.iter().position(|r| r.index == self.index).unwrap_or(0);
                return Ok(Value::Long(at as i32 + 1));
            }
            "setfirstpriority" => {
                self.move_to(0)?;
                return Ok(Value::Empty);
            }
            "setlastpriority" => {
                self.move_to(usize::MAX)?;
                return Ok(Value::Empty);
            }
            "stopiftrue" => return Ok(Value::Boolean(info.stop_if_true)),
            "appliesto" => {
                let areas: Vec<CellRange> = info
                    .range
                    .split_whitespace()
                    .filter_map(addr::parse_area)
                    .collect();
                if areas.is_empty() {
                    return Err(err1004("Unable to get the AppliesTo property"));
                }
                return Ok(obj(RangeObj::multi(&self.s, self.sheet, areas)));
            }
            "modifyappliestorange" => {
                let r =
                    range_of(need(args, 0, "Range")?).ok_or_else(|| ScriptError::runtime(424))?;
                if r.sheet != self.sheet {
                    return Err(err1004(
                        "The range a conditional format applies to must be on its sheet",
                    ));
                }
                let (_, def) = self.def()?;
                let idx = self.idx()?;
                let range = sqref_of(&r.areas);
                self.s
                    .write()?
                    .update_cf_rule(idx, self.index, &range, &def)
                    .map_err(engine)?;
                return Ok(Value::Empty);
            }
            "delete" => {
                let idx = self.idx()?;
                self.s
                    .write()?
                    .delete_cf_rule(idx, self.index)
                    .map_err(engine)?;
                return Ok(Value::Empty);
            }
            "scopetype" | "ptcondition" => return Ok(Value::Long(0)),
            _ => {}
        }
        if formats {
            match lower.as_str() {
                "interior" => return Ok(obj(Rc::new(CfInteriorObj { c: self.sub() }))),
                "font" => return Ok(obj(Rc::new(CfFontObj { c: self.sub() }))),
                "borders" => {
                    let edge = arg_i64(args, 0, "Index")?.map(side_of).transpose()?;
                    return Ok(obj(Rc::new(CfBordersObj {
                        c: self.sub(),
                        side: edge,
                    })));
                }
                "numberformat" => {
                    return Ok(match info.def.as_ref().and_then(|d| d.num_fmt.clone()) {
                        Some(f) => Value::String(f),
                        None => Value::Null,
                    });
                }
                _ => {}
            }
        }
        match (lower.as_str(), spec) {
            ("operator", Some(CfSpec::CellValue { op, .. })) => {
                Ok(Value::Long(operator_of(op) as i32))
            }
            ("formula1", Some(CfSpec::CellValue { value, .. })) => {
                Ok(Value::String(formula_of_value(&value)))
            }
            ("formula2", Some(CfSpec::CellValue { op, value2, .. })) => {
                if op.two_values() {
                    Ok(Value::String(formula_of_value(&value2)))
                } else {
                    Err(err1004("Application-defined or object-defined error"))
                }
            }
            ("formula1", Some(CfSpec::Formula { formula })) => Ok(Value::String(formula)),
            ("modify", Some(_)) => {
                let kind = to_i64(need(args, 0, "Type")?)?;
                let spec = spec_of(kind, args)?;
                self.update(|d| {
                    d.spec = spec;
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            ("text", Some(CfSpec::Text { text, .. })) => Ok(Value::String(text)),
            ("textoperator", Some(CfSpec::Text { op, .. })) => {
                Ok(Value::Long(text_operator_of(op) as i32))
            }
            ("dateoperator", Some(CfSpec::Dates { period })) => {
                Ok(Value::Long(date_operator_of(period) as i32))
            }
            ("topbottom", Some(CfSpec::TopBottom { bottom, .. })) => Ok(Value::Long(if bottom {
                XL_TOP10_BOTTOM as i32
            } else {
                XL_TOP10_TOP as i32
            })),
            ("rank", Some(CfSpec::TopBottom { rank, .. })) => Ok(Value::Long(rank as i32)),
            ("percent", Some(CfSpec::TopBottom { percent, .. })) => Ok(Value::Boolean(percent)),
            (
                "abovebelow",
                Some(CfSpec::Average {
                    below,
                    equal,
                    std_dev,
                }),
            ) => Ok(Value::Long(match (below, equal, std_dev > 0) {
                (false, _, true) => XL_ABOVE_STD_DEV,
                (true, _, true) => XL_BELOW_STD_DEV,
                (false, true, false) => XL_EQUAL_ABOVE_AVERAGE,
                (true, true, false) => XL_EQUAL_BELOW_AVERAGE,
                (false, false, false) => XL_ABOVE_AVERAGE,
                (true, false, false) => XL_BELOW_AVERAGE,
            } as i32)),
            ("numstddev", Some(CfSpec::Average { std_dev, .. })) => Ok(Value::Long(std_dev as i32)),
            ("dupeunique", Some(CfSpec::Duplicates { unique })) => Ok(Value::Long(if unique {
                XL_UNIQUE as i32
            } else {
                XL_DUPLICATE as i32
            })),
            ("barcolor", Some(CfSpec::DataBar { .. })) => Ok(obj(Rc::new(FormatColorObj {
                c: self.sub(),
                target: ColorTarget::Bar,
            }))),
            ("barfilltype", Some(CfSpec::DataBar { gradient, .. })) => {
                Ok(Value::Long(if gradient {
                    XL_DATA_BAR_FILL_GRADIENT as i32
                } else {
                    XL_DATA_BAR_FILL_SOLID as i32
                }))
            }
            ("showvalue", Some(CfSpec::DataBar { bar, .. })) => Ok(Value::Boolean(!bar.bar_only)),
            ("minpoint" | "maxpoint", Some(CfSpec::DataBar { .. })) => {
                Ok(obj(Rc::new(ConditionValueObj {
                    c: self.sub(),
                    max: lower == "maxpoint",
                })))
            }
            ("percentmin", Some(CfSpec::DataBar { bar, .. })) => {
                Ok(Value::Long(i32::from(bar.min_length)))
            }
            ("percentmax", Some(CfSpec::DataBar { bar, .. })) => {
                Ok(Value::Long(i32::from(bar.max_length)))
            }
            ("direction", Some(CfSpec::DataBar { bar, .. })) => {
                Ok(Value::Long(match bar.direction {
                    CfBarDirection::Context => XL_CONTEXT,
                    CfBarDirection::LeftToRight => XL_LTR,
                    CfBarDirection::RightToLeft => XL_RTL,
                } as i32))
            }
            ("axisposition", Some(CfSpec::DataBar { bar, .. })) => {
                Ok(Value::Long(match bar.axis {
                    CfBarAxis::Automatic => XL_DATA_BAR_AXIS_AUTOMATIC,
                    CfBarAxis::Middle => XL_DATA_BAR_AXIS_MIDPOINT,
                    CfBarAxis::None => XL_DATA_BAR_AXIS_NONE,
                } as i32))
            }
            ("barborder", Some(CfSpec::DataBar { .. })) => {
                Ok(obj(Rc::new(BarBorderObj { c: self.sub() })))
            }
            ("negativebarformat" | "axiscolor", Some(CfSpec::DataBar { .. })) => Err(
                ScriptError::unsupported("Negative bar and axis settings of data bars in macros"),
            ),
            ("colorscalecriteria", Some(CfSpec::ColorScale { .. })) => {
                let all = Rc::new(ColorScaleCriteriaObj { c: self.sub() });
                if arg(args, 0, "Index").is_some() {
                    return all.get("Item", args);
                }
                Ok(obj(all))
            }
            ("iconset", Some(CfSpec::IconSet { set, .. })) => Ok(obj(Rc::new(IconSetObj {
                id: icon_set_id(&set),
            }))),
            ("reverseorder", Some(CfSpec::IconSet { reverse, .. })) => Ok(Value::Boolean(reverse)),
            ("showicononly", Some(CfSpec::IconSet { show_value, .. })) => {
                Ok(Value::Boolean(!show_value))
            }
            ("iconcriteria", Some(CfSpec::IconSet { .. })) => {
                let all = Rc::new(IconCriteriaObj { c: self.sub() });
                if arg(args, 0, "Index").is_some() {
                    return all.get("Item", args);
                }
                Ok(obj(all))
            }
            ("percentilevalues", Some(CfSpec::IconSet { .. })) => Ok(Value::Boolean(false)),
            _ => no_member(),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "stopiftrue" => {
                let on = to_bool(&value)?;
                return self.update(|d| {
                    d.stop_if_true = on;
                    Ok(())
                });
            }
            "priority" => {
                let p = to_i64(&value)?;
                if p < 1 {
                    return Err(err1004("Unable to set the Priority property"));
                }
                return self.move_to(p as usize - 1);
            }
            "numberformat" => {
                let f = to_str(&value)?;
                return self.update(|d| {
                    if !d.spec.has_format() {
                        return Err(ScriptError::runtime(438));
                    }
                    d.num_fmt = (!f.is_empty() && !f.eq_ignore_ascii_case("general")).then_some(f);
                    Ok(())
                });
            }
            "scopetype" => return Ok(()),
            _ => {}
        }
        let tname = self.type_name();
        let wrong = || ScriptError::no_member(&tname, name);
        match lower.as_str() {
            "formula1" | "formula2" => {
                let f = formula_text(&value)?;
                let second = lower == "formula2";
                self.update(|d| match &mut d.spec {
                    CfSpec::CellValue { value, value2, .. } => {
                        if second {
                            *value2 = f;
                        } else {
                            *value = f;
                        }
                        Ok(())
                    }
                    CfSpec::Formula { formula } if !second => {
                        *formula = format!("={}", f.trim().trim_start_matches('='));
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "operator" => {
                let op = compare_of(to_i64(&value)?)?;
                self.update(|d| match &mut d.spec {
                    CfSpec::CellValue { op: o, .. } => {
                        *o = op;
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "text" => {
                let t = to_str(&value)?;
                self.update(|d| match &mut d.spec {
                    CfSpec::Text { text, .. } => {
                        *text = t;
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "textoperator" => {
                let op = text_op_of(to_i64(&value)?)?;
                self.update(|d| match &mut d.spec {
                    CfSpec::Text { op: o, .. } => {
                        *o = op;
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "dateoperator" => {
                let p = period_of(to_i64(&value)?)?;
                self.update(|d| match &mut d.spec {
                    CfSpec::Dates { period } => {
                        *period = p;
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "topbottom" => {
                let b = to_i64(&value)? == XL_TOP10_BOTTOM;
                self.update(|d| match &mut d.spec {
                    CfSpec::TopBottom { bottom, .. } => {
                        *bottom = b;
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "rank" => {
                let n = to_i64(&value)?;
                let n = u32::try_from(n).map_err(|_| err1004("Unable to set the Rank property"))?;
                self.update(|d| match &mut d.spec {
                    CfSpec::TopBottom { rank, .. } => {
                        *rank = n;
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "percent" => {
                let p = to_bool(&value)?;
                self.update(|d| match &mut d.spec {
                    CfSpec::TopBottom { percent, .. } => {
                        *percent = p;
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "abovebelow" => {
                let n = to_i64(&value)?;
                self.update(|d| match &mut d.spec {
                    CfSpec::Average {
                        below,
                        equal,
                        std_dev,
                    } => {
                        let (b, e, sd) = match n {
                            XL_ABOVE_AVERAGE => (false, false, false),
                            XL_BELOW_AVERAGE => (true, false, false),
                            XL_EQUAL_ABOVE_AVERAGE => (false, true, false),
                            XL_EQUAL_BELOW_AVERAGE => (true, true, false),
                            XL_ABOVE_STD_DEV => (false, false, true),
                            XL_BELOW_STD_DEV => (true, false, true),
                            _ => return Err(ScriptError::runtime(5)),
                        };
                        *below = b;
                        *equal = e;
                        if !sd {
                            *std_dev = 0;
                        } else if *std_dev == 0 {
                            *std_dev = 1;
                        }
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "numstddev" => {
                let n = u32::try_from(to_i64(&value)?)
                    .map_err(|_| err1004("Unable to set the NumStdDev property"))?;
                self.update(|d| match &mut d.spec {
                    CfSpec::Average { std_dev, .. } => {
                        *std_dev = n;
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "dupeunique" => {
                let u = to_i64(&value)? == XL_UNIQUE;
                self.update(|d| match &mut d.spec {
                    CfSpec::Duplicates { unique } => {
                        *unique = u;
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "barfilltype" => {
                let g = to_i64(&value)? == XL_DATA_BAR_FILL_GRADIENT;
                self.update_bar(|_, gradient, _| {
                    *gradient = g;
                    Ok(())
                })
            }
            "showvalue" => {
                let show = to_bool(&value)?;
                self.update_bar(|_, _, bar| {
                    bar.bar_only = !show;
                    Ok(())
                })
            }
            "percentmin" | "percentmax" => {
                let n = to_i64(&value)?;
                let n = u8::try_from(n)
                    .ok()
                    .filter(|n| *n <= 100)
                    .ok_or_else(|| err1004("The bar length is a percentage (0 to 100)"))?;
                let max = lower == "percentmax";
                self.update_bar(|_, _, bar| {
                    if max {
                        bar.max_length = n;
                    } else {
                        bar.min_length = n;
                    }
                    Ok(())
                })
            }
            "direction" => {
                let dir = match to_i64(&value)? {
                    XL_CONTEXT => CfBarDirection::Context,
                    XL_LTR => CfBarDirection::LeftToRight,
                    XL_RTL => CfBarDirection::RightToLeft,
                    _ => return Err(ScriptError::runtime(5)),
                };
                self.update_bar(|_, _, bar| {
                    bar.direction = dir;
                    Ok(())
                })
            }
            "axisposition" => {
                let axis = match to_i64(&value)? {
                    XL_DATA_BAR_AXIS_AUTOMATIC => CfBarAxis::Automatic,
                    XL_DATA_BAR_AXIS_MIDPOINT => CfBarAxis::Middle,
                    XL_DATA_BAR_AXIS_NONE => CfBarAxis::None,
                    _ => return Err(ScriptError::runtime(5)),
                };
                self.update_bar(|_, _, bar| {
                    bar.axis = axis;
                    Ok(())
                })
            }
            "iconset" => {
                // `= ActiveWorkbook.IconSets(xl3Arrows)` arrives as the
                // IconSet's default member, its number.
                let id = match &value {
                    Value::Object(Some(o)) => to_i64(&o.get("ID", &[])?)?,
                    v => to_i64(v)?,
                };
                let name = icon_set_name(id)?;
                self.update(|d| match &mut d.spec {
                    CfSpec::IconSet { set, steps, .. } => {
                        *set = name.to_owned();
                        steps.clear();
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "reverseorder" => {
                let r = to_bool(&value)?;
                self.update(|d| match &mut d.spec {
                    CfSpec::IconSet { reverse, .. } => {
                        *reverse = r;
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "showicononly" => {
                let only = to_bool(&value)?;
                self.update(|d| match &mut d.spec {
                    CfSpec::IconSet { show_value, .. } => {
                        *show_value = !only;
                        Ok(())
                    }
                    _ => Err(wrong()),
                })
            }
            "percentilevalues" => Ok(()),
            _ => Err(wrong()),
        }
    }
}

/// `FormatCondition.Font`: the rule's font colour and style.
pub struct CfFontObj {
    c: FormatConditionObj,
}

impl Dispatch for CfFontObj {
    fn type_name(&self) -> String {
        "Font".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let f = self.c.info()?.format;
        Ok(match name.to_ascii_lowercase().as_str() {
            "bold" => Value::Boolean(f.bold),
            "italic" => Value::Boolean(f.italic),
            "strikethrough" => Value::Boolean(f.strike),
            "underline" => Value::Long(if f.underline {
                XL_UNDERLINE_STYLE_SINGLE as i32
            } else {
                XL_UNDERLINE_STYLE_NONE as i32
            }),
            "color" => color_value(f.font),
            "colorindex" => match f.font {
                None => Value::Long(XL_COLOR_INDEX_AUTOMATIC as i32),
                Some(c) => Value::Long(color_index(c)),
            },
            "fontstyle" => Value::String(
                match (f.bold, f.italic) {
                    (true, true) => "Bold Italic",
                    (true, false) => "Bold",
                    (false, true) => "Italic",
                    (false, false) => "Regular",
                }
                .into(),
            ),
            "themecolor" | "tintandshade" => Value::Long(0),
            "name" | "size" => Value::Null,
            _ => return Err(ScriptError::no_member("Font", name)),
        })
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        let change: Box<dyn FnOnce(&mut CfFormat)> = match lower.as_str() {
            "bold" => {
                let b = to_bool(&value)?;
                Box::new(move |f| f.bold = b)
            }
            "italic" => {
                let b = to_bool(&value)?;
                Box::new(move |f| f.italic = b)
            }
            "strikethrough" => {
                let b = to_bool(&value)?;
                Box::new(move |f| f.strike = b)
            }
            "underline" => {
                let on = match scalar(&value)? {
                    Value::Boolean(b) => b,
                    v => to_i64(&v)? != XL_UNDERLINE_STYLE_NONE,
                };
                Box::new(move |f| f.underline = on)
            }
            "color" => {
                let c = bgr_to_rgb(to_i64(&value)?);
                Box::new(move |f| f.font = Some(c))
            }
            "colorindex" => {
                let c = color_from_index(to_i64(&value)?)?;
                Box::new(move |f| f.font = c)
            }
            "fontstyle" => {
                let v = to_str(&value)?.to_lowercase();
                let (b, i) = (v.contains("bold"), v.contains("italic"));
                Box::new(move |f| {
                    f.bold = b;
                    f.italic = i;
                })
            }
            "themecolor" | "tintandshade" | "themefont" => return Ok(()),
            "name" | "size" => {
                return Err(err1004(format!(
                    "Unable to set the {name} property of the Font class"
                )));
            }
            _ => return Err(ScriptError::no_member("Font", name)),
        };
        self.c.update(|d| {
            change(&mut d.format);
            Ok(())
        })
    }
}

/// `FormatCondition.Interior`: the rule's fill.
pub struct CfInteriorObj {
    c: FormatConditionObj,
}

impl Dispatch for CfInteriorObj {
    fn type_name(&self) -> String {
        "Interior".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let f = self.c.info()?.format;
        Ok(match name.to_ascii_lowercase().as_str() {
            "color" => color_value(f.fill),
            "colorindex" => match f.fill {
                None => Value::Long(XL_COLOR_INDEX_NONE as i32),
                Some(c) => Value::Long(color_index(c)),
            },
            "pattern" => Value::Long(if f.fill.is_some() {
                XL_SOLID as i32
            } else {
                XL_NONE as i32
            }),
            "patterncolorindex" => Value::Long(XL_AUTOMATIC as i32),
            "patterncolor" => Value::Long(0),
            "themecolor" | "tintandshade" | "patterntintandshade" => Value::Long(0),
            _ => return Err(ScriptError::no_member("Interior", name)),
        })
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let fill = match name.to_ascii_lowercase().as_str() {
            "color" => Some(bgr_to_rgb(to_i64(&value)?)),
            "colorindex" => color_from_index(to_i64(&value)?)?,
            "pattern" => match to_i64(&value)? {
                XL_NONE => None,
                XL_SOLID => return Ok(()),
                _ => {
                    return Err(ScriptError::unsupported(
                        "A patterned fill in a conditional format (Kantor keeps solid fills)",
                    ));
                }
            },
            "patterncolorindex"
            | "patterncolor"
            | "themecolor"
            | "tintandshade"
            | "patterntintandshade" => return Ok(()),
            _ => return Err(ScriptError::no_member("Interior", name)),
        };
        self.c.update(|d| {
            d.format.fill = fill;
            Ok(())
        })
    }
}

/// A conditional format's border side (0 left, 1 right, 2 top, 3 bottom)
/// by `XlBordersIndex` (`xlLeft` or `xlEdgeLeft`...).
fn side_of(edge: i64) -> Result<usize, ScriptError> {
    Ok(match edge {
        XL_LEFT | XL_EDGE_LEFT => 0,
        XL_RIGHT | XL_EDGE_RIGHT => 1,
        XL_TOP | XL_EDGE_TOP => 2,
        XL_BOTTOM | XL_EDGE_BOTTOM => 3,
        _ => return Err(ScriptError::runtime(9)),
    })
}

/// `FormatCondition.Borders` (all four sides) or `Borders(xlLeft)`.
pub struct CfBordersObj {
    c: FormatConditionObj,
    side: Option<usize>,
}

impl CfBordersObj {
    fn sides(&self) -> Result<[Option<sheets_engine::BorderLine>; 4], ScriptError> {
        let (_, def) = self.c.def()?;
        Ok(match &def.border {
            Some(b) => self.c.s.read()?.cf_border_sides(b),
            None => [None; 4],
        })
    }

    /// Set the chosen sides with `change(current)`.
    fn change(
        &self,
        change: impl Fn(Option<sheets_engine::BorderLine>) -> Option<sheets_engine::BorderLine>,
    ) -> Result<(), ScriptError> {
        let mut sides = self.sides()?;
        for (k, s) in sides.iter_mut().enumerate() {
            if self.side.is_none_or(|w| w == k) {
                *s = change(*s);
            }
        }
        let border = Workbook::cf_border_of(&sides).map_err(engine)?;
        self.c.update(|d| {
            if !d.spec.has_format() {
                return Err(ScriptError::runtime(438));
            }
            d.border = border;
            Ok(())
        })
    }
}

impl Dispatch for CfBordersObj {
    fn type_name(&self) -> String {
        if self.side.is_some() {
            "Border".into()
        } else {
            "Borders".into()
        }
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        let lower = name.to_ascii_lowercase();
        if (lower.is_empty() || lower == "item") && self.side.is_none() {
            let side = side_of(to_i64(need(args, 0, "Index")?)?)?;
            return Ok(obj(Rc::new(CfBordersObj {
                c: self.c.clone(),
                side: Some(side),
            })));
        }
        if lower == "count" {
            return Ok(Value::Long(4));
        }
        let sides = self.sides()?;
        let line = match self.side {
            Some(k) => sides[k],
            None => sides.iter().flatten().next().copied(),
        };
        Ok(match lower.as_str() {
            "linestyle" => Value::Long(line_style_of(line.map(|l| l.style)) as i32),
            "weight" => Value::Long(weight_of(line.map(|l| l.style)) as i32),
            "color" => color_value(line.and_then(|l| l.color)),
            "colorindex" => match line.and_then(|l| l.color) {
                None => Value::Long(XL_COLOR_INDEX_AUTOMATIC as i32),
                Some(c) => Value::Long(color_index(c)),
            },
            "themecolor" | "tintandshade" => Value::Long(0),
            _ => return Err(ScriptError::no_member("Borders", name)),
        })
    }

    fn put(&self, name: &str, args: &[Arg], value: Value) -> Result<(), ScriptError> {
        use sheets_engine::BorderLine;
        let lower = name.to_ascii_lowercase();
        if (lower.is_empty() || lower == "item") && !args.is_empty() {
            return match self.get(name, args)? {
                Value::Object(Some(o)) => o.put("LineStyle", &[], value),
                _ => Err(ScriptError::runtime(424)),
            };
        }
        match lower.as_str() {
            "linestyle" => {
                let line = to_i64(&value)?;
                self.change(|cur| {
                    line_style(line, weight_of(cur.map(|l| l.style))).map(|style| BorderLine {
                        style,
                        color: cur.and_then(|l| l.color),
                    })
                })
            }
            "weight" => {
                let weight = to_i64(&value)?;
                self.change(|cur| {
                    let keep = line_style_of(cur.map(|l| l.style));
                    let keep = if keep == XL_NONE { XL_CONTINUOUS } else { keep };
                    Some(BorderLine {
                        style: line_style(keep, weight).unwrap_or(LineStyle::Thin),
                        color: cur.and_then(|l| l.color),
                    })
                })
            }
            "color" | "colorindex" => {
                let n = to_i64(&value)?;
                let color = if lower == "color" {
                    Some(bgr_to_rgb(n))
                } else {
                    color_from_index(n)?
                };
                self.change(|cur| {
                    Some(BorderLine {
                        style: cur.map_or(LineStyle::Thin, |l| l.style),
                        color,
                    })
                })
            }
            "themecolor" | "tintandshade" => Ok(()),
            _ => Err(ScriptError::no_member("Borders", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok((0..4)
            .map(|k| {
                obj(Rc::new(CfBordersObj {
                    c: self.c.clone(),
                    side: Some(k),
                }))
            })
            .collect())
    }
}

/// Whose colour a `FormatColor` is.
#[derive(Clone, Copy)]
enum ColorTarget {
    /// A data bar's fill.
    Bar,
    /// A data bar's border.
    BarBorder,
    /// A colour scale's point (0-based).
    Scale(usize),
}

/// `Databar.BarColor`, `ColorScaleCriterion.FormatColor`.
pub struct FormatColorObj {
    c: FormatConditionObj,
    target: ColorTarget,
}

impl FormatColorObj {
    fn color(&self) -> Result<Option<u32>, ScriptError> {
        Ok(match self.target {
            ColorTarget::Scale(i) => self.c.scale_points()?.get(i).map(|p| p.1),
            ColorTarget::Bar | ColorTarget::BarBorder => match self.c.def()?.1.spec {
                CfSpec::DataBar { color, bar, .. } => match self.target {
                    ColorTarget::Bar => Some(color),
                    _ => bar.border,
                },
                _ => None,
            },
        })
    }

    fn set(&self, rgb: u32) -> Result<(), ScriptError> {
        match self.target {
            ColorTarget::Scale(i) => {
                let mut points = self.c.scale_points()?;
                let p = points.get_mut(i).ok_or_else(|| ScriptError::runtime(9))?;
                p.1 = rgb;
                self.c.set_scale_points(&points)
            }
            ColorTarget::Bar => self.c.update_bar(|color, _, _| {
                *color = rgb;
                Ok(())
            }),
            ColorTarget::BarBorder => self.c.update_bar(|_, _, bar| {
                bar.border = Some(rgb);
                Ok(())
            }),
        }
    }
}

impl Dispatch for FormatColorObj {
    fn type_name(&self) -> String {
        "FormatColor".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        Ok(match name.to_ascii_lowercase().as_str() {
            "color" | "" => Value::Long(rgb_to_bgr(self.color()?.unwrap_or(0))),
            "colorindex" => Value::Long(match self.color()? {
                None => XL_COLOR_INDEX_AUTOMATIC as i32,
                Some(c) => color_index(c),
            }),
            "themecolor" | "tintandshade" => Value::Long(0),
            _ => return Err(ScriptError::no_member("FormatColor", name)),
        })
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "color" | "" => self.set(bgr_to_rgb(to_i64(&value)?)),
            "colorindex" => match color_from_index(to_i64(&value)?)? {
                Some(c) => self.set(c),
                None => Ok(()),
            },
            "themecolor" | "tintandshade" => Ok(()),
            _ => Err(ScriptError::no_member("FormatColor", name)),
        }
    }
}

/// `Databar.BarBorder`.
pub struct BarBorderObj {
    c: FormatConditionObj,
}

impl Dispatch for BarBorderObj {
    fn type_name(&self) -> String {
        "DataBarBorder".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "type" => {
                let solid = matches!(
                    self.c.def()?.1.spec,
                    CfSpec::DataBar { bar, .. } if bar.border.is_some()
                );
                Ok(Value::Long(if solid {
                    XL_DATA_BAR_BORDER_SOLID as i32
                } else {
                    XL_DATA_BAR_BORDER_NONE as i32
                }))
            }
            "color" => Ok(obj(Rc::new(FormatColorObj {
                c: self.c.clone(),
                target: ColorTarget::BarBorder,
            }))),
            _ => Err(ScriptError::no_member("DataBarBorder", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "type" => {
                let solid = to_i64(&value)? == XL_DATA_BAR_BORDER_SOLID;
                self.c.update_bar(|color, _, bar| {
                    bar.border = if solid {
                        Some(bar.border.unwrap_or(*color))
                    } else {
                        None
                    };
                    Ok(())
                })
            }
            _ => Err(ScriptError::no_member("DataBarBorder", name)),
        }
    }
}

/// `Databar.MinPoint` / `MaxPoint`.
pub struct ConditionValueObj {
    c: FormatConditionObj,
    max: bool,
}

impl ConditionValueObj {
    fn point(&self) -> Result<CfValue, ScriptError> {
        match self.c.def()?.1.spec {
            CfSpec::DataBar { bar, .. } => Ok(if self.max { bar.max } else { bar.min }),
            _ => Err(ScriptError::runtime(438)),
        }
    }

    fn set(
        &self,
        change: impl FnOnce(&mut CfValue) -> Result<(), ScriptError>,
    ) -> Result<(), ScriptError> {
        let max = self.max;
        self.c
            .update_bar(|_, _, bar| change(if max { &mut bar.max } else { &mut bar.min }))
    }
}

impl Dispatch for ConditionValueObj {
    fn type_name(&self) -> String {
        "ConditionValue".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "type" => Ok(Value::Long(value_type(self.point()?.kind, self.max) as i32)),
            "value" => Ok(point_value(&self.point()?)),
            "modify" => {
                let kind = value_kind(to_i64(need(args, 0, "newtype")?)?)?;
                let value = arg(args, 1, "newvalue").cloned();
                self.set(|p| {
                    retype(p, kind);
                    if let Some(v) = value.filter(|_| kind.has_value()) {
                        set_point_value(p, &v)?;
                    }
                    Ok(())
                })?;
                Ok(Value::Empty)
            }
            _ => Err(ScriptError::no_member("ConditionValue", name)),
        }
    }
}

/// `ColorScale.ColorScaleCriteria`.
pub struct ColorScaleCriteriaObj {
    c: FormatConditionObj,
}

impl Dispatch for ColorScaleCriteriaObj {
    fn type_name(&self) -> String {
        "ColorScaleCriteria".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "count" => Ok(Value::Long(self.c.scale_points()?.len() as i32)),
            "" | "item" => {
                let i = to_i64(need(args, 0, "Index")?)?;
                let n = self.c.scale_points()?.len() as i64;
                if !(1..=n).contains(&i) {
                    return Err(ScriptError::runtime(9));
                }
                Ok(obj(Rc::new(ColorScaleCriterionObj {
                    c: self.c.clone(),
                    i: i as usize - 1,
                })))
            }
            _ => Err(ScriptError::no_member("ColorScaleCriteria", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        let n = self.c.scale_points()?.len();
        Ok((0..n)
            .map(|i| {
                obj(Rc::new(ColorScaleCriterionObj {
                    c: self.c.clone(),
                    i,
                }))
            })
            .collect())
    }
}

/// One point of a colour scale.
pub struct ColorScaleCriterionObj {
    c: FormatConditionObj,
    i: usize,
}

impl ColorScaleCriterionObj {
    fn change(
        &self,
        change: impl FnOnce(&mut CfValue) -> Result<(), ScriptError>,
    ) -> Result<(), ScriptError> {
        let mut points = self.c.scale_points()?;
        let p = points
            .get_mut(self.i)
            .ok_or_else(|| ScriptError::runtime(9))?;
        change(&mut p.0)?;
        self.c.set_scale_points(&points)
    }
}

impl Dispatch for ColorScaleCriterionObj {
    fn type_name(&self) -> String {
        "ColorScaleCriterion".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let points = self.c.scale_points()?;
        let (point, _) = points.get(self.i).ok_or_else(|| ScriptError::runtime(9))?;
        let last = self.i + 1 == points.len();
        match name.to_ascii_lowercase().as_str() {
            "type" => Ok(Value::Long(value_type(point.kind, last) as i32)),
            "value" => Ok(point_value(point)),
            "index" => Ok(Value::Long(self.i as i32 + 1)),
            "formatcolor" => Ok(obj(Rc::new(FormatColorObj {
                c: self.c.clone(),
                target: ColorTarget::Scale(self.i),
            }))),
            _ => Err(ScriptError::no_member("ColorScaleCriterion", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "type" => {
                let kind = value_kind(to_i64(&value)?)?;
                self.change(|p| {
                    retype(p, kind);
                    Ok(())
                })
            }
            "value" => self.change(|p| {
                if !p.kind.has_value() {
                    return Err(err1004(
                        "Unable to set the Value property of the ColorScaleCriterion class",
                    ));
                }
                set_point_value(p, &value)
            }),
            _ => Err(ScriptError::no_member("ColorScaleCriterion", name)),
        }
    }
}

/// `IconSetCondition.IconCriteria`.
pub struct IconCriteriaObj {
    c: FormatConditionObj,
}

impl Dispatch for IconCriteriaObj {
    fn type_name(&self) -> String {
        "IconCriteria".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "count" => Ok(Value::Long(self.c.icon_steps()?.len() as i32)),
            "" | "item" => {
                let i = to_i64(need(args, 0, "Index")?)?;
                let n = self.c.icon_steps()?.len() as i64;
                if !(1..=n).contains(&i) {
                    return Err(ScriptError::runtime(9));
                }
                Ok(obj(Rc::new(IconCriterionObj {
                    c: self.c.clone(),
                    i: i as usize - 1,
                })))
            }
            _ => Err(ScriptError::no_member("IconCriteria", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        let n = self.c.icon_steps()?.len();
        Ok((0..n)
            .map(|i| {
                obj(Rc::new(IconCriterionObj {
                    c: self.c.clone(),
                    i,
                }))
            })
            .collect())
    }
}

/// One icon of an icon set and the value from which it shows (lowest
/// first; the first one's value isn't used).
pub struct IconCriterionObj {
    c: FormatConditionObj,
    i: usize,
}

impl IconCriterionObj {
    fn change(
        &self,
        change: impl FnOnce(&mut CfIconStep) -> Result<(), ScriptError>,
    ) -> Result<(), ScriptError> {
        let i = self.i;
        self.c.update_icons(|_, _, _, steps| {
            let s = steps.get_mut(i).ok_or_else(|| ScriptError::runtime(9))?;
            change(s)
        })
    }
}

impl Dispatch for IconCriterionObj {
    fn type_name(&self) -> String {
        "IconCriterion".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        let steps = self.c.icon_steps()?;
        let step = steps.get(self.i).ok_or_else(|| ScriptError::runtime(9))?;
        match name.to_ascii_lowercase().as_str() {
            "type" => Ok(Value::Long(value_type(step.value.kind, false) as i32)),
            "value" => Ok(point_value(&step.value)),
            "operator" => Ok(Value::Long(if step.gte {
                XL_GREATER_EQUAL as i32
            } else {
                XL_GREATER as i32
            })),
            "index" => Ok(Value::Long(self.i as i32 + 1)),
            "icon" => Err(ScriptError::unsupported("IconCriterion.Icon")),
            _ => Err(ScriptError::no_member("IconCriterion", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "type" => {
                let kind = value_kind(to_i64(&value)?)?;
                if !CfValueKind::ICON.contains(&kind) {
                    return Err(err1004(
                        "Unable to set the Type property of the IconCriterion class",
                    ));
                }
                self.change(|s| {
                    retype(&mut s.value, kind);
                    Ok(())
                })
            }
            "value" => self.change(|s| set_point_value(&mut s.value, &value)),
            "operator" => {
                let gte = match to_i64(&value)? {
                    XL_GREATER_EQUAL => true,
                    XL_GREATER => false,
                    _ => return Err(ScriptError::runtime(5)),
                };
                self.change(|s| {
                    s.gte = gte;
                    Ok(())
                })
            }
            "icon" => {
                // Icons by `XlIcon` aren't mapped; "No Cell Icon" is.
                if to_i64(&value)? == 0 {
                    self.change(|s| {
                        s.icon = CfIconRef::none();
                        Ok(())
                    })
                } else {
                    Err(ScriptError::unsupported("IconCriterion.Icon"))
                }
            }
            _ => Err(ScriptError::no_member("IconCriterion", name)),
        }
    }
}

/// `Workbook.IconSets`.
pub struct IconSetsObj;

impl Dispatch for IconSetsObj {
    fn type_name(&self) -> String {
        "IconSets".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "count" => Ok(Value::Long(ICON_SETS.len() as i32)),
            "" | "item" => {
                let id = to_i64(need(args, 0, "Index")?)?;
                if !(1..=ICON_SETS.len() as i64).contains(&id) {
                    return Err(ScriptError::runtime(9));
                }
                Ok(obj(Rc::new(IconSetObj { id })))
            }
            _ => Err(ScriptError::no_member("IconSets", name)),
        }
    }

    fn enumerate(&self) -> Result<Vec<Value>, ScriptError> {
        Ok((1..=ICON_SETS.len() as i64)
            .map(|id| obj(Rc::new(IconSetObj { id })))
            .collect())
    }
}

/// One of Excel's icon sets; its default member is its `XlIconSet` number.
pub struct IconSetObj {
    pub id: i64,
}

impl Dispatch for IconSetObj {
    fn type_name(&self) -> String {
        "IconSet".into()
    }

    fn get(&self, name: &str, _args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "" | "id" => Ok(Value::Long(self.id as i32)),
            "count" => {
                let n = ICON_SETS[(self.id - 1) as usize]
                    .chars()
                    .next()
                    .and_then(|c| c.to_digit(10))
                    .unwrap_or(3);
                Ok(Value::Long(n as i32))
            }
            "item" => Err(ScriptError::unsupported("IconSet.Item")),
            _ => Err(ScriptError::no_member("IconSet", name)),
        }
    }
}

/// The constants of this module by their VBA names (registered with the
/// interpreter).
pub const CONSTANTS: &[(&str, i64)] = &[
    ("xlCellValue", 1),
    ("xlExpression", 2),
    ("xlColorScale", 3),
    ("xlDatabar", 4),
    ("xlTop10", 5),
    ("xlIconSets", 6),
    ("xlUniqueValues", 8),
    ("xlTextString", 9),
    ("xlBlanksCondition", 10),
    ("xlTimePeriod", 11),
    ("xlAboveAverageCondition", 12),
    ("xlNoBlanksCondition", 13),
    ("xlErrorsCondition", 16),
    ("xlNoErrorsCondition", 17),
    ("xlBetween", 1),
    ("xlNotBetween", 2),
    ("xlEqual", 3),
    ("xlNotEqual", 4),
    ("xlGreater", 5),
    ("xlLess", 6),
    ("xlGreaterEqual", 7),
    ("xlLessEqual", 8),
    ("xlContains", 0),
    ("xlDoesNotContain", 1),
    ("xlBeginsWith", 2),
    ("xlEndsWith", 3),
    ("xlToday", 0),
    ("xlYesterday", 1),
    ("xlLast7Days", 2),
    ("xlThisWeek", 3),
    ("xlLastWeek", 4),
    ("xlLastMonth", 5),
    ("xlTomorrow", 6),
    ("xlNextWeek", 7),
    ("xlNextMonth", 8),
    ("xlThisMonth", 9),
    ("xlConditionValueNone", -1),
    ("xlConditionValueNumber", 0),
    ("xlConditionValueLowestValue", 1),
    ("xlConditionValueHighestValue", 2),
    ("xlConditionValuePercent", 3),
    ("xlConditionValueFormula", 4),
    ("xlConditionValuePercentile", 5),
    ("xlConditionValueAutomaticMin", 6),
    ("xlConditionValueAutomaticMax", 7),
    ("xlTop10Top", 1),
    ("xlTop10Bottom", 0),
    ("xlAboveAverage", 0),
    ("xlBelowAverage", 1),
    ("xlEqualAboveAverage", 2),
    ("xlEqualBelowAverage", 3),
    ("xlAboveStdDev", 4),
    ("xlBelowStdDev", 5),
    ("xlUnique", 0),
    ("xlDuplicate", 1),
    ("xlDataBarFillSolid", 0),
    ("xlDataBarFillGradient", 1),
    ("xlContext", -5002),
    ("xlLTR", -5003),
    ("xlRTL", -5004),
    ("xlDataBarAxisAutomatic", 0),
    ("xlDataBarAxisMidpoint", 1),
    ("xlDataBarAxisNone", 2),
    ("xlDataBarBorderNone", 0),
    ("xlDataBarBorderSolid", 1),
    ("xl3Arrows", 1),
    ("xl3ArrowsGray", 2),
    ("xl3Flags", 3),
    ("xl3TrafficLights1", 4),
    ("xl3TrafficLights2", 5),
    ("xl3Signs", 6),
    ("xl3Symbols", 7),
    ("xl3Symbols2", 8),
    ("xl4Arrows", 9),
    ("xl4ArrowsGray", 10),
    ("xl4RedToBlack", 11),
    ("xl4CRV", 12),
    ("xl4TrafficLights", 13),
    ("xl5Arrows", 14),
    ("xl5ArrowsGray", 15),
    ("xl5CRV", 16),
    ("xl5Quarters", 17),
    ("xl3Stars", 18),
    ("xl3Triangles", 19),
    ("xl5Boxes", 20),
    ("xlPatternNone", -4142),
    ("xlPatternSolid", 1),
    ("xlPatternAutomatic", -4105),
    ("xlGray50", -4125),
    ("xlGray75", -4126),
    ("xlGray25", -4124),
    ("xlChecker", 9),
    ("xlSemiGray75", 10),
    ("xlLightHorizontal", 11),
    ("xlLightVertical", 12),
    ("xlLightDown", 13),
    ("xlLightUp", 14),
    ("xlGrid", 15),
    ("xlCrissCross", 16),
    ("xlGray16", 17),
    ("xlGray8", 18),
    ("xlPatternLinearGradient", 4000),
    ("xlPatternRectangularGradient", 4001),
];
