//! VBA's coercions and operators on values (MS-VBAL §5.5 "Let-coercion" and §5.6.9
//! "Operator expressions").
//!
//! Operands here are already plain values: callers replace objects by their default member
//! first. Locale-dependent conversions follow en-US (`.` decimal point, `,` thousands
//! separator, month/day/year dates), the locale VBA source itself is written in.

use std::cmp::Ordering;

use crate::date;
use crate::object::{ScriptError, Value};

type R<T> = Result<T, ScriptError>;

fn mismatch() -> ScriptError {
    ScriptError::runtime(13)
}

fn overflow() -> ScriptError {
    ScriptError::runtime(6)
}

fn invalid_null() -> ScriptError {
    ScriptError::runtime(94)
}

/// Case folding for `Option Compare Text`, one char to one char so positions stay put.
pub fn fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(l), None) => l,
        _ => c,
    }
}

/// Compares strings; `text` is `Option Compare Text` (case-insensitive).
pub fn cmp_str(a: &str, b: &str, text: bool) -> Ordering {
    if text {
        a.chars().map(fold).cmp(b.chars().map(fold))
    } else {
        a.cmp(b)
    }
}

/// Finds `needle` in `hay` at or after char index `from`; returns the char index.
pub fn find_str(hay: &str, needle: &str, from: usize, text: bool) -> Option<usize> {
    let h: Vec<char> = hay
        .chars()
        .map(|c| if text { fold(c) } else { c })
        .collect();
    let n: Vec<char> = needle
        .chars()
        .map(|c| if text { fold(c) } else { c })
        .collect();
    if n.is_empty() {
        return (from <= h.len()).then_some(from);
    }
    if n.len() > h.len() {
        return None;
    }
    (from..=h.len() - n.len()).find(|&i| h[i..i + n.len()] == n[..])
}

/// Finds the last `needle` in `hay` that ends at or before char index `end`.
pub fn rfind_str(hay: &str, needle: &str, end: usize, text: bool) -> Option<usize> {
    let h: Vec<char> = hay
        .chars()
        .map(|c| if text { fold(c) } else { c })
        .collect();
    let n: Vec<char> = needle
        .chars()
        .map(|c| if text { fold(c) } else { c })
        .collect();
    let end = end.min(h.len());
    if n.len() > end {
        return None;
    }
    (0..=end - n.len())
        .rev()
        .find(|&i| h[i..i + n.len()] == n[..])
}

/// Parses a number as `CDbl` does: blanks around, a sign, `,` thousands separators, `.`,
/// an exponent, or `&H`/`&O`.
pub fn parse_number(s: &str) -> Option<f64> {
    let t = s.trim_matches(|c| c == ' ' || c == '\t' || c == '\u{a0}');
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    if let Some(h) = lower.strip_prefix("&h") {
        return i64::from_str_radix(h.trim_end_matches('&'), 16)
            .ok()
            .map(|v| v as f64);
    }
    if let Some(o) = lower.strip_prefix("&o") {
        return i64::from_str_radix(o.trim_end_matches('&'), 8)
            .ok()
            .map(|v| v as f64);
    }
    let (neg, body) = match t.as_bytes()[0] {
        b'-' => (true, t[1..].trim_start()),
        b'+' => (false, t[1..].trim_start()),
        _ => (false, t),
    };
    let mut num = String::new();
    let mut seen_digit = false;
    let mut seen_point = false;
    let mut rest = body;
    for (i, c) in body.char_indices() {
        match c {
            '0'..='9' => {
                num.push(c);
                seen_digit = true;
            }
            ',' if !seen_point && seen_digit => {}
            '.' if !seen_point => {
                seen_point = true;
                num.push('.');
            }
            _ => {
                rest = &body[i..];
                break;
            }
        }
        rest = &body[i + c.len_utf8()..];
    }
    if !seen_digit {
        return None;
    }
    if let Some(exp) = rest.strip_prefix(['e', 'E', 'd', 'D']) {
        let digits = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        num.push('e');
        num.push_str(exp);
    } else if !rest.is_empty() {
        return None;
    }
    let v: f64 = num.parse().ok()?;
    Some(if neg { -v } else { v })
}

/// `Val`: the leading number in a string, ignoring blanks; 0 when there is none.
pub fn val(s: &str) -> f64 {
    let chars: Vec<char> = s
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\n' | '\r'))
        .collect();
    let lower: String = chars.iter().collect::<String>().to_ascii_lowercase();
    if let Some(h) = lower.strip_prefix("&h") {
        let digits: String = h.chars().take_while(char::is_ascii_hexdigit).collect();
        return i64::from_str_radix(&digits, 16).map_or(0.0, |v| {
            if v <= 0xFFFF {
                f64::from(v as u16 as i16)
            } else if v <= 0xFFFF_FFFF {
                f64::from(v as u32 as i32)
            } else {
                v as f64
            }
        });
    }
    if let Some(o) = lower.strip_prefix("&o") {
        let digits: String = o.chars().take_while(|c| ('0'..='7').contains(c)).collect();
        return i64::from_str_radix(&digits, 8).map_or(0.0, |v| v as f64);
    }
    let mut out = String::new();
    let mut i = 0;
    if matches!(chars.first(), Some('-' | '+')) {
        out.push(chars[0]);
        i = 1;
    }
    let mut point = false;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            out.push(c);
        } else if c == '.' && !point {
            point = true;
            out.push(c);
        } else {
            break;
        }
        i += 1;
    }
    if i < chars.len() && matches!(chars[i], 'e' | 'E' | 'd' | 'D') {
        let mut exp = String::from("e");
        let mut j = i + 1;
        if j < chars.len() && matches!(chars[j], '+' | '-') {
            exp.push(chars[j]);
            j += 1;
        }
        let start = exp.len();
        while j < chars.len() && chars[j].is_ascii_digit() {
            exp.push(chars[j]);
            j += 1;
        }
        if exp.len() > start {
            out.push_str(&exp);
        }
    }
    out.parse().unwrap_or(0.0)
}

/// Formats a floating-point number the way `CStr` does: `digits` significant digits (15 for
/// Double, 7 for Single), scientific notation outside 1E-5 .. 1E+digits.
pub fn format_float(x: f64, digits: usize) -> String {
    if x == 0.0 {
        return "0".to_string();
    }
    if !x.is_finite() {
        return if x.is_nan() {
            "NaN".to_string()
        } else if x > 0.0 {
            "Infinity".to_string()
        } else {
            "-Infinity".to_string()
        };
    }
    let (neg, ds, exp) = decimal_digits(x, digits);
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    if exp >= digits as i32 || exp < -5 {
        out.push(ds[0] as char);
        if ds.len() > 1 {
            out.push('.');
            out.extend(ds[1..].iter().map(|d| *d as char));
        }
        out.push('E');
        out.push(if exp < 0 { '-' } else { '+' });
        out.push_str(&format!("{:02}", exp.abs()));
    } else if exp >= 0 {
        let int_len = exp as usize + 1;
        for i in 0..int_len {
            out.push(ds.get(i).map_or('0', |d| *d as char));
        }
        if ds.len() > int_len {
            out.push('.');
            out.extend(ds[int_len..].iter().map(|d| *d as char));
        }
    } else {
        out.push_str("0.");
        for _ in 0..(-exp - 1) {
            out.push('0');
        }
        out.extend(ds.iter().map(|d| *d as char));
    }
    out
}

/// The significant decimal digits of `x` rounded to `digits` (trailing zeros removed) and the
/// decimal exponent of the first digit: 123.45 is `("12345", 2)`.
pub fn decimal_digits(x: f64, digits: usize) -> (bool, Vec<u8>, i32) {
    let s = format!("{:.*e}", digits.saturating_sub(1), x.abs());
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let mut ds: Vec<u8> = mant.bytes().filter(u8::is_ascii_digit).collect();
    while ds.len() > 1 && ds.last() == Some(&b'0') {
        ds.pop();
    }
    (x < 0.0, ds, exp)
}

fn currency_to_string(c: i64) -> String {
    let neg = c < 0;
    let a = c.unsigned_abs();
    let int = a / 10_000;
    let frac = a % 10_000;
    let mut s = if neg {
        format!("-{int}")
    } else {
        int.to_string()
    };
    if frac != 0 {
        let f = format!("{frac:04}");
        s.push('.');
        s.push_str(f.trim_end_matches('0'));
    }
    s
}

/// A date as `CStr` shows it (en-US short date and long time: `1/15/2024 6:30:00 PM`).
pub fn date_to_string(d: f64) -> String {
    let (day, secs) = date::split(d);
    let (y, m, dd) = date::ymd(d);
    let date_part = format!("{m}/{dd}/{y}");
    let h = secs / 3600;
    let time_part = format!(
        "{}:{:02}:{:02} {}",
        if h.is_multiple_of(12) { 12 } else { h % 12 },
        secs / 60 % 60,
        secs % 60,
        if h < 12 { "AM" } else { "PM" }
    );
    if secs == 0 {
        date_part
    } else if day == 0 {
        time_part
    } else {
        format!("{date_part} {time_part}")
    }
}

/// `CStr`.
pub fn to_str(v: &Value) -> R<String> {
    Ok(match v {
        Value::Empty => String::new(),
        Value::Null => return Err(invalid_null()),
        Value::Missing => return Err(ScriptError::runtime(449)),
        Value::Boolean(b) => if *b { "True" } else { "False" }.to_string(),
        Value::Byte(n) => n.to_string(),
        Value::Integer(n) => n.to_string(),
        Value::Long(n) => n.to_string(),
        Value::LongLong(n) => n.to_string(),
        Value::Single(n) => format_float(f64::from(*n), 7),
        Value::Double(n) => format_float(*n, 15),
        Value::Currency(c) => currency_to_string(*c),
        Value::Date(d) => date_to_string(*d),
        Value::String(s) => s.clone(),
        Value::Error(n) => format!("Error {n}"),
        Value::Array(_) | Value::Record(_) | Value::Object(_) => return Err(mismatch()),
    })
}

/// A value as text for display (`Debug.Print`, `MsgBox`): like `CStr`, with `Null` shown.
pub fn display(v: &Value) -> String {
    match v {
        Value::Null => "Null".to_string(),
        Value::Missing => "Error 448".to_string(),
        Value::Object(None) => "Nothing".to_string(),
        Value::Object(Some(o)) => o.type_name(),
        Value::Array(_) => "Array".to_string(),
        Value::Record(r) => r.type_name.clone(),
        other => to_str(other).unwrap_or_default(),
    }
}

/// `CDbl`.
pub fn to_f64(v: &Value) -> R<f64> {
    match v {
        Value::Empty => Ok(0.0),
        Value::Null => Err(invalid_null()),
        Value::String(s) => parse_number(s).ok_or_else(mismatch),
        other => other.as_f64().ok_or_else(mismatch),
    }
}

/// An integer by banker's rounding (`CLng`, MS-VBAL §5.5.1.2.1: "rounded to the nearest
/// integer, halfway values to the nearest even integer").
pub fn to_i64(v: &Value) -> R<i64> {
    match v {
        Value::Boolean(b) => Ok(if *b { -1 } else { 0 }),
        Value::Byte(n) => Ok(i64::from(*n)),
        Value::Integer(n) => Ok(i64::from(*n)),
        Value::Long(n) => Ok(i64::from(*n)),
        Value::LongLong(n) => Ok(*n),
        Value::Currency(c) => {
            let q = c.div_euclid(10_000);
            let r = c.rem_euclid(10_000);
            Ok(match r.cmp(&5_000) {
                Ordering::Less => q,
                Ordering::Greater => q + 1,
                Ordering::Equal => q + (q & 1),
            })
        }
        other => {
            let f = to_f64(other)?.round_ties_even();
            if !(-9.223_372_036_854_776e18..9.223_372_036_854_776e18).contains(&f) {
                return Err(overflow());
            }
            Ok(f as i64)
        }
    }
}

/// `CBool`.
pub fn to_bool(v: &Value) -> R<bool> {
    match v {
        Value::Empty => Ok(false),
        Value::Null => Err(invalid_null()),
        Value::Boolean(b) => Ok(*b),
        Value::String(s) => {
            let t = s.trim();
            if t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("#true#") {
                Ok(true)
            } else if t.eq_ignore_ascii_case("false") || t.eq_ignore_ascii_case("#false#") {
                Ok(false)
            } else {
                parse_number(t).map(|n| n != 0.0).ok_or_else(mismatch)
            }
        }
        other => other.as_f64().map(|n| n != 0.0).ok_or_else(mismatch),
    }
}

/// `CDate`.
pub fn to_date(v: &Value) -> R<f64> {
    let d = match v {
        Value::Date(d) => *d,
        Value::String(s) => match parse_date(s) {
            Some(d) => d,
            None => parse_number(s).ok_or_else(mismatch)?,
        },
        other => to_f64(other)?,
    };
    if !(date::MIN..=date::MAX).contains(&d) {
        return Err(overflow());
    }
    Ok(d)
}

fn fit_i64(n: i64, lo: i64, hi: i64) -> R<i64> {
    if (lo..=hi).contains(&n) {
        Ok(n)
    } else {
        Err(overflow())
    }
}

/// The scalar types values can be converted to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scalar {
    Boolean,
    Byte,
    Integer,
    Long,
    LongLong,
    Single,
    Double,
    Currency,
    Date,
    String,
}

/// Converts a plain value to a scalar type (`CInt`, assignment to an `Integer` variable…).
pub fn convert(v: &Value, to: Scalar) -> R<Value> {
    Ok(match to {
        Scalar::Boolean => Value::Boolean(to_bool(v)?),
        Scalar::Byte => Value::Byte(fit_i64(to_i64(v)?, 0, 255)? as u8),
        Scalar::Integer => Value::Integer(fit_i64(to_i64(v)?, -32_768, 32_767)? as i16),
        Scalar::Long => {
            Value::Long(fit_i64(to_i64(v)?, i64::from(i32::MIN), i64::from(i32::MAX))? as i32)
        }
        Scalar::LongLong => Value::LongLong(to_i64(v)?),
        Scalar::Single => {
            let f = to_f64(v)?;
            if f.abs() > f64::from(f32::MAX) {
                return Err(overflow());
            }
            Value::Single(f as f32)
        }
        Scalar::Double => Value::Double(to_f64(v)?),
        Scalar::Currency => Value::Currency(to_currency(v)?),
        Scalar::Date => Value::Date(to_date(v)?),
        Scalar::String => Value::String(to_str(v)?),
    })
}

fn to_currency(v: &Value) -> R<i64> {
    match v {
        Value::Currency(c) => Ok(*c),
        Value::Byte(_)
        | Value::Integer(_)
        | Value::Long(_)
        | Value::LongLong(_)
        | Value::Boolean(_) => to_i64(v)?.checked_mul(10_000).ok_or_else(overflow),
        other => {
            let f = (to_f64(other)? * 10_000.0).round_ties_even();
            if f.abs() >= 9.223_372_036_854_776e18 {
                return Err(overflow());
            }
            Ok(f as i64)
        }
    }
}

// ---- Arithmetic (MS-VBAL §5.6.9.3) ----

/// Integer kinds in widening order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum IntKind {
    Byte,
    Integer,
    Long,
    LongLong,
}

#[derive(Clone, Copy, Debug)]
enum Num {
    Int(i64, IntKind),
    Cur(i64),
    Single(f64),
    Double(f64),
    Date(f64),
}

impl Num {
    fn f(self) -> f64 {
        match self {
            Num::Int(n, _) => n as f64,
            Num::Cur(c) => c as f64 / 10_000.0,
            Num::Single(f) | Num::Double(f) | Num::Date(f) => f,
        }
    }

    fn cur(self) -> i128 {
        match self {
            Num::Int(n, _) => i128::from(n) * 10_000,
            Num::Cur(c) => i128::from(c),
            other => (other.f() * 10_000.0).round_ties_even() as i128,
        }
    }
}

fn num(v: &Value) -> R<Num> {
    Ok(match v {
        Value::Empty => Num::Int(0, IntKind::Integer),
        Value::Boolean(b) => Num::Int(if *b { -1 } else { 0 }, IntKind::Integer),
        Value::Byte(n) => Num::Int(i64::from(*n), IntKind::Byte),
        Value::Integer(n) => Num::Int(i64::from(*n), IntKind::Integer),
        Value::Long(n) => Num::Int(i64::from(*n), IntKind::Long),
        Value::LongLong(n) => Num::Int(*n, IntKind::LongLong),
        Value::Single(f) => Num::Single(f64::from(*f)),
        Value::Double(f) => Num::Double(*f),
        Value::Currency(c) => Num::Cur(*c),
        Value::Date(d) => Num::Date(*d),
        Value::String(s) => Num::Double(parse_number(s).ok_or_else(mismatch)?),
        Value::Null => return Err(invalid_null()),
        _ => return Err(mismatch()),
    })
}

/// An integer result in the operand kind, or the next wider kind when it doesn't fit. VBA
/// widens Variant results this way (§5.6.9.3: "if the result overflows the declared type of a
/// Variant operand, it is promoted"); a `Long` result that overflows becomes a Double.
fn int_result(n: i128, kind: IntKind) -> R<Value> {
    if kind <= IntKind::Byte && (0..=255).contains(&n) {
        return Ok(Value::Byte(n as u8));
    }
    if kind <= IntKind::Integer && (-32_768..=32_767).contains(&n) {
        return Ok(Value::Integer(n as i16));
    }
    if kind <= IntKind::Long {
        if let Ok(v) = i32::try_from(n) {
            return Ok(Value::Long(v));
        }
        return Ok(Value::Double(n as f64));
    }
    i64::try_from(n)
        .map(Value::LongLong)
        .map_err(|_| overflow())
}

fn double_result(f: f64) -> R<Value> {
    if f.is_finite() {
        Ok(Value::Double(f))
    } else {
        Err(overflow())
    }
}

fn single_result(f: f64) -> R<Value> {
    if f.is_finite() && f.abs() <= f64::from(f32::MAX) {
        Ok(Value::Single(f as f32))
    } else {
        Err(overflow())
    }
}

fn date_result(f: f64) -> R<Value> {
    if (date::MIN..=date::MAX).contains(&f) {
        Ok(Value::Date(f))
    } else {
        Err(overflow())
    }
}

fn cur_result(c: i128) -> R<Value> {
    i64::try_from(c)
        .map(Value::Currency)
        .map_err(|_| overflow())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
}

/// `+`, `-` and `*`.
pub fn arith(op: Arith, a: &Value, b: &Value) -> R<Value> {
    if a.is_null() || b.is_null() {
        return Ok(Value::Null);
    }
    if op == Arith::Add {
        // §5.6.9.3.3: String + String concatenates; Empty + String is the String.
        match (a, b) {
            (Value::String(x), Value::String(y)) => return Ok(Value::String(format!("{x}{y}"))),
            (Value::String(x), Value::Empty) | (Value::Empty, Value::String(x)) => {
                return Ok(Value::String(x.clone()));
            }
            _ => {}
        }
    }
    let (x, y) = (num(a)?, num(b)?);
    use Num::*;
    match (x, y) {
        (Date(_), _) | (_, Date(_)) => {
            let r = match op {
                Arith::Add => x.f() + y.f(),
                Arith::Sub => x.f() - y.f(),
                Arith::Mul => return double_result(x.f() * y.f()),
            };
            if op == Arith::Sub && matches!((x, y), (Date(_), Date(_))) {
                double_result(r)
            } else {
                date_result(r)
            }
        }
        (Double(_), _) | (_, Double(_)) => double_result(apply_f(op, x.f(), y.f())),
        (Single(_), Int(_, k)) | (Int(_, k), Single(_)) if k <= IntKind::Integer => {
            single_result(apply_f(op, x.f(), y.f()))
        }
        (Single(_), Single(_)) => single_result(apply_f(op, x.f(), y.f())),
        (Single(_), _) | (_, Single(_)) => double_result(apply_f(op, x.f(), y.f())),
        (Cur(_), _) | (_, Cur(_)) => {
            let (p, q) = (x.cur(), y.cur());
            match op {
                Arith::Add => cur_result(p + q),
                Arith::Sub => cur_result(p - q),
                Arith::Mul => {
                    let prod = p * q;
                    let (quot, rem) = (prod.div_euclid(10_000), prod.rem_euclid(10_000));
                    let rounded = match rem.cmp(&5_000) {
                        Ordering::Less => quot,
                        Ordering::Greater => quot + 1,
                        Ordering::Equal => quot + (quot & 1),
                    };
                    cur_result(rounded)
                }
            }
        }
        (Int(p, kp), Int(q, kq)) => {
            let (p, q) = (i128::from(p), i128::from(q));
            let r = match op {
                Arith::Add => p + q,
                Arith::Sub => p - q,
                Arith::Mul => p * q,
            };
            int_result(r, kp.max(kq))
        }
    }
}

fn apply_f(op: Arith, a: f64, b: f64) -> f64 {
    match op {
        Arith::Add => a + b,
        Arith::Sub => a - b,
        Arith::Mul => a * b,
    }
}

/// `/` (§5.6.9.3.5): always floating point.
pub fn divide(a: &Value, b: &Value) -> R<Value> {
    if a.is_null() || b.is_null() {
        return Ok(Value::Null);
    }
    let (x, y) = (num(a)?, num(b)?);
    let (p, q) = (x.f(), y.f());
    if q == 0.0 {
        return Err(ScriptError::runtime(if p == 0.0 { 6 } else { 11 }));
    }
    let small = |n: Num| {
        matches!(
            n,
            Num::Single(_) | Num::Int(_, IntKind::Byte | IntKind::Integer)
        )
    };
    if (matches!(x, Num::Single(_)) || matches!(y, Num::Single(_))) && small(x) && small(y) {
        single_result(p / q)
    } else {
        double_result(p / q)
    }
}

fn int_operand(n: Num) -> R<(i64, IntKind)> {
    match n {
        Num::Int(v, k) => Ok((v, k)),
        other => {
            let f = other.f().round_ties_even();
            if !(-2_147_483_648.0..=2_147_483_647.0).contains(&f) {
                return Err(overflow());
            }
            Ok((f as i64, IntKind::Long))
        }
    }
}

/// `\` (`modulo` false) and `Mod` (§5.6.9.3.6, §5.6.9.3.7): operands rounded to integers,
/// truncating division, the remainder takes the dividend's sign.
pub fn int_divide(a: &Value, b: &Value, modulo: bool) -> R<Value> {
    if a.is_null() || b.is_null() {
        return Ok(Value::Null);
    }
    let (p, kp) = int_operand(num(a)?)?;
    let (q, kq) = int_operand(num(b)?)?;
    if q == 0 {
        return Err(ScriptError::runtime(11));
    }
    let r = if modulo {
        i128::from(p) % i128::from(q)
    } else {
        i128::from(p) / i128::from(q)
    };
    int_result(r, kp.max(kq))
}

/// `^` (§5.6.9.3.1).
pub fn power(a: &Value, b: &Value) -> R<Value> {
    if a.is_null() || b.is_null() {
        return Ok(Value::Null);
    }
    let (p, q) = (num(a)?.f(), num(b)?.f());
    if p < 0.0 && q.fract() != 0.0 {
        return Err(ScriptError::runtime(5));
    }
    if p == 0.0 && q < 0.0 {
        return Err(ScriptError::runtime(5));
    }
    double_result(p.powf(q))
}

/// Unary `-`.
pub fn negate(a: &Value) -> R<Value> {
    if a.is_null() {
        return Ok(Value::Null);
    }
    match num(a)? {
        Num::Int(n, k) => int_result(-i128::from(n), k.max(IntKind::Integer)),
        Num::Cur(c) => cur_result(-i128::from(c)),
        Num::Single(f) => single_result(-f),
        Num::Double(f) => double_result(-f),
        Num::Date(f) => date_result(-f),
    }
}

/// `&` (§5.6.9.4): Null only when both sides are Null.
pub fn concat(a: &Value, b: &Value) -> R<Value> {
    if a.is_null() && b.is_null() {
        return Ok(Value::Null);
    }
    let s = |v: &Value| {
        if v.is_null() {
            Ok(String::new())
        } else {
            to_str(v)
        }
    };
    Ok(Value::String(s(a)? + &s(b)?))
}

// ---- Comparison (MS-VBAL §5.6.9.5) ----

fn is_numeric_like(v: &Value) -> bool {
    v.is_number() || matches!(v, Value::Boolean(_) | Value::Date(_))
}

/// Compares two values; `None` when either is Null.
///
/// - Numbers (and Booleans, Dates) compare numerically, Strings as text (binary or, under
///   `Option Compare Text`, case-insensitive).
/// - Empty is 0 against a number and `""` against a string.
/// - Number against String: VBA converts the string when the comparison mixes a typed number
///   with a string, and orders "numbers before strings" when both are Variants. Kantor
///   doesn't track static types, so it compares numerically when the string is numeric and
///   otherwise orders the number first (no Type mismatch), which matches what cell-value
///   comparisons in macros expect.
pub fn compare(a: &Value, b: &Value, text: bool) -> R<Option<Ordering>> {
    use Value::*;
    let num_cmp = |x: f64, y: f64| x.partial_cmp(&y).unwrap_or(Ordering::Equal);
    Ok(Some(match (a, b) {
        (Null, _) | (_, Null) => return Ok(None),
        (String(x), String(y)) => cmp_str(x, y, text),
        (String(x), Empty) => cmp_str(x, "", text),
        (Empty, String(y)) => cmp_str("", y, text),
        (Empty, Empty) => Ordering::Equal,
        (String(s), other) if is_numeric_like(other) => match parse_number(s) {
            Some(n) => num_cmp(n, other.as_f64().unwrap_or(0.0)),
            None => Ordering::Greater,
        },
        (other, String(s)) if is_numeric_like(other) => match parse_number(s) {
            Some(n) => num_cmp(other.as_f64().unwrap_or(0.0), n),
            None => Ordering::Less,
        },
        (Error(x), Error(y)) => x.cmp(y),
        (LongLong(x), LongLong(y)) => x.cmp(y),
        (Currency(x), Currency(y)) => x.cmp(y),
        (x, y) if (is_numeric_like(x) || x.is_empty()) && (is_numeric_like(y) || y.is_empty()) => {
            num_cmp(x.as_f64().unwrap_or(0.0), y.as_f64().unwrap_or(0.0))
        }
        _ => return Err(mismatch()),
    }))
}

// ---- Logical and bitwise operators (MS-VBAL §5.6.9.8) ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Logic {
    And,
    Or,
    Xor,
    Eqv,
    Imp,
}

enum Bits {
    Bool(bool),
    Int(i64, IntKind),
}

fn bits(v: &Value) -> R<Bits> {
    Ok(match v {
        Value::Boolean(b) => Bits::Bool(*b),
        Value::Empty => Bits::Int(0, IntKind::Integer),
        Value::Byte(n) => Bits::Int(i64::from(*n), IntKind::Byte),
        Value::Integer(n) => Bits::Int(i64::from(*n), IntKind::Integer),
        Value::Long(n) => Bits::Int(i64::from(*n), IntKind::Long),
        Value::LongLong(n) => Bits::Int(*n, IntKind::LongLong),
        Value::String(s) => {
            let t = s.trim();
            if t.eq_ignore_ascii_case("true") {
                Bits::Bool(true)
            } else if t.eq_ignore_ascii_case("false") {
                Bits::Bool(false)
            } else {
                Bits::Int(fit_long(to_i64(v)?)?, IntKind::Long)
            }
        }
        other => Bits::Int(fit_long(to_i64(other)?)?, IntKind::Long),
    })
}

fn fit_long(n: i64) -> R<i64> {
    fit_i64(n, i64::from(i32::MIN), i64::from(i32::MAX))
}

fn apply_logic(op: Logic, a: i64, b: i64) -> i64 {
    match op {
        Logic::And => a & b,
        Logic::Or => a | b,
        Logic::Xor => a ^ b,
        Logic::Eqv => !(a ^ b),
        Logic::Imp => !a | b,
    }
}

fn bits_value(n: i64, kind: IntKind) -> Value {
    match kind {
        IntKind::Byte => Value::Byte(n as u8),
        IntKind::Integer => Value::Integer(n as i16),
        IntKind::Long => Value::Long(n as i32),
        IntKind::LongLong => Value::LongLong(n),
    }
}

/// `And`, `Or`, `Xor`, `Eqv`, `Imp`: logical on Booleans, bitwise on numbers, with VBA's
/// Null rules (`Null And False` is False, `Null Or True` is True, otherwise Null).
pub fn logic(op: Logic, a: &Value, b: &Value) -> R<Value> {
    if a.is_null() || b.is_null() {
        let other = if a.is_null() { b } else { a };
        if other.is_null() {
            return Ok(Value::Null);
        }
        let truthy = |v: &Value| -> R<i64> {
            Ok(match bits(v)? {
                Bits::Bool(x) => {
                    if x {
                        -1
                    } else {
                        0
                    }
                }
                Bits::Int(n, _) => n,
            })
        };
        let n = truthy(other)?;
        return Ok(match op {
            Logic::And if n == 0 => other.clone(),
            Logic::Or if n == -1 => other.clone(),
            // Null Imp True = True; False Imp Null = True.
            Logic::Imp if b.is_null() && n == 0 => Value::Boolean(true),
            Logic::Imp if a.is_null() && n == -1 => other.clone(),
            _ => Value::Null,
        });
    }
    match (bits(a)?, bits(b)?) {
        (Bits::Bool(x), Bits::Bool(y)) => {
            let r = apply_logic(op, i64::from(x), i64::from(y)) & 1;
            Ok(Value::Boolean(r != 0))
        }
        (x, y) => {
            let split = |v: Bits| match v {
                Bits::Bool(b) => (if b { -1 } else { 0 }, IntKind::Integer),
                Bits::Int(n, k) => (n, k),
            };
            let ((p, kp), (q, kq)) = (split(x), split(y));
            let kind = kp.max(kq);
            Ok(bits_value(apply_logic(op, p, q), kind))
        }
    }
}

/// `Not`.
pub fn not(a: &Value) -> R<Value> {
    if a.is_null() {
        return Ok(Value::Null);
    }
    Ok(match bits(a)? {
        Bits::Bool(b) => Value::Boolean(!b),
        Bits::Int(n, k) => bits_value(!n, k),
    })
}

/// Applies a binary operator to plain values (everything but `Is`, which needs objects).
pub fn binary(op: super::ast::BinOp, a: &Value, b: &Value, text: bool) -> R<Value> {
    use super::ast::BinOp;
    let cmp = |want: fn(Ordering) -> bool| -> R<Value> {
        Ok(match compare(a, b, text)? {
            Some(o) => Value::Boolean(want(o)),
            None => Value::Null,
        })
    };
    match op {
        BinOp::Pow => power(a, b),
        BinOp::Mul => arith(Arith::Mul, a, b),
        BinOp::Div => divide(a, b),
        BinOp::IntDiv => int_divide(a, b, false),
        BinOp::Mod => int_divide(a, b, true),
        BinOp::Add => arith(Arith::Add, a, b),
        BinOp::Sub => arith(Arith::Sub, a, b),
        BinOp::Concat => concat(a, b),
        BinOp::Eq => cmp(|o| o == Ordering::Equal),
        BinOp::Ne => cmp(|o| o != Ordering::Equal),
        BinOp::Lt => cmp(|o| o == Ordering::Less),
        BinOp::Gt => cmp(|o| o == Ordering::Greater),
        BinOp::Le => cmp(|o| o != Ordering::Greater),
        BinOp::Ge => cmp(|o| o != Ordering::Less),
        BinOp::Like => {
            if a.is_null() || b.is_null() {
                return Ok(Value::Null);
            }
            Ok(Value::Boolean(like(&to_str(a)?, &to_str(b)?, text)?))
        }
        BinOp::Is => Err(ScriptError::runtime(424)),
        BinOp::And => logic(Logic::And, a, b),
        BinOp::Or => logic(Logic::Or, a, b),
        BinOp::Xor => logic(Logic::Xor, a, b),
        BinOp::Eqv => logic(Logic::Eqv, a, b),
        BinOp::Imp => logic(Logic::Imp, a, b),
    }
}

// ---- Like (MS-VBAL §5.6.9.7) ----

enum Pat {
    Any,
    Star,
    Digit,
    Char(char),
    Set(bool, Vec<(char, char)>),
}

fn compile_like(pattern: &str, text: bool) -> R<Vec<Pat>> {
    let f = |c: char| if text { fold(c) } else { c };
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '?' => out.push(Pat::Any),
            '*' => out.push(Pat::Star),
            '#' => out.push(Pat::Digit),
            '[' => {
                let mut j = i + 1;
                let neg = chars.get(j) == Some(&'!');
                if neg {
                    j += 1;
                }
                let mut items = Vec::new();
                loop {
                    let Some(&c) = chars.get(j) else {
                        return Err(ScriptError::runtime(93));
                    };
                    if c == ']' {
                        break;
                    }
                    if chars.get(j + 1) == Some(&'-') && chars.get(j + 2).is_some_and(|e| *e != ']')
                    {
                        let hi = chars[j + 2];
                        if f(hi) < f(c) {
                            return Err(ScriptError::runtime(93));
                        }
                        items.push((f(c), f(hi)));
                        j += 3;
                    } else {
                        items.push((f(c), f(c)));
                        j += 1;
                    }
                }
                out.push(Pat::Set(neg, items));
                i = j;
            }
            c => out.push(Pat::Char(f(c))),
        }
        i += 1;
    }
    Ok(out)
}

/// `s Like pattern`: `?` one char, `*` any run, `#` a digit, `[a-z]`/`[!a-z]` char lists.
pub fn like(s: &str, pattern: &str, text: bool) -> R<bool> {
    let p = compile_like(pattern, text)?;
    let s: Vec<char> = s.chars().map(|c| if text { fold(c) } else { c }).collect();
    let matches = |pat: &Pat, c: char| match pat {
        Pat::Any => true,
        Pat::Digit => c.is_ascii_digit(),
        Pat::Char(x) => *x == c,
        Pat::Set(neg, items) => items.iter().any(|(lo, hi)| (*lo..=*hi).contains(&c)) != *neg,
        Pat::Star => false,
    };
    let (mut si, mut pi) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < s.len() {
        if pi < p.len() && matches!(p[pi], Pat::Star) {
            star = Some((pi, si));
            pi += 1;
        } else if pi < p.len() && matches(&p[pi], s[si]) {
            si += 1;
            pi += 1;
        } else if let Some((sp, ss)) = star {
            pi = sp + 1;
            si = ss + 1;
            star = Some((sp, ss + 1));
        } else {
            return Ok(false);
        }
    }
    while pi < p.len() && matches!(p[pi], Pat::Star) {
        pi += 1;
    }
    Ok(pi == p.len())
}

// ---- Dates from text ----

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

const WEEKDAYS: [&str; 7] = [
    "sunday",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
];

fn month_from_word(w: &str) -> Option<u32> {
    if w.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| m.starts_with(w) || (w == "sept" && *m == "september"))
        .map(|i| i as u32 + 1)
}

fn is_weekday_word(w: &str) -> bool {
    w.len() >= 3 && WEEKDAYS.iter().any(|d| d.starts_with(w))
}

#[derive(Debug)]
enum DTok {
    Num(u32, usize),
    Word(String),
    Colon,
}

/// Two-digit years: 0–29 are 2000–2029, 30–99 are 1930–1999 (VBA's default window).
pub fn expand_year(y: i64, digits: usize) -> i64 {
    if digits <= 2 && y < 100 {
        if y < 30 { 2000 + y } else { 1900 + y }
    } else {
        y
    }
}

/// Parses a date and/or time the way `CDate`/`IsDate` accept them in en-US: `1/15/2024`,
/// `2024-01-15`, `15 Jan 2024`, `January 15, 2024`, `13:45`, `1:45:30 PM`, combinations.
/// A day/month order that can't be a valid month/day is swapped (`15/1/2024`).
pub fn parse_date(s: &str) -> Option<f64> {
    let mut toks = Vec::new();
    let chars: Vec<char> = s.trim().chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            toks.push(DTok::Num(text.parse().ok()?, i - start));
            continue;
        }
        if c.is_alphabetic() {
            let start = i;
            while i < chars.len() && chars[i].is_alphabetic() {
                i += 1;
            }
            toks.push(DTok::Word(
                chars[start..i].iter().collect::<String>().to_lowercase(),
            ));
            continue;
        }
        match c {
            ':' => toks.push(DTok::Colon),
            '/' | '-' | '.' | ',' | ' ' | '\t' => {}
            _ => return None,
        }
        i += 1;
    }
    // Time: h:m[:s] [am|pm], or h am|pm.
    let mut time: Option<(u32, u32, u32)> = None;
    let mut date_nums: Vec<(u32, usize)> = Vec::new();
    let mut month_word: Option<u32> = None;
    let mut k = 0;
    while k < toks.len() {
        match &toks[k] {
            DTok::Num(h, _) if matches!(toks.get(k + 1), Some(DTok::Colon)) => {
                let Some(DTok::Num(m, _)) = toks.get(k + 2) else {
                    return None;
                };
                let (mut h, m) = (*h, *m);
                let mut sec = 0;
                k += 3;
                if matches!(toks.get(k), Some(DTok::Colon)) {
                    let Some(DTok::Num(sv, _)) = toks.get(k + 1) else {
                        return None;
                    };
                    sec = *sv;
                    k += 2;
                }
                if let Some(DTok::Word(w)) = toks.get(k)
                    && (w == "am" || w == "pm" || w == "a" || w == "p")
                {
                    if h == 0 || h > 12 {
                        return None;
                    }
                    h = h % 12 + if w.starts_with('p') { 12 } else { 0 };
                    k += 1;
                }
                if time.is_some() || h > 23 || m > 59 || sec > 59 {
                    return None;
                }
                time = Some((h, m, sec));
            }
            DTok::Num(h, _) if matches!(toks.get(k + 1), Some(DTok::Word(w)) if w == "am" || w == "pm") =>
            {
                let Some(DTok::Word(w)) = toks.get(k + 1) else {
                    return None;
                };
                if *h == 0 || *h > 12 || time.is_some() {
                    return None;
                }
                time = Some((h % 12 + if w == "pm" { 12 } else { 0 }, 0, 0));
                k += 2;
            }
            DTok::Num(n, d) => {
                date_nums.push((*n, *d));
                k += 1;
            }
            DTok::Word(w) => {
                if let Some(m) = month_from_word(w) {
                    if month_word.is_some() {
                        return None;
                    }
                    month_word = Some(m);
                } else if !is_weekday_word(w) {
                    return None;
                }
                k += 1;
            }
            DTok::Colon => return None,
        }
    }
    let day_number = if date_nums.is_empty() && month_word.is_none() {
        time?;
        0
    } else {
        let this_year = date::ymd(date::now_utc()).0;
        let (y, m, d) = match (month_word, date_nums.as_slice()) {
            (Some(m), [(a, da), (b, db)]) => {
                if *da >= 3 || *a > 31 {
                    (expand_year(i64::from(*a), *da), m, *b)
                } else {
                    (expand_year(i64::from(*b), *db), m, *a)
                }
            }
            (Some(m), [(a, da)]) => {
                if *da >= 3 || *a > 31 {
                    (expand_year(i64::from(*a), *da), m, 1)
                } else {
                    (this_year, m, *a)
                }
            }
            (None, [(a, da), (b, _), (c, dc)]) => {
                if *da >= 3 {
                    (i64::from(*a), *b, *c)
                } else if *a > 12 && *b <= 12 {
                    (expand_year(i64::from(*c), *dc), *b, *a)
                } else {
                    (expand_year(i64::from(*c), *dc), *a, *b)
                }
            }
            (None, [(a, _), (b, db)]) => {
                if *db >= 3 {
                    (i64::from(*b), *a, 1)
                } else if *a > 12 && *b <= 12 {
                    (this_year, *b, *a)
                } else {
                    (this_year, *a, *b)
                }
            }
            _ => return None,
        };
        if !(100..=9999).contains(&y)
            || !(1..=12).contains(&m)
            || d == 0
            || d > date::days_in_month(y, m)
        {
            return None;
        }
        date::day_number(y, m, d)
    };
    let secs = time.map_or(0, |(h, m, s)| h * 3600 + m * 60 + s);
    Some(date::join(day_number, f64::from(secs)))
}

/// A date literal's contents (`#1/2/2024 10:30:00 AM#`).
pub fn parse_date_literal(s: &str) -> Option<f64> {
    if s.trim().is_empty() {
        return None;
    }
    parse_date(s)
}

// ---- Type information ----

/// The element type name of an array for `TypeName`: the common type of its elements, or
/// Variant. (Kantor doesn't keep declared element types on array values.)
fn array_elem_name(a: &crate::object::Array) -> String {
    let mut name: Option<String> = None;
    for v in a.values() {
        let n = match v {
            Value::Empty => continue,
            Value::Object(_) => "Object".to_string(),
            other => type_name(other),
        };
        match &name {
            None => name = Some(n),
            Some(prev) if *prev == n => {}
            Some(_) => return "Variant".to_string(),
        }
    }
    name.unwrap_or_else(|| "Variant".to_string())
}

/// `TypeName`.
pub fn type_name(v: &Value) -> String {
    match v {
        Value::Empty => "Empty".into(),
        Value::Null => "Null".into(),
        Value::Missing | Value::Error(_) => "Error".into(),
        Value::Boolean(_) => "Boolean".into(),
        Value::Byte(_) => "Byte".into(),
        Value::Integer(_) => "Integer".into(),
        Value::Long(_) => "Long".into(),
        Value::LongLong(_) => "LongLong".into(),
        Value::Single(_) => "Single".into(),
        Value::Double(_) => "Double".into(),
        Value::Currency(_) => "Currency".into(),
        Value::Date(_) => "Date".into(),
        Value::String(_) => "String".into(),
        Value::Array(a) => format!("{}()", array_elem_name(a)),
        Value::Record(r) => r.type_name.clone(),
        Value::Object(None) => "Nothing".into(),
        Value::Object(Some(o)) => o.type_name(),
    }
}

/// `VarType`.
pub fn var_type(v: &Value) -> i32 {
    match v {
        Value::Empty => 0,
        Value::Null => 1,
        Value::Integer(_) => 2,
        Value::Long(_) => 3,
        Value::Single(_) => 4,
        Value::Double(_) => 5,
        Value::Currency(_) => 6,
        Value::Date(_) => 7,
        Value::String(_) => 8,
        Value::Object(_) => 9,
        Value::Error(_) | Value::Missing => 10,
        Value::Boolean(_) => 11,
        Value::Byte(_) => 17,
        Value::LongLong(_) => 20,
        Value::Record(_) => 36,
        Value::Array(a) => {
            8192 + match array_elem_name(a).as_str() {
                "Integer" => 2,
                "Long" => 3,
                "Single" => 4,
                "Double" => 5,
                "Currency" => 6,
                "Date" => 7,
                "String" => 8,
                "Object" => 9,
                "Boolean" => 11,
                "Byte" => 17,
                "LongLong" => 20,
                _ => 12,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_from_text() {
        assert_eq!(parse_number(" 1,234.5 "), Some(1234.5));
        assert_eq!(parse_number("-1e3"), Some(-1000.0));
        assert_eq!(parse_number("&HFF"), Some(255.0));
        assert_eq!(parse_number("12abc"), None);
        assert_eq!(parse_number(""), None);
        assert_eq!(val("  1 2 3abc"), 123.0);
        assert_eq!(val("&HFFFF"), -1.0);
        assert_eq!(val("-3.5e2x"), -350.0);
        assert_eq!(val("abc"), 0.0);
    }

    #[test]
    fn cstr_of_doubles() {
        assert_eq!(format_float(0.1 + 0.2, 15), "0.3");
        assert_eq!(format_float(1.0 / 3.0, 15), "0.333333333333333");
        assert_eq!(format_float(1e20, 15), "1E+20");
        assert_eq!(
            format_float(123_456_789_012_345_680.0, 15),
            "1.23456789012346E+17"
        );
        assert_eq!(format_float(0.0001, 15), "0.0001");
        assert_eq!(format_float(0.000_001_5, 15), "1.5E-06");
        assert_eq!(format_float(-2.5, 15), "-2.5");
        assert_eq!(format_float(100.0, 15), "100");
    }

    #[test]
    fn bankers_rounding() {
        // MS-VBAL §5.5.1.2.1: halfway values go to the even integer.
        assert_eq!(
            convert(&Value::Double(2.5), Scalar::Integer),
            Ok(Value::Integer(2))
        );
        assert_eq!(
            convert(&Value::Double(3.5), Scalar::Integer),
            Ok(Value::Integer(4))
        );
        assert_eq!(
            convert(&Value::Double(-2.5), Scalar::Long),
            Ok(Value::Long(-2))
        );
        assert_eq!(
            convert(&Value::Currency(25_000), Scalar::Integer),
            Ok(Value::Integer(2))
        );
        assert_eq!(
            convert(&Value::Double(32767.6), Scalar::Integer)
                .unwrap_err()
                .number,
            6
        );
    }

    #[test]
    fn operators_follow_variant_rules() {
        let i = Value::Integer;
        assert_eq!(arith(Arith::Add, &i(32767), &i(1)), Ok(Value::Long(32768)));
        assert_eq!(
            arith(Arith::Add, &"1".into(), &"2".into()),
            Ok(Value::from("12"))
        );
        assert_eq!(
            arith(Arith::Add, &"1".into(), &i(2)),
            Ok(Value::Double(3.0))
        );
        assert_eq!(arith(Arith::Add, &Value::Null, &i(2)), Ok(Value::Null));
        assert_eq!(int_divide(&i(7), &i(2), false), Ok(i(3)));
        assert_eq!(int_divide(&i(-7), &i(2), true), Ok(i(-1)));
        assert_eq!(
            int_divide(&Value::Double(7.5), &i(2), false),
            Ok(Value::Long(4))
        );
        assert_eq!(divide(&i(1), &i(0)).unwrap_err().number, 11);
        assert_eq!(concat(&Value::Null, &"a".into()), Ok(Value::from("a")));
        assert_eq!(logic(Logic::And, &i(12), &i(10)), Ok(i(8)));
        assert_eq!(
            logic(Logic::And, &Value::Null, &Value::Boolean(false)),
            Ok(Value::Boolean(false))
        );
        assert_eq!(
            logic(Logic::Or, &Value::Null, &Value::Boolean(false)),
            Ok(Value::Null)
        );
        assert_eq!(not(&i(0)), Ok(i(-1)));
        assert_eq!(negate(&i(-32768)), Ok(Value::Long(32768)));
    }

    #[test]
    fn comparisons() {
        assert_eq!(
            compare(&Value::Empty, &"".into(), false),
            Ok(Some(Ordering::Equal))
        );
        assert_eq!(
            compare(&Value::Empty, &Value::Integer(0), false),
            Ok(Some(Ordering::Equal))
        );
        assert_eq!(
            compare(&"abc".into(), &"ABC".into(), true),
            Ok(Some(Ordering::Equal))
        );
        assert_ne!(
            compare(&"abc".into(), &"ABC".into(), false),
            Ok(Some(Ordering::Equal))
        );
        assert_eq!(
            compare(&Value::Double(5.0), &"5".into(), false),
            Ok(Some(Ordering::Equal))
        );
        assert_eq!(
            compare(&Value::Double(5.0), &"x".into(), false),
            Ok(Some(Ordering::Less))
        );
        assert_eq!(compare(&Value::Null, &Value::Integer(1), false), Ok(None));
    }

    #[test]
    fn like_patterns() {
        assert!(like("aBBBa", "a*a", false).unwrap());
        assert!(like("F", "[A-Z]", false).unwrap());
        assert!(!like("f", "[A-Z]", false).unwrap());
        assert!(like("f", "[A-Z]", true).unwrap());
        assert!(like("a2a", "a#a", false).unwrap());
        assert!(like("aM5b", "a[L-P]#[!c-e]", false).unwrap());
        assert!(!like("CAT123khg", "B?T*", false).unwrap());
        assert!(like("BAT123khg", "B?T*", false).unwrap());
        assert!(like("", "*", false).unwrap());
        assert_eq!(like("x", "[a-", false).unwrap_err().number, 93);
    }

    #[test]
    fn dates_from_text() {
        let d = |y, m, dd| date::day_number(y, m, dd) as f64;
        assert_eq!(parse_date("1/15/2024"), Some(d(2024, 1, 15)));
        assert_eq!(parse_date("15/1/2024"), Some(d(2024, 1, 15)));
        assert_eq!(parse_date("2024-01-15"), Some(d(2024, 1, 15)));
        assert_eq!(parse_date("15 Jan 2024"), Some(d(2024, 1, 15)));
        assert_eq!(parse_date("January 15, 2024"), Some(d(2024, 1, 15)));
        assert_eq!(parse_date("Monday, January 15, 2024"), Some(d(2024, 1, 15)));
        assert_eq!(parse_date("1/15/24 6:00 PM"), Some(d(2024, 1, 15) + 0.75));
        assert_eq!(parse_date("13:30"), Some(13.5 / 24.0));
        assert_eq!(parse_date("2/30/2024"), None);
        assert_eq!(parse_date("hello"), None);
        assert_eq!(
            date_to_string(d(2024, 1, 15) + 0.75),
            "1/15/2024 6:00:00 PM"
        );
        assert_eq!(date_to_string(0.5), "12:00:00 PM");
    }
}
