//! `Range.Validation`: data validation from macros (`Add`, `Delete`,
//! `Modify` and the rule's properties), through the engine's Data
//! Validation edits (`Workbook::set_validation` / `clear_validation`, what
//! the Data Validation dialog's OK and Clear All call), so each change
//! undoes and saves as one made by hand.
//!
//! The object is the range's sheet and cells; the rule is read from the
//! top-left cell each time. A property change puts the changed rule on the
//! range's cells (they leave the rule they had, as Excel applies a rule to
//! the cells of the range it was changed through).

use std::rc::Rc;

use kantor_script::{Arg, Dispatch, ScriptError, Value};
use sheets_engine::{
    CellRange, ErrorStyle, IME_MODES, Message, Validation, ValidationOperator, ValidationType,
};

use crate::range::RangeObj;
use crate::session::Session;
use crate::values::*;

/// `XlDVType`.
pub const XL_VALIDATE_INPUT_ONLY: i64 = 0;
pub const XL_VALIDATE_WHOLE_NUMBER: i64 = 1;
pub const XL_VALIDATE_DECIMAL: i64 = 2;
pub const XL_VALIDATE_LIST: i64 = 3;
pub const XL_VALIDATE_DATE: i64 = 4;
pub const XL_VALIDATE_TIME: i64 = 5;
pub const XL_VALIDATE_TEXT_LENGTH: i64 = 6;
pub const XL_VALIDATE_CUSTOM: i64 = 7;
/// `XlFormatConditionOperator`.
pub const XL_BETWEEN: i64 = 1;
pub const XL_NOT_BETWEEN: i64 = 2;
pub const XL_EQUAL: i64 = 3;
pub const XL_NOT_EQUAL: i64 = 4;
pub const XL_GREATER: i64 = 5;
pub const XL_LESS: i64 = 6;
pub const XL_GREATER_EQUAL: i64 = 7;
pub const XL_LESS_EQUAL: i64 = 8;
/// `XlDVAlertStyle`.
pub const XL_VALID_ALERT_STOP: i64 = 1;
pub const XL_VALID_ALERT_WARNING: i64 = 2;
pub const XL_VALID_ALERT_INFORMATION: i64 = 3;
/// `XlIMEMode` (0..=10, in the order of `sheets_engine::IME_MODES`).
pub const XL_IME_MODE_NO_CONTROL: i64 = 0;
pub const XL_IME_MODE_ON: i64 = 1;
pub const XL_IME_MODE_OFF: i64 = 2;
pub const XL_IME_MODE_DISABLE: i64 = 3;
pub const XL_IME_MODE_HIRAGANA: i64 = 4;
pub const XL_IME_MODE_KATAKANA: i64 = 5;
pub const XL_IME_MODE_KATAKANA_HALF: i64 = 6;
pub const XL_IME_MODE_ALPHA_FULL: i64 = 7;
pub const XL_IME_MODE_ALPHA: i64 = 8;
pub const XL_IME_MODE_HANGUL_FULL: i64 = 9;
pub const XL_IME_MODE_HANGUL: i64 = 10;

/// The constants above by VBA name, for registering.
pub const CONSTANTS: [(&str, i64); 30] = [
    ("xlValidateInputOnly", XL_VALIDATE_INPUT_ONLY),
    ("xlValidateWholeNumber", XL_VALIDATE_WHOLE_NUMBER),
    ("xlValidateDecimal", XL_VALIDATE_DECIMAL),
    ("xlValidateList", XL_VALIDATE_LIST),
    ("xlValidateDate", XL_VALIDATE_DATE),
    ("xlValidateTime", XL_VALIDATE_TIME),
    ("xlValidateTextLength", XL_VALIDATE_TEXT_LENGTH),
    ("xlValidateCustom", XL_VALIDATE_CUSTOM),
    ("xlBetween", XL_BETWEEN),
    ("xlNotBetween", XL_NOT_BETWEEN),
    ("xlEqual", XL_EQUAL),
    ("xlNotEqual", XL_NOT_EQUAL),
    ("xlGreater", XL_GREATER),
    ("xlLess", XL_LESS),
    ("xlGreaterEqual", XL_GREATER_EQUAL),
    ("xlLessEqual", XL_LESS_EQUAL),
    ("xlValidAlertStop", XL_VALID_ALERT_STOP),
    ("xlValidAlertWarning", XL_VALID_ALERT_WARNING),
    ("xlValidAlertInformation", XL_VALID_ALERT_INFORMATION),
    ("xlIMEModeNoControl", XL_IME_MODE_NO_CONTROL),
    ("xlIMEModeOn", XL_IME_MODE_ON),
    ("xlIMEModeOff", XL_IME_MODE_OFF),
    ("xlIMEModeDisable", XL_IME_MODE_DISABLE),
    ("xlIMEModeHiragana", XL_IME_MODE_HIRAGANA),
    ("xlIMEModeKatakana", XL_IME_MODE_KATAKANA),
    ("xlIMEModeKatakanaHalf", XL_IME_MODE_KATAKANA_HALF),
    ("xlIMEModeAlphaFull", XL_IME_MODE_ALPHA_FULL),
    ("xlIMEModeAlpha", XL_IME_MODE_ALPHA),
    ("xlIMEModeHangulFull", XL_IME_MODE_HANGUL_FULL),
    ("xlIMEModeHangul", XL_IME_MODE_HANGUL),
];

/// Cells `Validation.Value` checks at most.
const MAX_CHECKED_CELLS: u64 = 1_000_000;

/// `Range.Validation`.
pub fn validation(r: &RangeObj) -> Value {
    obj(Rc::new(ValidationObj {
        s: r.s.clone(),
        sheet: r.sheet,
        areas: r.areas.clone(),
    }))
}

fn kind_of(n: i64) -> Result<ValidationType, ScriptError> {
    Ok(match n {
        XL_VALIDATE_INPUT_ONLY => ValidationType::Any,
        XL_VALIDATE_WHOLE_NUMBER => ValidationType::Whole,
        XL_VALIDATE_DECIMAL => ValidationType::Decimal,
        XL_VALIDATE_LIST => ValidationType::List,
        XL_VALIDATE_DATE => ValidationType::Date,
        XL_VALIDATE_TIME => ValidationType::Time,
        XL_VALIDATE_TEXT_LENGTH => ValidationType::TextLength,
        XL_VALIDATE_CUSTOM => ValidationType::Custom,
        _ => return Err(ScriptError::runtime(5)),
    })
}

fn kind_number(k: ValidationType) -> i64 {
    match k {
        ValidationType::Any => XL_VALIDATE_INPUT_ONLY,
        ValidationType::Whole => XL_VALIDATE_WHOLE_NUMBER,
        ValidationType::Decimal => XL_VALIDATE_DECIMAL,
        ValidationType::List => XL_VALIDATE_LIST,
        ValidationType::Date => XL_VALIDATE_DATE,
        ValidationType::Time => XL_VALIDATE_TIME,
        ValidationType::TextLength => XL_VALIDATE_TEXT_LENGTH,
        ValidationType::Custom => XL_VALIDATE_CUSTOM,
    }
}

fn operator_of(n: i64) -> Result<ValidationOperator, ScriptError> {
    Ok(match n {
        XL_BETWEEN => ValidationOperator::Between,
        XL_NOT_BETWEEN => ValidationOperator::NotBetween,
        XL_EQUAL => ValidationOperator::Equal,
        XL_NOT_EQUAL => ValidationOperator::NotEqual,
        XL_GREATER => ValidationOperator::GreaterThan,
        XL_LESS => ValidationOperator::LessThan,
        XL_GREATER_EQUAL => ValidationOperator::GreaterThanOrEqual,
        XL_LESS_EQUAL => ValidationOperator::LessThanOrEqual,
        _ => return Err(ScriptError::runtime(5)),
    })
}

fn operator_number(o: ValidationOperator) -> i64 {
    match o {
        ValidationOperator::Between => XL_BETWEEN,
        ValidationOperator::NotBetween => XL_NOT_BETWEEN,
        ValidationOperator::Equal => XL_EQUAL,
        ValidationOperator::NotEqual => XL_NOT_EQUAL,
        ValidationOperator::GreaterThan => XL_GREATER,
        ValidationOperator::LessThan => XL_LESS,
        ValidationOperator::GreaterThanOrEqual => XL_GREATER_EQUAL,
        ValidationOperator::LessThanOrEqual => XL_LESS_EQUAL,
    }
}

fn style_of(n: i64) -> Result<ErrorStyle, ScriptError> {
    Ok(match n {
        XL_VALID_ALERT_STOP => ErrorStyle::Stop,
        XL_VALID_ALERT_WARNING => ErrorStyle::Warning,
        XL_VALID_ALERT_INFORMATION => ErrorStyle::Information,
        _ => return Err(ScriptError::runtime(5)),
    })
}

fn style_number(s: ErrorStyle) -> i64 {
    match s {
        ErrorStyle::Stop => XL_VALID_ALERT_STOP,
        ErrorStyle::Warning => XL_VALID_ALERT_WARNING,
        ErrorStyle::Information => XL_VALID_ALERT_INFORMATION,
    }
}

/// Whether the operator takes two bounds.
fn two_bounds(o: ValidationOperator) -> bool {
    matches!(
        o,
        ValidationOperator::Between | ValidationOperator::NotBetween
    )
}

/// Whether the rule compares the value with bounds.
fn compares(k: ValidationType) -> bool {
    matches!(
        k,
        ValidationType::Whole
            | ValidationType::Decimal
            | ValidationType::Date
            | ValidationType::Time
            | ValidationType::TextLength
    )
}

/// A `Formula1`/`Formula2` argument as the rule stores it (no `=`): a
/// list's `"a,b,c"` (quoted) or reference, a bound's number (dates as
/// serial numbers) or formula.
fn operand(kind: ValidationType, v: &Value) -> Result<String, ScriptError> {
    let v = scalar(v)?;
    match (&v, kind) {
        (Value::String(s), ValidationType::List) => sheets_engine::list_source(s).map_err(engine),
        (Value::String(s), _) => {
            let t = s.trim();
            if let Some(f) = t.strip_prefix('=') {
                if f.trim().is_empty() {
                    return Err(err1004("Application-defined or object-defined error"));
                }
                return Ok(f.trim().to_owned());
            }
            if kind == ValidationType::Custom {
                return Ok(t.to_owned());
            }
            sheets_engine::validation_operand(kind, t).map_err(engine)
        }
        (Value::Boolean(b), _) => Ok(if *b { "TRUE" } else { "FALSE" }.to_owned()),
        (other, ValidationType::List) => {
            let n = other.as_f64().ok_or_else(|| ScriptError::runtime(13))?;
            sheets_engine::list_source(&number_text(n)).map_err(engine)
        }
        (other, _) => {
            let n = other.as_f64().ok_or_else(|| ScriptError::runtime(13))?;
            Ok(number_text(n))
        }
    }
}

/// A stored `formula1`/`formula2` as `Validation.Formula1` returns it:
/// a typed list's items (`a,b,c`), a number as is, a formula with `=`.
fn formula_text(kind: ValidationType, f: &str) -> String {
    let f = f.trim();
    if kind == ValidationType::List
        && let Some(inner) = f.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
    {
        return inner.replace("\"\"", "\"");
    }
    if f.parse::<f64>().is_ok() {
        f.to_owned()
    } else {
        format!("={f}")
    }
}

/// `Range.Validation`: the rule on the range's cells (read from its
/// top-left cell).
pub struct ValidationObj {
    s: Rc<Session>,
    sheet: u32,
    areas: Vec<CellRange>,
}

impl ValidationObj {
    /// The rule on the top-left cell now, if any.
    fn rule(&self) -> Result<Option<Validation>, ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let a = self.areas[0];
        let v = self.s.read()?.validation(idx, a.top, a.left);
        Ok(v.as_deref().cloned())
    }

    /// The rule, or error 1004 when the cells have none (as Excel's
    /// properties fail then).
    fn need_rule(&self) -> Result<Validation, ScriptError> {
        self.rule()?
            .ok_or_else(|| err1004("Application-defined or object-defined error"))
    }

    /// Whether any cell of the range has a rule.
    fn any_rule(&self) -> Result<bool, ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let rules = self.s.read()?.sheet_validations(idx);
        Ok(rules.iter().any(|v| {
            v.ranges
                .iter()
                .any(|r| self.areas.iter().any(|a| a.intersects(r)))
        }))
    }

    /// Puts `rule` on the range's cells (one engine edit).
    fn put_rule(&self, rule: Validation) -> Result<(), ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        self.s
            .write()?
            .set_validation(idx, &self.areas, rule)
            .map_err(engine)
    }

    /// The rule changed by `f`.
    fn change(&self, f: impl FnOnce(&mut Validation)) -> Result<(), ScriptError> {
        let mut rule = self.need_rule()?;
        f(&mut rule);
        self.put_rule(rule)
    }

    /// `Add` / `Modify`: a rule from `Type`, `AlertStyle`, `Operator`,
    /// `Formula1`, `Formula2`, starting from `base`.
    fn build(&self, args: &[Arg], base: Option<Validation>) -> Result<Validation, ScriptError> {
        let failed = |what: &str| err1004(format!("{what} method of Validation class failed"));
        let what = if base.is_some() { "Modify" } else { "Add" };
        let kind = match arg_i64(args, 0, "Type")? {
            Some(n) => kind_of(n)?,
            None => match &base {
                Some(b) => b.kind,
                None => return Err(ScriptError::runtime(449)),
            },
        };
        let style = match arg_i64(args, 1, "AlertStyle")? {
            Some(n) => style_of(n)?,
            None => base
                .as_ref()
                .and_then(|b| b.error.as_ref().map(|e| e.0))
                .unwrap_or(ErrorStyle::Stop),
        };
        let operator = match arg_i64(args, 2, "Operator")? {
            Some(n) => operator_of(n)?,
            None => base
                .as_ref()
                .map_or(ValidationOperator::Between, |b| b.operator),
        };
        // A changed type drops the old bounds.
        let keep = base.as_ref().filter(|b| b.kind == kind);
        // An input-only rule has no formula (Excel ignores `Formula1`).
        let formula1 = match arg(args, 3, "Formula1") {
            Some(_) if kind == ValidationType::Any => None,
            Some(v) => Some(operand(kind, v)?),
            None => keep.and_then(|b| b.formula1.clone()),
        };
        let formula2 = match arg(args, 4, "Formula2") {
            Some(v) if compares(kind) && two_bounds(operator) => Some(operand(kind, v)?),
            Some(_) => None,
            None => keep.and_then(|b| b.formula2.clone()),
        };
        if kind != ValidationType::Any && formula1.is_none() {
            return Err(failed(what));
        }
        if compares(kind) && two_bounds(operator) && formula2.is_none() {
            return Err(failed(what));
        }
        let formula2 = formula2.filter(|_| compares(kind) && two_bounds(operator));
        let formula1 = formula1.filter(|_| kind != ValidationType::Any);
        Ok(match base {
            Some(b) => Validation {
                kind,
                operator,
                formula1,
                formula2,
                error: b.error.map(|(_, m)| (style, m)),
                ..b
            },
            // Excel's defaults for a new rule: ignore blanks, show the
            // drop-down, the input message and the error alert.
            None => Validation {
                kind,
                operator,
                allow_blank: true,
                dropdown: true,
                prompt: None,
                error: Some((style, Message::default())),
                formula1,
                formula2,
                ranges: Vec::new(),
                ime_mode: None,
            },
        })
    }

    /// `Validation.Value`: whether every cell of the range (its used part)
    /// passes its rule.
    fn passes(&self) -> Result<bool, ScriptError> {
        let idx = self.s.sheet_index(self.sheet)?;
        let mut book = self.s.read()?;
        let (rows, cols) = book.used_size(idx);
        let mut checked = 0u64;
        for a in &self.areas {
            let Some(a) = a.clip(rows.max(1), cols.max(1)) else {
                continue;
            };
            for row in a.top..=a.bottom {
                for col in a.left..=a.right {
                    checked += 1;
                    if checked > MAX_CHECKED_CELLS {
                        return Ok(true);
                    }
                    if book.validate_cell(idx, row, col).is_some() {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }
}

impl Dispatch for ValidationObj {
    fn type_name(&self) -> String {
        "Validation".into()
    }

    fn get(&self, name: &str, args: &[Arg]) -> Result<Value, ScriptError> {
        match name.to_ascii_lowercase().as_str() {
            "add" => {
                if self.any_rule()? {
                    return Err(err1004(
                        "Application-defined or object-defined error (the cells already have data validation: Delete it first)",
                    ));
                }
                let rule = self.build(args, None)?;
                self.put_rule(rule)?;
                Ok(Value::Empty)
            }
            "modify" => {
                let base = self.need_rule()?;
                let rule = self.build(args, Some(base))?;
                self.put_rule(rule)?;
                Ok(Value::Empty)
            }
            "delete" => {
                let idx = self.s.sheet_index(self.sheet)?;
                self.s
                    .write()?
                    .clear_validation(idx, &self.areas)
                    .map_err(engine)?;
                Ok(Value::Empty)
            }
            "type" => Ok(Value::Long(kind_number(self.need_rule()?.kind) as i32)),
            "operator" => {
                let r = self.need_rule()?;
                Ok(Value::Long(if compares(r.kind) {
                    operator_number(r.operator) as i32
                } else {
                    0
                }))
            }
            "alertstyle" => Ok(Value::Long(
                self.need_rule()?
                    .error
                    .map_or(XL_VALID_ALERT_STOP, |(s, _)| style_number(s)) as i32,
            )),
            "formula1" => {
                let r = self.need_rule()?;
                Ok(Value::String(
                    r.formula1
                        .map(|f| formula_text(r.kind, &f))
                        .unwrap_or_default(),
                ))
            }
            "formula2" => {
                let r = self.need_rule()?;
                Ok(Value::String(
                    r.formula2
                        .map(|f| formula_text(r.kind, &f))
                        .unwrap_or_default(),
                ))
            }
            "ignoreblank" => Ok(Value::Boolean(self.need_rule()?.allow_blank)),
            "incelldropdown" => Ok(Value::Boolean(self.need_rule()?.dropdown)),
            "showinput" => Ok(Value::Boolean(self.need_rule()?.prompt.is_some())),
            "showerror" => Ok(Value::Boolean(self.need_rule()?.error.is_some())),
            "inputtitle" => Ok(Value::String(
                self.need_rule()?
                    .prompt
                    .map(|m| m.title)
                    .unwrap_or_default(),
            )),
            "inputmessage" => Ok(Value::String(
                self.need_rule()?.prompt.map(|m| m.text).unwrap_or_default(),
            )),
            "errortitle" => Ok(Value::String(
                self.need_rule()?
                    .error
                    .map(|(_, m)| m.title)
                    .unwrap_or_default(),
            )),
            "errormessage" => Ok(Value::String(
                self.need_rule()?
                    .error
                    .map(|(_, m)| m.text)
                    .unwrap_or_default(),
            )),
            "imemode" => {
                let r = self.need_rule()?;
                let n = r
                    .ime_mode
                    .as_deref()
                    .and_then(|m| IME_MODES.iter().position(|(k, _)| *k == m))
                    .unwrap_or(0);
                Ok(Value::Long(n as i32))
            }
            "value" => Ok(Value::Boolean(self.passes()?)),
            "parent" => Ok(obj(RangeObj::multi(
                &self.s,
                self.sheet,
                self.areas.clone(),
            ))),
            _ => Err(ScriptError::no_member("Validation", name)),
        }
    }

    fn put(&self, name: &str, _args: &[Arg], value: Value) -> Result<(), ScriptError> {
        let lower = name.to_ascii_lowercase();
        match lower.as_str() {
            "type" => {
                let kind = kind_of(to_i64(&value)?)?;
                self.change(|r| r.kind = kind)
            }
            "operator" => {
                let op = operator_of(to_i64(&value)?)?;
                self.change(|r| r.operator = op)
            }
            "alertstyle" => {
                let style = style_of(to_i64(&value)?)?;
                self.change(|r| {
                    if let Some(e) = &mut r.error {
                        e.0 = style;
                    }
                })
            }
            "ignoreblank" => {
                let b = to_bool(&value)?;
                self.change(|r| r.allow_blank = b)
            }
            "incelldropdown" => {
                let b = to_bool(&value)?;
                self.change(|r| r.dropdown = b)
            }
            "showerror" => {
                let b = to_bool(&value)?;
                self.change(|r| match (b, &r.error) {
                    (false, _) => r.error = None,
                    (true, None) => r.error = Some((ErrorStyle::Stop, Message::default())),
                    (true, Some(_)) => {}
                })
            }
            // The engine keeps an input message only while it's shown and
            // not empty: hiding it forgets its text.
            "showinput" => {
                let b = to_bool(&value)?;
                self.change(|r| {
                    if !b {
                        r.prompt = None;
                    }
                })
            }
            "inputtitle" | "inputmessage" => {
                let text = to_str(&value)?;
                self.change(|r| {
                    let mut m = r.prompt.take().unwrap_or_default();
                    if lower == "inputtitle" {
                        m.title = text;
                    } else {
                        m.text = text;
                    }
                    r.prompt = (!(m.title.is_empty() && m.text.is_empty())).then_some(m);
                })
            }
            "errortitle" | "errormessage" => {
                let text = to_str(&value)?;
                self.change(|r| {
                    let (style, mut m) = r
                        .error
                        .take()
                        .unwrap_or((ErrorStyle::Stop, Message::default()));
                    if lower == "errortitle" {
                        m.title = text;
                    } else {
                        m.text = text;
                    }
                    r.error = Some((style, m));
                })
            }
            "formula1" | "formula2" => {
                let rule = self.need_rule()?;
                let f = operand(rule.kind, &value)?;
                self.change(|r| {
                    if lower == "formula1" {
                        r.formula1 = Some(f);
                    } else {
                        r.formula2 = Some(f);
                    }
                })
            }
            "imemode" => {
                let n = to_i64(&value)?;
                let mode = usize::try_from(n)
                    .ok()
                    .and_then(|i| IME_MODES.get(i))
                    .ok_or_else(|| ScriptError::runtime(5))?
                    .0;
                self.change(|r| r.ime_mode = (n > 0).then(|| mode.to_owned()))
            }
            _ => Err(ScriptError::no_member("Validation", name)),
        }
    }
}
