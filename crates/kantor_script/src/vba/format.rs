//! VBA's `Format` function (VBA language reference, "Format function" and "User-defined
//! number/date/string formats"). en-US names and separators.
//!
//! VBA formats are not Excel number formats: minutes are `n` (or `m` next to `h`/`s`), there
//! are no colours or conditions, and `Format` rounds halfway values away from zero.

use super::convert::{self, decimal_digits};
use crate::date;
use crate::object::{ScriptError, Value};

type R<T> = Result<T, ScriptError>;

pub const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

pub const DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

fn abbrev(name: &str) -> &str {
    &name[..3]
}

/// `Format(value, fmt, firstdayofweek, firstweekofyear)`. `first_day` is 1 (Sunday) to 7;
/// `first_week` is 1 (week of Jan 1), 2 (first four-day week) or 3 (first full week).
pub fn format(v: &Value, fmt: &str, first_day: u32, first_week: u32) -> R<String> {
    if let Some(s) = named(v, fmt)? {
        return Ok(s);
    }
    if fmt.is_empty() {
        return Ok(match v {
            Value::Null | Value::Empty => String::new(),
            other => convert::to_str(other)?,
        });
    }
    match classify(fmt) {
        Kind::Date => {
            let d = match v {
                Value::Null | Value::Empty => return Ok(String::new()),
                Value::String(s) => {
                    match convert::parse_date(s).or_else(|| convert::parse_number(s)) {
                        Some(d) => d,
                        None => return Ok(s.clone()),
                    }
                }
                other => convert::to_f64(other)?,
            };
            Ok(format_date(d, fmt, first_day, first_week))
        }
        Kind::Number => {
            let x = match v {
                Value::Null => {
                    let sections = split_sections(fmt);
                    return Ok(sections
                        .get(3)
                        .map(|s| render_literals(s))
                        .unwrap_or_default());
                }
                Value::Empty => return Ok(String::new()),
                Value::String(s) => match convert::parse_number(s) {
                    Some(x) => x,
                    None => return Ok(s.clone()),
                },
                other => convert::to_f64(other)?,
            };
            Ok(format_number(x, fmt))
        }
        Kind::Text => {
            let s = match v {
                Value::Null | Value::Empty => String::new(),
                other => convert::to_str(other)?,
            };
            Ok(format_text(&s, fmt))
        }
    }
}

/// The predefined format names.
fn named(v: &Value, fmt: &str) -> R<Option<String>> {
    let lower = fmt.to_ascii_lowercase();
    let number = |code: &str| -> R<Option<String>> {
        match v {
            Value::Null | Value::Empty => Ok(Some(String::new())),
            Value::String(s) if convert::parse_number(s).is_none() => Ok(Some(s.clone())),
            other => Ok(Some(format_number(convert::to_f64(other)?, code))),
        }
    };
    let datef = |code: &str| -> R<Option<String>> {
        match v {
            Value::Null | Value::Empty => Ok(Some(String::new())),
            Value::String(s) => Ok(Some(match convert::parse_date(s) {
                Some(d) => format_date(d, code, 1, 1),
                None => s.clone(),
            })),
            other => Ok(Some(format_date(convert::to_f64(other)?, code, 1, 1))),
        }
    };
    let pair = |yes: &str, no: &str| -> R<Option<String>> {
        match v {
            Value::Null | Value::Empty => Ok(Some(String::new())),
            other => Ok(Some(
                if convert::to_bool(other)? { yes } else { no }.to_string(),
            )),
        }
    };
    match lower.as_str() {
        "general number" => match v {
            Value::Null | Value::Empty => Ok(Some(String::new())),
            Value::String(s) => Ok(Some(s.clone())),
            other => Ok(Some(convert::format_float(convert::to_f64(other)?, 15))),
        },
        "currency" => number("$#,##0.00;($#,##0.00)"),
        "fixed" => number("0.00"),
        "standard" => number("#,##0.00"),
        "percent" => number("0.00%"),
        "scientific" => number("0.00E+00"),
        "yes/no" => pair("Yes", "No"),
        "true/false" => pair("True", "False"),
        "on/off" => pair("On", "Off"),
        "general date" => match v {
            Value::Null | Value::Empty => Ok(Some(String::new())),
            Value::String(s) => Ok(Some(
                convert::parse_date(s).map_or_else(|| s.clone(), convert::date_to_string),
            )),
            other => Ok(Some(convert::date_to_string(convert::to_f64(other)?))),
        },
        "long date" => datef("dddd, mmmm d, yyyy"),
        "medium date" => datef("dd-mmm-yy"),
        "short date" => datef("m/d/yyyy"),
        "long time" => datef("h:mm:ss AM/PM"),
        "medium time" => datef("hh:mm AM/PM"),
        "short time" => datef("hh:mm"),
        _ => Ok(None),
    }
}

enum Kind {
    Number,
    Date,
    Text,
}

/// Decides how a user-defined format reads, looking only outside quotes and escapes.
fn classify(fmt: &str) -> Kind {
    let mut has_digit = false;
    let mut has_date = false;
    let mut has_month = false;
    let mut has_text = false;
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    i += 1;
                }
            }
            '\\' => i += 1,
            '0' | '#' => has_digit = true,
            'd' | 'D' | 'y' | 'Y' | 'h' | 'H' | 'n' | 'N' | 's' | 'S' | 'w' | 'W' | 'q' | 'Q'
            | 'c' | 'C' | 't' | 'T' => has_date = true,
            'm' | 'M' => has_month = true,
            'a' | 'A' => {
                let rest: String = chars[i..]
                    .iter()
                    .take(5)
                    .collect::<String>()
                    .to_ascii_lowercase();
                if rest.starts_with("am/pm") || rest.starts_with("a/p") || rest.starts_with("ampm")
                {
                    has_date = true;
                }
            }
            '@' | '&' | '<' | '>' | '!' => has_text = true,
            _ => {}
        }
        i += 1;
    }
    if has_digit {
        Kind::Number
    } else if has_date || has_month {
        Kind::Date
    } else if has_text {
        Kind::Text
    } else {
        Kind::Number
    }
}

fn split_sections(fmt: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut chars = fmt.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                let last = out.last_mut().expect("sections start non-empty");
                last.push(c);
                for q in chars.by_ref() {
                    last.push(q);
                    if q == '"' {
                        break;
                    }
                }
            }
            '\\' => {
                let last = out.last_mut().expect("sections start non-empty");
                last.push(c);
                if let Some(n) = chars.next() {
                    last.push(n);
                }
            }
            ';' => out.push(String::new()),
            c => out.last_mut().expect("sections start non-empty").push(c),
        }
    }
    out
}

fn render_literals(section: &str) -> String {
    let mut out = String::new();
    let mut chars = section.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                    out.push(q);
                }
            }
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            c => out.push(c),
        }
    }
    out
}

// ---- Numbers ----

#[derive(Debug)]
enum Item {
    Digit(char),
    Point,
    Percent,
    Exp {
        upper: bool,
        plus: bool,
        digits: Vec<char>,
    },
    Lit(String),
}

fn format_number(x: f64, fmt: &str) -> String {
    let sections = split_sections(fmt);
    let pick = |i: usize| sections.get(i).filter(|s| !s.is_empty());
    let (section, x, minus) = if x < 0.0 {
        match pick(1) {
            Some(s) => (s.clone(), -x, false),
            None => (sections[0].clone(), -x, true),
        }
    } else if x == 0.0 {
        match pick(2) {
            Some(s) => (s.clone(), x, false),
            None => (sections[0].clone(), x, false),
        }
    } else {
        (sections[0].clone(), x, false)
    };
    let body = render_section(x, &section);
    if minus { format!("-{body}") } else { body }
}

fn parse_section(section: &str) -> (Vec<Item>, bool, i32) {
    let chars: Vec<char> = section.chars().collect();
    let mut items = Vec::new();
    let mut thousands = false;
    let mut scale = 0;
    let mut seen_point = false;
    let mut seen_digit = false;
    let mut i = 0;
    let is_ph = |c: char| matches!(c, '0' | '#' | '?');
    while i < chars.len() {
        let c = chars[i];
        match c {
            '"' => {
                let mut s = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    s.push(chars[i]);
                    i += 1;
                }
                items.push(Item::Lit(s));
            }
            '\\' => {
                if let Some(n) = chars.get(i + 1) {
                    items.push(Item::Lit(n.to_string()));
                    i += 1;
                }
            }
            c if is_ph(c) => {
                seen_digit = true;
                items.push(Item::Digit(c));
            }
            '.' if !seen_point => {
                seen_point = true;
                items.push(Item::Point);
            }
            ',' => {
                let next = chars.get(i + 1).copied();
                if seen_digit && !seen_point && next.is_some_and(is_ph) {
                    thousands = true;
                } else if seen_digit && (next.is_none() || next == Some('.') || next == Some(',')) {
                    scale += 1;
                } else {
                    items.push(Item::Lit(",".into()));
                }
            }
            '%' => items.push(Item::Percent),
            'E' | 'e'
                if matches!(chars.get(i + 1), Some('+' | '-'))
                    && chars.get(i + 2).is_some_and(|c| is_ph(*c)) =>
            {
                let plus = chars[i + 1] == '+';
                let mut digits = Vec::new();
                let mut j = i + 2;
                while j < chars.len() && is_ph(chars[j]) {
                    digits.push(chars[j]);
                    j += 1;
                }
                items.push(Item::Exp {
                    upper: c == 'E',
                    plus,
                    digits,
                });
                i = j - 1;
            }
            c => items.push(Item::Lit(c.to_string())),
        }
        i += 1;
    }
    (items, thousands, scale)
}

/// Digits of `x` rounded half away from zero to `d` decimals, as (integer, fraction).
fn round_decimal(x: f64, d: usize) -> (Vec<u8>, Vec<u8>) {
    if x == 0.0 {
        return (vec![b'0'], vec![b'0'; d]);
    }
    let (_, ds, exp) = decimal_digits(x, 15);
    let (mut int, mut frac): (Vec<u8>, Vec<u8>) = if exp >= 0 {
        let k = exp as usize + 1;
        let mut int: Vec<u8> = ds.iter().take(k).copied().collect();
        while int.len() < k {
            int.push(b'0');
        }
        (int, ds.iter().skip(k).copied().collect())
    } else {
        let mut frac = vec![b'0'; (-exp - 1) as usize];
        frac.extend_from_slice(&ds);
        (vec![b'0'], frac)
    };
    if frac.len() > d {
        let up = frac[d] >= b'5';
        frac.truncate(d);
        if up {
            let mut carry = true;
            for digit in frac.iter_mut().rev().chain(int.iter_mut().rev()) {
                if *digit == b'9' {
                    *digit = b'0';
                } else {
                    *digit += 1;
                    carry = false;
                    break;
                }
            }
            if carry {
                int.insert(0, b'1');
            }
        }
    }
    frac.resize(d, b'0');
    while int.len() > 1 && int[0] == b'0' {
        int.remove(0);
    }
    (int, frac)
}

fn group_thousands(digits: &[u8]) -> String {
    let mut out = String::new();
    for (i, d) in digits.iter().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*d as char);
    }
    out
}

fn render_section(x: f64, section: &str) -> String {
    let (items, thousands, scale) = parse_section(section);
    let percents = items.iter().filter(|i| matches!(i, Item::Percent)).count() as i32;
    let exp_at = items.iter().position(|i| matches!(i, Item::Exp { .. }));
    let point_at = items.iter().position(|i| matches!(i, Item::Point));
    let mantissa_end = exp_at.unwrap_or(items.len());
    let int_end = point_at.unwrap_or(mantissa_end).min(mantissa_end);
    let int_ph: Vec<char> = items[..int_end]
        .iter()
        .filter_map(|i| match i {
            Item::Digit(c) => Some(*c),
            _ => None,
        })
        .collect();
    let frac_ph: Vec<char> = items[int_end..mantissa_end]
        .iter()
        .filter_map(|i| match i {
            Item::Digit(c) => Some(*c),
            _ => None,
        })
        .collect();
    let mut x = x * 100f64.powi(percents) / 1000f64.powi(scale);
    let mut exponent = 0i32;
    if exp_at.is_some() && x != 0.0 {
        let int_digits = int_ph.len().max(1) as i32;
        exponent = x.log10().floor() as i32 - (int_digits - 1);
        x /= 10f64.powi(exponent);
        let (int, _) = round_decimal(x, frac_ph.len());
        if int.len() as i32 > int_digits {
            exponent += 1;
            x /= 10.0;
        }
    }
    let (int, frac) = round_decimal(x, frac_ph.len());
    let int: Vec<u8> = if int == *b"0" { Vec::new() } else { int };
    // Fraction digits: drop trailing zeros the `#` placeholders don't need.
    let keep = frac_ph
        .iter()
        .rposition(|c| *c == '0')
        .map_or(0, |p| p + 1)
        .max(frac.iter().rposition(|d| *d != b'0').map_or(0, |p| p + 1));
    let mut out = String::new();
    let mut int_seen = 0usize;
    let mut frac_seen = 0usize;
    // Integer digits by placeholder, right-aligned; extra digits go to the first one.
    let n_int = int_ph.len();
    let int_text = |out: &mut String, slot: usize| {
        // `slot` counts placeholders from the left.
        let from_right = n_int - 1 - slot;
        if thousands {
            if slot == 0 {
                let zeros = int_ph
                    .iter()
                    .position(|c| *c == '0')
                    .map_or(0, |p| n_int - p);
                let mut digits = int.clone();
                while digits.len() < zeros {
                    digits.insert(0, b'0');
                }
                out.push_str(&group_thousands(&digits));
            }
            return;
        }
        if slot == 0 && int.len() > n_int {
            out.extend(int[..int.len() - n_int + 1].iter().map(|d| *d as char));
            return;
        }
        if from_right < int.len() {
            out.push(int[int.len() - 1 - from_right] as char);
        } else {
            match int_ph[slot] {
                '0' => out.push('0'),
                '?' => out.push(' '),
                _ => {}
            }
        }
    };
    if n_int == 0 && !int.is_empty() {
        out.extend(int.iter().map(|d| *d as char));
    }
    for (idx, item) in items.iter().enumerate() {
        match item {
            Item::Digit(c) => {
                if idx < int_end {
                    int_text(&mut out, int_seen);
                    int_seen += 1;
                } else {
                    if frac_seen < keep {
                        out.push(frac[frac_seen] as char);
                    } else if *c == '0' {
                        out.push('0');
                    } else if *c == '?' {
                        out.push(' ');
                    }
                    frac_seen += 1;
                }
            }
            Item::Point => out.push('.'),
            Item::Percent => out.push('%'),
            Item::Exp {
                upper,
                plus,
                digits,
            } => {
                out.push(if *upper { 'E' } else { 'e' });
                if exponent < 0 {
                    out.push('-');
                } else if *plus {
                    out.push('+');
                }
                let min = digits.iter().filter(|c| **c == '0').count();
                out.push_str(&format!("{:0min$}", exponent.unsigned_abs()));
            }
            Item::Lit(s) => out.push_str(s),
        }
    }
    out
}

// ---- Dates ----

#[derive(Debug, Clone, PartialEq)]
enum DTok {
    Lit(String),
    General,
    Day(usize),
    Weekday,
    Week,
    Month(usize),
    Minute(usize),
    Quarter,
    DayOfYear,
    Year(usize),
    Hour(usize),
    Second(usize),
    LongTime,
    AmPm(String),
}

/// The week number of a date (`ww`, `DatePart("ww")`).
pub fn week_of_year(d: f64, first_day: u32, first_week: u32) -> i64 {
    let (y, _, _) = date::ymd(d);
    let day = date::split(d).0;
    let week_start = |dn: i64| dn - (dn + 6 - i64::from(first_day) + 1).rem_euclid(7);
    let first_week_start = |year: i64| {
        let jan1 = date::day_number(year, 1, 1);
        let start = week_start(jan1);
        let days_in_year_part = start + 7 - jan1;
        match first_week {
            2 if days_in_year_part < 4 => start + 7,
            3 if start != jan1 => start + 7,
            _ => start,
        }
    };
    let mut start = first_week_start(y);
    if day < start {
        start = first_week_start(y - 1);
    } else if first_week != 1 {
        let next = first_week_start(y + 1);
        if day >= next {
            start = next;
        }
    }
    (week_start(day) - start) / 7 + 1
}

fn tokenize_date(fmt: &str) -> Vec<DTok> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let run = |i: usize, c: char| {
        let mut j = i;
        while j < chars.len() && chars[j].eq_ignore_ascii_case(&c) {
            j += 1;
        }
        j - i
    };
    while i < chars.len() {
        let c = chars[i];
        let lc = c.to_ascii_lowercase();
        match lc {
            '"' => {
                let mut s = String::new();
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    s.push(chars[i]);
                    i += 1;
                }
                out.push(DTok::Lit(s));
                i += 1;
            }
            '\\' => {
                if let Some(n) = chars.get(i + 1) {
                    out.push(DTok::Lit(n.to_string()));
                }
                i += 2;
            }
            'c' => {
                out.push(DTok::General);
                i += 1;
            }
            'd' => {
                let n = run(i, 'd');
                out.push(DTok::Day(n.min(6)));
                i += n;
            }
            'w' => {
                let n = run(i, 'w');
                out.push(if n >= 2 { DTok::Week } else { DTok::Weekday });
                i += n;
            }
            'm' => {
                let n = run(i, 'm');
                out.push(DTok::Month(n.min(4)));
                i += n;
            }
            'q' => {
                out.push(DTok::Quarter);
                i += 1;
            }
            'y' => {
                let n = run(i, 'y');
                match n {
                    1 => out.push(DTok::DayOfYear),
                    2 => out.push(DTok::Year(2)),
                    3 => {
                        out.push(DTok::Year(2));
                        out.push(DTok::DayOfYear);
                    }
                    _ => out.push(DTok::Year(4)),
                }
                i += n;
            }
            'h' => {
                let n = run(i, 'h');
                out.push(DTok::Hour(n.min(2)));
                i += n;
            }
            'n' => {
                let n = run(i, 'n');
                out.push(DTok::Minute(n.min(2)));
                i += n;
            }
            's' => {
                let n = run(i, 's');
                out.push(DTok::Second(n.min(2)));
                i += n;
            }
            't' if run(i, 't') >= 5 => {
                out.push(DTok::LongTime);
                i += run(i, 't');
            }
            'a' => {
                let rest: String = chars[i..].iter().take(5).collect();
                let lower = rest.to_ascii_lowercase();
                if lower.starts_with("am/pm") {
                    out.push(DTok::AmPm(rest[..5].to_string()));
                    i += 5;
                } else if lower.starts_with("ampm") {
                    out.push(DTok::AmPm("AMPM".into()));
                    i += 4;
                } else if lower.starts_with("a/p") {
                    out.push(DTok::AmPm(rest[..3].to_string()));
                    i += 3;
                } else {
                    out.push(DTok::Lit(c.to_string()));
                    i += 1;
                }
            }
            _ => {
                out.push(DTok::Lit(c.to_string()));
                i += 1;
            }
        }
    }
    // `m`/`mm` right after an hour or right before a second is minutes.
    let is_field = |t: &DTok| !matches!(t, DTok::Lit(_));
    for k in 0..out.len() {
        if let DTok::Month(n) = out[k]
            && n <= 2
        {
            let prev = out[..k].iter().rev().find(|t| is_field(t));
            let next = out[k + 1..].iter().find(|t| is_field(t));
            if matches!(prev, Some(DTok::Hour(_))) || matches!(next, Some(DTok::Second(_))) {
                out[k] = DTok::Minute(n);
            }
        }
    }
    out
}

fn format_date(d: f64, fmt: &str, first_day: u32, first_week: u32) -> String {
    let toks = tokenize_date(fmt);
    let twelve = toks.iter().any(|t| matches!(t, DTok::AmPm(_)));
    let (y, m, day) = date::ymd(d);
    let (h, mi, s) = date::hms(d);
    let wd = date::weekday(d);
    let mut out = String::new();
    for t in &toks {
        match t {
            DTok::Lit(s) => out.push_str(s),
            DTok::General => out.push_str(&convert::date_to_string(d)),
            DTok::Day(1) => out.push_str(&day.to_string()),
            DTok::Day(2) => out.push_str(&format!("{day:02}")),
            DTok::Day(3) => out.push_str(abbrev(DAY_NAMES[wd as usize - 1])),
            DTok::Day(4) => out.push_str(DAY_NAMES[wd as usize - 1]),
            DTok::Day(5) => out.push_str(&format!("{m}/{day}/{y}")),
            DTok::Day(_) => out.push_str(&format!(
                "{}, {} {}, {}",
                DAY_NAMES[wd as usize - 1],
                MONTH_NAMES[m as usize - 1],
                day,
                y
            )),
            DTok::Weekday => {
                let n = (wd + 7 - first_day) % 7 + 1;
                out.push_str(&n.to_string());
            }
            DTok::Week => out.push_str(&week_of_year(d, first_day, first_week).to_string()),
            DTok::Month(1) => out.push_str(&m.to_string()),
            DTok::Month(2) => out.push_str(&format!("{m:02}")),
            DTok::Month(3) => out.push_str(abbrev(MONTH_NAMES[m as usize - 1])),
            DTok::Month(_) => out.push_str(MONTH_NAMES[m as usize - 1]),
            DTok::Minute(1) => out.push_str(&mi.to_string()),
            DTok::Minute(_) => out.push_str(&format!("{mi:02}")),
            DTok::Quarter => out.push_str(&((m - 1) / 3 + 1).to_string()),
            DTok::DayOfYear => {
                let doy = date::split(d).0 - date::day_number(y, 1, 1) + 1;
                out.push_str(&doy.to_string());
            }
            DTok::Year(2) => out.push_str(&format!("{:02}", y.rem_euclid(100))),
            DTok::Year(_) => out.push_str(&format!("{y:04}")),
            DTok::Hour(n) => {
                let hh = if twelve {
                    if h.is_multiple_of(12) { 12 } else { h % 12 }
                } else {
                    h
                };
                if *n >= 2 {
                    out.push_str(&format!("{hh:02}"));
                } else {
                    out.push_str(&hh.to_string());
                }
            }
            DTok::Second(1) => out.push_str(&s.to_string()),
            DTok::Second(_) => out.push_str(&format!("{s:02}")),
            DTok::LongTime => out.push_str(&format_date(d, "h:mm:ss AM/PM", 1, 1)),
            DTok::AmPm(pat) => {
                let pm = h >= 12;
                let text = match pat.to_ascii_lowercase().as_str() {
                    "ampm" => if pm { "PM" } else { "AM" }.to_string(),
                    "a/p" => {
                        let c = if pm { 'p' } else { 'a' };
                        if pat.starts_with('A') {
                            c.to_ascii_uppercase().to_string()
                        } else {
                            c.to_string()
                        }
                    }
                    _ => {
                        let t = if pm { "pm" } else { "am" };
                        if pat.starts_with('A') {
                            t.to_ascii_uppercase()
                        } else {
                            t.to_string()
                        }
                    }
                };
                out.push_str(&text);
            }
        }
    }
    out
}

// ---- Text ----

fn format_text(s: &str, fmt: &str) -> String {
    let section = split_sections(fmt).into_iter().next().unwrap_or_default();
    let upper = section.contains('>');
    let lower = section.contains('<');
    let left_to_right = section.contains('!');
    let s: String = if upper {
        s.to_uppercase()
    } else if lower {
        s.to_lowercase()
    } else {
        s.to_string()
    };
    let mut items: Vec<Option<char>> = Vec::new(); // None: placeholder, Some: literal
    let mut kinds: Vec<char> = Vec::new();
    let mut chars = section.chars();
    while let Some(c) = chars.next() {
        match c {
            '@' | '&' => {
                items.push(None);
                kinds.push(c);
            }
            '<' | '>' | '!' => {}
            '\\' => {
                if let Some(n) = chars.next() {
                    items.push(Some(n));
                }
            }
            '"' => {
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                    items.push(Some(q));
                }
            }
            c => items.push(Some(c)),
        }
    }
    if kinds.is_empty() {
        let lits: String = items.iter().flatten().collect();
        return format!("{s}{lits}");
    }
    let text: Vec<char> = s.chars().collect();
    let n = kinds.len();
    let mut fill: Vec<String> = vec![String::new(); n];
    if left_to_right {
        for (k, f) in fill.iter_mut().enumerate() {
            if let Some(c) = text.get(k) {
                f.push(*c);
            } else if kinds[k] == '@' {
                f.push(' ');
            }
        }
        if text.len() > n {
            fill[n - 1].extend(&text[n..]);
        }
    } else {
        let extra = text.len().saturating_sub(n);
        for (k, f) in fill.iter_mut().enumerate() {
            let from_right = n - 1 - k;
            if from_right < text.len() {
                if k == 0 && extra > 0 {
                    f.extend(&text[..=extra]);
                } else {
                    f.push(text[text.len() - 1 - from_right]);
                }
            } else if kinds[k] == '@' {
                f.push(' ');
            }
        }
    }
    let mut out = String::new();
    let mut k = 0;
    for item in items {
        match item {
            Some(c) => out.push(c),
            None => {
                out.push_str(&fill[k]);
                k += 1;
            }
        }
    }
    out
}

/// `FormatNumber`/`FormatPercent`/`FormatCurrency`: `digits` decimals (-1 is 2), with
/// thousands grouping and a leading zero unless turned off; `parens` for negatives.
pub fn format_fixed(x: f64, digits: i32, leading_zero: bool, parens: bool, group: bool) -> String {
    let d = if digits < 0 { 2 } else { digits as usize };
    let (int, frac) = round_decimal(x.abs(), d);
    let mut s = if group {
        group_thousands(&int)
    } else {
        int.iter().map(|b| *b as char).collect()
    };
    if s == "0" && !leading_zero && d > 0 {
        s.clear();
    }
    if d > 0 {
        s.push('.');
        s.extend(frac.iter().map(|b| *b as char));
    }
    let zero = int == *b"0" && frac.iter().all(|b| *b == b'0');
    if x < 0.0 && !zero {
        if parens {
            format!("({s})")
        } else {
            format!("-{s}")
        }
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(v: impl Into<Value>, fmt: &str) -> String {
        format(&v.into(), fmt, 1, 1).unwrap()
    }

    #[test]
    fn numbers() {
        assert_eq!(f(1234.567, "#,##0.00"), "1,234.57");
        assert_eq!(f(0.5, "0.00"), "0.50");
        assert_eq!(f(0.5, "#.##"), ".5");
        assert_eq!(f(5.0, "#.##"), "5.");
        assert_eq!(f(2.5, "0"), "3");
        assert_eq!(f(0.125, "0.00"), "0.13");
        assert_eq!(f(1.005, "0.00"), "1.01");
        assert_eq!(f(0.256, "0.0%"), "25.6%");
        assert_eq!(f(-1234.5, "#,##0.00;(#,##0.00)"), "(1,234.50)");
        assert_eq!(f(-3.0, "0.0"), "-3.0");
        assert_eq!(f(0.0, "0.0;-0.0;\"zero\""), "zero");
        assert_eq!(f(12345.0, "0.00E+00"), "1.23E+04");
        assert_eq!(f(0.00012, "0.0E-00"), "1.2E-04");
        assert_eq!(f(7.0, "000"), "007");
        assert_eq!(f(1234567.0, "#,##0"), "1,234,567");
        assert_eq!(f(1234567.0, "#,##0,"), "1,235");
        assert_eq!(f(42.0, "\"Total: \"0"), "Total: 42");
        assert_eq!(f(1234.5, "Currency"), "$1,234.50");
        assert_eq!(f(-1234.5, "Currency"), "($1,234.50)");
        assert_eq!(f(0.1234, "Percent"), "12.34%");
        assert_eq!(f(1234.5, "Standard"), "1,234.50");
        assert_eq!(f(1.23456, "Fixed"), "1.23");
        assert_eq!(f(1.0, "Yes/No"), "Yes");
        assert_eq!(f("abc", "0.00"), "abc");
        assert_eq!(f(123456789.0, "000-00-0000"), "123-45-6789");
    }

    #[test]
    fn dates() {
        let d = Value::Date(
            date::day_number(2024, 3, 5) as f64 + (14.0 * 3600.0 + 7.0 * 60.0 + 9.0) / 86400.0,
        );
        let g = |fmt: &str| format(&d, fmt, 1, 1).unwrap();
        assert_eq!(g("yyyy-mm-dd"), "2024-03-05");
        assert_eq!(g("dd/mm/yyyy hh:nn:ss"), "05/03/2024 14:07:09");
        assert_eq!(g("hh:mm"), "14:07");
        assert_eq!(g("h:mm AM/PM"), "2:07 PM");
        assert_eq!(g("ddd d mmm yy"), "Tue 5 Mar 24");
        assert_eq!(g("dddd, mmmm d, yyyy"), "Tuesday, March 5, 2024");
        assert_eq!(g("Long Date"), "Tuesday, March 5, 2024");
        assert_eq!(g("Short Date"), "3/5/2024");
        assert_eq!(g("q"), "1");
        assert_eq!(g("y"), "65");
        assert_eq!(g("w"), "3");
        assert_eq!(g("ww"), "10");
        assert_eq!(g("mmmm yyyy"), "March 2024");
        assert_eq!(
            format(&"2024-03-05".into(), "dd mmm yyyy", 1, 1).unwrap(),
            "05 Mar 2024"
        );
    }

    #[test]
    fn text_and_helpers() {
        assert_eq!(f("hello", ">"), "HELLO");
        assert_eq!(f("HeLLo", "<"), "hello");
        assert_eq!(f("abc", "@@@@@"), "  abc");
        assert_eq!(f("abc", "!@@@@@"), "abc  ");
        assert_eq!(f("5551234", "@@@-@@@@"), "555-1234");
        assert_eq!(format_fixed(-1234.567, 2, true, false, true), "-1,234.57");
        assert_eq!(format_fixed(0.5, 1, false, false, true), ".5");
        assert_eq!(format_fixed(-3.0, 0, true, true, true), "(3)");
    }
}
