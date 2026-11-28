//! PLS-SEM: partial least squares structural equation modeling, what
//! SmartPLS does, as a pure Rust library (no UI, no file I/O, so it builds
//! for WASM too).
//!
//! - [`Data`][]: an n × p table with column names; missing values are NaN
//!   (or a code such as -99) and are deleted listwise or replaced by means.
//! - [`Model`][]: constructs (reflective or composite, Mode A or B weights)
//!   with their indicators, and the structural paths.
//! - [`estimate`][]: the PLS path algorithm (path, centroid or factorial
//!   weighting) or consistent PLS (PLSc), then outer loadings and weights,
//!   reliability (alpha, rho_A, rho_C, AVE), discriminant validity
//!   (Fornell–Larcker, HTMT, HTMT2, cross-loadings), VIFs, path
//!   coefficients, R², f², indirect and total effects, model fit (SRMR,
//!   d_ULS, d_G, chi², NFI) and, when asked, bootstrap standard errors,
//!   t and p values and percentile/BCa confidence intervals.
//! - [`check`][]: the model against the data before estimating.
//!
//! docs/pls-json.md describes the model and results JSON;
//! docs/decisions/0006-pls-sem.md the choices behind them.

mod bootstrap;
mod check;
mod core;
mod data;
mod dist;
mod fit;
mod linalg;
mod model;
mod results;
mod spec;

use std::borrow::Cow;

pub use bootstrap::Progress;
pub use bootstrap::{BootstrapResults, Estimate};
pub use check::{Finding, Level, check};
pub use data::{Data, Missing};
pub use fit::{Fit, FitMeasures};
pub use model::{
    Algorithm, Bootstrap, Construct, HigherOrder, IndicatorSide, Layout, Measurement, Model,
    Options, Path, SignChanges, WeightMode, Weighting,
};
pub use results::{
    ConstructResult, Effect, Effects, OuterResult, PathResult, Results, SpecificEffect, Table,
};

use linalg::Mat;
use spec::Spec;

/// A message's code and arguments, so a front end can word it in its own
/// language: check findings ([`Finding::check`] with [`Finding::args`]) and
/// result warnings ([`Results::warning_codes`]). The English text is kept
/// beside it for the CLI, MCP and agent tools.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Code {
    pub code: &'static str,
    pub args: Vec<String>,
}

impl Code {
    pub fn new(code: &'static str, args: &[&str]) -> Self {
        Self {
            code,
            args: args.iter().map(|a| (*a).to_owned()).collect(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// The model is malformed or doesn't match the data.
    Model(String),
    /// The data can't be used (too few rows, constant indicators…).
    Data(String),
    /// The algorithm failed (collinear blocks, inadmissible PLSc…).
    Estimation(String),
    /// A feature of a later phase (higher-order constructs).
    Unsupported(String),
    /// Stopped through [`Progress::cancel`].
    Cancelled,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Model(s) => write!(f, "model: {s}"),
            Error::Data(s) => write!(f, "data: {s}"),
            Error::Estimation(s) => write!(f, "estimation: {s}"),
            Error::Unsupported(s) => write!(f, "not supported yet: {s}"),
            Error::Cancelled => write!(f, "stopped"),
        }
    }
}

impl std::error::Error for Error {}

fn opt(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

fn table(rows: &[String], cols: &[String], f: impl Fn(usize, usize) -> f64) -> Table {
    Table {
        rows: rows.to_vec(),
        cols: cols.to_vec(),
        values: (0..rows.len())
            .map(|r| (0..cols.len()).map(|c| opt(f(r, c))).collect())
            .collect(),
    }
}

/// The model's indicators from `data` with missing values handled,
/// centred: `(x, kept rows, means, sds, replaced)`.
struct Prepared {
    x: Vec<f64>,
    kept: Vec<usize>,
    sd: Vec<f64>,
    replaced: usize,
}

fn prepare(data: &Data, spec: &Spec, opts: &Options) -> Result<Prepared, Error> {
    let p = spec.p();
    let (mut x, kept) = data.prepare(&spec.ind_cols, opts.missing);
    let n = kept.len();
    if n < 3 {
        return Err(Error::Data(format!(
            "{n} complete rows: too few to estimate the model"
        )));
    }
    let replaced = match opts.missing {
        Missing::Mean => data.missing_counts(&spec.ind_cols).iter().sum(),
        Missing::Listwise => 0,
    };
    let (mean, sd) = data::mean_sd(&x, p);
    for (i, s) in sd.iter().enumerate() {
        if s.is_nan() || *s <= 0.0 || !s.is_finite() {
            return Err(Error::Data(format!(
                "indicator {} has the same value in every row",
                spec.ind_names[i]
            )));
        }
    }
    for r in 0..n {
        for i in 0..p {
            x[r * p + i] -= mean[i];
        }
    }
    Ok(Prepared {
        x,
        kept,
        sd,
        replaced,
    })
}

/// Estimate `model` on `data`.
pub fn estimate(data: &Data, model: &Model, opts: &Options) -> Result<Results, Error> {
    estimate_with(data, model, opts, None)
}

/// [`estimate`], reporting the bootstrap's progress to `progress` and
/// stopping with [`Error::Cancelled`] when it is cancelled (a UI running
/// it on another thread).
pub fn estimate_with(
    data: &Data,
    model: &Model,
    opts: &Options,
    progress: Option<&Progress>,
) -> Result<Results, Error> {
    let data: Cow<Data> = match opts.missing_value {
        Some(code) => Cow::Owned(data.clone().with_missing_code(code)),
        None => Cow::Borrowed(data),
    };
    let spec = Spec::new(model, &data)?;
    let prep = prepare(&data, &spec, opts)?;
    let (p, k) = (spec.p(), spec.k());
    let n = prep.kept.len();
    let s = data::correlation_two_pass(&prep.x, p)
        .ok_or_else(|| Error::Data("the indicators' correlations can't be computed".into()))?;
    let core = core::estimate(&s, &spec, opts)?;
    let mut warnings = Vec::new();
    let mut warning_codes = Vec::new();
    if !core.converged {
        warnings.push(format!(
            "the algorithm did not converge in {} iterations",
            core.iterations
        ));
        warning_codes.push(Code::new("not_converged", &[&core.iterations.to_string()]));
    }

    // Measurement model.
    let mut outer_vif = vec![1.0; p];
    for b in &spec.blocks {
        if b.len() > 1 {
            match s.select(b, b).inverse() {
                Some(inv) => {
                    for (r, &i) in b.iter().enumerate() {
                        outer_vif[i] = inv[(r, r)];
                    }
                }
                None => {
                    for &i in b {
                        outer_vif[i] = f64::NAN;
                    }
                }
            }
        }
    }
    let outer: Vec<OuterResult> = (0..p)
        .map(|i| OuterResult {
            construct: spec.names[spec.ind_block[i]].clone(),
            indicator: spec.ind_names[i].clone(),
            loading: opt(core.loadings[i]),
            weight: opt(core.w[i]),
            vif: opt(outer_vif[i]),
        })
        .collect();
    let mut ave = vec![f64::NAN; k];
    let constructs: Vec<ConstructResult> = (0..k)
        .map(|j| {
            let b = &spec.blocks[j];
            let m = b.len() as f64;
            let alpha = if b.len() < 2 {
                1.0
            } else {
                let total: f64 = b
                    .iter()
                    .flat_map(|&x| b.iter().map(move |&y| (x, y)))
                    .map(|(x, y)| s[(x, y)])
                    .sum();
                m / (m - 1.0) * (1.0 - m / total)
            };
            let sum_l: f64 = b.iter().map(|&i| core.loadings[i]).sum();
            let err: f64 = b.iter().map(|&i| 1.0 - core.loadings[i].powi(2)).sum();
            let rho_c = sum_l * sum_l / (sum_l * sum_l + err);
            ave[j] = if spec.measurement[j] == Measurement::Reflective {
                b.iter().map(|&i| core.loadings[i].powi(2)).sum::<f64>() / m
            } else {
                f64::NAN
            };
            let r2 = core.r2[j];
            let kp = spec.preds[j].len() as f64;
            let nf = n as f64;
            let r2_adj = 1.0 - (1.0 - r2) * (nf - 1.0) / (nf - kp - 1.0);
            // SmartPLS's BIC: n ln(SSE / n) + (k + 1) ln n, with the
            // construct's residual sum of squares SSE = (n − 1)(1 − R²).
            let bic = nf * ((nf - 1.0) * (1.0 - r2) / nf).ln() + (kp + 1.0) * nf.ln();
            // Reliability describes reflective measurement only (as in
            // SmartPLS): composites have none.
            let refl = spec.measurement[j] == Measurement::Reflective;
            let only = |x: f64| if refl { opt(x) } else { None };
            ConstructResult {
                name: spec.names[j].clone(),
                measurement: spec.measurement[j],
                weights: if spec.mode_b[j] {
                    WeightMode::ModeB
                } else {
                    WeightMode::ModeA
                },
                indicators: b.iter().map(|&i| spec.ind_names[i].clone()).collect(),
                alpha: only(alpha),
                rho_a: only(core.rho_a[j]),
                rho_c: only(rho_c),
                ave: only(ave[j]),
                r2: opt(r2),
                r2_adj: opt(r2_adj),
                bic: opt(bic),
            }
        })
        .collect();
    if opts.algorithm == Algorithm::Plsc {
        for (j, c) in constructs.iter().enumerate() {
            if spec.measurement[j] == Measurement::Reflective && core.rho_a[j] > 1.0 {
                warnings.push(format!(
                    "rho_A of {} is above 1 (an inadmissible PLSc solution)",
                    c.name
                ));
                warning_codes.push(Code::new("rho_a_above_one", &[&c.name]));
            }
        }
        if core.cc.data.iter().any(|r| r.abs() > 1.0 + 1e-12) {
            warnings.push(
                "a corrected construct correlation is above 1 in absolute value (an inadmissible PLSc solution)"
                    .into(),
            );
            warning_codes.push(Code::new("correlation_above_one", &[]));
        }
        if let Some(o) = outer
            .iter()
            .find(|o| o.loading.is_some_and(|l| l.abs() > 1.0))
        {
            warnings.push(format!(
                "the consistent loading of {} is above 1 (a Heywood case)",
                o.indicator
            ));
            warning_codes.push(Code::new("heywood_case", &[&o.indicator]));
        }
    }

    // Structural model.
    let f2 = core::f2(&spec, &core);
    let paths: Vec<PathResult> = spec
        .paths
        .iter()
        .map(|&(f, t)| {
            let pr = &spec.preds[t];
            let vif = if pr.len() < 2 {
                1.0
            } else {
                core.cc.select(pr, pr).inverse().map_or(f64::NAN, |inv| {
                    let r = pr.iter().position(|&l| l == f).unwrap_or(0);
                    inv[(r, r)]
                })
            };
            PathResult {
                from: spec.names[f].clone(),
                to: spec.names[t].clone(),
                coefficient: opt(core.beta[(f, t)]),
                f2: opt(f2[(f, t)]),
                vif: opt(vif),
            }
        })
        .collect();
    let total = core::total_effects(&core.beta);
    let specific = core::specific_indirect(&spec, &core.beta);
    let mut effects = Effects::default();
    for &f in &spec.order {
        for &t in &spec.order {
            if spec.reaches(f, t) {
                effects.total.push(Effect {
                    from: spec.names[f].clone(),
                    to: spec.names[t].clone(),
                    effect: opt(total[(f, t)]),
                });
            }
            if spec.has_indirect(f, t) {
                effects.total_indirect.push(Effect {
                    from: spec.names[f].clone(),
                    to: spec.names[t].clone(),
                    effect: opt(total[(f, t)] - core.beta[(f, t)]),
                });
            }
        }
    }
    effects.specific_indirect = spec
        .chains
        .iter()
        .zip(&specific)
        .map(|(c, &e)| SpecificEffect {
            path: c.iter().map(|&j| spec.names[j].clone()).collect(),
            effect: opt(e),
        })
        .collect();

    let names = &spec.names;
    let correlations = table(names, names, |a, b| core.cc[(a, b)]);
    let fornell_larcker = table(names, names, |a, b| match a.cmp(&b) {
        std::cmp::Ordering::Equal => ave[a].sqrt(),
        std::cmp::Ordering::Greater => core.cc[(a, b)],
        std::cmp::Ordering::Less => f64::NAN,
    });
    let (h1, h2) = core::htmt(&s, &spec);
    let htmt = table(
        names,
        names,
        |a, b| if a > b { h1[(a, b)] } else { f64::NAN },
    );
    let htmt2 = table(
        names,
        names,
        |a, b| if a > b { h2[(a, b)] } else { f64::NAN },
    );
    let cross_loadings = table(&spec.ind_names, names, |i, j| core.sw[(i, j)]);
    let fit = fit::fit(&s, &spec, &core, n);

    // Construct scores.
    let score_rows: Vec<String> = prep.kept.iter().map(|r| (r + 1).to_string()).collect();
    let mut scores = Mat::zeros(n, k);
    for r in 0..n {
        for i in 0..p {
            scores[(r, spec.ind_block[i])] += core.w[i] * prep.x[r * p + i] / prep.sd[i];
        }
    }
    let scores = table(&score_rows, names, |r, j| scores[(r, j)]);

    let bootstrap = opts
        .bootstrap
        .as_ref()
        .filter(|b| b.samples > 0)
        .map(|b| bootstrap::run(&prep.x, &spec, opts, b, &core, &s, progress))
        .transpose()?;
    if let Some(b) = &bootstrap
        && b.failed > 0
    {
        warnings.push(format!(
            "{} of {} bootstrap subsamples could not be estimated and were left out",
            b.failed, b.samples
        ));
        warning_codes.push(Code::new(
            "bootstrap_failed",
            &[&b.failed.to_string(), &b.samples.to_string()],
        ));
    }

    Ok(Results {
        algorithm: opts.algorithm,
        weighting: opts.weighting,
        n,
        rows: data.rows(),
        dropped: data.rows() - n,
        replaced: prep.replaced,
        iterations: core.iterations,
        converged: core.converged,
        constructs,
        outer,
        paths,
        correlations,
        fornell_larcker,
        htmt,
        htmt2,
        cross_loadings,
        effects,
        fit,
        bootstrap,
        warnings,
        warning_codes,
        scores,
    })
}
