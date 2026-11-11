//! Number format codes on axes and data labels, through `kantor_numfmt`
//! (the same formatting as Sheets' cells): sections, colours (ignored
//! here), conditions, scaling commas, literals and locale tags
//! (`[$Rp-421]`), scientific, fractions and dates. Padding (`_x`) and
//! fill (`*x`) are trimmed away, as charts have no column to pad to.
//! General is the chart's own (up to 10 significant characters, no cell
//! width to fit).

use kantor_numfmt::{DateSystem, NumberFormat};

/// `v` formatted with `code` (`None` / `General` → General).
pub fn format_number(v: f64, code: Option<&str>) -> String {
    let code = code.unwrap_or("General").trim();
    if code.is_empty() || code.eq_ignore_ascii_case("general") {
        return general(v);
    }
    let fmt = NumberFormat::parse_cached(code);
    if !fmt.is_valid() || fmt.is_general() {
        return general(v);
    }
    fmt.display_number(v, DateSystem::D1900)
        .text
        .trim()
        .to_owned()
}

/// Excel's General: integers as is, others with up to 10 significant
/// characters, scientific for very large or small numbers.
pub fn general(v: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-9..1e11).contains(&a) {
        let s = format!("{v:.5E}");
        // 1.23450E3 → 1.2345E+03
        let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        let e: i32 = e.parse().unwrap_or(0);
        let sign = if e < 0 { '-' } else { '+' };
        return format!("{m}E{sign}{:02}", e.abs());
    }
    let int_digits = (a.log10().floor() as i32 + 1).max(1) as usize;
    let decimals = 10usize.saturating_sub(int_digits).min(9);
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        s
    }
}

/// Whether a format code shows dates or times.
pub fn is_date_code(code: &str) -> bool {
    NumberFormat::parse_cached(code).is_date()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let f = |v, c| format_number(v, Some(c));
        assert_eq!(f(1234.567, "0"), "1235");
        assert_eq!(f(1234.567, "0.0"), "1234.6");
        assert_eq!(f(1234.567, "#,##0"), "1,235");
        assert_eq!(f(1234567.891, "#,##0.00"), "1,234,567.89");
        assert_eq!(f(0.256, "0%"), "26%");
        assert_eq!(f(0.256, "0.0%"), "25.6%");
        assert_eq!(f(-5.0, "0"), "-5");
        assert_eq!(f(-5.0, "0;(0)"), "(5)");
        assert_eq!(f(0.0, "0;-0;\"-\""), "-");
        assert_eq!(f(2500.0, "\"$\"#,##0"), "$2,500");
        // The locale tag (421: Indonesian) picks its separators, as in cells.
        assert_eq!(f(2500.0, "[$Rp-421]#,##0"), "Rp2.500");
        assert_eq!(f(2_500_000.0, "#,##0,,\"M\""), "3M");
        assert_eq!(f(1.5, "0.##"), "1.5");
        // As Excel shows it, point and all.
        assert_eq!(f(2.0, "0.##"), "2.");
        assert_eq!(f(0.5, "#,##0"), "1");
        assert_eq!(f(12345.0, "0.00E+00"), "1.23E+04");
        assert_eq!(f(12.0, "_(* #,##0_);_(* (#,##0);_(* \"-\"_);_(@_)"), "12");
        assert_eq!(f(7.0, "General"), "7");
    }

    #[test]
    fn general_numbers() {
        assert_eq!(general(0.1 + 0.2), "0.3");
        assert_eq!(general(-12.5), "-12.5");
        assert_eq!(general(1e12), "1E+12");
        assert_eq!(general(1_234_567.0), "1234567");
        assert_eq!(general(2.0 / 3.0), "0.666666667");
    }

    #[test]
    fn dates() {
        // 45658 = 2025-01-01 (a Wednesday).
        assert_eq!(format_number(45658.0, Some("m/d/yyyy")), "1/1/2025");
        assert_eq!(format_number(45658.0, Some("mmm-yy")), "Jan-25");
        assert_eq!(
            format_number(45658.0, Some("dddd, d mmmm yyyy")),
            "Wednesday, 1 January 2025"
        );
        assert_eq!(format_number(45658.75, Some("h:mm AM/PM")), "6:00 PM");
        assert_eq!(format_number(1.0, Some("yyyy-mm-dd")), "1900-01-01");
        assert!(is_date_code("mmm-yy") && !is_date_code("0.0"));
    }
}
