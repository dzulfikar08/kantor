//! The model against the data before estimating: what would stop the
//! estimation (errors) and what weakens it (warnings).

use serde::Serialize;

use crate::model::{Model, Options};
use crate::spec::Spec;
use crate::{Data, Missing, data};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Finding {
    pub level: Level,
    /// A short name of the check (`missing_column`, `ten_times_rule`, …).
    pub check: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub construct: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub indicator: Option<String>,
    /// The message's arguments in order, for front ends that word `check`
    /// in their own language (numbers in English notation; not in the
    /// JSON).
    #[serde(skip)]
    pub args: Vec<String>,
}

fn finding(level: Level, check: &'static str, message: String) -> Finding {
    Finding {
        level,
        check,
        message,
        construct: None,
        indicator: None,
        args: Vec::new(),
    }
}

impl Finding {
    fn with_args(mut self, args: &[&str]) -> Self {
        self.args = args.iter().map(|a| (*a).to_owned()).collect();
        self
    }
}

/// Check `model` against `data`.
pub fn check(data: &Data, model: &Model, opts: &Options) -> Vec<Finding> {
    let mut out = Vec::new();
    // Every indicator that isn't a column, not just the first.
    for c in &model.constructs {
        for ind in &c.indicators {
            if data.column(ind).is_none() {
                let mut f = finding(
                    Level::Error,
                    "missing_column",
                    format!("indicator {ind} of {} is not a column of the data", c.name),
                )
                .with_args(&[ind, &c.name]);
                f.construct = Some(c.name.clone());
                f.indicator = Some(ind.clone());
                out.push(f);
            }
        }
    }
    if !out.is_empty() {
        return out;
    }
    let data = match opts.missing_value {
        Some(code) => data.clone().with_missing_code(code),
        None => data.clone(),
    };
    let spec = match Spec::resolve(model, &data) {
        Ok(s) => s,
        Err(p) => {
            let mut f = finding(Level::Error, p.code.code, p.message);
            f.args = p.code.args;
            out.push(f);
            return out;
        }
    };
    // Indicators in more than one construct.
    for (i, &col) in spec.ind_cols.iter().enumerate() {
        if let Some(first) = spec.ind_cols.iter().position(|&c| c == col)
            && first < i
        {
            let mut f = finding(
                Level::Warning,
                "shared_indicator",
                format!(
                    "indicator {} measures both {} and {} (only higher-order models repeat indicators)",
                    spec.ind_names[i],
                    spec.names[spec.ind_block[first]],
                    spec.names[spec.ind_block[i]]
                ),
            )
            .with_args(&[
                &spec.ind_names[i],
                &spec.names[spec.ind_block[first]],
                &spec.names[spec.ind_block[i]],
            ]);
            f.indicator = Some(spec.ind_names[i].clone());
            out.push(f);
        }
    }
    // Missing values.
    let missing = data.missing_counts(&spec.ind_cols);
    let total: usize = missing.iter().sum();
    let (x, kept) = data.prepare(&spec.ind_cols, opts.missing);
    if total > 0 {
        for (i, &m) in missing.iter().enumerate() {
            if m > 0 {
                let share = m as f64 / data.rows().max(1) as f64;
                let mut f = finding(
                    if share > 0.15 {
                        Level::Warning
                    } else {
                        Level::Info
                    },
                    "missing_values",
                    format!(
                        "{} has {m} missing value{} ({:.1}% of rows)",
                        spec.ind_names[i],
                        if m == 1 { "" } else { "s" },
                        share * 100.0
                    ),
                )
                .with_args(&[
                    &spec.ind_names[i],
                    &m.to_string(),
                    &format!("{:.1}", share * 100.0),
                ]);
                f.indicator = Some(spec.ind_names[i].clone());
                out.push(f);
            }
        }
        if opts.missing == Missing::Listwise {
            let dropped = data.rows() - kept.len();
            out.push(
                finding(
                    if dropped * 10 > data.rows() {
                        Level::Warning
                    } else {
                        Level::Info
                    },
                    "listwise_deletion",
                    format!(
                        "listwise deletion leaves out {dropped} of {} rows",
                        data.rows()
                    ),
                )
                .with_args(&[&dropped.to_string(), &data.rows().to_string()]),
            );
        }
    }
    let n = kept.len();
    let p = spec.p();
    if n < 3 {
        out.push(
            finding(
                Level::Error,
                "too_few_rows",
                format!("{n} usable rows: too few to estimate the model"),
            )
            .with_args(&[&n.to_string()]),
        );
        return out;
    }
    // Constant indicators.
    let (_, sd) = data::mean_sd(&x, p);
    for (i, s) in sd.iter().enumerate() {
        if s.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) {
            let mut f = finding(
                Level::Error,
                "constant_indicator",
                format!("{} has the same value in every row", spec.ind_names[i]),
            )
            .with_args(&[&spec.ind_names[i]]);
            f.indicator = Some(spec.ind_names[i].clone());
            out.push(f);
        }
    }
    // The 10-times rule (Barclay, Higgins & Thompson 1995): ten times the
    // most arrows pointing at one construct.
    let mut most = 0;
    let mut at = 0;
    for j in 0..spec.k() {
        let arrows = spec.preds[j].len().max(if spec.mode_b[j] {
            spec.blocks[j].len()
        } else {
            0
        });
        if arrows > most {
            most = arrows;
            at = j;
        }
    }
    if n < 10 * most {
        let mut f = finding(
            Level::Warning,
            "ten_times_rule",
            format!(
                "{n} rows is fewer than ten times the {most} arrows pointing at {} ({} needed by the 10-times rule; a power analysis is the better guide)",
                spec.names[at],
                10 * most
            ),
        )
        .with_args(&[
            &n.to_string(),
            &most.to_string(),
            &spec.names[at],
            &(10 * most).to_string(),
        ]);
        f.construct = Some(spec.names[at].clone());
        out.push(f);
    }
    // Formative blocks: collinear indicators.
    if let Some(s) = data::correlation_two_pass(&x, p) {
        for (j, b) in spec.blocks.iter().enumerate() {
            if !spec.mode_b[j] {
                continue;
            }
            match s.select(b, b).inverse() {
                None => {
                    let mut f = finding(
                        Level::Error,
                        "collinear_block",
                        format!(
                            "the indicators of {} are perfectly collinear (Mode B can't weight them)",
                            spec.names[j]
                        ),
                    )
                    .with_args(&[&spec.names[j]]);
                    f.construct = Some(spec.names[j].clone());
                    out.push(f);
                }
                Some(inv) => {
                    for (r, &i) in b.iter().enumerate() {
                        if inv[(r, r)] > 5.0 {
                            let mut f = finding(
                                Level::Warning,
                                "outer_vif",
                                format!(
                                    "{} of formative {} has a VIF of {:.2} (above 5: collinear)",
                                    spec.ind_names[i],
                                    spec.names[j],
                                    inv[(r, r)]
                                ),
                            )
                            .with_args(&[
                                &spec.ind_names[i],
                                &spec.names[j],
                                &format!("{:.2}", inv[(r, r)]),
                            ]);
                            f.construct = Some(spec.names[j].clone());
                            f.indicator = Some(spec.ind_names[i].clone());
                            out.push(f);
                        }
                    }
                }
            }
        }
    }
    out
}
