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

use std::sync::atomic::{AtomicU8, Ordering};

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

impl Model<'_> {
    /// A typed date in the input locale: serial and format.
    fn kantor_parse_date(&self, text: &str, indonesian: bool) -> Option<(i32, String)> {
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
            // A date, then a time: "16/11/2026 9:30".
            if let Some((date, time)) = trimmed.split_once(char::is_whitespace) {
                if let (Some((d, df)), Some((t, tf))) =
                    (self.kantor_parse_date(date, indonesian), parse_time(time))
                {
                    if t < 1.0 && tf != "mm:ss.0" {
                        return Ok((d as f64 + t, Some(format!("{df} {tf}"))));
                    }
                }
            }
        }
        if indonesian {
            if let Some((d, f)) = self.kantor_parse_date(value, indonesian) {
                return Ok((d as f64, Some(f)));
            }
        }
        parse_formatted_number(value, currencies, self.locale)
    }
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
    fn time_formats() {
        assert!(format_shows_time("h:mm"));
        assert!(format_shows_time("m/d/yyyy h:mm"));
        assert!(!format_shows_time("d/m/yyyy"));
        assert!(!format_shows_time("\"hours\" 0"));
    }
}
