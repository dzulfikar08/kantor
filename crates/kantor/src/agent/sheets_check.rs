//! `kantor sheets check` (and the MCP tool `sheets_check`): an audit of a
//! workbook's data and formulas, the mistakes reviewers look for. Each
//! finding is an error, warning or info with its sheet and cells; cells
//! with the same problem down a column are reported once, as a range. The
//! checks and how to fix what they find are in docs/sheets-json.md
//! ("Checking a workbook").

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use serde_json::{Value, json};
use sheets_engine::formula::{TokenKind, references, tokens};
use sheets_engine::{CellRange, ValueKind, Workbook, cell_name, column_name};

use super::sheets_clean::{DateOrder, parse_date_text, parse_number_text};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    fn name(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// One finding.
#[derive(Clone, Debug)]
pub struct Finding {
    pub severity: Severity,
    pub check: &'static str,
    /// 0-based; `None` for the whole workbook.
    pub sheet: Option<u32>,
    pub range: Option<CellRange>,
    pub message: String,
    /// The numbers and texts behind it.
    pub details: Value,
}

/// Every check: name, severity, what it finds.
pub const CHECKS: &[(&str, &str, &str)] = &[
    (
        "formula_error",
        "error (warning for #N/A)",
        "a formula shows #REF!, #DIV/0!, #VALUE!, #NAME?, #NUM!, #NULL!, #SPILL! or #CALC! (or #N/A, often a lookup that found nothing)",
    ),
    (
        "circular_reference",
        "error",
        "a formula depends on its own value (#CIRC!)",
    ),
    (
        "error_value",
        "warning",
        "an error typed as a value (not a formula's)",
    ),
    (
        "number_as_text",
        "warning",
        "numbers stored as text (\"1,250.50\", \"Rp 12.000\"): they don't add up in SUM; codes with leading zeros, phone numbers and long digit strings are left alone",
    ),
    (
        "date_as_text",
        "warning",
        "dates stored as text (\"15/01/2026\", \"3 Maret 2026\"): they don't sort, filter or subtract as dates",
    ),
    (
        "inconsistent_formula",
        "warning",
        "a formula that differs from the formulas above and below it (or left and right), which agree with each other, as Excel's green triangle",
    ),
    (
        "hardcoded_number",
        "warning",
        "a number typed into a formula next to a reference (=B2*1.11): put it in a cell or a name so it can be seen and changed; 0, 1, 2, 10, 12, 24, 60, 100, 1000, 365 and numbers in function arguments are left alone",
    ),
    (
        "typed_total",
        "error / warning",
        "a total row or column (\"Total\", \"Jumlah\", \"Subtotal\"...) with a typed number where a formula should add the numbers above (or left): error when it doesn't match their sum, warning when it does (it won't follow changes)",
    ),
    (
        "sum_misses_rows",
        "warning",
        "a SUM under (or beside) a block of numbers that leaves out some of the block's rows (columns)",
    ),
    (
        "refers_to_empty",
        "warning",
        "a formula that uses an empty cell on its own (=B2*C2 with C2 empty), outside ranges",
    ),
    (
        "mixed_formats",
        "warning",
        "one column's numbers in different kinds of number format (percent and plain, two currencies, dates and numbers)",
    ),
    (
        "mixed_units",
        "warning",
        "one column's values written with different units (\"12 kg\", \"3 lb\")",
    ),
    (
        "duplicate_rows",
        "warning",
        "rows of a data block that repeat an earlier row exactly",
    ),
    (
        "extra_spaces",
        "warning",
        "text with spaces at the start or end, or doubled (lookups and filters miss it)",
    ),
    (
        "merged_in_data",
        "warning",
        "merged cells inside a block of data or a table (they break sorting, filtering and copying)",
    ),
    (
        "volatile_function",
        "warning",
        "NOW, TODAY, RAND, RANDBETWEEN, OFFSET, INDIRECT, CELL or INFO in a workbook with many formulas: they recalculate on every change",
    ),
    (
        "external_link",
        "warning",
        "a formula or name that refers to another workbook",
    ),
    (
        "broken_name",
        "error",
        "a defined name that refers to #REF!",
    ),
    (
        "hidden_data",
        "info",
        "a hidden sheet, or hidden rows or columns, holding data",
    ),
];

/// The findings of a check run.
pub struct Report {
    pub findings: Vec<Finding>,
    pub sheets: Vec<String>,
    pub cells: u64,
}

impl Report {
    pub fn has_errors(&self) -> bool {
        self.findings.iter().any(|f| f.severity == Severity::Error)
    }

    fn count(&self, s: Severity) -> usize {
        self.findings.iter().filter(|f| f.severity == s).count()
    }

    pub fn json(&self) -> Value {
        let findings: Vec<Value> = self
            .findings
            .iter()
            .map(|f| {
                let mut v = json!({
                    "severity": f.severity.name(),
                    "check": f.check,
                    "sheet": f.sheet.and_then(|s| self.sheets.get(s as usize)),
                    "range": f.range.map(|r| r.name()),
                    "message": f.message,
                });
                if let Value::Object(m) = &f.details {
                    for (k, x) in m {
                        v[k] = x.clone();
                    }
                }
                v
            })
            .collect();
        json!({
            "sheets": self.sheets.len(),
            "cells_checked": self.cells,
            "errors": self.count(Severity::Error),
            "warnings": self.count(Severity::Warning),
            "info": self.count(Severity::Info),
            "findings": findings,
        })
    }

    pub fn text(&self, file: &str) -> String {
        let mut out = format!(
            "{file}: {} error(s), {} warning(s), {} info ({} cells in {} sheet(s) checked)\n",
            self.count(Severity::Error),
            self.count(Severity::Warning),
            self.count(Severity::Info),
            self.cells,
            self.sheets.len()
        );
        let mut last: Option<Option<u32>> = None;
        for f in &self.findings {
            if last != Some(f.sheet) {
                match f.sheet {
                    Some(s) => {
                        let _ = writeln!(out, "\nSheet {:?}", self.sheets[s as usize]);
                    }
                    None => out.push_str("\nWorkbook\n"),
                }
                last = Some(f.sheet);
            }
            let _ = writeln!(
                out,
                "  {:<8} {:<20} {:<12} {}",
                f.severity.name(),
                f.check,
                f.range.map(|r| r.name()).unwrap_or_default(),
                f.message
            );
        }
        if self.findings.is_empty() {
            out.push_str("No problems found.\n");
        }
        out
    }
}

/// Cells found by one check, to be grouped into ranges down a column.
struct Hits {
    /// `(check, severity, key) -> [(row, col, sample)]`; the key holds the
    /// part of the message cells must share to be grouped.
    map: BTreeMap<(&'static str, Severity, String), Vec<(u32, u32, String)>>,
}

impl Hits {
    fn new() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }

    fn add(
        &mut self,
        check: &'static str,
        sev: Severity,
        key: String,
        r: u32,
        c: u32,
        sample: String,
    ) {
        self.map
            .entry((check, sev, key))
            .or_default()
            .push((r, c, sample));
    }

    /// Runs of cells down a column (then single cells), as findings.
    fn findings(
        self,
        sheet: u32,
        message: impl Fn(&str, &str, &str, usize) -> String,
    ) -> Vec<Finding> {
        let mut out = Vec::new();
        for ((check, sev, key), mut cells) in self.map {
            cells.sort_by_key(|(r, c, _)| (*c, *r));
            let mut i = 0;
            while i < cells.len() {
                let (r0, c0, ref sample) = cells[i];
                let mut j = i + 1;
                while j < cells.len() && cells[j].1 == c0 && cells[j].0 == cells[j - 1].0 + 1 {
                    j += 1;
                }
                let r1 = cells[j - 1].0;
                let n = j - i;
                out.push(Finding {
                    severity: sev,
                    check,
                    sheet: Some(sheet),
                    range: Some(CellRange::from_corners((r0, c0), (r1, c0))),
                    message: message(check, &key, sample, n),
                    details: json!({"cells": n, "example": sample}),
                });
                i = j;
            }
        }
        out
    }
}

/// Is `t` a total's label ("Total", "Grand total", "Jumlah", "Subtotal")?
fn is_total_label(t: &str) -> bool {
    let t = t.trim().to_lowercase();
    let t = t.trim_end_matches(':');
    [
        "total",
        "grand total",
        "subtotal",
        "sub total",
        "sub-total",
        "jumlah",
        "total keseluruhan",
        "sum",
        "totals",
        "grand total:",
    ]
    .contains(&t)
        || t.starts_with("total ")
        || t.ends_with(" total")
        || t.starts_with("jumlah ")
}

/// A formula in R1C1 form relative to its cell, to compare formulas that
/// were filled from one another.
pub fn r1c1(formula: &str, row: u32, col: u32) -> String {
    let refs = references(formula);
    let mut out = String::new();
    let mut at = 0;
    for f in refs {
        out.push_str(&formula[at..f.span.start]);
        let text = &formula[f.span.clone()];
        let cells = text.rsplit_once('!').map_or(text, |(_, c)| c);
        if let Some(sheet) = &f.sheet {
            out.push_str(&sheet.to_lowercase());
            out.push('!');
        }
        let parts: Vec<&str> = cells.split(':').collect();
        let corners = [(f.range.top, f.range.left), (f.range.bottom, f.range.right)];
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                out.push(':');
            }
            let col_abs = p.starts_with('$');
            let row_abs = p.trim_start_matches('$').contains('$');
            // The corner this part names (a range's parts may be given in
            // any order; `range` is sorted, so compare with both).
            let (r, c) = if parts.len() == 1 {
                corners[0]
            } else {
                corners[i.min(1)]
            };
            let rr = if row_abs {
                format!("R{}", r + 1)
            } else {
                format!("R[{}]", i64::from(r) - i64::from(row))
            };
            let cc = if col_abs {
                format!("C{}", c + 1)
            } else {
                format!("C[{}]", i64::from(c) - i64::from(col))
            };
            out.push_str(&rr);
            out.push_str(&cc);
        }
        at = f.span.end;
    }
    out.push_str(&formula[at..]);
    out.to_uppercase()
}

/// Numbers typed into a formula as arithmetic operands next to a
/// reference: `(value, text)`.
fn typed_numbers(formula: &str) -> Vec<(f64, String)> {
    let toks: Vec<_> = tokens(formula)
        .into_iter()
        .filter(|t| t.1 != TokenKind::Space)
        .collect();
    if !toks.iter().any(|t| t.1 == TokenKind::Ref) {
        return Vec::new();
    }
    let arith =
        |k: Option<&TokenKind>| matches!(k, Some(TokenKind::Punct('*' | '/' | '+' | '-' | '^')));
    let mut out = Vec::new();
    for (i, (span, kind)) in toks.iter().enumerate() {
        if *kind != TokenKind::Number {
            continue;
        }
        let prev = i.checked_sub(1).map(|p| &toks[p].1);
        let next = toks.get(i + 1).map(|t| &t.1);
        // A unary minus belongs to the number.
        let prev_op = if matches!(prev, Some(TokenKind::Punct('-')))
            && (i < 2 || matches!(toks[i - 2].1, TokenKind::Punct('(' | ',' | '=' | ';')))
        {
            None
        } else {
            prev
        };
        if !(arith(prev_op) || arith(next)) {
            continue;
        }
        // Percent signs make it a rate typed in: still a typed number.
        let text = &formula[span.clone()];
        let Ok(v) = text.parse::<f64>() else {
            continue;
        };
        const PLAIN: [f64; 11] = [
            0.0, 1.0, 2.0, 10.0, 12.0, 24.0, 60.0, 100.0, 1000.0, 365.0, 0.5,
        ];
        if PLAIN.contains(&v) {
            continue;
        }
        out.push((v, text.to_owned()));
    }
    out
}

/// The kind of a number format, for mixed formats in a column.
fn format_kind(code: &str, date: bool) -> &'static str {
    let c = code.to_lowercase();
    if date {
        "date"
    } else if c.contains('%') {
        "percent"
    } else if c.contains("rp") || c.contains("idr") {
        "rupiah"
    } else if c.contains('$') && !c.contains("[$") || c.contains("usd") {
        "dollar"
    } else if c.contains('€') || c.contains("eur") {
        "euro"
    } else if c.contains('£') {
        "pound"
    } else if c == "@" {
        "text"
    } else {
        "number"
    }
}

/// The unit after a number in text (`12 kg` → `kg`).
fn unit_of(t: &str) -> Option<String> {
    let t = t.trim();
    let digits = t
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',')
        .count();
    if digits == 0 || digits == t.len() {
        return None;
    }
    let unit = t[digits..].trim();
    (unit.len() <= 6
        && !unit.is_empty()
        && unit
            .chars()
            .all(|c| c.is_alphabetic() || c == '²' || c == '³' || c == '/'))
    .then(|| unit.to_lowercase())
}

/// Text that is a number for the number_as_text check (not a code).
fn numeric_text(t: &str) -> Option<f64> {
    let s = t.trim();
    if s.is_empty() || s.len() > 20 {
        return None;
    }
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    // Codes: leading zeros, phone numbers, long ids.
    if (digits.len() > 1 && s.starts_with('0') && !s.starts_with("0.") && !s.starts_with("0,"))
        || s.starts_with('+')
        || digits.len() > 15
    {
        return None;
    }
    parse_number_text(s, '.')
        .or_else(|| parse_number_text(s, ','))
        .map(|(n, _)| n)
}

/// Run every check on `book` (or one sheet).
pub fn check(book: &Workbook, only: Option<u32>) -> Report {
    sheets_engine::numbers_invariant(|| check_inner(book, only))
}

fn check_inner(book: &Workbook, only: Option<u32>) -> Report {
    let names = book.sheet_names();
    let mut findings = Vec::new();
    let mut cells = 0u64;
    let total_formulas: usize = (0..names.len() as u32)
        .map(|s| super::sheets_author::formula_cells(book, s).len())
        .sum();
    for s in 0..names.len() as u32 {
        if only.is_some_and(|o| o != s) {
            continue;
        }
        let (f, n) = check_sheet(book, s, total_formulas);
        findings.extend(f);
        cells += n;
    }
    if only.is_none() {
        for n in book.defined_names() {
            let f = n.formula.trim();
            if f.starts_with("#REF!") || f.contains("!#REF!") {
                findings.push(Finding {
                    severity: Severity::Error,
                    check: "broken_name",
                    sheet: None,
                    range: None,
                    message: format!("name {} refers to {f}", n.name),
                    details: json!({"name": n.name, "refers_to": f}),
                });
            } else if f.contains('[') && f.contains(']') && !f.starts_with('"') {
                findings.push(Finding {
                    severity: Severity::Warning,
                    check: "external_link",
                    sheet: None,
                    range: None,
                    message: format!("name {} refers to another workbook: {f}", n.name),
                    details: json!({"name": n.name, "refers_to": f}),
                });
            }
        }
    }
    findings.sort_by(|a, b| {
        (
            a.sheet.map_or(u32::MAX, |s| s),
            a.severity,
            a.range.map(|r| (r.left, r.top)),
        )
            .cmp(&(
                b.sheet.map_or(u32::MAX, |s| s),
                b.severity,
                b.range.map(|r| (r.left, r.top)),
            ))
    });
    Report {
        findings,
        sheets: names,
        cells,
    }
}

/// One cell's facts.
struct Cell {
    input: String,
    kind: ValueKind,
    formula: bool,
}

fn check_sheet(book: &Workbook, s: u32, total_formulas: usize) -> (Vec<Finding>, u64) {
    let filled = book.filled_cells(s);
    let mut grid: HashMap<(u32, u32), Cell> = HashMap::with_capacity(filled.len());
    for &(r, c) in &filled {
        let input = book.input(s, r, c);
        let formula = input.starts_with('=');
        grid.insert(
            (r, c),
            Cell {
                input,
                kind: book.value_kind(s, r, c),
                formula,
            },
        );
    }
    let n = filled.len() as u64;
    let mut out = Vec::new();
    let mut hits = Hits::new();
    let number = |r: u32, c: u32| {
        grid.get(&(r, c))
            .is_some_and(|x| x.kind == ValueKind::Number)
    };
    let mut volatile = 0usize;
    let mut volatile_at = None;

    for &(r, c) in &filled {
        let cell = &grid[&(r, c)];
        let shown = || book.display_value(s, r, c);
        // Errors.
        if cell.kind == ValueKind::Error {
            let e = shown();
            if cell.formula {
                if e == "#CIRC!" {
                    hits.add(
                        "circular_reference",
                        Severity::Error,
                        e,
                        r,
                        c,
                        cell.input.clone(),
                    );
                } else if e == "#N/A" {
                    hits.add(
                        "formula_error",
                        Severity::Warning,
                        e,
                        r,
                        c,
                        cell.input.clone(),
                    );
                } else {
                    hits.add(
                        "formula_error",
                        Severity::Error,
                        e,
                        r,
                        c,
                        cell.input.clone(),
                    );
                }
            } else {
                hits.add(
                    "error_value",
                    Severity::Warning,
                    e,
                    r,
                    c,
                    cell.input.clone(),
                );
            }
        }
        if cell.formula {
            let upper = cell.input.to_uppercase();
            // External workbooks: [Book.xlsx] or [1] outside strings.
            let outside: String = tokens(&cell.input)
                .into_iter()
                .filter(|t| t.1 != TokenKind::Str)
                .map(|t| cell.input[t.0].to_owned())
                .collect();
            if outside.contains('[')
                && outside.contains("]")
                && (outside.contains("]!") || outside.contains(".xls") || outside.contains("]'"))
            {
                hits.add(
                    "external_link",
                    Severity::Warning,
                    String::new(),
                    r,
                    c,
                    cell.input.clone(),
                );
            }
            for f in [
                "NOW(",
                "TODAY(",
                "RAND(",
                "RANDBETWEEN(",
                "OFFSET(",
                "INDIRECT(",
                "CELL(",
                "INFO(",
            ] {
                if upper.contains(f) {
                    volatile += 1;
                    volatile_at.get_or_insert((r, c, f.trim_end_matches('(').to_owned()));
                    break;
                }
            }
            for (_, text) in typed_numbers(&cell.input) {
                hits.add(
                    "hardcoded_number",
                    Severity::Warning,
                    text,
                    r,
                    c,
                    cell.input.clone(),
                );
            }
            // Single empty cells used by the formula.
            {
                for f in references(&cell.input) {
                    if f.sheet.is_none()
                        && f.range.is_single()
                        && !grid.contains_key(&(f.range.top, f.range.left))
                        && !upper.contains("ISBLANK(")
                        && !upper.contains("IF(")
                    {
                        hits.add(
                            "refers_to_empty",
                            Severity::Warning,
                            String::new(),
                            r,
                            c,
                            format!(
                                "{} uses {} (empty)",
                                cell.input,
                                cell_name(f.range.top, f.range.left)
                            ),
                        );
                        break;
                    }
                }
            }
            // SUM ranges missing rows of the block above (or columns left).
            if upper.starts_with("=SUM(") && upper.ends_with(')') {
                let refs = references(&cell.input);
                if let [f] = refs.as_slice()
                    && f.sheet.is_none()
                    && f.span.end + 1 == cell.input.len()
                {
                    let g = f.range;
                    if g.left == g.right && g.left == c && g.bottom < r {
                        // The numbers directly above the formula.
                        let mut top = r;
                        while top > 0 && number(top - 1, c) && !grid[&(top - 1, c)].formula {
                            top -= 1;
                        }
                        if top < r {
                            let block = CellRange::from_corners((top, c), (r - 1, c));
                            let missing_bottom =
                                g.bottom + 1 < r && (g.bottom + 1..r).any(|x| number(x, c));
                            let missing_top = g.top > top;
                            if missing_bottom || missing_top {
                                hits.add(
                                    "sum_misses_rows",
                                    Severity::Warning,
                                    format!(
                                        "{} adds {} but the numbers above run {}",
                                        cell.input,
                                        g.name(),
                                        block.name()
                                    ),
                                    r,
                                    c,
                                    cell.input.clone(),
                                );
                            }
                        }
                    } else if g.top == g.bottom && g.top == r && g.right < c {
                        let mut left = c;
                        while left > 0 && number(r, left - 1) && !grid[&(r, left - 1)].formula {
                            left -= 1;
                        }
                        if left < c {
                            let block = CellRange::from_corners((r, left), (r, c - 1));
                            if (g.right + 1 < c && (g.right + 1..c).any(|x| number(r, x)))
                                || g.left > left
                            {
                                hits.add(
                                    "sum_misses_rows",
                                    Severity::Warning,
                                    format!(
                                        "{} adds {} but the numbers beside it run {}",
                                        cell.input,
                                        g.name(),
                                        block.name()
                                    ),
                                    r,
                                    c,
                                    cell.input.clone(),
                                );
                            }
                        }
                    }
                }
            }
        } else if cell.kind == ValueKind::Text {
            let t = shown();
            if t.starts_with(' ') || t.ends_with(' ') || t.contains("  ") || t.contains('\u{a0}') {
                hits.add(
                    "extra_spaces",
                    Severity::Warning,
                    String::new(),
                    r,
                    c,
                    t.clone(),
                );
            }
            if let Some(n) = numeric_text(&t) {
                let _ = n;
                hits.add(
                    "number_as_text",
                    Severity::Warning,
                    String::new(),
                    r,
                    c,
                    t.clone(),
                );
            } else if t.chars().filter(char::is_ascii_digit).count() >= 4
                && (parse_date_text(&t, DateOrder::Dmy).is_some()
                    || parse_date_text(&t, DateOrder::Mdy).is_some())
            {
                hits.add(
                    "date_as_text",
                    Severity::Warning,
                    String::new(),
                    r,
                    c,
                    t.clone(),
                );
            }
        }
    }

    // Inconsistent formulas: down columns, then along rows.
    let mut r1: HashMap<(u32, u32), String> = HashMap::new();
    for (&(r, c), cell) in &grid {
        if cell.formula {
            r1.insert((r, c), r1c1(&cell.input, r, c));
        }
    }
    let mut flagged = HashSet::new();
    for (&(r, c), me) in &r1 {
        for (a, b) in [
            (r.checked_sub(1).map(|x| (x, c)), Some((r + 1, c))),
            (c.checked_sub(1).map(|x| (r, x)), Some((r, c + 1))),
        ] {
            let (Some(a), Some(b)) = (a, b) else { continue };
            if let (Some(x), Some(y)) = (r1.get(&a), r1.get(&b))
                && x == y
                && x != me
                && flagged.insert((r, c))
            {
                let near = grid[&a].input.clone();
                hits.add(
                    "inconsistent_formula",
                    Severity::Warning,
                    String::new(),
                    r,
                    c,
                    format!("{} (its neighbours: {near})", grid[&(r, c)].input),
                );
            }
        }
    }

    out.extend(hits.findings(s, |check, key, sample, n| {
        let many = |one: &str, more: &str| {
            if n == 1 {
                one.to_owned()
            } else {
                format!("{n} {more}")
            }
        };
        match check {
            "formula_error" => {
                format!("{} {key}: {sample}", many("formula shows", "formulas show"))
            }
            "circular_reference" => format!(
                "{}: {sample}",
                many("circular reference", "cells in a circular reference")
            ),
            "error_value" => format!("{key} typed as a value"),
            "number_as_text" => format!(
                "{} (\"{sample}\"); fix: to_numbers",
                many("number stored as text", "numbers stored as text")
            ),
            "date_as_text" => format!(
                "{} (\"{sample}\"); fix: to_dates",
                many("date stored as text", "dates stored as text")
            ),
            "inconsistent_formula" => format!("formula differs from its neighbours: {sample}"),
            "hardcoded_number" => format!(
                "{key} typed into {}: {sample}; put it in a cell or name",
                many("a formula", "formulas")
            ),
            "sum_misses_rows" => key.to_owned(),
            "refers_to_empty" => format!(
                "{}: {sample}",
                many("formula uses an empty cell", "formulas use empty cells")
            ),
            "extra_spaces" => format!(
                "{} (\"{sample}\"); fix: trim",
                many("text with extra spaces", "texts with extra spaces")
            ),
            "external_link" => format!("refers to another workbook: {sample}"),
            other => format!("{other}: {sample}"),
        }
    }));

    if volatile > 0 && total_formulas >= 1000 {
        let (r, c, f) = volatile_at.expect("counted");
        out.push(Finding {
            severity: Severity::Warning,
            check: "volatile_function",
            sheet: Some(s),
            range: Some(CellRange::from_corners((r, c), (r, c))),
            message: format!("{volatile} formula(s) use volatile functions such as {f} in a workbook of {total_formulas} formulas: every change recalculates them"),
            details: json!({"cells": volatile}),
        });
    }
    out.extend(totals(book, s, &grid));
    out.extend(columns(book, s, &grid));
    out.extend(duplicates_and_merges(book, s, &grid));
    out.extend(hidden(book, s, &grid));
    (out, n)
}

/// Typed totals: rows (columns) labelled as totals with typed numbers
/// under (beside) a block of numbers.
fn totals(book: &Workbook, s: u32, grid: &HashMap<(u32, u32), Cell>) -> Vec<Finding> {
    let mut out = Vec::new();
    let is_num = |r: u32, c: u32| {
        grid.get(&(r, c))
            .is_some_and(|x| x.kind == ValueKind::Number)
    };
    let (rows, cols) = book.used_size(s);
    let mut seen = HashSet::new();
    for (&(r, c), cell) in grid {
        if cell.kind != ValueKind::Text
            || cell.formula
            || !is_total_label(&book.display_value(s, r, c))
        {
            continue;
        }
        // A total row: numbers to the right of the label.
        for cc in c + 1..cols.min(c + 60) {
            let Some(x) = grid.get(&(r, cc)) else {
                continue;
            };
            if x.kind != ValueKind::Number || x.formula || !seen.insert((r, cc)) {
                continue;
            }
            let mut top = r;
            while top > 0 && is_num(top - 1, cc) {
                top -= 1;
            }
            if r - top < 2 {
                continue;
            }
            let sum: f64 = (top..r)
                .filter_map(|x| book.number_at(s, x, cc).map(|n| n.0))
                .sum();
            let typed = book.number_at(s, r, cc).map_or(0.0, |n| n.0);
            let block = CellRange::from_corners((top, cc), (r - 1, cc));
            out.push(total_finding(s, r, cc, typed, sum, block));
        }
        // A total column: numbers under the label.
        for rr in r + 1..rows.min(r + 100_000) {
            let Some(x) = grid.get(&(rr, c)) else { break };
            if x.kind != ValueKind::Number {
                break;
            }
            if x.formula || !seen.insert((rr, c)) {
                continue;
            }
            let mut left = c;
            while left > 0 && is_num(rr, left - 1) {
                left -= 1;
            }
            if c - left < 2 {
                continue;
            }
            let sum: f64 = (left..c)
                .filter_map(|x| book.number_at(s, rr, x).map(|n| n.0))
                .sum();
            let typed = book.number_at(s, rr, c).map_or(0.0, |n| n.0);
            let block = CellRange::from_corners((rr, left), (rr, c - 1));
            out.push(total_finding(s, rr, c, typed, sum, block));
        }
    }
    out
}

fn total_finding(s: u32, r: u32, c: u32, typed: f64, sum: f64, block: CellRange) -> Finding {
    let same = (typed - sum).abs() <= 1e-9 * sum.abs().max(1.0);
    let fix = format!("=SUM({})", block.name());
    Finding {
        severity: if same {
            Severity::Warning
        } else {
            Severity::Error
        },
        check: "typed_total",
        sheet: Some(s),
        range: Some(CellRange::from_corners((r, c), (r, c))),
        message: if same {
            format!(
                "total typed as a number ({}); it won't follow changes: use {fix}",
                super::sheets_author::number(typed)
            )
        } else {
            format!(
                "total typed as {} but {} adds up to {}: use {fix}",
                super::sheets_author::number(typed),
                block.name(),
                super::sheets_author::number(sum)
            )
        },
        details: json!({"typed": typed, "sum": sum, "block": block.name(), "fix": fix}),
    }
}

/// Mixed number formats and units down columns.
fn columns(book: &Workbook, s: u32, grid: &HashMap<(u32, u32), Cell>) -> Vec<Finding> {
    let mut by_col: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &(r, c) in grid.keys() {
        by_col.entry(c).or_default().push(r);
    }
    let mut out = Vec::new();
    for (c, mut rows) in by_col {
        rows.sort_unstable();
        let mut kinds: BTreeMap<&'static str, (u32, String)> = BTreeMap::new();
        let mut units: BTreeMap<String, (u32, String)> = BTreeMap::new();
        let mut numbers = 0;
        for &r in &rows {
            let cell = &grid[&(r, c)];
            match cell.kind {
                ValueKind::Number => {
                    numbers += 1;
                    let st = book.cell_style(s, r, c);
                    let k = format_kind(&st.num_fmt, book.is_date_format(s, r, c));
                    kinds.entry(k).or_insert((r, st.num_fmt.clone()));
                }
                ValueKind::Text if !cell.formula => {
                    if let Some(u) = unit_of(&book.display_value(s, r, c)) {
                        units.entry(u).or_insert((r, book.display_value(s, r, c)));
                    }
                }
                _ => {}
            }
        }
        let (top, bottom) = (rows[0], *rows.last().unwrap_or(&rows[0]));
        let range = CellRange::from_corners((top, c), (bottom, c));
        if numbers >= 5 && kinds.len() > 1 {
            let list: Vec<String> = kinds
                .iter()
                .map(|(k, (r, f))| format!("{k} ({f} in {})", cell_name(*r, c)))
                .collect();
            out.push(Finding {
                severity: Severity::Warning,
                check: "mixed_formats",
                sheet: Some(s),
                range: Some(range),
                message: format!(
                    "column {} mixes number formats: {}",
                    column_name(c),
                    list.join(", ")
                ),
                details: json!({"formats": kinds.keys().collect::<Vec<_>>()}),
            });
        }
        if units.len() > 1 {
            let list: Vec<String> = units
                .iter()
                .map(|(u, (r, t))| format!("{u} (\"{t}\" in {})", cell_name(*r, c)))
                .collect();
            out.push(Finding {
                severity: Severity::Warning,
                check: "mixed_units",
                sheet: Some(s),
                range: Some(range),
                message: format!("column {} mixes units: {}", column_name(c), list.join(", ")),
                details: json!({"units": units.keys().collect::<Vec<_>>()}),
            });
        }
    }
    out
}

/// Duplicate rows in data blocks, merged cells inside data.
fn duplicates_and_merges(
    book: &Workbook,
    s: u32,
    grid: &HashMap<(u32, u32), Cell>,
) -> Vec<Finding> {
    let mut out = Vec::new();
    let (rows, cols) = book.used_size(s);
    if rows == 0 || cols == 0 {
        return out;
    }
    // Data blocks: regions around the first cell of each run of rows.
    let mut regions: Vec<CellRange> = Vec::new();
    let mut cells: Vec<&(u32, u32)> = grid.keys().collect();
    cells.sort_unstable();
    for &&(r, c) in &cells {
        if regions.iter().any(|g| g.contains(r, c)) {
            continue;
        }
        let g = book.current_region(s, r, c);
        if g.rows_count() >= 3 && g.cols_count() >= 2 && g.rows_count() <= 200_000 {
            regions.push(g);
        } else {
            regions.push(CellRange::from_corners((r, c), (r, c)));
        }
        if regions.len() > 500 {
            break;
        }
    }
    for g in regions
        .iter()
        .filter(|g| g.rows_count() >= 3 && g.cols_count() >= 2)
    {
        let mut seen: HashMap<Vec<String>, u32> = HashMap::new();
        let mut dups = Vec::new();
        for r in g.top + 1..=g.bottom {
            let key: Vec<String> = (g.left..=g.right)
                .map(|c| match grid.get(&(r, c)) {
                    Some(x) if x.formula => format!("f:{}", r1c1(&x.input, r, c)),
                    Some(_) => book.display_value(s, r, c).trim().to_lowercase(),
                    None => String::new(),
                })
                .collect();
            if key.iter().all(String::is_empty)
                || key.iter().all(|k| k.starts_with("f:") || k.is_empty())
            {
                continue;
            }
            match seen.get(&key) {
                Some(&first) => dups.push((r, first)),
                None => {
                    seen.insert(key, r);
                }
            }
        }
        if !dups.is_empty() {
            let rows_list: Vec<String> = dups
                .iter()
                .take(10)
                .map(|(r, f)| format!("{} (= {})", r + 1, f + 1))
                .collect();
            out.push(Finding {
                severity: Severity::Warning,
                check: "duplicate_rows",
                sheet: Some(s),
                range: Some(*g),
                message: format!(
                    "{} row(s) repeat an earlier row: {}{}; fix: remove_duplicates",
                    dups.len(),
                    rows_list.join(", "),
                    if dups.len() > 10 { ", …" } else { "" }
                ),
                details: json!({"rows": dups.iter().map(|d| d.0 + 1).collect::<Vec<_>>()}),
            });
        }
        // Merged cells below the block's first (header) row.
        let inner = CellRange {
            top: g.top + 1,
            ..*g
        };
        for m in book.merges_in(s, inner) {
            if m.top > g.top {
                out.push(Finding {
                    severity: Severity::Warning,
                    check: "merged_in_data",
                    sheet: Some(s),
                    range: Some(m),
                    message: format!(
                        "merged cells {} inside the data {}: they break sorting and filtering",
                        m.name(),
                        g.name()
                    ),
                    details: json!({"block": g.name()}),
                });
            }
        }
    }
    for t in book.tables(s) {
        for m in book.merges_in(s, t.area) {
            if !out
                .iter()
                .any(|f| f.check == "merged_in_data" && f.range == Some(m))
            {
                out.push(Finding {
                    severity: Severity::Warning,
                    check: "merged_in_data",
                    sheet: Some(s),
                    range: Some(m),
                    message: format!("merged cells {} in table {}", m.name(), t.name),
                    details: json!({"table": t.name}),
                });
            }
        }
    }
    out
}

/// Data out of sight: hidden sheet, hidden rows and columns with data.
fn hidden(book: &Workbook, s: u32, grid: &HashMap<(u32, u32), Cell>) -> Vec<Finding> {
    let mut out = Vec::new();
    if grid.is_empty() {
        return out;
    }
    if book.sheet_hidden(s) {
        out.push(Finding {
            severity: Severity::Info,
            check: "hidden_data",
            sheet: Some(s),
            range: None,
            message: format!("the sheet is hidden and holds {} filled cells", grid.len()),
            details: json!({"cells": grid.len()}),
        });
    }
    let rows: HashSet<u32> = grid
        .keys()
        .map(|k| k.0)
        .filter(|&r| book.is_row_hidden(s, r))
        .collect();
    let cols: HashSet<u32> = grid
        .keys()
        .map(|k| k.1)
        .filter(|&c| book.is_column_hidden(s, c))
        .collect();
    if !rows.is_empty() {
        let mut v: Vec<u32> = rows.into_iter().collect();
        v.sort_unstable();
        out.push(Finding {
            severity: Severity::Info,
            check: "hidden_data",
            sheet: Some(s),
            range: Some(CellRange::rows(v[0], *v.last().unwrap_or(&v[0]))),
            message: format!(
                "{} hidden row(s) hold data (first: {})",
                v.len(),
                v.iter()
                    .take(10)
                    .map(|r| (r + 1).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            details: json!({"rows": v.len()}),
        });
    }
    if !cols.is_empty() {
        let mut v: Vec<u32> = cols.into_iter().collect();
        v.sort_unstable();
        out.push(Finding {
            severity: Severity::Info,
            check: "hidden_data",
            sheet: Some(s),
            range: Some(CellRange::columns(v[0], *v.last().unwrap_or(&v[0]))),
            message: format!(
                "hidden column(s) {} hold data",
                v.iter()
                    .map(|c| column_name(*c))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            details: json!({"columns": v.len()}),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::sheets_author::{build, run_tool};

    fn checks(r: &Report) -> Vec<(&'static str, Severity, String)> {
        r.findings
            .iter()
            .map(|f| {
                (
                    f.check,
                    f.severity,
                    f.range.map(|r| r.name()).unwrap_or_default(),
                )
            })
            .collect()
    }

    fn has(r: &Report, check: &str, range: &str) -> bool {
        r.findings
            .iter()
            .any(|f| f.check == check && f.range.map(|r| r.name()).as_deref() == Some(range))
    }

    /// A sales export with the usual mistakes.
    fn messy() -> Workbook {
        build(
            &json!({"sheets": [{"name": "Sales", "rows": [
                ["Region", "Units", "Price", "Revenue", "Note"],
                ["North", 10, 2.5, "=B2*C2", "ok"],
                ["South", "'4", 3, "=B3*C3", " late "],
                ["East", 7, 1.25, "=B4*C4*1.11", ""],
                ["West", 5, 2, "=B5*C5", ""],
                ["North", 10, 2.5, "=B6*C6", "ok"],
                ["Total", 36, null, 999, null],
                [null, null, null, "=SUM(D2:D5)", null],
                ["Ratio", null, null, "=D2/F1", null],
                ["Bad", null, null, "=D2/0", null],
                ["Ship date", "'15/01/2026", null, "=NOSUCH(1)", null],
            ]}]}),
            None,
        )
        .unwrap()
    }

    #[std::prelude::v1::test]
    fn finds_the_usual_mistakes() {
        let b = messy();
        let r = check(&b, None);
        let all = checks(&r);
        assert!(has(&r, "number_as_text", "B3"), "{all:?}");
        assert!(has(&r, "date_as_text", "B11"), "{all:?}");
        assert!(has(&r, "extra_spaces", "E3"), "{all:?}");
        assert!(has(&r, "inconsistent_formula", "D4"), "{all:?}");
        assert!(has(&r, "hardcoded_number", "D4"), "{all:?}");
        assert!(has(&r, "typed_total", "D7"), "{all:?}");
        assert!(has(&r, "typed_total", "B7"), "{all:?}");
        assert!(has(&r, "formula_error", "D10"), "{all:?}");
        assert!(has(&r, "formula_error", "D11"), "{all:?}");
        assert!(has(&r, "refers_to_empty", "D9"), "{all:?}");
        assert!(
            has(&r, "duplicate_rows", "A1:E11")
                || r.findings.iter().any(|f| f.check == "duplicate_rows"),
            "{all:?}"
        );
        let total = r
            .findings
            .iter()
            .find(|f| f.check == "typed_total" && f.range.unwrap().name() == "D7")
            .unwrap();
        assert_eq!(total.severity, Severity::Error);
        // B7: the numbers above (B4:B6, under the text "4") add up to 22.
        assert!(has(&r, "typed_total", "B7"));
        assert!(r.has_errors());
        let j = r.json();
        assert!(j["errors"].as_u64().unwrap() >= 3);
        assert!(j["findings"][0]["severity"].is_string());
        let text = r.text("messy.xlsx");
        assert!(text.contains("Sheet \"Sales\""));
    }

    #[std::prelude::v1::test]
    fn clean_workbooks_pass() {
        let b = build(
            &json!({"sheets": [{"name": "Data", "rows": [
            ["Month", "Sales", "Cost", "Margin"],
            ["Jan", 100, 60, "=B2-C2"],
            ["Feb", 120, 70, "=B3-C3"],
            ["Mar", 90, 50, "=B4-C4"],
            ["Total", "=SUM(B2:B4)", "=SUM(C2:C4)", "=SUM(D2:D4)"],
            ["Share", "=B5/B5*100", null, "=ROUND(D5/B5, 2)"],
        ], "cells": {"F1": "Code", "F2": "'00123", "F3": "'+62 812 1234"}}]}),
            None,
        )
        .unwrap();
        let r = check(&b, None);
        assert!(r.findings.is_empty(), "{:?}", checks(&r));
    }

    #[std::prelude::v1::test]
    fn sums_that_miss_rows_and_more() {
        let mut b = build(
            &json!({"sheets": [{"name": "S", "rows": [
            ["Item", "Amount"], ["a", 1], ["b", 2], ["c", 3], ["d", 4], ["Total", "=SUM(B2:B4)"],
        ]}, {"name": "Hidden", "rows": [["x"]]}]}),
            None,
        )
        .unwrap();
        run_tool(&mut b, "hide_sheet", &json!({"sheet": "Hidden"}), None).unwrap();
        run_tool(
            &mut b,
            "write_range",
            &json!({"sheet": "S", "top_left": "D1", "values": [["=A1"], ["=D1"], ["=D2"]]}),
            None,
        )
        .unwrap();
        run_tool(
            &mut b,
            "write_range",
            &json!({"sheet": "S", "top_left": "F1", "values": [["12 kg"], ["3 lb"], ["=F9+F8"]]}),
            None,
        )
        .unwrap();
        run_tool(
            &mut b,
            "write_range",
            &json!({"sheet": "S", "top_left": "H1", "values": [["=G1+1"], ["=H1"], ["=H2"]]}),
            None,
        )
        .unwrap();
        let r = check(&b, None);
        let all = checks(&r);
        assert!(has(&r, "sum_misses_rows", "B6"), "{all:?}");
        assert!(
            r.findings.iter().any(|f| f.check == "hidden_data"),
            "{all:?}"
        );
        assert!(
            r.findings.iter().any(|f| f.check == "mixed_units"),
            "{all:?}"
        );
        assert!(!r.has_errors(), "{all:?}");
        // R1C1 forms.
        assert_eq!(
            r1c1("=B2*$C$1+Data!A1", 1, 3),
            "=R[0]C[-2]*R1C3+DATA!R[-1]C[-3]"
        );
        assert_eq!(r1c1("=SUM(B2:B4)", 4, 1), r1c1("=SUM(C2:C4)", 4, 2));
        assert_eq!(typed_numbers("=B2*1.11").len(), 1);
        assert!(typed_numbers("=ROUND(B2, 2)").is_empty());
        assert!(typed_numbers("=B2*100").is_empty());
        assert!(typed_numbers("=IF(B2>500, 1, 0)").is_empty());
        assert_eq!(typed_numbers("=-5+B2").len(), 1);
        assert!(numeric_text("00123").is_none());
        assert!(numeric_text("+62 812").is_none());
        assert_eq!(numeric_text("1,250.50"), Some(1250.5));
        assert!(is_total_label("Grand Total"));
        assert!(is_total_label("Jumlah"));
        assert!(!is_total_label("Totally"));
    }
}
