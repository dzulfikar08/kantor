//! A model resolved against the data: constructs and indicators as
//! numbers, the structural model as predecessor lists in causal order.

use std::collections::HashMap;

use crate::model::{Measurement, Model, WeightMode};
use crate::{Code, Data, Error};

/// Why a model can't be resolved: its code and arguments (see [`Code`])
/// and the English message.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Problem {
    pub code: Code,
    pub message: String,
    /// A later phase's feature (higher-order constructs).
    pub unsupported: bool,
}

impl Problem {
    fn new(code: &'static str, args: &[&str], message: String) -> Self {
        Self {
            code: Code::new(code, args),
            message,
            unsupported: false,
        }
    }
}

impl From<Problem> for Error {
    fn from(p: Problem) -> Self {
        if p.unsupported {
            Error::Unsupported(p.message)
        } else {
            Error::Model(p.message)
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Spec {
    pub names: Vec<String>,
    /// Each construct's indicators (positions in `ind_*`).
    pub blocks: Vec<Vec<usize>>,
    pub ind_names: Vec<String>,
    /// The data column of each indicator.
    pub ind_cols: Vec<usize>,
    pub ind_block: Vec<usize>,
    pub measurement: Vec<Measurement>,
    pub mode_b: Vec<bool>,
    pub preds: Vec<Vec<usize>>,
    /// Paths in the model's order.
    pub paths: Vec<(usize, usize)>,
    /// Constructs with no predecessors first, then the rest in causal order.
    pub order: Vec<usize>,
    /// Chains of three or more constructs along paths (specific indirect
    /// effects).
    pub chains: Vec<Vec<usize>>,
}

impl Spec {
    pub fn k(&self) -> usize {
        self.names.len()
    }

    pub fn p(&self) -> usize {
        self.ind_names.len()
    }

    /// Resolve `model` against `data`'s columns.
    pub fn new(model: &Model, data: &Data) -> Result<Spec, Error> {
        Self::resolve(model, data).map_err(Error::from)
    }

    /// [`Spec::new`], with the problem's code.
    pub fn resolve(model: &Model, data: &Data) -> Result<Spec, Problem> {
        let mut names = Vec::new();
        let mut index: HashMap<&str, usize> = HashMap::new();
        if model.constructs.is_empty() {
            return Err(Problem::new(
                "no_constructs",
                &[],
                "the model has no constructs".into(),
            ));
        }
        for c in &model.constructs {
            if c.name.trim().is_empty() {
                return Err(Problem::new(
                    "unnamed_construct",
                    &[],
                    "a construct has no name".into(),
                ));
            }
            if index.insert(c.name.as_str(), names.len()).is_some() {
                return Err(Problem::new(
                    "duplicate_construct",
                    &[&c.name],
                    format!("construct {} is defined twice", c.name),
                ));
            }
            if !c.dimensions.is_empty() || c.higher_order.is_some() {
                return Err(Problem {
                    unsupported: true,
                    ..Problem::new(
                        "higher_order",
                        &[&c.name],
                        format!(
                            "{} is a higher-order construct; these aren't estimated yet",
                            c.name
                        ),
                    )
                });
            }
            if c.indicators.is_empty() {
                return Err(Problem::new(
                    "no_indicators",
                    &[&c.name],
                    format!("construct {} has no indicators", c.name),
                ));
            }
            names.push(c.name.clone());
        }
        let mut blocks = Vec::new();
        let mut ind_names = Vec::new();
        let mut ind_cols = Vec::new();
        let mut ind_block = Vec::new();
        for (j, c) in model.constructs.iter().enumerate() {
            let mut b = Vec::new();
            for ind in &c.indicators {
                let col = data.column(ind).ok_or_else(|| {
                    Problem::new(
                        "missing_column",
                        &[ind, &c.name],
                        format!("indicator {ind} of {} is not a column of the data", c.name),
                    )
                })?;
                if b.iter().any(|&i: &usize| ind_cols[i] == col) {
                    return Err(Problem::new(
                        "duplicate_indicator",
                        &[ind, &c.name],
                        format!("indicator {ind} is listed twice in {}", c.name),
                    ));
                }
                b.push(ind_names.len());
                ind_names.push(ind.clone());
                ind_cols.push(col);
                ind_block.push(j);
            }
            blocks.push(b);
        }
        let k = names.len();
        let mut preds = vec![Vec::new(); k];
        let mut succs = vec![Vec::new(); k];
        let mut paths = Vec::new();
        for p in &model.paths {
            let lookup = |n: &str| {
                index.get(n).copied().ok_or_else(|| {
                    Problem::new(
                        "unknown_construct",
                        &[&p.from, &p.to, n],
                        format!("path {} -> {}: no construct {n}", p.from, p.to),
                    )
                })
            };
            let (f, t) = (lookup(&p.from)?, lookup(&p.to)?);
            if f == t {
                return Err(Problem::new(
                    "self_path",
                    &[&p.from],
                    format!("path {} -> {} is a loop", p.from, p.to),
                ));
            }
            if paths.contains(&(f, t)) {
                continue;
            }
            if paths.contains(&(t, f)) {
                return Err(Problem::new(
                    "path_and_back",
                    &[&p.from, &p.to],
                    format!(
                        "paths {} -> {} and back make the model non-recursive",
                        p.from, p.to
                    ),
                ));
            }
            paths.push((f, t));
            preds[t].push(f);
            succs[f].push(t);
        }
        for j in 0..k {
            if preds[j].is_empty() && succs[j].is_empty() {
                return Err(Problem::new(
                    "no_paths",
                    &[&names[j]],
                    format!("construct {} has no structural paths", names[j]),
                ));
            }
        }
        // Causal order (Kahn): exogenous constructs first.
        let mut indeg: Vec<usize> = preds.iter().map(Vec::len).collect();
        let mut order: Vec<usize> = (0..k).filter(|&j| indeg[j] == 0).collect();
        let mut i = 0;
        while i < order.len() {
            let j = order[i];
            for &s in &succs[j] {
                indeg[s] -= 1;
                if indeg[s] == 0 {
                    order.push(s);
                }
            }
            i += 1;
        }
        if order.len() < k {
            let cyc: Vec<&str> = (0..k)
                .filter(|j| !order.contains(j))
                .map(|j| names[j].as_str())
                .collect();
            let list = cyc.join(", ");
            return Err(Problem::new(
                "cycle",
                &[&list],
                format!(
                    "the structural model has a cycle through {list} (PLS-SEM needs a recursive model)"
                ),
            ));
        }
        let mut chains = Vec::new();
        fn walk(j: usize, succs: &[Vec<usize>], cur: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
            for &s in &succs[j] {
                cur.push(s);
                if cur.len() >= 3 {
                    out.push(cur.clone());
                }
                walk(s, succs, cur, out);
                cur.pop();
            }
        }
        for &j in &order {
            let mut cur = vec![j];
            walk(j, &succs, &mut cur, &mut chains);
        }
        let measurement: Vec<Measurement> =
            model.constructs.iter().map(|c| c.measurement).collect();
        let mode_b = model
            .constructs
            .iter()
            .map(|c| c.weight_mode() == WeightMode::ModeB && c.indicators.len() > 1)
            .collect();
        Ok(Spec {
            names,
            blocks,
            ind_names,
            ind_cols,
            ind_block,
            measurement,
            mode_b,
            preds,
            paths,
            order,
            chains,
        })
    }

    /// Whether `from` reaches `to` along paths of two or more steps.
    pub fn has_indirect(&self, from: usize, to: usize) -> bool {
        self.chains
            .iter()
            .any(|c| c[0] == from && c[c.len() - 1] == to)
    }

    /// Whether `from` reaches `to` along paths.
    pub fn reaches(&self, from: usize, to: usize) -> bool {
        self.paths.contains(&(from, to)) || self.has_indirect(from, to)
    }
}
