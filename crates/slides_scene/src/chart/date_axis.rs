//! Date axes (`c:dateAx`), as Excel draws them: categories that are dates
//! sit at their date on a time scale in days, months or years (the base
//! unit), so dates missing from the data leave room and dates in the same
//! unit share a place; labels go at every major unit.

use super::model::TimeUnit;

/// A date (Excel's 1900 date system serial) as (year, month 1..12, day).
pub fn ymd(serial: f64) -> (i64, i64, i64) {
    let d = serial.floor() as i64;
    // Excel's 29 February 1900 (serial 60) doesn't exist; earlier serials
    // are a day off from the calendar.
    if d == 60 {
        return (1900, 2, 29);
    }
    civil(if d < 60 { d + 1 } else { d } - 25_569)
}

/// Days since 1970-01-01 → civil date (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The serial of a civil date (1900 system; dates after February 1900).
pub fn serial(y: i64, m: i64, d: i64) -> f64 {
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = y2.div_euclid(400);
    let yoe = y2 - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468; // since 1970-01-01
    (days + 25_569) as f64
}

/// Whole `unit`s from `a` to `b` (serials).
pub fn units_between(a: f64, b: f64, unit: TimeUnit) -> i64 {
    let (ya, ma, _) = ymd(a);
    let (yb, mb, _) = ymd(b);
    match unit {
        TimeUnit::Days => b.floor() as i64 - a.floor() as i64,
        TimeUnit::Months => (yb * 12 + mb) - (ya * 12 + ma),
        TimeUnit::Years => yb - ya,
    }
}

/// The start of the `k`th `unit` from the one `a` is in (days keep `a`).
pub fn unit_start(a: f64, k: i64, unit: TimeUnit) -> f64 {
    let (y, m, _) = ymd(a);
    match unit {
        TimeUnit::Days => a.floor() + k as f64,
        TimeUnit::Months => {
            let t = y * 12 + (m - 1) + k;
            serial(t.div_euclid(12), t.rem_euclid(12) + 1, 1)
        }
        TimeUnit::Years => serial(y + k, 1, 1),
    }
}

/// Excel's base unit for dates `dates` (sorted): days when two dates are
/// less than a month apart or fall on different days of their months,
/// else months, or years when all are on the same day of the year.
pub fn auto_unit(dates: &[f64]) -> TimeUnit {
    let ds: Vec<(i64, i64, i64)> = dates.iter().map(|d| ymd(*d)).collect();
    let min_gap = dates
        .windows(2)
        .map(|w| w[1].floor() - w[0].floor())
        .filter(|g| *g > 0.0)
        .fold(f64::INFINITY, f64::min);
    if min_gap < 28.0 || ds.iter().any(|d| d.2 != ds[0].2) {
        return TimeUnit::Days;
    }
    if ds.iter().all(|d| d.1 == ds[0].1) && min_gap >= 365.0 {
        TimeUnit::Years
    } else {
        TimeUnit::Months
    }
}

/// A date axis' layout: each point's place (in units from the first,
/// centred: `k + 0.5`), the number of units, and each unit's date and
/// whether it takes a label (`major`: the major unit and its time unit).
pub struct DateSlots {
    pub at: Vec<f64>,
    pub count: usize,
    pub starts: Vec<f64>,
    pub labelled: Vec<bool>,
}

/// The most units a date axis spreads over (more: drawn as categories).
pub const MAX_UNITS: i64 = 5_000;

pub fn slots(
    dates: &[Option<f64>],
    base: Option<TimeUnit>,
    major: Option<(f64, Option<TimeUnit>)>,
) -> Option<DateSlots> {
    slots_within(dates, base, major, (None, None))
}

/// [`slots`] between the axis' own bounds (`c:min`, `c:max`, serials)
/// where it has them: a point outside them isn't drawn (its place is NaN).
pub fn slots_within(
    dates: &[Option<f64>],
    base: Option<TimeUnit>,
    major: Option<(f64, Option<TimeUnit>)>,
    (lo, hi): (Option<f64>, Option<f64>),
) -> Option<DateSlots> {
    let ds: Vec<f64> = dates.iter().copied().collect::<Option<Vec<f64>>>()?;
    if ds.is_empty() || ds.iter().any(|d| !d.is_finite() || *d < 61.0) {
        return None;
    }
    let mut sorted = ds.clone();
    sorted.sort_by(f64::total_cmp);
    let unit = base.unwrap_or_else(|| auto_unit(&sorted));
    let ok = |v: &f64| v.is_finite() && *v >= 61.0;
    let first = lo.filter(ok).unwrap_or(sorted[0]).floor();
    let last = hi.filter(ok).unwrap_or(*sorted.last()?);
    if last < first {
        return None;
    }
    let count = units_between(first, last, unit) + 1;
    if !(1..=MAX_UNITS).contains(&count) {
        return None;
    }
    let at = ds
        .iter()
        .map(|d| {
            let k = units_between(first, *d, unit);
            if *d < first || k >= count {
                f64::NAN
            } else {
                k as f64 + 0.5
            }
        })
        .collect();
    let starts: Vec<f64> = (0..count).map(|k| unit_start(first, k, unit)).collect();
    let labelled = match major {
        None => vec![true; count as usize],
        Some((step, tu)) => {
            let tu = tu.unwrap_or(unit).max(unit);
            let step = step.round().max(1.0) as i64;
            starts
                .iter()
                .map(|s| {
                    let n = units_between(first, *s, tu);
                    let at_start = tu == unit || unit_start(first, n, tu) == s.floor();
                    at_start && n % step == 0
                })
                .collect()
        }
    };
    Some(DateSlots {
        at,
        count: count as usize,
        starts,
        labelled,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_cut_and_extend_the_axis() {
        // Weekly dates from 2025-01-01; the axis from 2025-01-05 to
        // 2025-01-31 in days.
        let d: Vec<Option<f64>> = (0..4).map(|k| Some(45_658.0 + 7.0 * k as f64)).collect();
        let s = slots_within(
            &d,
            Some(TimeUnit::Days),
            None,
            (Some(45_662.0), Some(45_688.0)),
        )
        .unwrap();
        assert_eq!(s.count, 27);
        assert!(s.at[0].is_nan(), "before the minimum");
        assert_eq!(s.at[1], 3.5);
        assert_eq!(s.at[3], 17.5);
    }

    #[test]
    fn serials_and_dates() {
        assert_eq!(ymd(1.0), (1900, 1, 1));
        assert_eq!(ymd(59.0), (1900, 2, 28));
        assert_eq!(ymd(61.0), (1900, 3, 1));
        assert_eq!(ymd(45_658.0), (2025, 1, 1));
        assert_eq!(serial(2025, 1, 1), 45_658.0);
        assert_eq!(serial(2024, 2, 29), 45_351.0);
        assert_eq!(ymd(45_351.0), (2024, 2, 29));
    }

    #[test]
    fn days_leave_room_for_missing_dates() {
        // 1, 2 and 5 January 2025: five days, the third point at the fifth.
        let d = [Some(45_658.0), Some(45_659.0), Some(45_662.0)];
        let s = slots(&d, None, None).unwrap();
        assert_eq!(s.count, 5);
        assert_eq!(s.at, [0.5, 1.5, 4.5]);
    }

    #[test]
    fn months_share_a_place_and_label_by_major_unit() {
        // The 1st of Jan, Feb, Apr 2025, and 15 Apr: months as the base.
        let d = [
            Some(serial(2025, 1, 1)),
            Some(serial(2025, 2, 1)),
            Some(serial(2025, 4, 1)),
            Some(serial(2025, 4, 15)),
        ];
        let s = slots(&d, Some(TimeUnit::Months), Some((2.0, None))).unwrap();
        assert_eq!(s.count, 4);
        assert_eq!(s.at, [0.5, 1.5, 3.5, 3.5]);
        assert_eq!(s.labelled, [true, false, true, false]);
        assert_eq!(s.starts[2], serial(2025, 3, 1));
        // Chosen by Excel's rule: monthly dates → months.
        assert_eq!(
            auto_unit(&[serial(2025, 1, 1), serial(2025, 3, 1)]),
            TimeUnit::Months
        );
        assert_eq!(
            auto_unit(&[serial(2020, 6, 1), serial(2024, 6, 1)]),
            TimeUnit::Years
        );
        // Days with years as the major unit: labels on 1 January only.
        let d = [Some(serial(2024, 12, 30)), Some(serial(2025, 1, 2))];
        let s = slots(&d, Some(TimeUnit::Days), Some((1.0, Some(TimeUnit::Years)))).unwrap();
        assert_eq!(s.labelled, [false, false, true, false]);
    }
}
