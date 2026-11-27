//! The PLS path algorithm (Wold, Lohmöller) and consistent PLS (Dijkstra
//! & Henseler 2015), run on the indicators' correlation matrix: every
//! quantity PLS needs is a moment of the standardized data, so one
//! estimation costs O(p²) per iteration whatever the sample size.

use crate::Error;
use crate::linalg::Mat;
use crate::model::{Algorithm, Measurement, Options, Weighting};
use crate::spec::Spec;

/// What one estimation gives (the full sample, a bootstrap subsample or a
/// jackknife sample).
#[derive(Clone, Debug)]
pub(crate) struct Core {
    /// Outer weights, scaled so that each construct score has variance 1.
    pub w: Vec<f64>,
    pub iterations: usize,
    pub converged: bool,
    /// p × k correlations of indicators with construct scores
    /// (cross-loadings).
    pub sw: Mat,
    /// k × k correlations of the construct scores.
    pub c: Mat,
    /// Dijkstra–Henseler's rho_A per construct (1 for single indicators).
    pub rho_a: Vec<f64>,
    /// The construct correlations the structural model uses: corrected
    /// for attenuation in PLSc.
    pub cc: Mat,
    /// Outer loadings as reported: consistent ones for reflective
    /// constructs in PLSc.
    pub loadings: Vec<f64>,
    /// Path coefficients, `beta[(from, to)]`.
    pub beta: Mat,
    /// R² per construct (NaN for exogenous ones).
    pub r2: Vec<f64>,
}

/// One weight per indicator → a p × k matrix.
fn w_full(spec: &Spec, w: &[f64]) -> Mat {
    let mut m = Mat::zeros(spec.p(), spec.k());
    for (i, &b) in spec.ind_block.iter().enumerate() {
        m[(i, b)] = w[i];
    }
    m
}

/// Scale each block's weights so its construct score has variance 1.
fn normalize(s: &Mat, spec: &Spec, w: &mut [f64]) -> Result<(), Error> {
    for (j, b) in spec.blocks.iter().enumerate() {
        let mut v = 0.0;
        for &a in b {
            for &c in b {
                v += w[a] * s[(a, c)] * w[c];
            }
        }
        if !(v > 1e-300) || !v.is_finite() {
            return Err(Error::Estimation(format!(
                "the weights of {} vanished",
                spec.names[j]
            )));
        }
        let f = v.sqrt();
        for &a in b {
            w[a] /= f;
        }
    }
    Ok(())
}

/// The inner weights `e[(l, j)]`: how much construct `l` counts in the
/// inner proxy of construct `j`.
fn inner_weights(spec: &Spec, c: &Mat, scheme: Weighting) -> Result<Mat, Error> {
    let k = spec.k();
    let mut e = Mat::zeros(k, k);
    for &(f, t) in &spec.paths {
        let v = match scheme {
            Weighting::Centroid => c[(f, t)].signum(),
            Weighting::Factorial | Weighting::Path => c[(f, t)],
        };
        e[(f, t)] = v;
        e[(t, f)] = v;
    }
    if scheme == Weighting::Path {
        for j in 0..k {
            let p = &spec.preds[j];
            if p.is_empty() {
                continue;
            }
            let beta = c
                .select(p, p)
                .solve(&c.select(p, &[j]))
                .ok_or_else(|| singular(spec, j))?;
            for (i, &l) in p.iter().enumerate() {
                e[(l, j)] = beta[(i, 0)];
            }
        }
    }
    Ok(e)
}

fn singular(spec: &Spec, j: usize) -> Error {
    Error::Estimation(format!(
        "the predictors of {} are perfectly collinear",
        spec.names[j]
    ))
}

/// The outer weights at convergence: `(weights, iterations, converged)`.
pub(crate) fn iterate(
    s: &Mat,
    spec: &Spec,
    opts: &Options,
) -> Result<(Vec<f64>, usize, bool), Error> {
    let p = spec.p();
    let mut w = vec![1.0; p];
    normalize(s, spec, &mut w)?;
    // Mode B blocks' correlation matrices, solved once.
    let mode_b_inv: Vec<Option<Mat>> = spec
        .blocks
        .iter()
        .enumerate()
        .map(|(j, b)| {
            if spec.mode_b[j] {
                s.select(b, b).inverse().map(Some).ok_or_else(|| {
                    Error::Estimation(format!(
                        "the indicators of {} are perfectly collinear (Mode B needs them independent)",
                        spec.names[j]
                    ))
                })
            } else {
                Ok(None)
            }
        })
        .collect::<Result<_, _>>()?;
    let max = opts.max_iterations.max(1);
    for it in 1..=max {
        let wm = w_full(spec, &w);
        let sw = s.mul(&wm);
        let c = wm.t().mul(&sw);
        let e = inner_weights(spec, &c, opts.weighting)?;
        // Covariances of the indicators with the inner proxies.
        let t = sw.mul(&e);
        let mut nw = vec![0.0; p];
        for (j, b) in spec.blocks.iter().enumerate() {
            match &mode_b_inv[j] {
                Some(inv) => {
                    for (r, &a) in b.iter().enumerate() {
                        nw[a] = b
                            .iter()
                            .enumerate()
                            .map(|(cix, &cc)| inv[(r, cix)] * t[(cc, j)])
                            .sum();
                    }
                }
                None => {
                    for &a in b {
                        nw[a] = t[(a, j)];
                    }
                }
            }
        }
        normalize(s, spec, &mut nw)?;
        let diff: f64 = nw.iter().zip(&w).map(|(a, b)| (a - b).abs()).sum();
        w = nw;
        if diff < opts.stop {
            return Ok((w, it, true));
        }
    }
    Ok((w, max, false))
}

/// rho_A and the consistency factor ĉ of a block (Dijkstra & Henseler
/// 2015): ρ_A = (w'w)² · w'(S − diag S)w / w'(ww' − diag ww')w with the
/// weights scaled to unit score variance, and ĉ = √ρ_A / w'w.
fn rho_a(s: &Mat, b: &[usize], w: &[f64]) -> (f64, f64) {
    if b.len() < 2 {
        return (1.0, 1.0);
    }
    let mut num = 0.0;
    for &x in b {
        for &y in b {
            if x != y {
                num += w[x] * s[(x, y)] * w[y];
            }
        }
    }
    let ww: f64 = b.iter().map(|&x| w[x] * w[x]).sum();
    let w4: f64 = b.iter().map(|&x| w[x].powi(4)).sum();
    let den = ww * ww - w4;
    let c2 = num / den;
    (ww * ww * c2, if c2 > 0.0 { c2.sqrt() } else { f64::NAN })
}

/// Everything else from the converged weights.
pub(crate) fn finish(
    s: &Mat,
    spec: &Spec,
    opts: &Options,
    w: Vec<f64>,
    iterations: usize,
    converged: bool,
) -> Result<Core, Error> {
    let k = spec.k();
    let wm = w_full(spec, &w);
    let sw = s.mul(&wm);
    let c = wm.t().mul(&sw);
    let mut rho = vec![1.0; k];
    let mut chat = vec![1.0; k];
    for (j, b) in spec.blocks.iter().enumerate() {
        let (r, ch) = rho_a(s, b, &w);
        rho[j] = r;
        chat[j] = ch;
    }
    let consistent = |j: usize| {
        opts.algorithm == Algorithm::Plsc
            && spec.measurement[j] == Measurement::Reflective
            && spec.blocks[j].len() > 1
    };
    let mut q = vec![1.0; k];
    for j in 0..k {
        if consistent(j) {
            if !(rho[j] > 0.0) || !chat[j].is_finite() {
                return Err(Error::Estimation(format!(
                    "rho_A of {} is not positive, so PLSc can't correct it",
                    spec.names[j]
                )));
            }
            q[j] = rho[j].sqrt();
        }
    }
    let cc = Mat::from_fn(k, k, |a, b| {
        if a == b {
            1.0
        } else {
            c[(a, b)] / (q[a] * q[b])
        }
    });
    let loadings: Vec<f64> = (0..spec.p())
        .map(|i| {
            let j = spec.ind_block[i];
            if consistent(j) {
                chat[j] * w[i]
            } else {
                sw[(i, j)]
            }
        })
        .collect();
    let mut beta = Mat::zeros(k, k);
    let mut r2 = vec![f64::NAN; k];
    for j in 0..k {
        let p = &spec.preds[j];
        if p.is_empty() {
            continue;
        }
        let rhs = cc.select(p, &[j]);
        let b = cc
            .select(p, p)
            .solve(&rhs)
            .ok_or_else(|| singular(spec, j))?;
        let mut r = 0.0;
        for (i, &l) in p.iter().enumerate() {
            beta[(l, j)] = b[(i, 0)];
            r += b[(i, 0)] * rhs[(i, 0)];
        }
        r2[j] = r;
    }
    Ok(Core {
        w,
        iterations,
        converged,
        sw,
        c,
        rho_a: rho,
        cc,
        loadings,
        beta,
        r2,
    })
}

/// R² of construct `j` regressed on its predecessors except `drop`.
pub(crate) fn r2_without(spec: &Spec, cc: &Mat, j: usize, drop: usize) -> f64 {
    let p: Vec<usize> = spec.preds[j]
        .iter()
        .copied()
        .filter(|&l| l != drop)
        .collect();
    if p.is_empty() {
        return 0.0;
    }
    let rhs = cc.select(&p, &[j]);
    match cc.select(&p, &p).solve(&rhs) {
        Some(b) => (0..p.len()).map(|i| b[(i, 0)] * rhs[(i, 0)]).sum(),
        None => f64::NAN,
    }
}

/// Cohen's f² of each path: (R² − R²_without) / (1 − R²).
pub(crate) fn f2(spec: &Spec, core: &Core) -> Mat {
    let k = spec.k();
    let mut f = Mat::from_fn(k, k, |_, _| f64::NAN);
    for &(l, j) in &spec.paths {
        let r2 = core.r2[j];
        f[(l, j)] = (r2 - r2_without(spec, &core.cc, j, l)) / (1.0 - r2);
    }
    f
}

/// Total effects: B + B² + … (the model is recursive, so the sum ends).
pub(crate) fn total_effects(beta: &Mat) -> Mat {
    let k = beta.rows;
    let mut total = beta.clone();
    let mut pow = beta.clone();
    for _ in 1..k {
        pow = pow.mul(beta);
        if pow.data.iter().all(|x| *x == 0.0) {
            break;
        }
        for (t, p) in total.data.iter_mut().zip(&pow.data) {
            *t += p;
        }
    }
    total
}

/// The product of the path coefficients along each chain.
pub(crate) fn specific_indirect(spec: &Spec, beta: &Mat) -> Vec<f64> {
    spec.chains
        .iter()
        .map(|c| c.windows(2).map(|w| beta[(w[0], w[1])]).product())
        .collect()
}

/// HTMT (Henseler, Ringle & Sarstedt 2015) with absolute correlations as
/// SmartPLS 4 does, and HTMT2 (Roemer, Schuberth & Henseler 2021) with
/// geometric means. As in SmartPLS, a single indicator's monotrait mean is
/// 1, and pairs with a composite (formative) construct have none (NaN).
pub(crate) fn htmt(s: &Mat, spec: &Spec) -> (Mat, Mat) {
    let k = spec.k();
    let mono = |b: &[usize], geo: bool| -> f64 {
        if b.len() < 2 {
            return 1.0;
        }
        let mut acc = 0.0;
        let mut n = 0.0;
        for (x, &a) in b.iter().enumerate() {
            for &c in &b[x + 1..] {
                let r = s[(a, c)].abs();
                acc += if geo { r.ln() } else { r };
                n += 1.0;
            }
        }
        if geo { (acc / n).exp() } else { acc / n }
    };
    let hetero = |a: &[usize], b: &[usize], geo: bool| -> f64 {
        let mut acc = 0.0;
        let mut n = 0.0;
        for &x in a {
            for &y in b {
                let r = s[(x, y)].abs();
                acc += if geo { r.ln() } else { r };
                n += 1.0;
            }
        }
        if geo { (acc / n).exp() } else { acc / n }
    };
    let mut h1 = Mat::from_fn(k, k, |_, _| f64::NAN);
    let mut h2 = h1.clone();
    let m1: Vec<f64> = spec.blocks.iter().map(|b| mono(b, false)).collect();
    let m2: Vec<f64> = spec.blocks.iter().map(|b| mono(b, true)).collect();
    for i in 0..k {
        for j in 0..i {
            if spec.measurement[i] != Measurement::Reflective
                || spec.measurement[j] != Measurement::Reflective
            {
                continue;
            }
            let (bi, bj) = (&spec.blocks[i], &spec.blocks[j]);
            let v1 = hetero(bi, bj, false) / (m1[i] * m1[j]).sqrt();
            let v2 = hetero(bi, bj, true) / (m2[i] * m2[j]).sqrt();
            h1[(i, j)] = v1;
            h1[(j, i)] = v1;
            h2[(i, j)] = v2;
            h2[(j, i)] = v2;
        }
    }
    (h1, h2)
}

/// One estimation from a correlation matrix.
pub(crate) fn estimate(s: &Mat, spec: &Spec, opts: &Options) -> Result<Core, Error> {
    let (w, it, conv) = iterate(s, spec, opts)?;
    finish(s, spec, opts, w, it, conv)
}
