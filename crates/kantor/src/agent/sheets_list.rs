//! `kantor sheets list` (and the MCP tool `sheets_list`): a workbook as
//! JSON for an agent. Each sheet comes with a summary (used range, frozen
//! panes, merged cells, tables, charts and their series, conditional
//! formats, validation, pivot tables, a profile of each column) and one
//! page of its cells, so big sheets stay readable: the cells of 100,000
//! rows are never written out unless asked for, page by page.

use std::collections::HashMap;

use anyhow::Result;
use serde_json::{Map, Value, json};
use sheets_engine::drawing::{Anchor, DrawingKind};
use sheets_engine::{CellRange, ValueKind, Workbook, cell_name, column_name};

use super::sheets_author::{cell_value, number, parse_ref, px_to_chars, used_part};

/// Rows of cells shown per sheet unless asked for more.
pub const DEFAULT_ROWS: u32 = 30;
/// Rows shown of a range asked for (`--range`) unless asked for more.
pub const DEFAULT_RANGE_ROWS: u32 = 500;
/// The most rows one listing shows of a sheet.
pub const MAX_ROWS: u32 = 5_000;
/// Cells profiled per sheet (the column summaries).
const PROFILE_CELLS: u64 = 1_000_000;

/// What to list.
#[derive(Clone, Debug, Default)]
pub struct ListOpts {
    /// One sheet (name or number from 1).
    pub sheet: Option<String>,
    /// The cells to show (`A1:H200`, `Data!A1:H200`).
    pub range: Option<String>,
    /// Formulas in the grid in place of their values.
    pub formulas: bool,
    /// Rows of cells per sheet (0: none, only the summary).
    pub max_rows: Option<u32>,
}

/// The listing of `book`.
pub fn list(book: &Workbook, opts: &ListOpts) -> Result<Value> {
    sheets_engine::numbers_invariant(|| list_inner(book, opts))
}

fn list_inner(book: &Workbook, opts: &ListOpts) -> Result<Value> {
    let names = book.sheet_names();
    let only = match &opts.sheet {
        Some(s) => {
            let v = match s.parse::<u64>() {
                Ok(n) => json!({"sheet": n}),
                Err(_) => json!({"sheet": s}),
            };
            Some(super::sheets::sheet_index(book, &v, 0)?)
        }
        None => None,
    };
    let range = match &opts.range {
        Some(r) => Some(parse_ref(book, r, only.unwrap_or(book.active_sheet()))?),
        None => None,
    };
    let mut sheets = Vec::new();
    for i in 0..names.len() as u32 {
        let wanted = match (only, range) {
            (Some(s), _) => s == i,
            (None, Some((s, _))) => s == i,
            (None, None) => true,
        };
        if !wanted {
            continue;
        }
        let page = range.filter(|(s, _)| *s == i).map(|(_, r)| r);
        sheets.push(sheet_json(book, i, page, opts));
    }
    let names_json: Vec<Value> = book
        .defined_names()
        .into_iter()
        .filter(|n| !n.name.starts_with("_xlnm._FilterDatabase"))
        .map(|n| {
            let mut v = json!({"name": n.name, "refers_to": n.formula});
            if let Some(s) = n.scope {
                v["sheet"] = json!(names.get(s as usize));
            }
            v
        })
        .collect();
    Ok(json!({
        "sheets": sheets,
        "names": names_json,
        "active_sheet": names.get(book.active_sheet() as usize),
    }))
}

fn sheet_json(book: &Workbook, s: u32, page: Option<CellRange>, opts: &ListOpts) -> Value {
    let name = book.sheet_names()[s as usize].clone();
    let (rows, cols) = book.used_size(s);
    let empty = rows == 0 || cols == 0 || book.filled_cells(s).is_empty();
    let used = (!empty).then(|| CellRange::from_corners((0, 0), (rows - 1, cols - 1)));
    let (fr, fc) = book.frozen(s);
    let mut v = json!({
        "number": s + 1,
        "name": name,
        "hidden": book.sheet_hidden(s),
        "used_range": used.map(|u| u.name()),
        "rows": rows,
        "columns": cols,
        "frozen": if fr == 0 && fc == 0 { Value::Null } else { json!(cell_name(fr, fc)) },
    });
    if !book.show_gridlines(s) {
        v["show_gridlines"] = json!(false);
    }
    let Some(used) = used else {
        return v;
    };
    let merged: Vec<String> = book
        .merges_in(s, used)
        .iter()
        .take(200)
        .map(CellRange::name)
        .collect();
    if !merged.is_empty() {
        v["merged"] = json!(merged);
    }
    let hidden_rows: Vec<u32> = (0..rows).filter(|&r| book.is_row_hidden(s, r)).collect();
    if !hidden_rows.is_empty() {
        v["hidden_rows"] = json!({"count": hidden_rows.len(), "first": hidden_rows.iter().take(20).map(|r| r + 1).collect::<Vec<_>>()});
    }
    let hidden_cols: Vec<String> = (0..cols)
        .filter(|&c| book.is_column_hidden(s, c))
        .map(column_name)
        .collect();
    if !hidden_cols.is_empty() {
        v["hidden_columns"] = json!(hidden_cols);
    }
    let widths: Map<String, Value> = (0..cols.min(100))
        .filter_map(|c| {
            let w = px_to_chars(book, s, book.column_width(s, c));
            let default = px_to_chars(book, s, book.column_scale(s).default_px);
            ((w - default).abs() > 0.05 && w > 0.0).then(|| (column_name(c), json!(w)))
        })
        .collect();
    if !widths.is_empty() {
        v["column_widths"] = json!(widths);
    }
    let tables: Vec<Value> = book
        .tables(s)
        .iter()
        .map(|t| {
            json!({"name": t.name, "range": t.area.name(), "columns": t.columns,
                   "style": t.style, "total_row": t.totals_rows > 0})
        })
        .collect();
    if !tables.is_empty() {
        v["tables"] = json!(tables);
    }
    let charts = charts(book, s);
    if !charts.is_empty() {
        v["charts"] = json!(charts);
    }
    let cf: Vec<Value> = book
        .cf_rules(s, None)
        .iter()
        .map(|r| json!({"range": r.range, "rule": r.summary}))
        .collect();
    if !cf.is_empty() {
        v["conditional_formats"] = json!(cf);
    }
    let dv: Vec<Value> = book
        .sheet_validations(s)
        .iter()
        .map(|d| {
            let mut x = json!({
                "range": d.ranges.iter().map(CellRange::name).collect::<Vec<_>>().join(" "),
                "type": format!("{:?}", d.kind).to_lowercase(),
            });
            if let Some(f) = &d.formula1 {
                x["formula1"] = json!(f);
            }
            if let Some(f) = &d.formula2 {
                x["formula2"] = json!(f);
            }
            x
        })
        .collect();
    if !dv.is_empty() {
        v["validations"] = json!(dv);
    }
    let pivots: Vec<Value> = book
        .pivots(s)
        .iter()
        .map(|p| json!({"name": p.name, "range": p.area.name()}))
        .collect();
    if !pivots.is_empty() {
        v["pivots"] = json!(pivots);
    }
    v["column_summary"] = json!(profile(book, s, used));
    let default = if page.is_some() {
        DEFAULT_RANGE_ROWS
    } else {
        DEFAULT_ROWS
    };
    let max_rows = opts.max_rows.unwrap_or(default).min(MAX_ROWS);
    if max_rows > 0 {
        let want = page.map_or(used, |p| used_part(book, s, p));
        v["cells"] = cells(book, s, want, max_rows, opts.formulas);
    }
    v
}

/// A cell range's name from an anchor (the cells it covers).
fn anchor_cells(a: &Anchor) -> Option<String> {
    anchor_range(a).map(|r| r.name())
}

/// The cells an object covers (a cell it only touches at its edge left
/// out); `None` when it is placed by position.
pub fn anchor_range(a: &Anchor) -> Option<CellRange> {
    match a {
        Anchor::TwoCell { from, to, .. } => {
            let r = to
                .row
                .saturating_sub(u32::from(to.row_off == 0))
                .max(from.row);
            let c = to
                .col
                .saturating_sub(u32::from(to.col_off == 0))
                .max(from.col);
            Some(CellRange::from_corners((from.row, from.col), (r, c)))
        }
        Anchor::OneCell { from, .. } => Some(CellRange::from_corners(
            (from.row, from.col),
            (from.row, from.col),
        )),
        Anchor::Absolute { .. } => None,
    }
}

/// A chart part's series: name, categories and values (references or
/// the text cached in the part).
pub fn chart_series(xml: &str) -> Vec<Value> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let f_or_v = |n: Option<roxmltree::Node>| -> Value {
        let Some(n) = n else {
            return Value::Null;
        };
        if let Some(f) = n.descendants().find(|d| d.has_tag_name((C_NS, "f"))) {
            return json!(f.text().unwrap_or_default());
        }
        match n.descendants().find(|d| d.has_tag_name((C_NS, "v"))) {
            Some(v) => json!(v.text().unwrap_or_default()),
            None => Value::Null,
        }
    };
    doc.descendants()
        .filter(|n| n.has_tag_name((C_NS, "ser")))
        .map(|ser| {
            let kid = |name: &str| ser.children().find(|c| c.has_tag_name((C_NS, name)));
            let plot = ser
                .parent()
                .map(|p| p.tag_name().name().to_owned())
                .unwrap_or_default();
            json!({
                "name": f_or_v(kid("tx")),
                "categories": f_or_v(kid("cat").or_else(|| kid("xVal"))),
                "values": f_or_v(kid("val").or_else(|| kid("yVal"))),
                "plot": plot.trim_end_matches("Chart"),
            })
        })
        .collect()
}

const C_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";

fn charts(book: &Workbook, s: u32) -> Vec<Value> {
    let mut out = Vec::new();
    for d in book.drawings(s) {
        let DrawingKind::Chart { xml, .. } = d.kind.as_ref() else {
            continue;
        };
        let mut v = json!({"name": d.name, "at": anchor_cells(&d.anchor)});
        if let Some(info) = book.chart_info(d.key) {
            v["type"] = json!(info.kind_label);
            if !info.spec.title.is_empty() {
                v["title"] = json!(info.spec.title);
            }
        }
        v["series"] = json!(chart_series(xml));
        out.push(v);
    }
    let pictures = book
        .drawings(s)
        .iter()
        .filter(|d| !matches!(d.kind.as_ref(), DrawingKind::Chart { .. }))
        .count();
    if pictures > 0 {
        out.push(json!({"other_objects": pictures}));
    }
    out
}

/// What each column of `used` holds.
fn profile(book: &Workbook, s: u32, used: CellRange) -> Vec<Value> {
    let cols = used.cols_count().min(200);
    let rows_scanned = ((PROFILE_CELLS / u64::from(cols.max(1))) as u32).min(used.rows_count());
    let mut out = Vec::new();
    for c in used.left..used.left + cols {
        let (mut nums, mut texts, mut dates, mut formulas, mut blanks, mut errors, mut bools) =
            (0u32, 0u32, 0u32, 0u32, 0u32, 0u32, 0u32);
        let (mut min, mut max) = (f64::INFINITY, f64::NEG_INFINITY);
        let mut fmts: HashMap<String, u32> = HashMap::new();
        let mut header = None;
        for r in used.top..used.top + rows_scanned {
            let input = book.input(s, r, c);
            if input.is_empty() {
                blanks += 1;
                continue;
            }
            if input.starts_with('=') {
                formulas += 1;
            }
            match book.value_kind(s, r, c) {
                ValueKind::Number => {
                    let st = book.cell_style(s, r, c);
                    if r > used.top || header.is_some() {
                        *fmts.entry(st.num_fmt.clone()).or_default() += 1;
                    }
                    if book.is_date_format(s, r, c) {
                        dates += 1;
                    } else {
                        nums += 1;
                    }
                    if let Some((n, _)) = book.number_at(s, r, c) {
                        min = min.min(n);
                        max = max.max(n);
                    }
                }
                ValueKind::Text => {
                    if r == used.top {
                        header = Some(book.display_value(s, r, c));
                    } else {
                        texts += 1;
                    }
                }
                ValueKind::Boolean => bools += 1,
                ValueKind::Error => errors += 1,
            }
        }
        let mut v = json!({"column": column_name(c)});
        if let Some(h) = header {
            v["header"] = json!(h);
        }
        for (k, n) in [
            ("numbers", nums),
            ("dates", dates),
            ("text", texts),
            ("formulas", formulas),
            ("booleans", bools),
            ("errors", errors),
            ("blank", blanks),
        ] {
            if n > 0 {
                v[k] = json!(n);
            }
        }
        if min.is_finite() {
            v["min"] = number(min);
            v["max"] = number(max);
        }
        if let Some((f, _)) = fmts.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))) {
            v["number_format"] = json!(f);
            if fmts.len() > 1 {
                v["number_formats"] = json!(fmts.len());
            }
        }
        out.push(v);
    }
    if rows_scanned < used.rows_count() {
        out.push(json!({"rows_profiled": rows_scanned}));
    }
    out
}

/// A page of cells: values (or formulas) row by row, the formulas and
/// number formats beside them.
fn cells(book: &Workbook, s: u32, want: CellRange, max_rows: u32, show_formulas: bool) -> Value {
    let cols = want.cols_count().min(200);
    let shown = CellRange {
        bottom: want.bottom.min(want.top + max_rows - 1),
        right: want.left + cols - 1,
        ..want
    };
    let mut values = Vec::new();
    let mut formulas = Map::new();
    // Number formats in vertical runs: {"B2:B31": "#,##0"}.
    let mut formats = Map::new();
    for c in shown.left..=shown.right {
        let mut run: Option<(u32, String)> = None;
        let mut close = |run: &mut Option<(u32, String)>, end: u32| {
            if let Some((start, f)) = run.take() {
                let name = CellRange::from_corners((start, c), (end, c)).name();
                formats.insert(name, json!(f));
            }
        };
        for r in shown.top..=shown.bottom {
            let f = if book.input(s, r, c).is_empty() {
                String::new()
            } else {
                book.cell_style(s, r, c).num_fmt.clone()
            };
            let f = if f.eq_ignore_ascii_case("general") {
                String::new()
            } else {
                f
            };
            let same = matches!(&run, Some((_, cur)) if *cur == f);
            if !same {
                close(&mut run, r.saturating_sub(1));
                if !f.is_empty() {
                    run = Some((r, f));
                }
            }
        }
        close(&mut run, shown.bottom);
    }
    for r in shown.top..=shown.bottom {
        let mut row = Vec::new();
        for c in shown.left..=shown.right {
            let input = book.input(s, r, c);
            if input.starts_with('=') {
                if show_formulas {
                    row.push(json!(input));
                    continue;
                }
                formulas.insert(cell_name(r, c), json!(input));
            }
            row.push(cell_value(book, s, r, c));
        }
        // Trailing nulls are left out.
        while row.last().is_some_and(Value::is_null) {
            row.pop();
        }
        values.push(Value::Array(row));
    }
    let mut v = json!({"range": shown.name(), "values": values});
    if !formulas.is_empty() {
        v["formulas"] = Value::Object(formulas);
    }
    if !formats.is_empty() {
        v["number_formats"] = Value::Object(formats);
    }
    if shown.bottom < want.bottom || shown.right < want.right {
        v["truncated"] = json!(true);
        let mut note = format!("cut: shows {} of {}", shown.name(), want.name());
        if shown.bottom < want.bottom {
            let next = CellRange {
                top: shown.bottom + 1,
                ..want
            }
            .name();
            note.push_str(&format!(
                "; list --range {next} (or --max-rows) for the rest"
            ));
            v["next"] = json!(next);
        }
        v["note"] = json!(note);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::sheets_author::build;

    #[std::prelude::v1::test]
    fn lists_sheets_with_summaries_and_pages() {
        let mut rows = vec![json!(["Date", "Item", "Amount"])];
        for i in 0..100 {
            rows.push(json!([
                format!("2026-01-{:02}", i % 28 + 1),
                format!("Item {i}"),
                i * 10
            ]));
        }
        let b = build(&json!({"sheets": [
            {"name": "Data", "rows": rows, "cells": {"E1": "=SUM(C2:C101)"},
             "formats": [{"range": "C2:C101", "number_format": "integer"}],
             "freeze": "A2", "tables": [{"range": "A1:C101", "name": "T"}],
             "conditional_formats": [{"range": "C2:C101", "type": "data_bar"}],
             "charts": [{"chart": "line", "data": "B1:C11", "at": "G2", "to": "N16", "title": "Amounts"}]},
            {"name": "Empty"}
        ]}), None)
        .unwrap();
        let v = list(&b, &ListOpts::default()).unwrap();
        let d = &v["sheets"][0];
        assert_eq!(d["used_range"], "A1:E101");
        assert_eq!(d["frozen"], "A2");
        assert_eq!(d["tables"][0]["name"], "T");
        assert_eq!(d["conditional_formats"][0]["range"], "C2:C101");
        let chart = &d["charts"][0];
        assert_eq!(chart["at"], "G2:N16");
        // Built charts have Excel's white chart area (readable on a dark grid).
        let bytes = crate::agent::sheets_author::to_bytes(&b).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut part = String::new();
        std::io::Read::read_to_string(&mut zip.by_name("xl/charts/chart1.xml").unwrap(), &mut part)
            .unwrap();
        assert!(
            part.contains(&format!(
                "</c:chart><c:spPr>{}</c:spPr>",
                sheets_engine::CHART_AREA_FILL
            )),
            "{part}"
        );
        assert_eq!(chart["series"][0]["values"], "Data!$C$2:$C$11");
        assert_eq!(chart["series"][0]["categories"], "Data!$B$2:$B$11");
        assert_eq!(chart["series"][0]["name"], "Data!$C$1");
        let cells = &d["cells"];
        assert_eq!(cells["range"], "A1:E30");
        assert_eq!(cells["next"], "A31:E101");
        assert_eq!(cells["formulas"]["E1"], "=SUM(C2:C101)");
        assert_eq!(cells["values"][0][4], json!(49500));
        assert_eq!(cells["number_formats"]["C2:C30"], "#,##0");
        let prof = &d["column_summary"];
        assert_eq!(prof[0]["header"], "Date");
        assert_eq!(prof[0]["dates"], 100);
        assert_eq!(prof[2]["max"], json!(990));
        assert_eq!(v["sheets"][1]["used_range"], Value::Null);
        // A page of another range, with formulas shown.
        let v = list(
            &b,
            &ListOpts {
                range: Some("Data!C99:E101".into()),
                formulas: true,
                max_rows: Some(2),
                ..ListOpts::default()
            },
        )
        .unwrap();
        assert_eq!(v["sheets"].as_array().unwrap().len(), 1);
        assert_eq!(v["sheets"][0]["cells"]["range"], "C99:E100");
        assert_eq!(v["sheets"][0]["cells"]["next"], "C101:E101");
        assert_eq!(v["sheets"][0]["cells"]["truncated"], true);
        assert!(
            v["sheets"][0]["cells"]["note"]
                .as_str()
                .unwrap()
                .contains("C101:E101")
        );
        // A range asked for shows all of it (up to 500 rows), not 30.
        let v = list(
            &b,
            &ListOpts {
                range: Some("Data!A31:E101".into()),
                ..ListOpts::default()
            },
        )
        .unwrap();
        let cells = &v["sheets"][0]["cells"];
        assert_eq!(cells["range"], "A31:E101");
        assert_eq!(cells["values"].as_array().unwrap().len(), 71);
        assert!(cells.get("truncated").is_none() && cells.get("next").is_none());
        let v = list(
            &b,
            &ListOpts {
                sheet: Some("2".into()),
                max_rows: Some(0),
                ..ListOpts::default()
            },
        )
        .unwrap();
        assert_eq!(v["sheets"][0]["name"], "Empty");
    }
}
