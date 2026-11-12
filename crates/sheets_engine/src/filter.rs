//! AutoFilter: filtering a table's or a sheet's rows by values or by
//! conditions, as Excel does.
//!
//! The state is the `<autoFilter>` element itself, kept in IronCalc's model
//! (the table's, or the sheet's: `vendor/ironcalc_base` KANTOR_PATCHES §13),
//! so undo, saving and moved rows all see one thing. Rows filtered out are
//! hidden rows, as in the file (`<row hidden="1">`), so the grid's row-height
//! model skips them and SUBTOTAL/AGGREGATE know them as filtered (a hidden
//! row under a filter that is on).
//!
//! Applying a filter re-evaluates every column's condition over the data rows
//! and hides or shows rows to match, like Excel's Reapply. Filters Kantor
//! doesn't evaluate (top 10, dynamic dates, colours, icons) are kept as they
//! are and leave rows as the file had them.

use std::collections::{BTreeSet, HashSet};

use anyhow::{Result, anyhow, bail};
use ironcalc_base::kantor_tables::auto_filter_ref;
use roxmltree::Node;

use crate::sort::{CellValue, cell_value};
use crate::{CalcMode, CellRange, Workbook, cell_name};

/// Whose filter: a table's (by name) or the sheet's own AutoFilter.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FilterScope {
    Table(String),
    Sheet,
}

/// A comparison of a condition filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    Equal,
    NotEqual,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
}

impl FilterOp {
    fn ooxml(self) -> &'static str {
        match self {
            FilterOp::Equal => "equal",
            FilterOp::NotEqual => "notEqual",
            FilterOp::GreaterThan => "greaterThan",
            FilterOp::GreaterThanOrEqual => "greaterThanOrEqual",
            FilterOp::LessThan => "lessThan",
            FilterOp::LessThanOrEqual => "lessThanOrEqual",
        }
    }

    fn from_ooxml(s: &str) -> Self {
        match s {
            "notEqual" => FilterOp::NotEqual,
            "greaterThan" => FilterOp::GreaterThan,
            "greaterThanOrEqual" => FilterOp::GreaterThanOrEqual,
            "lessThan" => FilterOp::LessThan,
            "lessThanOrEqual" => FilterOp::LessThanOrEqual,
            _ => FilterOp::Equal,
        }
    }
}

/// One condition: `op` against `value` (a number, a date serial, or text with
/// `*` and `?` wildcards for equal / not equal).
#[derive(Debug, Clone, PartialEq)]
pub struct Condition {
    pub op: FilterOp,
    pub value: String,
}

/// A date (or part of one) ticked in a value list: `<dateGroupItem>`.
#[derive(Debug, Clone, PartialEq)]
pub struct DateGroup {
    pub year: i32,
    pub month: Option<u32>,
    pub day: Option<u32>,
}

/// What a column is filtered by.
#[derive(Debug, Clone, PartialEq)]
pub enum FilterKind {
    /// The values shown (as displayed, any case), and whether blank cells are.
    Values {
        values: Vec<String>,
        blanks: bool,
        dates: Vec<DateGroup>,
    },
    /// One or two conditions, both (`and`) or either.
    Custom {
        and: bool,
        conditions: Vec<Condition>,
    },
    /// A filter Kantor keeps but doesn't evaluate (top 10, dynamic, colour,
    /// icon): the `<filterColumn>` as the file had it.
    Other(String),
}

/// A filtered column: its offset from the filter's first column.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnFilter {
    pub col: u32,
    pub kind: FilterKind,
}

/// A table's or a sheet's filter: where (header row and data) and the
/// filtered columns.
#[derive(Debug, Clone, PartialEq)]
pub struct FilterInfo {
    pub scope: FilterScope,
    /// The header row and the data rows under it.
    pub area: CellRange,
    pub columns: Vec<ColumnFilter>,
}

impl FilterInfo {
    /// Whether column `col` (absolute) is filtered.
    pub fn is_filtered(&self, col: u32) -> bool {
        col >= self.area.left && self.columns.iter().any(|c| c.col == col - self.area.left)
    }

    /// The filter of column `col` (absolute).
    pub fn column(&self, col: u32) -> Option<&FilterKind> {
        let offset = col.checked_sub(self.area.left)?;
        self.columns
            .iter()
            .find(|c| c.col == offset)
            .map(|c| &c.kind)
    }
}

/// What a column mostly holds: which conditions its filter menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnKind {
    Number,
    Date,
    Text,
}

/// The date serial of `text` (`2025-03-01`, `2025/3/1`, `1/3/2025` as day
/// first), for date conditions; a number is taken as a serial.
pub fn date_serial(text: &str) -> Option<f64> {
    let t = text.trim();
    if let Ok(n) = t.parse::<f64>() {
        return Some(n);
    }
    let parts: Vec<i64> = t
        .split(['-', '/', '.'])
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    let (y, m, d) = match parts.as_slice() {
        [y, m, d] if *y > 31 => (*y, *m, *d),
        [d, m, y] if *y > 31 => (*y, *m, *d),
        _ => return None,
    };
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    // Howard Hinnant's days_from_civil, then Excel's offset.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some((days + 25569) as f64)
}

/// A value of a column's list (the filter menu's checkboxes).
#[derive(Debug, Clone, PartialEq)]
pub struct FilterValue {
    /// As displayed; empty for "(Blanks)".
    pub text: String,
    /// Shown by the column's current filter.
    pub checked: bool,
}

// ---------------------------------------------------------------- XML

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Read an `<autoFilter>` element's filter columns.
pub(crate) fn parse_columns(xml: &str) -> Vec<ColumnFilter> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    doc.root_element()
        .children()
        .filter(|n| n.tag_name().name() == "filterColumn")
        .filter_map(|n| {
            let col = n.attribute("colId")?.parse().ok()?;
            Some(ColumnFilter {
                col,
                kind: parse_kind(n, xml),
            })
        })
        .collect()
}

fn parse_kind(n: Node, xml: &str) -> FilterKind {
    let other = || FilterKind::Other(xml[n.range()].to_owned());
    let child = n.children().find(|c| c.is_element());
    match child.map(|c| c.tag_name().name()) {
        Some("filters") => {
            let f = child.expect("checked");
            let mut values = Vec::new();
            let mut dates = Vec::new();
            for c in f.children().filter(|c| c.is_element()) {
                match c.tag_name().name() {
                    "filter" => values.push(c.attribute("val").unwrap_or_default().to_owned()),
                    "dateGroupItem" => {
                        let Some(year) = c.attribute("year").and_then(|y| y.parse().ok()) else {
                            return other();
                        };
                        let grouping = c.attribute("dateTimeGrouping").unwrap_or("year");
                        let month = c.attribute("month").and_then(|m| m.parse().ok());
                        let day = c.attribute("day").and_then(|d| d.parse().ok());
                        let (month, day) = match grouping {
                            "year" => (None, None),
                            "month" => (month, None),
                            "day" => (month, day),
                            // Hours, minutes, seconds: not evaluated.
                            _ => return other(),
                        };
                        dates.push(DateGroup { year, month, day });
                    }
                    _ => return other(),
                }
            }
            FilterKind::Values {
                values,
                blanks: f
                    .attribute("blank")
                    .is_some_and(|b| b == "1" || b == "true"),
                dates,
            }
        }
        Some("customFilters") => {
            let f = child.expect("checked");
            let conditions = f
                .children()
                .filter(|c| c.tag_name().name() == "customFilter")
                .map(|c| Condition {
                    op: FilterOp::from_ooxml(c.attribute("operator").unwrap_or("equal")),
                    value: c.attribute("val").unwrap_or_default().to_owned(),
                })
                .collect();
            FilterKind::Custom {
                and: f.attribute("and").is_some_and(|a| a == "1" || a == "true"),
                conditions,
            }
        }
        _ => other(),
    }
}

fn column_xml(c: &ColumnFilter) -> String {
    match &c.kind {
        FilterKind::Other(xml) => xml.clone(),
        FilterKind::Values {
            values,
            blanks,
            dates,
        } => {
            let mut s = format!("<filterColumn colId=\"{}\"><filters", c.col);
            if *blanks {
                s.push_str(" blank=\"1\"");
            }
            s.push('>');
            for v in values {
                s.push_str(&format!("<filter val=\"{}\"/>", esc(v)));
            }
            for d in dates {
                let grouping = match (d.month, d.day) {
                    (Some(_), Some(_)) => "day",
                    (Some(_), None) => "month",
                    _ => "year",
                };
                s.push_str(&format!("<dateGroupItem year=\"{}\"", d.year));
                if let Some(m) = d.month {
                    s.push_str(&format!(" month=\"{m}\""));
                }
                if let Some(day) = d.day {
                    s.push_str(&format!(" day=\"{day}\""));
                }
                s.push_str(&format!(" dateTimeGrouping=\"{grouping}\"/>"));
            }
            s.push_str("</filters></filterColumn>");
            s
        }
        FilterKind::Custom { and, conditions } => {
            let mut s = format!("<filterColumn colId=\"{}\"><customFilters", c.col);
            if *and {
                s.push_str(" and=\"1\"");
            }
            s.push('>');
            for cond in conditions {
                s.push_str(&format!(
                    "<customFilter operator=\"{}\" val=\"{}\"/>",
                    cond.op.ooxml(),
                    esc(&cond.value)
                ));
            }
            s.push_str("</customFilters></filterColumn>");
            s
        }
    }
}

/// An `<autoFilter>` element.
pub(crate) fn to_xml(reference: &str, columns: &[ColumnFilter]) -> String {
    if columns.is_empty() {
        return format!("<autoFilter ref=\"{reference}\"/>");
    }
    let mut cols: Vec<&ColumnFilter> = columns.iter().collect();
    cols.sort_by_key(|c| c.col);
    let body: String = cols.into_iter().map(column_xml).collect();
    format!("<autoFilter ref=\"{reference}\">{body}</autoFilter>")
}

// ---------------------------------------------------------------- matching

/// Excel's `*`/`?` wildcards (`~` escapes), any case.
pub(crate) fn wildcard_match(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let t: Vec<char> = text.to_lowercase().chars().collect();
    fn go(p: &[char], t: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => (0..=t.len()).any(|i| go(&p[1..], &t[i..])),
            Some('?') => !t.is_empty() && go(&p[1..], &t[1..]),
            Some('~') if p.len() > 1 => !t.is_empty() && t[0] == p[1] && go(&p[2..], &t[1..]),
            Some(c) => !t.is_empty() && t[0] == *c && go(&p[1..], &t[1..]),
        }
    }
    go(&p, &t)
}

/// `(year, month, day)` of an Excel date serial (1900 date system).
pub fn ymd(serial: f64) -> (i32, u32, u32) {
    // Days from 1970-01-01; Excel's day 25569 is 1970-01-01 (serials above
    // 60 count the phantom 1900-02-29).
    let days = serial.floor() as i64 - 25569;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((y + i64::from(m <= 2)) as i32, m, d)
}

/// What a filter looks at in a cell.
pub(crate) struct Probe {
    pub value: CellValue,
    pub text: String,
    pub is_date: bool,
}

fn condition_matches(c: &Condition, p: &Probe) -> bool {
    let blank = matches!(p.value, CellValue::Blank);
    // Excel writes "non blanks" as `notEqual " "`.
    if c.value == " " || c.value.is_empty() {
        return match c.op {
            FilterOp::Equal => blank,
            FilterOp::NotEqual => !blank,
            _ => false,
        };
    }
    let wanted_number = c.value.trim().parse::<f64>().ok();
    if let (CellValue::Number(n), Some(w)) = (&p.value, wanted_number) {
        let (n, w) = (*n, w);
        return match c.op {
            FilterOp::Equal => (n - w).abs() < 1e-9,
            FilterOp::NotEqual => (n - w).abs() >= 1e-9,
            FilterOp::GreaterThan => n > w,
            FilterOp::GreaterThanOrEqual => n >= w,
            FilterOp::LessThan => n < w,
            FilterOp::LessThanOrEqual => n <= w,
        };
    }
    if blank {
        return c.op == FilterOp::NotEqual;
    }
    let text = p.text.as_str();
    match c.op {
        FilterOp::Equal => wildcard_match(&c.value, text),
        FilterOp::NotEqual => !wildcard_match(&c.value, text),
        // A number compared with text, or text with a number: as Excel, no
        // match for ordering comparisons across kinds.
        _ if wanted_number.is_some() || matches!(p.value, CellValue::Number(_)) => false,
        op => {
            let (a, b) = (text.to_lowercase(), c.value.to_lowercase());
            match op {
                FilterOp::GreaterThan => a > b,
                FilterOp::GreaterThanOrEqual => a >= b,
                FilterOp::LessThan => a < b,
                _ => a <= b,
            }
        }
    }
}

/// Whether a cell passes a column filter. `visible`: the row is shown now
/// (filters Kantor doesn't evaluate keep it as it is).
pub(crate) fn passes(kind: &FilterKind, p: &Probe, visible: bool) -> bool {
    match kind {
        FilterKind::Other(_) => visible,
        FilterKind::Values {
            values,
            blanks,
            dates,
        } => {
            if matches!(p.value, CellValue::Blank) {
                return *blanks;
            }
            if let (true, CellValue::Number(n)) = (p.is_date, &p.value) {
                let (y, m, d) = ymd(*n);
                if dates.iter().any(|g| {
                    g.year == y
                        && g.month.is_none_or(|gm| gm == m)
                        && g.day.is_none_or(|gd| gd == d)
                }) {
                    return true;
                }
            }
            let text = p.text.trim().to_lowercase();
            values.iter().any(|v| v.trim().to_lowercase() == text)
        }
        FilterKind::Custom { and, conditions } => {
            if conditions.is_empty() {
                return true;
            }
            if *and {
                conditions.iter().all(|c| condition_matches(c, p))
            } else {
                conditions.iter().any(|c| condition_matches(c, p))
            }
        }
    }
}

// ---------------------------------------------------------------- workbook

impl Workbook {
    fn probe(&self, sheet: u32, row: u32, col: u32) -> Probe {
        let value = cell_value(self.model.get_model(), sheet, row, col);
        let is_date = matches!(value, CellValue::Number(_))
            && self
                .number_format_by_index(self.style_index(sheet, row, col))
                .is_date();
        let text = match &value {
            CellValue::Blank => String::new(),
            _ => self.display_value(sheet, row, col),
        };
        Probe {
            value,
            text,
            is_date,
        }
    }

    /// The filters of `sheet`: its tables' with filter buttons, and the
    /// sheet's own AutoFilter.
    pub fn filters(&self, sheet: u32) -> Vec<FilterInfo> {
        let model = self.model.get_model();
        let mut out = Vec::new();
        for t in self.tables(sheet) {
            let Some(header) = t.header_row() else {
                continue;
            };
            let Some(xml) = model
                .workbook
                .tables
                .get(&t.name)
                .and_then(|x| x.kantor_auto_filter.as_deref())
            else {
                continue;
            };
            let (_, bottom) = t.data_rows();
            out.push(FilterInfo {
                scope: FilterScope::Table(t.name.clone()),
                area: CellRange {
                    top: header,
                    bottom: bottom.max(header),
                    left: t.area.left,
                    right: t.area.right,
                },
                columns: parse_columns(xml),
            });
        }
        if let Some(info) = self.sheet_filter(sheet) {
            out.push(info);
        }
        out
    }

    fn sheet_filter(&self, sheet: u32) -> Option<FilterInfo> {
        let ws = self.model.get_model().workbook.worksheet(sheet).ok()?;
        let xml = ws.kantor_auto_filter.as_deref()?;
        let area = crate::shift::parse_range(auto_filter_ref(xml)?)?.area();
        Some(FilterInfo {
            scope: FilterScope::Sheet,
            area: CellRange {
                top: area.top,
                bottom: area.bottom,
                left: area.left,
                right: area.right,
            },
            columns: parse_columns(xml),
        })
    }

    /// The filter `scope` of `sheet`.
    pub fn filter(&self, sheet: u32, scope: &FilterScope) -> Option<FilterInfo> {
        self.filters(sheet).into_iter().find(|f| &f.scope == scope)
    }

    /// The filter whose header row has a button at `(row, col)`.
    pub fn filter_button_at(&self, sheet: u32, row: u32, col: u32) -> Option<FilterInfo> {
        self.filters(sheet)
            .into_iter()
            .find(|f| f.area.top == row && (f.area.left..=f.area.right).contains(&col))
    }

    /// The values of column `col` (absolute) for its filter menu: each
    /// distinct displayed value of the rows the *other* columns' filters
    /// let through, sorted like a sort ascending, with "(Blanks)" (empty
    /// text) last; checked if the column's filter shows it.
    pub fn filter_values(&self, sheet: u32, scope: &FilterScope, col: u32) -> Vec<FilterValue> {
        let Some(f) = self.filter(sheet, scope) else {
            return Vec::new();
        };
        let offset = col.saturating_sub(f.area.left);
        let others: Vec<&ColumnFilter> = f.columns.iter().filter(|c| c.col != offset).collect();
        let own = f.columns.iter().find(|c| c.col == offset).map(|c| &c.kind);
        let hidden = self.hidden_rows(sheet);
        let mut seen: HashSet<String> = HashSet::new();
        let mut items: Vec<(CellValue, String, bool)> = Vec::new();
        let mut blanks: Option<bool> = None;
        for row in f.area.top + 1..=f.area.bottom {
            let visible = !hidden.contains(&row);
            let through = others.iter().all(|c| {
                let p = self.probe(sheet, row, f.area.left + c.col);
                passes(&c.kind, &p, visible)
            });
            if !through {
                continue;
            }
            let p = self.probe(sheet, row, col);
            let checked = own.is_none_or(|k| passes(k, &p, visible));
            if matches!(p.value, CellValue::Blank) {
                blanks = Some(blanks.unwrap_or(false) || checked);
                continue;
            }
            let key = p.text.trim().to_lowercase();
            if seen.insert(key) {
                items.push((p.value, p.text, checked));
            }
        }
        items.sort_by(|a, b| crate::sort::compare_values(&a.0, &b.0));
        let mut out: Vec<FilterValue> = items
            .into_iter()
            .map(|(_, text, checked)| FilterValue { text, checked })
            .collect();
        if let Some(checked) = blanks {
            out.push(FilterValue {
                text: String::new(),
                checked,
            });
        }
        out
    }

    /// What column `col` of `scope`'s data mostly holds (its first 200
    /// filled cells): dates, numbers or text.
    pub fn filter_value_kind(&self, sheet: u32, scope: &FilterScope, col: u32) -> ColumnKind {
        let Some(f) = self.filter(sheet, scope) else {
            return ColumnKind::Text;
        };
        let (mut numbers, mut dates, mut texts) = (0, 0, 0);
        for row in f.area.top + 1..=f.area.bottom {
            let p = self.probe(sheet, row, col);
            match p.value {
                CellValue::Blank => continue,
                CellValue::Number(_) if p.is_date => dates += 1,
                CellValue::Number(_) => numbers += 1,
                _ => texts += 1,
            }
            if numbers + dates + texts >= 200 {
                break;
            }
        }
        if dates > numbers && dates >= texts {
            ColumnKind::Date
        } else if numbers >= texts && numbers > 0 {
            ColumnKind::Number
        } else {
            ColumnKind::Text
        }
    }

    /// Filter column `col` (absolute) of `scope` by `kind` (`None` clears
    /// it), and show or hide the rows to match every column's filter. One
    /// undo step. Returns `(rows shown, data rows)`.
    pub fn set_column_filter(
        &mut self,
        sheet: u32,
        scope: &FilterScope,
        col: u32,
        kind: Option<FilterKind>,
    ) -> Result<(usize, usize)> {
        let f = self
            .filter(sheet, scope)
            .ok_or_else(|| anyhow!("no filter here"))?;
        if col < f.area.left || col > f.area.right {
            bail!("column outside the filter");
        }
        let offset = col - f.area.left;
        let mut columns: Vec<ColumnFilter> =
            f.columns.into_iter().filter(|c| c.col != offset).collect();
        if let Some(kind) = kind {
            columns.push(ColumnFilter { col: offset, kind });
        }
        self.apply_filter(sheet, scope, columns)
    }

    /// Clear every column filter of `scope` (the buttons stay).
    pub fn clear_filter(&mut self, sheet: u32, scope: &FilterScope) -> Result<(usize, usize)> {
        self.apply_filter(sheet, scope, Vec::new())
    }

    /// Re-apply `scope`'s filter (Excel's Reapply), e.g. after edits.
    pub fn reapply_filter(&mut self, sheet: u32, scope: &FilterScope) -> Result<(usize, usize)> {
        let f = self
            .filter(sheet, scope)
            .ok_or_else(|| anyhow!("no filter here"))?;
        self.apply_filter(sheet, scope, f.columns)
    }

    /// Set the filter columns of `scope` and hide the rows they filter out.
    fn apply_filter(
        &mut self,
        sheet: u32,
        scope: &FilterScope,
        columns: Vec<ColumnFilter>,
    ) -> Result<(usize, usize)> {
        let mut f = self
            .filter(sheet, scope)
            .ok_or_else(|| anyhow!("no filter here"))?;
        // A sheet filter takes in rows typed under it since (Excel's
        // current region).
        if *scope == FilterScope::Sheet {
            f.area.bottom = self.region_bottom(sheet, f.area);
        }
        let hidden = self.hidden_rows(sheet);
        let mut changes: Vec<(i32, bool)> = Vec::new();
        let mut shown = 0;
        let total = (f.area.bottom - f.area.top) as usize;
        for row in f.area.top + 1..=f.area.bottom {
            let visible = !hidden.contains(&row);
            let pass = columns.iter().all(|c| {
                let p = self.probe(sheet, row, f.area.left + c.col);
                passes(&c.kind, &p, visible)
            });
            if pass {
                shown += 1;
            }
            if pass == visible {
                continue;
            }
            changes.push((row as i32 + 1, !pass));
        }
        let reference = format!(
            "{}:{}",
            cell_name(f.area.top, f.area.left),
            cell_name(f.area.bottom, f.area.right)
        );
        let xml = to_xml(&reference, &columns);
        let active = !columns.is_empty();
        self.model.pause_evaluation();
        let mut steps = 0;
        let result = (|| -> Result<()> {
            match scope {
                FilterScope::Sheet => {
                    let old = self
                        .model
                        .get_model()
                        .workbook
                        .worksheet(sheet)
                        .ok()
                        .and_then(|w| w.kantor_auto_filter.clone());
                    if old.as_deref() != Some(xml.as_str()) {
                        self.model
                            .kantor_set_sheet_filter(sheet, Some(xml.clone()))
                            .map_err(|e| anyhow!("{e}"))?;
                        steps += 1;
                    }
                }
                FilterScope::Table(name) => {
                    let mut t = self
                        .model
                        .get_model()
                        .workbook
                        .tables
                        .get(name)
                        .cloned()
                        .ok_or_else(|| anyhow!("no table {name}"))?;
                    let old = t.clone();
                    t.kantor_auto_filter = Some(xml.clone());
                    t.has_filters = active;
                    if t != old {
                        self.model
                            .kantor_set_table(name, Some(t))
                            .map_err(|e| anyhow!("{e}"))?;
                        steps += 1;
                    }
                }
            }
            if self
                .model
                .kantor_set_rows_hidden(sheet, &changes)
                .map_err(|e| anyhow!("{e}"))?
            {
                steps += 1;
            }
            Ok(())
        })();
        self.end_batch(steps);
        result?;
        Ok((shown, total))
    }

    /// Resume evaluation after a batch of `steps` IronCalc edits and record
    /// them as one undo step.
    pub(crate) fn end_batch(&mut self, steps: u32) {
        if self.calc.mode == CalcMode::Automatic {
            self.model.resume_evaluation();
            if steps > 0 {
                self.model.evaluate();
            }
        }
        if steps > 0 {
            self.history.edited_steps(steps);
        }
    }

    /// The last row of the data region under a sheet filter: its rows, and
    /// rows below that have something in its columns.
    fn region_bottom(&self, sheet: u32, area: CellRange) -> u32 {
        let mut bottom = area.bottom;
        while bottom + 1 < crate::MAX_ROWS
            && (area.left..=area.right).any(|c| self.is_filled(sheet, bottom + 1, c))
        {
            bottom += 1;
        }
        bottom
    }

    /// The data rows (first, last; 0-based) of every filter of `sheet` that
    /// filters something: a hidden row there is filtered out, not hidden by
    /// hand (see `hide.rs`).
    pub(crate) fn filtered_row_spans(&self, sheet: u32) -> Vec<(u32, u32)> {
        self.filters(sheet)
            .into_iter()
            .filter(|f| !f.columns.is_empty())
            .map(|f| {
                let bottom = match f.scope {
                    FilterScope::Sheet => self.region_bottom(sheet, f.area),
                    FilterScope::Table(_) => f.area.bottom,
                };
                (f.area.top + 1, bottom.max(f.area.bottom))
            })
            .filter(|(top, bottom)| top <= bottom)
            .collect()
    }

    /// Rows of `sheet` that are hidden (0-based).
    pub(crate) fn hidden_rows(&self, sheet: u32) -> HashSet<u32> {
        self.model
            .get_model()
            .workbook
            .worksheet(sheet)
            .map(|ws| {
                ws.rows
                    .iter()
                    .filter(|r| r.hidden && r.r >= 1)
                    .map(|r| r.r as u32 - 1)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Turn filter buttons on or off for the selection, like Excel's
    /// Ctrl+Shift+L: in a table, its buttons; elsewhere the sheet's
    /// AutoFilter, over the selection (or the data region around a single
    /// cell). Turning a filter off shows every row it hid. One undo step.
    /// Returns whether filter buttons are now on.
    pub fn toggle_filter(&mut self, sheet: u32, range: CellRange) -> Result<bool> {
        if let Some(t) = self.table_at(sheet, range.top, range.left) {
            let mut table = self
                .model
                .get_model()
                .workbook
                .tables
                .get(&t.name)
                .cloned()
                .ok_or_else(|| anyhow!("no table"))?;
            if t.header_rows == 0 {
                bail!("a table without a header row has no filter buttons");
            }
            let on = table.kantor_auto_filter.is_none();
            let shows = if on {
                Vec::new()
            } else {
                self.rows_to_show(sheet, t.data_range())
            };
            table.kantor_auto_filter = on.then(|| {
                let (_, bottom) = t.data_rows();
                to_xml(
                    &format!(
                        "{}:{}",
                        cell_name(t.area.top, t.area.left),
                        cell_name(bottom.max(t.area.top), t.area.right)
                    ),
                    &[],
                )
            });
            table.has_filters = false;
            self.model.pause_evaluation();
            let mut steps = 0;
            let r = (|| -> Result<()> {
                self.model
                    .kantor_set_table(&t.name, Some(table))
                    .map_err(|e| anyhow!("{e}"))?;
                steps += 1;
                if self
                    .model
                    .kantor_set_rows_hidden(sheet, &shows)
                    .map_err(|e| anyhow!("{e}"))?
                {
                    steps += 1;
                }
                Ok(())
            })();
            self.end_batch(steps);
            r?;
            return Ok(on);
        }
        if let Some(f) = self.sheet_filter(sheet) {
            let shows = self.rows_to_show(
                sheet,
                CellRange {
                    top: f.area.top + 1,
                    ..f.area
                },
            );
            self.model.pause_evaluation();
            let mut steps = 0;
            let r = (|| -> Result<()> {
                self.model
                    .kantor_set_sheet_filter(sheet, None)
                    .map_err(|e| anyhow!("{e}"))?;
                steps += 1;
                if self
                    .model
                    .kantor_set_rows_hidden(sheet, &shows)
                    .map_err(|e| anyhow!("{e}"))?
                {
                    steps += 1;
                }
                Ok(())
            })();
            self.end_batch(steps);
            r?;
            return Ok(false);
        }
        let area = if range.is_single() {
            self.current_region(sheet, range.top, range.left)
        } else {
            let (rows, cols) = self.used_size(sheet);
            range
                .clip(rows.max(1), cols.max(1))
                .ok_or_else(|| anyhow!("nothing to filter"))?
        };
        if area.top == area.bottom && !self.is_filled(sheet, area.top, area.left) {
            bail!("select the data to filter");
        }
        let xml = to_xml(
            &format!(
                "{}:{}",
                cell_name(area.top, area.left),
                cell_name(area.bottom, area.right)
            ),
            &[],
        );
        self.model
            .kantor_set_sheet_filter(sheet, Some(xml))
            .map_err(|e| anyhow!("{e}"))?;
        self.history.edited();
        Ok(true)
    }

    /// Hidden rows of `range` to show again.
    fn rows_to_show(&self, sheet: u32, range: CellRange) -> Vec<(i32, bool)> {
        let hidden = self.hidden_rows(sheet);
        (range.top..=range.bottom)
            .filter(|r| hidden.contains(r))
            .map(|r| (r as i32 + 1, false))
            .collect()
    }

    /// The block of filled cells around `(row, col)`, bounded by empty rows
    /// and columns (Excel's current region, as Ctrl+A grows it).
    pub fn current_region(&self, sheet: u32, row: u32, col: u32) -> CellRange {
        let mut r = CellRange {
            top: row,
            bottom: row,
            left: col,
            right: col,
        };
        loop {
            let mut grown = false;
            let row_filled = |wb: &Self, rr: u32, r: &CellRange| {
                (r.left.saturating_sub(1)..=(r.right + 1).min(crate::MAX_COLS - 1))
                    .any(|c| wb.is_filled(sheet, rr, c))
            };
            let col_filled = |wb: &Self, cc: u32, r: &CellRange| {
                (r.top.saturating_sub(1)..=(r.bottom + 1).min(crate::MAX_ROWS - 1))
                    .any(|rr| wb.is_filled(sheet, rr, cc))
            };
            if r.top > 0 && row_filled(self, r.top - 1, &r) {
                r.top -= 1;
                grown = true;
            }
            if r.bottom + 1 < crate::MAX_ROWS && row_filled(self, r.bottom + 1, &r) {
                r.bottom += 1;
                grown = true;
            }
            if r.left > 0 && col_filled(self, r.left - 1, &r) {
                r.left -= 1;
                grown = true;
            }
            if r.right + 1 < crate::MAX_COLS && col_filled(self, r.right + 1, &r) {
                r.right += 1;
                grown = true;
            }
            if !grown {
                return r;
            }
        }
    }
}

/// Distinct column values, for tests and callers that want the set.
pub fn checked_values(values: &[FilterValue]) -> BTreeSet<String> {
    values
        .iter()
        .filter(|v| v.checked)
        .map(|v| v.text.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_round_trips() {
        let xml = r#"<autoFilter ref="A1:D9"><filterColumn colId="0"><filters blank="1"><filter val="North"/><filter val="A &amp; B"/><dateGroupItem year="2025" month="3" dateTimeGrouping="month"/></filters></filterColumn><filterColumn colId="2"><customFilters and="1"><customFilter operator="greaterThanOrEqual" val="10"/><customFilter operator="lessThan" val="20"/></customFilters></filterColumn><filterColumn colId="3"><top10 val="5"/></filterColumn></autoFilter>"#;
        let cols = parse_columns(xml);
        assert_eq!(cols.len(), 3);
        assert_eq!(
            cols[0].kind,
            FilterKind::Values {
                values: vec!["North".into(), "A & B".into()],
                blanks: true,
                dates: vec![DateGroup {
                    year: 2025,
                    month: Some(3),
                    day: None
                }],
            }
        );
        assert!(matches!(cols[2].kind, FilterKind::Other(_)));
        assert_eq!(to_xml("A1:D9", &cols), xml);
    }

    #[test]
    fn wildcards_and_dates() {
        assert!(wildcard_match("ab*", "ABC"));
        assert!(wildcard_match("*b?", "abc"));
        assert!(!wildcard_match("*x*", "abc"));
        assert!(wildcard_match("a~*", "a*"));
        assert_eq!(ymd(45658.0), (2025, 1, 1));
        assert_eq!(ymd(61.0), (1900, 3, 1));
        assert_eq!(date_serial("2025-01-01"), Some(45658.0));
        assert_eq!(date_serial("1/3/2025"), date_serial("2025-03-01"));
        assert_eq!(date_serial("nope"), None);
    }

    #[test]
    fn conditions() {
        let num = |n: f64| Probe {
            value: CellValue::Number(n),
            text: n.to_string(),
            is_date: false,
        };
        let text = |t: &str| Probe {
            value: CellValue::Text(t.to_lowercase()),
            text: t.into(),
            is_date: false,
        };
        let between = FilterKind::Custom {
            and: true,
            conditions: vec![
                Condition {
                    op: FilterOp::GreaterThanOrEqual,
                    value: "10".into(),
                },
                Condition {
                    op: FilterOp::LessThanOrEqual,
                    value: "20".into(),
                },
            ],
        };
        assert!(passes(&between, &num(10.0), true));
        assert!(!passes(&between, &num(21.0), true));
        assert!(!passes(&between, &text("15"), true));
        let contains = FilterKind::Custom {
            and: false,
            conditions: vec![Condition {
                op: FilterOp::Equal,
                value: "*jawa*".into(),
            }],
        };
        assert!(passes(&contains, &text("Jawa Timur"), true));
        assert!(!passes(&contains, &text("Bali"), true));
        let non_blank = FilterKind::Custom {
            and: false,
            conditions: vec![Condition {
                op: FilterOp::NotEqual,
                value: " ".into(),
            }],
        };
        let blank = Probe {
            value: CellValue::Blank,
            text: String::new(),
            is_date: false,
        };
        assert!(!passes(&non_blank, &blank, true));
        assert!(passes(&non_blank, &text("x"), true));
    }
}
