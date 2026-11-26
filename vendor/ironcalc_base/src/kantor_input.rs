//! KANTOR PATCH (typed times): what `set_user_input` stores for a typed
//! time or date and time, as Excel does.
//!
//! * `9:30`, `9:30 PM`, `14:05:30`, `9 pm`, `25:30`, `1:30.5` become the
//!   time's serial number with `h:mm`, `h:mm AM/PM`, `h:mm:ss`,
//!   `[h]:mm:ss` or `mm:ss.0`.
//! * A date followed by a time (`11/16/2026 9:30`, `2026-11-16 14:05`)
//!   becomes the date's serial plus the time, with the date's format and
//!   the time's.
//! * The order of day and month follows the input locale the app sets once
//!   ([`set_input_locale`]): `"id"` reads `16/11/2026` day first and knows
//!   the Indonesian month names (`16-Agu-2026`, `1-Mei-26`); otherwise the
//!   workbook's locale decides, as before.
//! * Dates written with a month name, in any case, as Excel reads them:
//!   `15 Desember 2026`, `15-des-26`, `December 15, 2026`, `15 Dec`
//!   (this year), `Dec 2026` (the 1st). English names always, Indonesian
//!   ones in the Indonesian input locale. The format follows what was
//!   typed (`d mmmm yyyy`, `mmmm d, yyyy`, `d mmm`, `mmm yyyy`).

use std::sync::atomic::{AtomicU8, Ordering};

use crate::formatter::dates::{date_to_serial_number, from_excel_date};
use crate::formatter::format::{parse_date, parse_formatted_number};
use crate::locale::get_locale;
use crate::Model;

static INPUT_LOCALE: AtomicU8 = AtomicU8::new(0);

/// How typed dates are read: `"id"` (Indonesian: day first, Indonesian
/// month names) or anything else (the workbook's locale).
pub fn set_input_locale(locale: &str) {
    let v = if locale.eq_ignore_ascii_case("id") || locale.to_ascii_lowercase().starts_with("id-") {
        1
    } else {
        0
    };
    INPUT_LOCALE.store(v, Ordering::Relaxed);
}

fn indonesian() -> bool {
    INPUT_LOCALE.load(Ordering::Relaxed) == 1
}

/// A typed time: the fraction of a day (more than 1 for `25:30`) and its
/// format.
fn parse_time(text: &str) -> Option<(f64, String)> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    // AM/PM suffix, with or without a space: "9:30 PM", "9:30pm", "9 a".
    let lower = t.to_ascii_lowercase();
    let (body, pm) = if let Some(b) = lower.strip_suffix("am").or_else(|| lower.strip_suffix('a')) {
        (b.trim_end(), Some(false))
    } else if let Some(b) = lower.strip_suffix("pm").or_else(|| lower.strip_suffix('p')) {
        (b.trim_end(), Some(true))
    } else {
        (lower.as_str(), None)
    };
    if body.is_empty() {
        return None;
    }
    let parts: Vec<&str> = body.split(':').collect();
    if parts.len() > 3 || (parts.len() == 1 && pm.is_none()) {
        return None;
    }
    let digits = |s: &str| !s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit());
    // Minutes and seconds with a fraction: "1:30.5" (Excel's mm:ss.0).
    if parts.len() == 2 && pm.is_none() && parts[1].contains('.') {
        let (s, frac) = parts[1].split_once('.')?;
        if !digits(parts[0])
            || s.len() != 2
            || !digits(s)
            || !frac.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        let m: f64 = parts[0].parse().ok()?;
        let sec: f64 = format!("{s}.{frac}0").parse().ok()?;
        if sec >= 60.0 {
            return None;
        }
        return Some(((m * 60.0 + sec) / 86_400.0, "mm:ss.0".to_string()));
    }
    if !parts.iter().all(|p| digits(p)) {
        return None;
    }
    let h: u32 = parts[0].parse().ok()?;
    let m: u32 = parts.get(1).map_or(Some(0), |p| p.parse().ok())?;
    let s: u32 = parts.get(2).map_or(Some(0), |p| p.parse().ok())?;
    // Minutes and seconds are two digits ("9:5" is not a time in Excel).
    if parts[1..].iter().any(|p| p.len() != 2) || m > 59 || s > 59 {
        return None;
    }
    let hours = match pm {
        Some(pm) => {
            if h > 12 {
                return None;
            }
            match (h, pm) {
                (12, false) => 0,
                (12, true) => 12,
                (h, true) => h + 12,
                (h, false) => h,
            }
        }
        None => h,
    };
    if hours > 9999 {
        return None;
    }
    let value = (hours as f64 * 3600.0 + m as f64 * 60.0 + s as f64) / 86_400.0;
    let format = if pm.is_some() {
        match parts.len() {
            1 => "h AM/PM",
            2 => "h:mm AM/PM",
            _ => "h:mm:ss AM/PM",
        }
    } else if hours >= 24 {
        "[h]:mm:ss"
    } else if parts.len() == 3 {
        "h:mm:ss"
    } else {
        "h:mm"
    };
    Some((value, format.to_string()))
}

/// Indonesian month names: the name, the month (0-based) and whether it is
/// the long name.
const ID_MONTHS: [(&str, usize, bool); 26] = [
    ("januari", 0, true),
    ("februari", 1, true),
    ("maret", 2, true),
    ("april", 3, true),
    ("mei", 4, true),
    ("juni", 5, true),
    ("juli", 6, true),
    ("agustus", 7, true),
    ("september", 8, true),
    ("oktober", 9, true),
    ("november", 10, true),
    ("desember", 11, true),
    ("jan", 0, false),
    ("feb", 1, false),
    ("mar", 2, false),
    ("apr", 3, false),
    ("jun", 5, false),
    ("jul", 6, false),
    ("agu", 7, false),
    ("ags", 7, false),
    ("agt", 7, false),
    ("sep", 8, false),
    ("sept", 8, false),
    ("okt", 9, false),
    ("nov", 10, false),
    ("des", 11, false),
];

/// English month names, as Excel reads them in any case.
const EN_MONTHS: [(&str, usize, bool); 24] = [
    ("january", 0, true),
    ("february", 1, true),
    ("march", 2, true),
    ("april", 3, true),
    ("may", 4, true),
    ("june", 5, true),
    ("july", 6, true),
    ("august", 7, true),
    ("september", 8, true),
    ("october", 9, true),
    ("november", 10, true),
    ("december", 11, true),
    ("jan", 0, false),
    ("feb", 1, false),
    ("mar", 2, false),
    ("apr", 3, false),
    ("jun", 5, false),
    ("jul", 6, false),
    ("aug", 7, false),
    ("sep", 8, false),
    ("sept", 8, false),
    ("oct", 9, false),
    ("nov", 10, false),
    ("dec", 11, false),
];

/// A month name or abbreviation (`Desember`, `des`, `Sept.`): the month
/// (1-based) and whether it is the long name.
fn month_name(word: &str, indonesian: bool) -> Option<(u32, bool)> {
    let w = word.strip_suffix('.').unwrap_or(word).to_lowercase();
    let id = ID_MONTHS.iter().filter(|_| indonesian);
    EN_MONTHS
        .iter()
        .chain(id)
        .find(|(n, _, _)| *n == w)
        .map(|(_, m, long)| (*m as u32 + 1, *long))
}

/// A date written with a month name and numbers: `15 Desember 2026`,
/// `15-des-26`, `December 15, 2026`, `Dec 15`, `15 Dec` (both in
/// `this_year`), `Dec 2026` (the 1st). Words are separated by spaces, `-`,
/// `/` or `,`; anything else (`Mei Ling`, `Mar 2026 report`) is not a
/// date. Day, month, year and the format that shows them as typed.
fn parse_month_name_date(
    text: &str,
    indonesian: bool,
    this_year: impl FnOnce() -> Option<i32>,
) -> Option<(u32, u32, i32, String)> {
    let text = text.trim();
    // Words and the separators between them.
    let mut words: Vec<&str> = Vec::new();
    let mut seps: Vec<String> = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let end = rest
            .find(|c: char| !(c.is_alphanumeric() || c == '.'))
            .unwrap_or(rest.len());
        if end == 0 {
            return None;
        }
        words.push(&rest[..end]);
        rest = &rest[end..];
        if rest.is_empty() {
            break;
        }
        let end = rest
            .find(|c: char| !(c.is_whitespace() || matches!(c, '-' | '/' | ',')))
            .unwrap_or(rest.len());
        let sep = rest[..end].trim();
        let sep = match sep {
            "" => " ",
            "," => ", ",
            "-" | "/" => sep,
            _ => return None,
        };
        if end == rest.len() {
            return None;
        }
        seps.push(sep.to_string());
        rest = &rest[end..];
    }
    if !(2..=3).contains(&words.len()) {
        return None;
    }
    let digits =
        |w: &str, lens: &[usize]| lens.contains(&w.len()) && w.bytes().all(|b| b.is_ascii_digit());
    let mut months = words
        .iter()
        .enumerate()
        .filter_map(|(i, w)| month_name(w, indonesian).map(|m| (i, m)));
    let (at, (month, long)) = months.next()?;
    if months.next().is_some() {
        return None;
    }
    let m_fmt = if long { "mmmm" } else { "mmm" };
    let day = |w: &str| {
        digits(w, &[1, 2]).then(|| {
            let d: u32 = w.parse().unwrap_or(0);
            // "05" keeps its zero; "15" is just the day.
            (d, if w.starts_with('0') { "dd" } else { "d" })
        })
    };
    let year = |w: &str| {
        digits(w, &[2, 4]).then(|| {
            let y: i32 = w.parse().unwrap_or(0);
            let y = match (w.len(), y) {
                (2, y) if y < 30 => 2000 + y,
                (2, y) => 1900 + y,
                (_, y) => y,
            };
            (y, if w.len() == 2 { "yy" } else { "yyyy" })
        })
    };
    let (d, m, y, format) = match (words.len(), at) {
        // 15 Desember 2026
        (3, 1) => {
            let (d, df) = day(words[0])?;
            let (y, yf) = year(words[2])?;
            (
                d,
                month,
                y,
                format!("{df}{}{m_fmt}{}{yf}", seps[0], seps[1]),
            )
        }
        // December 15, 2026
        (3, 0) => {
            let (d, df) = day(words[1])?;
            let (y, yf) = year(words[2])?;
            (
                d,
                month,
                y,
                format!("{m_fmt}{}{df}{}{yf}", seps[0], seps[1]),
            )
        }
        // 15 Des
        (2, 1) => {
            let (d, df) = day(words[0])?;
            (d, month, this_year()?, format!("{df}{}{m_fmt}", seps[0]))
        }
        // Dec 15, Dec 2026
        (2, 0) => {
            if let Some((d, df)) = day(words[1]) {
                (d, month, this_year()?, format!("{m_fmt}{}{df}", seps[0]))
            } else if digits(words[1], &[4]) {
                let (y, yf) = year(words[1])?;
                (1, month, y, format!("{m_fmt}{}{yf}", seps[0]))
            } else {
                return None;
            }
        }
        _ => return None,
    };
    Some((d, m, y, format))
}

impl Model<'_> {
    /// This year in the model's timezone, for dates typed without one.
    fn kantor_this_year(&self) -> Option<i32> {
        use chrono::Datelike;
        let serial = crate::tz::excel_serial_for_now(&self.tz)?;
        from_excel_date(serial.floor() as i64)
            .ok()
            .map(|d| d.year())
    }

    /// A date typed with a month name: serial and format.
    fn kantor_month_name_date(&self, text: &str, indonesian: bool) -> Option<(i32, String)> {
        let (d, m, y, format) =
            parse_month_name_date(text, indonesian, || self.kantor_this_year())?;
        Some((date_to_serial_number(d, m, y).ok()?, format))
    }

    /// A typed date in the input locale: serial and format.
    fn kantor_parse_date(&self, text: &str, indonesian: bool) -> Option<(i32, String)> {
        self.kantor_parse_numeric_date(text, indonesian)
            .or_else(|| self.kantor_month_name_date(text, indonesian))
    }

    /// [`Model::kantor_parse_date`] for the forms IronCalc reads
    /// (`16/11/2026`, `16-Agu-2026`).
    fn kantor_parse_numeric_date(&self, text: &str, indonesian: bool) -> Option<(i32, String)> {
        if !indonesian {
            return parse_date(text, self.locale).ok();
        }
        // Day first, with the Indonesian month names turned into English
        // ones for the en-GB reader.
        let gb = get_locale("en-GB").ok()?;
        let sep = ['/', '-', '.'].into_iter().find(|c| text.contains(*c))?;
        let parts: Vec<String> = text
            .split(sep)
            .map(|p| {
                let l = p.to_lowercase();
                match ID_MONTHS.iter().find(|(id, _, _)| *id == l) {
                    Some((_, m, true)) => gb.dates.months[*m].clone(),
                    Some((_, m, false)) => gb.dates.months_short[*m].clone(),
                    None => p.to_string(),
                }
            })
            .collect();
        parse_date(&parts.join(&sep.to_string()), gb).ok()
    }

    /// What `set_user_input` stores for a typed value that isn't a formula,
    /// text, boolean or error: the number and its format.
    pub(crate) fn kantor_parse_input(
        &self,
        value: &str,
        currencies: &[&str],
    ) -> Result<(f64, Option<String>), String> {
        self.kantor_parse_input_in(value, currencies, indonesian())
    }

    /// [`Model::kantor_parse_input`] with the input locale given.
    pub(crate) fn kantor_parse_input_in(
        &self,
        value: &str,
        currencies: &[&str],
        indonesian: bool,
    ) -> Result<(f64, Option<String>), String> {
        let trimmed = value.trim();
        if trimmed.contains(':')
            || trimmed.to_ascii_lowercase().ends_with('m')
            || trimmed.to_ascii_lowercase().ends_with(" a")
            || trimmed.to_ascii_lowercase().ends_with(" p")
        {
            if let Some((v, f)) = parse_time(trimmed) {
                return Ok((v, Some(f)));
            }
            // A date, then a time: "16/11/2026 9:30", "15 Desember 2026
            // 14:05", "Dec 15, 2026 2:05 PM".
            let splits = trimmed
                .char_indices()
                .filter(|(_, c)| c.is_whitespace())
                .map(|(i, _)| (&trimmed[..i], &trimmed[i..]));
            for (date, time) in splits {
                let Some((t, tf)) = parse_time(time) else {
                    continue;
                };
                if let Some((d, df)) = self.kantor_parse_date(date.trim_end(), indonesian) {
                    if t < 1.0 && tf != "mm:ss.0" {
                        return Ok((d as f64 + t, Some(format!("{df} {tf}"))));
                    }
                }
            }
        }
        if indonesian {
            if let Some((d, f)) = self.kantor_parse_numeric_date(value, indonesian) {
                return Ok((d as f64, Some(f)));
            }
        }
        parse_formatted_number(value, currencies, self.locale).or_else(|e| {
            self.kantor_month_name_date(value, indonesian)
                .map(|(d, f)| (d as f64, Some(f)))
                .ok_or(e)
        })
    }
}

/// A format code without its locale-only tags (`[$-421]d mmmm yyyy` →
/// `d mmmm yyyy`), which IronCalc's lexer can't read: a date typed into a
/// cell with such a date format keeps the format.
pub(crate) fn without_locale_tags(format: &str) -> String {
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(i) = rest.find("[$-") {
        out.push_str(&rest[..i]);
        match rest[i..].find(']') {
            Some(end) => rest = &rest[i + end + 1..],
            None => {
                rest = &rest[i..];
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether a number format shows the time of day (`h`, `s`, AM/PM), as
/// opposed to a date only.
pub(crate) fn format_shows_time(format: &str) -> bool {
    let mut quoted = false;
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => quoted = !quoted,
            '\\' => {
                chars.next();
            }
            _ if quoted => {}
            'h' | 'H' | 's' | 'S' => return true,
            _ => {}
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        let t = |s: &str| parse_time(s).map(|(v, f)| ((v * 86_400.0).round() as i64, f));
        assert_eq!(t("9:30"), Some((34_200, "h:mm".into())));
        assert_eq!(t("09:30"), Some((34_200, "h:mm".into())));
        assert_eq!(t("9:30 PM"), Some((77_400, "h:mm AM/PM".into())));
        assert_eq!(t("9:30pm"), Some((77_400, "h:mm AM/PM".into())));
        assert_eq!(t("12:15 am"), Some((900, "h:mm AM/PM".into())));
        assert_eq!(t("14:05:30"), Some((50_730, "h:mm:ss".into())));
        assert_eq!(t("9 pm"), Some((75_600, "h AM/PM".into())));
        assert_eq!(t("25:30"), Some((91_800, "[h]:mm:ss".into())));
        assert_eq!(t("1:30.5").map(|x| x.1), Some("mm:ss.0".into()));
        assert_eq!(t("9:75"), None);
        assert_eq!(t("9:5"), None);
        assert_eq!(t("13:00 PM"), None);
        assert_eq!(t("9"), None);
        assert_eq!(t("a:30"), None);
        assert_eq!(t("1:2:3:4"), None);
        assert_eq!(t("spam"), None);
    }

    #[test]
    fn month_name_dates() {
        let p = |s: &str, id: bool| parse_month_name_date(s, id, || Some(2026));
        let ok = |d, m, y, f: &str| Some((d, m, y, f.to_string()));
        // Indonesian, full and short, any case and separator.
        assert_eq!(p("15 Desember 2026", true), ok(15, 12, 2026, "d mmmm yyyy"));
        assert_eq!(p("15 desember 2026", true), ok(15, 12, 2026, "d mmmm yyyy"));
        assert_eq!(p("05-DES-26", true), ok(5, 12, 2026, "dd-mmm-yy"));
        assert_eq!(p("1/Mei/2026", true), ok(1, 5, 2026, "d/mmmm/yyyy"));
        assert_eq!(p("17 Agt 2026", true), ok(17, 8, 2026, "d mmm yyyy"));
        assert_eq!(p("17 agu 2026", true), ok(17, 8, 2026, "d mmm yyyy"));
        assert_eq!(p("3 Okt, 2026", true), ok(3, 10, 2026, "d mmm, yyyy"));
        assert_eq!(p("Januari 9, 2027", true), ok(9, 1, 2027, "mmmm d, yyyy"));
        assert_eq!(p("15 Des", true), ok(15, 12, 2026, "d mmm"));
        assert_eq!(p("Maret 2025", true), ok(1, 3, 2025, "mmmm yyyy"));
        // English names in both locales.
        assert_eq!(p("15 December 2026", true), ok(15, 12, 2026, "d mmmm yyyy"));
        assert_eq!(
            p("15 December 2026", false),
            ok(15, 12, 2026, "d mmmm yyyy")
        );
        assert_eq!(
            p("December 15, 2026", false),
            ok(15, 12, 2026, "mmmm d, yyyy")
        );
        assert_eq!(p("dec 15 2026", false), ok(15, 12, 2026, "mmm d yyyy"));
        assert_eq!(p("Sept. 4, 2026", false), ok(4, 9, 2026, "mmm d, yyyy"));
        assert_eq!(p("Dec 15", false), ok(15, 12, 2026, "mmm d"));
        assert_eq!(p("15-dec", false), ok(15, 12, 2026, "d-mmm"));
        assert_eq!(p("Oct 2026", false), ok(1, 10, 2026, "mmm yyyy"));
        // Indonesian names only in Indonesian.
        assert_eq!(p("15 Desember 2026", false), None);
        assert_eq!(p("1 Mei 2026", false), None);
        // Not dates.
        for text in [
            "Mei Ling",
            "Mar 2026 report",
            "May",
            "Maret",
            "15 2026",
            "15 Des 2026 Des",
            "Des Des 2026",
            "15 Desember 202",
            "123 Des",
            "Des 123",
            "15 Des 2026,",
            "15 . Des 2026",
            "15+Des+2026",
            "Juni Juli",
        ] {
            assert_eq!(p(text, true), None, "{text}");
        }
    }

    #[test]
    fn locale_tags() {
        assert_eq!(without_locale_tags("[$-421]d mmmm yyyy"), "d mmmm yyyy");
        assert_eq!(without_locale_tags("[$-F800]dddd"), "dddd");
        assert_eq!(without_locale_tags("[$Rp-421]#,##0"), "[$Rp-421]#,##0");
        assert_eq!(without_locale_tags("d mmm [$-409"), "d mmm [$-409");
    }

    #[test]
    fn time_formats() {
        assert!(format_shows_time("h:mm"));
        assert!(format_shows_time("m/d/yyyy h:mm"));
        assert!(!format_shows_time("d/m/yyyy"));
        assert!(!format_shows_time("\"hours\" 0"));
    }
}
