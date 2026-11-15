//! Seasonality detection as Excel does it. The series is detrended by least
//! squares and standardised. A tapered periodogram of the residuals gives a
//! short-period candidate; a periodogram of the standardised series proposes
//! longer ones, each scored by the shape of the autocorrelation around its
//! lag. An amplitude comparison arbitrates when both stages report a short
//! period. Ported from formulon's `eval/ets_series.cpp` (see `mod.rs`).

use std::f64::consts::PI;

use super::num::{bit_compare, div_zero, flip_sign, mul, round_away, sign_of, to_u32};

const TWO_PI: f64 = 6.283185307179586;
const INT32_MIN: f64 = -2147483648.0;
const INT32_MAX: f64 = 2147483647.0;
/// Longest series considered by the detector.
const MAX_DETECT_LENGTH: usize = 175680;
/// From this length on, only the largest power-of-two tail is analysed.
const WINDOWED_LENGTH: usize = 2049;
/// Detected periods at or above this are reported as none.
const MAX_DETECTED_PERIOD: u32 = 8785;
const TAPER_FRACTION: f64 = 0.1;
const POWER_SCALE: f64 = 0.875;
const CENTRING_THRESHOLD: f64 = 3e-7;
const PERIOD_BIAS: f64 = 3e-7;
const MIN_PEAK_ACF: f64 = 0.45;
const STRONG_PEAK_RATIO: f64 = 4.0;
const SELECTION_RATIO: f64 = 2.0;
const PEAK_SIGMA: f64 = 2.5;
const MIN_REPEATS: f64 = 2.5;

/// Least-squares line over `xs[start..end]`: (slope, intercept).
fn regress(xs: &[f64], ys: &[f64], start: usize, end: usize) -> (f64, f64) {
    let (mut sx, mut sxx, mut sy, mut sxy) = (0.0, 0.0, 0.0, 0.0);
    for i in start..end {
        let x = xs[i];
        let y = ys[i];
        sx += x;
        sxx += mul(x, x);
        sy += y;
        sxy += mul(x, y);
    }
    let dn = (end - start) as f64;
    let d = mul(dn, sxx) - mul(sx, sx);
    let num = mul(dn, sxy) - mul(sx, sy);
    let slope = div_zero(num, d);
    let t = div_zero(mul(slope, sx), dn);
    (slope, div_zero(sy, dn) - t)
}

#[derive(Clone, Copy)]
struct Complex {
    re: f64,
    im: f64,
}

/// Radix-2 transform of a real input of power-of-two length; the output is
/// conjugated.
fn fft_pow2(x: &[f64], count: usize, inverse: bool) -> Vec<Complex> {
    let mut d = vec![0.0; 2 * count + 2];
    for k in 0..count {
        d[1 + 2 * k] = x[k];
    }
    let n2 = 2 * count;
    let mut j = 1;
    let mut i = 1;
    while i < n2 {
        if i < j {
            d.swap(j, i);
            d.swap(j + 1, i + 1);
        }
        let mut m = count;
        while m >= 2 && j > m {
            j -= m;
            m >>= 1;
        }
        j += m;
        i += 2;
    }
    let mut mmax = 2;
    loop {
        let mut theta = div_zero(TWO_PI, mmax as f64);
        if inverse {
            theta = flip_sign(theta);
        }
        let wpi = theta.sin();
        let mut wt = mul(theta, 0.5).sin();
        wt = mul(wt, wt);
        let wpr = mul(wt, -2.0);
        let istep = 2 * mmax;
        let mut wr = 1.0;
        let mut wi = 0.0;
        let mut m = 1;
        while m < mmax {
            let mut i = m;
            while i <= n2 {
                let jj = i + mmax;
                let t1 = mul(wi, d[jj + 1]);
                let t0 = mul(wr, d[jj]);
                let tempr = t0 - t1;
                let t2 = mul(wi, d[jj]);
                let t3 = mul(wr, d[jj + 1]);
                let tempi = t3 + t2;
                d[jj] = d[i] - tempr;
                d[jj + 1] = d[i + 1] - tempi;
                d[i] += tempr;
                d[i + 1] += tempi;
                i += istep;
            }
            let wtemp = wr;
            let mut a = mul(wpi, wi);
            let mut b = mul(wpr, wr);
            b -= a;
            wr += b;
            a = mul(wpi, wtemp);
            b = mul(wpr, wi);
            b += a;
            wi += b;
            m += 2;
        }
        mmax = istep;
        if istep >= n2 {
            break;
        }
    }
    (0..count)
        .map(|k| Complex {
            re: d[1 + 2 * k],
            im: flip_sign(d[2 + 2 * k]),
        })
        .collect()
}

/// Direct transform for lengths that are not a power of two.
fn dft_direct(x: &[f64], count: usize) -> Vec<Complex> {
    let step = div_zero(TWO_PI, count as f64);
    let mut ang: f64 = 0.0;
    let mut tw = Vec::with_capacity(count);
    for _ in 0..count {
        tw.push(Complex {
            re: ang.cos(),
            im: flip_sign(ang.sin()),
        });
        ang += step;
    }
    let mut out = Vec::with_capacity(count);
    for j in 0..count {
        let (mut re, mut im) = (0.0, 0.0);
        let mut idx = 0usize;
        for &xi in x.iter().take(count) {
            let k = idx % count;
            re += mul(tw[k].re, xi);
            im += mul(tw[k].im, xi);
            idx += j;
        }
        out.push(Complex { re, im });
    }
    out
}

fn transform(x: &[f64], count: usize, inverse: bool) -> Vec<Complex> {
    if count & (count - 1) == 0 {
        fft_pow2(x, count, inverse)
    } else {
        dft_direct(x, count)
    }
}

/// Transform length of the autocorrelation: a power of two above 2n.
fn acf_length(n: usize) -> usize {
    if n & (n - 1) == 0 {
        return 2 * n;
    }
    let mut p = 1;
    let mut m = n;
    while m > 3 {
        p <<= 1;
        m >>= 1;
    }
    p << 3
}

#[derive(Default)]
struct Periodogram {
    freq: Vec<f64>,
    power: Vec<f64>,
    mean: f64,
    sd: f64,
}

fn make_periodogram(y: &[f64]) -> Periodogram {
    let n = y.len();
    let half = ((n + 1) >> 1) as i64;
    let centre = |i: usize| (1 - half + i as i64) as f64;
    let mut s = 0.0;
    let mut sxy = 0.0;
    for (i, &v) in y.iter().enumerate() {
        s += v;
        sxy += mul(v, centre(i));
    }
    let mean = div_zero(s, n as f64);
    let wrapped = (n.wrapping_mul(n).wrapping_sub(1)) as u32;
    let mut den = (2.0 * n as f64) * wrapped as f64;
    den = div_zero(den, 24.0);
    let slope = div_zero(sxy, den);
    let mut d = vec![0.0; n];
    for i in 0..n {
        let mut v = mul(centre(i), slope);
        v += mean;
        v = flip_sign(v);
        d[i] = v + y[i];
    }
    let taper = to_u32(mul(n as f64, TAPER_FRACTION));
    let two_m = mul(taper as f64, 2.0);
    for i in 0..taper as usize {
        let mut w = mul((2 * i + 1) as f64, PI);
        w = div_zero(w, two_m);
        w = w.cos();
        w = flip_sign(w);
        w += 1.0;
        w = mul(w, 0.5);
        d[i] = mul(w, d[i]);
        d[n - 1 - i] = mul(w, d[n - 1 - i]);
    }
    let out = transform(&d, n, false);
    let scale = mul(n as f64, POWER_SCALE);
    let bins = n >> 1;
    let mut pg = Periodogram::default();
    let mut sum = 0.0;
    let mut sum2 = 0.0;
    for i in 0..bins {
        let p = mul(out[i + 1].re, out[i + 1].re);
        let mut q = mul(out[i + 1].im, out[i + 1].im);
        q += p;
        q = div_zero(q, scale);
        sum += q;
        sum2 += mul(q, q);
        pg.freq.push(div_zero((i + 1) as f64, n as f64));
        pg.power.push(q);
    }
    if n >= 2 {
        pg.mean = div_zero(sum, bins as f64);
        if n >= 4 {
            let v = div_zero(sum2, (bins - 1) as f64) - mul(pg.mean, pg.mean);
            pg.sd = if v >= 0.0 { v.sqrt() } else { 0.0 };
        }
    }
    pg
}

/// Autocorrelation of the standardised series via zero-padded transforms,
/// normalised by the overlap count at each lag.
fn make_acf(std_y: &[f64]) -> Vec<f64> {
    let n = std_y.len();
    let len = acf_length(n);
    let mut s = 0.0;
    for &v in std_y {
        s += v;
    }
    let mean = div_zero(s, n as f64);
    let mut buf = vec![0.0; len];
    for i in 0..n {
        buf[i] = if bit_compare(mean, CENTRING_THRESHOLD) == 1 {
            std_y[i] - mean
        } else {
            std_y[i]
        };
    }
    let correlate = |b: &[f64]| -> Vec<f64> {
        let spec = transform(b, len, false);
        let pw: Vec<f64> = spec
            .iter()
            .map(|c| mul(c.re, c.re) + mul(c.im, c.im))
            .collect();
        let inv = transform(&pw, len, true);
        inv[..n].iter().map(|c| c.re).collect()
    };
    let raw = correlate(&buf);
    let mut ones = vec![0.0; len];
    for v in ones.iter_mut().take(n) {
        *v = 1.0;
    }
    let cnt = correlate(&ones);
    let dn = n as f64;
    let mut base = div_zero(raw[0], cnt[0]);
    base = div_zero(base, dn);
    let mut acf = vec![0.0; n];
    acf[0] = 1.0;
    for k in 1..n {
        let mut v = div_zero(raw[k], dn);
        v = div_zero(v, cnt[k]);
        acf[k] = div_zero(v, base);
    }
    acf
}

trait Powered {
    fn power(&self) -> f64;
}

/// Descending order by power; up to eight items a selection sort, whose tie
/// order differs from a stable sort.
fn sort_by_power_desc<T: Powered>(items: &mut [T]) {
    if items.len() <= 8 {
        let mut hi = items.len();
        while hi > 1 {
            hi -= 1;
            let mut mx = 0;
            for p in 1..=hi {
                if bit_compare(items[mx].power(), items[p].power()) == 1 {
                    mx = p;
                }
            }
            if mx != hi {
                items.swap(mx, hi);
            }
        }
        return;
    }
    items.sort_by(|a, b| {
        b.power()
            .partial_cmp(&a.power())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

#[derive(Clone, Copy)]
struct Candidate {
    period: i64,
    power: f64,
}

impl Powered for Candidate {
    fn power(&self) -> f64 {
        self.power
    }
}

#[derive(Clone, Copy)]
struct LagRange {
    period: i64,
    lo: i64,
    hi: i64,
}

#[derive(Clone, Copy, Default)]
struct Peak {
    period: i64,
    power: f64,
    acf: f64,
}

impl Powered for Peak {
    fn power(&self) -> f64 {
        self.power
    }
}

struct Detector<'a> {
    y: &'a [f64],
    n: usize,
    resid: Periodogram,
    std: Periodogram,
    acf: Vec<f64>,
    short_period: i64,
    long_period: i64,
    short_power: f64,
}

impl<'a> Detector<'a> {
    fn new(y: &'a [f64]) -> Detector<'a> {
        let n = y.len();
        let xs: Vec<f64> = (0..n).map(|i| (i + 1) as f64).collect();
        let line = regress(&xs, y, 0, n);
        let mut r = vec![0.0; n];
        let mut s = 0.0;
        let mut s2 = 0.0;
        for i in 0..n {
            let mut v = mul((i + 1) as f64, line.0);
            v += line.1;
            v = flip_sign(v);
            v += y[i];
            r[i] = v;
            s += v;
            s2 += mul(v, v);
        }
        let mean = div_zero(s, n as f64);
        let m2 = mul(mean, mean);
        let var = div_zero(s2, (n - 1) as f64) - m2;
        let sd = if var >= 0.0 { var.sqrt() } else { 0.0 };
        let mut std_y = r.clone();
        if sign_of(sd) != 0 {
            for i in 0..n {
                std_y[i] = div_zero(r[i] - mean, sd);
            }
        }
        Detector {
            y,
            n,
            resid: make_periodogram(&r),
            std: make_periodogram(&std_y),
            acf: make_acf(&std_y),
            short_period: 1,
            long_period: 1,
            short_power: 0.0,
        }
    }

    fn run(mut self) -> u32 {
        self.short_stage();
        self.long_stage();
        self.arbitrate()
    }

    fn acf_at(&self, lag: i64) -> f64 {
        if lag >= 0 && (lag as usize) < self.acf.len() {
            self.acf[lag as usize]
        } else {
            0.0
        }
    }

    /// Short-period candidate from the residual periodogram.
    fn short_stage(&mut self) {
        let n = self.n;
        if n < 4 {
            return;
        }
        let cap = if 2 * n < 25 { (2 * n) / 5 } else { 5 } as u32;
        let pw = &self.resid.power;
        let fr = &self.resid.freq;
        let mut best = pw[0];
        let mut second = pw[1];
        let mut per = to_u32(round_away(div_zero(1.0, fr[0])));
        if bit_compare(pw[1], best) == 1 {
            second = best;
            best = pw[1];
            per = to_u32(round_away(div_zero(1.0, fr[1]) - PERIOD_BIAS));
        }
        for i in 2..(n >> 1) {
            let p = pw[i];
            if bit_compare(p, best) == 1 {
                second = best;
                best = p;
                per = to_u32(round_away(div_zero(1.0, fr[i]) - PERIOD_BIAS));
            } else if bit_compare(p, second) == 1 {
                second = p;
            }
        }
        self.short_power = best;
        if per >= 2 && per <= cap {
            let ratio = if sign_of(second) != 0 {
                div_zero(best, second)
            } else {
                0.0
            };
            let thr = 2.0 - div_zero(3.0, n as f64);
            if bit_compare(ratio, thr) == 1 && bit_compare(best, self.resid.mean) == 1 {
                self.short_period = per as i64;
            }
        } else {
            let thr = mul(PEAK_SIGMA, self.resid.sd) + self.resid.mean;
            if bit_compare(best, thr) == 1 && per as f64 * MIN_REPEATS <= n as f64 {
                self.short_period = per as i64;
            }
        }
    }

    fn candidates(&self) -> Vec<Candidate> {
        let n = self.n;
        let bins = n >> 1;
        let mut cands = Vec::new();
        let mut strong = Vec::new();
        let pw = &self.std.power;
        for i in 0..bins {
            let p = pw[i];
            if bit_compare(p, self.std.mean) != 1 {
                continue;
            }
            let per = to_u32(round_away(div_zero(1.0, self.std.freq[i]))) as i64;
            if per < 4 || n as i64 <= 2 * per {
                continue;
            }
            cands.push(Candidate {
                period: per,
                power: p,
            });
            if i == 0 || i == bins - 1 {
                continue;
            }
            let prev = pw[i - 1];
            let next = pw[i + 1];
            if sign_of(prev) != 0 && sign_of(next) != 0 {
                if bit_compare(div_zero(p, prev), STRONG_PEAK_RATIO) == 1
                    && bit_compare(div_zero(p, next), STRONG_PEAK_RATIO) == 1
                {
                    strong.push(Candidate {
                        period: per,
                        power: p,
                    });
                }
            } else if sign_of(prev) == 0 && sign_of(next) == 0 {
                strong.push(Candidate {
                    period: per,
                    power: p,
                });
            }
        }
        sort_by_power_desc(&mut cands);
        cands.truncate(3);
        for e in strong {
            if !cands.iter().any(|c| c.period == e.period) {
                cands.push(e);
            }
        }
        cands
    }

    fn lag_range(&self, c: &Candidate) -> LagRange {
        let dn = self.n as f64;
        let d = div_zero(dn, c.period as f64);
        let lo = to_u32(round_away(div_zero(dn, d + 1.0) - 1.0)) as i64;
        let hi = to_u32(round_away(div_zero(dn, d - 1.0) + 1.0)) as i64;
        let lo2 = if lo < 4 { 3 } else { lo };
        let mut hi2 = if hi < 3 && lo < 4 { 5 } else { hi };
        if hi2 > self.n as i64 {
            hi2 = lo2;
        }
        LagRange {
            period: c.period,
            lo: lo2,
            hi: hi2,
        }
    }

    /// A short-lag peak: the autocorrelation rises into it and falls after.
    fn narrow_peak(&self, p: i64, x2: i64) -> Option<Peak> {
        let w = if p == 5 { 4 } else { 2 };
        let cnt = (2 * w + 1) as usize;
        let mut xs = vec![0.0; cnt];
        let mut ys = vec![0.0; cnt];
        for i in 0..cnt {
            let x = p - w + i as i64;
            xs[i] = x as f64;
            ys[i] = self.acf_at(x);
        }
        let s1 = regress(&xs, &ys, 0, w as usize + 1).0;
        let s2 = regress(&xs, &ys, w as usize + 1, cnt).0;
        if sign_of(s1) == 1 && sign_of(s2) == -1 {
            return Some(Peak {
                period: x2,
                power: 0.0,
                acf: self.acf_at(x2),
            });
        }
        None
    }

    /// Two-piece linear fit of the flank-detrended autocorrelation over the
    /// lag range; a peak when it rises then falls.
    fn broad_peak(&self, r: &LagRange, x2: i64) -> Option<Peak> {
        let span = r.hi - r.lo;
        let cnt = (span + 1) as usize;
        let mut fx = Vec::new();
        let mut fy = Vec::new();
        for k in -3..=-1 {
            let lag = r.lo + k;
            if lag + 1 >= 3 {
                fx.push((lag + 1) as f64);
                fy.push(self.acf_at(lag));
            }
        }
        for k in -1..=1 {
            let lag = r.hi + k;
            if lag + 1 <= self.n as i64 {
                fx.push((lag + 1) as f64);
                fy.push(self.acf_at(lag));
            }
        }
        let line = regress(&fx, &fy, 0, fx.len());
        let mut xs = vec![0.0; cnt];
        let mut ys = vec![0.0; cnt];
        for i in 0..cnt {
            let lag = r.lo + i as i64;
            let mut v = mul(lag as f64, line.0);
            v += line.1;
            v = flip_sign(v);
            v += self.acf_at(lag);
            xs[i] = lag as f64;
            ys[i] = v;
        }
        let step = to_u32(div_zero((span - 3) as f64, 100.0).ceil()) as i64;
        let last = span - 1;
        if last < 3 {
            return None;
        }
        let mut best = INT32_MAX;
        let mut found = None;
        let mut i = 2;
        while i < last {
            let si = i as usize;
            let left = regress(&xs, &ys, 0, si + 1);
            let right = regress(&xs, &ys, si, cnt);
            let mut m1 = 0.0;
            for j in 0..=si {
                let mut v = mul(xs[j], left.0);
                v += left.1;
                v = flip_sign(v);
                v += ys[j];
                v = mul(v, v);
                m1 += v;
            }
            m1 = div_zero(m1, (i + 1) as f64);
            let mut m2 = 0.0;
            for j in si..cnt {
                let mut v = mul(xs[j], right.0);
                v += right.1;
                v = flip_sign(v);
                v += ys[j];
                v = mul(v, v);
                m2 += v;
            }
            m2 = div_zero(m2, (cnt - si) as f64);
            let total = m1 + m2;
            if bit_compare(total, best) == -1 {
                found = if sign_of(left.0) == 1 && sign_of(right.0) == -1 {
                    Some(Peak {
                        period: x2,
                        power: 0.0,
                        acf: self.acf_at(x2),
                    })
                } else {
                    None
                };
                best = total;
            }
            i += step;
        }
        found
    }

    /// Scores one lag range; `doubled` marks the retry at twice the period.
    fn score(&self, r: &LagRange, doubled: bool) -> Option<Peak> {
        let cnt = r.hi - r.lo + 1;
        let mut best = INT32_MIN;
        let mut bl = 0;
        for lag in r.lo..=r.hi {
            let v = self.acf_at(lag);
            if bit_compare(v, best) == 1 {
                bl = lag;
                best = v;
            }
        }
        let mut x2 = bl;
        if doubled {
            x2 = to_u32(div_zero(bl as f64, 2.0).floor()) as i64;
        }
        if x2 as f64 * MIN_REPEATS > self.n as f64 {
            return None;
        }
        if bl == r.lo || bl == r.hi {
            return None;
        }
        if (3..=5).contains(&bl) || (doubled && (6..=11).contains(&bl)) {
            return self.narrow_peak(bl, x2);
        }
        if cnt < 5 {
            return None;
        }
        self.broad_peak(r, x2)
    }

    /// Long-period candidates from the standardised periodogram.
    fn long_stage(&mut self) {
        let cands = self.candidates();
        if cands.is_empty() {
            return;
        }
        let n = self.n as i64;
        let mut scored: Vec<Peak> = Vec::new();
        for c in &cands {
            let mut r = self.lag_range(c);
            let half = to_u32(div_zero(r.period as f64, 2.0).floor()) as i64;
            if half < r.hi {
                r.lo = r.lo.max(half);
            }
            let peak = match self.score(&r, false) {
                Some(p) => p,
                None => {
                    if 2 * r.hi >= n {
                        continue;
                    }
                    let twice = LagRange {
                        period: 2 * r.period,
                        lo: 2 * r.lo,
                        hi: 2 * r.hi,
                    };
                    match self.score(&twice, true) {
                        Some(p) => p,
                        None => continue,
                    }
                }
            };
            if peak.acf < MIN_PEAK_ACF {
                continue;
            }
            if let Some(hit) = scored.iter_mut().find(|e| e.period == peak.period) {
                if peak.acf > hit.acf {
                    hit.acf = peak.acf;
                }
            } else {
                scored.push(peak);
            }
        }
        if scored.is_empty() {
            return;
        }
        if scored.len() == 1 {
            self.long_period = scored[0].period;
            return;
        }
        let mut pool = scored;
        if (2..=5).contains(&self.short_period) {
            let e = Peak {
                period: self.short_period,
                power: self.short_power,
                acf: self.acf_at(self.short_period),
            };
            if let Some(hit) = pool.iter_mut().find(|t| t.period == self.short_period) {
                if bit_compare(hit.power, e.power) != 1 {
                    hit.power = e.power;
                }
                if bit_compare(hit.acf, e.acf) != 1 {
                    hit.acf = e.acf;
                }
            } else {
                pool.push(e);
            }
        }
        sort_by_power_desc(&mut pool);
        let cnt = pool.len();
        let mut reach = 1;
        if cnt >= 2 {
            let mut i = 0;
            loop {
                let ratio = div_zero(pool[i].power, pool[i + 1].power);
                if bit_compare(ratio, SELECTION_RATIO) == 1 {
                    reach = i;
                    break;
                }
                i += 1;
                if i + 1 >= cnt {
                    reach = cnt;
                    break;
                }
            }
        }
        let top = if reach < cnt { reach } else { cnt - 1 };
        let mut cur = INT32_MIN;
        for k in (0..=top).rev() {
            if bit_compare(pool[k].acf, cur) == 1 {
                self.long_period = pool[k].period;
                cur = pool[k].acf;
            }
        }
    }

    /// Compares the amplitude at the two stage periods when both are short.
    fn arbitrate(&self) -> u32 {
        let p38 = self.long_period;
        let p3c = self.short_period;
        if p38 == 1 {
            return p3c as u32;
        }
        if p3c == 1 || p38 > 5 {
            return p38 as u32;
        }
        let n = self.n;
        if (n as i64) < 2 * p3c {
            return p38 as u32;
        }
        let periods = [p38, p3c];
        let mut amps = [0.0; 2];
        for s in 0..2 {
            let period = periods[s] as f64;
            let k = to_u32(div_zero(n as f64, period).floor()) as usize;
            let length = k * periods[s] as usize;
            let (mut c, mut sn) = (0.0, 0.0);
            for (i, t) in (n - length..n).enumerate() {
                let mut v = mul(i as f64, PI);
                v = mul(v, 2.0);
                v = flip_sign(v);
                v = div_zero(v, period);
                c += mul(v.cos(), self.y[t]);
                sn += mul(v.sin(), self.y[t]);
            }
            let a = mul(c, c);
            let mut b = mul(sn, sn);
            b += a;
            b = b.sqrt();
            amps[s] = div_zero(b, length as f64);
        }
        let mut cur = INT32_MIN;
        let mut win = p38;
        for s in 0..2 {
            if bit_compare(amps[s], cur) == 1 {
                win = periods[s];
                cur = amps[s];
            }
        }
        win as u32
    }
}

/// Auto-detected season length: 1 means none, otherwise at least 2.
pub(crate) fn detect_seasonality(y: &[f64]) -> usize {
    let mut n = y.len().min(MAX_DETECT_LENGTH);
    if n >= WINDOWED_LENGTH {
        let mut p = 1;
        while (p << 1) <= n {
            p <<= 1;
        }
        n = p;
    }
    if n < 2 {
        return 1;
    }
    let window = &y[y.len() - n..];
    let period = Detector::new(window).run();
    if period >= MAX_DETECTED_PERIOD || period <= 1 {
        1
    } else {
        period as usize
    }
}
