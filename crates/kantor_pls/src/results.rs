//! What `estimate` returns (serialises to the results JSON of
//! docs/pls-json.md).

use serde::Serialize;

use crate::bootstrap::BootstrapResults;
use crate::fit::Fit;
use crate::model::{Algorithm, Measurement, WeightMode, Weighting};

/// A labelled matrix (`values[row][col]`, null where undefined).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Table {
    pub rows: Vec<String>,
    pub cols: Vec<String>,
    pub values: Vec<Vec<Option<f64>>>,
}

impl Table {
    pub fn get(&self, row: &str, col: &str) -> Option<f64> {
        let r = self.rows.iter().position(|x| x == row)?;
        let c = self.cols.iter().position(|x| x == col)?;
        self.values[r][c]
    }
}

/// Quality criteria of one construct.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ConstructResult {
    pub name: String,
    #[serde(rename = "type")]
    pub measurement: Measurement,
    pub weights: WeightMode,
    pub indicators: Vec<String>,
    /// Cronbach's alpha (standardized indicators). Reliability and AVE
    /// are for reflective constructs only.
    pub alpha: Option<f64>,
    /// Dijkstra–Henseler's rho_A.
    pub rho_a: Option<f64>,
    /// Composite reliability rho_C.
    pub rho_c: Option<f64>,
    /// Average variance extracted.
    pub ave: Option<f64>,
    /// None for exogenous constructs.
    pub r2: Option<f64>,
    pub r2_adj: Option<f64>,
    /// Bayesian information criterion of the construct's regression on
    /// its predecessors (SmartPLS's BIC; lower is better).
    pub bic: Option<f64>,
}

/// One indicator's outer estimates.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OuterResult {
    pub construct: String,
    pub indicator: String,
    pub loading: Option<f64>,
    pub weight: Option<f64>,
    /// Variance inflation factor within its block.
    pub vif: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PathResult {
    pub from: String,
    pub to: String,
    pub coefficient: Option<f64>,
    pub f2: Option<f64>,
    /// Inner VIF of `from` among `to`'s predictors.
    pub vif: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Effect {
    pub from: String,
    pub to: String,
    pub effect: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SpecificEffect {
    /// The constructs along the chain, first to last.
    pub path: Vec<String>,
    pub effect: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Effects {
    pub total: Vec<Effect>,
    pub total_indirect: Vec<Effect>,
    pub specific_indirect: Vec<SpecificEffect>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Results {
    pub algorithm: Algorithm,
    pub weighting: Weighting,
    /// Rows estimated (after missing values were handled).
    pub n: usize,
    /// Rows in the data.
    pub rows: usize,
    /// Rows left out for missing values (listwise deletion).
    pub dropped: usize,
    /// Missing values replaced by the mean.
    pub replaced: usize,
    pub iterations: usize,
    pub converged: bool,
    pub constructs: Vec<ConstructResult>,
    pub outer: Vec<OuterResult>,
    pub paths: Vec<PathResult>,
    /// Construct correlations (corrected for attenuation in PLSc).
    pub correlations: Table,
    /// √AVE on the diagonal, construct correlations below it.
    pub fornell_larcker: Table,
    pub htmt: Table,
    pub htmt2: Table,
    /// Indicators × constructs.
    pub cross_loadings: Table,
    pub effects: Effects,
    pub fit: Fit,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootstrap: Option<BootstrapResults>,
    pub warnings: Vec<String>,
    /// Construct scores (standardized), one row per estimated row: not in
    /// the JSON.
    #[serde(skip)]
    pub scores: Table,
}

impl Results {
    pub fn construct(&self, name: &str) -> Option<&ConstructResult> {
        self.constructs.iter().find(|c| c.name == name)
    }

    pub fn path(&self, from: &str, to: &str) -> Option<&PathResult> {
        self.paths.iter().find(|p| p.from == from && p.to == to)
    }

    pub fn outer(&self, indicator: &str) -> Option<&OuterResult> {
        self.outer.iter().find(|o| o.indicator == indicator)
    }
}
