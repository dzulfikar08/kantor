//! Results as the diagram and the results view show them (no GPUI, tested
//! without a window): the numbers on the arrows, in the ellipses and on the
//! indicator links, and the result tables with their rule-of-thumb flags
//! (Hair et al. 2022), which Copy table and Insert as sheet also use.

use std::collections::HashMap;

use kantor_pls::{Estimate, Measurement, Results, WeightMode};
use kantor_ui::i18n::{t, t_dyn, tk};

/// How numbers are written.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fmt {
    /// Decimal comma (Indonesian).
    pub comma: bool,
    /// p values one-tailed.
    pub one_tailed: bool,
    /// The significance level.
    pub alpha: f64,
}

impl Default for Fmt {
    fn default() -> Self {
        Self {
            comma: false,
            one_tailed: false,
            alpha: 0.05,
        }
    }
}

impl Fmt {
    /// Three decimals ("0.812", "0,812"); "–" for none.
    pub fn num(&self, x: Option<f64>) -> String {
        match x {
            Some(v) if v.is_finite() => {
                let s = format!("{v:.3}");
                let s = if s == "-0.000" { "0.000".to_owned() } else { s };
                if self.comma { s.replace('.', ",") } else { s }
            }
            _ => "–".to_owned(),
        }
    }

    /// The p value of a bootstrap estimate, one- or two-tailed.
    pub fn p_of(&self, e: &Estimate) -> Option<f64> {
        if self.one_tailed { e.p_one_tailed } else { e.p }
    }
}

/// The numbers drawn on the diagram.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiagramLabels {
    /// (from, to) → the coefficient, with the p value in brackets after a
    /// bootstrap, and whether it is significant (`None`: no bootstrap).
    pub paths: HashMap<(String, String), (String, Option<bool>)>,
    /// Endogenous construct → "R² 0.681".
    pub r2: HashMap<String, String>,
    /// Indicator → its loading (reflective) or weight (formative), and
    /// whether it is flagged (a loading below 0.708).
    pub outer: HashMap<String, (String, bool)>,
}

/// The labels of `r` for the diagram.
pub fn diagram_labels(r: &Results, f: &Fmt) -> DiagramLabels {
    let mut out = DiagramLabels::default();
    let boot = r.bootstrap.as_ref();
    for p in &r.paths {
        let est = boot.and_then(|b| {
            b.paths
                .iter()
                .find(|e| e.from.as_deref() == Some(&p.from) && e.to.as_deref() == Some(&p.to))
        });
        let (text, sig) = match est.and_then(|e| f.p_of(e)) {
            Some(pv) => (
                format!("{} ({})", f.num(p.coefficient), f.num(Some(pv))),
                Some(pv < f.alpha),
            ),
            None => (f.num(p.coefficient), None),
        };
        out.paths
            .insert((p.from.clone(), p.to.clone()), (text, sig));
    }
    for c in &r.constructs {
        if c.r2.is_some() {
            out.r2.insert(c.name.clone(), format!("R² {}", f.num(c.r2)));
        }
    }
    for o in &r.outer {
        let Some(c) = r.construct(&o.construct) else {
            continue;
        };
        let (value, low) = if c.weights == WeightMode::ModeB {
            (o.weight, false)
        } else {
            let low = c.measurement == Measurement::Reflective
                && c.indicators.len() > 1
                && o.loading.is_some_and(|l| l.abs() < 0.708);
            (o.loading, low)
        };
        out.outer.insert(o.indicator.clone(), (f.num(value), low));
    }
    out
}

/// One cell of a result table.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub text: String,
    /// The number, for Insert as sheet.
    pub value: Option<f64>,
    /// Outside the usual threshold.
    pub flag: bool,
}

impl Cell {
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            value: None,
            flag: false,
        }
    }

    fn num(f: &Fmt, x: Option<f64>) -> Self {
        Self {
            text: f.num(x),
            value: x.filter(|v| v.is_finite()),
            flag: false,
        }
    }

    fn flagged(mut self, flag: bool) -> Self {
        self.flag = flag && self.value.is_some();
        self
    }
}

/// A result table with its title, column heads and a note on thresholds.
#[derive(Clone, Debug, PartialEq)]
pub struct ResultTable {
    pub title: String,
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    pub note: Option<String>,
}

impl ResultTable {
    fn new(title: &str, columns: Vec<String>) -> Self {
        Self {
            title: title.to_owned(),
            columns,
            rows: Vec::new(),
            note: None,
        }
    }

    fn note(mut self, note: &str) -> Self {
        self.note = Some(note.to_owned());
        self
    }

    /// The table as tab-separated text (its title first), the way
    /// spreadsheets copy cells.
    pub fn tsv(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.title);
        out.push('\n');
        out.push_str(&self.columns.join("\t"));
        out.push('\n');
        for row in &self.rows {
            let cells: Vec<&str> = row.iter().map(|c| c.text.as_str()).collect();
            out.push_str(&cells.join("\t"));
            out.push('\n');
        }
        out
    }
}

/// The results view's sections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    Measurement,
    Discriminant,
    Structural,
    Fit,
}

impl Section {
    pub const ALL: [Section; 4] = [
        Section::Measurement,
        Section::Discriminant,
        Section::Structural,
        Section::Fit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Section::Measurement => t("Measurement"),
            Section::Discriminant => t("Discriminant validity"),
            Section::Structural => t("Structural model"),
            Section::Fit => t("Model fit"),
        }
    }
}

/// Column heads: words are catalog keys (`tk`), symbols (`VIF`, `t`) are
/// kept as they are.
fn cols(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| t_dyn(n).to_owned()).collect()
}

/// `{names..., SE, t, p, CI}` heads after a bootstrap.
fn boot_cols(f: &Fmt, mut c: Vec<String>) -> Vec<String> {
    let lo = format!("{}%", trim_pct(f.alpha / 2.0 * 100.0));
    let hi = format!("{}%", trim_pct(100.0 - f.alpha / 2.0 * 100.0));
    c.extend([
        t("Mean").to_owned(),
        "SE".to_owned(),
        "t".to_owned(),
        if f.one_tailed {
            t("p (one-tailed)").to_owned()
        } else {
            t("p (two-tailed)").to_owned()
        },
        lo,
        hi,
    ]);
    c
}

fn trim_pct(x: f64) -> String {
    let s = format!("{x:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// The bootstrap cells of `e` (mean, SE, t, p flagged when not
/// significant, CI).
fn boot_cells(f: &Fmt, e: Option<&Estimate>) -> Vec<Cell> {
    let Some(e) = e else {
        return (0..6).map(|_| Cell::text("–")).collect();
    };
    let p = f.p_of(e);
    vec![
        Cell::num(f, e.mean),
        Cell::num(f, e.se),
        Cell::num(f, e.t),
        Cell::num(f, p).flagged(p.is_some_and(|p| p >= f.alpha)),
        Cell::num(f, e.ci[0]),
        Cell::num(f, e.ci[1]),
    ]
}

fn find_pair<'a>(list: &'a [Estimate], from: &str, to: &str) -> Option<&'a Estimate> {
    list.iter()
        .find(|e| e.from.as_deref() == Some(from) && e.to.as_deref() == Some(to))
}

/// The tables of one section.
pub fn tables(r: &Results, section: Section, f: &Fmt) -> Vec<ResultTable> {
    match section {
        Section::Measurement => measurement(r, f),
        Section::Discriminant => discriminant(r, f),
        Section::Structural => structural(r, f),
        Section::Fit => fit(r, f),
    }
}

fn type_label(c: &kantor_pls::ConstructResult) -> String {
    match (c.measurement, c.weights) {
        (Measurement::Reflective, _) => t("Reflective").to_owned(),
        (Measurement::Composite, WeightMode::ModeB) => t("Formative (Mode B)").to_owned(),
        (Measurement::Composite, WeightMode::ModeA) => t("Composite (Mode A)").to_owned(),
    }
}

fn measurement(r: &Results, f: &Fmt) -> Vec<ResultTable> {
    let boot = r.bootstrap.as_ref();
    let mut head = cols(&[
        tk("Construct"),
        tk("Indicator"),
        tk("Loading"),
        tk("Weight"),
        "VIF",
    ]);
    if boot.is_some() {
        head.extend(cols(&["t", "p"]));
    }
    let mut outer = ResultTable::new(t("Outer loadings and weights"), head).note(t(
        "Flagged: reflective loadings below 0.708, VIF of 5 or more; after a bootstrap, p values at or above the significance level (the loading's for reflective constructs, the weight's for formative ones).",
    ));
    for o in &r.outer {
        let c = r.construct(&o.construct);
        let reflective = c.is_some_and(|c| c.measurement == Measurement::Reflective);
        let mode_b = c.is_some_and(|c| c.weights == WeightMode::ModeB);
        let many = c.is_some_and(|c| c.indicators.len() > 1);
        let mut row = vec![
            Cell::text(o.construct.clone()),
            Cell::text(o.indicator.clone()),
            Cell::num(f, o.loading)
                .flagged(reflective && many && o.loading.is_some_and(|l| l.abs() < 0.708)),
            Cell::num(f, o.weight),
            Cell::num(f, o.vif).flagged(o.vif.is_some_and(|v| v >= 5.0)),
        ];
        if let Some(b) = boot {
            let list = if mode_b { &b.weights } else { &b.loadings };
            let e = list
                .iter()
                .find(|e| e.indicator.as_deref() == Some(o.indicator.as_str()));
            let p = e.and_then(|e| f.p_of(e));
            row.push(Cell::num(f, e.and_then(|e| e.t)));
            row.push(Cell::num(f, p).flagged(many && p.is_some_and(|p| p >= f.alpha)));
        }
        outer.rows.push(row);
    }
    let mut rel = ResultTable::new(
        t("Reliability and convergent validity"),
        cols(&[
            tk("Construct"),
            tk("Type"),
            tk("Cronbach's alpha"),
            "rho_A",
            "rho_C",
            "AVE",
        ]),
    )
    .note(t(
        "Flagged: alpha, rho_A or rho_C below 0.70, rho_C above 0.95, AVE below 0.50. Formative constructs have no reliability.",
    ));
    for c in &r.constructs {
        let single = c.indicators.len() < 2;
        let low = |x: Option<f64>| !single && x.is_some_and(|v| v < 0.7);
        rel.rows.push(vec![
            Cell::text(c.name.clone()),
            Cell::text(type_label(c)),
            Cell::num(f, c.alpha).flagged(low(c.alpha)),
            Cell::num(f, c.rho_a).flagged(low(c.rho_a)),
            Cell::num(f, c.rho_c)
                .flagged(low(c.rho_c) || (!single && c.rho_c.is_some_and(|v| v > 0.95))),
            Cell::num(f, c.ave).flagged(!single && c.ave.is_some_and(|v| v < 0.5)),
        ]);
    }
    vec![rel, outer]
}

/// A lower-triangle matrix table; `flag(row, col, value)`.
fn matrix(
    title: &str,
    m: &kantor_pls::Table,
    f: &Fmt,
    flag: impl Fn(usize, usize, f64) -> bool,
) -> ResultTable {
    let mut head = vec![String::new()];
    head.extend(m.cols.iter().cloned());
    let mut table = ResultTable::new(title, head);
    for (i, name) in m.rows.iter().enumerate() {
        let mut row = vec![Cell::text(name.clone())];
        for (j, v) in m.values[i].iter().enumerate() {
            row.push(Cell::num(f, *v).flagged(v.is_some_and(|v| flag(i, j, v))));
        }
        table.rows.push(row);
    }
    table
}

fn discriminant(r: &Results, f: &Fmt) -> Vec<ResultTable> {
    let fl = &r.fornell_larcker;
    let diag = |i: usize| {
        fl.values
            .get(i)
            .and_then(|row| row.get(i).copied().flatten())
    };
    let fornell = matrix(t("Fornell–Larcker criterion"), fl, f, |i, j, v| {
        i != j
            && (diag(i).is_some_and(|d| v.abs() > d) || diag(j).is_some_and(|d| v.abs() > d))
    })
    .note(t(
        "The square root of each construct's AVE (diagonal) should exceed its correlations with the other constructs; flagged where it doesn't.",
    ));
    let htmt = matrix(
        t("Heterotrait–monotrait ratio (HTMT)"),
        &r.htmt,
        f,
        |_, _, v| v > 0.85,
    )
    .note(t(
        "Flagged: above 0.85 (0.90 is the limit for conceptually similar constructs).",
    ));
    let mut out = vec![fornell, htmt];
    if let Some(b) = r.bootstrap.as_ref().filter(|b| !b.htmt.is_empty()) {
        let mut ci = ResultTable::new(
            t("HTMT confidence intervals"),
            cols(&[tk("Constructs"), "HTMT", tk("Mean"), tk("Upper bound (one-sided)")]),
        )
        .note(t(
            "Discriminant validity holds when the one-sided upper bound is below 0.90 (flagged at 0.85 or more).",
        ));
        for e in &b.htmt {
            ci.rows.push(vec![
                Cell::text(format!(
                    "{} ↔ {}",
                    e.from.as_deref().unwrap_or(""),
                    e.to.as_deref().unwrap_or("")
                )),
                Cell::num(f, e.original),
                Cell::num(f, e.mean),
                Cell::num(f, e.upper_one_sided)
                    .flagged(e.upper_one_sided.is_some_and(|u| u >= 0.85)),
            ]);
        }
        out.push(ci);
    }
    let cl = &r.cross_loadings;
    let own: Vec<Option<usize>> = cl
        .rows
        .iter()
        .map(|ind| {
            let c = r.outer(ind).map(|o| o.construct.as_str())?;
            cl.cols.iter().position(|x| x == c)
        })
        .collect();
    let mut head = vec![t("Indicator").to_owned()];
    head.extend(cl.cols.iter().cloned());
    let mut cross = ResultTable::new(t("Cross-loadings"), head).note(t(
        "Each indicator should load highest on its own construct; flagged where another construct's loading is as high.",
    ));
    for (i, ind) in cl.rows.iter().enumerate() {
        let mine = own[i].and_then(|c| cl.values[i][c]);
        let mut row = vec![Cell::text(ind.clone())];
        for (j, v) in cl.values[i].iter().enumerate() {
            let other = Some(j) != own[i];
            row.push(
                Cell::num(f, *v)
                    .flagged(other && mine.is_some_and(|m| v.is_some_and(|v| v.abs() >= m.abs()))),
            );
        }
        cross.rows.push(row);
    }
    out.push(cross);
    out
}

fn structural(r: &Results, f: &Fmt) -> Vec<ResultTable> {
    let boot = r.bootstrap.as_ref();
    let base = cols(&[tk("Path"), tk("Coefficient"), "f²", "VIF"]);
    let head = if boot.is_some() {
        boot_cols(f, base)
    } else {
        base
    };
    let mut paths = ResultTable::new(t("Path coefficients"), head).note(t(
        "f² of 0.02, 0.15 and 0.35 are small, medium and large effects. Flagged: inner VIF of 5 or more; after a bootstrap, p values at or above the significance level.",
    ));
    for p in &r.paths {
        let mut row = vec![
            Cell::text(format!("{} → {}", p.from, p.to)),
            Cell::num(f, p.coefficient),
            Cell::num(f, p.f2),
            Cell::num(f, p.vif).flagged(p.vif.is_some_and(|v| v >= 5.0)),
        ];
        if let Some(b) = boot {
            row.extend(boot_cells(f, find_pair(&b.paths, &p.from, &p.to)));
        }
        paths.rows.push(row);
    }
    let mut head = cols(&[tk("Construct"), "R²", tk("Adjusted R²")]);
    if boot.is_some() {
        head.push("p".to_owned());
    }
    let mut r2 = ResultTable::new(t("Coefficient of determination (R²)"), head).note(t(
        "R² of 0.25, 0.50 and 0.75 are weak, moderate and substantial (Hair et al.).",
    ));
    for c in r.constructs.iter().filter(|c| c.r2.is_some()) {
        let mut row = vec![
            Cell::text(c.name.clone()),
            Cell::num(f, c.r2),
            Cell::num(f, c.r2_adj),
        ];
        if let Some(b) = boot {
            let e =
                b.r2.iter()
                    .find(|e| e.construct.as_deref() == Some(c.name.as_str()));
            row.push(Cell::num(f, e.and_then(|e| f.p_of(e))));
        }
        r2.rows.push(row);
    }
    let mut out = vec![paths, r2];
    let effect_head = |first: &'static str| {
        let base = cols(&[first, tk("Effect")]);
        if boot.is_some() {
            boot_cols(f, base)
        } else {
            base
        }
    };
    if !r.effects.specific_indirect.is_empty() {
        let mut t1 = ResultTable::new(t("Specific indirect effects"), effect_head(tk("Path")));
        for (k, e) in r.effects.specific_indirect.iter().enumerate() {
            let mut row = vec![Cell::text(e.path.join(" → ")), Cell::num(f, e.effect)];
            if let Some(b) = boot {
                let est = b
                    .specific_indirect
                    .iter()
                    .find(|x| x.path.as_ref() == Some(&e.path))
                    .or_else(|| b.specific_indirect.get(k));
                row.extend(boot_cells(f, est));
            }
            t1.rows.push(row);
        }
        out.push(t1);
    }
    for (title, list, est) in [
        (
            t("Total indirect effects"),
            &r.effects.total_indirect,
            boot.map(|b| &b.total_indirect),
        ),
        (t("Total effects"), &r.effects.total, boot.map(|b| &b.total)),
    ] {
        if list.is_empty() {
            continue;
        }
        let mut table = ResultTable::new(title, effect_head(tk("Path")));
        for e in list {
            let mut row = vec![
                Cell::text(format!("{} → {}", e.from, e.to)),
                Cell::num(f, e.effect),
            ];
            if let Some(est) = est {
                row.extend(boot_cells(f, find_pair(est, &e.from, &e.to)));
            }
            table.rows.push(row);
        }
        out.push(table);
    }
    out
}

fn fit(r: &Results, f: &Fmt) -> Vec<ResultTable> {
    let mut fit = ResultTable::new(
        t("Model fit"),
        cols(&[tk("Measure"), tk("Saturated model"), tk("Estimated model")]),
    )
    .note(t("Flagged: SRMR of 0.08 or more."));
    let (s, e) = (&r.fit.saturated, &r.fit.estimated);
    let srmr_flag = |x: Option<f64>| x.is_some_and(|v| v >= 0.08);
    fit.rows.push(vec![
        Cell::text("SRMR"),
        Cell::num(f, s.srmr).flagged(srmr_flag(s.srmr)),
        Cell::num(f, e.srmr).flagged(srmr_flag(e.srmr)),
    ]);
    for (name, a, b) in [
        ("d_ULS", s.d_uls, e.d_uls),
        ("d_G", s.d_g, e.d_g),
        ("Chi²", s.chi2, e.chi2),
        ("NFI", s.nfi, e.nfi),
    ] {
        fit.rows
            .push(vec![Cell::text(name), Cell::num(f, a), Cell::num(f, b)]);
    }
    let mut info = ResultTable::new(t("Estimation"), cols(&[tk("Setting"), tk("Value")]));
    let algo = match r.algorithm {
        kantor_pls::Algorithm::Pls => "PLS-SEM",
        kantor_pls::Algorithm::Plsc => t("Consistent PLS (PLSc)"),
    };
    let scheme = match r.weighting {
        kantor_pls::Weighting::Path => t("Path"),
        kantor_pls::Weighting::Centroid => t("Centroid"),
        kantor_pls::Weighting::Factorial => t("Factorial"),
    };
    let count = |n: usize| Cell {
        text: n.to_string(),
        value: Some(n as f64),
        flag: false,
    };
    info.rows
        .push(vec![Cell::text(t("Algorithm")), Cell::text(algo)]);
    info.rows
        .push(vec![Cell::text(t("Weighting scheme")), Cell::text(scheme)]);
    info.rows
        .push(vec![Cell::text(t("Cases used")), count(r.n)]);
    info.rows.push(vec![
        Cell::text(t("Rows left out (missing values)")),
        count(r.dropped),
    ]);
    info.rows.push(vec![
        Cell::text(t("Missing values replaced")),
        count(r.replaced),
    ]);
    info.rows
        .push(vec![Cell::text(t("Iterations")), count(r.iterations)]);
    info.rows.push(vec![
        Cell::text(t("Converged")),
        Cell::text(if r.converged { t("Yes") } else { t("No") }),
    ]);
    if let Some(b) = &r.bootstrap {
        info.rows.push(vec![
            Cell::text(t("Bootstrap subsamples")),
            count(b.samples),
        ]);
        info.rows.push(vec![
            Cell::text(t("Subsamples that failed")),
            count(b.failed),
        ]);
        info.rows
            .push(vec![Cell::text(t("Seed")), count(b.seed as usize)]);
    }
    let mut out = vec![fit, info];
    if !r.warnings.is_empty() {
        let mut w = ResultTable::new(t("Warnings"), cols(&[tk("Warning")]));
        for text in &r.warnings {
            w.rows.push(vec![Cell::text(text.clone())]);
        }
        out.push(w);
    }
    out
}

/// Result tables laid out on a worksheet (Insert as new sheet): one under
/// the other, a blank row between, from A1.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SheetCells {
    /// `(row, col, input)`: numbers in English notation, text kept as text.
    pub cells: Vec<(u32, u32, String)>,
    /// Rows to make bold: `(row, first col, last col)`.
    pub bold: Vec<(u32, u32, u32)>,
    /// Number cells (shown with three decimals).
    pub numbers: Vec<(u32, u32)>,
    /// Flagged cells (filled).
    pub flagged: Vec<(u32, u32)>,
    /// The widest table's column count.
    pub cols: u32,
}

/// Where each table's cells go on a new sheet.
pub fn sheet_cells(tables: &[ResultTable]) -> SheetCells {
    let mut out = SheetCells::default();
    let mut row = 0u32;
    let text = |s: &str| {
        if s.starts_with('=') || s.trim().parse::<f64>().is_ok() {
            format!("'{s}")
        } else {
            s.to_owned()
        }
    };
    for t in tables {
        out.cells.push((row, 0, text(&t.title)));
        out.bold.push((row, 0, 0));
        row += 1;
        let last = t.columns.len().saturating_sub(1) as u32;
        for (j, c) in t.columns.iter().enumerate() {
            if !c.is_empty() {
                out.cells.push((row, j as u32, text(c)));
            }
        }
        out.bold.push((row, 0, last));
        out.cols = out.cols.max(t.columns.len() as u32);
        row += 1;
        for r in &t.rows {
            for (j, cell) in r.iter().enumerate() {
                let col = j as u32;
                match cell.value {
                    Some(v) => {
                        let v = (v * 1e6).round() / 1e6;
                        out.cells.push((row, col, format!("{v}")));
                        out.numbers.push((row, col));
                    }
                    None if cell.text.is_empty() || cell.text == "–" => {}
                    None => out.cells.push((row, col, text(&cell.text))),
                }
                if cell.flag {
                    out.flagged.push((row, col));
                }
            }
            row += 1;
        }
        if let Some(note) = &t.note {
            out.cells.push((row, 0, text(note)));
            row += 1;
        }
        row += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kantor_pls::{Bootstrap, Construct, Data, Model, Options, Path, estimate};

    /// Two blocks of three items: X → Y, one weak item on Y.
    fn results(boot: bool) -> Results {
        let n = 120;
        let mut rows = Vec::new();
        let mut seed: u64 = 7;
        let mut rnd = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 33) as f64 / (1u64 << 31) as f64) - 0.5
        };
        for _ in 0..n {
            let x = rnd() * 2.0;
            let y = 0.6 * x + rnd();
            rows.push(vec![
                x + rnd() * 0.5,
                x + rnd() * 0.5,
                x + rnd() * 0.5,
                y + rnd() * 0.5,
                y + rnd() * 0.5,
                y * 0.2 + rnd() * 2.0,
            ]);
        }
        let names = ["x1", "x2", "x3", "y1", "y2", "y3"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let data = Data::new(names, rows).unwrap();
        let model = Model {
            constructs: vec![
                Construct::reflective("X", &["x1", "x2", "x3"]),
                Construct::reflective("Y", &["y1", "y2", "y3"]),
            ],
            paths: vec![Path::new("X", "Y")],
            ..Default::default()
        };
        let opts = Options {
            bootstrap: boot.then(|| Bootstrap {
                samples: 100,
                ..Default::default()
            }),
            ..Default::default()
        };
        estimate(&data, &model, &opts).unwrap()
    }

    #[std::prelude::v1::test]
    fn numbers_and_p_values() {
        let f = Fmt::default();
        assert_eq!(f.num(Some(0.81249)), "0.812");
        assert_eq!(f.num(Some(-0.0001)), "0.000");
        assert_eq!(f.num(None), "–");
        let id = Fmt {
            comma: true,
            ..Fmt::default()
        };
        assert_eq!(id.num(Some(1.5)), "1,500");
        let e = Estimate {
            p: Some(0.04),
            p_one_tailed: Some(0.02),
            ..Default::default()
        };
        assert_eq!(f.p_of(&e), Some(0.04));
        let one = Fmt {
            one_tailed: true,
            ..f
        };
        assert_eq!(one.p_of(&e), Some(0.02));
        assert_eq!(trim_pct(2.5), "2.5");
        assert_eq!(trim_pct(97.5), "97.5");
        assert_eq!(trim_pct(5.0), "5");
    }

    #[std::prelude::v1::test]
    fn diagram_labels_follow_the_results() {
        let r = results(false);
        let f = Fmt::default();
        let l = diagram_labels(&r, &f);
        let (text, sig) = &l.paths[&("X".to_owned(), "Y".to_owned())];
        assert_eq!(*text, f.num(r.paths[0].coefficient));
        assert_eq!(*sig, None);
        assert_eq!(
            l.r2["Y"],
            format!("R² {}", f.num(r.construct("Y").unwrap().r2))
        );
        assert!(!l.r2.contains_key("X"));
        // The weak item is flagged.
        assert!(l.outer["y3"].1, "{:?}", l.outer["y3"]);
        assert!(!l.outer["x1"].1);
        // After a bootstrap the p value follows in brackets.
        let r = results(true);
        let l = diagram_labels(&r, &f);
        let (text, sig) = &l.paths[&("X".to_owned(), "Y".to_owned())];
        assert!(text.contains(" ("), "{text}");
        assert_eq!(*sig, Some(true));
    }

    #[std::prelude::v1::test]
    fn tables_flag_thresholds_and_copy_as_tsv() {
        let r = results(true);
        let f = Fmt::default();
        for s in Section::ALL {
            let tables = tables(&r, s, &f);
            assert!(!tables.is_empty());
            for t in &tables {
                for row in &t.rows {
                    assert_eq!(row.len(), t.columns.len(), "{}", t.title);
                }
            }
        }
        let m = tables(&r, Section::Measurement, &f);
        let outer = &m[1];
        let y3 = outer.rows.iter().find(|r| r[1].text == "y3").unwrap();
        assert!(y3[2].flag);
        let tsv = outer.tsv();
        assert!(tsv.starts_with(&format!("{}\n", outer.title)));
        assert_eq!(tsv.lines().count(), 2 + 6);
        let s = tables(&r, Section::Structural, &f);
        assert_eq!(s[0].rows.len(), 1);
        assert_eq!(s[0].rows[0][0].text, "X → Y");
        assert!(s[0].rows[0][1].value.is_some());
        let fit = tables(&r, Section::Fit, &f);
        assert_eq!(fit[0].rows.len(), 5);
    }

    #[std::prelude::v1::test]
    fn tables_go_on_a_sheet_one_under_another() {
        let t1 = ResultTable {
            title: "First".into(),
            columns: vec!["".into(), "B".into()],
            rows: vec![vec![
                Cell::text("2024"),
                Cell {
                    text: "0.812".into(),
                    value: Some(0.81234567),
                    flag: true,
                },
            ]],
            note: Some("Note".into()),
        };
        let mut t2 = t1.clone();
        t2.title = "Second".into();
        t2.note = None;
        let s = sheet_cells(&[t1, t2]);
        assert_eq!(s.cells[0], (0, 0, "First".into()));
        assert_eq!(s.cells[1], (1, 1, "B".into()));
        assert_eq!(s.cells[2], (2, 0, "'2024".into()));
        assert_eq!(s.cells[3], (2, 1, "0.812346".into()));
        assert_eq!(s.cells[4], (3, 0, "Note".into()));
        // A blank row, then the second table.
        assert_eq!(s.cells[5], (5, 0, "Second".into()));
        assert_eq!(s.numbers, vec![(2, 1), (7, 1)]);
        assert_eq!(s.flagged, vec![(2, 1), (7, 1)]);
        assert_eq!(s.bold[1], (1, 0, 1));
        assert_eq!(s.cols, 2);
    }
}
