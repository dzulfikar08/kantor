//! Bootstrap inference: resample rows with replacement, estimate again,
//! and summarise each statistic's distribution (standard error, t and p
//! values, percentile and BCa confidence intervals).
//!
//! Subsample `b` draws from its own ChaCha8 stream (seed, stream `b`), so
//! results are the same on any number of threads.

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use serde::Serialize;

use crate::core::{self, Core};
use crate::data::Moments;
use crate::dist::{normal_cdf, normal_quantile, t_two_tailed};
use crate::linalg::Mat;
use crate::model::{Bootstrap, Measurement, Options, SignChanges};
use crate::spec::Spec;

/// What a bootstrap statistic is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Key {
    Path(usize, usize),
    Loading(usize),
    Weight(usize),
    R2(usize),
    F2(usize, usize),
    TotalIndirect(usize, usize),
    Total(usize, usize),
    Specific(usize),
    Htmt(usize, usize),
}

pub(crate) fn keys(spec: &Spec) -> Vec<Key> {
    let k = spec.k();
    let mut v = Vec::new();
    v.extend(spec.paths.iter().map(|&(f, t)| Key::Path(f, t)));
    v.extend((0..spec.p()).map(Key::Loading));
    v.extend((0..spec.p()).map(Key::Weight));
    v.extend((0..k).filter(|&j| !spec.preds[j].is_empty()).map(Key::R2));
    v.extend(spec.paths.iter().map(|&(f, t)| Key::F2(f, t)));
    for &f in &spec.order {
        for &t in &spec.order {
            if spec.has_indirect(f, t) {
                v.push(Key::TotalIndirect(f, t));
            }
        }
    }
    for &f in &spec.order {
        for &t in &spec.order {
            if spec.reaches(f, t) {
                v.push(Key::Total(f, t));
            }
        }
    }
    v.extend((0..spec.chains.len()).map(Key::Specific));
    for i in 0..k {
        for j in i + 1..k {
            if spec.measurement[i] == Measurement::Reflective
                && spec.measurement[j] == Measurement::Reflective
            {
                v.push(Key::Htmt(i, j));
            }
        }
    }
    v
}

/// The statistics of one estimation, in `keys` order.
pub(crate) fn values(keys: &[Key], s: &Mat, spec: &Spec, c: &Core) -> Vec<f64> {
    let need_f2 = keys.iter().any(|k| matches!(k, Key::F2(..)));
    let f2 = need_f2.then(|| core::f2(spec, c));
    let total = core::total_effects(&c.beta);
    let specific = core::specific_indirect(spec, &c.beta);
    let need_htmt = keys.iter().any(|k| matches!(k, Key::Htmt(..)));
    let htmt = need_htmt.then(|| core::htmt(s, spec).0);
    keys.iter()
        .map(|k| match *k {
            Key::Path(f, t) => c.beta[(f, t)],
            Key::Loading(i) => c.loadings[i],
            Key::Weight(i) => c.w[i],
            Key::R2(j) => c.r2[j],
            Key::F2(f, t) => f2.as_ref().map_or(f64::NAN, |m| m[(f, t)]),
            Key::TotalIndirect(f, t) => total[(f, t)] - c.beta[(f, t)],
            Key::Total(f, t) => total[(f, t)],
            Key::Specific(i) => specific[i],
            Key::Htmt(i, j) => htmt.as_ref().map_or(f64::NAN, |m| m[(i, j)]),
        })
        .collect()
}

/// Statistics whose sign "individual sign changes" align.
fn signed(k: &Key) -> bool {
    !matches!(k, Key::R2(_) | Key::F2(..) | Key::Htmt(..))
}

/// A uniform index below `n` (Lemire's unbiased multiply-and-reject).
fn below(rng: &mut ChaCha8Rng, n: usize) -> usize {
    let n64 = n as u64;
    let threshold = n64.wrapping_neg() % n64;
    loop {
        let m = u128::from(rng.next_u64()) * u128::from(n64);
        if (m as u64) >= threshold {
            return (m >> 64) as usize;
        }
    }
}

/// One estimation with sign changes applied; `None` when it fails.
fn run_one(
    s: &Mat,
    spec: &Spec,
    opts: &Options,
    original: &Core,
    signs: SignChanges,
) -> Option<Core> {
    let (mut w, it, conv) = core::iterate(s, spec, opts).ok()?;
    let mut c = core::finish(s, spec, opts, w.clone(), it, conv).ok()?;
    if signs == SignChanges::Construct {
        let mut flipped = false;
        for b in &spec.blocks {
            let same: f64 = b
                .iter()
                .map(|&i| (c.loadings[i] - original.loadings[i]).abs())
                .sum();
            let flip: f64 = b
                .iter()
                .map(|&i| (-c.loadings[i] - original.loadings[i]).abs())
                .sum();
            if flip < same {
                for &i in b {
                    w[i] = -w[i];
                }
                flipped = true;
            }
        }
        if flipped {
            c = core::finish(s, spec, opts, w, it, conv).ok()?;
        }
    }
    Some(c)
}

#[cfg(feature = "parallel")]
fn map_range<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync + Send) -> Vec<T> {
    use rayon::prelude::*;
    (0..n).into_par_iter().map(f).collect()
}

#[cfg(not(feature = "parallel"))]
fn map_range<T: Send>(n: usize, f: impl Fn(usize) -> T + Sync + Send) -> Vec<T> {
    (0..n).map(f).collect()
}

/// One statistic summarised over the subsamples.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Estimate {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub construct: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicator: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<String>>,
    pub original: Option<f64>,
    pub mean: Option<f64>,
    /// Standard deviation of the bootstrap estimates.
    pub se: Option<f64>,
    /// original / se.
    pub t: Option<f64>,
    /// Two-tailed p value (Student t, subsamples − 1 degrees of freedom).
    pub p: Option<f64>,
    /// One-tailed p value (half the two-tailed one).
    pub p_one_tailed: Option<f64>,
    /// Percentile confidence interval [lower, upper].
    pub ci: [Option<f64>; 2],
    /// Bias-corrected and accelerated interval.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ci_bca: Option<[Option<f64>; 2]>,
    /// The upper bound of the one-sided interval (the 1 − alpha
    /// percentile): HTMT is below the threshold when this is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upper_one_sided: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct BootstrapResults {
    /// Subsamples asked for.
    pub samples: usize,
    /// Subsamples that couldn't be estimated (left out).
    pub failed: usize,
    /// Subsamples that reached the iteration limit (kept).
    pub not_converged: usize,
    pub seed: u64,
    pub alpha: f64,
    pub sign_changes: SignChanges,
    pub paths: Vec<Estimate>,
    pub loadings: Vec<Estimate>,
    pub weights: Vec<Estimate>,
    pub r2: Vec<Estimate>,
    pub f2: Vec<Estimate>,
    pub total_indirect: Vec<Estimate>,
    pub specific_indirect: Vec<Estimate>,
    pub total: Vec<Estimate>,
    pub htmt: Vec<Estimate>,
}

fn opt(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

/// R's quantile type 7 of sorted values.
fn quantile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() || !q.is_finite() {
        return f64::NAN;
    }
    let h = (sorted.len() - 1) as f64 * q.clamp(0.0, 1.0);
    let lo = h.floor() as usize;
    let hi = (lo + 1).min(sorted.len() - 1);
    sorted[lo] + (h - lo as f64) * (sorted[hi] - sorted[lo])
}

/// Bootstrap `x` (the model's indicators, centred, n × p row-major).
pub(crate) fn run(
    x: &[f64],
    spec: &Spec,
    opts: &Options,
    boot: &Bootstrap,
    original: &Core,
    s: &Mat,
) -> BootstrapResults {
    let p = spec.p();
    let n = x.len() / p;
    let keys = keys(spec);
    let orig = values(&keys, s, spec, original);
    let signs = boot.sign_changes;
    let samples = boot.samples;
    let runs: Vec<Option<(Vec<f64>, bool)>> = map_range(samples, |b| {
        let mut rng = ChaCha8Rng::seed_from_u64(boot.seed);
        rng.set_stream(b as u64);
        let mut counts = vec![0u32; n];
        for _ in 0..n {
            counts[below(&mut rng, n)] += 1;
        }
        let sb = Moments::of(x, p, Some(&counts)).correlation(p)?;
        let c = run_one(&sb, spec, opts, original, signs)?;
        let mut v = values(&keys, &sb, spec, &c);
        if v.iter()
            .zip(&keys)
            .any(|(x, k)| !x.is_finite() && !matches!(k, Key::F2(..)))
        {
            return None;
        }
        if signs == SignChanges::Individual {
            for ((x, o), k) in v.iter_mut().zip(&orig).zip(&keys) {
                if signed(k) && x.signum() != o.signum() && *o != 0.0 {
                    *x = -*x;
                }
            }
        }
        Some((v, c.converged))
    });
    let failed = runs.iter().filter(|r| r.is_none()).count();
    let not_converged = runs
        .iter()
        .filter(|r| r.as_ref().is_some_and(|r| !r.1))
        .count();
    let ok: Vec<&Vec<f64>> = runs.iter().flatten().map(|r| &r.0).collect();
    // Jackknife (leave one row out) for the BCa acceleration.
    let jack: Option<Vec<Vec<f64>>> = boot.bca.then(|| {
        let full = Moments::of(x, p, None);
        let rows: Vec<Option<Vec<f64>>> = map_range(n, |i| {
            let sj = full.without(x, p, i).correlation(p)?;
            let c = run_one(&sj, spec, opts, original, SignChanges::Construct)?;
            Some(values(&keys, &sj, spec, &c))
        });
        rows.into_iter().flatten().collect()
    });
    let m = ok.len();
    let df = (m.max(2) - 1) as f64;
    let alpha = boot.alpha.clamp(1e-6, 0.5);
    let mut out = BootstrapResults {
        samples,
        failed,
        not_converged,
        seed: boot.seed,
        alpha: boot.alpha,
        sign_changes: signs,
        ..Default::default()
    };
    let name = |j: usize| Some(spec.names[j].clone());
    for (idx, key) in keys.iter().enumerate() {
        let o = orig[idx];
        let mut vals: Vec<f64> = ok
            .iter()
            .map(|v| v[idx])
            .filter(|x| x.is_finite())
            .collect();
        vals.sort_by(f64::total_cmp);
        let mm = vals.len();
        let mean = vals.iter().sum::<f64>() / mm as f64;
        let se = if mm > 1 {
            (vals.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (mm as f64 - 1.0)).sqrt()
        } else {
            f64::NAN
        };
        let t = o / se;
        let pv = t_two_tailed(t, df);
        let ci = [
            opt(quantile(&vals, alpha / 2.0)),
            opt(quantile(&vals, 1.0 - alpha / 2.0)),
        ];
        let ci_bca = jack.as_ref().map(|jack| {
            let below = vals.iter().filter(|v| **v < o).count() as f64;
            let z0 = normal_quantile(below / mm as f64);
            let js: Vec<f64> = jack
                .iter()
                .map(|v| v[idx])
                .filter(|x| x.is_finite())
                .collect();
            let jm = js.iter().sum::<f64>() / js.len() as f64;
            let num: f64 = js.iter().map(|v| (jm - v).powi(3)).sum();
            let den: f64 = js.iter().map(|v| (jm - v).powi(2)).sum();
            let a = if den > 0.0 {
                num / (6.0 * den.powf(1.5))
            } else {
                0.0
            };
            let level = |q: f64| {
                let z = normal_quantile(q);
                normal_cdf(z0 + (z0 + z) / (1.0 - a * (z0 + z)))
            };
            [
                opt(quantile(&vals, level(alpha / 2.0))),
                opt(quantile(&vals, level(1.0 - alpha / 2.0))),
            ]
        });
        let mut e = Estimate {
            original: opt(o),
            mean: opt(mean),
            se: opt(se),
            t: opt(t),
            p: opt(pv),
            p_one_tailed: opt(pv / 2.0),
            ci,
            ci_bca,
            ..Default::default()
        };
        match *key {
            Key::Path(f, t) => {
                e.from = name(f);
                e.to = name(t);
                out.paths.push(e);
            }
            Key::Loading(i) | Key::Weight(i) => {
                e.construct = name(spec.ind_block[i]);
                e.indicator = Some(spec.ind_names[i].clone());
                if matches!(key, Key::Loading(_)) {
                    out.loadings.push(e);
                } else {
                    out.weights.push(e);
                }
            }
            Key::R2(j) => {
                e.construct = name(j);
                out.r2.push(e);
            }
            Key::F2(f, t) => {
                e.from = name(f);
                e.to = name(t);
                out.f2.push(e);
            }
            Key::TotalIndirect(f, t) => {
                e.from = name(f);
                e.to = name(t);
                out.total_indirect.push(e);
            }
            Key::Total(f, t) => {
                e.from = name(f);
                e.to = name(t);
                out.total.push(e);
            }
            Key::Specific(i) => {
                e.path = Some(
                    spec.chains[i]
                        .iter()
                        .map(|&j| spec.names[j].clone())
                        .collect(),
                );
                out.specific_indirect.push(e);
            }
            Key::Htmt(i, j) => {
                e.from = name(i);
                e.to = name(j);
                e.upper_one_sided = opt(quantile(&vals, 1.0 - alpha));
                out.htmt.push(e);
            }
        }
    }
    out
}
