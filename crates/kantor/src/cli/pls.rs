//! `kantor pls`: PLS-SEM (what SmartPLS does) on a workbook or CSV file.
//!
//! ```text
//! kantor pls run --model model.json --data survey.xlsx [--sheet S] [--range A1:K300]
//!                [--bootstrap 5000] [--seed N] [--out report.docx] [--out results.xlsx]
//! kantor pls check --model model.json --data survey.xlsx [--json]
//! kantor pls schema > pls-json.schema.json
//! ```
//!
//! The model JSON and the results are described in docs/pls-json.md.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use kantor_pls::Results;
use serde_json::json;

use crate::agent::pls::{self, Settings, Words};

pub const SCHEMA: &str = include_str!("../../../../docs/pls-json.schema.json");

pub const USAGE: &str = "usage: kantor pls run --model MODEL.json --data DATA [options] [--out FILE]...\n\
    \x20      kantor pls check --model MODEL.json --data DATA [--sheet S] [--range R] [--json]\n\
    \x20      kantor pls schema\n\
    \n\
    PLS-SEM (partial least squares structural equation modeling, as SmartPLS\n\
    does) on survey data in a workbook or CSV file. MODEL.json names the\n\
    constructs, their indicators (the data's column names) and the paths\n\
    between constructs (docs/pls-json.md).\n\
    \n\
    \x20 run      estimate the model: loadings, weights, reliability (alpha,\n\
    \x20          rho_A, rho_C, AVE), HTMT and Fornell-Larcker, VIFs, path\n\
    \x20          coefficients, R², f², indirect and total effects, model fit\n\
    \x20          (SRMR, d_ULS, d_G, NFI); with --bootstrap, standard errors,\n\
    \x20          t and p values and confidence intervals. Prints a summary;\n\
    \x20          --out writes the results: .docx (an APA-style report),\n\
    \x20          .xlsx (every table, the bootstrap and construct scores) or\n\
    \x20          .json (everything). Repeat --out for several.\n\
    \x20 check    check the model against the data first: missing columns,\n\
    \x20          constant indicators, missing values, text in number columns,\n\
    \x20          the 10-times rule, collinear formative indicators. Exit\n\
    \x20          status 1 on errors.\n\
    \x20 schema   print the JSON Schema of model files\n\
    \n\
    Options:\n\
    \x20     --model FILE        the model (JSON)\n\
    \x20     --data FILE         .xlsx, .xlsm, .xls, .xlsb, .ods, .csv or .tsv: column\n\
    \x20                         names in the first row of the range, one row per case\n\
    \x20     --sheet S           the data's sheet (name or number from 1)\n\
    \x20     --range R           the cells to read (A1:K300; default: the used area)\n\
    \x20     --algorithm A       pls (default) or plsc (consistent PLS)\n\
    \x20     --weighting W       path (default), centroid or factorial\n\
    \x20     --bootstrap N       bootstrap subsamples (5000 recommended; 0: none)\n\
    \x20     --seed N            the bootstrap's random seed (default 1)\n\
    \x20     --alpha A           significance level of the intervals (default 0.05)\n\
    \x20     --sign-changes S    none (default), construct or individual\n\
    \x20     --no-bca            skip the bias-corrected (BCa) intervals\n\
    \x20     --missing M         listwise (default) or mean (replacement)\n\
    \x20     --missing-value V   a number meaning missing, such as -99\n\
    \x20 -o, --out FILE          write results (.docx, .xlsx, .json); repeatable\n\
    \x20     --lang L            en (default) or id: the report's language\n\
    \x20     --title T           the report's title\n\
    \x20 -f, --force             overwrite existing output files\n\
    \x20     --json              print the summary (run) or findings (check) as JSON\n\
    \x20 -h, --help              show this help\n\
    \n\
    Settings in the model's \"options\" apply unless an option above is given.\n\
    Exit status: 0 on success, 1 when the work failed, 2 for usage errors.";

#[derive(Default)]
struct Opts {
    model: Option<String>,
    data: Option<String>,
    sheet: Option<String>,
    range: Option<String>,
    settings: Settings,
    out: Vec<PathBuf>,
    lang: Option<String>,
    title: Option<String>,
    force: bool,
    json: bool,
}

fn parse(args: &[OsString]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter();
    let value = |it: &mut std::slice::Iter<OsString>, flag: &str| -> Result<String, String> {
        it.next()
            .map(|v| v.to_string_lossy().into_owned())
            .ok_or_else(|| format!("{flag} needs a value"))
    };
    fn word<T: serde::de::DeserializeOwned>(
        v: &str,
        flag: &str,
        choices: &str,
    ) -> Result<T, String> {
        serde_json::from_value(json!(v)).map_err(|_| format!("{flag} {v}: one of {choices}"))
    }
    while let Some(a) = it.next() {
        let a = a.to_str().ok_or("arguments must be valid UTF-8")?;
        match a {
            "--model" => o.model = Some(value(&mut it, a)?),
            "--data" => o.data = Some(value(&mut it, a)?),
            "--sheet" => o.sheet = Some(value(&mut it, a)?),
            "--range" => o.range = Some(value(&mut it, a)?),
            "--algorithm" => {
                o.settings.algorithm = Some(word(&value(&mut it, a)?, a, "pls, plsc")?);
            }
            "--weighting" => {
                o.settings.weighting =
                    Some(word(&value(&mut it, a)?, a, "path, centroid, factorial")?);
            }
            "--missing" => {
                o.settings.missing = Some(word(&value(&mut it, a)?, a, "listwise, mean")?)
            }
            "--sign-changes" => {
                o.settings.sign_changes =
                    Some(word(&value(&mut it, a)?, a, "none, construct, individual")?);
            }
            "--bootstrap" | "--seed" | "--alpha" | "--missing-value" => {
                let v = value(&mut it, a)?;
                let bad = || format!("{a} {v}: give a number");
                match a {
                    "--bootstrap" => o.settings.bootstrap = Some(v.parse().map_err(|_| bad())?),
                    "--seed" => o.settings.seed = Some(v.parse().map_err(|_| bad())?),
                    "--alpha" => o.settings.alpha = Some(v.parse().map_err(|_| bad())?),
                    _ => o.settings.missing_value = Some(v.parse().map_err(|_| bad())?),
                }
            }
            "--no-bca" => o.settings.bca = Some(false),
            "-o" | "--out" | "--output" => o.out.push(PathBuf::from(value(&mut it, a)?)),
            "--lang" => o.lang = Some(value(&mut it, a)?),
            "--title" => o.title = Some(value(&mut it, a)?),
            "-f" | "--force" => o.force = true,
            "--json" => o.json = true,
            s => return Err(format!("unknown argument {s}")),
        }
    }
    Ok(o)
}

/// Run `kantor pls …`; returns the exit status.
pub fn main(args: &[OsString]) -> i32 {
    let Some(cmd) = args.first().and_then(|a| a.to_str()) else {
        eprintln!("{USAGE}");
        return 2;
    };
    if matches!(cmd, "-h" | "--help" | "help") || args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return 0;
    }
    let opts = match parse(&args[1..]) {
        Ok(o) => o,
        Err(msg) => {
            eprintln!("kantor pls: {msg}\n\n{USAGE}");
            return 2;
        }
    };
    if cmd == "schema" {
        print!("{SCHEMA}");
        return 0;
    }
    if !matches!(cmd, "run" | "check") {
        eprintln!("kantor pls: unknown command {cmd}\n\n{USAGE}");
        return 2;
    }
    let (Some(model), Some(data)) = (&opts.model, &opts.data) else {
        eprintln!("kantor pls {cmd}: give --model MODEL.json and --data FILE\n\n{USAGE}");
        return 2;
    };
    if let Some(l) = &opts.lang
        && !matches!(l.as_str(), "en" | "id" | "en-US" | "id-ID")
    {
        eprintln!("kantor pls: --lang {l}: en or id");
        return 2;
    }
    let result = if cmd == "run" {
        run(&opts, model, data)
    } else {
        check(&opts, model, data)
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("kantor pls {cmd}: {e:#}");
            1
        }
    }
}

fn load(opts: &Opts, model: &str, data: &str) -> Result<(kantor_pls::Model, pls::Loaded)> {
    let model_path = std::path::absolute(model).with_context(|| model.to_owned())?;
    let model = pls::model_from(&json!(model_path), None)?;
    let loaded = pls::read_data(
        Path::new(data),
        opts.sheet.as_deref(),
        opts.range.as_deref(),
    )?;
    Ok((model, loaded))
}

fn check(opts: &Opts, model: &str, data: &str) -> Result<i32> {
    let (model, loaded) = load(opts, model, data)?;
    let options = opts.settings.options(&model)?;
    let report = pls::check_json(&loaded, &model, &options);
    if opts.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{data}: sheet {}, {} ({} rows)",
            loaded.sheet,
            loaded.range,
            loaded.data.rows()
        );
        for f in report["findings"].as_array().into_iter().flatten() {
            println!(
                "{}: {}",
                f["level"].as_str().unwrap_or_default(),
                f["message"].as_str().unwrap_or_default()
            );
        }
        if report["findings"].as_array().is_none_or(Vec::is_empty) {
            println!("no problems found");
        }
    }
    Ok(i32::from(report["errors"].as_u64().unwrap_or(0) > 0))
}

fn run(opts: &Opts, model: &str, data: &str) -> Result<i32> {
    for out in &opts.out {
        if out.exists() && !opts.force {
            bail!("{} exists (use -f to overwrite)", out.display());
        }
        let ext = out
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "docx" | "xlsx" | "json") {
            bail!("--out {}: give a .docx, .xlsx or .json file", out.display());
        }
    }
    let (model, loaded) = load(opts, model, data)?;
    let options = opts.settings.options(&model)?;
    let report = pls::check_json(&loaded, &model, &options);
    for f in report["findings"].as_array().into_iter().flatten() {
        if f["level"] != "info" {
            eprintln!(
                "kantor pls: {}: {}",
                f["level"].as_str().unwrap_or_default(),
                f["message"].as_str().unwrap_or_default()
            );
        }
    }
    if report["errors"].as_u64().unwrap_or(0) > 0 {
        bail!("the model doesn't fit the data (kantor pls check)");
    }
    if let Some(b) = &options.bootstrap {
        eprintln!(
            "kantor pls: bootstrapping {} subsamples on {} threads…",
            b.samples,
            std::thread::available_parallelism().map_or(1, |n| n.get())
        );
    }
    let started = std::time::Instant::now();
    let r =
        kantor_pls::estimate(&loaded.data, &model, &options).map_err(|e| anyhow::anyhow!("{e}"))?;
    let words = Words {
        id: opts.lang.as_deref().is_some_and(|l| l.starts_with("id")),
    };
    for out in &opts.out {
        pls::write_output(&r, out, &words, opts.title.as_deref())?;
        eprintln!("kantor pls: wrote {}", out.display());
    }
    if opts.json {
        let mut v = pls::summary(&r);
        v["data"] = json!({"file": data, "sheet": loaded.sheet, "range": loaded.range});
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        print!("{}", text_summary(&r));
        eprintln!("kantor pls: {:.2} s", started.elapsed().as_secs_f64());
    }
    Ok(0)
}

fn f3(x: Option<f64>) -> String {
    x.map_or_else(|| "–".to_owned(), |v| format!("{v:.3}"))
}

/// A short plain-text summary for the terminal.
fn text_summary(r: &Results) -> String {
    let mut s = String::new();
    let algo = match r.algorithm {
        kantor_pls::Algorithm::Pls => "PLS",
        kantor_pls::Algorithm::Plsc => "PLSc",
    };
    s += &format!(
        "{algo}, n = {}, {} iterations{}\n\n",
        r.n,
        r.iterations,
        if r.converged {
            ""
        } else {
            " (did not converge)"
        }
    );
    s += "Construct        alpha   rho_A   rho_C   AVE     R²\n";
    for c in &r.constructs {
        s += &format!(
            "{:<16} {:<7} {:<7} {:<7} {:<7} {}\n",
            c.name,
            f3(c.alpha),
            f3(c.rho_a),
            f3(c.rho_c),
            f3(c.ave),
            f3(c.r2)
        );
    }
    s += "\nPath                          beta    ";
    let boot = r.bootstrap.as_ref();
    if boot.is_some() {
        s += "t       p       95% CI            ";
    }
    s += "f²\n";
    for p in &r.paths {
        let label = format!("{} -> {}", p.from, p.to);
        s += &format!("{label:<29} {:<7} ", f3(p.coefficient));
        if let Some(b) = boot
            && let Some(e) = b
                .paths
                .iter()
                .find(|e| e.from.as_deref() == Some(&p.from) && e.to.as_deref() == Some(&p.to))
        {
            s += &format!(
                "{:<7} {:<7} [{}, {}]   ",
                f3(e.t),
                f3(e.p),
                f3(e.ci[0]),
                f3(e.ci[1])
            );
        }
        s += &format!("{}\n", f3(p.f2));
    }
    let f = &r.fit.saturated;
    s += &format!(
        "\nSRMR {}  d_ULS {}  d_G {}  NFI {}\n",
        f3(f.srmr),
        f3(f.d_uls),
        f3(f.d_g),
        f3(f.nfi)
    );
    for w in &r.warnings {
        s += &format!("warning: {w}\n");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[std::prelude::v1::test]
    fn schema_is_json_and_options_parse() {
        let v: Value = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(v["type"], "object");
        let args: Vec<OsString> = [
            "--model",
            "m.json",
            "--data",
            "d.csv",
            "--algorithm",
            "plsc",
            "--bootstrap",
            "500",
            "--seed",
            "9",
            "--out",
            "r.docx",
            "-o",
            "r.xlsx",
            "--missing",
            "mean",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        let o = parse(&args).unwrap();
        assert_eq!(o.out.len(), 2);
        assert_eq!(o.settings.bootstrap, Some(500));
        assert!(parse(&[OsString::from("--algorithm"), OsString::from("x")]).is_err());
        let m: kantor_pls::Model = serde_json::from_value(json!({
            "constructs": [{"name": "A", "indicators": ["a"]}, {"name": "B", "indicators": ["b"]}],
            "paths": [["A", "B"]],
        }))
        .unwrap();
        let opts = o.settings.options(&m).unwrap();
        assert_eq!(opts.algorithm, kantor_pls::Algorithm::Plsc);
        assert_eq!(opts.bootstrap.unwrap().seed, 9);
    }
}
