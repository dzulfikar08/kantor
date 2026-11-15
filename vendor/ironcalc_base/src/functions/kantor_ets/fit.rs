//! The additive Holt-Winters fit (ETS AAA) as Excel does it: smoothing
//! parameters picked from a fixed candidate table by a holdout-scored SSE,
//! the initial trend a least-squares slope, error statistics over the
//! holdout. Ported from formulon's `eval/holt_winters.cpp` (see `mod.rs`).

use super::num::{div, mul, round_away};

struct SeasonalCandidate {
    alpha: f64,
    beta: f64,
    gamma: f64,
}

const fn sc(alpha: f64, beta: f64, gamma: f64) -> SeasonalCandidate {
    SeasonalCandidate { alpha, beta, gamma }
}

/// Candidates for seasonal fits: by gamma, then beta ascending, then alpha
/// descending. The first strict minimum wins.
const SEASONAL_CANDIDATES: [SeasonalCandidate; 39] = [
    sc(0.751, 0.001, 0.001),
    sc(0.501, 0.001, 0.001),
    sc(0.251, 0.001, 0.001),
    sc(0.126, 0.001, 0.001),
    sc(0.998, 0.099, 0.001),
    sc(0.126, 0.125, 0.001),
    sc(0.998, 0.249, 0.001),
    sc(0.5, 0.25, 0.001),
    sc(0.251, 0.25, 0.001),
    sc(0.998, 0.499, 0.001),
    sc(0.501, 0.5, 0.001),
    sc(0.998, 0.749, 0.001),
    sc(0.751, 0.75, 0.001),
    sc(0.998, 0.899, 0.001),
    sc(0.833333, 0.0833333, 0.0833333),
    sc(0.166667, 0.0833333, 0.0833333),
    sc(0.833333, 0.75, 0.0833333),
    sc(0.9, 0.001, 0.099),
    sc(0.9, 0.899, 0.099),
    sc(0.002, 0.001, 0.125),
    sc(0.666667, 0.166667, 0.166667),
    sc(0.333333, 0.166667, 0.166667),
    sc(0.666667, 0.5, 0.166667),
    sc(0.75, 0.001, 0.249),
    sc(0.75, 0.749, 0.249),
    sc(0.251, 0.001, 0.25),
    sc(0.002, 0.001, 0.25),
    sc(0.5, 0.25, 0.25),
    sc(0.251, 0.25, 0.25),
    sc(0.5, 0.001, 0.499),
    sc(0.5, 0.499, 0.499),
    sc(0.002, 0.001, 0.5),
    sc(0.333333, 0.166667, 0.5),
    sc(0.25, 0.001, 0.749),
    sc(0.25, 0.249, 0.749),
    sc(0.002, 0.001, 0.75),
    sc(0.166667, 0.0833333, 0.75),
    sc(0.1, 0.001, 0.899),
    sc(0.1, 0.099, 0.899),
];

/// Candidates (alpha, beta) for non-seasonal fits: by beta ascending, then
/// alpha descending.
const TREND_CANDIDATES: [(f64, f64); 25] = [
    (0.9, 0.001),
    (0.75, 0.001),
    (0.5, 0.001),
    (0.25, 0.001),
    (0.126, 0.001),
    (0.1, 0.001),
    (0.002, 0.001),
    (0.833333, 0.0833333),
    (0.166667, 0.0833333),
    (0.998, 0.099),
    (0.1, 0.099),
    (0.126, 0.125),
    (0.666667, 0.166667),
    (0.333333, 0.166667),
    (0.998, 0.249),
    (0.25, 0.249),
    (0.5, 0.25),
    (0.998, 0.499),
    (0.5, 0.499),
    (0.666667, 0.5),
    (0.998, 0.749),
    (0.75, 0.749),
    (0.833333, 0.75),
    (0.998, 0.899),
    (0.9, 0.899),
];

/// Forecasts below this magnitude may be snapped to zero.
const TINY_RESULT: f64 = 3e-7;
/// Decades between the initial trend and a forecast beyond which it snaps.
const TINY_RESULT_DECADES: f64 = 3.0;

/// Fitted parameters, final state and fit statistics.
#[derive(Debug, Clone)]
pub(crate) struct Fit {
    pub alpha: f64,
    pub beta: f64,
    /// `f64::EPSILON` for a non-seasonal model.
    pub gamma: f64,
    pub mae: f64,
    pub rmse: f64,
    pub mase: f64,
    pub smape: f64,
    /// Mean squared one-step error over the whole series.
    pub sigma2: f64,
    pub initial_trend: f64,
    /// Season length the recurrences use (at least 2).
    pub period: usize,
    /// [level, trend, seasonal slots…]; the seasonal slots are a ring.
    pub state: Vec<f64>,
}

impl Fit {
    /// Point forecast `h` steps after the last observation (`h` may be ≤ 0).
    pub fn forecast(&self, h: i64) -> f64 {
        let m = self.period as i64;
        let v = mul(h as f64, self.state[1]) + self.state[0];
        let k = if h >= 1 { h } else { h + m * ((-h) / m + 1) };
        let idx = self.state.len() - 1 - ((k - 1) % m) as usize;
        v + self.state[idx]
    }

    /// 0 for a result that is rounding noise next to the initial trend.
    pub fn zero_if_negligible(&self, r: f64) -> f64 {
        let mag = r.abs();
        if r != 0.0 && mag < TINY_RESULT && mag < self.initial_trend.abs() {
            let mut ratio = self.initial_trend.abs() / mag;
            if ratio.abs() > f64::MAX || ratio.abs() < f64::MIN_POSITIVE {
                ratio = 0.0;
            }
            if ratio.log10().abs() >= TINY_RESULT_DECADES {
                return 0.0;
            }
        }
        r
    }

    /// Forecast error variance at horizon `1 ≤ h ≤ 2^53`.
    pub fn variance(&self, h: i64) -> f64 {
        if h == 1 {
            return self.sigma2;
        }
        let (a, b, g) = (self.alpha, self.beta, self.gamma);
        let m = self.period as f64;
        let dh = h as f64;
        let dh1 = (h - 1) as f64;
        let k = ((h - 1) / self.period as i64) as f64;
        let d2h = (2 * h - 1) as f64;
        let mut v = mul(dh, d2h);
        v = mul(v, b);
        v = mul(v, b);
        let trend_term = div(v, 6.0);
        let a2 = mul(a, a);
        v = mul(dh, a);
        v = mul(v, b);
        v += trend_term;
        v += a2;
        v = mul(v, dh1);
        let base = 1.0 + v;
        v = k + 1.0;
        v = mul(v, m);
        v = mul(v, b);
        let c = v + g;
        v = mul(2.0, a);
        v += c;
        v = mul(v, g);
        v = mul(v, k);
        v += base;
        mul(v, self.sigma2)
    }
}

/// Least-squares slope of `y[..n]` against 1..n; with `state`, the seasonal
/// slot of each point is subtracted first.
fn ols_slope(y: &[f64], n: usize, state: Option<&[f64]>, m: usize) -> f64 {
    if n == 1 {
        return 0.0;
    }
    let mut sy = 0.0;
    let mut sxy = 0.0;
    for (i, &yi) in y.iter().enumerate().take(n) {
        let mut v = yi;
        if let Some(st) = state {
            v -= st[m + 1 - (i % m)];
        }
        sy += v;
        sxy += mul((i + 1) as f64, v);
    }
    let dn = n as f64;
    let mut sx = dn + 1.0;
    sx = mul(sx, dn);
    sx = div(sx, 2.0);
    sx = round_away(sx);
    let t1 = dn + 1.0;
    let t2 = dn + 0.5;
    let mut sxx = div(dn, 3.0);
    sxx = mul(sxx, t1);
    sxx = mul(sxx, t2);
    sxx = round_away(sxx);
    let mut d = mul(sx, sx);
    d = mul(dn, sxx) - d;
    let c = mul(sx, sy);
    let num = mul(dn, sxy) - c;
    div(num, d)
}

struct Fitter {
    n: usize,
    seasonal: bool,
    mp: usize,
    size: usize,
    dm: f64,
    yw: Vec<f64>,
    nw: usize,
    fit_w: usize,
    fit_n: usize,
    alpha: f64,
    beta: f64,
    gamma: f64,
    initial_trend: f64,
}

impl Fitter {
    fn new(y: &[f64], m: usize) -> Fitter {
        let mp = if m > 2 { m } else { 2 };
        Fitter {
            n: y.len(),
            seasonal: m > 1,
            mp,
            size: mp + 2,
            dm: mp as f64,
            yw: y.to_vec(),
            nw: 0,
            fit_w: 0,
            fit_n: 0,
            alpha: 0.0,
            beta: 0.0,
            gamma: 0.0,
            initial_trend: 0.0,
        }
    }

    fn run(mut self) -> Fit {
        self.pad_short_series();
        self.nw = self.yw.len();
        self.fit_w = self.split(self.nw);
        self.fit_n = self.split(self.n);

        let mut st = vec![0.0; self.size];
        st[0] = self.yw[0];
        if self.seasonal {
            self.seasonal_init(&mut st);
            st[0] -= st[self.mp + 1];
        }
        self.normalize(&mut st);
        st[1] = ols_slope(&self.yw, self.nw, Some(&st), self.mp);
        st[0] -= st[1];

        self.search(&st);
        self.initial_trend = st[1];
        self.final_pass(&st)
    }

    /// A short series is extended by linear extrapolation, phase by phase
    /// when seasonal.
    fn pad_short_series(&mut self) {
        let (n, mp) = (self.n, self.mp);
        if 7 * mp < 2 * n {
            return;
        }
        let mut extra = (7 * mp - 2 * n + 1) / 2;
        let rep = mp.saturating_sub(n);
        let mut slope = 0.0;
        if rep > 0 {
            let last = self.yw[n - 1];
            for _ in 0..rep {
                self.yw.push(last);
            }
            extra = extra.saturating_sub(rep);
        } else if self.seasonal {
            let mut acc = 0.0;
            for p in 0..mp {
                let mut arr = [0.0; 4];
                let mut cnt = 0;
                let mut k = p;
                while k < n && cnt < 4 {
                    arr[cnt] = self.yw[k];
                    cnt += 1;
                    k += mp;
                }
                acc += ols_slope(&arr, cnt, None, 0);
            }
            slope = div(acc, self.dm);
            slope = div(slope, self.dm);
        } else {
            slope = ols_slope(&self.yw, n, None, 0);
        }
        let mut pos = n as f64;
        for k in 0..extra {
            let j = (k + n) % mp;
            let mut v = pos - j as f64;
            v = mul(v, slope);
            v += self.yw[j];
            self.yw.push(v);
            pos += 1.0;
        }
    }

    /// The training part of `n` points (the rest is the holdout).
    fn split(&self, n: usize) -> usize {
        let k = mul(0.8, n as f64) as usize;
        let mut f = if n > 10 { k } else { n };
        let hold = n - f;
        if hold != 0 {
            let cap = mul(0.4, n as f64) as usize;
            let mut h = if hold > 2 * self.mp {
                hold
            } else {
                2 * self.mp
            };
            if h <= 10 {
                h = 10;
            }
            if h >= cap {
                h = cap;
            }
            f = n - h;
        }
        f
    }

    /// Seasonal factors from a centred 2m-point moving-average decomposition.
    fn seasonal_init(&self, st: &mut [f64]) {
        let m = self.mp;
        let lim = self.nw.min(5 * m);
        let nma = lim - 2 * m;
        let d2m = (2 * m) as f64;
        let mut ma = vec![0.0; nma];
        let mut run = 0.0;
        for i in 0..lim {
            if i >= 2 * m {
                ma[i - 2 * m] = div(run, d2m);
                run -= self.yw[i - 2 * m];
            }
            run += self.yw[i];
        }
        let half = m - (m >> 1);
        for p in 0..m {
            let mut cnt = 0.0;
            let mut s = 0.0;
            let mut k = p;
            while k < nma {
                cnt += 1.0;
                s += self.yw[k + (m >> 1) - 1];
                s -= ma[k];
                k += m;
            }
            s = div(s, cnt);
            let idx = if p <= half {
                half + 2 - p
            } else {
                half + 2 + m - p
            };
            st[idx] = s;
        }
    }

    /// Makes the seasonal slots sum to zero, moving the mean into the level.
    fn normalize(&self, st: &mut [f64]) {
        let mut s = 0.0;
        for v in &st[2..2 + self.mp] {
            s += *v;
        }
        s = div(s, self.dm);
        st[0] += s;
        for v in &mut st[2..2 + self.mp] {
            *v -= s;
        }
    }

    /// One error correction; returns the next seasonal slot.
    fn update(&self, s: &mut [f64], e: f64, a: f64, b: f64, g: f64, idx: usize) -> usize {
        let mut v = mul(e, a);
        v += s[1];
        s[0] += v;
        v = mul(e, b);
        s[1] += v;
        v = mul(e, g);
        v += s[idx];
        s[idx] = v;
        if idx == 2 {
            self.size - 1
        } else {
            idx - 1
        }
    }

    /// Scaled SSE of one candidate; stops once it exceeds `best`.
    fn sse(&self, st0: &[f64], a: f64, b: f64, g: f64, best: f64) -> f64 {
        let mut st = st0.to_vec();
        let thr = div(best, 0.1);
        let mut s = 0.0;
        let mut idx = self.size - 1;
        for t in 0..self.nw {
            let f = st[0] + st[1] + st[idx];
            let e = self.yw[t] - f;
            s += mul(e, e);
            if s > thr {
                break;
            }
            if t < self.fit_w {
                idx = self.update(&mut st, e, a, b, g, idx);
            } else {
                st[0] += st[1];
                idx -= 1;
                if idx <= 1 {
                    idx = self.size - 1;
                }
            }
        }
        mul(s, 0.1)
    }

    fn search(&mut self, st: &[f64]) {
        let mut best = mul(f64::MAX, 0.1);
        let mut consider = |this: &mut Fitter, a: f64, b: f64, g: f64| {
            let v = this.sse(st, a, b, g, best);
            if v < best {
                best = v;
                this.alpha = a;
                this.beta = b;
                this.gamma = g;
                return best == 0.0;
            }
            false
        };
        if self.seasonal {
            for c in &SEASONAL_CANDIDATES {
                if consider(self, c.alpha, c.beta, c.gamma) {
                    break;
                }
            }
        } else {
            for &(a, b) in &TREND_CANDIDATES {
                if consider(self, a, b, f64::EPSILON) {
                    break;
                }
            }
        }
    }

    /// Runs the chosen parameters over the series for the final state and
    /// the error statistics.
    fn final_pass(mut self, st0: &[f64]) -> Fit {
        let mut st = st0.to_vec();
        let (a, b, g) = (self.alpha, self.beta, self.gamma);
        let n = self.n;
        let half = if n > 10 { n >> 1 } else { 0 };
        let mut sum_b = 0.0;
        let mut sum_b2 = 0.0;
        let mut naive = 0.0;
        let mut sse_all = 0.0;
        let mut sae = 0.0;
        let mut s2 = 0.0;
        let mut smape = 0.0;
        let mut idx = self.size - 1;
        let fit = self.fit_n;
        for t in 0..n {
            let f = st[0] + st[1] + st[idx];
            let e = self.yw[t] - f;
            sse_all += mul(e, e);
            idx = self.update(&mut st, e, a, b, g, idx);
            if t > half {
                sum_b += st[1];
                sum_b2 += mul(st[1], st[1]);
            }
            if t != 0 && (t < fit || fit == n) {
                naive += (self.yw[t] - self.yw[t - 1]).abs();
            }
            if t >= fit || fit == n {
                if e != 0.0 {
                    sae += e.abs();
                    s2 += mul(e, e);
                }
                if !(f == 0.0 && self.yw[t] == 0.0) {
                    let d = f.abs() + self.yw[t].abs();
                    smape += div(e, d).abs();
                }
            }
        }
        // Rotate the seasonal slots so that the last one is used next.
        let size = self.size;
        if idx != size - 1 {
            let old: Vec<f64> = st[2..2 + self.mp].to_vec();
            for (i, v) in old.into_iter().enumerate() {
                let mut k = size - idx + 1 + i;
                if k > size - 1 {
                    k = k % size + 2;
                }
                st[k] = v;
            }
        }
        let cnt = if n > 10 { n >> 1 } else { n };
        let mean_b = div(sum_b, cnt as f64);
        let dn1 = (cnt - 1) as f64;
        let mut v = mul(sum_b, sum_b);
        v = div(v, dn1);
        v = sum_b2 - v;
        v = div(v, dn1);
        v = v.abs();
        let sd = v.sqrt();
        let sdv = if cnt > 1 { sd } else { 0.0 };
        let cv = if mean_b != 0.0 {
            div(sdv, mean_b).abs()
        } else {
            0.0
        };
        if mean_b == 0.0 || cv > 1.0 {
            st[0] = self.yw[n - 1] - st[2];
            st[1] = st0[1];
            self.beta = 0.001;
        }
        naive = div(naive, (fit - 1) as f64);
        let cnt2 = (n - if fit != n { fit } else { 0 }) as f64;
        let mae = div(sae, cnt2);
        smape = mul(smape, 2.0);
        self.normalize(&mut st);

        Fit {
            alpha: self.alpha,
            beta: self.beta,
            gamma: self.gamma,
            sigma2: div(sse_all, n as f64),
            mae,
            rmse: div(s2, cnt2).sqrt(),
            smape: div(smape, cnt2),
            mase: if naive != 0.0 { div(mae, naive) } else { 0.0 },
            initial_trend: self.initial_trend,
            period: self.mp,
            state: st,
        }
    }
}

/// Fits `y` (at least two points) with season length `m` (1: none).
pub(crate) fn fit(y: &[f64], m: usize) -> Fit {
    Fitter::new(y, m).run()
}
