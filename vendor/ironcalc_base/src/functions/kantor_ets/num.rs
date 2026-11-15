//! Floating-point steps exactly as Excel's FORECAST.ETS code takes them:
//! products and quotients flush subnormal results to zero, comparisons are
//! made on the bit image, rounding is half away from zero.

/// A subnormal result is zero.
pub(crate) fn flush(r: f64) -> f64 {
    if r != 0.0 && r.abs() < f64::MIN_POSITIVE {
        0.0
    } else {
        r
    }
}

pub(crate) fn mul(a: f64, b: f64) -> f64 {
    flush(a * b)
}

pub(crate) fn div(a: f64, b: f64) -> f64 {
    flush(a / b)
}

/// Division that gives 0 for a tiny divisor or a result out of range.
pub(crate) fn div_zero(a: f64, b: f64) -> f64 {
    if b.abs() < f64::MIN_POSITIVE {
        return 0.0;
    }
    let r = a / b;
    if r.abs() > f64::MAX || r.abs() < f64::MIN_POSITIVE {
        0.0
    } else {
        r
    }
}

/// Round half away from zero.
pub(crate) fn round_away(v: f64) -> f64 {
    if v >= 0.0 {
        (v + 0.5).floor()
    } else {
        -(-v + 0.5).floor()
    }
}

/// Negation that leaves values whose top 16 bits are zero unchanged.
pub(crate) fn flip_sign(v: f64) -> f64 {
    if (v.to_bits() >> 48) == 0 {
        v
    } else {
        -v
    }
}

/// Three-way compare on the sign-magnitude integer image: -0.0 sorts below
/// +0.0 and NaN above every number.
pub(crate) fn bit_compare(a: f64, b: f64) -> i32 {
    let ia = a.to_bits() as i64;
    let ib = b.to_bits() as i64;
    if ia == ib {
        return 0;
    }
    let mut r = if ia < ib { -1 } else { 1 };
    if (ia & ib) < 0 {
        r = -r;
    }
    r
}

pub(crate) fn sign_of(v: f64) -> i32 {
    bit_compare(v, 0.0)
}

/// Magnitude truncated to an unsigned 32-bit value, saturating.
pub(crate) fn to_u32(v: f64) -> u32 {
    let v = v.abs();
    if v.is_nan() {
        return 0;
    }
    if v >= 4294967295.0 {
        return u32::MAX;
    }
    v as u32
}

pub(crate) fn top_bits_zero(v: f64) -> bool {
    (v.to_bits() >> 48) == 0
}

pub(crate) fn sign_bit_set(v: f64) -> bool {
    (v.to_bits() as i64) < 0
}

/// Below 2^53 every integer is exact in a double.
pub(crate) const MAX_WHOLE_COUNT: f64 = 9007199254740992.0;

/// `v` truncated toward zero, unless it is non-finite or not below 2^53.
pub(crate) fn whole_count(v: f64) -> Option<i64> {
    if v.abs() < MAX_WHOLE_COUNT {
        Some(v as i64)
    } else {
        None
    }
}
