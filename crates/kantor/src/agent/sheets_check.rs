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
        "a total row or column (\"Total\", \"Jumlah\", \"Subtotal\"...) with a typed number where a formula should add the numbers above (or left): error when it doesn't match their sum, warning when it does (it won't follow changes). The block is the column's (row's) whole run of data up to its header, blanks and numbers stored as text included; text numbers are named, as SUM skips them until converted",
    ),
    (
        "sum_misses_rows",
        "error / warning",
        "a SUM under (or beside) a block of numbers that leaves out some of the block's rows (columns): error when they hold numbers (the total is wrong), warning when they hold only numbers stored as text",
    ),
    (
        "refers_to_empty",
        "warning",
        "a formula that uses an empty cell on its own (=B2*C2 with C2 empty), outside ranges",
    ),
    (
        "mixed_formats",
        "warning",
        "one column's numbers in different kinds of number format (percent and plain, two currencies, dates and numbers), or its dates in different date formats (yyyy-mm-dd next to dd/mm/yyyy)",
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
    (
        "text_cut_off",
        "warning",
        "text that doesn't fit where it's shown, measured with the fonts the PDF and PNG use: wider than its cell when the next cell isn't empty, wider than its merged cells (merged cells don't spill), cut at the printed page's edge, or taller than its row",
    ),
    (
        "number_hashes",
        "warning",
        "a number or date wider than its column, shown as ####",
    ),
    (
        "outside_print_area",
        "warning",
        "filled cells, charts or pictures outside the sheet's print area: they don't print, or print cut",
    ),
    (
        "split_by_page",
        "warning",
        "a chart or picture split over two or more printed pages by a page break",
    ),
    (
        "unsorted_bars",
        "info",
        "a bar or column chart of one series over 4+ text categories (not months, dates, years or other ordered labels) whose values aren't sorted",
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
            "checks": CHECKS.iter().map(|c| c.0).collect::<Vec<_>>(),
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
/// `(check, severity, key)`: the key holds the part of the message cells
/// must share to be grouped.
type HitKey = (&'static str, Severity, String);

struct Hits {
    /// Cells with their sample text, `(row, col, sample)`.
    map: BTreeMap<HitKey, Vec<(u32, u32, String)>>,
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
            // One finding per column: its first to last cell.
            let mut i = 0;
            while i < cells.len() {
                let (r0, c0, ref sample) = cells[i];
                let mut j = i + 1;
                while j < cells.len() && cells[j].1 == c0 {
                    j += 1;
                }
                let r1 = cells[j - 1].0;
                let n = j - i;
                let mut msg = message(check, &key, sample, n);
                let list: Vec<String> = cells[i..j]
                    .iter()
                    .take(20)
                    .map(|(r, c, _)| cell_name(*r, *c))
                    .collect();
                if (r1 - r0 + 1) as usize != n {
                    if n <= 6 {
                        msg.push_str(&format!(" (cells {})", list.join(", ")));
                    } else {
                        msg.push_str(&format!(
                            " ({n} cells between {} and {})",
                            cell_name(r0, c0),
                            cell_name(r1, c0)
                        ));
                    }
                }
                out.push(Finding {
                    severity: sev,
                    check,
                    sheet: Some(sheet),
                    range: Some(CellRange::from_corners((r0, c0), (r1, c0))),
                    message: msg,
                    details: json!({"cells": n, "example": sample, "cell_list": list}),
                });
                i = j;
            }
        }
        out
    }
}

/// A subtotal formula (SUM, SUBTOTAL, AGGREGATE): the end of a block.
fn is_subtotal(input: &str) -> bool {
    let u = input.to_uppercase();
    u.starts_with("=SUM(") || u.starts_with("=SUBTOTAL(") || u.starts_with("=AGGREGATE(")
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

/// A date format reduced to what it shows (`[$-421]dd/mm/yyyy;@` and
/// `DD/MM/YYYY` are the same), to compare a column's date formats.
fn date_shape(code: &str) -> String {
    let mut out = String::new();
    let mut in_bracket = false;
    let mut in_quote = false;
    for ch in code.split(';').next().unwrap_or("").chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                out.push(ch);
            }
            '[' if !in_quote => in_bracket = true,
            ']' if !in_quote => in_bracket = false,
            '\\' | '@' if !in_quote => {}
            _ if in_bracket => {}
            _ => out.extend(ch.to_lowercase()),
        }
    }
    out.trim().to_owned()
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
    // Made on first use: it scans the system's fonts.
    let mut shaper: Option<sheets_pdf::Shaper> = None;
    let total_formulas: usize = (0..names.len() as u32)
        .map(|s| super::sheets_author::formula_cells(book, s).len())
        .sum();
    for s in 0..names.len() as u32 {
        if only.is_some_and(|o| o != s) {
            continue;
        }
        let (f, n) = check_sheet(book, s, total_formulas);
        findings.extend(f);
        findings.extend(print_fit(book, s, &mut shaper));
        findings.extend(unsorted_bars(book, s));
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
            a.sheet.unwrap_or(u32::MAX),
            a.severity,
            a.range.map(|r| (r.left, r.top)),
        )
            .cmp(&(
                b.sheet.unwrap_or(u32::MAX),
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

/// The data block a total or a SUM adds up (see [`Region::block`]).
struct Block {
    /// From the first to the last cell of data.
    range: CellRange,
    /// Numbers (formulas' too): what SUM adds.
    numbers: usize,
    sum: f64,
    /// Numbers stored as text (SUM skips them) and what they add up to.
    texts: Vec<(u32, u32)>,
    text_sum: f64,
    /// Formulas showing errors (SUM shows the error too).
    errors: usize,
}

impl Block {
    /// The cells stored as text, for a message: `E8, E15 and 3 more`.
    fn text_list(&self) -> String {
        let names: Vec<String> = self
            .texts
            .iter()
            .take(4)
            .map(|(r, c)| cell_name(*r, *c))
            .collect();
        let more = self.texts.len().saturating_sub(4);
        if more > 0 {
            format!("{} and {more} more", names.join(", "))
        } else {
            names.join(", ")
        }
    }
}

/// A sheet's cells with what block searches need.
struct Region<'a> {
    book: &'a Workbook,
    s: u32,
    grid: &'a HashMap<(u32, u32), Cell>,
    /// Rows (columns) with any filled cell.
    rows: HashSet<u32>,
    cols: HashSet<u32>,
    /// Rows whose first cell is a total's label, columns whose top cell is.
    total_rows: HashSet<u32>,
    total_cols: HashSet<u32>,
}

impl<'a> Region<'a> {
    fn new(book: &'a Workbook, s: u32, grid: &'a HashMap<(u32, u32), Cell>) -> Self {
        let mut first_in_row: HashMap<u32, u32> = HashMap::new();
        let mut first_in_col: HashMap<u32, u32> = HashMap::new();
        for &(r, c) in grid.keys() {
            let e = first_in_row.entry(r).or_insert(c);
            *e = (*e).min(c);
            let e = first_in_col.entry(c).or_insert(r);
            *e = (*e).min(r);
        }
        let label = |r: u32, c: u32| {
            let cell = &grid[&(r, c)];
            cell.kind == ValueKind::Text
                && !cell.formula
                && is_total_label(&book.display_value(s, r, c))
        };
        Self {
            book,
            s,
            grid,
            rows: first_in_row.keys().copied().collect(),
            cols: first_in_col.keys().copied().collect(),
            total_rows: first_in_row
                .iter()
                .filter(|&(&r, &c)| label(r, c))
                .map(|(r, _)| *r)
                .collect(),
            total_cols: first_in_col
                .iter()
                .filter(|&(&c, &r)| label(r, c))
                .map(|(c, _)| *c)
                .collect(),
        }
    }

    /// The block of data above `(r, c)` (`up`), or left of it: the run of
    /// the column's (row's) numbers, numbers stored as text, formula errors
    /// and blank cells, up to a header or other text, a subtotal (SUM,
    /// SUBTOTAL), another total row (column) or an empty row (column),
    /// the region's edge. `None` without two cells of data.
    fn block(&self, r: u32, c: u32, up: bool) -> Option<Block> {
        let mut b = Block {
            range: CellRange::from_corners((r, c), (r, c)),
            numbers: 0,
            sum: 0.0,
            texts: Vec::new(),
            text_sum: 0.0,
            errors: 0,
        };
        let (mut first, mut last) = (None, None);
        // Whether the data are dates (a column of dates can be added up);
        // a date beside amounts is the row's (column's) label instead.
        let mut dated = None;
        let mut i = if up { r } else { c };
        while i > 0 {
            i -= 1;
            let (x, y) = if up { (i, c) } else { (r, i) };
            let Some(cell) = self.grid.get(&(x, y)) else {
                let empty_line = if up {
                    !self.rows.contains(&i)
                } else {
                    !self.cols.contains(&i)
                };
                if empty_line {
                    break;
                }
                continue;
            };
            if (up && self.total_rows.contains(&i)) || (!up && self.total_cols.contains(&i)) {
                break;
            }
            if cell.formula && is_subtotal(&cell.input) {
                break;
            }
            match cell.kind {
                ValueKind::Number => {
                    let date = self.book.is_date_format(self.s, x, y);
                    if *dated.get_or_insert(date) != date {
                        break;
                    }
                    b.numbers += 1;
                    b.sum += self.book.number_at(self.s, x, y).map_or(0.0, |n| n.0);
                }
                ValueKind::Error if cell.formula => b.errors += 1,
                ValueKind::Text if !cell.formula => {
                    match numeric_text(&self.book.display_value(self.s, x, y)) {
                        Some(v) => {
                            b.texts.push((x, y));
                            b.text_sum += v;
                        }
                        None => break,
                    }
                }
                _ => break,
            }
            first = Some(i);
            last.get_or_insert(i);
        }
        let (Some(first), Some(last)) = (first, last) else {
            return None;
        };
        if b.numbers + b.texts.len() < 2 {
            return None;
        }
        b.texts.reverse();
        b.range = if up {
            CellRange::from_corners((first, c), (last, c))
        } else {
            CellRange::from_corners((r, first), (r, last))
        };
        Some(b)
    }
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
    let region = Region::new(book, s, &grid);
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
                    let down = g.left == g.right && g.left == c && g.bottom < r;
                    let across = g.top == g.bottom && g.top == r && g.right < c;
                    if (down || across)
                        && let Some(b) = region.block(r, c, down)
                        && let Some((sev, key)) = sum_misses(&region, &cell.input, g, &b, down)
                    {
                        hits.add("sum_misses_rows", sev, key, r, c, cell.input.clone());
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
    out.extend(totals(book, s, &region));
    out.extend(columns(book, s, &grid));
    out.extend(duplicates_and_merges(book, s, &grid));
    out.extend(hidden(book, s, &grid));
    (out, n)
}

/// A SUM (`input`, adding `g`) under (beside) block `b` that leaves out
/// some of its rows (columns): error when they hold numbers (the total is
/// wrong), warning when they hold only numbers stored as text.
fn sum_misses(
    region: &Region,
    input: &str,
    g: CellRange,
    b: &Block,
    down: bool,
) -> Option<(Severity, String)> {
    let (lo, hi, glo, ghi) = if down {
        (b.range.top, b.range.bottom, g.top, g.bottom)
    } else {
        (b.range.left, b.range.right, g.left, g.right)
    };
    let missed: Vec<u32> = (lo..=hi).filter(|i| *i < glo || *i > ghi).collect();
    if missed.is_empty() {
        return None;
    }
    let at = |i: u32| {
        let (b0, b1) = (b.range.top, b.range.left);
        if down { (i, b1) } else { (b0, i) }
    };
    let numbers = missed
        .iter()
        .filter(|i| {
            region
                .grid
                .get(&at(**i))
                .is_some_and(|x| x.kind == ValueKind::Number)
        })
        .count();
    let texts = missed.iter().filter(|i| b.texts.contains(&at(**i))).count();
    if numbers == 0 && texts == 0 {
        return None;
    }
    let way = if down { "above" } else { "beside it" };
    let mut msg = format!(
        "{input} adds {} but the data {way} runs {}",
        g.name(),
        b.range.name()
    );
    if numbers > 0 {
        msg.push_str(&format!(
            ": it leaves out {numbers} number(s), so the total is wrong; use =SUM({})",
            b.range.name()
        ));
    } else {
        msg.push_str(&format!(
            ": the cells it leaves out are numbers stored as text; convert them (to_numbers) and use =SUM({})",
            b.range.name()
        ));
    }
    if !b.texts.is_empty() && numbers > 0 {
        msg.push_str(&format!(
            "; {} cell(s) in it are numbers stored as text ({}), which SUM skips: convert them first (to_numbers)",
            b.texts.len(),
            b.text_list()
        ));
    }
    Some((
        if numbers > 0 {
            Severity::Error
        } else {
            Severity::Warning
        },
        msg,
    ))
}

/// Typed totals: rows (columns) labelled as totals with typed numbers
/// under (beside) a block of numbers.
fn totals(book: &Workbook, s: u32, region: &Region) -> Vec<Finding> {
    let grid = region.grid;
    let mut out = Vec::new();
    let (rows, cols) = book.used_size(s);
    let mut seen = HashSet::new();
    // Pivot tables' totals are values Excel recalculates on refresh.
    let pivots: Vec<CellRange> = book.pivots(s).iter().map(|p| p.bounds).collect();
    let in_pivot = |r: u32, c: u32| pivots.iter().any(|p| p.contains(r, c));
    for (&(r, c), cell) in grid {
        if in_pivot(r, c) {
            continue;
        }
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
            let Some(b) = region.block(r, cc, true) else {
                continue;
            };
            let typed = book.number_at(s, r, cc).map_or(0.0, |n| n.0);
            out.push(total_finding(s, r, cc, typed, &b));
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
            let Some(b) = region.block(rr, c, false) else {
                continue;
            };
            let typed = book.number_at(s, rr, c).map_or(0.0, |n| n.0);
            out.push(total_finding(s, rr, c, typed, &b));
        }
    }
    out
}

fn total_finding(s: u32, r: u32, c: u32, typed: f64, b: &Block) -> Finding {
    let near = |a: f64, x: f64| (a - x).abs() <= 1e-9 * x.abs().max(a.abs()).max(1.0);
    let n = super::sheets_author::number;
    let block = b.range.name();
    let fix = format!("=SUM({block})");
    let with_text = b.sum + b.text_sum;
    let (severity, mut message) = if b.texts.is_empty() {
        if near(typed, b.sum) {
            (
                Severity::Warning,
                format!(
                    "total typed as a number ({}); it won't follow changes: use {fix}",
                    n(typed)
                ),
            )
        } else {
            (
                Severity::Error,
                format!(
                    "total typed as {} but {block} adds up to {}: use {fix}",
                    n(typed),
                    n(b.sum)
                ),
            )
        }
    } else {
        let texts = format!(
            "{} cell(s) of {block} are numbers stored as text ({}), which SUM skips",
            b.texts.len(),
            b.text_list()
        );
        if near(typed, with_text) {
            (
                Severity::Warning,
                format!(
                    "total typed as {}, which is {block} with its numbers stored as text; {texts}: convert them first (to_numbers), then use {fix}",
                    n(typed)
                ),
            )
        } else {
            (
                Severity::Error,
                format!(
                    "total typed as {} but {block} adds up to {} ({} with its numbers stored as text); {texts}: convert them first (to_numbers), then use {fix}",
                    n(typed),
                    n(b.sum),
                    n(with_text)
                ),
            )
        }
    };
    if b.errors > 0 {
        message.push_str(&format!(
            "; {} formula(s) in {block} show errors (SUM will too): fix them",
            b.errors
        ));
    }
    let text_cells: Vec<String> = b
        .texts
        .iter()
        .take(20)
        .map(|(r, c)| cell_name(*r, *c))
        .collect();
    Finding {
        severity,
        check: "typed_total",
        sheet: Some(s),
        range: Some(CellRange::from_corners((r, c), (r, c))),
        message,
        details: json!({
            "typed": typed,
            "sum": b.sum,
            "block": block,
            "numbers": b.numbers,
            "numbers_as_text": b.texts.len(),
            "sum_with_text": with_text,
            "text_cells": text_cells,
            "formula_errors": b.errors,
            "fix": fix,
        }),
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
        let mut dates = 0;
        let mut date_formats: BTreeMap<String, (u32, String)> = BTreeMap::new();
        for &r in &rows {
            let cell = &grid[&(r, c)];
            match cell.kind {
                ValueKind::Number => {
                    numbers += 1;
                    let st = book.cell_style(s, r, c);
                    let date = book.is_date_format(s, r, c);
                    let k = format_kind(&st.num_fmt, date);
                    kinds.entry(k).or_insert((r, st.num_fmt.clone()));
                    if date {
                        dates += 1;
                        date_formats
                            .entry(date_shape(&st.num_fmt))
                            .or_insert((r, st.num_fmt.clone()));
                    }
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
        if dates >= 3 && date_formats.len() > 1 {
            let list: Vec<String> = date_formats
                .values()
                .map(|(r, f)| format!("{f} in {}", cell_name(*r, c)))
                .collect();
            out.push(Finding {
                severity: Severity::Warning,
                check: "mixed_formats",
                sheet: Some(s),
                range: Some(range),
                message: format!(
                    "column {} shows its dates in different formats: {}; fix: format the column with one date format",
                    column_name(c),
                    list.join(", ")
                ),
                details: json!({"date_formats": date_formats.values().map(|(_, f)| f).collect::<Vec<_>>()}),
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

/// Most pages of a sheet looked at for text that doesn't fit.
const FIT_PAGES: usize = 60;

/// Sheets with more filled cells than this aren't checked for how they
/// print (laying out a big data sheet's pages is slow and rarely the
/// point).
const FIT_CELLS: usize = 50_000;

/// The start of a text for a message.
fn excerpt(t: &str) -> String {
    let t = t.trim();
    if t.chars().count() <= 40 {
        t.to_owned()
    } else {
        let s: String = t.chars().take(38).collect();
        format!("{}…", s.trim_end())
    }
}

/// Points, rounded for a message.
fn pt(x: f32) -> String {
    format!("{} pt", x.round())
}

/// Why a cell's text is cut.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Cut {
    /// The next cell isn't empty (or the text is right-aligned or
    /// centred against one), so it can't spill.
    Blocked,
    /// The printed page's edge, with more columns on the next page.
    PageEdge,
    /// The end of what prints (the used range or the print area): there
    /// is nothing past it to spill into.
    RangeEnd,
    /// The row is too short.
    Short,
}

/// A cell whose text doesn't fit: row, text, the size it needs and the
/// room it has (points).
type CutCell = (u32, String, f32, f32);

/// Text cut off and numbers shown as `####`, where the print layout
/// (`kantor convert`, PDF export) puts them: every cell's text is placed
/// in its room as the PDF draws it and measured with the same fonts.
/// Content outside the print area and objects split by page breaks.
fn print_fit(book: &Workbook, s: u32, shaper: &mut Option<sheets_pdf::Shaper>) -> Vec<Finding> {
    let mut out = Vec::new();
    if book.sheet_hidden(s) {
        return out;
    }
    let filled = book.filled_cells(s);
    if (filled.is_empty() && book.drawings(s).is_empty()) || filled.len() > FIT_CELLS {
        return out;
    }
    let shaper = shaper.get_or_insert_with(sheets_pdf::Shaper::new);
    let opts = sheets_pdf::Options {
        sheets: Some(vec![s]),
        ..Default::default()
    };
    let plan = sheets_pdf::Plan::new(book, &opts, shaper);
    let Some(sp) = plan.sheets.first() else {
        return out;
    };
    out.extend(outside_print_area(book, s, sp, &filled));
    let chars = |pt: f64| super::sheets_author::px_to_chars(book, s, pt / 0.75).ceil();
    let mut seen = HashSet::new();
    // (column, cut) → cells.
    let mut cut: BTreeMap<(u32, Cut), Vec<CutCell>> = BTreeMap::new();
    let mut hashes: BTreeMap<u32, Vec<CutCell>> = BTreeMap::new();
    for i in 0..plan.pages.len().min(FIT_PAGES) {
        let page = sheets_pdf::extract_plain(book, &plan, i);
        let pr = plan.pages[i];
        let last_band = pr.cols + 1 >= sp.areas.get(pr.area).map_or(1, |a| a.1.cols.len());
        for t in &page.texts {
            let Some((r, c)) = t.at else { continue };
            if !seen.insert((r, c)) {
                continue;
            }
            let Some(o) = sheets_pdf::overrun(shaper, t) else {
                continue;
            };
            let shown = excerpt(&t.text);
            if o.hashes {
                hashes
                    .entry(c)
                    .or_default()
                    .push((r, shown, o.need, o.room));
                continue;
            }
            let merge = book.merge_at(s, r, c);
            let right = t.clip[0] + t.clip[2];
            // The page's edge: a merged block reaching past it, or
            // overflowing text stopped at the page's last column.
            let page_edge = (o.right > 0.0
                && (t.cell[0] + t.cell[2] > right + 0.5
                    || (merge.is_none() && (right - page.block[0]).abs() < 0.5)))
                || (o.left > 0.0 && t.cell[0] < t.clip[0] - 0.5 && merge.is_some());
            let horizontal = o.left > 0.0 || o.right > 0.0;
            if let Some(m) = merge {
                let (msg, fix) = if !horizontal {
                    (
                        format!(
                            "\"{shown}\" is cut off at the {}: it needs {} of height and the merged cells {} are {} tall",
                            if o.top > 0.0 { "top" } else { "bottom" },
                            pt(o.need),
                            m.name(),
                            pt(o.room)
                        ),
                        format!(
                            "rows don't grow to fit merged cells, so set the height of row{} (about {} in all)",
                            if m.rows_count() > 1 {
                                format!("s {}–{}", m.top + 1, m.bottom + 1)
                            } else {
                                format!(" {}", m.top + 1)
                            },
                            pt(o.need + 4.0)
                        ),
                    )
                } else if page_edge {
                    (
                        format!(
                            "\"{shown}\" is cut off at the printed page's edge: the merged cells {} run past it",
                            m.name()
                        ),
                        "fit the sheet to the page width (page_setup fit: \"width\" or \"page\") or narrow the columns".to_owned(),
                    )
                } else {
                    (
                        format!(
                            "\"{shown}\" is cut off: it needs {} but the merged cells {} are {} wide, and merged cells don't spill into their neighbours",
                            pt(o.need),
                            m.name(),
                            pt(o.room)
                        ),
                        "widen the columns, shorten the text, or wrap it (and make the row taller)"
                            .to_owned(),
                    )
                };
                out.push(Finding {
                    severity: Severity::Warning,
                    check: "text_cut_off",
                    sheet: Some(s),
                    range: Some(m),
                    message: format!("{msg}; {fix}"),
                    details: json!({"text": t.text, "need_pt": o.need.round(), "room_pt": o.room.round(), "merged": m.name(), "page_edge": page_edge}),
                });
                continue;
            }
            let why = if !horizontal {
                Cut::Short
            } else if page_edge && last_band {
                Cut::RangeEnd
            } else if page_edge {
                Cut::PageEdge
            } else {
                Cut::Blocked
            };
            cut.entry((c, why))
                .or_default()
                .push((r, shown, o.need, o.room));
        }
    }
    let range_of = |c: u32, cells: &[CutCell]| {
        let (r0, r1) = cells
            .iter()
            .fold((u32::MAX, 0), |(a, b), x| (a.min(x.0), b.max(x.0)));
        CellRange::from_corners((r0, c), (r1, c))
    };
    for ((c, why), cells) in cut {
        let n = cells.len();
        let (r, shown, need, room) = &cells[0];
        let at = cell_name(*r, c);
        let col = column_name(c);
        let fix = match why {
            Cut::Blocked => {
                let wide = cells.iter().map(|x| x.2).fold(0.0f32, f32::max);
                format!(
                    "the cell beside it isn't empty, so it can't spill; widen column {col} (to about {} characters), shorten the text or wrap it",
                    chars(f64::from(wide) + 6.0)
                )
            }
            Cut::PageEdge => "the printed page's edge cuts it; fit the sheet to the page width (page_setup fit: \"width\" or \"page\") or narrow the columns".to_owned(),
            Cut::RangeEnd => format!(
                "column {col} is the last one that prints, so it can't spill past it; widen column {col} (to about {} characters) or wrap the text",
                chars(f64::from(cells.iter().map(|x| x.2).fold(0.0f32, f32::max)) + 6.0)
            ),
            Cut::Short => format!(
                "the row is too short for it; make row {} at least {} tall (or leave its height automatic)",
                r + 1,
                pt(need + 4.0)
            ),
        };
        let what = if why == Cut::Short {
            format!("it needs {} of height and has {}", pt(*need), pt(*room))
        } else {
            format!("it needs {} and has {}", pt(*need), pt(*room))
        };
        let message = if n == 1 {
            format!("\"{shown}\" is cut off: {what}; {fix}")
        } else {
            format!("{n} texts are cut off (\"{shown}\" in {at}: {what}); {fix}")
        };
        let list: Vec<String> = cells.iter().take(20).map(|x| cell_name(x.0, c)).collect();
        out.push(Finding {
            severity: Severity::Warning,
            check: "text_cut_off",
            sheet: Some(s),
            range: Some(range_of(c, &cells)),
            message,
            details: json!({"cells": n, "example": shown, "need_pt": need.round(), "room_pt": room.round(), "cell_list": list}),
        });
    }
    for (c, cells) in hashes {
        let n = cells.len();
        let wide = cells.iter().map(|x| x.2).fold(0.0f32, f32::max);
        let (_, shown, _, room) = &cells[0];
        let col = column_name(c);
        let want = chars(f64::from(wide) + 6.0);
        let message = if n == 1 {
            format!(
                "{shown} shows as ####: it needs {} and column {col} gives it {}; widen column {col} to about {want} characters",
                pt(wide),
                pt(*room)
            )
        } else {
            format!(
                "{n} numbers show as #### (the widest needs {}, column {col} gives {}); widen column {col} to about {want} characters",
                pt(wide),
                pt(*room)
            )
        };
        let list: Vec<String> = cells.iter().take(20).map(|x| cell_name(x.0, c)).collect();
        out.push(Finding {
            severity: Severity::Warning,
            check: "number_hashes",
            sheet: Some(s),
            range: Some(range_of(c, &cells)),
            message,
            details: json!({"cells": n, "example": shown, "need_pt": wide.round(), "room_pt": room.round(), "width_chars": want, "cell_list": list}),
        });
    }
    out
}

/// Filled cells and objects outside the print area, objects split by page
/// breaks.
fn outside_print_area(
    book: &Workbook,
    s: u32,
    sp: &sheets_pdf::SheetPlan,
    filled: &[(u32, u32)],
) -> Vec<Finding> {
    let mut out = Vec::new();
    let areas: Vec<CellRange> = sp.areas.iter().map(|a| a.0).collect();
    let set = !sp.print.print_area.is_empty();
    let names = areas
        .iter()
        .map(CellRange::name)
        .collect::<Vec<_>>()
        .join(", ");
    if set {
        let mut left: Vec<(u32, u32)> = filled
            .iter()
            .copied()
            .filter(|&(r, c)| !areas.iter().any(|a| a.contains(r, c)))
            .filter(|&(r, c)| !book.display_value(s, r, c).trim().is_empty())
            .filter(|&(r, c)| !book.is_row_hidden(s, r) && !book.is_column_hidden(s, c))
            .collect();
        if !left.is_empty() {
            left.sort_unstable();
            let (r0, c0, r1, c1) = left
                .iter()
                .fold((u32::MAX, u32::MAX, 0, 0), |(a, b, c, d), &(r, cc)| {
                    (a.min(r), b.min(cc), c.max(r), d.max(cc))
                });
            let (fr, fc) = left[0];
            out.push(Finding {
                severity: Severity::Warning,
                check: "outside_print_area",
                sheet: Some(s),
                range: Some(CellRange::from_corners((r0, c0), (r1, c1))),
                message: format!(
                    "{} filled cell(s) are outside the print area {names}, so they don't print (\"{}\" in {}); widen the print area (page_setup print_area) or clear it",
                    left.len(),
                    excerpt(&book.display_value(s, fr, fc)),
                    cell_name(fr, fc)
                ),
                details: json!({"cells": left.len(), "print_area": names}),
            });
        }
    }
    for d in book.drawings(s) {
        if !d.prints || d.hidden {
            continue;
        }
        let kind = match d.kind.as_ref() {
            sheets_engine::drawing::DrawingKind::Chart { .. } => "chart",
            sheets_engine::drawing::DrawingKind::Picture { .. } => "picture",
            _ => continue,
        };
        let Some(at) = super::sheets_list::anchor_range(&d.anchor) else {
            continue;
        };
        let label = if d.name.is_empty() {
            format!("a {kind} at {}", at.name())
        } else {
            format!("{kind} \"{}\" at {}", d.name, at.name())
        };
        let inside = areas
            .iter()
            .find(|a| a.contains(at.top, at.left) && a.contains(at.bottom, at.right));
        if set && inside.is_none() {
            let some = areas.iter().any(|a| a.intersects(&at));
            out.push(Finding {
                severity: Severity::Warning,
                check: "outside_print_area",
                sheet: Some(s),
                range: Some(at),
                message: format!(
                    "{label} is {} the print area {names}, so it {}; widen the print area (page_setup print_area) to take it in",
                    if some { "partly outside" } else { "outside" },
                    if some { "prints cut" } else { "doesn't print" }
                ),
                details: json!({"object": d.name, "kind": kind, "print_area": names}),
            });
            continue;
        }
        // Page breaks through it.
        let Some(grid) = sp
            .areas
            .iter()
            .find(|a| a.0.contains(at.top, at.left))
            .map(|a| &a.1)
        else {
            continue;
        };
        let bands = |b: &[(u32, u32)], lo: u32, hi: u32| {
            b.iter().filter(|(x, y)| *y >= lo && *x <= hi).count()
        };
        let across = bands(&grid.cols, at.left, at.right);
        let down = bands(&grid.rows, at.top, at.bottom);
        if across > 1 || down > 1 {
            let mut how = Vec::new();
            if across > 1 {
                how.push(format!("{across} pages across"));
            }
            if down > 1 {
                how.push(format!("{down} pages down"));
            }
            let fix = if across > 1 {
                "fit the sheet to the page width (page_setup fit: \"width\" or \"page\"), or move or resize it"
            } else {
                "move it, resize it or fit the sheet to one page (page_setup fit: \"page\")"
            };
            out.push(Finding {
                severity: Severity::Warning,
                check: "split_by_page",
                sheet: Some(s),
                range: Some(at),
                message: format!(
                    "{label} is split over {} by page breaks; {fix}",
                    how.join(" and ")
                ),
                details: json!({"object": d.name, "kind": kind, "pages_across": across, "pages_down": down}),
            });
        }
    }
    out
}

/// Is a category label one with an order of its own (months, days,
/// quarters, years, numbers, ranges, sizes, levels)? Then sorting a chart
/// by value would scramble it.
fn ordered_label(t: &str) -> bool {
    let l = t.trim().to_lowercase();
    if l.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    // Months (English, Indonesian) and their short forms, days, periods
    // and steps, levels and sizes.
    const WORDS: &str = "january february march april may june july august september \
        october november december jan feb mar apr jun jul aug sep sept oct nov dec \
        januari februari maret mei juni juli agustus oktober desember agu agt okt des \
        peb monday tuesday wednesday thursday friday saturday sunday mon tue tues wed \
        thu thur thurs fri sat sun senin selasa rabu kamis jumat jum'at sabtu minggu \
        ahad quarter kuartal triwulan semester week pekan month bulan year tahun day \
        hari phase fase tahap step langkah stage round q h fy ytd mtd first second \
        third fourth fifth last pertama kedua ketiga keempat kelima terakhir before \
        after sebelum sesudah early mid late awal tengah akhir low medium high rendah \
        sedang tinggi very sangat small large kecil besar xs s m l xl xxl poor fair \
        good excellent buruk cukup baik under over less more kurang lebih below above \
        min max";
    l.split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|w| !w.is_empty())
        .any(|w| WORDS.split_whitespace().any(|x| x == w))
}

/// Bar and column charts of one series over 4+ text categories whose
/// values aren't sorted: sorted bars read as a ranking at a glance.
fn unsorted_bars(book: &Workbook, s: u32) -> Vec<Finding> {
    let mut out = Vec::new();
    let names = book.sheet_names();
    let resolve = |f: &str| -> Option<(u32, CellRange)> {
        let refs = references(&format!("={f}"));
        let [r] = refs.as_slice() else { return None };
        let sheet = match &r.sheet {
            Some(n) => names.iter().position(|x| x.eq_ignore_ascii_case(n))? as u32,
            None => s,
        };
        (r.range.top == r.range.bottom || r.range.left == r.range.right).then_some((sheet, r.range))
    };
    let cells = |(sheet, g): (u32, CellRange)| -> Vec<(u32, u32, u32)> {
        let mut v = Vec::new();
        for r in g.top..=g.bottom.min(g.top + 500) {
            for c in g.left..=g.right.min(g.left + 500) {
                if !book.is_row_hidden(sheet, r) && !book.is_column_hidden(sheet, c) {
                    v.push((sheet, r, c));
                }
            }
        }
        v
    };
    for d in book.drawings(s) {
        let sheets_engine::drawing::DrawingKind::Chart { xml, .. } = d.kind.as_ref() else {
            continue;
        };
        let series = super::sheets_list::chart_series(xml);
        let [one] = series.as_slice() else { continue };
        if one["plot"] != "bar" {
            continue;
        }
        let (Some(cat), Some(val)) = (one["categories"].as_str(), one["values"].as_str()) else {
            continue;
        };
        let (Some(cat), Some(val)) = (resolve(cat), resolve(val)) else {
            continue;
        };
        let cats = cells(cat);
        let vals = cells(val);
        if cats.len() != vals.len() || cats.len() < 4 {
            continue;
        }
        let mut labels = Vec::new();
        let mut values = Vec::new();
        let mut text = true;
        for (&(cs, cr, cc), &(vs, vr, vc)) in cats.iter().zip(&vals) {
            let label = book.display_value(cs, cr, cc);
            if book.value_kind(cs, cr, cc) != ValueKind::Text || label.trim().is_empty() {
                text = false;
                break;
            }
            let Some((v, _)) = book.number_at(vs, vr, vc) else {
                text = false;
                break;
            };
            labels.push(label);
            values.push(v);
        }
        if !text || labels.iter().any(|l| ordered_label(l)) {
            continue;
        }
        // "Other" kept last is still a sorted chart.
        let other = |l: &str| {
            matches!(
                l.trim().to_lowercase().as_str(),
                "other" | "others" | "lainnya" | "lain-lain" | "lain lain" | "rest" | "sisanya"
            )
        };
        let mut v: &[f64] = &values;
        if labels.last().is_some_and(|l| other(l)) {
            v = &values[..values.len() - 1];
        }
        let up = v.windows(2).all(|w| w[0] <= w[1]);
        let down = v.windows(2).all(|w| w[0] >= w[1]);
        if up || down {
            continue;
        }
        let at = super::sheets_list::anchor_range(&d.anchor);
        let cat_name = cat.1.name();
        let val_name = val.1.name();
        let label = if d.name.is_empty() {
            "a chart".to_owned()
        } else {
            format!("chart \"{}\"", d.name)
        };
        out.push(Finding {
            severity: Severity::Info,
            check: "unsorted_bars",
            sheet: Some(s),
            range: at,
            message: format!(
                "{label} shows {} categories ({cat_name}) in an order that isn't by value; sort the categories by value ({val_name}) so the ranking reads at a glance",
                labels.len()
            ),
            details: json!({"chart": d.name, "categories": cat_name, "values": val_name}),
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
        assert!(has(&r, "formula_error", "D9:D10"), "{all:?}");
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
        // B7: 36 is B2:B6 with the "4" stored as text in B3 (SUM gives 32).
        let b7 = r
            .findings
            .iter()
            .find(|f| f.check == "typed_total" && f.range.unwrap().name() == "B7")
            .unwrap();
        assert_eq!(b7.severity, Severity::Warning, "{}", b7.message);
        assert!(b7.message.contains("B2:B6"), "{}", b7.message);
        assert!(b7.message.contains("B3"), "{}", b7.message);
        assert_eq!(b7.details["sum"], json!(32.0));
        assert_eq!(b7.details["numbers_as_text"], json!(1));
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
        ], "cells": {"F1": "Code", "F2": "'00123", "F3": "'+62 812 1234"},
            // F is the last column printed: its text can't spill.
            "widths": {"F": 14}}]}),
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
        // B5 holds a number the SUM leaves out: the total is wrong.
        let miss = r
            .findings
            .iter()
            .find(|f| f.check == "sum_misses_rows")
            .unwrap();
        assert_eq!(miss.severity, Severity::Error, "{}", miss.message);
        assert!(miss.message.contains("B2:B5"), "{}", miss.message);
        assert!(
            r.findings.iter().any(|f| f.check == "hidden_data"),
            "{all:?}"
        );
        assert!(
            r.findings.iter().any(|f| f.check == "mixed_units"),
            "{all:?}"
        );
        assert!(
            r.findings
                .iter()
                .all(|f| f.severity != Severity::Error || f.check == "sum_misses_rows"),
            "{all:?}"
        );
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

    /// Months as dates in the first column label the rows: a row total
    /// that adds the amounts beside them is right, not a SUM that misses
    /// the date (the false error that sent a job round in circles).
    #[std::prelude::v1::test]
    fn dates_labelling_rows_are_not_data() {
        let mut b = build(
            &json!({"sheets": [{"name": "Ringkasan", "rows": [
                ["Bulan", "Jakarta", "Bandung", "Surabaya", "Total"],
                [46023, 120, 95, 80, "=SUM(B2:D2)"],
                [46054, 132, 99, 85, "=SUM(B3:D3)"],
                [46082, 140, 101, 90, "=SUM(B4:D4)"],
                ["Total", "=SUM(B2:B4)", "=SUM(C2:C4)", "=SUM(D2:D4)", "=SUM(E2:E4)"],
            ]}]}),
            None,
        )
        .unwrap();
        run_tool(
            &mut b,
            "format",
            &json!({"sheet": "Ringkasan", "range": "A2:A4", "number_format": "mmm yyyy"}),
            None,
        )
        .unwrap();
        let r = check(&b, None);
        let all = checks(&r);
        assert!(
            !r.findings.iter().any(|f| f.check == "sum_misses_rows"),
            "{all:?}"
        );
        assert!(!r.has_errors(), "{all:?}");

        // A plain number there is still data the total leaves out.
        let b = build(
            &json!({"sheets": [{"name": "S", "rows": [
                ["Item", "Q1", "Q2", "Q3", "Total"],
                [5, 120, 95, 80, "=SUM(B2:D2)"],
                [6, 132, 99, 85, "=SUM(B3:D3)"],
            ]}]}),
            None,
        )
        .unwrap();
        let r = check(&b, None);
        assert!(has(&r, "sum_misses_rows", "E2"), "{:?}", checks(&r));

        // A column of dates added up is still checked as data.
        let mut b = build(
            &json!({"sheets": [{"name": "D", "rows": [
                ["Due"], [46023], [46054], [46082], ["=SUM(A2:A3)"],
            ]}]}),
            None,
        )
        .unwrap();
        run_tool(
            &mut b,
            "format",
            &json!({"sheet": "D", "range": "A2:A5", "number_format": "yyyy-mm-dd"}),
            None,
        )
        .unwrap();
        let r = check(&b, None);
        assert!(has(&r, "sum_misses_rows", "A5"), "{:?}", checks(&r));
    }

    /// A long column with numbers stored as text here and there, as an
    /// export makes them: the total's block is the whole column.
    #[std::prelude::v1::test]
    fn typed_totals_see_the_whole_column() {
        let mut rows = vec![json!(["Order", "Units", "Price"])];
        for i in 0..40 {
            let units = if i % 10 == 3 {
                json!(format!("'{}", i + 1))
            } else {
                json!(i + 1)
            };
            let price = if i == 20 { Value::Null } else { json!(100) };
            rows.push(json!([format!("SO-{i}"), units, price]));
        }
        rows.push(json!(["Total", 999, 3900]));
        let b = build(&json!({"sheets": [{"name": "S", "rows": rows}]}), None).unwrap();
        let r = check(&b, None);
        let all = checks(&r);
        let b42 = r
            .findings
            .iter()
            .find(|f| f.check == "typed_total" && f.range.unwrap().name() == "B42")
            .unwrap_or_else(|| panic!("{all:?}"));
        assert_eq!(b42.severity, Severity::Error);
        assert_eq!(b42.details["block"], "B2:B41", "{}", b42.message);
        // 1..40 = 820; the four text ones (4, 14, 24, 34) = 76.
        assert_eq!(b42.details["sum"], json!(744.0));
        assert_eq!(b42.details["sum_with_text"], json!(820.0));
        assert_eq!(b42.details["numbers_as_text"], json!(4));
        assert!(b42.message.contains("B5, B15, B25, B35"), "{}", b42.message);
        assert!(
            b42.message.contains("convert them first"),
            "{}",
            b42.message
        );
        // A blank cell inside the column doesn't end the block.
        let c42 = r
            .findings
            .iter()
            .find(|f| f.check == "typed_total" && f.range.unwrap().name() == "C42")
            .unwrap_or_else(|| panic!("{all:?}"));
        assert_eq!(c42.details["block"], "C2:C41", "{}", c42.message);
        assert_eq!(c42.severity, Severity::Warning, "{}", c42.message);
    }

    #[std::prelude::v1::test]
    fn sums_missing_only_text_numbers_warn() {
        let b = build(
            &json!({"sheets": [{"name": "S", "rows": [
                ["Item", "Amount"], ["a", "'5"], ["b", 2], ["c", 3], ["Total", "=SUM(B3:B4)"],
            ]}]}),
            None,
        )
        .unwrap();
        let r = check(&b, None);
        let miss = r
            .findings
            .iter()
            .find(|f| f.check == "sum_misses_rows")
            .unwrap_or_else(|| panic!("{:?}", checks(&r)));
        assert_eq!(miss.severity, Severity::Warning, "{}", miss.message);
        assert!(miss.message.contains("stored as text"), "{}", miss.message);
    }

    #[std::prelude::v1::test]
    fn dates_in_different_formats() {
        let mut b = build(
            &json!({"sheets": [{"name": "S", "rows": [
                ["Date"], [46037], [46038], [46039], [46040],
            ]}]}),
            None,
        )
        .unwrap();
        let one = |r: u32| CellRange::from_corners((r, 0), (r, 0));
        for r in 1..=4 {
            let f = if r == 3 { "dd/mm/yyyy" } else { "yyyy-mm-dd" };
            b.set_range_style_value(0, one(r), "num_fmt", f).unwrap();
        }
        let r = check(&b, None);
        let f = r
            .findings
            .iter()
            .find(|f| f.check == "mixed_formats")
            .unwrap_or_else(|| panic!("{:?}", checks(&r)));
        assert!(f.message.contains("dd/mm/yyyy in A4"), "{}", f.message);
        assert!(f.message.contains("yyyy-mm-dd in A2"), "{}", f.message);
        // One shape written two ways is one format.
        b.set_range_style_value(0, one(3), "num_fmt", "[$-421]YYYY-MM-DD;@")
            .unwrap();
        let r = check(&b, None);
        assert!(
            !r.findings.iter().any(|f| f.check == "mixed_formats"),
            "{:?}",
            checks(&r)
        );
        assert_eq!(date_shape("[$-421]dd/mm/yyyy;@"), "dd/mm/yyyy");
    }

    fn found<'a>(r: &'a Report, check: &str) -> Vec<&'a Finding> {
        r.findings.iter().filter(|f| f.check == check).collect()
    }

    /// Text cut by merged cells and full neighbours, numbers as ####; text
    /// that spills or fits is left alone.
    #[std::prelude::v1::test]
    fn text_that_does_not_fit() {
        let long =
            "A subtitle much longer than its merged cells could ever show on one line, ".repeat(3);
        let spill = "A note that spills into the empty cells beside it";
        let b = build(
            &json!({"sheets": [{"name": "S",
            "cells": {
                "A2": long,
                "A4": spill, "B4": "x",
                "A6": spill,
                "A8": "Short note",
                "B10": 123_456_789_012_i64,
                "A12": "Big", "A14": "Wrapped and fits",
                "J1": "A note in the last column that prints"
            },
            "widths": {"A": 10, "B": 4, "C:H": 10},
            "heights": {"12": 9},
            "formats": [
                {"range": "A2:D2", "merge": true, "align": "center"},
                {"range": "A8:F8", "merge": true},
                {"range": "B10", "number_format": "#,##0"},
                {"range": "A12", "size": 24},
                {"range": "A14", "wrap": true}
            ]}]}),
            None,
        )
        .unwrap();
        let r = check(&b, None);
        let all = checks(&r);
        let cut = found(&r, "text_cut_off");
        let merged = cut
            .iter()
            .find(|f| f.range.unwrap().name() == "A2:D2")
            .unwrap_or_else(|| panic!("{all:?}"));
        assert!(
            merged.message.contains("merged cells A2:D2"),
            "{}",
            merged.message
        );
        assert_eq!(merged.details["page_edge"], json!(false));
        assert!(has(&r, "text_cut_off", "A4"), "{all:?}");
        let a4 = cut
            .iter()
            .find(|f| f.range.unwrap().name() == "A4")
            .unwrap();
        assert!(a4.message.contains("isn't empty"), "{}", a4.message);
        // A big font in a 9 pt row.
        let a12 = cut
            .iter()
            .find(|f| f.range.unwrap().name() == "A12")
            .unwrap_or_else(|| panic!("{all:?}"));
        assert!(a12.message.contains("row 12"), "{}", a12.message);
        assert!(has(&r, "number_hashes", "B10"), "{all:?}");
        // Nothing prints past the last column to spill into.
        let j1 = cut
            .iter()
            .find(|f| f.range.unwrap().name() == "J1")
            .unwrap_or_else(|| panic!("{all:?}"));
        assert!(
            j1.message.contains("last one that prints"),
            "{}",
            j1.message
        );
        // Spilling into empty cells, a short text in a wide merge and wrapped
        // text in an automatic row are fine.
        for ok in ["A6", "A8:F8", "A8", "A14"] {
            assert!(
                !r.findings
                    .iter()
                    .any(|f| f.range.map(|g| g.name()).as_deref() == Some(ok)),
                "{ok}: {all:?}"
            );
        }
        assert_eq!(cut.len(), 4, "{all:?}");
    }

    /// A merged title wider than the printed page is cut at the page's
    /// edge, and a wide chart is split over two pages; fitting the sheet to
    /// the page fixes both.
    #[std::prelude::v1::test]
    fn the_page_cuts_wide_content() {
        let title = "Quarterly revenue by outlet and month with the reported amounts beside them";
        let sheet = |fit: bool| {
            let mut s = json!({"name": "Dash",
                "rows": [[title], [], ["Outlet", "Revenue"], ["Dago", 5], ["Kopo", 9], ["Antapani", 7], ["Buah Batu", 3]],
                "widths": {"A:C": 60},
                "formats": [{"range": "A1:C1", "merge": true, "align": "center", "size": 12}],
                "charts": [{"chart": "line", "data": "Dash!A3:B7", "at": "B10", "to": "C30"}]});
            if fit {
                s["page_setup"] = json!({"fit": "page", "landscape": true});
            }
            json!({"sheets": [s]})
        };
        let b = build(&sheet(false), None).unwrap();
        let r = check(&b, None);
        let all = checks(&r);
        let title_cut = found(&r, "text_cut_off")
            .into_iter()
            .find(|f| f.range.unwrap().name() == "A1:C1")
            .unwrap_or_else(|| panic!("{all:?}"));
        assert_eq!(
            title_cut.details["page_edge"],
            json!(true),
            "{}",
            title_cut.message
        );
        assert!(title_cut.message.contains("page"), "{}", title_cut.message);
        let split = found(&r, "split_by_page");
        assert_eq!(split.len(), 1, "{all:?}");
        assert!(
            split[0].message.contains("pages across"),
            "{}",
            split[0].message
        );

        let b = build(&sheet(true), None).unwrap();
        let r = check(&b, None);
        let all = checks(&r);
        for c in [
            "text_cut_off",
            "split_by_page",
            "number_hashes",
            "outside_print_area",
        ] {
            assert!(found(&r, c).is_empty(), "{c}: {all:?}");
        }
    }

    /// Rows don't grow to fit merged cells: a big title in a merged row
    /// is cut at the top; a stored height that fits it is fine.
    #[std::prelude::v1::test]
    fn merged_titles_taller_than_their_row() {
        let book = |height: Option<u32>| {
            let mut s = json!({"name": "S",
                "rows": [["Sales summary"], [], ["Region", "Units"], ["North", 5]],
                "formats": [{"range": "A1:D1", "merge": true, "size": 24, "bold": true}]});
            if let Some(h) = height {
                s["heights"] = json!({"1": h});
            }
            build(&json!({"sheets": [s]}), None).unwrap()
        };
        let r = check(&book(None), None);
        let all = checks(&r);
        let f = found(&r, "text_cut_off")
            .into_iter()
            .find(|f| f.range.unwrap().name() == "A1:D1")
            .unwrap_or_else(|| panic!("{all:?}"));
        assert!(f.message.contains("cut off at the top"), "{}", f.message);
        assert!(f.message.contains("row 1"), "{}", f.message);
        let r = check(&book(Some(36)), None);
        assert!(found(&r, "text_cut_off").is_empty(), "{:?}", checks(&r));
    }

    #[std::prelude::v1::test]
    fn content_outside_the_print_area() {
        let b = build(
            &json!({"sheets": [{"name": "S",
                "rows": [["Item", "Amount"], ["a", 1], ["b", 2], ["c", 3]],
                "cells": {"F2": "Checked by Ana"},
                "charts": [{"chart": "column", "data": "S!A1:B4", "at": "D6", "to": "J20"}],
                "page_setup": {"print_area": "A1:C10"}}]}),
            None,
        )
        .unwrap();
        let r = check(&b, None);
        let all = checks(&r);
        let out = found(&r, "outside_print_area");
        assert_eq!(out.len(), 2, "{all:?}");
        assert!(has(&r, "outside_print_area", "F2"), "{all:?}");
        assert!(has(&r, "outside_print_area", "D6:J20"), "{all:?}");
    }

    /// One series of outlets in no order: sort them. Months, a sorted
    /// chart, two series and a line chart are left alone.
    #[std::prelude::v1::test]
    fn unsorted_bar_charts() {
        let b = build(
            &json!({"sheets": [{"name": "D",
                "rows": [
                    ["Outlet", "Revenue", "Month", "Sales", "Cost"],
                    ["Dago", 3466, "Jan", 50, 20],
                    ["Buah Batu", 1546, "Feb", 70, 30],
                    ["Antapani", 1548, "Mar", 40, 25],
                    ["Kopo", 1446, "Apr", 90, 35],
                ],
                "charts": [
                    {"chart": "bar", "data": "D!A1:B5", "at": "G1", "to": "M15", "title": "By outlet"},
                    {"chart": "column", "data": "D!C1:D5", "at": "G17", "to": "M30"},
                    {"chart": "column", "categories": "D!A2:A5", "series": [{"name": "Revenue", "values": "D!B2:B5"}, {"name": "Sales", "values": "D!D2:D5"}], "at": "G32", "to": "M45"},
                    {"chart": "line", "data": "D!A1:B5", "at": "G47", "to": "M60"}
                ],
                "page_setup": {"fit": "page"}}]}),
            None,
        )
        .unwrap();
        let r = check(&b, None);
        let all = checks(&r);
        let f = found(&r, "unsorted_bars");
        assert_eq!(f.len(), 1, "{all:?}");
        assert_eq!(f[0].severity, Severity::Info);
        assert_eq!(f[0].range.unwrap().name(), "G1:M15", "{}", f[0].message);
        assert!(
            f[0].message.contains("sort the categories by value"),
            "{}",
            f[0].message
        );
        assert!(f[0].message.contains("A2:A5"), "{}", f[0].message);
        assert!(ordered_label("Q1 2026"));
        assert!(ordered_label("Maret"));
        assert!(ordered_label("High"));
        assert!(!ordered_label("Buah Batu"));
        assert!(!ordered_label("Dago"));
    }

    #[std::prelude::v1::test]
    fn sorted_bars_pass() {
        let b = build(
            &json!({"sheets": [{"name": "D",
                "rows": [["Outlet", "Revenue"], ["Dago", 3466], ["Antapani", 1548], ["Buah Batu", 1546], ["Kopo", 1446], ["Other", 1500]],
                "widths": {"A": 16},
                "charts": [{"chart": "bar", "data": "D!A1:B6", "at": "D1", "to": "J15"}],
                "page_setup": {"fit": "page"}}]}),
            None,
        )
        .unwrap();
        let r = check(&b, None);
        assert!(r.findings.is_empty(), "{:?}", checks(&r));
    }
}
