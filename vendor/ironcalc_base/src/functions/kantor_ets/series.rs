//! Reading the (value, timeline) pairs and putting them on a regular grid as
//! Excel does: cells are paired, sorted when a step is negative, the step is
//! read from a histogram of the differences, and series with gaps or repeated
//! timestamps are rebuilt node by node from the last timestamp backwards (by
//! months on a calendar timeline). Ported from formulon's
//! `eval/ets_series.cpp` (see `mod.rs`).

use crate::expressions::token::Error;

use super::num::{
    bit_compare, div_zero, mul, round_away, sign_bit_set, sign_of, to_u32, top_bits_zero,
    whole_count,
};

const STEP_TOLERANCE: f64 = 1e-5;
/// Percentage of the steps that must fall in the chosen histogram window.
const MIN_WINDOW_PERCENT: i64 = 50;
const MAX_HISTOGRAM_BINS: i64 = 3000;
const MAX_REFINE_PASSES: i64 = 11;
const SAME_STEP_TOLERANCE: f64 = 3e-7;
const MAX_DATE_SERIAL: u32 = 2958465;
/// Longest run of regular steps tracked when anchoring the grid.
const MAX_ANCHOR_RUN: usize = 20;
/// Largest share (percent) of grid nodes that may be missing.
const MAX_MISSING_PERCENT: i64 = 30;
const MAX_GRID_NODES: i64 = 1_000_000;

/// How values sharing a timestamp are combined (option 1–7).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Aggregation {
    Average,
    Count,
    CountA,
    Max,
    Median,
    Min,
    Sum,
}

impl Aggregation {
    pub(crate) fn from_option(v: i64) -> Option<Aggregation> {
        Some(match v {
            1 => Aggregation::Average,
            2 => Aggregation::Count,
            3 => Aggregation::CountA,
            4 => Aggregation::Max,
            5 => Aggregation::Median,
            6 => Aggregation::Min,
            7 => Aggregation::Sum,
            _ => return None,
        })
    }

    fn counts(self) -> bool {
        matches!(self, Aggregation::Count | Aggregation::CountA)
    }
}

/// One input cell, as far as the series reader tells them apart.
#[derive(Clone, Debug)]
pub(crate) enum Cell {
    Number(f64),
    Blank,
    Text,
    Bool,
    Error(Error),
}

/// Timeline step and calendar class.
#[derive(Clone, Debug, Default)]
pub(crate) struct Timeline {
    /// Step between grid nodes (days, or the histogram step for calendar data).
    pub step: f64,
    pub is_date: bool,
    /// Months per node on a calendar timeline, otherwise 0.
    pub months: i64,
    /// Number of steps in the chosen histogram window.
    pub count: i64,
    /// Timestamp of the last grid node.
    pub last: f64,
}

/// The regular series a fit runs on.
pub(crate) struct Series {
    pub y: Vec<f64>,
    pub timeline: Timeline,
}

// -- calendar -----------------------------------------------------------

fn floor_div(a: i64, b: i64) -> i64 {
    let mut q = a / b;
    if a % b != 0 && ((a < 0) != (b < 0)) {
        q -= 1;
    }
    q
}

/// Days since 1970-01-01 of a proleptic Gregorian date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = floor_div(y, 400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> Ymd {
    let z = z + 719468;
    let era = floor_div(z, 146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    Ymd { year, month, day }
}

/// Serial 0 is 1899-12-30.
fn epoch_days() -> i64 {
    days_from_civil(1899, 12, 30)
}

fn is_date_serial(x: f64) -> bool {
    sign_of(x) != -1 && to_u32(x) <= MAX_DATE_SERIAL
}

#[derive(Clone, Copy)]
struct Ymd {
    year: i64,
    month: i64,
    day: i64,
}

fn ymd_of(serial: f64) -> Ymd {
    let days = whole_count(serial.floor()).unwrap_or(0);
    civil_from_days(days + epoch_days())
}

/// A day past the month end rolls into the next month.
fn serial_of(v: &Ymd) -> f64 {
    (days_from_civil(v.year, v.month, 1) - epoch_days() + (v.day - 1)) as f64
}

/// Moves (year, month) in place, keeping the day.
fn add_months(v: &mut Ymd, delta: i64) -> f64 {
    let total = v.year * 12 + (v.month + delta) - 1;
    v.year = floor_div(total, 12);
    v.month = total - v.year * 12 + 1;
    serial_of(v)
}

/// Nearest month-step node from `a` toward `b`, and the signed step count.
fn month_walk(months: i64, a: f64, b: f64) -> Result<(f64, i64), Error> {
    let (delta, sign) = if bit_compare(a, b) == -1 {
        (months, 1)
    } else {
        (-months, -1)
    };
    if months <= 0 {
        return Err(Error::VALUE);
    }
    let mut cnt = 0;
    let mut cur = a;
    let mut ymd = ymd_of(a);
    let mut d = (a - b).abs();
    loop {
        let next = add_months(&mut ymd, delta);
        let d2 = (next - b).abs();
        if bit_compare(d2, d) >= 0 {
            break;
        }
        cur = next;
        cnt += sign;
        d = d2;
    }
    Ok((cur, cnt))
}

// -- ordering -----------------------------------------------------------

/// Sorts `keys[lo..=hi]` ascending, permuting `vals` alongside. Up to eight
/// elements a selection pass, larger runs a median-of-three quicksort. The
/// order of equal keys fixes the summation order of grouped values.
fn sort_pairs(k: &mut [f64], mut vals: Option<&mut [f64]>, mut lo: i64, mut hi: i64) {
    let cmp = |k: &[f64], i: i64, j: i64| bit_compare(k[i as usize], k[j as usize]);
    let swap_at = |k: &mut [f64], vals: &mut Option<&mut [f64]>, i: i64, j: i64| {
        k.swap(i as usize, j as usize);
        if let Some(v) = vals.as_deref_mut() {
            v.swap(i as usize, j as usize);
        }
    };
    let mut stack: Vec<(i64, i64)> = Vec::new();
    loop {
        let size = hi - lo + 1;
        if size <= 8 {
            while hi > lo {
                let mut mx = lo;
                for p in lo + 1..=hi {
                    if cmp(k, p, mx) > 0 {
                        mx = p;
                    }
                }
                if mx != hi {
                    swap_at(k, &mut vals, mx, hi);
                }
                hi -= 1;
            }
        } else {
            let mut mid = lo + size / 2;
            if cmp(k, lo, mid) > 0 {
                swap_at(k, &mut vals, lo, mid);
            }
            if cmp(k, lo, hi) > 0 {
                swap_at(k, &mut vals, lo, hi);
            }
            if cmp(k, mid, hi) > 0 {
                swap_at(k, &mut vals, mid, hi);
            }
            let mut loguy = lo;
            let mut higuy = hi;
            loop {
                if mid > loguy {
                    loguy += 1;
                    while loguy < mid && cmp(k, loguy, mid) <= 0 {
                        loguy += 1;
                    }
                }
                if mid <= loguy {
                    loguy += 1;
                    while loguy <= hi && cmp(k, loguy, mid) <= 0 {
                        loguy += 1;
                    }
                }
                higuy -= 1;
                while higuy > mid && cmp(k, higuy, mid) > 0 {
                    higuy -= 1;
                }
                if higuy < loguy {
                    break;
                }
                swap_at(k, &mut vals, loguy, higuy);
                if higuy == mid {
                    mid = loguy;
                }
            }
            higuy += 1;
            if mid < higuy {
                higuy -= 1;
                while higuy > mid && cmp(k, higuy, mid) == 0 {
                    higuy -= 1;
                }
            }
            if mid >= higuy {
                higuy -= 1;
                while higuy > lo && cmp(k, higuy, mid) == 0 {
                    higuy -= 1;
                }
            }
            if higuy - lo >= hi - loguy {
                if lo < higuy {
                    stack.push((lo, higuy));
                }
                if loguy < hi {
                    lo = loguy;
                    continue;
                }
            } else {
                if loguy < hi {
                    stack.push((loguy, hi));
                }
                if lo < higuy {
                    hi = higuy;
                    continue;
                }
            }
        }
        match stack.pop() {
            Some((l, h)) => {
                lo = l;
                hi = h;
            }
            None => break,
        }
    }
}

// -- reading ------------------------------------------------------------

/// Statistics of the consecutive timeline differences.
struct StepStats {
    min: f64,
    max: f64,
    /// Non-positive steps (repeats, and negative steps when `needs_sort`).
    nonpositive: i64,
    needs_sort: bool,
}

impl StepStats {
    fn new() -> StepStats {
        StepStats {
            min: f64::MAX,
            max: 0.0,
            nonpositive: 0,
            needs_sort: false,
        }
    }

    fn add(&mut self, d: f64) {
        if bit_compare(d, self.min) == -1 {
            if top_bits_zero(d) {
                self.nonpositive += 1;
            } else if d.is_nan() || d.is_infinite() {
                // not counted
            } else if sign_bit_set(d) {
                self.nonpositive += 1;
                self.needs_sort = true;
            } else {
                self.min = d;
            }
        }
        if bit_compare(d, self.max) == 1 {
            self.max = d;
        }
    }
}

/// Pairs the cells in order. A pair whose timeline or value cell is blank,
/// text or a boolean is dropped, except that errors propagate, text values
/// count as 1 under COUNTA and blank values are 0 without interpolation.
/// Under COUNT/COUNTA every kept value is 1.
fn read_pairs(
    values: &[Cell],
    times: &[Cell],
    completion: i64,
    agg: Aggregation,
) -> Result<(Vec<f64>, Vec<f64>, StepStats), Error> {
    if values.len() != times.len() {
        return Err(Error::NA);
    }
    let mut t = Vec::new();
    let mut y = Vec::new();
    let mut stats = StepStats::new();
    for (tc, yc) in times.iter().zip(values) {
        if let Cell::Error(e) = tc {
            return Err(e.clone());
        }
        if let Cell::Error(e) = yc {
            return Err(e.clone());
        }
        let Cell::Number(tn) = tc else {
            continue;
        };
        let yv = match yc {
            Cell::Blank if completion == 0 => {
                if agg.counts() {
                    1.0
                } else {
                    0.0
                }
            }
            Cell::Text => {
                if agg != Aggregation::CountA {
                    continue;
                }
                1.0
            }
            Cell::Number(n) => {
                if agg.counts() {
                    1.0
                } else {
                    *n
                }
            }
            _ => continue,
        };
        if let Some(&last) = t.last() {
            stats.add(tn - last);
        }
        t.push(*tn);
        y.push(yv);
    }
    Ok((t, y, stats))
}

// -- step detection -----------------------------------------------------

#[derive(Default)]
struct Histogram {
    counts: Vec<i64>,
    reps: Vec<f64>,
    same: Vec<i64>,
    nbins: i64,
    zeros: i64,
}

/// Do [a, b] and [lo, hi] intersect (inclusive)?
fn ranges_overlap(a: f64, b: f64, lo: f64, hi: f64) -> bool {
    let pos = |v: f64| {
        if bit_compare(v, a) == -1 {
            -1
        } else if bit_compare(v, b) == 1 {
            1
        } else {
            0
        }
    };
    let pa = pos(lo);
    let pb = pos(hi);
    if pa != pb {
        return true;
    }
    pa == 0
}

/// Number of steps whose bins intersect [lo, hi].
fn count_in(low: f64, bw: f64, counts: &[i64], nbins: i64, lo: f64, hi: f64) -> i64 {
    let index = |v: f64| -> i64 {
        let q = div_zero(v - low, bw);
        if !top_bits_zero(q) && !(q.is_nan() || q.is_infinite()) && sign_bit_set(q) {
            return -1;
        }
        to_u32(q) as i64
    };
    let i0 = index(lo);
    let i1 = index(hi);
    let start = if i1 < 0 { i0 } else { i0.max(0) };
    let last = if start < nbins && i1 >= nbins {
        nbins - 1
    } else {
        i1
    };
    let first = start.max(0);
    if first > last || first >= nbins {
        return 0;
    }
    let mut sum = 0;
    let mut j = first;
    while j < nbins {
        sum += counts[j as usize];
        if j >= last {
            break;
        }
        j += 1;
    }
    sum
}

/// Steps of about k years.
fn classify_multiyear(tl: &mut Timeline, lo: f64, hi: f64) {
    let k = to_u32(round_away(div_zero(lo, 365.0))) as i64;
    if !(2..=100).contains(&k) {
        return;
    }
    let nominal = k * 365 + (k >> 2);
    let mut tol = 2;
    let mut v = k;
    while v > 1 {
        tol += 1;
        v >>= 1;
    }
    if ranges_overlap(lo, hi, (nominal - tol) as f64, (nominal + tol) as f64) {
        tl.months = 12 * k;
    }
}

/// Calendar class of a constant step; `m` is the number of steps.
fn classify_constant(tl: &mut Timeline, step: f64, m: i64) {
    tl.step = step;
    tl.months = 0;
    tl.count = m;
    if !tl.is_date {
        return;
    }
    if ranges_overlap(step, step, 30.0, 31.0) {
        let lim = if step == 29.5 {
            2
        } else {
            2 + i64::from(step > 29.5)
        };
        if m > lim {
            return;
        }
        tl.months = 1;
    } else if ranges_overlap(step, step, 363.0, 367.0) {
        tl.months = 12;
    } else if ranges_overlap(step, step, 90.0, 92.0) {
        let lim = if step == 90.5 {
            2
        } else {
            2 + i64::from(step > 90.5)
        };
        if m > lim {
            return;
        }
        tl.months = 3;
    } else if ranges_overlap(step, step, 181.5, 184.5) {
        if m > 2 {
            return;
        }
        tl.months = 6;
    } else if bit_compare(step, 365.0) == 1 {
        classify_multiyear(tl, step, step);
    }
}

/// Calendar class of the chosen histogram bin [lo, hi]; `rep` is the common
/// step of a pure bin.
#[allow(clippy::too_many_arguments)]
fn classify_bin(
    tl: &mut Timeline,
    lo: f64,
    hi: f64,
    rep: Option<f64>,
    low: f64,
    bw: f64,
    counts: &[i64],
    nbins: i64,
) {
    tl.step = rep.unwrap_or(lo);
    tl.months = 0;
    if !tl.is_date {
        return;
    }
    let cnt = |a: f64, b: f64| count_in(low, bw, counts, nbins, a, b);
    if ranges_overlap(lo, hi, 30.0, 31.0) {
        let c1 = cnt(27.5, 31.5);
        if c1 < 2 {
            tl.months = 1;
            return;
        }
        let c2 = cnt(27.5, 29.5);
        let c3 = cnt(29.5, 30.5);
        let c4 = cnt(30.5, 31.5);
        let c5 = cnt(29.5, 31.5);
        if c4 == 0 && (c3 > 2 || c2 > 2) {
            return;
        }
        if c3 == 0 {
            if c4 > 4 || c2 > 2 {
                return;
            }
            tl.months = 1;
            return;
        }
        if c2 < 3 || c5 > 9 || c5 + c2 != c1 {
            tl.months = 1;
        }
        return;
    }
    if ranges_overlap(lo, hi, 363.0, 367.0) {
        tl.months = 12;
        return;
    }
    if ranges_overlap(lo, hi, 90.0, 92.0) {
        let c1 = cnt(89.5, 92.5);
        if c1 < 2 {
            tl.months = 3;
            return;
        }
        let c2 = cnt(89.5, 90.5);
        let c3 = cnt(90.5, 91.5);
        let c4 = cnt(91.5, 92.5);
        let c5 = cnt(90.5, 92.5);
        if c3 == 0 && (c4 > 3 || c2 > 2) {
            return;
        }
        if c4 == 0 {
            if c3 > 3 || c2 > 2 {
                return;
            }
            tl.months = 3;
            return;
        }
        if c2 < 3 || c5 > 1 {
            tl.months = 3;
            return;
        }
        if c5 + c2 == c1 {
            return;
        }
        tl.months = 3;
        return;
    }
    if ranges_overlap(lo, hi, 181.5, 184.5) {
        let c1 = cnt(89.5, 92.5);
        if c1 < 2 {
            tl.months = 6;
            return;
        }
        let c2 = cnt(180.5, 181.5);
        let c3 = cnt(181.5, 182.5);
        let c4 = cnt(183.5, 184.5);
        if c2 == 0 && (c4 > 2 || c3 > 2) {
            return;
        }
        if c4 == 0 {
            if c2 > 2 || c3 > 2 {
                return;
            }
            tl.months = 6;
            return;
        }
        if c3 < 3 {
            tl.months = 6;
            return;
        }
        let s = c4 + c2;
        if s + c3 != c1 {
            tl.months = 6;
            return;
        }
        if s < 6 {
            return;
        }
        tl.months = 6;
        return;
    }
    if bit_compare(lo, 365.0) == 1 {
        classify_multiyear(tl, lo, hi);
    }
}

struct Binning {
    bw: f64,
    nbins: i64,
    bw0: f64,
    capped: bool,
}

/// Histogram geometry for the full range of steps.
fn make_binning(st: &StepStats) -> Binning {
    let mut base = if st.min == 0.0 {
        STEP_TOLERANCE
    } else if bit_compare(STEP_TOLERANCE, st.min) == 1 && bit_compare(STEP_TOLERANCE, st.max) == -1
    {
        STEP_TOLERANCE
    } else {
        st.min
    };
    base = div_zero(base, 5.0);
    let bw0 = div_zero(base, 5.0);
    let range = st.max - st.min;
    let q = div_zero(range, bw0) + 1.0;
    if bit_compare(q, MAX_HISTOGRAM_BINS as f64) == 1 {
        return Binning {
            bw: div_zero(range, (MAX_HISTOGRAM_BINS - 1) as f64),
            nbins: MAX_HISTOGRAM_BINS,
            bw0,
            capped: true,
        };
    }
    let nb = to_u32(q);
    let bw = div_zero(range, nb as f64);
    let q2 = div_zero(range, bw) + 1.5;
    Binning {
        bw,
        nbins: to_u32(q2) as i64,
        bw0: bw,
        capped: false,
    }
}

fn fill_histogram(
    h: &mut Histogram,
    t: &[f64],
    nbins: i64,
    bw: f64,
    low: f64,
    high: f64,
    bounded: bool,
) {
    let nb = nbins.max(0) as usize;
    h.counts = vec![0; nb];
    h.reps = vec![0.0; nb];
    h.same = vec![0; nb];
    h.nbins = nbins;
    h.zeros = 0;
    for i in 1..t.len() {
        let d = t[i] - t[i - 1];
        if top_bits_zero(d) || d.is_nan() || d.is_infinite() {
            h.zeros += 1;
            continue;
        }
        if bounded && (bit_compare(d, low) == -1 || bit_compare(d, high) == 1) {
            continue;
        }
        let q = div_zero(d - low, bw);
        let mut b = to_u32(q) as i64;
        if b >= nbins {
            b = nbins - 1;
        }
        let b = b as usize;
        let c = h.counts[b];
        h.counts[b] = c + 1;
        if c == 0 {
            h.reps[b] = d;
            h.same[b] = 1;
        } else if (h.reps[b] - d).abs() <= SAME_STEP_TOLERANCE {
            h.same[b] += 1;
        }
    }
}

/// Picks the step bin: a sliding window over wide histograms, the largest
/// bin otherwise. True when the bin is pure.
fn choose_bin(h: &Histogram, low: f64, bw: f64, tl: &mut Timeline) -> bool {
    let counts = &h.counts;
    let nbins = h.nbins;
    let c = |i: i64| counts[i as usize];
    let b;
    let wtotal;
    if nbins <= 4 {
        let mut total = 0;
        for i in 0..nbins {
            total += c(i);
        }
        let mut best_i = 0;
        if nbins >= 1 && total > 0 {
            for i in 1..nbins {
                if c(i) > c(best_i) {
                    best_i = i;
                }
            }
        }
        b = best_i;
        wtotal = if nbins >= 1 { total } else { 0 };
    } else {
        let scaled_low = div_zero(low, bw);
        let mut w8 = 0;
        let mut w11 = 0;
        let mut w10 = 0;
        let mut w12: i64 = 0;
        let mut w13: i64 = 1;
        let mut w14: i64 = 10;
        let mut acc = c(0);
        let mut best = c(0);
        loop {
            if w13 < nbins {
                acc += c(w13);
            }
            let mid = (w12 + w13) / 2;
            let thr = (w14 - (5 * w12 + mid)) as f64;
            if bit_compare(scaled_low, thr) == -1 {
                if w12 >= 0 {
                    acc -= c(w12);
                }
                w12 += 1;
            } else {
                w12 -= 1;
                if w12 >= 0 {
                    acc += c(w12);
                }
            }
            let mid2 = (w12 + w13) / 2;
            if mid2 >= nbins {
                break;
            }
            if best < acc || (best == acc && c(w8) < c(mid2)) {
                best = acc;
                w8 = mid2;
                w11 = w12;
                w10 = w13;
            }
            w13 += 1;
            w14 += 5;
        }
        let mut w28 = w11.max(0);
        let mut w27 = w10.min(nbins - 1);
        let mut w25 = 0;
        let mut w9 = 0;
        if w28 <= w27 {
            w25 = w28;
            w9 = c(w28);
            for i in w28 + 1..=w27 {
                if w9 < c(i) {
                    w25 = i;
                    w9 = c(i);
                }
            }
        }
        if w25 != w8 && !(w9 < best - 1 && 15 * best <= 100 * (best - w9)) {
            let mut v = mul(w25 as f64, bw) + low;
            v = div_zero(v, 10.0);
            v = div_zero(v, bw);
            let r = to_u32(v) as i64;
            w28 = (w25 - r).max(0);
            w27 = (w25 + r).min(nbins - 1);
            let mut total = 0;
            for i in w28..=w27 {
                total += c(i);
            }
            wtotal = total;
            b = w25;
        } else {
            b = if w25 != w8 { w8 } else { w25 };
            wtotal = best;
        }
    }
    let mut rep = None;
    if c(b) >= 1 && h.same[b as usize] == c(b) {
        rep = Some(h.reps[b as usize]);
    }
    let lo = mul(b as f64, bw) + low;
    let hi = lo + bw;
    classify_bin(tl, lo, hi, rep, low, bw, counts, nbins);
    tl.count = wtotal;
    rep.is_some()
}

/// 4 · max(0.4 a, b).
fn window_width(a: f64, b: f64) -> f64 {
    let mut v = div_zero(a, 5.0);
    v = mul(v, 2.0);
    if bit_compare(v, b) == -1 {
        v = b;
    }
    mul(v, 4.0)
}

struct Refined {
    low: f64,
    high: f64,
    bw: f64,
    nbins: i64,
    capped: bool,
}

/// A narrower histogram around the current step estimate.
fn refine_histogram(st: &StepStats, bw: f64, mut bw0: f64, step: f64) -> Refined {
    let lowbase = st.min;
    let width = window_width(step, bw);
    let mut first = true;
    loop {
        let v = div_zero(width, bw0);
        if bit_compare(v, 2900.0) == -1 {
            break;
        }
        bw0 = mul(
            bw0,
            if bit_compare(v, 48000.0) == 1 {
                16.0
            } else {
                2.0
            },
        );
        first = false;
    }
    let mut newlow = step;
    let v = div_zero(step - lowbase, bw0);
    if bit_compare(v, -1.0) != -1 {
        let n0 = to_u32(v);
        newlow = mul(n0 as f64, bw0) + lowbase;
    }
    let w2 = window_width(newlow, bw);
    let k = to_u32(div_zero(w2, bw0));
    let half = div_zero(mul(k as f64, bw0), 2.0);
    let mut low = newlow - half;
    let mut high = newlow + half;
    if bit_compare(low, lowbase) == -1 {
        low = lowbase;
    }
    if bit_compare(high, st.max) == 1 {
        high = st.max;
    }
    Refined {
        low,
        high,
        nbins: to_u32(div_zero(high - low, bw0) + 1.0) as i64,
        bw: bw0,
        capped: first && bit_compare(step, bw0) == 1,
    }
}

/// Detects the step; the flag says the series has gaps or repeated
/// timestamps and must be rebuilt.
fn detect_step(t: &[f64], st: &StepStats, tl: &mut Timeline) -> Result<bool, Error> {
    let n = t.len() as i64;
    tl.is_date = is_date_serial(t[0]) && is_date_serial(t[t.len() - 1]);
    tl.months = if tl.is_date { -1 } else { 0 };
    if st.min == st.max || (st.nonpositive != 0 && st.max == 0.0) {
        if st.max == 0.0 {
            return Err(Error::NUM);
        }
        classify_constant(tl, st.max, n - st.nonpositive - 1);
        return Ok(st.nonpositive != 0);
    }
    let bin = make_binning(st);
    let mut bw = bin.bw;
    let mut nbins = bin.nbins;
    let mut bw0 = bin.bw0;
    let mut capped = bin.capped;
    let mut low = st.min;
    let mut h = Histogram::default();
    fill_histogram(&mut h, t, nbins, bw, low, st.max, false);
    let mut found = choose_bin(&h, low, bw, tl);
    if !found && capped {
        let mut pass = 2;
        loop {
            let r = refine_histogram(st, bw, bw0, tl.step);
            low = r.low;
            bw = r.bw;
            nbins = r.nbins;
            capped = r.capped;
            fill_histogram(&mut h, t, nbins, bw, low, r.high, true);
            found = choose_bin(&h, low, bw, tl);
            if found || !capped {
                break;
            }
            if pass >= MAX_REFINE_PASSES {
                return Err(Error::NUM);
            }
            bw0 = bw;
            pass += 1;
        }
    }
    let irregular = tl.count != n - 1;
    let m = n - h.zeros;
    if tl.count * 100 <= (m - 1) * MIN_WINDOW_PERCENT {
        return Err(Error::NUM);
    }
    Ok(irregular)
}

// -- grid ---------------------------------------------------------------

struct Grid<'a> {
    tl: &'a Timeline,
    step: f64,
    agg: Aggregation,
    count_mode: bool,
    tol: f64,
    half: f64,
}

/// Maximum number of grid nodes.
fn grid_capacity(n: i64, first: f64, last: f64, step: f64) -> i64 {
    let span = div_zero(last - first, step);
    let by_count = (50 * n) / 35;
    let c = span.ceil();
    let mut w8 = if bit_compare(c, MAX_GRID_NODES as f64) == 1 {
        by_count.max(MAX_GRID_NODES)
    } else {
        to_u32(c) as i64 + 1
    };
    w8 += if w8 > 54 { w8 / 5 } else { 10 };
    let mut w9 = by_count.min(w8);
    if w8 >= MAX_GRID_NODES {
        w9 = by_count;
    }
    w9
}

/// Aggregates the points from `end` backwards while t ≥ `lower`: the value
/// and the index of the group's first point.
fn aggregate_group(
    g: &Grid,
    lower: f64,
    end: i64,
    t: &[f64],
    y: &mut [f64],
) -> Result<(f64, i64), Error> {
    let tt = |i: i64| t[i as usize];
    if end <= 0 || bit_compare(lower, tt(end - 1)) == 1 {
        let v = if g.count_mode { 1.0 } else { y[end as usize] };
        return Ok((v, end));
    }
    let mut j = end;
    let value = match g.agg {
        Aggregation::Average | Aggregation::Sum => {
            let mut acc = y[end as usize];
            let mut cnt = 1;
            while j > 0 {
                if bit_compare(lower, tt(j - 1)) == 1 {
                    break;
                }
                j -= 1;
                acc += y[j as usize];
                cnt += 1;
            }
            if g.agg == Aggregation::Average {
                div_zero(acc, cnt as f64)
            } else {
                acc
            }
        }
        Aggregation::Count | Aggregation::CountA => {
            let mut cnt = 1;
            while j > 0 {
                if bit_compare(lower, tt(j - 1)) == 1 {
                    break;
                }
                j -= 1;
                cnt += 1;
            }
            cnt as f64
        }
        Aggregation::Max | Aggregation::Min => {
            let want = if g.agg == Aggregation::Max { -1 } else { 1 };
            let mut val = y[end as usize];
            while j > 0 {
                if bit_compare(lower, tt(j - 1)) == 1 {
                    break;
                }
                j -= 1;
                if bit_compare(val, y[j as usize]) == want {
                    val = y[j as usize];
                }
            }
            val
        }
        Aggregation::Median => {
            while j > 0 && bit_compare(lower, tt(j - 1)) != 1 {
                j -= 1;
            }
            let cnt = end - j + 1;
            if cnt > 1 {
                sort_pairs(y, None, j, end);
            }
            if cnt & 1 != 0 {
                y[(j + (cnt >> 1)) as usize]
            } else {
                let m = (j + (cnt >> 1)) as usize;
                div_zero(y[m] + y[m - 1], 2.0)
            }
        }
    };
    Ok((value, j))
}

/// Index of the first of a run of equal timestamps ending at `j`.
fn group_start(t: &[f64], mut j: i64) -> i64 {
    while j > 0 && t[(j - 1) as usize] == t[j as usize] {
        j -= 1;
    }
    j
}

/// Index of the point nearest `node` among the groups from `j` backwards,
/// and its distance.
fn nearest_group(t: &[f64], mut j: i64, floor_index: i64, node: f64) -> (i64, f64) {
    let mut bestd = f64::MAX;
    let mut at = j;
    while j >= floor_index {
        let d = (node - t[j as usize]).abs();
        if bit_compare(d, bestd) >= 0 {
            break;
        }
        bestd = d;
        at = j;
        if j == 0 {
            break;
        }
        j = group_start(t, j - 1);
    }
    (at, bestd)
}

struct Node {
    node: f64,
    steps: i64,
    value: f64,
    first: i64,
    stored: f64,
}

/// The last grid node, anchored on the run of regular steps that covers most
/// of the series.
fn first_node(g: &Grid, t: &[f64], y: &mut [f64]) -> Result<Node, Error> {
    let last = t.len() as i64 - 1;
    let tt = |i: i64| t[i as usize];
    let x23 = group_start(t, last);
    let mut maxk: usize = 0;
    let mut table = [-1i64; MAX_ANCHOR_RUN + 1];
    let record =
        |table: &mut [i64; MAX_ANCHOR_RUN + 1], maxk: &mut usize, run: usize, runstart: i64| {
            let k = run.min(MAX_ANCHOR_RUN);
            if table[k] < 0 {
                table[k] = runstart;
            }
            if k > *maxk {
                *maxk = k;
            }
        };
    if x23 > 0 {
        let mut run: usize = 0;
        let mut runstart: i64 = -1;
        let mut cur = x23;
        loop {
            let prev = group_start(t, cur - 1);
            let d = ((tt(cur) - tt(prev)) - g.step).abs();
            if bit_compare(d, g.tol) == 1 {
                if run >= 2 && run > maxk {
                    record(&mut table, &mut maxk, run, runstart);
                }
                runstart = -1;
                run = 0;
            } else if run == 0 {
                run = 1;
                runstart = cur;
            } else {
                run += 1;
                if run >= MAX_ANCHOR_RUN {
                    if table[MAX_ANCHOR_RUN] < 0 {
                        table[MAX_ANCHOR_RUN] = runstart;
                    }
                    if maxk < MAX_ANCHOR_RUN {
                        maxk = MAX_ANCHOR_RUN;
                    }
                    runstart = -1;
                }
            }
            cur = prev;
            if prev <= 0 {
                break;
            }
        }
        if run >= 2 {
            if run > maxk {
                record(&mut table, &mut maxk, run, runstart);
            }
        } else if run == 1 {
            if maxk == 0 {
                if table[2] < 0 {
                    table[2] = runstart;
                }
                maxk = 2;
            }
        } else if maxk == 0 {
            return Err(Error::NUM);
        }
    } else if maxk == 0 {
        return Err(Error::NUM);
    }
    let span = tt(last) - t[0];
    let mut best = 0.0;
    let mut anchor = -1;
    let mut k = maxk;
    while k >= 2 {
        let p = table[k];
        if p >= 0 {
            let mut v = div_zero(tt(last) - tt(p), span);
            v = 1.0 - v;
            v = mul(v, k as f64);
            if bit_compare(v, best) == 1 {
                best = v;
                anchor = p;
            }
        }
        k -= 1;
    }
    if anchor < 0 {
        return Err(Error::VALUE);
    }
    let lastg = group_start(t, last);
    let mut stored = None;
    let mut cur_t;
    if lastg == anchor {
        cur_t = tt(last);
    } else {
        if g.tl.months >= 1 {
            cur_t = month_walk(g.tl.months, tt(anchor), tt(lastg))?.0;
        } else {
            let mut v = div_zero(tt(last) - tt(anchor), g.step);
            v = round_away(v);
            v = mul(v, g.step);
            cur_t = v + tt(anchor);
        }
        let (at, bestd) = nearest_group(t, lastg, anchor, cur_t);
        if bit_compare(bestd, g.tol) == 1 {
            stored = Some(tt(at));
        } else {
            cur_t = tt(at);
        }
    }
    let lower = cur_t - g.half;
    let (value, first) = aggregate_group(g, lower, last, t, y)?;
    Ok(Node {
        node: cur_t,
        steps: 0,
        value,
        first,
        stored: stored.unwrap_or(cur_t),
    })
}

/// The node before `cur_t`, with the number of steps between them.
fn previous_node(g: &Grid, t: &[f64], y: &mut [f64], ptr: i64, cur_t: f64) -> Result<Node, Error> {
    let x22 = group_start(t, ptr - 1);
    let t_prev = t[x22 as usize];
    let mut node;
    let steps;
    if g.tl.months >= 1 {
        let (n, cnt) = month_walk(g.tl.months, cur_t, t_prev)?;
        node = n;
        steps = -cnt;
    } else {
        let tmp = cur_t - g.step;
        let lower = tmp - g.half;
        if bit_compare(lower, t_prev) != 1 {
            steps = 1;
            node = tmp;
        } else {
            let mut v = div_zero(cur_t - t_prev, g.step);
            v = round_away(v);
            steps = to_u32(v) as i64;
            v = mul(v, g.step);
            node = cur_t - v;
        }
    }
    let (at, bestd) = nearest_group(t, x22, 0, node);
    let mut stored = None;
    if bit_compare(bestd, g.tol) == 1 {
        stored = Some(t[at as usize]);
    } else {
        node = t[at as usize];
    }
    let lower2 = node - g.half;
    let (value, first) = aggregate_group(g, lower2, ptr - 1, t, y)?;
    Ok(Node {
        node,
        steps,
        value,
        first,
        stored: stored.unwrap_or(node),
    })
}

/// Rebuilds the series on a regular grid, filling skipped nodes as the data
/// completion option says: (times, values, filled nodes).
fn build_grid(
    t: &[f64],
    y: &mut [f64],
    tl: &Timeline,
    completion: i64,
    agg: Aggregation,
) -> Result<(Vec<f64>, Vec<f64>, i64), Error> {
    let n = t.len() as i64;
    let g = Grid {
        tl,
        step: tl.step,
        agg,
        count_mode: agg.counts(),
        tol: div_zero(tl.step, 10.0),
        half: div_zero(tl.step, 2.0),
    };
    let cap = grid_capacity(n, t[0], t[t.len() - 1], g.step);
    let mut room = cap - 1;
    let mut missing = 0;
    let mut out_t = Vec::new();
    let mut out_y = Vec::new();
    let mut append = |time: f64, value: f64, real: bool| -> Result<(), Error> {
        if room < 0 || room > cap {
            return Err(Error::NUM);
        }
        out_t.push(time);
        out_y.push(value);
        if !real {
            missing += 1;
        }
        room -= 1;
        Ok(())
    };
    let start = first_node(&g, t, y)?;
    let mut cur_t = start.node;
    let mut cur_y = start.value;
    let mut ptr = start.first;
    append(start.stored, cur_y, true)?;
    while ptr > 0 {
        let p = previous_node(&g, t, y, ptr, cur_t)?;
        if p.steps >= 2 {
            if tl.months > 0 {
                let kd = p.steps as f64;
                let mut ymd = ymd_of(cur_t);
                let mut gcount = p.steps;
                while gcount >= 2 {
                    gcount -= 1;
                    let node_t = add_months(&mut ymd, -tl.months);
                    let mut value = 0.0;
                    if completion == 1 {
                        let diff = cur_y - p.value;
                        let mut v = gcount as f64;
                        v = mul(v, diff);
                        v = div_zero(v, kd);
                        value = v + p.value;
                    }
                    append(node_t, value, false)?;
                }
            } else {
                let span = cur_t - p.node;
                let mut gcount = p.steps;
                let mut node_t = cur_t;
                while gcount >= 2 {
                    node_t -= g.step;
                    let mut value = 0.0;
                    if completion == 1 {
                        let diff = cur_y - p.value;
                        let mut v = node_t - p.node;
                        v = mul(v, diff);
                        v = div_zero(v, span);
                        value = v + p.value;
                    }
                    append(node_t, value, false)?;
                    gcount -= 1;
                }
            }
        }
        append(p.stored, p.value, true)?;
        cur_t = p.node;
        cur_y = p.value;
        ptr = p.first;
    }
    out_t.reverse();
    out_y.reverse();
    Ok((out_t, out_y, missing))
}

/// Reads, sorts and regularises the series.
pub(crate) fn prepare_series(
    values: &[Cell],
    timeline: &[Cell],
    completion: i64,
    agg: Aggregation,
) -> Result<Series, Error> {
    let (mut t, mut y, mut stats) = read_pairs(values, timeline, completion, agg)?;
    if t.len() <= 1 {
        return Err(Error::DIV);
    }
    if stats.needs_sort {
        let hi = t.len() as i64 - 1;
        sort_pairs(&mut t, Some(&mut y), 0, hi);
        stats = StepStats::new();
        for i in 1..t.len() {
            stats.add(t[i] - t[i - 1]);
        }
        if stats.needs_sort {
            return Err(Error::VALUE);
        }
    }
    let mut tl = Timeline::default();
    let irregular = detect_step(&t, &stats, &mut tl)?;
    let (t2, y2) = if irregular {
        let (t2, y2, missing) = build_grid(&t, &mut y, &tl, completion, agg)?;
        if MAX_MISSING_PERCENT * (t2.len() as i64) < 100 * missing {
            return Err(Error::NUM);
        }
        (t2, y2)
    } else {
        (t, y)
    };
    if t2.len() <= 1 {
        return Err(Error::DIV);
    }
    tl.last = t2[t2.len() - 1];
    Ok(Series {
        y: y2,
        timeline: tl,
    })
}

/// Where `target` falls on the grid: a (possibly fractional) number of steps
/// after the last node, and whether it lies between two steps.
pub(crate) fn target_position(tl: &Timeline, target: f64) -> Result<(f64, bool), Error> {
    if tl.step == 0.0 {
        return Err(Error::VALUE);
    }
    if tl.months >= 1 {
        if whole_count(target.floor()).is_none() {
            return Err(Error::NUM);
        }
        let mut ymd = ymd_of(tl.last);
        let at = ymd_of(target);
        let mut k = floor_div(
            (at.year - ymd.year) * 12 + (at.month - ymd.month),
            tl.months,
        );
        let mut node = add_months(&mut ymd, tl.months * k);
        while bit_compare(node, target) == 1 {
            k -= 1;
            node = add_months(&mut ymd, -tl.months);
        }
        if node == target {
            return Ok((k as f64, false));
        }
        let mut extra = 0;
        let mut hi = node;
        while bit_compare(hi, target) == -1 {
            hi = add_months(&mut ymd, tl.months);
            extra -= 1;
        }
        if hi == target {
            return Ok(((k - extra) as f64, false));
        }
        if extra != -1 {
            k += -extra - 1;
            node = add_months(&mut ymd, -tl.months);
        }
        let span = hi - node;
        let v = div_zero(target - node, span);
        return Ok((v + k as f64, true));
    }
    let pos = div_zero(target - tl.last, tl.step);
    let r = round_away(pos);
    if r == pos {
        return Ok((pos, false));
    }
    let d = mul((r - pos).abs(), 1000.0);
    if bit_compare(d, 1.0) == 1 {
        return Ok((pos, true));
    }
    Ok((r, false))
}
