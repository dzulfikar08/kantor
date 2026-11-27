//! OLE Automation dates: a `f64` counting days since 1899-12-30; the fraction is the time of day.
//!
//! For negative dates the fraction still counts forward from midnight (`-1.25` is 1899-12-29
//! 06:00), as in OLE Automation and VBA. Calendar arithmetic is proleptic Gregorian.

/// Days from 1970-01-01 to a proleptic Gregorian date (H. Hinnant's `days_from_civil`).
pub const fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let m = m as i64;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The date `z` days after 1970-01-01 (H. Hinnant's `civil_from_days`).
pub const fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = (if z >= 0 { z } else { z - 146_096 }) / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

const EPOCH: i64 = days_from_civil(1899, 12, 30);

/// The earliest and latest dates VBA accepts (100-01-01 and 9999-12-31 23:59:59).
pub const MIN: f64 = -657_434.0;
pub const MAX: f64 = 2_958_465.999_988_426;

pub fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(year) {
                29
            } else {
                28
            }
        }
    }
}

/// The day number of a calendar date (no normalisation: pass a valid date).
pub fn day_number(year: i64, month: u32, day: u32) -> i64 {
    days_from_civil(year, month, day) - EPOCH
}

/// The day number of a date, normalising months outside 1..=12 and days outside the month
/// (`DateSerial(2024, 14, 0)` is 2025-01-31).
pub fn day_number_normalised(year: i64, month: i64, day: i64) -> i64 {
    let y = year + (month - 1).div_euclid(12);
    let m = (month - 1).rem_euclid(12) as u32 + 1;
    day_number(y, m, 1) + day - 1
}

/// Splits an OLE date into its day number and the seconds since midnight, rounding to the
/// nearest second.
pub fn split(ole: f64) -> (i64, u32) {
    let day = ole.trunc();
    let frac = (ole - day).abs();
    let mut secs = (frac * 86_400.0).round() as i64;
    let mut day = day as i64;
    if secs >= 86_400 {
        secs -= 86_400;
        day += if ole < 0.0 { -1 } else { 1 };
    }
    (day, secs as u32)
}

/// Builds an OLE date from a day number and seconds since midnight.
pub fn join(day: i64, secs: f64) -> f64 {
    let t = secs / 86_400.0;
    if day < 0 {
        day as f64 - t
    } else {
        day as f64 + t
    }
}

/// The calendar date of an OLE date.
pub fn ymd(ole: f64) -> (i64, u32, u32) {
    let (day, _) = split(ole);
    civil_from_days(day + EPOCH)
}

/// The time of day of an OLE date.
pub fn hms(ole: f64) -> (u32, u32, u32) {
    let (_, s) = split(ole);
    (s / 3600, s / 60 % 60, s % 60)
}

/// The weekday, 1 = Sunday to 7 = Saturday.
pub fn weekday(ole: f64) -> u32 {
    let (day, _) = split(ole);
    // 1899-12-30 was a Saturday.
    ((day + 6).rem_euclid(7) + 1) as u32
}

/// The current time in UTC as an OLE date.
pub fn now_utc() -> f64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    (secs / 86_400.0) - EPOCH as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_serials() {
        assert_eq!(day_number(1899, 12, 30), 0);
        assert_eq!(day_number(1900, 1, 1), 2);
        assert_eq!(day_number(2024, 1, 15), 45306);
        assert_eq!(ymd(45306.75), (2024, 1, 15));
        assert_eq!(hms(45306.75), (18, 0, 0));
        assert_eq!(ymd(-1.25), (1899, 12, 29));
        assert_eq!(hms(-1.25), (6, 0, 0));
        // 2024-01-15 was a Monday.
        assert_eq!(weekday(45306.0), 2);
        assert_eq!(day_number_normalised(2024, 14, 0), day_number(2025, 1, 31));
        assert_eq!(day_number_normalised(2024, 3, 0), day_number(2024, 2, 29));
        assert_eq!(ymd(MIN), (100, 1, 1));
        assert_eq!(ymd(MAX), (9999, 12, 31));
    }
}
