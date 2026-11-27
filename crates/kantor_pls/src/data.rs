//! The data: an n × p table of numbers with column names, and missing
//! values (NaN, or a code such as -99) handled by listwise deletion or mean
//! replacement.

use serde::{Deserialize, Serialize};

use crate::Error;

/// A table of observations (rows) by variables (columns). Missing values
/// are `NaN`.
#[derive(Clone, Debug, PartialEq)]
pub struct Data {
    names: Vec<String>,
    rows: usize,
    /// Row-major values.
    values: Vec<f64>,
}

/// How rows with missing values are treated (SmartPLS's options).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Missing {
    /// Rows with a missing value in any indicator the model uses are left
    /// out (casewise / listwise deletion).
    #[default]
    Listwise,
    /// Missing values are replaced by the indicator's mean over the rows
    /// that have it.
    Mean,
}

impl Data {
    /// Columns `names` and `rows` of values (each as long as `names`).
    pub fn new(names: Vec<String>, rows: Vec<Vec<f64>>) -> Result<Self, Error> {
        let p = names.len();
        let mut values = Vec::with_capacity(rows.len() * p);
        for (i, r) in rows.iter().enumerate() {
            if r.len() != p {
                return Err(Error::Data(format!(
                    "row {} has {} values for {p} columns",
                    i + 1,
                    r.len()
                )));
            }
            values.extend_from_slice(r);
        }
        Ok(Self {
            names,
            rows: rows.len(),
            values,
        })
    }

    /// Columns given one by one (each as long as the first).
    pub fn from_columns(columns: Vec<(String, Vec<f64>)>) -> Result<Self, Error> {
        let n = columns.first().map_or(0, |c| c.1.len());
        if let Some((name, c)) = columns.iter().find(|c| c.1.len() != n) {
            return Err(Error::Data(format!(
                "column {name} has {} values, the first has {n}",
                c.len()
            )));
        }
        let names = columns.iter().map(|c| c.0.clone()).collect();
        let rows = (0..n)
            .map(|r| columns.iter().map(|c| c.1[r]).collect())
            .collect();
        Self::new(names, rows)
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn cols(&self) -> usize {
        self.names.len()
    }

    pub fn get(&self, row: usize, col: usize) -> f64 {
        self.values[row * self.names.len() + col]
    }

    /// The column called `name` (exact, then ignoring case and spaces).
    pub fn column(&self, name: &str) -> Option<usize> {
        let norm = |s: &str| s.trim().to_lowercase();
        self.names
            .iter()
            .position(|n| n == name)
            .or_else(|| self.names.iter().position(|n| norm(n) == norm(name)))
    }

    pub fn column_values(&self, col: usize) -> Vec<f64> {
        (0..self.rows).map(|r| self.get(r, col)).collect()
    }

    /// Treat `code` (e.g. -99) as missing.
    pub fn with_missing_code(mut self, code: f64) -> Self {
        for v in &mut self.values {
            if *v == code {
                *v = f64::NAN;
            }
        }
        self
    }

    /// The columns `cols` (in that order) with missing values handled;
    /// returns the matrix (row-major, n' × cols.len()) and the original
    /// row numbers kept.
    pub(crate) fn prepare(&self, cols: &[usize], missing: Missing) -> (Vec<f64>, Vec<usize>) {
        let p = cols.len();
        match missing {
            Missing::Listwise => {
                let mut out = Vec::new();
                let mut kept = Vec::new();
                for r in 0..self.rows {
                    if cols.iter().all(|&c| self.get(r, c).is_finite()) {
                        out.extend(cols.iter().map(|&c| self.get(r, c)));
                        kept.push(r);
                    }
                }
                (out, kept)
            }
            Missing::Mean => {
                let means: Vec<f64> = cols
                    .iter()
                    .map(|&c| {
                        let (s, k) = (0..self.rows)
                            .map(|r| self.get(r, c))
                            .filter(|v| v.is_finite())
                            .fold((0.0, 0usize), |(s, k), v| (s + v, k + 1));
                        if k == 0 { f64::NAN } else { s / k as f64 }
                    })
                    .collect();
                let mut out = Vec::with_capacity(self.rows * p);
                for r in 0..self.rows {
                    for (j, &c) in cols.iter().enumerate() {
                        let v = self.get(r, c);
                        out.push(if v.is_finite() { v } else { means[j] });
                    }
                }
                (out, (0..self.rows).collect())
            }
        }
    }

    /// Missing values per column `cols`.
    pub fn missing_counts(&self, cols: &[usize]) -> Vec<usize> {
        cols.iter()
            .map(|&c| {
                (0..self.rows)
                    .filter(|&r| !self.get(r, c).is_finite())
                    .count()
            })
            .collect()
    }
}

/// Sums of a row-major n × p matrix: column sums and cross products, from
/// which correlation matrices of the whole sample, a resample (rows
/// counted with weights) or the sample less one row come cheaply.
#[derive(Clone, Debug)]
pub(crate) struct Moments {
    pub n: f64,
    pub sum: Vec<f64>,
    /// p × p, row-major.
    pub cross: Vec<f64>,
}

impl Moments {
    /// Moments of `x` (n × p) with each row counted `count[r]` times
    /// (`None`: once). Centre `x` first (on the sample means): the
    /// correlations then lose no precision to cancellation.
    pub fn of(x: &[f64], p: usize, count: Option<&[u32]>) -> Self {
        let n = if p == 0 { 0 } else { x.len() / p };
        let mut m = Moments {
            n: 0.0,
            sum: vec![0.0; p],
            cross: vec![0.0; p * p],
        };
        for r in 0..n {
            let c = count.map_or(1.0, |c| f64::from(c[r]));
            if c == 0.0 {
                continue;
            }
            let row = &x[r * p..(r + 1) * p];
            m.n += c;
            for i in 0..p {
                let a = row[i] * c;
                m.sum[i] += a;
                let cr = &mut m.cross[i * p..i * p + i + 1];
                for (j, v) in cr.iter_mut().enumerate() {
                    *v += a * row[j];
                }
            }
        }
        m.mirror(p);
        m
    }

    fn mirror(&mut self, p: usize) {
        for i in 0..p {
            for j in 0..i {
                self.cross[j * p + i] = self.cross[i * p + j];
            }
        }
    }

    /// The moments without row `row` of `x`.
    pub fn without(&self, x: &[f64], p: usize, row: usize) -> Self {
        let mut m = self.clone();
        let r = &x[row * p..(row + 1) * p];
        m.n -= 1.0;
        for i in 0..p {
            m.sum[i] -= r[i];
            for j in 0..p {
                m.cross[i * p + j] -= r[i] * r[j];
            }
        }
        m
    }

    /// The correlation matrix (p × p); `None` when a variable is constant.
    pub fn correlation(&self, p: usize) -> Option<crate::linalg::Mat> {
        let n = self.n;
        let mean: Vec<f64> = self.sum.iter().map(|s| s / n).collect();
        let mut cov = crate::linalg::Mat::zeros(p, p);
        for i in 0..p {
            for j in 0..=i {
                let c = self.cross[i * p + j] - n * mean[i] * mean[j];
                cov[(i, j)] = c;
                cov[(j, i)] = c;
            }
        }
        let sd: Vec<f64> = (0..p).map(|i| cov[(i, i)].max(0.0).sqrt()).collect();
        let scale = (0..p)
            .map(|i| self.cross[i * p + i] / n)
            .fold(0.0f64, f64::max)
            .max(1e-300);
        if sd
            .iter()
            .any(|s| *s * *s <= 1e-12 * scale * n || !s.is_finite())
        {
            return None;
        }
        for i in 0..p {
            for j in 0..p {
                cov[(i, j)] = if i == j {
                    1.0
                } else {
                    (cov[(i, j)] / (sd[i] * sd[j])).clamp(-1.0, 1.0)
                };
            }
        }
        Some(cov)
    }
}

/// Correlation matrix computed two-pass (exact) from a row-major matrix.
pub(crate) fn correlation_two_pass(x: &[f64], p: usize) -> Option<crate::linalg::Mat> {
    let n = x.len() / p.max(1);
    if n < 2 {
        return None;
    }
    let mut mean = vec![0.0; p];
    for r in 0..n {
        for i in 0..p {
            mean[i] += x[r * p + i];
        }
    }
    for m in &mut mean {
        *m /= n as f64;
    }
    let mut cov = crate::linalg::Mat::zeros(p, p);
    let mut d = vec![0.0; p];
    for r in 0..n {
        for i in 0..p {
            d[i] = x[r * p + i] - mean[i];
        }
        for i in 0..p {
            for j in 0..=i {
                cov[(i, j)] += d[i] * d[j];
            }
        }
    }
    let sd: Vec<f64> = (0..p).map(|i| cov[(i, i)].sqrt()).collect();
    if sd.iter().any(|s| !(*s > 0.0) || !s.is_finite()) {
        return None;
    }
    let mut c = crate::linalg::Mat::identity(p);
    for i in 0..p {
        for j in 0..i {
            let v = cov[(i, j)] / (sd[i] * sd[j]);
            c[(i, j)] = v;
            c[(j, i)] = v;
        }
    }
    Some(c)
}

/// Means and standard deviations (n − 1) of each column.
pub(crate) fn mean_sd(x: &[f64], p: usize) -> (Vec<f64>, Vec<f64>) {
    let n = x.len() / p.max(1);
    let mut mean = vec![0.0; p];
    for r in 0..n {
        for i in 0..p {
            mean[i] += x[r * p + i];
        }
    }
    for m in &mut mean {
        *m /= n as f64;
    }
    let mut var = vec![0.0; p];
    for r in 0..n {
        for i in 0..p {
            let d = x[r * p + i] - mean[i];
            var[i] += d * d;
        }
    }
    let sd = var
        .iter()
        .map(|v| (v / (n as f64 - 1.0).max(1.0)).sqrt())
        .collect();
    (mean, sd)
}
