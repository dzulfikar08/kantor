//! Excel number format codes, the basics charts use: `General`, `0`,
//! `0.0`, `#,##0.00`, `0%`, scaling commas (`#,##0,`), literals (`"$"0`,
//! `\$`, `[$Rp-421]`), up to four sections (`pos;neg;zero;text`),
//! scientific (`0.00E+00`) and dates (`d`, `mmm`, `yyyy`...). Colours,
//! conditions, padding (`_x`) and fill (`*x`) are skipped. Output uses
//! `.` for decimals and `,` for thousands (the codes' own symbols).

/// `v` formatted with `code` (`None` / `General` → General).
pub fn format_number(v: f64, code: Option<&str>) -> String {
    let code = code.unwrap_or("General").trim();
    if code.is_empty() || code.eq_ignore_ascii_case("general") {
        return general(v);
    }
    let sections = split_sections(code);
    let (section, v, neg_sign) = match sections.len() {
        1 => (sections[0].as_str(), v, v < 0.0),
        2 => {
            if v < 0.0 {
                (sections[1].as_str(), -v, false)
            } else {
                (sections[0].as_str(), v, false)
            }
        }
        _ => {
            if v < 0.0 {
                (sections[1].as_str(), -v, false)
            } else if v == 0.0 {
                (sections[2].as_str(), v, false)
            } else {
                (sections[0].as_str(), v, false)
            }
        }
    };
    let out = format_section(section, v.abs());
    if neg_sign
        && v != 0.0
        && !out.is_empty()
        && out.chars().any(|c| c.is_ascii_digit() && c != '0')
    {
        format!("-{out}")
    } else {
        out
    }
}

fn split_sections(code: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut quoted = false;
    let mut chars = code.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                out.last_mut().unwrap().push(c);
            }
            '\\' => {
                let s = out.last_mut().unwrap();
                s.push(c);
                if let Some(n) = chars.next() {
                    s.push(n);
                }
            }
            ';' if !quoted => out.push(String::new()),
            _ => out.last_mut().unwrap().push(c),
        }
    }
    out
}

/// One token of a section.
enum Tok {
    Lit(String),
    /// Digit placeholders and separators (`0`, `#`, `?`, `,`, `.`).
    Num(String),
    Percent,
    Exp(String),
    Date(String),
    /// `General` inside a section with literals.
    General,
}

fn tokens(section: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let cs: Vec<char> = section.chars().collect();
    let mut i = 0;
    let push_lit = |out: &mut Vec<Tok>, s: &str| match out.last_mut() {
        Some(Tok::Lit(l)) => l.push_str(s),
        _ => out.push(Tok::Lit(s.to_owned())),
    };
    while i < cs.len() {
        let c = cs[i];
        match c {
            '"' => {
                let end = cs[i + 1..]
                    .iter()
                    .position(|&c| c == '"')
                    .map_or(cs.len(), |p| i + 1 + p);
                let s: String = cs[i + 1..end].iter().collect();
                push_lit(&mut out, &s);
                i = end + 1;
            }
            '\\' => {
                if let Some(n) = cs.get(i + 1) {
                    push_lit(&mut out, &n.to_string());
                }
                i += 2;
            }
            '_' | '*' => i += 2,
            '[' => {
                let end = cs[i..]
                    .iter()
                    .position(|&c| c == ']')
                    .map_or(cs.len(), |p| i + p);
                let inner: String = cs[i + 1..end].iter().collect();
                // `[$Rp-421]`: a currency symbol; colours and conditions skipped.
                if let Some(cur) = inner.strip_prefix('$') {
                    push_lit(&mut out, cur.split('-').next().unwrap_or_default());
                }
                i = end + 1;
            }
            '0' | '#' | '?' | '.' | ',' => {
                let mut s = String::new();
                while i < cs.len() && matches!(cs[i], '0' | '#' | '?' | '.' | ',') {
                    s.push(cs[i]);
                    i += 1;
                }
                // A lone `.` or `,` between literals is text.
                if s.chars().any(|c| matches!(c, '0' | '#' | '?')) {
                    out.push(Tok::Num(s));
                } else {
                    push_lit(&mut out, &s);
                }
            }
            '%' => {
                out.push(Tok::Percent);
                i += 1;
            }
            'E' | 'e' if matches!(cs.get(i + 1), Some('+' | '-')) => {
                let mut s = String::new();
                i += 2;
                while i < cs.len() && matches!(cs[i], '0' | '#') {
                    s.push(cs[i]);
                    i += 1;
                }
                out.push(Tok::Exp(s));
            }
            'y' | 'Y' | 'm' | 'M' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' => {
                let lower = c.to_ascii_lowercase();
                let mut s = String::new();
                while i < cs.len() && cs[i].to_ascii_lowercase() == lower {
                    s.push(lower);
                    i += 1;
                }
                out.push(Tok::Date(s));
            }
            'G' | 'g'
                if cs[i..]
                    .iter()
                    .collect::<String>()
                    .to_lowercase()
                    .starts_with("general") =>
            {
                out.push(Tok::General);
                i += 7;
            }
            'A' | 'a'
                if cs[i..]
                    .iter()
                    .collect::<String>()
                    .to_uppercase()
                    .starts_with("AM/PM") =>
            {
                out.push(Tok::Date("ampm".into()));
                i += 5;
            }
            _ => {
                push_lit(&mut out, &c.to_string());
                i += 1;
            }
        }
    }
    out
}

fn format_section(section: &str, v: f64) -> String {
    let toks = tokens(section);
    if toks.iter().any(|t| matches!(t, Tok::Date(_))) {
        return format_date(&toks, v);
    }
    let percent = toks.iter().filter(|t| matches!(t, Tok::Percent)).count();
    let mut v = v * 100f64.powi(percent as i32);
    let exp = toks.iter().find_map(|t| match t {
        Tok::Exp(s) => Some(s.len().max(1)),
        _ => None,
    });
    let mut exponent = 0i32;
    if exp.is_some() && v != 0.0 {
        exponent = v.abs().log10().floor() as i32;
        v /= 10f64.powi(exponent);
    }
    // Only the first number pattern takes the value (`0.0" of "0` is rare).
    let mut used = false;
    let mut out = String::new();
    for t in &toks {
        match t {
            Tok::Lit(s) => out.push_str(s),
            Tok::Percent => out.push('%'),
            Tok::General => out.push_str(&general(v)),
            Tok::Num(pat) if !used => {
                used = true;
                out.push_str(&digits(pat, v));
            }
            Tok::Num(_) | Tok::Date(_) => {}
            Tok::Exp(s) => {
                let sign = if exponent < 0 { '-' } else { '+' };
                out.push_str(&format!(
                    "E{sign}{:0w$}",
                    exponent.abs(),
                    w = s.len().max(1)
                ));
            }
        }
    }
    if !used && !toks.iter().any(|t| matches!(t, Tok::General)) && out.is_empty() {
        return general(v);
    }
    out
}

/// A number with a digit pattern such as `#,##0.00` or `0,` (thousands).
fn digits(pat: &str, v: f64) -> String {
    let (int_pat, frac_pat) = match pat.find('.') {
        Some(i) => (&pat[..i], &pat[i + 1..]),
        None => (pat, ""),
    };
    // Trailing commas scale by 1000 each.
    let trailing = int_pat.len() - int_pat.trim_end_matches(',').len();
    let int_pat = int_pat.trim_end_matches(',');
    let v = v / 1000f64.powi(trailing as i32);
    let group = int_pat.contains(',');
    let min_int = int_pat.chars().filter(|&c| c == '0').count();
    let frac_digits: Vec<char> = frac_pat.chars().filter(|c| *c != ',').collect();
    let decimals = frac_digits.len();
    // Excel rounds halves away from zero (format! rounds them to even).
    let k = 10f64.powi(decimals as i32);
    let s = format!("{:.*}", decimals, (v.abs() * k).round() / k);
    let (int, frac) = match s.find('.') {
        Some(i) => (&s[..i], &s[i + 1..]),
        None => (s.as_str(), ""),
    };
    let mut int = int.trim_start_matches('0').to_owned();
    while int.len() < min_int {
        int.insert(0, '0');
    }
    if group {
        let b = int.as_bytes();
        let mut g = String::new();
        for (i, c) in b.iter().enumerate() {
            if i > 0 && (b.len() - i).is_multiple_of(3) {
                g.push(',');
            }
            g.push(*c as char);
        }
        int = g;
    }
    // Optional decimals (`#`) drop trailing zeros.
    let mut frac: Vec<char> = frac.chars().collect();
    while let (Some(&'0'), Some(p)) = (frac.last(), frac_digits.get(frac.len().wrapping_sub(1)))
        && *p != '0'
    {
        frac.pop();
    }
    let frac: String = frac.into_iter().collect();
    if frac.is_empty() {
        if frac_pat.is_empty() {
            int
        } else {
            format!("{int}.")
        }
        .trim_end_matches('.')
        .to_owned()
    } else {
        format!("{int}.{frac}")
    }
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
    split_sections(code)
        .first()
        .is_some_and(|s| tokens(s).iter().any(|t| matches!(t, Tok::Date(_))))
}

fn civil(serial: f64) -> (i64, u32, u32) {
    // Excel's 1900 system: serial 1 = 1900-01-01, 60 = the phantom
    // 29 Feb 1900, 25569 = 1970-01-01.
    let mut days = serial.floor() as i64;
    if days > 59 {
        days -= 1;
    }
    // Days since 1970-01-01 → civil date (Howard Hinnant's algorithm).
    let z = days - 25_568 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

const MONTHS: [&str; 12] = [
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
const DAYS: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];

fn format_date(toks: &[Tok], v: f64) -> String {
    let (y, mo, d) = civil(v);
    let secs = ((v - v.floor()) * 86_400.0).round() as i64;
    let (h, mi, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let ampm = toks
        .iter()
        .any(|t| matches!(t, Tok::Date(s) if s == "ampm"));
    let weekday = (v.floor() as i64 + 6).rem_euclid(7) as usize;
    let mut out = String::new();
    for (i, t) in toks.iter().enumerate() {
        match t {
            Tok::Lit(l) => out.push_str(l),
            Tok::Date(k) => {
                // `m` after `h` or before `s` is minutes.
                let minutes = k.starts_with('m')
                    && k.len() <= 2
                    && (toks[..i]
                        .iter()
                        .rev()
                        .find(|t| matches!(t, Tok::Date(_)))
                        .is_some_and(|t| matches!(t, Tok::Date(p) if p.starts_with('h')))
                        || toks[i + 1..]
                            .iter()
                            .find(|t| matches!(t, Tok::Date(_)))
                            .is_some_and(|t| matches!(t, Tok::Date(p) if p.starts_with('s'))));
                let piece = match k.as_str() {
                    "yy" | "y" => format!("{:02}", y.rem_euclid(100)),
                    k if k.starts_with('y') => y.to_string(),
                    "m" if minutes => mi.to_string(),
                    "mm" if minutes => format!("{mi:02}"),
                    "m" => mo.to_string(),
                    "mm" => format!("{mo:02}"),
                    "mmm" => MONTHS[(mo - 1) as usize][..3].to_owned(),
                    "mmmmm" => MONTHS[(mo - 1) as usize][..1].to_owned(),
                    k if k.starts_with('m') => MONTHS[(mo - 1) as usize].to_owned(),
                    "d" => d.to_string(),
                    "dd" => format!("{d:02}"),
                    "ddd" => DAYS[weekday][..3].to_owned(),
                    k if k.starts_with('d') => DAYS[weekday].to_owned(),
                    "h" | "hh" => {
                        let h = if ampm { (h + 11) % 12 + 1 } else { h };
                        if k == "hh" {
                            format!("{h:02}")
                        } else {
                            h.to_string()
                        }
                    }
                    "s" => s.to_string(),
                    k if k.starts_with('s') => format!("{s:02}"),
                    "ampm" => (if h < 12 { "AM" } else { "PM" }).to_owned(),
                    _ => String::new(),
                };
                out.push_str(&piece);
            }
            _ => {}
        }
    }
    out
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
        assert_eq!(f(2500.0, "[$Rp-421]#,##0"), "Rp2,500");
        assert_eq!(f(2_500_000.0, "#,##0,,\"M\""), "3M");
        assert_eq!(f(1.5, "0.##"), "1.5");
        assert_eq!(f(2.0, "0.##"), "2");
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
