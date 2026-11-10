//! Value axis scaling as Excel does it (approximately): a "nice" major
//! unit (1, 2 or 5 × 10ⁿ), zero included unless the data sits far from
//! it, about 5 % headroom above the largest value.

/// A value axis range and its major unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
    pub unit: f64,
}

impl Scale {
    /// The major tick values, `min` to `max`.
    pub fn ticks(&self) -> Vec<f64> {
        let n = ((self.max - self.min) / self.unit)
            .round()
            .clamp(0.0, 1000.0) as usize;
        (0..=n)
            .map(|i| {
                let v = self.min + i as f64 * self.unit;
                // Snap away float noise (0.30000000000000004).
                let snap = self.unit * 1e-9;
                if v.abs() < snap {
                    0.0
                } else {
                    round_sig(v, 12)
                }
            })
            .collect()
    }

    /// Where `v` falls, 0 at `min` and 1 at `max`.
    pub fn frac(&self, v: f64) -> f64 {
        let span = self.max - self.min;
        if span.abs() < f64::EPSILON {
            0.0
        } else {
            (v - self.min) / span
        }
    }
}

fn round_sig(v: f64, digits: i32) -> f64 {
    if v == 0.0 {
        return 0.0;
    }
    let mag = 10f64.powi(digits - 1 - v.abs().log10().floor() as i32);
    (v * mag).round() / mag
}

/// The smallest of 1, 2, 5 × 10ⁿ that is at least `raw`.
pub fn nice_unit(raw: f64) -> f64 {
    if raw <= 0.0 || !raw.is_finite() {
        return 1.0;
    }
    let exp = raw.log10().floor();
    let base = 10f64.powf(exp);
    let f = raw / base;
    let nice = if f <= 1.0 + 1e-9 {
        1.0
    } else if f <= 2.0 + 1e-9 {
        2.0
    } else if f <= 5.0 + 1e-9 {
        5.0
    } else {
        10.0
    };
    nice * base
}

/// Fixed parts of an axis (`c:scaling` min/max, `c:majorUnit`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Fixed {
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub unit: Option<f64>,
}

/// Axis range for data between `lo` and `hi`, at most `intervals` major
/// units long (fewer on short axes).
pub fn auto_scale(lo: f64, hi: f64, fixed: Fixed, intervals: usize) -> Scale {
    let intervals = intervals.max(1) as f64;
    let (mut lo, mut hi) = if lo.is_finite() && hi.is_finite() && lo <= hi {
        (lo, hi)
    } else if fixed == Fixed::default() {
        // No data: 0 to 1.
        return Scale {
            min: 0.0,
            max: 1.0,
            unit: nice_unit(1.0 / intervals),
        };
    } else {
        (0.0, 1.0)
    };
    if let Some(m) = fixed.min {
        lo = lo.max(m);
        hi = hi.max(m);
    }
    if let Some(m) = fixed.max {
        hi = hi.min(m);
        lo = lo.min(m);
    }
    if lo == hi {
        // One value: an axis from zero to it (or around zero).
        if lo == 0.0 {
            hi = 1.0;
        } else if lo > 0.0 {
            lo = 0.0;
        } else {
            hi = 0.0;
        }
    }
    // Zero is included unless the values sit far from it (their spread is
    // under a sixth of their size).
    let min0 = match fixed.min {
        Some(m) => m,
        None if lo >= 0.0 && (hi - lo) / hi >= 1.0 / 6.0 => 0.0,
        None if lo >= 0.0 => lo - (hi - lo) / 2.0,
        None => lo,
    };
    let max0 = match fixed.max {
        Some(m) => m,
        None if hi <= 0.0 && (hi - lo) / -lo >= 1.0 / 6.0 => 0.0,
        None if hi <= 0.0 => hi + (hi - lo) / 2.0,
        None => hi,
    };
    let headroom = |span: f64| span * 0.05;
    let span = (max0 - min0).max(f64::MIN_POSITIVE);
    // The unit comes from the data's span; the headroom only moves max.
    let unit = fixed
        .unit
        .filter(|u| *u > 0.0)
        .unwrap_or_else(|| nice_unit(span / intervals));
    let min = match fixed.min {
        Some(m) => m,
        None if min0 == 0.0 => 0.0,
        None => {
            let pad = if min0 < 0.0 { headroom(span) } else { 0.0 };
            ((min0 - pad) / unit).floor() * unit
        }
    };
    let max = match fixed.max {
        Some(m) => m,
        None if max0 == 0.0 => 0.0,
        None => {
            let pad = if max0 > 0.0 { headroom(span) } else { 0.0 };
            // On the tick grid that starts at min.
            let m = min + ((max0 + pad - min) / unit - 1e-9).ceil() * unit;
            if m <= min { min + unit } else { m }
        }
    };
    // Too many ticks for a fixed unit: keep it but cap the count.
    let unit = if ((max - min) / unit) > 1000.0 {
        (max - min) / 1000.0
    } else {
        unit
    };
    Scale {
        min: round_sig(min, 12),
        max: round_sig(max, 12),
        unit,
    }
}

/// How many major intervals fit on an axis `len` px long whose labels are
/// `label` px tall (vertical axes) or wide (horizontal ones).
pub fn intervals_for(len: f32, label: f32) -> usize {
    let per = (label * 2.5).max(8.0);
    ((len / per).floor() as usize).clamp(2, 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(lo: f64, hi: f64, n: usize) -> (f64, f64, f64) {
        let s = auto_scale(lo, hi, Fixed::default(), n);
        (s.min, s.max, s.unit)
    }

    #[test]
    fn nice_units() {
        assert_eq!(nice_unit(0.7), 1.0);
        assert_eq!(nice_unit(1.3), 2.0);
        assert_eq!(nice_unit(3.0), 5.0);
        assert_eq!(nice_unit(7.0), 10.0);
        assert_eq!(nice_unit(230.0), 500.0);
        assert!((nice_unit(0.023) - 0.05).abs() < 1e-12);
    }

    #[test]
    fn excel_like_ranges() {
        // Excel: 0..100 → 0..120 by 20.
        assert_eq!(s(0.0, 100.0, 6), (0.0, 120.0, 20.0));
        assert_eq!(s(3.0, 95.0, 5), (0.0, 100.0, 20.0));
        // Close together: zero left out.
        let (min, max, unit) = s(950.0, 1000.0, 6);
        assert!(
            min > 0.0 && min <= 925.0 && max >= 1000.0,
            "{min} {max} {unit}"
        );
        // Negative and positive.
        let (min, max, _) = s(-30.0, 70.0, 6);
        assert!(min <= -30.0 && min > -60.0 && (70.0..=100.0).contains(&max));
        // All negative: zero on top.
        assert_eq!(s(-80.0, -10.0, 4).1, 0.0);
        // A single value.
        assert_eq!(s(5.0, 5.0, 5).0, 0.0);
        // Nothing to show.
        assert_eq!(s(f64::NAN, f64::NAN, 5), (0.0, 1.0, 0.2));
        // Excel: 0..1 → 0..1.2 by 0.2.
        assert_eq!(s(0.0, 1.0, 5), (0.0, 1.2, 0.2));
    }

    #[test]
    fn fixed_parts_are_kept() {
        let f = Fixed {
            min: Some(10.0),
            max: None,
            unit: Some(15.0),
        };
        let sc = auto_scale(12.0, 70.0, f, 5);
        assert_eq!((sc.min, sc.unit), (10.0, 15.0));
        assert_eq!(sc.max, 85.0);
        assert_eq!(sc.ticks(), vec![10.0, 25.0, 40.0, 55.0, 70.0, 85.0]);
        let f = Fixed {
            min: None,
            max: Some(1.0),
            unit: None,
        };
        let sc = auto_scale(0.0, 0.9, f, 5);
        assert_eq!((sc.min, sc.max, sc.unit), (0.0, 1.0, 0.2));
        assert_eq!(sc.ticks()[3], 0.6);
    }

    #[test]
    fn interval_counts() {
        assert_eq!(intervals_for(300.0, 12.0), 10);
        assert_eq!(intervals_for(100.0, 12.0), 3);
        assert_eq!(intervals_for(10.0, 12.0), 2);
    }
}
