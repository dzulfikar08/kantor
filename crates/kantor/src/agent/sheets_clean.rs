//! Tidying data: sort, remove duplicates, trim text, numbers and dates
//! stored as text, split and combine columns, find and replace
//! ([`super::sheets_author`]'s tools).

use std::collections::HashSet;

use anyhow::{Context as _, Result, bail};
use serde_json::{Map, Value, json};
use sheets_engine::{CellRange, SortKey, ValueKind, Workbook, cell_name, column_name};

use super::sheets_author::{
    as_text_input, columns_arg, get, opt_bool, opt_str, parse_ref, range_arg, req_str, sheet_arg,
    used_part,
};

pub(super) fn run(
    book: &mut Workbook,
    tool: &str,
    o: &Map<String, Value>,
) -> Result<(String, Value)> {
    match tool {
        "sort" => sort(book, o),
        "remove_duplicates" => dedupe(book, o),
        "trim" => trim(book, o),
        "to_numbers" => to_numbers(book, o),
        "to_dates" => to_dates(book, o),
        "split_column" => split(book, o),
        "combine_columns" => combine(book, o),
        "find_replace" => find_replace(book, o),
        other => bail!("unknown tool {other}"),
    }
}

/// Whether a cell holds a formula.
pub fn is_formula(book: &Workbook, sheet: u32, r: u32, c: u32) -> bool {
    book.input(sheet, r, c).starts_with('=')
}

/// A text constant's text (not formulas, numbers, ...).
fn text_constant(book: &Workbook, sheet: u32, r: u32, c: u32) -> Option<String> {
    if book.value_kind(sheet, r, c) != ValueKind::Text || is_formula(book, sheet, r, c) {
        return None;
    }
    let s = book.display_value(sheet, r, c);
    (!s.is_empty()).then_some(s)
}

/// The header row: `header` given, else guessed as Excel does.
fn header_of(book: &Workbook, o: &Map<String, Value>, sheet: u32, r: CellRange) -> Result<bool> {
    Ok(match opt_bool(o, "header")? {
        Some(h) => h,
        None => book.guess_header(sheet, r),
    })
}

fn sort(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let (sheet, r) = range_arg(book, o, "range")?;
    let r = used_part(book, sheet, r);
    let all_desc = opt_bool(o, "descending")?.unwrap_or(false);
    let by = get(o, "by").context("no `by`")?;
    let items: Vec<Value> = match by {
        Value::Array(a) => a.clone(),
        v => vec![v.clone()],
    };
    let mut keys = Vec::new();
    for item in &items {
        let (col, desc) = match item {
            Value::String(c) => (c.as_str(), all_desc),
            Value::Object(k) => {
                for key in k.keys() {
                    if !["column", "order", "descending"].contains(&key.as_str()) {
                        bail!("a sort key has no \"{key}\" (keys: column, order, descending)");
                    }
                }
                let desc = match (opt_str(k, "order")?, opt_bool(k, "descending")?) {
                    (Some("desc" | "descending"), _) | (_, Some(true)) => true,
                    (Some("asc" | "ascending") | None, _) => all_desc && k.get("order").is_none(),
                    (Some(other), _) => bail!("sort `order` must be asc or desc, not \"{other}\""),
                };
                (req_str(k, "column")?, desc)
            }
            _ => bail!("`by` is a column letter, {{column, order}}, or a list of them"),
        };
        let c = columns_arg(&json!(col), "by")?[0];
        if c < r.left || c > r.right {
            bail!("sort column {col} is outside {}", r.name());
        }
        keys.push(SortKey {
            col: c,
            descending: desc,
        });
    }
    let header = header_of(book, o, sheet, r)?;
    book.sort_range(sheet, r, &keys, header)?;
    Ok((
        format!("sorted {}", r.name()),
        json!({"range": r.name(), "header": header}),
    ))
}

/// A cell's value for comparing rows: case and outer spaces ignored.
fn key_of(book: &Workbook, sheet: u32, r: u32, c: u32) -> String {
    match book.number_at(sheet, r, c) {
        Some((n, _)) => format!("n:{n}"),
        None => format!(
            "t:{}",
            book.display_value(sheet, r, c).trim().to_lowercase()
        ),
    }
}

fn dedupe(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let (sheet, r) = range_arg(book, o, "range")?;
    let r = used_part(book, sheet, r);
    let cols = match get(o, "columns") {
        Some(v) => columns_arg(v, "columns")?,
        None => (r.left..=r.right).collect(),
    };
    if cols.iter().any(|c| *c < r.left || *c > r.right) {
        bail!("`columns` must be inside {}", r.name());
    }
    let header = header_of(book, o, sheet, r)?;
    let first = r.top + u32::from(header);
    let mut seen = HashSet::new();
    let mut dups = Vec::new();
    for row in first..=r.bottom {
        let key: Vec<String> = cols.iter().map(|&c| key_of(book, sheet, row, c)).collect();
        if key.iter().all(|k| k == "t:") {
            continue;
        }
        if !seen.insert(key) {
            dups.push(row);
        }
    }
    if dups.is_empty() {
        return Ok(("no duplicate rows".into(), json!({"removed": 0})));
    }
    // Whole rows go when the duplicates have nothing outside the range.
    let whole_rows = dups.iter().all(|&row| {
        book.content_columns_in_row(sheet, row)
            .iter()
            .all(|c| (r.left..=r.right).contains(c))
    });
    if whole_rows {
        // Delete runs of rows from the bottom up.
        let mut i = dups.len();
        while i > 0 {
            let last = dups[i - 1];
            let mut j = i - 1;
            while j > 0 && dups[j - 1] + 1 == dups[j] {
                j -= 1;
            }
            book.delete_rows(sheet, dups[j], last)?;
            i = j;
        }
    } else {
        // Only the range's cells move up (as Excel does); formulas would
        // keep pointing at their old rows, so they are refused.
        if (first..=r.bottom).any(|row| (r.left..=r.right).any(|c| is_formula(book, sheet, row, c)))
        {
            bail!(
                "{} has formulas and its duplicate rows have cells outside it; give a range spanning those cells so whole rows are deleted",
                r.name()
            );
        }
        let gone: HashSet<u32> = dups.iter().copied().collect();
        let keep: Vec<u32> = (first..=r.bottom).filter(|x| !gone.contains(x)).collect();
        let mut cells = Vec::new();
        for (i, row) in (first..=r.bottom).enumerate() {
            for c in r.left..=r.right {
                let v = match keep.get(i) {
                    Some(&src) => book.input(sheet, src, c),
                    None => String::new(),
                };
                cells.push((row, c, v));
            }
        }
        book.set_inputs(sheet, &cells)?;
    }
    let shown: Vec<u32> = dups.iter().take(50).map(|r| r + 1).collect();
    Ok((
        format!("removed {} duplicate row(s)", dups.len()),
        json!({"removed": dups.len(), "rows": shown, "header": header}),
    ))
}

/// Text tidied as Excel's TRIM and CLEAN do (and non-breaking spaces).
fn tidy(s: &str, collapse: bool) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .map(|c| {
            if matches!(c, '\u{a0}' | '\u{2007}' | '\u{202f}' | '\t') {
                ' '
            } else {
                c
            }
        })
        .filter(|c| !matches!(c, '\u{200b}' | '\u{feff}'))
        .collect();
    let lines: Vec<String> = cleaned
        .split('\n')
        .map(|l| {
            if collapse {
                l.split(' ')
                    .filter(|w| !w.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                l.trim().to_owned()
            }
        })
        .collect();
    lines.join("\n").trim().to_owned()
}

fn trim(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let (sheet, r) = range_arg(book, o, "range")?;
    let r = used_part(book, sheet, r);
    let collapse = opt_bool(o, "collapse")?.unwrap_or(true);
    let mut cells = Vec::new();
    for row in r.top..=r.bottom {
        for c in r.left..=r.right {
            if let Some(s) = text_constant(book, sheet, row, c) {
                let t = tidy(&s, collapse);
                if t != s {
                    cells.push((row, c, as_text_input(&t)));
                }
            }
        }
    }
    let n = cells.len();
    book.set_inputs(sheet, &cells)?;
    Ok((format!("trimmed {n} cell(s)"), json!({"changed": n})))
}

/// The decimal separator `decimal` asks for, or the one the texts use.
fn decimal_of(texts: &[String], decimal: &str) -> Result<char> {
    Ok(match decimal {
        "." => '.',
        "," => ',',
        "auto" => {
            // A comma followed by one or two digits at the end, or after a
            // dot grouping, means a decimal comma (1.234,5 / 12,50).
            let comma = texts.iter().any(|t| {
                let t = t.trim().trim_end_matches('%').trim();
                match t.rfind(',') {
                    Some(i) => {
                        let tail = &t[i + 1..];
                        let digits = tail.chars().take_while(char::is_ascii_digit).count();
                        (digits == 1 || digits == 2) && digits == tail.len()
                            || (t[..i].contains('.') && digits == tail.len())
                    }
                    None => false,
                }
            });
            // A dot followed by three digits after a dot grouping (1.234.567).
            let dots = texts.iter().any(|t| t.matches('.').count() > 1);
            if comma || dots { ',' } else { '.' }
        }
        other => bail!("`decimal` must be \".\", \",\" or auto, not {other:?}"),
    })
}

/// A number written as text: currency words and signs, grouping, `(neg)`,
/// trailing minus, percent. `None` when it isn't one.
pub fn parse_number_text(text: &str, decimal: char) -> Option<(f64, bool)> {
    let mut t = text.trim().replace(['\u{a0}', '\u{202f}', ' '], "");
    if t.is_empty() {
        return None;
    }
    let mut neg = false;
    if t.starts_with('(') && t.ends_with(')') {
        neg = true;
        t = t[1..t.len() - 1].to_owned();
    }
    if let Some(x) = t.strip_suffix('-') {
        neg = !neg;
        t = x.to_owned();
    }
    if let Some(x) = t.strip_prefix('-') {
        neg = !neg;
        t = x.to_owned();
    } else if let Some(x) = t.strip_prefix('+') {
        t = x.to_owned();
    }
    let lower = t.to_lowercase();
    let mut body = lower.as_str();
    let mut rupiah = false;
    for cur in [
        "rp.", "rp", "idr", "usd", "us$", "eur", "sgd", "$", "€", "£", "¥",
    ] {
        if let Some(x) = body.strip_prefix(cur) {
            body = x;
            rupiah = matches!(cur, "rp." | "rp" | "idr");
            break;
        }
    }
    for cur in ["idr", "usd", "eur", "rp", "$", "€"] {
        if let Some(x) = body.strip_suffix(cur) {
            body = x;
            rupiah |= matches!(cur, "idr" | "rp");
            break;
        }
    }
    // Rupiah amounts group with dots and have no cents: Rp 12.000.
    let decimal = if rupiah
        && decimal == '.'
        && !body.contains(',')
        && body.split('.').skip(1).all(|g| g.len() == 3)
    {
        ','
    } else {
        decimal
    };
    if let Some(x) = body.strip_prefix('-') {
        neg = !neg;
        body = x;
    }
    let (body, percent) = match body.strip_suffix('%') {
        Some(b) => (b, true),
        None => (body, false),
    };
    if body.is_empty()
        || !body
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
    {
        return None;
    }
    if !body
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == decimal)
    {
        return None;
    }
    let group = if decimal == '.' { ',' } else { '.' };
    // Grouping must be in threes: 1,234,567 (not 1,2,3).
    let (int, frac) = match body.rfind(decimal) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    if frac.is_some_and(|f| f.contains([decimal, group])) {
        return None;
    }
    if int.contains(group) {
        let parts: Vec<&str> = int.split(group).collect();
        if parts[0].is_empty() || parts[0].len() > 3 || parts[1..].iter().any(|p| p.len() != 3) {
            return None;
        }
    }
    let plain = format!(
        "{}{}",
        int.replace(group, ""),
        frac.map(|f| format!(".{f}")).unwrap_or_default()
    );
    let mut n: f64 = plain.parse().ok()?;
    if percent {
        n /= 100.0;
    }
    if neg {
        n = -n;
    }
    n.is_finite().then_some((n, percent))
}

fn to_numbers(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let (sheet, r) = range_arg(book, o, "range")?;
    let r = used_part(book, sheet, r);
    let mut found = Vec::new();
    for row in r.top..=r.bottom {
        for c in r.left..=r.right {
            if let Some(s) = text_constant(book, sheet, row, c) {
                found.push((row, c, s));
            }
        }
    }
    let texts: Vec<String> = found.iter().map(|f| f.2.clone()).collect();
    let dec = decimal_of(&texts, opt_str(o, "decimal")?.unwrap_or("auto"))?;
    let mut cells = Vec::new();
    let mut percents = Vec::new();
    let mut skipped = Vec::new();
    for (row, c, s) in found {
        match parse_number_text(&s, dec) {
            Some((n, pct)) => {
                cells.push((row, c, super::sheets_author::number(n).to_string()));
                if pct {
                    percents.push(CellRange::from_corners((row, c), (row, c)));
                }
            }
            None => {
                if skipped.len() < 20 {
                    skipped.push(json!({"cell": cell_name(row, c), "text": s}));
                }
            }
        }
    }
    let n = cells.len();
    book.set_inputs(sheet, &cells)?;
    if let Some(f) = opt_str(o, "number_format")? {
        let code = super::sheets_author::number_format_code(f);
        for (row, c, _) in &cells {
            book.set_range_style_value(
                sheet,
                CellRange::from_corners((*row, *c), (*row, *c)),
                "num_fmt",
                &code,
            )?;
        }
    } else {
        for p in percents {
            book.set_range_style_value(sheet, p, "num_fmt", "0%")?;
        }
    }
    let mut out = json!({"converted": n, "decimal": dec.to_string()});
    if !skipped.is_empty() {
        out["left_as_text"] = json!(skipped);
    }
    Ok((format!("converted {n} cell(s) to numbers"), out))
}

const MONTHS: [(&str, u32); 31] = [
    ("january", 1),
    ("januari", 1),
    ("february", 2),
    ("februari", 2),
    ("pebruari", 2),
    ("march", 3),
    ("maret", 3),
    ("april", 4),
    ("may", 5),
    ("mei", 5),
    ("june", 6),
    ("juni", 6),
    ("july", 7),
    ("juli", 7),
    ("august", 8),
    ("agustus", 8),
    ("september", 9),
    ("october", 10),
    ("oktober", 10),
    ("november", 11),
    ("nopember", 11),
    ("december", 12),
    ("desember", 12),
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("agu", 8),
];
const MONTHS_SHORT: [(&str, u32); 6] = [
    ("sep", 9),
    ("sept", 9),
    ("oct", 10),
    ("okt", 10),
    ("nov", 11),
    ("dec", 12),
];

fn month_word(w: &str) -> Option<u32> {
    let w = w.trim_end_matches('.').to_lowercase();
    if w == "des" {
        return Some(12);
    }
    MONTHS
        .iter()
        .chain(MONTHS_SHORT.iter())
        .find(|(n, _)| *n == w)
        .map(|(_, m)| *m)
}

/// The order of day, month and year in numeric dates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateOrder {
    Dmy,
    Mdy,
    Ymd,
}

/// The parts of a date written as text (`(a, b, c)` numbers or a month
/// word), and a time of day.
fn date_parts(text: &str) -> Option<(Vec<String>, f64)> {
    let t = text.trim();
    // A time after the date: `2026-01-15 13:45(:00)`, `...T13:45`.
    let (date, time) = match t.split_once(['T', ' ']).filter(|(_, b)| b.contains(':')) {
        Some((d, tm)) => (d, Some(tm)),
        None => (t, None),
    };
    let secs = match time {
        Some(tm) => {
            let p: Vec<&str> = tm.trim_end_matches('Z').split(':').collect();
            let h: f64 = p.first()?.parse().ok()?;
            let m: f64 = p.get(1)?.parse().ok()?;
            let s: f64 = p.get(2).map_or(Some(0.0), |s| s.parse().ok())?;
            if !(0.0..24.0).contains(&h) || !(0.0..60.0).contains(&m) || !(0.0..60.0).contains(&s) {
                return None;
            }
            (h * 3600.0 + m * 60.0 + s) / 86_400.0
        }
        None => 0.0,
    };
    let parts: Vec<String> = date
        .split(['/', '-', '.', ' ', ','])
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
        .collect();
    (parts.len() == 3).then_some((parts, secs))
}

/// Year, month, day of a date written as text.
fn ymd_of(text: &str, order: DateOrder) -> Option<(i32, u32, u32, f64)> {
    let (p, secs) = date_parts(text)?;
    let num = |s: &str| s.parse::<u32>().ok();
    let year = |s: &str| -> Option<i32> {
        let y: i32 = s.parse().ok()?;
        Some(match s.len() {
            2 => {
                if y < 50 {
                    2000 + y
                } else {
                    1900 + y
                }
            }
            4 => y,
            _ => return None,
        })
    };
    let (y, m, d) = if let Some(m) = month_word(&p[1]) {
        // 15 Jan 2026, 15-Jan-26
        (year(&p[2])?, m, num(&p[0])?)
    } else if let Some(m) = month_word(&p[0]) {
        // Jan 15 2026
        (year(&p[2])?, m, num(&p[1])?)
    } else if p[0].len() == 4 {
        (year(&p[0])?, num(&p[1])?, num(&p[2])?)
    } else {
        match order {
            DateOrder::Dmy => (year(&p[2])?, num(&p[1])?, num(&p[0])?),
            DateOrder::Mdy => (year(&p[2])?, num(&p[0])?, num(&p[1])?),
            DateOrder::Ymd => (year(&p[0])?, num(&p[1])?, num(&p[2])?),
        }
    };
    let days = [
        31,
        if y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
            29
        } else {
            28
        },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    ((1..=12).contains(&m) && d >= 1 && d <= days[m as usize - 1] && (1900..=9999).contains(&y))
        .then_some((y, m, d, secs))
}

/// Excel's serial number of a date (1900 system).
pub fn serial(y: i32, m: u32, d: u32) -> f64 {
    // Days from 1899-12-30 (civil-from-days, Howard Hinnant's algorithm).
    let (y, m) = if m <= 2 { (y - 1, m + 9) } else { (y, m - 3) };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m as i32 + 2) / 5 + d as i32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era as i64 * 146_097 + doe as i64 - 719_468;
    // 1970-01-01 is serial 25569.
    (days + 25_569) as f64
}

/// The order `order` asks for, or the one the texts show.
fn order_of(texts: &[String], order: &str) -> Result<DateOrder> {
    Ok(match order {
        "dmy" => DateOrder::Dmy,
        "mdy" => DateOrder::Mdy,
        "ymd" => DateOrder::Ymd,
        "auto" => {
            let mut dmy = false;
            let mut mdy = false;
            for t in texts {
                if let Some((p, _)) = date_parts(t)
                    && p[0].len() <= 2
                    && let (Ok(a), Ok(b)) = (p[0].parse::<u32>(), p[1].parse::<u32>())
                {
                    dmy |= a > 12 && b <= 12;
                    mdy |= b > 12 && a <= 12;
                }
            }
            if mdy && !dmy {
                DateOrder::Mdy
            } else {
                DateOrder::Dmy
            }
        }
        other => bail!("`order` must be dmy, mdy, ymd or auto, not {other:?}"),
    })
}

/// A date written as text, as a serial number (`order` for 01/02/2026).
pub fn parse_date_text(text: &str, order: DateOrder) -> Option<f64> {
    let t = text.trim();
    if t.len() < 6 || t.len() > 40 || !t.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let (y, m, d, secs) = ymd_of(t, order)?;
    Some(serial(y, m, d) + secs)
}

fn to_dates(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let (sheet, r) = range_arg(book, o, "range")?;
    let r = used_part(book, sheet, r);
    let mut found = Vec::new();
    for row in r.top..=r.bottom {
        for c in r.left..=r.right {
            if let Some(s) = text_constant(book, sheet, row, c) {
                found.push((row, c, s));
            }
        }
    }
    let texts: Vec<String> = found.iter().map(|f| f.2.clone()).collect();
    let order = order_of(&texts, opt_str(o, "order")?.unwrap_or("auto"))?;
    let code =
        super::sheets_author::number_format_code(opt_str(o, "number_format")?.unwrap_or("date"));
    let mut cells = Vec::new();
    let mut with_time = false;
    let mut skipped = Vec::new();
    for (row, c, s) in found {
        match parse_date_text(&s, order) {
            Some(n) => {
                with_time |= n.fract() != 0.0;
                cells.push((row, c, super::sheets_author::number(n).to_string()));
            }
            None if skipped.len() < 20 => {
                skipped.push(json!({"cell": cell_name(row, c), "text": s}));
            }
            None => {}
        }
    }
    let n = cells.len();
    book.set_inputs(sheet, &cells)?;
    let code = if with_time && opt_str(o, "number_format")?.is_none() {
        "yyyy-mm-dd hh:mm".to_owned()
    } else {
        code
    };
    for (row, c, _) in &cells {
        book.set_range_style_value(
            sheet,
            CellRange::from_corners((*row, *c), (*row, *c)),
            "num_fmt",
            &code,
        )?;
    }
    let mut out = json!({"converted": n, "order": format!("{order:?}").to_lowercase()});
    if !skipped.is_empty() {
        out["left_as_text"] = json!(skipped);
    }
    Ok((format!("converted {n} cell(s) to dates"), out))
}

fn split(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let (sheet, r) = range_arg(book, o, "range")?;
    let r = used_part(book, sheet, r);
    if r.left != r.right {
        bail!("split_column takes a one-column range");
    }
    let delim = req_str(o, "delimiter")?;
    if delim.is_empty() {
        bail!("`delimiter` can't be empty");
    }
    let to = match opt_str(o, "to")? {
        Some(t) => parse_ref(book, t, sheet)?.1,
        None => CellRange::from_corners((r.top, r.left + 1), (r.top, r.left + 1)),
    };
    let overwrite = opt_bool(o, "overwrite")?.unwrap_or(false);
    let mut cells = Vec::new();
    let mut width = 0;
    for (i, row) in (r.top..=r.bottom).enumerate() {
        let s = book.display_value(sheet, row, r.left);
        if s.is_empty() {
            continue;
        }
        let parts: Vec<&str> = s.split(delim).map(str::trim).collect();
        width = width.max(parts.len());
        for (k, p) in parts.iter().enumerate() {
            cells.push((to.top + i as u32, to.left + k as u32, (*p).to_owned()));
        }
    }
    if !overwrite
        && let Some((rr, cc, _)) = cells.iter().find(|(rr, cc, _)| {
            (*rr, *cc) != (*rr, r.left) && !book.input(sheet, *rr, *cc).is_empty()
        })
    {
        bail!(
            "{} is not empty; give `to` (empty columns) or overwrite: true",
            cell_name(*rr, *cc)
        );
    }
    book.set_inputs(sheet, &cells)?;
    let out = CellRange::from_corners(
        (to.top, to.left),
        (
            to.top + r.rows_count() - 1,
            to.left + width.max(1) as u32 - 1,
        ),
    );
    Ok((
        format!("split {} into {}", r.name(), out.name()),
        json!({"range": out.name(), "columns": width}),
    ))
}

fn combine(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let (sheet, r) = range_arg(book, o, "range")?;
    let r = used_part(book, sheet, r);
    let to = parse_ref(book, req_str(o, "to")?, sheet)?.1;
    let sep = opt_str(o, "separator")?.unwrap_or(" ");
    let formulas = opt_bool(o, "formulas")?.unwrap_or(true);
    let mut cells = Vec::new();
    for (i, row) in (r.top..=r.bottom).enumerate() {
        let text = if formulas {
            let q = format!("\"{}\"", sep.replace('"', "\"\""));
            let parts: Vec<String> = (r.left..=r.right)
                .map(|c| format!("{}{}", column_name(c), row + 1))
                .collect();
            if sep.is_empty() {
                format!("={}", parts.join("&"))
            } else {
                format!("={}", parts.join(&format!("&{q}&")))
            }
        } else {
            let parts: Vec<String> = (r.left..=r.right)
                .map(|c| book.display_value(sheet, row, c))
                .filter(|s| !s.is_empty())
                .collect();
            as_text_input(&parts.join(sep))
        };
        cells.push((to.top + i as u32, to.left, text));
    }
    book.set_inputs(sheet, &cells)?;
    let out = CellRange::from_corners((to.top, to.left), (to.top + r.rows_count() - 1, to.left));
    Ok((
        format!("combined {} into {}", r.name(), out.name()),
        json!({"range": out.name()}),
    ))
}

fn find_replace(book: &mut Workbook, o: &Map<String, Value>) -> Result<(String, Value)> {
    let find = req_str(o, "find")?;
    let replace = req_str(o, "replace")?;
    if find.is_empty() {
        bail!("`find` can't be empty");
    }
    let case = opt_bool(o, "match_case")?.unwrap_or(false);
    let whole = opt_bool(o, "whole_cell")?.unwrap_or(false);
    let in_formulas = opt_bool(o, "in_formulas")?.unwrap_or(false);
    let targets: Vec<(u32, CellRange)> = if opt_bool(o, "all_sheets")? == Some(true) {
        (0..book.sheet_names().len() as u32)
            .map(|s| (s, used_part(book, s, CellRange::all())))
            .collect()
    } else if get(o, "range").is_some() {
        let (s, r) = range_arg(book, o, "range")?;
        vec![(s, used_part(book, s, r))]
    } else {
        let s = sheet_arg(book, o)?;
        vec![(s, used_part(book, s, CellRange::all()))]
    };
    let lower_find = find.to_lowercase();
    let mut total = 0;
    let mut cells_changed = Vec::new();
    for (sheet, r) in targets {
        let mut cells = Vec::new();
        for (row, c) in book.filled_cells(sheet) {
            if !r.contains(row, c) {
                continue;
            }
            let formula = is_formula(book, sheet, row, c);
            if formula && !in_formulas {
                continue;
            }
            let text = if formula {
                book.input(sheet, row, c)
            } else if book.value_kind(sheet, row, c) == ValueKind::Text {
                book.display_value(sheet, row, c)
            } else {
                book.input(sheet, row, c)
            };
            let (new, n) = if whole {
                let hit = if case {
                    text == find
                } else {
                    text.to_lowercase() == lower_find
                };
                if hit {
                    (replace.to_owned(), 1)
                } else {
                    (text.clone(), 0)
                }
            } else if case {
                (text.replace(find, replace), text.matches(find).count())
            } else {
                replace_ci(&text, &lower_find, replace)
            };
            if n > 0 && new != text {
                total += n;
                let input = if formula || book.value_kind(sheet, row, c) != ValueKind::Text {
                    new
                } else {
                    as_text_input(&new)
                };
                cells.push((row, c, input));
                if cells_changed.len() < 50 {
                    cells_changed.push(json!(format!(
                        "{}!{}",
                        super::sheets_author::quoted(book, sheet),
                        cell_name(row, c)
                    )));
                }
            }
        }
        book.set_inputs(sheet, &cells)?;
    }
    Ok((
        format!("replaced {total} match(es)"),
        json!({"replaced": total, "cells": cells_changed}),
    ))
}

/// `text` with every case-insensitive `find` (lower case) replaced.
fn replace_ci(text: &str, find: &str, replace: &str) -> (String, usize) {
    let lower = text.to_lowercase();
    if lower.len() != text.len() {
        // Lower-casing changed byte lengths: fall back to an exact match.
        return (text.replace(find, replace), text.matches(find).count());
    }
    let mut out = String::new();
    let mut n = 0;
    let mut i = 0;
    while let Some(k) = lower[i..].find(find) {
        out.push_str(&text[i..i + k]);
        out.push_str(replace);
        i += k + find.len();
        n += 1;
    }
    out.push_str(&text[i..]);
    (out, n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::sheets_author::{cell_value, run_tool};

    fn run_ok(b: &mut Workbook, tool: &str, args: Value) -> Value {
        run_tool(b, tool, &args, None)
            .unwrap_or_else(|e| panic!("{tool}: {e:#}"))
            .1
    }

    fn messy() -> Workbook {
        let mut b = Workbook::new_empty().unwrap();
        let rows = json!([
            ["Name", "Amount", "Date", "City"],
            ["  Ani  Rahma ", "'1,250.50", "'15/01/2026", "Jakarta"],
            ["Budi", "'Rp 12.000", "'2026-02-03", "Bandung"],
            ["ani rahma", "'1,250.50", "'15/01/2026", "jakarta"],
            ["Citra", "'(500)", "'3 Maret 2026", "Surabaya"],
            ["Dodi", "'15%", "'Jan 5, 2026", "Medan"],
        ]);
        run_ok(
            &mut b,
            "write_range",
            json!({"top_left": "A1", "values": rows}),
        );
        b
    }

    #[std::prelude::v1::test]
    fn numbers_from_text() {
        for (t, d, want) in [
            ("1,250.50", '.', Some(1250.5)),
            ("1.250,50", ',', Some(1250.5)),
            ("Rp 12.000", ',', Some(12000.0)),
            ("Rp12,000", '.', Some(12000.0)),
            ("(500)", '.', Some(-500.0)),
            ("500-", '.', Some(-500.0)),
            ("-$3.5", '.', Some(-3.5)),
            ("15%", '.', Some(0.15)),
            ("1,2,3", '.', None),
            ("abc", '.', None),
            ("12 kg", '.', None),
            ("2026-01-15", '.', None),
        ] {
            assert_eq!(parse_number_text(t, d).map(|x| x.0), want, "{t}");
        }
        assert_eq!(decimal_of(&["1.234,5".into()], "auto").unwrap(), ',');
        assert_eq!(decimal_of(&["1,234.5".into()], "auto").unwrap(), '.');
        assert_eq!(decimal_of(&["12.000.000".into()], "auto").unwrap(), ',');
    }

    #[std::prelude::v1::test]
    fn dates_from_text() {
        assert_eq!(serial(2026, 1, 15), 46037.0);
        assert_eq!(serial(1900, 3, 1), 61.0);
        let d = DateOrder::Dmy;
        assert_eq!(parse_date_text("15/01/2026", d), Some(46037.0));
        assert_eq!(parse_date_text("2026-01-15", d), Some(46037.0));
        assert_eq!(parse_date_text("15 Januari 2026", d), Some(46037.0));
        assert_eq!(parse_date_text("Jan 15, 2026", d), Some(46037.0));
        assert_eq!(parse_date_text("15-Jan-26", d), Some(46037.0));
        assert_eq!(parse_date_text("01/15/2026", DateOrder::Mdy), Some(46037.0));
        assert_eq!(parse_date_text("2026-01-15 12:00", d), Some(46037.5));
        assert_eq!(parse_date_text("31/02/2026", d), None);
        assert_eq!(parse_date_text("hello", d), None);
        assert_eq!(
            order_of(&["01/15/2026".into()], "auto").unwrap(),
            DateOrder::Mdy
        );
        assert_eq!(
            order_of(&["15/01/2026".into()], "auto").unwrap(),
            DateOrder::Dmy
        );
    }

    #[std::prelude::v1::test]
    fn cleans_a_messy_table() {
        let mut b = messy();
        let out = run_ok(&mut b, "trim", json!({"range": "A1:D6"}));
        assert_eq!(out["changed"], 1);
        assert_eq!(b.display_value(0, 1, 0), "Ani Rahma");
        let out = run_ok(
            &mut b,
            "to_numbers",
            json!({"range": "B2:B6", "decimal": "auto"}),
        );
        // Rupiah group with dots whatever the decimal separator.
        assert_eq!(out["converted"], 5, "{out}");
        assert_eq!(cell_value(&b, 0, 1, 1), json!(1250.5));
        assert_eq!(cell_value(&b, 0, 2, 1), json!(12000));
        assert_eq!(cell_value(&b, 0, 4, 1), json!(-500));
        assert_eq!(cell_value(&b, 0, 5, 1), json!(0.15));
        assert_eq!(b.cell_style(0, 5, 1).num_fmt, "0%");
        let out = run_ok(&mut b, "to_dates", json!({"range": "C2:C6"}));
        assert_eq!(out["converted"], 5, "{out}");
        assert_eq!(cell_value(&b, 0, 1, 2), json!(46037));
        assert_eq!(b.cell_style(0, 1, 2).num_fmt, "yyyy-mm-dd");
        let out = run_ok(&mut b, "remove_duplicates", json!({"range": "A1:D6"}));
        assert_eq!(out["removed"], 1, "{out}");
        assert_eq!(b.display_value(0, 3, 0), "Citra");
        run_ok(
            &mut b,
            "sort",
            json!({"range": "A1:D5", "by": [{"column": "B", "order": "desc"}]}),
        );
        assert_eq!(b.display_value(0, 1, 0), "Budi");
        assert_eq!(b.display_value(0, 0, 0), "Name");
        let out = run_ok(
            &mut b,
            "find_replace",
            json!({"find": "jakarta", "replace": "Jakarta Pusat", "whole_cell": true}),
        );
        assert_eq!(out["replaced"], 1);
        run_ok(
            &mut b,
            "split_column",
            json!({"range": "A2:A5", "delimiter": " ", "to": "F2"}),
        );
        assert_eq!(b.display_value(0, 2, 6), "Rahma");
        run_ok(
            &mut b,
            "combine_columns",
            json!({"range": "F2:G5", "to": "H2", "separator": " "}),
        );
        assert_eq!(b.input(0, 1, 7), "=F2&\" \"&G2");
        assert!(
            run_tool(
                &mut b,
                "split_column",
                &json!({"range": "A2:A5", "delimiter": " ", "to": "F2"}),
                None
            )
            .is_err()
        );
    }

    #[std::prelude::v1::test]
    fn text_that_looks_like_numbers_stays_text() {
        let mut b = Workbook::new_empty().unwrap();
        run_ok(
            &mut b,
            "write_range",
            json!({"top_left": "A1", "values": [["' 00123 "]]}),
        );
        run_ok(&mut b, "trim", json!({"range": "A1"}));
        assert_eq!(b.display_value(0, 0, 0), "00123");
        assert_eq!(b.value_kind(0, 0, 0), ValueKind::Text);
        assert_eq!(tidy("a\u{a0}\u{a0}b\t c", true), "a b c");
        assert_eq!(replace_ci("Hello hello", "hello", "x"), ("x x".into(), 2));
    }
}
