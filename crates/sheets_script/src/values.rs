//! Values between VBA and cells, and argument helpers.

use std::rc::Rc;

use kantor_script::vba::convert;
use kantor_script::{Arg, Array, Dispatch, ObjectRef, ScriptError, Value, find_arg};
use sheets_engine::script::KantorValue;

/// Excel's `CVErr` codes by error text.
pub fn error_code(text: &str) -> i32 {
    match text {
        "#NULL!" => 2000,
        "#DIV/0!" => 2007,
        "#VALUE!" => 2015,
        "#REF!" => 2023,
        "#NAME?" => 2029,
        "#NUM!" => 2036,
        "#N/A" => 2042,
        "#GETTING_DATA" => 2043,
        "#SPILL!" => 2045,
        "#CALC!" => 2050,
        _ => 2015,
    }
}

/// The error text of a `CVErr` code.
pub fn error_text(code: i32) -> &'static str {
    match code {
        2000 => "#NULL!",
        2007 => "#DIV/0!",
        2023 => "#REF!",
        2029 => "#NAME?",
        2036 => "#NUM!",
        2042 => "#N/A",
        2043 => "#GETTING_DATA",
        2045 => "#SPILL!",
        2050 => "#CALC!",
        _ => "#VALUE!",
    }
}

/// A cell value as VBA sees it; `date` reads a number as a Date (the cell
/// has a date format and this is `.Value`, not `.Value2`).
pub fn from_cell(v: KantorValue, date: bool) -> Value {
    match v {
        KantorValue::Empty => Value::Empty,
        KantorValue::Number(n) if date && (0.0..2_958_466.0).contains(&n) => Value::Date(n),
        KantorValue::Number(n) => Value::Double(n),
        KantorValue::Text(s) => Value::String(s),
        KantorValue::Bool(b) => Value::Boolean(b),
        KantorValue::Error(e) => Value::Error(error_code(&e)),
        KantorValue::Array(rows) => Value::array(Array::from_rows(
            1,
            rows.into_iter()
                .map(|r| r.into_iter().map(|v| from_cell(v, false)).collect())
                .collect(),
        )),
    }
}

/// A VBA value as a UDF hands it back to a formula.
pub fn to_kantor(v: &Value) -> KantorValue {
    match v {
        Value::Empty | Value::Missing | Value::Null => KantorValue::Empty,
        Value::Boolean(b) => KantorValue::Bool(*b),
        Value::String(s) => KantorValue::Text(s.clone()),
        Value::Error(n) => KantorValue::Error(error_text(*n).to_owned()),
        Value::Array(a) => KantorValue::Array(
            a.rows()
                .iter()
                .map(|r| r.iter().map(to_kantor).collect())
                .collect(),
        ),
        Value::Object(Some(o)) => match o.get("", &[]) {
            Ok(v) if !v.is_object() => to_kantor(&v),
            _ => KantorValue::Error("#VALUE!".to_owned()),
        },
        Value::Object(None) | Value::Record(_) => KantorValue::Error("#VALUE!".to_owned()),
        other => match other.as_f64() {
            Some(n) => KantorValue::Number(n),
            None => KantorValue::Error("#VALUE!".to_owned()),
        },
    }
}

/// A number as Excel takes it in a cell or formula.
pub fn number_text(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// What to type into a cell for `v`: `None` clears it. Dates also return
/// the number format to give a cell that has none.
pub fn cell_input(v: &Value) -> Result<(Option<String>, Option<&'static str>), ScriptError> {
    Ok(match v {
        Value::Empty | Value::Null | Value::Missing => (None, None),
        Value::String(s) if s.is_empty() => (None, None),
        Value::String(s) => (Some(s.clone()), None),
        Value::Boolean(b) => (Some(if *b { "TRUE" } else { "FALSE" }.to_owned()), None),
        Value::Error(n) => (Some(error_text(*n).to_owned()), None),
        Value::Date(d) => {
            let fmt = if *d < 1.0 && *d >= 0.0 {
                "h:mm:ss AM/PM"
            } else if d.fract() != 0.0 {
                "m/d/yyyy h:mm"
            } else {
                "m/d/yyyy"
            };
            (Some(number_text(*d)), Some(fmt))
        }
        Value::Object(_) | Value::Record(_) | Value::Array(_) => {
            return Err(ScriptError::runtime(13));
        }
        other => (
            Some(number_text(
                other.as_f64().ok_or_else(|| ScriptError::runtime(13))?,
            )),
            None,
        ),
    })
}

/// An object replaced by its default member (`Range("A1")` used as a
/// number), repeatedly.
pub fn scalar(v: &Value) -> Result<Value, ScriptError> {
    let mut v = v.clone();
    for _ in 0..8 {
        match v {
            Value::Object(Some(o)) => v = o.get("", &[])?,
            Value::Object(None) => return Err(ScriptError::runtime(91)),
            other => return Ok(other),
        }
    }
    Err(ScriptError::runtime(13))
}

pub fn to_f64(v: &Value) -> Result<f64, ScriptError> {
    convert::to_f64(&scalar(v)?)
}

pub fn to_i64(v: &Value) -> Result<i64, ScriptError> {
    convert::to_i64(&scalar(v)?)
}

pub fn to_str(v: &Value) -> Result<String, ScriptError> {
    convert::to_str(&scalar(v)?)
}

pub fn to_bool(v: &Value) -> Result<bool, ScriptError> {
    convert::to_bool(&scalar(v)?)
}

/// Optional argument helpers.
pub fn arg<'a>(args: &'a [Arg], i: usize, name: &str) -> Option<&'a Value> {
    find_arg(args, i, name)
}

pub fn arg_i64(args: &[Arg], i: usize, name: &str) -> Result<Option<i64>, ScriptError> {
    arg(args, i, name).map(to_i64).transpose()
}

pub fn arg_str(args: &[Arg], i: usize, name: &str) -> Result<Option<String>, ScriptError> {
    arg(args, i, name).map(to_str).transpose()
}

pub fn arg_bool(args: &[Arg], i: usize, name: &str) -> Result<Option<bool>, ScriptError> {
    arg(args, i, name).map(to_bool).transpose()
}

/// The required argument, or error 449 (Argument not optional).
pub fn need<'a>(args: &'a [Arg], i: usize, name: &str) -> Result<&'a Value, ScriptError> {
    arg(args, i, name).ok_or_else(|| ScriptError::runtime(449))
}

pub fn obj(o: Rc<impl Dispatch + 'static>) -> Value {
    Value::object(o as ObjectRef)
}

/// Error 1004 with a message, as Excel's object model raises.
pub fn err1004(msg: impl Into<String>) -> ScriptError {
    ScriptError::new(1004, msg).with_source("Kantor Sheets")
}

/// Error 1004 for an engine failure.
pub fn engine(e: impl std::fmt::Display) -> ScriptError {
    err1004(format!("Application-defined or object-defined error ({e})"))
}

/// VBA colours are `&HBBGGRR`; cells keep `0xRRGGBB`.
pub fn bgr_to_rgb(c: i64) -> u32 {
    let c = (c & 0xFF_FFFF) as u32;
    ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)
}

pub fn rgb_to_bgr(c: u32) -> i32 {
    (((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF)) as i32
}

/// Excel's default 56-colour palette (`ColorIndex` 1..=56), as `0xRRGGBB`.
pub const PALETTE: [u32; 56] = [
    0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x800000,
    0x008000, 0x000080, 0x808000, 0x800080, 0x008080, 0xC0C0C0, 0x808080, 0x9999FF, 0x993366,
    0xFFFFCC, 0xCCFFFF, 0x660066, 0xFF8080, 0x0066CC, 0xCCCCFF, 0x000080, 0xFF00FF, 0xFFFF00,
    0x00FFFF, 0x800080, 0x800000, 0x008080, 0x0000FF, 0x00CCFF, 0xCCFFFF, 0xCCFFCC, 0xFFFF99,
    0x99CCFF, 0xFF99CC, 0xCC99FF, 0xFFCC99, 0x3366FF, 0x33CCCC, 0x99CC00, 0xFFCC00, 0xFF9900,
    0xFF6600, 0x666699, 0x969696, 0x003366, 0x339966, 0x003300, 0x333300, 0x993300, 0x993366,
    0x333399, 0x333333,
];

/// The nearest palette entry (1-based) to `rgb`.
pub fn color_index(rgb: u32) -> i32 {
    let d = |a: u32, b: u32| {
        let ch = |x: u32, s: u32| ((x >> s) & 0xFF) as i32;
        (0..3)
            .map(|i| (ch(a, i * 8) - ch(b, i * 8)).pow(2))
            .sum::<i32>()
    };
    PALETTE
        .iter()
        .enumerate()
        .min_by_key(|(_, p)| d(**p, rgb))
        .map_or(1, |(i, _)| i as i32 + 1)
}

/// A formula literal for an argument of a worksheet function.
pub fn formula_literal(v: &Value) -> Result<String, ScriptError> {
    Ok(match v {
        Value::Empty | Value::Missing => String::new(),
        Value::String(s) => format!("\"{}\"", s.replace('"', "\"\"")),
        Value::Boolean(b) => if *b { "TRUE" } else { "FALSE" }.to_owned(),
        Value::Error(n) => error_text(*n).to_owned(),
        Value::Null => return Err(ScriptError::runtime(94)),
        Value::Array(a) => {
            let rows = a.rows();
            let mut out = String::from("{");
            for (i, r) in rows.iter().enumerate() {
                if i > 0 {
                    out.push(';');
                }
                for (j, x) in r.iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    let lit = formula_literal(x)?;
                    out.push_str(if lit.is_empty() { "\"\"" } else { &lit });
                }
            }
            out.push('}');
            out
        }
        Value::Object(_) | Value::Record(_) => return Err(ScriptError::runtime(13)),
        other => number_text(other.as_f64().ok_or_else(|| ScriptError::runtime(13))?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours() {
        // RGB(255, 0, 0) is 255 in VBA.
        assert_eq!(bgr_to_rgb(255), 0xFF0000);
        assert_eq!(rgb_to_bgr(0xFF0000), 255);
        assert_eq!(color_index(0xFF0000), 3);
        assert_eq!(color_index(0xFFFF00), 6);
    }

    #[test]
    fn inputs() {
        assert_eq!(
            cell_input(&Value::Double(1.5)).unwrap().0.as_deref(),
            Some("1.5")
        );
        assert_eq!(cell_input(&Value::Long(3)).unwrap().0.as_deref(), Some("3"));
        assert_eq!(cell_input(&Value::Empty).unwrap().0, None);
        assert_eq!(
            cell_input(&Value::Date(45000.0)).unwrap().1,
            Some("m/d/yyyy")
        );
        assert_eq!(
            formula_literal(&Value::string("a\"b")).unwrap(),
            "\"a\"\"b\""
        );
    }
}
