//! kantor_pls against reference implementations.
//!
//! - `cases.json` + `goldens/*.json`: synthetic datasets (tests/data, made by
//!   tools/pls/gen_data.py) estimated by seminr and cSEM
//!   (tools/pls/goldens.R). Non-random results agree to 1e-6; bootstrap
//!   standard errors and intervals loosely (different random numbers).
//! - `smartpls.json`: SmartPLS 4's own output for 14 textbook models
//!   (tools/pls/smartpls_refs.py), printed to three decimals, so they agree
//!   to 0.0005. Their datasets aren't in the repository: set
//!   KANTOR_PLS_SMARTPLS to the unzipped supplement's `supplementary`
//!   folder (CI downloads it), otherwise these cases are skipped.

use std::path::{Path, PathBuf};

use kantor_pls::{Bootstrap, Data, Model, Options, Results, estimate};
use serde_json::Value;

fn dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests")
}

/// A CSV (or `;` / tab separated) file of numbers; empty cells are NaN.
fn read_table(path: &Path) -> Data {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut lines = text.lines().filter(|l| !l.trim().is_empty());
    let header = lines.next().expect("header");
    let sep = [';', '\t', ',']
        .into_iter()
        .max_by_key(|c| header.matches(*c).count())
        .unwrap();
    let names: Vec<String> = header.split(sep).map(|s| s.trim().to_owned()).collect();
    let rows: Vec<Vec<f64>> = lines
        .map(|l| {
            l.split(sep)
                .map(|v| {
                    let v = v.trim();
                    if v.is_empty() {
                        f64::NAN
                    } else {
                        v.parse().unwrap_or(f64::NAN)
                    }
                })
                .collect()
        })
        .collect();
    Data::new(names, rows).unwrap()
}

struct Check {
    case: String,
    failures: Vec<String>,
    compared: usize,
}

impl Check {
    fn new(case: &str) -> Self {
        Self {
            case: case.to_owned(),
            failures: Vec::new(),
            compared: 0,
        }
    }

    fn near(&mut self, what: &str, mine: Option<f64>, want: Option<f64>, tol: f64) {
        let Some(want) = want else { return };
        self.compared += 1;
        match mine {
            Some(m) if (m - want).abs() <= tol => {}
            _ => self.failures.push(format!(
                "{}: {what}: kantor {mine:?}, reference {want} (tolerance {tol})",
                self.case
            )),
        }
    }

    fn done(self) -> Vec<String> {
        assert!(self.compared > 0, "{}: nothing compared", self.case);
        self.failures
    }
}

fn f(v: &Value) -> Option<f64> {
    v.as_f64()
}

const TOL: f64 = 1e-6;

/// `plsc`: seminr takes PLSc's Fornell-Larcker correlations and inner VIFs
/// from the construct scores (attenuated); Kantor from the consistent
/// construct correlations its paths come from, so those two
/// aren't compared for PLSc cases.
fn compare_seminr(c: &mut Check, r: &Results, s: &Value, plsc: bool) {
    for p in s["paths"].as_array().unwrap() {
        let (from, to) = (p["from"].as_str().unwrap(), p["to"].as_str().unwrap());
        let mine = r.path(from, to).and_then(|x| x.coefficient);
        c.near(&format!("path {from} -> {to}"), mine, f(&p["value"]), TOL);
    }
    for (ind, v) in s["loadings"].as_object().unwrap() {
        c.near(
            &format!("loading {ind}"),
            r.outer(ind).and_then(|o| o.loading),
            f(v),
            TOL,
        );
    }
    for (ind, v) in s["weights"].as_object().unwrap() {
        c.near(
            &format!("weight {ind}"),
            r.outer(ind).and_then(|o| o.weight),
            f(v),
            TOL,
        );
    }
    for (name, v) in s["r2"].as_object().unwrap() {
        c.near(
            &format!("R² {name}"),
            r.construct(name).and_then(|x| x.r2),
            f(v),
            TOL,
        );
    }
    for (name, v) in s["r2_adj"].as_object().unwrap() {
        c.near(
            &format!("adjusted R² {name}"),
            r.construct(name).and_then(|x| x.r2_adj),
            f(v),
            TOL,
        );
    }
    for (name, rel) in s["reliability"].as_object().unwrap() {
        let Some(con) = r.construct(name) else {
            c.failures.push(format!("{}: no construct {name}", c.case));
            continue;
        };
        if con.alpha.is_none() {
            continue; // composite: reliability isn't reported
        }
        c.near(&format!("alpha {name}"), con.alpha, f(&rel["alpha"]), TOL);
        c.near(&format!("rho_A {name}"), con.rho_a, f(&rel["rhoA"]), TOL);
        c.near(&format!("rho_C {name}"), con.rho_c, f(&rel["rhoC"]), TOL);
        c.near(&format!("AVE {name}"), con.ave, f(&rel["AVE"]), TOL);
    }
    let reflective = |n: &str| r.construct(n).is_some_and(|x| x.alpha.is_some());
    for h in s["htmt"].as_array().unwrap() {
        let (a, b) = (h["from"].as_str().unwrap(), h["to"].as_str().unwrap());
        if !(reflective(a) && reflective(b)) {
            continue;
        }
        let mine = r.htmt.get(a, b).or_else(|| r.htmt.get(b, a));
        c.near(&format!("HTMT {a}/{b}"), mine, f(&h["value"]), TOL);
    }
    for e in s["fornell_larcker"].as_array().unwrap() {
        let (a, b) = (e["from"].as_str().unwrap(), e["to"].as_str().unwrap());
        if (a == b && !reflective(a)) || (plsc && a != b) {
            continue;
        }
        c.near(
            &format!("Fornell-Larcker {a}/{b}"),
            r.fornell_larcker.get(a, b),
            f(&e["value"]),
            TOL,
        );
    }
    for (ind, row) in s["cross_loadings"].as_object().unwrap() {
        for (con, v) in row.as_object().unwrap() {
            // PLSc's own-construct entry is the consistent loading in
            // seminr; Kantor's cross-loadings are always correlations.
            if r.outer(ind).is_some_and(|o| o.construct == *con) {
                continue;
            }
            c.near(
                &format!("cross-loading {ind}/{con}"),
                r.cross_loadings.get(ind, con),
                f(v),
                TOL,
            );
        }
    }
    for (ind, v) in s["vif_items"].as_object().unwrap() {
        c.near(
            &format!("outer VIF {ind}"),
            r.outer(ind).and_then(|o| o.vif),
            f(v),
            TOL,
        );
    }
    if let Some(list) = s["vif_antecedents"].as_array().filter(|_| !plsc) {
        for e in list {
            let (a, b) = (e["from"].as_str().unwrap(), e["to"].as_str().unwrap());
            c.near(
                &format!("inner VIF {a} -> {b}"),
                r.path(a, b).and_then(|p| p.vif),
                f(&e["value"]),
                1e-5,
            );
        }
    }
    // seminr's f² re-estimates the model without each predictor; SmartPLS
    // (and Kantor) take R² without it from the same construct scores. The
    // SmartPLS cases check f².
    let effect = |list: &[kantor_pls::Effect], a: &str, b: &str| {
        list.iter()
            .find(|e| e.from == a && e.to == b)
            .and_then(|e| e.effect)
    };
    for e in s["total"].as_array().unwrap() {
        let (a, b) = (e["from"].as_str().unwrap(), e["to"].as_str().unwrap());
        c.near(
            &format!("total effect {a} -> {b}"),
            effect(&r.effects.total, a, b),
            f(&e["value"]),
            TOL,
        );
    }
    for e in s["total_indirect"].as_array().unwrap() {
        let (a, b) = (e["from"].as_str().unwrap(), e["to"].as_str().unwrap());
        let mine = effect(&r.effects.total_indirect, a, b);
        c.near(
            &format!("total indirect {a} -> {b}"),
            mine,
            f(&e["value"]),
            TOL,
        );
    }
}

fn compare_csem(c: &mut Check, r: &Results, s: &Value) {
    for p in s["paths"].as_array().unwrap() {
        let (from, to) = (p["from"].as_str().unwrap(), p["to"].as_str().unwrap());
        let mine = r.path(from, to).and_then(|x| x.coefficient);
        c.near(
            &format!("cSEM path {from} -> {to}"),
            mine,
            f(&p["value"]),
            TOL,
        );
    }
    for (ind, v) in s["loadings"].as_object().unwrap() {
        c.near(
            &format!("cSEM loading {ind}"),
            r.outer(ind).and_then(|o| o.loading),
            f(v),
            TOL,
        );
    }
    for (ind, v) in s["weights"].as_object().unwrap() {
        c.near(
            &format!("cSEM weight {ind}"),
            r.outer(ind).and_then(|o| o.weight),
            f(v),
            TOL,
        );
    }
    for (which, mine) in [
        ("saturated", &r.fit.saturated),
        ("estimated", &r.fit.estimated),
    ] {
        let w = &s["fit"][which];
        c.near(&format!("{which} SRMR"), mine.srmr, f(&w["srmr"]), TOL);
        c.near(&format!("{which} d_ULS"), mine.d_uls, f(&w["d_uls"]), TOL);
        c.near(&format!("{which} d_G"), mine.d_g, f(&w["d_g"]), TOL);
        c.near(&format!("{which} NFI"), mine.nfi, f(&w["nfi"]), TOL);
        // chi² is (n − 1) times a likelihood: compare relatively.
        let want = f(&w["chi2"]);
        c.near(
            &format!("{which} chi²"),
            mine.chi2,
            want,
            want.map_or(0.0, |x| x.abs() * 1e-7 + 1e-6),
        );
    }
    if let Some(list) = s["htmt2"].as_array() {
        for h in list {
            let (a, b) = (h["from"].as_str().unwrap(), h["to"].as_str().unwrap());
            let mine = r.htmt2.get(a, b).or_else(|| r.htmt2.get(b, a));
            c.near(&format!("HTMT2 {a}/{b}"), mine, f(&h["value"]), TOL);
        }
    }
}

/// seminr's bootstrap (another random stream): standard errors within
/// 15% and 2.5%/97.5% percentiles within 0.4 standard errors (the tails
/// of 2000 subsamples vary by about 0.1 SE from one stream to another,
/// more for skewed estimates).
fn compare_bootstrap(c: &mut Check, r: &Results, b: &Value, paths_only: bool) {
    let boot = r.bootstrap.as_ref().expect("bootstrap results");
    // PLSc can fail on a rare subsample (rho_A not positive); those are left
    // out, as seminr and SmartPLS do. More than 1% would be a bug.
    if boot.failed * 100 > boot.samples {
        c.failures
            .push(format!("{}: {} failed subsamples", c.case, boot.failed));
    }
    let mut one = |label: &str, mine: Option<&kantor_pls::Estimate>, want: &Value| {
        let Some(m) = mine else {
            c.failures
                .push(format!("{}: no bootstrap estimate {label}", c.case));
            return;
        };
        let sd = f(&want["sd"]).unwrap();
        // A single indicator's loading is 1 in every subsample (SE 0 up to rounding).
        c.near(
            &format!("bootstrap SE {label}"),
            m.se,
            Some(sd),
            0.15 * sd + 1e-9,
        );
        c.near(
            &format!("bootstrap 2.5% {label}"),
            m.ci[0],
            f(&want["lower"]),
            0.4 * sd + 0.005,
        );
        c.near(
            &format!("bootstrap 97.5% {label}"),
            m.ci[1],
            f(&want["upper"]),
            0.4 * sd + 0.005,
        );
    };
    for p in b["paths"].as_array().unwrap() {
        let label = p["label"].as_str().unwrap();
        let (a, t) = label.split_once(" -> ").unwrap();
        let mine = boot
            .paths
            .iter()
            .find(|e| e.from.as_deref() == Some(a) && e.to.as_deref() == Some(t));
        one(label, mine, p);
    }
    if paths_only {
        return;
    }
    for p in b["loadings"].as_array().unwrap() {
        let label = p["label"].as_str().unwrap();
        let (ind, _) = label.split_once(" -> ").unwrap();
        let mine = boot
            .loadings
            .iter()
            .find(|e| e.indicator.as_deref() == Some(ind));
        one(label, mine, p);
    }
}

fn options(v: &Value) -> Options {
    serde_json::from_value(v.clone()).expect("case options")
}

#[test]
fn seminr_and_csem_goldens() {
    let cases: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(dir().join("cases.json")).unwrap()).unwrap();
    let mut failures = Vec::new();
    for case in &cases {
        let name = case["name"].as_str().unwrap();
        let golden: Value = serde_json::from_str(
            &std::fs::read_to_string(dir().join("goldens").join(format!("{name}.json")))
                .unwrap_or_else(|e| panic!("golden {name}: {e} (run tools/pls/goldens.R)")),
        )
        .unwrap();
        let data = read_table(&dir().join("data").join(case["data"].as_str().unwrap()));
        let model: Model = serde_json::from_str(
            &std::fs::read_to_string(dir().join("models").join(case["model"].as_str().unwrap()))
                .unwrap(),
        )
        .unwrap();
        let mut opts = options(&case["options"]);
        let boot = case["bootstrap"].as_u64().unwrap_or(0) as usize;
        if boot > 0 {
            opts.bootstrap = Some(Bootstrap {
                samples: boot,
                seed: 7,
                bca: false,
                ..Default::default()
            });
        }
        let r = estimate(&data, &model, &opts).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut c = Check::new(name);
        assert_eq!(
            r.n as u64,
            golden["n"].as_u64().unwrap(),
            "{name}: rows estimated"
        );
        if golden["seminr"].is_object() {
            let plsc = case["options"]["algorithm"] == "plsc";
            compare_seminr(&mut c, &r, &golden["seminr"], plsc);
            if let Some(b) = golden["seminr"].get("bootstrap") {
                // The mediation case is an inadmissible PLSc solution (a
                // loading above 1): only its paths are stable enough.
                compare_bootstrap(&mut c, &r, b, name.starts_with("mediation"));
            }
        }
        compare_csem(&mut c, &r, &golden["csem"]);
        failures.extend(c.done());
    }
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn smartpls_reference_values() {
    let Ok(sup) = std::env::var("KANTOR_PLS_SMARTPLS") else {
        eprintln!("KANTOR_PLS_SMARTPLS is not set: SmartPLS reference cases skipped");
        return;
    };
    let sup = PathBuf::from(sup);
    let refs: Value =
        serde_json::from_str(&std::fs::read_to_string(dir().join("smartpls.json")).unwrap())
            .unwrap();
    // SmartPLS prints three decimals.
    const T: f64 = 0.000_501;
    let mut failures = Vec::new();
    for case in refs["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let data = read_table(&sup.join(case["dataset"].as_str().unwrap()));
        let model: Model = serde_json::from_value(case["model"].clone()).unwrap();
        let opts = options(&case["options"]);
        let r = estimate(&data, &model, &opts).unwrap_or_else(|e| panic!("{name}: {e}"));
        let rf = &case["reference"];
        let mut c = Check::new(name);
        if let Some(m) = rf["paths"].as_object() {
            for (k, v) in m {
                let (a, b) = k.split_once("->").unwrap();
                c.near(
                    &format!("path {k}"),
                    r.path(a, b).and_then(|p| p.coefficient),
                    f(v),
                    T,
                );
            }
        }
        for (key, field) in [("loadings", 0), ("weights", 1), ("vifOuter", 2)] {
            if let Some(m) = rf[key].as_object() {
                for (ind, v) in m {
                    let o = r.outer(ind);
                    let mine = o.and_then(|o| match field {
                        0 => o.loading,
                        1 => o.weight,
                        _ => o.vif,
                    });
                    c.near(&format!("{key} {ind}"), mine, f(v), T);
                }
            }
        }
        if let Some(m) = rf["fSquared"].as_object() {
            for (k, v) in m {
                let (a, b) = k.split_once("->").unwrap();
                c.near(&format!("f² {k}"), r.path(a, b).and_then(|p| p.f2), f(v), T);
            }
        }
        if let Some(m) = rf["htmt"].as_object() {
            for (k, v) in m {
                let (a, b) = k.split_once(':').unwrap();
                let mine = r.htmt.get(a, b).or_else(|| r.htmt.get(b, a));
                c.near(&format!("HTMT {k}"), mine, f(v), T);
            }
        }
        if let Some(m) = rf["innerSummary"].as_object() {
            for (con, s) in m {
                let x = r.construct(con);
                c.near(
                    &format!("R² {con}"),
                    x.and_then(|x| x.r2),
                    f(&s["rSquared"]),
                    T,
                );
                c.near(
                    &format!("adj. R² {con}"),
                    x.and_then(|x| x.r2_adj),
                    f(&s["rSquaredAdj"]),
                    T,
                );
                c.near(
                    &format!("alpha {con}"),
                    x.and_then(|x| x.alpha),
                    f(&s["cAlpha"]),
                    T,
                );
                c.near(
                    &format!("rho_A {con}"),
                    x.and_then(|x| x.rho_a),
                    f(&s["rhoA"]),
                    T,
                );
                c.near(
                    &format!("rho_C {con}"),
                    x.and_then(|x| x.rho_c),
                    f(&s["rhoC"]),
                    T,
                );
                c.near(
                    &format!("AVE {con}"),
                    x.and_then(|x| x.ave),
                    f(&s["ave"]),
                    T,
                );
                c.near(
                    &format!("BIC {con}"),
                    x.and_then(|x| x.bic),
                    f(&s["bic"]),
                    T,
                );
            }
        }
        // Not every case printed every fit measure: index the Value (Null
        // when missing). Case 02 is case 09's model (same paths, data and
        // settings), yet its SRMR (0.057) and NFI (0.844) differ from 09's
        // (0.053, 0.845, with d_ULS, d_G and chi² too), which Kantor
        // matches: 02's fit values aren't SmartPLS 4's, so they're skipped.
        let fit_ref = Some(&rf["modelFit"]).filter(|v| v.is_object() && !name.starts_with("02-"));
        if let Some(m) = fit_ref {
            let fit = &r.fit.saturated;
            c.near("SRMR", fit.srmr, f(&m["srmr"]), T);
            c.near("d_ULS", fit.d_uls, f(&m["dULS"]), T);
            c.near("d_G", fit.d_g, f(&m["dG"]), T);
            c.near("NFI", fit.nfi, f(&m["nfi"]), T);
            c.near("chi²", fit.chi2, f(&m["chiSquare"]), T);
        }
        failures.extend(c.done());
    }
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
