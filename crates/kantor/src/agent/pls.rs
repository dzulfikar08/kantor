//! PLS-SEM on files: data from a workbook or CSV (through the Sheets
//! engine), a model in JSON (docs/pls-json.md), the estimate from
//! `kantor_pls`, and what is made of it: a compact summary for the AI
//! (`pls_run`), an APA-style report (.docx through `docs_author`) and a
//! results workbook (.xlsx through `sheets_author`). Shared by `kantor pls`
//! and the MCP tools.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use kantor_pls::{
    Algorithm, Bootstrap, Data, Estimate, Finding, Level, Measurement, Model, Options, Results,
    SignChanges, Weighting,
};
use serde_json::{Map, Value, json};
use sheets_engine::Workbook;

/// The data read from a file: the table and what was noticed on the way.
pub struct Loaded {
    pub data: Data,
    pub sheet: String,
    /// The cells read, `A1:K251`.
    pub range: String,
    /// Columns with text in them (read as missing values): name → count.
    pub text_cells: Vec<(String, usize)>,
}

fn col_name(mut c: u32) -> String {
    let mut s = Vec::new();
    loop {
        s.push(b'A' + (c % 26) as u8);
        if c < 26 {
            break;
        }
        c = c / 26 - 1;
    }
    s.reverse();
    String::from_utf8(s).unwrap_or_default()
}

/// Read `path` (.xlsx, .xls, .xlsb, .ods, .csv, .tsv): the first row of
/// `range` (default: the sheet's used area) names the columns, the rows
/// below are observations. Empty cells and text are missing values.
pub fn read_data(path: &Path, sheet: Option<&str>, range: Option<&str>) -> Result<Loaded> {
    if !path.is_file() {
        bail!("{} is not a file", path.display());
    }
    let book = Workbook::open(path).with_context(|| format!("can't read {}", path.display()))?;
    let sheet_arg = match sheet {
        None => Value::Null,
        Some(s) => match s.parse::<u64>() {
            Ok(n) => json!(n),
            Err(_) => json!(s),
        },
    };
    let mut index = super::sheets::sheet_index(&book, &json!({"sheet": sheet_arg}), 0)?;
    let (rows, cols) = book.used_size(index);
    let (top, left, bottom, right) = match range {
        None => (0, 0, rows.saturating_sub(1), cols.saturating_sub(1)),
        Some(text) => {
            let (s, r) = super::sheets_author::parse_ref(&book, text, index)?;
            index = s;
            let (rows, cols) = book.used_size(index);
            (
                r.top,
                r.left,
                r.bottom.min(rows.saturating_sub(1)),
                r.right.min(cols.saturating_sub(1)),
            )
        }
    };
    if rows == 0 || bottom <= top {
        bail!("the data needs a header row and at least one row below it");
    }
    let mut names: Vec<String> = Vec::new();
    for c in left..=right {
        let mut name = book.display_value(index, top, c).trim().to_owned();
        if name.is_empty() {
            name = col_name(c);
        }
        if names.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
            bail!("two columns are called {name}: column names must differ");
        }
        names.push(name);
    }
    let mut text = vec![0usize; names.len()];
    let mut out = Vec::new();
    for r in top + 1..=bottom {
        let mut row = Vec::with_capacity(names.len());
        let mut empty = true;
        for (j, c) in (left..=right).enumerate() {
            match book.number_at(index, r, c) {
                Some((n, _)) => {
                    empty = false;
                    row.push(n);
                }
                None => {
                    if !book.display_value(index, r, c).trim().is_empty() {
                        text[j] += 1;
                        empty = false;
                    }
                    row.push(f64::NAN);
                }
            }
        }
        if !empty {
            out.push(row);
        }
    }
    let text_cells = names
        .iter()
        .zip(&text)
        .filter(|(_, n)| **n > 0)
        .map(|(c, n)| (c.clone(), *n))
        .collect();
    Ok(Loaded {
        data: Data::new(names, out).map_err(|e| anyhow::anyhow!("{e}"))?,
        sheet: book.sheet_names()[index as usize].clone(),
        range: format!(
            "{}{}:{}{}",
            col_name(left),
            top + 1,
            col_name(right),
            bottom + 1
        ),
        text_cells,
    })
}

/// `sheet`: a name, or a number from 1.
pub fn sheet_arg(args: &Value) -> Option<String> {
    match args.get("sheet")? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// A path argument: absolute (or `~/…`), or relative to `base` when given.
fn resolve(p: &str, base: Option<&Path>) -> Result<std::path::PathBuf> {
    match base {
        Some(b) if Path::new(p.trim()).is_relative() && !p.trim().starts_with("~/") => {
            Ok(b.join(p.trim()))
        }
        _ => super::path_arg(p),
    }
}

/// A model from JSON: the object itself, or a path to a file holding it.
pub fn model_from(v: &Value, base: Option<&Path>) -> Result<Model> {
    let v = match v {
        Value::String(p) => {
            let path = resolve(p, base)?;
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("can't read the model {}", path.display()))?;
            serde_json::from_str(&text)
                .with_context(|| format!("{} is not valid JSON", path.display()))?
        }
        other => other.clone(),
    };
    serde_json::from_value(v).context("the model doesn't match docs/pls-json.md")
}

/// Settings given on the command line or in tool arguments, over the
/// model's own `options`.
#[derive(Default)]
pub struct Settings {
    pub algorithm: Option<Algorithm>,
    pub weighting: Option<Weighting>,
    pub bootstrap: Option<usize>,
    pub seed: Option<u64>,
    pub alpha: Option<f64>,
    pub missing: Option<kantor_pls::Missing>,
    pub missing_value: Option<f64>,
    pub sign_changes: Option<SignChanges>,
    pub bca: Option<bool>,
}

impl Settings {
    /// From tool arguments (`algorithm`, `weighting`, `bootstrap`, `seed`,
    /// `alpha`, `missing`, `missing_value`, `sign_changes`, `bca`).
    pub fn from_args(args: &Value) -> Result<Self> {
        fn de<T: serde::de::DeserializeOwned>(
            args: &Value,
            key: &str,
            what: &str,
        ) -> Result<Option<T>> {
            args.get(key)
                .filter(|v| !v.is_null())
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()
                .with_context(|| format!("`{key}`: {what}"))
        }
        let num = |key: &str| -> Result<Option<f64>> {
            match args.get(key).filter(|v| !v.is_null()) {
                None => Ok(None),
                Some(v) => v
                    .as_f64()
                    .map(Some)
                    .with_context(|| format!("`{key}` must be a number")),
            }
        };
        Ok(Self {
            algorithm: de(args, "algorithm", "pls or plsc")?,
            weighting: de(args, "weighting", "path, centroid or factorial")?,
            bootstrap: super::opt_count(args, "bootstrap", 0)?.map(|n| n as usize),
            seed: super::opt_count(args, "seed", 0)?,
            alpha: num("alpha")?,
            missing: de(args, "missing", "listwise or mean")?,
            missing_value: num("missing_value")?,
            sign_changes: de(args, "sign_changes", "none, construct or individual")?,
            bca: super::opt_bool(args, "bca")?,
        })
    }

    pub fn options(&self, model: &Model) -> Result<Options> {
        let mut o = model.options.clone().unwrap_or_default();
        if let Some(a) = self.algorithm {
            o.algorithm = a;
        }
        if let Some(w) = self.weighting {
            o.weighting = w;
        }
        if let Some(m) = self.missing {
            o.missing = m;
        }
        if self.missing_value.is_some() {
            o.missing_value = self.missing_value;
        }
        match self.bootstrap {
            Some(0) => o.bootstrap = None,
            Some(n) => o.bootstrap.get_or_insert_with(Bootstrap::default).samples = n,
            None => {}
        }
        if let Some(b) = &mut o.bootstrap {
            if let Some(s) = self.seed {
                b.seed = s;
            }
            if let Some(a) = self.alpha {
                if !(a > 0.0 && a < 1.0) {
                    bail!("`alpha` must be between 0 and 1 (0.05 for 95% intervals)");
                }
                b.alpha = a;
            }
            if let Some(s) = self.sign_changes {
                b.sign_changes = s;
            }
            if let Some(x) = self.bca {
                b.bca = x;
            }
            if b.samples > 100_000 {
                bail!("at most 100000 bootstrap subsamples");
            }
        }
        Ok(o)
    }
}

/// Findings as JSON, with the notes from reading the data.
pub fn check_json(loaded: &Loaded, model: &Model, opts: &Options) -> Value {
    let mut findings: Vec<Value> = kantor_pls::check(&loaded.data, model, opts)
        .iter()
        .map(|f| serde_json::to_value(f).unwrap_or_default())
        .collect();
    let used: Vec<&str> = model
        .constructs
        .iter()
        .flat_map(|c| c.indicators.iter().map(String::as_str))
        .collect();
    for (col, n) in &loaded.text_cells {
        if used.iter().any(|u| u.eq_ignore_ascii_case(col)) {
            findings.push(json!({
                "level": "warning", "check": "text_cells", "indicator": col,
                "message": format!("{col} has {n} cell{} with text, read as missing values", if *n == 1 { "" } else { "s" }),
            }));
        }
    }
    let errors = findings.iter().filter(|f| f["level"] == "error").count();
    let warnings = findings.iter().filter(|f| f["level"] == "warning").count();
    json!({
        "rows": loaded.data.rows(),
        "sheet": loaded.sheet,
        "range": loaded.range,
        "errors": errors,
        "warnings": warnings,
        "findings": findings,
    })
}

/// Findings as text lines.
pub fn check_text(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| {
            let level = match f.level {
                Level::Error => "error",
                Level::Warning => "warning",
                Level::Info => "info",
            };
            format!("{level}: {}\n", f.message)
        })
        .collect()
}

fn r4(x: Option<f64>) -> Value {
    match x {
        Some(v) if v.is_finite() => json!((v * 10_000.0).round() / 10_000.0),
        _ => Value::Null,
    }
}

fn est_json(e: &Estimate) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("se".into(), r4(e.se));
    m.insert("t".into(), r4(e.t));
    m.insert("p".into(), r4(e.p));
    m.insert("ci".into(), json!([r4(e.ci[0]), r4(e.ci[1])]));
    if let Some(b) = e.ci_bca {
        m.insert("ci_bca".into(), json!([r4(b[0]), r4(b[1])]));
    }
    m
}

/// What the AI needs to write the interpretation: rounded estimates,
/// bootstrap inference, and the usual rules of thumb (Hair et al.) already
/// checked, so it doesn't have to know them to flag problems.
pub fn summary(r: &Results) -> Value {
    let boot = r.bootstrap.as_ref();
    let find =
        |list: &[Estimate], f: &dyn Fn(&Estimate) -> bool| list.iter().find(|e| f(e)).cloned();
    let mut flags: Vec<String> = Vec::new();
    let constructs: Vec<Value> = r
        .constructs
        .iter()
        .map(|c| {
            if c.measurement == Measurement::Reflective && c.indicators.len() > 1 {
                if c.ave.is_some_and(|a| a < 0.5) {
                    flags.push(format!("AVE of {} is below 0.50 (convergent validity)", c.name));
                }
                if c.rho_c.is_some_and(|a| a < 0.7) {
                    flags.push(format!("rho_C of {} is below 0.70 (internal consistency)", c.name));
                }
                if c.rho_c.is_some_and(|a| a > 0.95) {
                    flags.push(format!("rho_C of {} is above 0.95 (redundant items)", c.name));
                }
            }
            let mut m = json!({
                "name": c.name,
                "type": if c.measurement == Measurement::Reflective { "reflective" } else { "formative" },
                "indicators": c.indicators.len(),
            });
            if c.alpha.is_some() {
                m["alpha"] = r4(c.alpha);
                m["rho_a"] = r4(c.rho_a);
                m["rho_c"] = r4(c.rho_c);
                m["ave"] = r4(c.ave);
            }
            if c.r2.is_some() {
                m["r2"] = r4(c.r2);
                m["r2_adj"] = r4(c.r2_adj);
                if let Some(e) = boot.and_then(|b| find(&b.r2, &|e| e.construct.as_deref() == Some(&c.name))) {
                    m["r2_p"] = r4(e.p);
                }
            }
            m
        })
        .collect();
    let outer: Vec<Value> = r
        .outer
        .iter()
        .map(|o| {
            let formative = r
                .construct(&o.construct)
                .is_some_and(|c| c.measurement == Measurement::Composite);
            let mut m = json!({"construct": o.construct, "indicator": o.indicator, "loading": r4(o.loading), "vif": r4(o.vif)});
            if formative {
                m["weight"] = r4(o.weight);
                if let Some(e) = boot.and_then(|b| find(&b.weights, &|e| e.indicator.as_deref() == Some(&o.indicator))) {
                    m["weight_p"] = r4(e.p);
                }
                if o.vif.is_some_and(|v| v > 5.0) {
                    flags.push(format!("VIF of formative indicator {} is {:.2} (above 5)", o.indicator, o.vif.unwrap_or_default()));
                }
            } else {
                if let Some(e) = boot.and_then(|b| find(&b.loadings, &|e| e.indicator.as_deref() == Some(&o.indicator))) {
                    m["loading_p"] = r4(e.p);
                }
                if o.loading.is_some_and(|l| l < 0.708)
                    && r.construct(&o.construct).is_some_and(|c| c.indicators.len() > 1)
                {
                    flags.push(format!("loading of {} is {:.3} (below 0.708)", o.indicator, o.loading.unwrap_or_default()));
                }
            }
            m
        })
        .collect();
    let paths: Vec<Value> = r
        .paths
        .iter()
        .map(|p| {
            let mut m = json!({"from": p.from, "to": p.to, "beta": r4(p.coefficient), "f2": r4(p.f2), "vif": r4(p.vif)});
            if p.vif.is_some_and(|v| v > 5.0) {
                flags.push(format!("inner VIF of {} -> {} is above 5 (collinearity)", p.from, p.to));
            }
            if let Some(e) = boot.and_then(|b| find(&b.paths, &|e| e.from.as_deref() == Some(&p.from) && e.to.as_deref() == Some(&p.to))) {
                m.as_object_mut().unwrap().extend(est_json(&e));
            }
            m
        })
        .collect();
    let mut htmt = Vec::new();
    for (i, row) in r.htmt.rows.iter().enumerate() {
        for (j, col) in r.htmt.cols.iter().enumerate() {
            if let Some(v) = r.htmt.values[i][j] {
                let mut m = json!({"a": col, "b": row, "htmt": r4(Some(v))});
                if v > 0.9 {
                    flags.push(format!(
                        "HTMT of {col} and {row} is {v:.3} (above 0.90: discriminant validity)"
                    ));
                }
                if let Some(e) = boot.and_then(|b| {
                    find(&b.htmt, &|e| {
                        (e.from.as_deref() == Some(col) && e.to.as_deref() == Some(row))
                            || (e.from.as_deref() == Some(row) && e.to.as_deref() == Some(col))
                    })
                }) {
                    m["ci"] = json!([r4(e.ci[0]), r4(e.ci[1])]);
                    m["upper_one_sided"] = r4(e.upper_one_sided);
                }
                htmt.push(m);
            }
        }
    }
    let indirect: Vec<Value> = r
        .effects
        .specific_indirect
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut m = json!({"path": e.path, "effect": r4(e.effect)});
            if let Some(b) = boot.and_then(|b| b.specific_indirect.get(i)) {
                m.as_object_mut().unwrap().extend(est_json(b));
            }
            m
        })
        .collect();
    let total: Vec<Value> = r
        .effects
        .total
        .iter()
        .map(|e| {
            let mut m = json!({"from": e.from, "to": e.to, "effect": r4(e.effect)});
            if let Some(b) = boot.and_then(|b| {
                find(&b.total, &|x| {
                    x.from.as_deref() == Some(&e.from) && x.to.as_deref() == Some(&e.to)
                })
            }) {
                m["p"] = r4(b.p);
            }
            m
        })
        .collect();
    // Fornell–Larcker: √AVE below a correlation with another construct.
    for (i, a) in r.fornell_larcker.rows.iter().enumerate() {
        for (j, b) in r.fornell_larcker.cols.iter().enumerate() {
            if i <= j {
                continue;
            }
            let cor = r.fornell_larcker.values[i][j].map(f64::abs);
            for (k, name) in [(i, a), (j, b)] {
                if let (Some(c), Some(d)) = (cor, r.fornell_larcker.values[k][k])
                    && c > d
                {
                    flags.push(format!("Fornell-Larcker: {name}'s √AVE ({d:.3}) is below its correlation with {} ({c:.3})", if name == a { b } else { a }));
                }
            }
        }
    }
    let fit = &r.fit.saturated;
    if fit.srmr.is_some_and(|s| s > 0.08) {
        flags.push(format!(
            "SRMR is {:.3} (above 0.08)",
            fit.srmr.unwrap_or_default()
        ));
    }
    let mut out = json!({
        "n": r.n,
        "rows_in_data": r.rows,
        "dropped_for_missing": r.dropped,
        "replaced_missing": r.replaced,
        "algorithm": r.algorithm,
        "weighting": r.weighting,
        "converged": r.converged,
        "iterations": r.iterations,
        "constructs": constructs,
        "outer": outer,
        "paths": paths,
        "htmt": htmt,
        "specific_indirect": indirect,
        "total_effects": total,
        "fit": {"srmr": r4(fit.srmr), "d_uls": r4(fit.d_uls), "d_g": r4(fit.d_g), "nfi": r4(fit.nfi), "chi2": r4(fit.chi2),
                "srmr_estimated": r4(r.fit.estimated.srmr)},
        "rule_of_thumb_flags": flags,
        "warnings": r.warnings,
    });
    if let Some(b) = boot {
        out["bootstrap"] = json!({"samples": b.samples, "failed": b.failed, "seed": b.seed, "alpha": b.alpha,
            "note": "se, t, p (two-tailed) and percentile ci from the bootstrap; ci_bca bias-corrected and accelerated"});
    } else {
        out["bootstrap"] = json!(null);
        out["note"] = json!(
            "no bootstrap: run with bootstrap (e.g. 5000) for standard errors, t and p values"
        );
    }
    out
}

/// Labels of the report and workbook in English or Indonesian.
pub struct Words {
    pub id: bool,
}

impl Words {
    fn t<'a>(&self, en: &'a str, id: &'a str) -> &'a str {
        if self.id { id } else { en }
    }

    /// A number to three decimals ("0.812" or "0,812").
    fn num(&self, x: Option<f64>) -> String {
        match x {
            Some(v) if v.is_finite() => {
                let s = format!("{v:.3}");
                let s = if s == "-0.000" { "0.000".to_owned() } else { s };
                if self.id { s.replace('.', ",") } else { s }
            }
            _ => "–".to_owned(),
        }
    }

    /// A p value, APA style: ".012", "< .001" (Indonesian "0,012", "< 0,001").
    fn p(&self, x: Option<f64>) -> String {
        match x {
            Some(v) if v.is_finite() => {
                if v < 0.001 {
                    self.t("< .001", "< 0,001").to_owned()
                } else {
                    let s = format!("{v:.3}");
                    if self.id {
                        s.replace('.', ",")
                    } else {
                        s.trim_start_matches('0').to_owned()
                    }
                }
            }
            _ => "–".to_owned(),
        }
    }

    fn ci(&self, ci: [Option<f64>; 2]) -> String {
        let sep = if self.id { "; " } else { ", " };
        format!("[{}{sep}{}]", self.num(ci[0]), self.num(ci[1]))
    }
}

fn paths_label(r: &Results, w: &Words) -> String {
    let algo = match r.algorithm {
        Algorithm::Pls => w.t("PLS-SEM", "PLS-SEM"),
        Algorithm::Plsc => w.t("consistent PLS (PLSc)", "PLS konsisten (PLSc)"),
    };
    let scheme = match r.weighting {
        Weighting::Path => w.t("path weighting scheme", "skema pembobotan jalur (path)"),
        Weighting::Centroid => w.t("centroid weighting scheme", "skema pembobotan sentroid"),
        Weighting::Factorial => w.t("factorial weighting scheme", "skema pembobotan faktorial"),
    };
    format!("{algo}, {scheme}")
}

/// The report as docs JSON (docs/docs-json.md): APA-style tables.
pub fn report_doc(r: &Results, w: &Words, title: Option<&str>) -> Value {
    let mut blocks: Vec<Value> = Vec::new();
    let title = title
        .map(str::to_owned)
        .unwrap_or_else(|| w.t("PLS-SEM results", "Hasil analisis PLS-SEM").to_owned());
    let boot = r.bootstrap.as_ref();
    let mut setup = if w.id {
        format!(
            "Model diestimasi dengan {} pada {} responden",
            paths_label(r, w),
            r.n
        )
    } else {
        format!(
            "The model was estimated with {} on {} cases",
            paths_label(r, w),
            r.n
        )
    };
    if r.dropped > 0 {
        setup += &if w.id {
            format!(" ({} baris dengan data hilang tidak diikutkan)", r.dropped)
        } else {
            format!(" ({} rows with missing values left out)", r.dropped)
        };
    }
    if r.replaced > 0 {
        setup += &if w.id {
            format!(" ({} nilai hilang diganti dengan rata-rata)", r.replaced)
        } else {
            format!(" ({} missing values replaced by the mean)", r.replaced)
        };
    }
    setup.push('.');
    if let Some(b) = boot {
        setup += &if w.id {
            format!(
                " Signifikansi diuji dengan bootstrapping {} subsampel (seed {}), interval kepercayaan persentil {}%.",
                b.samples,
                b.seed,
                ((1.0 - b.alpha) * 100.0).round()
            )
        } else {
            format!(
                " Significance was tested by bootstrapping with {} subsamples (seed {}), with {}% percentile confidence intervals.",
                b.samples,
                b.seed,
                ((1.0 - b.alpha) * 100.0).round()
            )
        };
    }
    blocks.push(json!({"h1": w.t("Estimation", "Estimasi")}));
    blocks.push(json!(setup));

    // Measurement model.
    blocks.push(json!({"h1": w.t("Measurement model", "Model pengukuran (outer model)")}));
    let mut rows: Vec<Value> = Vec::new();
    for c in r
        .constructs
        .iter()
        .filter(|c| c.measurement == Measurement::Reflective)
    {
        for (i, ind) in c.indicators.iter().enumerate() {
            let o = r.outer(ind);
            let first = i == 0;
            let cell = |x: Option<f64>| if first { json!(w.num(x)) } else { json!("") };
            rows.push(json!([
                if first { c.name.clone() } else { String::new() },
                ind,
                w.num(o.and_then(|o| o.loading)),
                cell(c.alpha),
                cell(c.rho_a),
                cell(c.rho_c),
                cell(c.ave),
            ]));
        }
    }
    if !rows.is_empty() {
        blocks.push(json!({
            "type": "table", "id": "reliability",
            "caption": w.t("Indicator loadings, reliability and convergent validity", "Loading indikator, reliabilitas dan validitas konvergen"),
            "header": [w.t("Construct", "Konstruk"), w.t("Indicator", "Indikator"), "Loading", "α", "ρA", "ρC", "AVE"],
            "columns": [3, 2, 1, 1, 1, 1, 1],
            "rows": rows,
        }));
        blocks.push(json!(w.t(
            "*Note.* α = Cronbach's alpha; ρA and ρC = composite reliability (Dijkstra–Henseler's rho_A, Jöreskog's rho_C); AVE = average variance extracted. Loadings of at least 0.708, reliability between 0.70 and 0.95 and AVE of at least 0.50 are commonly recommended (Hair et al., 2022).",
            "*Catatan.* α = Cronbach's alpha; ρA dan ρC = reliabilitas komposit (rho_A Dijkstra–Henseler, rho_C Jöreskog); AVE = average variance extracted. Umumnya dianjurkan loading minimal 0,708, reliabilitas antara 0,70 dan 0,95, serta AVE minimal 0,50 (Hair dkk., 2022)."
        )));
    }
    // Formative constructs.
    let formative: Vec<&kantor_pls::ConstructResult> = r
        .constructs
        .iter()
        .filter(|c| c.measurement == Measurement::Composite)
        .collect();
    if !formative.is_empty() {
        let mut rows = Vec::new();
        for c in formative {
            for (i, ind) in c.indicators.iter().enumerate() {
                let o = r.outer(ind);
                let e = boot.and_then(|b| {
                    b.weights
                        .iter()
                        .find(|e| e.indicator.as_deref() == Some(ind))
                });
                rows.push(json!([
                    if i == 0 {
                        c.name.clone()
                    } else {
                        String::new()
                    },
                    ind,
                    w.num(o.and_then(|o| o.weight)),
                    w.num(e.and_then(|e| e.t)),
                    w.p(e.and_then(|e| e.p)),
                    w.num(o.and_then(|o| o.loading)),
                    w.num(o.and_then(|o| o.vif)),
                ]));
            }
        }
        blocks.push(json!({
            "type": "table", "id": "formative",
            "caption": w.t("Formative indicators", "Indikator formatif"),
            "header": [w.t("Construct", "Konstruk"), w.t("Indicator", "Indikator"), w.t("Weight", "Bobot"), "t", "p", "Loading", "VIF"],
            "columns": [3, 2, 1, 1, 1, 1, 1],
            "rows": rows,
        }));
    }
    // Discriminant validity.
    blocks.push(json!({"h1": w.t("Discriminant validity", "Validitas diskriminan")}));
    let matrix = |t: &kantor_pls::Table| -> (Vec<String>, Vec<Value>) {
        let keep: Vec<usize> = (0..t.rows.len())
            .filter(|&i| {
                t.values
                    .iter()
                    .enumerate()
                    .any(|(k, row)| row[i].is_some() || t.values[i][k].is_some())
            })
            .collect();
        let header: Vec<String> = std::iter::once(String::new())
            .chain(keep.iter().map(|&i| t.cols[i].clone()))
            .collect();
        let rows = keep
            .iter()
            .map(|&i| {
                let mut row = vec![json!(t.rows[i])];
                row.extend(keep.iter().map(|&j| {
                    json!(if t.values[i][j].is_some() {
                        w.num(t.values[i][j])
                    } else {
                        String::new()
                    })
                }));
                Value::Array(row)
            })
            .collect();
        (header, rows)
    };
    let (h, rows) = matrix(&r.htmt);
    if !rows.is_empty() {
        blocks.push(json!({
            "type": "table", "id": "htmt",
            "caption": w.t("Heterotrait–monotrait ratio (HTMT)", "Heterotrait–monotrait ratio (HTMT)"),
            "header": h, "rows": rows,
        }));
        let mut note = w
            .t(
                "*Note.* HTMT values below 0.90 (0.85 for conceptually distinct constructs) indicate discriminant validity (Henseler et al., 2015).",
                "*Catatan.* Nilai HTMT di bawah 0,90 (0,85 untuk konstruk yang berbeda secara konseptual) menunjukkan validitas diskriminan (Henseler dkk., 2015).",
            )
            .to_owned();
        if let Some(b) = boot
            && !b.htmt.is_empty()
        {
            let worst = b
                .htmt
                .iter()
                .filter_map(|e| e.upper_one_sided.map(|u| (u, e)))
                .max_by(|a, b| a.0.total_cmp(&b.0));
            if let Some((u, e)) = worst {
                note += &if w.id {
                    format!(
                        " Batas atas interval kepercayaan bootstrap satu sisi tertinggi adalah {} ({} – {}).",
                        w.num(Some(u)),
                        e.from.as_deref().unwrap_or_default(),
                        e.to.as_deref().unwrap_or_default()
                    )
                } else {
                    format!(
                        " The highest one-sided bootstrap upper bound is {} ({} – {}).",
                        w.num(Some(u)),
                        e.from.as_deref().unwrap_or_default(),
                        e.to.as_deref().unwrap_or_default()
                    )
                };
            }
        }
        blocks.push(json!(note));
    }
    let (h, rows) = matrix(&r.fornell_larcker);
    if !rows.is_empty() {
        blocks.push(json!({
            "type": "table", "id": "fornell_larcker",
            "caption": w.t("Fornell–Larcker criterion", "Kriteria Fornell–Larcker"),
            "header": h, "rows": rows,
        }));
        blocks.push(json!(w.t(
            "*Note.* The diagonal shows the square root of the AVE; it should exceed the construct's correlations below it.",
            "*Catatan.* Diagonal berisi akar kuadrat AVE; nilainya sebaiknya lebih besar daripada korelasi konstruk di bawahnya."
        )));
    }

    // Structural model.
    blocks.push(json!({"h1": w.t("Structural model", "Model struktural (inner model)")}));
    let mut rows = Vec::new();
    for p in &r.paths {
        let e = boot.and_then(|b| {
            b.paths
                .iter()
                .find(|e| e.from.as_deref() == Some(&p.from) && e.to.as_deref() == Some(&p.to))
        });
        let mut row = vec![
            json!(format!("{} → {}", p.from, p.to)),
            json!(w.num(p.coefficient)),
        ];
        if let Some(b) = boot {
            row.push(json!(w.num(e.and_then(|e| e.se))));
            row.push(json!(w.num(e.and_then(|e| e.t))));
            row.push(json!(w.p(e.and_then(|e| e.p))));
            row.push(json!(e.map_or_else(|| "–".to_owned(), |e| w.ci(e.ci))));
            let sig = e.and_then(|e| e.p).is_some_and(|p| p < b.alpha);
            row.push(json!(if sig {
                w.t("Supported", "Diterima")
            } else {
                w.t("Not supported", "Ditolak")
            }));
        }
        row.push(json!(w.num(p.f2)));
        row.push(json!(w.num(p.vif)));
        rows.push(Value::Array(row));
    }
    let mut header = vec![json!(w.t("Path", "Jalur")), json!("β")];
    if boot.is_some() {
        header.extend([
            json!("SE"),
            json!("t"),
            json!("p"),
            json!(w.t("95% CI", "IK 95%")),
            json!(w.t("Decision", "Keputusan")),
        ]);
    }
    header.extend([json!("f²"), json!("VIF")]);
    blocks.push(json!({
        "type": "table", "id": "paths",
        "caption": w.t("Path coefficients", "Koefisien jalur"),
        "header": header, "rows": rows,
    }));
    blocks.push(json!(w.t(
        "*Note.* β = standardized path coefficient; f² of 0.02, 0.15 and 0.35 are small, medium and large effects (Cohen, 1988); inner VIF below 3 shows no collinearity problem.",
        "*Catatan.* β = koefisien jalur terstandar; f² sebesar 0,02, 0,15 dan 0,35 berarti efek kecil, sedang dan besar (Cohen, 1988); VIF inner di bawah 3 menunjukkan tidak ada masalah kolinearitas."
    )));
    let rows: Vec<Value> = r
        .constructs
        .iter()
        .filter(|c| c.r2.is_some())
        .map(|c| json!([c.name, w.num(c.r2), w.num(c.r2_adj)]))
        .collect();
    blocks.push(json!({
        "type": "table", "id": "r2",
        "caption": w.t("Coefficient of determination", "Koefisien determinasi"),
        "header": [w.t("Construct", "Konstruk"), "R²", w.t("Adjusted R²", "R² disesuaikan")],
        "rows": rows,
    }));
    if !r.effects.specific_indirect.is_empty() {
        let mut rows = Vec::new();
        for (i, e) in r.effects.specific_indirect.iter().enumerate() {
            let b = boot.and_then(|b| b.specific_indirect.get(i));
            let mut row = vec![json!(e.path.join(" → ")), json!(w.num(e.effect))];
            if boot.is_some() {
                row.push(json!(w.num(b.and_then(|b| b.t))));
                row.push(json!(w.p(b.and_then(|b| b.p))));
                row.push(json!(b.map_or_else(|| "–".to_owned(), |b| w.ci(b.ci))));
            }
            rows.push(Value::Array(row));
        }
        let mut header = vec![
            json!(w.t("Indirect path", "Jalur tidak langsung")),
            json!(w.t("Effect", "Efek")),
        ];
        if boot.is_some() {
            header.extend([json!("t"), json!("p"), json!(w.t("95% CI", "IK 95%"))]);
        }
        blocks.push(json!({
            "type": "table", "id": "indirect",
            "caption": w.t("Specific indirect effects", "Efek tidak langsung spesifik"),
            "header": header, "rows": rows,
        }));
    }
    let f = &r.fit;
    blocks.push(json!({
        "type": "table", "id": "fit",
        "caption": w.t("Model fit", "Kecocokan model (model fit)"),
        "header": ["", w.t("Saturated model", "Model jenuh"), w.t("Estimated model", "Model estimasi")],
        "rows": [
            ["SRMR", w.num(f.saturated.srmr), w.num(f.estimated.srmr)],
            ["d_ULS", w.num(f.saturated.d_uls), w.num(f.estimated.d_uls)],
            ["d_G", w.num(f.saturated.d_g), w.num(f.estimated.d_g)],
            ["χ²", w.num(f.saturated.chi2), w.num(f.estimated.chi2)],
            ["NFI", w.num(f.saturated.nfi), w.num(f.estimated.nfi)],
        ],
    }));
    blocks.push(json!(w.t(
        "*Note.* SRMR below 0.08 indicates good fit (Hu & Bentler, 1999).",
        "*Catatan.* SRMR di bawah 0,08 menunjukkan kecocokan yang baik (Hu & Bentler, 1999)."
    )));
    if !r.warnings.is_empty() {
        blocks.push(json!({"h2": w.t("Warnings", "Peringatan")}));
        blocks.push(json!({"bullets": r.warnings}));
    }
    json!({
        "preset": "report",
        "lang": if w.id { "id-ID" } else { "en-US" },
        "properties": {"title": title},
        "numbered_headings": false,
        "blocks": std::iter::once(json!({"type": "heading", "level": 1, "text": title, "numbered": false}))
            .chain(blocks)
            .collect::<Vec<_>>(),
    })
}

fn n(x: Option<f64>) -> Value {
    match x {
        Some(v) if v.is_finite() => json!(v),
        _ => Value::Null,
    }
}

/// The results as sheets JSON (docs/sheets-json.md): one sheet per table,
/// numbers as numbers (three decimals shown).
pub fn results_book(r: &Results, w: &Words) -> Value {
    let mut sheets = Vec::new();
    let sheet = |name: &str, header: Vec<Value>, rows: Vec<Vec<Value>>| -> Value {
        let ncols = header.len().max(1) as u32;
        let last = col_name(ncols - 1);
        let last_row = rows.len() + 1;
        let mut all = vec![Value::Array(header)];
        all.extend(rows.into_iter().map(Value::Array));
        json!({
            "name": name,
            "rows": all,
            "formats": [
                {"range": format!("A1:{last}1"), "bold": true, "fill": "#E8EEF4", "borders": "bottom"},
                {"range": format!("B2:{last}{}", last_row.max(2)), "number_format": "0.000"},
            ],
            "freeze": "B2",
            "autofit": true,
        })
    };
    let boot = r.bootstrap.as_ref();
    sheets.push(sheet(
        w.t("Constructs", "Konstruk"),
        [
            "Construct",
            "Type",
            "Cronbach's alpha",
            "rho_A",
            "rho_C",
            "AVE",
            "R²",
            "Adjusted R²",
            "BIC",
        ]
        .map(|s| json!(s))
        .to_vec(),
        r.constructs
            .iter()
            .map(|c| {
                vec![
                    json!(c.name),
                    json!(if c.measurement == Measurement::Reflective {
                        "reflective"
                    } else {
                        "formative"
                    }),
                    n(c.alpha),
                    n(c.rho_a),
                    n(c.rho_c),
                    n(c.ave),
                    n(c.r2),
                    n(c.r2_adj),
                    n(c.bic),
                ]
            })
            .collect(),
    ));
    sheets.push(sheet(
        w.t("Outer model", "Model luar"),
        ["Indicator", "Construct", "Loading", "Weight", "VIF"]
            .map(|s| json!(s))
            .to_vec(),
        r.outer
            .iter()
            .map(|o| {
                vec![
                    json!(o.indicator),
                    json!(o.construct),
                    n(o.loading),
                    n(o.weight),
                    n(o.vif),
                ]
            })
            .collect(),
    ));
    sheets.push(sheet(
        w.t("Paths", "Jalur"),
        ["From", "To", "Coefficient", "f²", "Inner VIF"]
            .map(|s| json!(s))
            .to_vec(),
        r.paths
            .iter()
            .map(|p| {
                vec![
                    json!(p.from),
                    json!(p.to),
                    n(p.coefficient),
                    n(p.f2),
                    n(p.vif),
                ]
            })
            .collect(),
    ));
    let matrix_sheet = |name: &str, t: &kantor_pls::Table| -> Value {
        let mut header = vec![json!("")];
        header.extend(t.cols.iter().map(|c| json!(c)));
        let rows = t
            .rows
            .iter()
            .zip(&t.values)
            .map(|(name, row)| {
                let mut v = vec![json!(name)];
                v.extend(row.iter().map(|x| n(*x)));
                v
            })
            .collect();
        sheet(name, header, rows)
    };
    sheets.push(matrix_sheet("HTMT", &r.htmt));
    sheets.push(matrix_sheet("HTMT2", &r.htmt2));
    sheets.push(matrix_sheet("Fornell-Larcker", &r.fornell_larcker));
    sheets.push(matrix_sheet(
        w.t("Correlations", "Korelasi"),
        &r.correlations,
    ));
    sheets.push(matrix_sheet(
        w.t("Cross loadings", "Cross loading"),
        &r.cross_loadings,
    ));
    let mut effects = Vec::new();
    for e in &r.effects.total {
        effects.push(vec![
            json!("total"),
            json!(format!("{} -> {}", e.from, e.to)),
            n(e.effect),
        ]);
    }
    for e in &r.effects.total_indirect {
        effects.push(vec![
            json!("total indirect"),
            json!(format!("{} -> {}", e.from, e.to)),
            n(e.effect),
        ]);
    }
    for e in &r.effects.specific_indirect {
        effects.push(vec![
            json!("specific indirect"),
            json!(e.path.join(" -> ")),
            n(e.effect),
        ]);
    }
    sheets.push(sheet(
        w.t("Effects", "Efek"),
        ["Effect", "Path", "Value"].map(|s| json!(s)).to_vec(),
        effects,
    ));
    let f = &r.fit;
    sheets.push(sheet(
        "Model fit",
        ["Measure", "Saturated model", "Estimated model"]
            .map(|s| json!(s))
            .to_vec(),
        vec![
            vec![json!("SRMR"), n(f.saturated.srmr), n(f.estimated.srmr)],
            vec![json!("d_ULS"), n(f.saturated.d_uls), n(f.estimated.d_uls)],
            vec![json!("d_G"), n(f.saturated.d_g), n(f.estimated.d_g)],
            vec![
                json!("Chi-square"),
                n(f.saturated.chi2),
                n(f.estimated.chi2),
            ],
            vec![json!("NFI"), n(f.saturated.nfi), n(f.estimated.nfi)],
        ],
    ));
    if let Some(b) = boot {
        let mut rows = Vec::new();
        let groups: [(&str, &Vec<Estimate>); 9] = [
            ("path", &b.paths),
            ("loading", &b.loadings),
            ("weight", &b.weights),
            ("R²", &b.r2),
            ("f²", &b.f2),
            ("total indirect", &b.total_indirect),
            ("specific indirect", &b.specific_indirect),
            ("total effect", &b.total),
            ("HTMT", &b.htmt),
        ];
        for (kind, list) in groups {
            for e in list {
                let label = if let Some(p) = &e.path {
                    p.join(" -> ")
                } else if let (Some(a), Some(b)) = (&e.from, &e.to) {
                    format!("{a} -> {b}")
                } else if let Some(i) = &e.indicator {
                    format!("{i} ({})", e.construct.as_deref().unwrap_or_default())
                } else {
                    e.construct.clone().unwrap_or_default()
                };
                let bca = e.ci_bca.unwrap_or([None, None]);
                rows.push(vec![
                    json!(kind),
                    json!(label),
                    n(e.original),
                    n(e.mean),
                    n(e.se),
                    n(e.t),
                    n(e.p),
                    n(e.ci[0]),
                    n(e.ci[1]),
                    n(bca[0]),
                    n(bca[1]),
                ]);
            }
        }
        sheets.push(sheet(
            "Bootstrap",
            [
                "Statistic",
                "Of",
                "Original",
                "Mean",
                "SE",
                "t",
                "p",
                "CI lower",
                "CI upper",
                "BCa lower",
                "BCa upper",
            ]
            .map(|s| json!(s))
            .to_vec(),
            rows,
        ));
    }
    let mut header = vec![json!("Row")];
    header.extend(r.scores.cols.iter().map(|c| json!(c)));
    sheets.push(sheet(
        w.t("Scores", "Skor"),
        header,
        r.scores
            .rows
            .iter()
            .zip(&r.scores.values)
            .map(|(row, vals)| {
                let mut v = vec![json!(row.parse::<u64>().unwrap_or(0))];
                v.extend(vals.iter().map(|x| n(*x)));
                v
            })
            .collect(),
    ));
    json!({"sheets": sheets, "active": w.t("Paths", "Jalur")})
}

/// Run `pls_run`'s arguments: `{model, data, sheet, range, ...settings,
/// out}`. Returns the summary (and the files written).
pub fn run_tool(args: &Value, base: Option<&Path>) -> Result<Value> {
    let data_path = resolve(super::req_str(args, "data")?, base)?;
    let model = model_from(args.get("model").context("`model` is required")?, base)?;
    let settings = Settings::from_args(args)?;
    let opts = settings.options(&model)?;
    let sheet = sheet_arg(args);
    let loaded = read_data(&data_path, sheet.as_deref(), super::opt_str(args, "range")?)?;
    let checked = check_json(&loaded, &model, &opts);
    if checked["errors"].as_u64().unwrap_or(0) > 0 {
        bail!(
            "the model doesn't fit the data: {}",
            checked["findings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|f| f["level"] == "error")
                .filter_map(|f| f["message"].as_str())
                .collect::<Vec<_>>()
                .join("; ")
        );
    }
    let r =
        kantor_pls::estimate(&loaded.data, &model, &opts).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut out = summary(&r);
    out["data"] = json!({"file": data_path.display().to_string(), "sheet": loaded.sheet, "range": loaded.range});
    let lang_id = matches!(super::opt_str(args, "lang")?, Some("id" | "id-ID"));
    let words = Words { id: lang_id };
    let overwrite = super::opt_bool(args, "overwrite")?.unwrap_or(false);
    let mut written = Vec::new();
    for key in ["report", "workbook", "results_json"] {
        let Some(p) = super::opt_str(args, key)? else {
            continue;
        };
        let path = resolve(p, base)?;
        if path.exists() && !overwrite {
            bail!(
                "{} exists; pass overwrite=true to replace it",
                path.display()
            );
        }
        write_output(&r, &path, &words, None)?;
        written.push(path.display().to_string());
    }
    if !written.is_empty() {
        out["files"] = json!(written);
    }
    let notes: Vec<&Value> = checked["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["level"] == "warning")
        .collect();
    if !notes.is_empty() {
        out["data_warnings"] = json!(
            notes
                .iter()
                .filter_map(|f| f["message"].as_str())
                .collect::<Vec<_>>()
        );
    }
    Ok(out)
}

/// Write the results as .json (everything), .docx (the report) or .xlsx
/// (the results workbook), by the file's extension.
pub fn write_output(r: &Results, path: &Path, words: &Words, title: Option<&str>) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let bytes = match ext.as_str() {
        "json" => {
            let mut v = serde_json::to_value(r)?;
            v["scores"] = serde_json::to_value(&r.scores)?;
            serde_json::to_vec_pretty(&v)?
        }
        "docx" => {
            let doc = report_doc(r, words, title);
            super::docs_author::build(&doc, None)?.bytes
        }
        "xlsx" => {
            let book = super::sheets_author::build(&results_book(r, words), None)?;
            super::sheets_author::to_bytes(&book)?
        }
        _ => bail!(
            "{}: the output must be .json, .docx or .xlsx",
            path.display()
        ),
    };
    crate::cli::slides::write_file(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../kantor_pls/tests")
    }

    #[std::prelude::v1::test]
    fn runs_on_a_csv_and_writes_reports() {
        let dir = tempfile::tempdir().unwrap();
        let t = fixture();
        let args = json!({
            "data": t.join("data/mediation.csv"),
            "model": t.join("models/mediation.json"),
            "bootstrap": 200, "seed": 3, "missing": "mean",
            "report": dir.path().join("report.docx"),
            "workbook": dir.path().join("results.xlsx"),
            "results_json": dir.path().join("results.json"),
            "lang": "id",
        });
        let out = run_tool(&args, None).unwrap();
        assert_eq!(out["n"], 150);
        assert_eq!(out["paths"].as_array().unwrap().len(), 3);
        assert!(out["paths"][0]["p"].is_number(), "{out}");
        assert_eq!(out["specific_indirect"][0]["path"], json!(["X", "M", "Y"]));
        assert_eq!(out["files"].as_array().unwrap().len(), 3);
        let book = Workbook::open(dir.path().join("results.xlsx")).unwrap();
        assert!(book.sheet_names().iter().any(|s| s == "Bootstrap"));
        assert!(
            std::fs::metadata(dir.path().join("report.docx"))
                .unwrap()
                .len()
                > 2000
        );
        // Existing files are kept.
        assert!(
            run_tool(&args, None)
                .unwrap_err()
                .to_string()
                .contains("overwrite")
        );
        // An xlsx data file reads the same.
        let loaded = read_data(&t.join("data/ecsi_synth.csv"), None, Some("A1:H251")).unwrap();
        assert_eq!(loaded.data.cols(), 8);
        assert_eq!(loaded.data.rows(), 250);
        assert_eq!(loaded.range, "A1:H251");
    }

    #[std::prelude::v1::test]
    fn english_report_formats_p_values() {
        let w = Words { id: false };
        assert_eq!(w.p(Some(0.0123)), ".012");
        assert_eq!(w.p(Some(0.0001)), "< .001");
        assert_eq!(w.num(Some(-0.00001)), "0.000");
        let w = Words { id: true };
        assert_eq!(w.p(Some(0.0123)), "0,012");
        assert_eq!(w.ci([Some(0.1), Some(0.25)]), "[0,100; 0,250]");
    }
}
