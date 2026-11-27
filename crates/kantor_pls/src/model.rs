//! The model: constructs measured by indicators, and the structural paths
//! between constructs. Deserialises from the JSON in docs/pls-json.md.

use serde::{Deserialize, Serialize};

use crate::Missing;

/// A path model: the measurement model (constructs and their indicators)
/// and the structural model (paths between constructs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub constructs: Vec<Construct>,
    #[serde(default, deserialize_with = "paths_de")]
    pub paths: Vec<Path>,
    /// Estimation settings saved with the model (overridden by the caller's
    /// `Options` where it sets them; see `Options::merged`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Options>,
}

/// How a construct relates to its indicators.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measurement {
    /// A common factor measured by its indicators (SmartPLS "reflective"):
    /// Mode A weights, and consistent PLS corrects it for measurement
    /// error.
    #[default]
    Reflective,
    /// A composite of its indicators (SmartPLS "formative"): Mode B
    /// weights unless `weights` says Mode A; never corrected by PLSc.
    #[serde(alias = "formative")]
    Composite,
}

/// How a construct's outer weights are estimated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightMode {
    /// Correlation weights (each indicator's covariance with the inner
    /// proxy).
    #[serde(alias = "A", alias = "a")]
    ModeA,
    /// Regression weights (the inner proxy regressed on the block).
    #[serde(alias = "B", alias = "b")]
    ModeB,
}

/// How a higher-order construct is estimated (reserved: not estimated yet).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HigherOrder {
    /// The lower-order constructs' indicators repeated on the higher-order
    /// construct.
    RepeatedIndicators,
    /// Stage one's lower-order construct scores become the higher-order
    /// construct's indicators.
    TwoStage,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Construct {
    pub name: String,
    /// Data columns measuring the construct.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indicators: Vec<String>,
    #[serde(default, rename = "type")]
    pub measurement: Measurement,
    /// Default: Mode A for reflective, Mode B for composite constructs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights: Option<WeightMode>,
    /// Lower-order constructs of a higher-order construct (a later phase:
    /// `estimate` refuses models with them).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dimensions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub higher_order: Option<HigherOrder>,
}

impl Construct {
    pub fn reflective(name: &str, indicators: &[&str]) -> Self {
        Self {
            name: name.to_owned(),
            indicators: indicators.iter().map(|s| (*s).to_owned()).collect(),
            ..Default::default()
        }
    }

    pub fn composite(name: &str, indicators: &[&str], weights: WeightMode) -> Self {
        Self {
            name: name.to_owned(),
            indicators: indicators.iter().map(|s| (*s).to_owned()).collect(),
            measurement: Measurement::Composite,
            weights: Some(weights),
            ..Default::default()
        }
    }

    pub fn weight_mode(&self) -> WeightMode {
        self.weights.unwrap_or(match self.measurement {
            Measurement::Reflective => WeightMode::ModeA,
            Measurement::Composite => WeightMode::ModeB,
        })
    }
}

/// A structural path `from` → `to`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Path {
    pub from: String,
    pub to: String,
}

impl Path {
    pub fn new(from: &str, to: &str) -> Self {
        Self {
            from: from.to_owned(),
            to: to.to_owned(),
        }
    }
}

/// `["A", "B"]`, or `{"from": "A" | ["A", ...], "to": "B" | ["B", ...]}`
/// (every `from` to every `to`, as seminr's `paths()`).
#[derive(Deserialize)]
#[serde(untagged)]
enum PathSpec {
    Pair(String, String),
    Many { from: OneOrMany, to: OneOrMany },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn list(self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        }
    }
}

fn paths_de<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Path>, D::Error> {
    let specs = Vec::<PathSpec>::deserialize(d)?;
    let mut out = Vec::new();
    for s in specs {
        match s {
            PathSpec::Pair(from, to) => out.push(Path { from, to }),
            PathSpec::Many { from, to } => {
                let to = to.list();
                for f in from.list() {
                    for t in &to {
                        out.push(Path {
                            from: f.clone(),
                            to: t.clone(),
                        });
                    }
                }
            }
        }
    }
    Ok(out)
}

/// PLS-SEM (composites) or consistent PLS (common factors for reflective
/// constructs, Dijkstra & Henseler 2015).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    #[default]
    Pls,
    #[serde(alias = "pls_c", alias = "consistent")]
    Plsc,
}

/// The inner weighting scheme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weighting {
    /// Lohmöller's path weighting scheme (SmartPLS's default).
    #[default]
    #[serde(alias = "path_weighting")]
    Path,
    Centroid,
    Factorial,
}

/// Sign changes of bootstrap estimates (SmartPLS's options).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignChanges {
    /// Estimates kept as they come (SmartPLS 4's default).
    #[default]
    None,
    /// A construct whose loadings come out flipped against the full sample
    /// is turned around before its estimates are taken.
    Construct,
    /// Each estimate takes the full sample's sign.
    Individual,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bootstrap {
    /// Subsamples (5000 as SmartPLS recommends).
    pub samples: usize,
    /// Seed of the random resampling; the same seed gives the same results
    /// on any machine and thread count.
    pub seed: u64,
    /// Significance level of the confidence intervals (0.05: 95% CIs).
    pub alpha: f64,
    /// Bias-corrected and accelerated intervals (a jackknife of n runs).
    pub bca: bool,
    pub sign_changes: SignChanges,
}

impl Default for Bootstrap {
    fn default() -> Self {
        Self {
            samples: 5000,
            seed: 1,
            alpha: 0.05,
            bca: true,
            sign_changes: SignChanges::None,
        }
    }
}

/// Estimation settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Options {
    pub algorithm: Algorithm,
    pub weighting: Weighting,
    pub max_iterations: usize,
    /// Stop when the sum of absolute changes of the outer weights between
    /// two iterations is below this (10⁻⁷ as SmartPLS and seminr).
    pub stop: f64,
    pub missing: Missing,
    /// A number that means "missing" in the data (e.g. -99), besides
    /// empty cells.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub missing_value: Option<f64>,
    /// `None`: no bootstrap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootstrap: Option<Bootstrap>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            algorithm: Algorithm::Pls,
            weighting: Weighting::Path,
            max_iterations: 300,
            stop: 1e-7,
            missing: Missing::Listwise,
            missing_value: None,
            bootstrap: None,
        }
    }
}
