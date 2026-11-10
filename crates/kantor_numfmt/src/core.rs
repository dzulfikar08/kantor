//! The parts of `gridcraft-core` the formatter needs, adapted from GridCraft at commit
//! e9addf52fe68847e6c1310fcfa5d42b7aa8b9650 (`crates/core/src/{date,color,value}.rs`,
//! MIT OR Apache-2.0). See `PROVENANCE.md`.
//!
//! `date` and `color` are copied as they are. `Value` and `CellError` are cut down to what
//! formatting and `TEXT()` look at, and `parse_number_text` is a small stand-in for GridCraft's
//! full number-entry parser (plain numbers, thousands commas and a trailing `%`).

/// Date serial numbers.
///
/// The 1900 system counts days from 1899-12-31 (serial 1 = 1900-01-01) and, like Lotus 1-2-3 and
/// Excel, treats 1900 as a leap year: serial 60 is the fictitious 1900-02-29. The 1904 system
/// counts from 1904-01-01 (serial 0). Times are fractions of a day.
pub mod date {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Hash)]
    pub enum DateSystem {
        #[default]
        D1900,
        D1904,
    }

    /// A calendar date and time of day.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub struct DateTime {
        pub year: i32,
        pub month: u32,
        pub day: u32,
        pub hour: u32,
        pub minute: u32,
        pub second: u32,
        /// Milliseconds (rounded).
        pub milli: u32,
        /// 0 = Sunday.
        pub weekday: u32,
    }

    /// Days since 1970-01-01 for a proleptic Gregorian date, exact for every `i32` year.
    ///
    /// The Gregorian calendar repeats every 400 years (146_097 days): chrono converts the year
    /// within its cycle and whole cycles are added back as days. 1970-01-01 is
    /// `num_days_from_ce` 719_163.
    pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
        let (cycles, year) = (i128::from(i64::from(y).div_euclid(400)), i64::from(y).rem_euclid(400));
        i32::try_from(year)
            .ok()
            .and_then(|year| chrono::NaiveDate::from_ymd_opt(year, m, d))
            .and_then(|date| {
                let days = i128::from(chrono::Datelike::num_days_from_ce(&date)) - 719_163 + cycles * 146_097;
                i64::try_from(days).ok()
            })
            .unwrap_or(0)
    }

    /// Proleptic Gregorian `(year, month, day)` for a day count `z` since 1970-01-01 (the
    /// inverse of [`days_from_civil`]).
    pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
        use chrono::Datelike;
        let (cycles, rest) = (z.div_euclid(146_097), z.rem_euclid(146_097));
        match i32::try_from(rest + 719_163).ok().and_then(chrono::NaiveDate::from_num_days_from_ce_opt) {
            Some(d) => {
                let y = i128::from(d.year()) + i128::from(cycles) * 400;
                (i32::try_from(y).unwrap_or(if y < 0 { i32::MIN } else { i32::MAX }), d.month(), d.day())
            }
            // Unreachable: `rest + 719_163` is always a valid CE day.
            None => (1970, 1, 1),
        }
    }

    const UNIX_1899_12_30: i64 = -25569; // days_from_civil(1899, 12, 30)
    const UNIX_1904_01_01: i64 = -24107;

    /// Number of days in month `m` of year `y` (1–31), or `0` for an invalid month number.
    pub fn days_in_month(y: i32, m: u32) -> u32 {
        if !(1..=12).contains(&m) {
            return 0;
        }
        let y = i64::from(y).rem_euclid(400) as i32;
        let first = |y: i32, m: u32| chrono::NaiveDate::from_ymd_opt(y, m, 1);
        match (first(y, m), if m == 12 { first(y + 1, 1) } else { first(y, m + 1) }) {
            (Some(a), Some(b)) => u32::try_from((b - a).num_days()).unwrap_or(0),
            _ => 0,
        }
    }

    /// Last serial that is a valid date in a date system: 9999-12-31.
    pub fn max_serial(sys: DateSystem) -> i64 {
        match sys {
            DateSystem::D1900 => 2_958_465,
            DateSystem::D1904 => 2_958_465 - 1462,
        }
    }

    /// Date/time from a serial. `None` for negative or too-large serials.
    pub fn datetime_from_serial(sys: DateSystem, serial: f64) -> Option<DateTime> {
        if !serial.is_finite() || !(0.0..max_serial(sys) as f64 + 1.0).contains(&serial) {
            return None;
        }
        let mut day = serial.floor() as i64;
        let mut frac_ms = ((serial - serial.floor()) * 86_400_000.0).round() as i64;
        if frac_ms >= 86_400_000 {
            frac_ms -= 86_400_000;
            day += 1;
            if day > max_serial(sys) {
                return None;
            }
        }
        let (y, m, d, weekday) = match sys {
            DateSystem::D1900 => {
                // Excel's weekdays continue the fictitious calendar: serial 1 is a Sunday.
                let wd = ((day + 6) % 7) as u32;
                if day == 60 {
                    (1900, 2, 29, wd)
                } else if day == 0 {
                    (1900, 1, 0, wd)
                } else {
                    let real = if day < 60 { day + 1 } else { day };
                    let (y, m, d) = civil_from_days(real + UNIX_1899_12_30);
                    (y, m, d, wd)
                }
            }
            DateSystem::D1904 => {
                let unix = day + UNIX_1904_01_01;
                let (y, m, d) = civil_from_days(unix);
                (y, m, d, ((unix + 4).rem_euclid(7)) as u32)
            }
        };
        let secs = frac_ms / 1000;
        Some(DateTime {
            year: y,
            month: m,
            day: d,
            hour: (secs / 3600) as u32,
            minute: ((secs / 60) % 60) as u32,
            second: (secs % 60) as u32,
            milli: (frac_ms % 1000) as u32,
            weekday,
        })
    }

    pub const MONTHS: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    pub const WEEKDAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
}

/// The legacy indexed colour palette used by `[ColorN]`.
pub mod color {
    /// The 64-entry legacy indexed colour palette, stored as `RRGGBB`.
    pub const INDEXED: [u32; 64] = [
        0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00, 0xFF00FF, 0x00FFFF, 0x000000, 0xFFFFFF, 0xFF0000, 0x00FF00, 0x0000FF, 0xFFFF00,
        0xFF00FF, 0x00FFFF, 0x800000, 0x008000, 0x000080, 0x808000, 0x800080, 0x008080, 0xC0C0C0, 0x808080, 0x9999FF, 0x993366, 0xFFFFCC, 0xCCFFFF,
        0x660066, 0xFF8080, 0x0066CC, 0xCCCCFF, 0x000080, 0xFF00FF, 0xFFFF00, 0x00FFFF, 0x800080, 0x800000, 0x008080, 0x0000FF, 0x00CCFF, 0xCCFFFF,
        0xCCFFCC, 0xFFFF99, 0x99CCFF, 0xFF99CC, 0xCC99FF, 0xFFCC99, 0x3366FF, 0x33CCCC, 0x99CC00, 0xFFCC00, 0xFF9900, 0xFF6600, 0x666699, 0x969696,
        0x003366, 0x339966, 0x003300, 0x333300, 0x993300, 0x993366, 0x333399, 0x333333,
    ];

    /// Resolve an Excel number-format color index (`Color1` through `Color56`).
    pub fn indexed_color(index: u8) -> Option<[u8; 3]> {
        if !(1..=56).contains(&index) {
            return None;
        }
        let slot = index.checked_add(7)?;
        let rgb = *INDEXED.get(slot as usize)?;
        Some([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8])
    }
}

pub use date::DateSystem;

/// Excel's error values (the ones formatting and `TEXT()` can produce or pass through).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CellError {
    Null,
    Div0,
    Value,
    Ref,
    Name,
    Num,
    NA,
}

impl CellError {
    pub fn as_str(&self) -> &'static str {
        match self {
            CellError::Null => "#NULL!",
            CellError::Div0 => "#DIV/0!",
            CellError::Value => "#VALUE!",
            CellError::Ref => "#REF!",
            CellError::Name => "#NAME?",
            CellError::Num => "#NUM!",
            CellError::NA => "#N/A",
        }
    }
}

/// A cell value to format.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Empty,
    Number(f64),
    Text(String),
    Bool(bool),
    Error(CellError),
}

impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Number(v)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_string())
    }
}

/// Numeric text as `TEXT()` reads it: a plain number (`5`, `-1.5e3`), with thousands commas
/// (`1,000`) or a trailing percent sign (`50%`). GridCraft also reads currency, dates and times
/// here; Kantor only uses `TEXT()` in tests, so this stays small.
pub fn parse_number_text(s: &str) -> Option<f64> {
    let t = s.trim();
    let (t, pct) = match t.strip_suffix('%') {
        Some(rest) => (rest.trim_end(), true),
        None => (t, false),
    };
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || "+-.,eE".contains(c)) {
        return None;
    }
    let plain: String = t.chars().filter(|&c| c != ',').collect();
    let n: f64 = plain.parse().ok()?;
    let n = if pct { n / 100.0 } else { n };
    n.is_finite().then_some(n)
}
