//! Model fit: the indicators' correlation matrix the model implies,
//! compared with the observed one (SRMR, d_ULS, d_G, chi², NFI).
//!
//! As SmartPLS 4 (and cSEM with `=~`): reflective constructs are common
//! factors, implying λλ' within their block (with PLS's loadings in PLS,
//! the consistent ones in PLSc); composites (formative constructs) keep
//! their block's observed correlations. Between blocks the implied
//! correlation is λ_a · φ_ij · λ_b. The *saturated* model takes φ as
//! estimated; the *estimated* model takes the construct correlations the
//! structural model implies (as cSEM: endogenous variances from the
//! structural equations, then set to 1). d_G uses base-10 logarithms, as
//! SmartPLS and cSEM do (Dijkstra & Henseler 2015 write ln).

use serde::Serialize;

use crate::core::Core;
use crate::linalg::Mat;
use crate::model::Measurement;
use crate::spec::Spec;

/// Fit measures of one implied matrix.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FitMeasures {
    pub srmr: Option<f64>,
    pub d_uls: Option<f64>,
    pub d_g: Option<f64>,
    pub chi2: Option<f64>,
    pub nfi: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Fit {
    pub saturated: FitMeasures,
    pub estimated: FitMeasures,
}

/// Construct correlations implied by the structural model: exogenous
/// constructs keep theirs; each endogenous construct is its predecessors'
/// linear combination plus a disturbance (variance 1 − R²) uncorrelated
/// with everything before it.
pub(crate) fn implied_constructs(spec: &Spec, core: &Core) -> Mat {
    let k = spec.k();
    let mut phi = core.cc.clone();
    let mut done: Vec<usize> = Vec::new();
    for &j in &spec.order {
        if !spec.preds[j].is_empty() {
            for &m in &done {
                let v: f64 = spec.preds[j]
                    .iter()
                    .map(|&l| core.beta[(l, j)] * phi[(l, m)])
                    .sum();
                phi[(j, m)] = v;
                phi[(m, j)] = v;
            }
            let p = &spec.preds[j];
            let mut var = 1.0 - core.r2[j];
            for &a in p {
                for &b in p {
                    var += core.beta[(a, j)] * core.beta[(b, j)] * phi[(a, b)];
                }
            }
            phi[(j, j)] = var;
        }
        done.push(j);
    }
    for j in 0..k {
        phi[(j, j)] = 1.0;
    }
    phi
}

pub(crate) fn implied_indicators(s: &Mat, spec: &Spec, core: &Core, phi: &Mat) -> Mat {
    let p = spec.p();
    let factor =
        |j: usize| spec.measurement[j] == Measurement::Reflective && spec.blocks[j].len() > 1;
    Mat::from_fn(p, p, |a, b| {
        let (ja, jb) = (spec.ind_block[a], spec.ind_block[b]);
        if a == b {
            1.0
        } else if ja == jb {
            if factor(ja) {
                core.loadings[a] * core.loadings[b]
            } else {
                s[(a, b)]
            }
        } else {
            core.loadings[a] * phi[(ja, jb)] * core.loadings[b]
        }
    })
}

fn opt(x: f64) -> Option<f64> {
    x.is_finite().then_some(x)
}

pub(crate) fn measures(s: &Mat, sigma: &Mat, n: usize) -> FitMeasures {
    let p = s.rows;
    let mut sq = 0.0;
    let mut tri = 0.0;
    for a in 0..p {
        for b in 0..=a {
            let d = s[(a, b)] - sigma[(a, b)];
            sq += d * d;
            tri += 1.0;
        }
    }
    let srmr = (sq / tri).sqrt();
    let mut full = 0.0;
    for (x, y) in s.data.iter().zip(&sigma.data) {
        full += (x - y) * (x - y);
    }
    let d_uls = 0.5 * full;
    // d_G: eigenvalues of S⁻¹Σ̂, as those of L⁻¹Σ̂L⁻ᵀ (S = LLᵀ).
    let d_g = s
        .cholesky()
        .and_then(|l| l.inverse())
        .map(|li| {
            let m = li.mul(sigma).mul(&li.t());
            let m = Mat::from_fn(p, p, |a, b| 0.5 * (m[(a, b)] + m[(b, a)]));
            0.5 * m
                .sym_eigenvalues()
                .iter()
                .map(|e| e.log10().powi(2))
                .sum::<f64>()
        })
        .unwrap_or(f64::NAN);
    let (chi2, nfi) = match (s.log_det(), sigma.log_det(), sigma.inverse()) {
        (Some((lds, ss)), Some((ldm, sm)), Some(inv)) if ss > 0.0 && sm > 0.0 => {
            let tr: f64 = (0..p)
                .map(|a| (0..p).map(|b| s[(a, b)] * inv[(b, a)]).sum::<f64>())
                .sum();
            let f = ldm + tr - lds - p as f64;
            let m = (n as f64 - 1.0).max(0.0);
            let chi2 = m * f;
            let null = m * -lds;
            (chi2, 1.0 - chi2 / null)
        }
        _ => (f64::NAN, f64::NAN),
    };
    FitMeasures {
        srmr: opt(srmr),
        d_uls: opt(d_uls),
        d_g: opt(d_g),
        chi2: opt(chi2),
        nfi: opt(nfi),
    }
}

pub(crate) fn fit(s: &Mat, spec: &Spec, core: &Core, n: usize) -> Fit {
    let sat = implied_indicators(s, spec, core, &core.cc);
    let est = implied_indicators(s, spec, core, &implied_constructs(spec, core));
    Fit {
        saturated: measures(s, &sat, n),
        estimated: measures(s, &est, n),
    }
}
