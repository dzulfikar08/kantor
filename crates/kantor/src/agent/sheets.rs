//! Sheets tools on a [`Workbook`]. Rows and columns are 0-based inside,
//! A1-style or 1-based in the tools' arguments and answers.

use anyhow::{Result, bail};
use serde_json::{Value, json};
use sheets_engine::formula::parse_range;
use sheets_engine::{
    BorderPreset, CellRange, HAlign, MAX_COLS, MAX_ROWS, NumberFormat, Rgb, StyleChange, VAlign,
    Workbook, cell_name, column_name,
};

use super::{check_len, opt_bool, opt_count, opt_str, req_str};

/// Cells one `sheets_write_range` may write.
pub const MAX_WRITE: usize = 10_000;
const READ_DEFAULT: u64 = 2_000;
const READ_MAX: u64 = 20_000;

/// The sheet `args.sheet` names (a name, or a number from 1); `shown` if absent.
pub fn sheet_index(book: &Workbook, args: &Value, shown: u32) -> Result<u32> {
    let names = book.sheet_names();
    match args.get("sheet") {
        None | Some(Value::Null) => Ok(shown),
        Some(Value::Number(n)) => match n.as_u64() {
            Some(i) if i >= 1 && (i as usize) <= names.len() => Ok(i as u32 - 1),
            _ => bail!("there is no sheet {n}; the workbook has {}", names.len()),
        },
        Some(Value::String(name)) => names
            .iter()
            .position(|n| n == name)
            .or_else(|| names.iter().position(|n| n.eq_ignore_ascii_case(name)))
            .map(|i| i as u32)
            .ok_or_else(|| {
                anyhow::anyhow!("there is no sheet \"{name}\"; sheets: {}", names.join(", "))
            }),
        Some(_) => bail!("`sheet` must be a sheet name or number"),
    }
}

/// A range argument (`A1:D20`, `B7`).
pub fn range_arg(args: &Value, key: &str) -> Result<CellRange> {
    let text = req_str(args, key)?;
    parse_range(text).ok_or_else(|| {
        anyhow::anyhow!("`{key}`: \"{text}\" is not a cell or range like B7 or A1:D20")
    })
}

/// A column letter (`C`), 0-based.
pub fn column_arg(args: &Value, key: &str) -> Result<Option<u32>> {
    let Some(text) = opt_str(args, key)? else {
        return Ok(None);
    };
    let letters = text.trim();
    if letters.is_empty() || !letters.chars().all(|c| c.is_ascii_alphabetic()) {
        bail!("`{key}`: \"{text}\" is not a column letter like C");
    }
    match parse_range(&format!("{letters}1")) {
        Some(r) if r.is_single() => Ok(Some(r.left)),
        _ => bail!("`{key}`: \"{text}\" is not a column letter like C"),
    }
}

/// A row number from 1, 0-based.
pub fn row_arg(args: &Value, key: &str) -> Result<Option<u32>> {
    match opt_count(args, key, 1)? {
        Some(n) if n > u64::from(MAX_ROWS) => bail!("`{key}` is past the last row"),
        other => Ok(other.map(|n| n as u32 - 1)),
    }
}

pub fn list_sheets(book: &Workbook, shown: u32) -> Value {
    let sheets: Vec<Value> = book
        .sheet_names()
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let (rows, cols) = book.used_size(i as u32);
            let used = if rows == 0 || cols == 0 {
                Value::Null
            } else {
                json!(CellRange::from_corners((0, 0), (rows - 1, cols - 1)).name())
            };
            json!({"number": i + 1, "name": name, "used_range": used, "on_screen": i as u32 == shown})
        })
        .collect();
    json!({"sheets": sheets})
}

pub fn read_range(book: &Workbook, sheet: u32, args: &Value) -> Result<Value> {
    let range = match opt_str(args, "range")? {
        Some(_) => range_arg(args, "range")?,
        None => {
            let (rows, cols) = book.used_size(sheet);
            CellRange::from_corners((0, 0), (rows.max(1) - 1, cols.max(1) - 1))
        }
    };
    let include = opt_str(args, "include")?.unwrap_or("both");
    let (values, inputs) = match include {
        "values" => (true, false),
        "inputs" => (false, true),
        "both" => (true, true),
        other => bail!("`include` must be values, inputs or both, not \"{other}\""),
    };
    let max = opt_count(args, "max_cells", 1)?
        .unwrap_or(READ_DEFAULT)
        .min(READ_MAX);
    let width = u64::from(range.cols_count());
    let rows_fit = (max / width) as u32;
    if rows_fit == 0 {
        bail!(
            "{} is {width} columns wide, more than max_cells ({max}) allows for one row",
            range.name()
        );
    }
    let shown = CellRange {
        bottom: range.bottom.min(range.top + rows_fit - 1),
        ..range
    };
    let grid = |f: &dyn Fn(u32, u32) -> String| -> Vec<Vec<String>> {
        (shown.top..=shown.bottom)
            .map(|r| (shown.left..=shown.right).map(|c| f(r, c)).collect())
            .collect()
    };
    let mut out = json!({
        "sheet": book.sheet_names().get(sheet as usize).cloned().unwrap_or_default(),
        "range": shown.name(),
    });
    if values {
        out["values"] = json!(grid(&|r, c| book.display_value(sheet, r, c)));
    }
    if inputs {
        out["inputs"] = json!(grid(&|r, c| book.input(sheet, r, c)));
    }
    if shown.bottom < range.bottom {
        out["truncated"] = json!(true);
        out["next_range"] = json!(
            CellRange {
                top: shown.bottom + 1,
                ..range
            }
            .name()
        );
    }
    Ok(out)
}

/// Cells to write and the range they cover.
#[derive(Debug, PartialEq)]
pub struct WritePlan {
    pub range: CellRange,
    pub cells: Vec<(u32, u32, String)>,
}

fn cell_text(v: &Value) -> Result<Option<String>> {
    Ok(match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "TRUE" } else { "FALSE" }.to_owned()),
        _ => bail!("cells must be text, numbers, booleans or null, not {v}"),
    })
}

pub fn plan_write(args: &Value) -> Result<WritePlan> {
    let start = range_arg(args, "top_left")?;
    let Some(rows) = args.get("values").and_then(Value::as_array) else {
        bail!("`values` must be an array of rows");
    };
    let mut cells = Vec::new();
    let mut width = 0;
    for (r, row) in rows.iter().enumerate() {
        let Some(row) = row.as_array() else {
            bail!("row {} of `values` is not an array", r + 1);
        };
        width = width.max(row.len());
        for (c, v) in row.iter().enumerate() {
            let Some(text) = cell_text(v)? else {
                continue;
            };
            check_len("a cell", &text)?;
            cells.push((start.top as usize + r, start.left as usize + c, text));
            if cells.len() > MAX_WRITE {
                bail!("more than {MAX_WRITE} cells in one call; write in parts");
            }
        }
    }
    if rows.is_empty() || width == 0 {
        bail!("`values` has no cells");
    }
    let bottom = start.top as usize + rows.len() - 1;
    let right = start.left as usize + width - 1;
    if bottom >= MAX_ROWS as usize || right >= MAX_COLS as usize {
        bail!("the values go past the sheet's last row or column");
    }
    Ok(WritePlan {
        range: CellRange::from_corners((start.top, start.left), (bottom as u32, right as u32)),
        cells: cells
            .into_iter()
            .map(|(r, c, t)| (r as u32, c as u32, t))
            .collect(),
    })
}

/// A `sheets_format` request: style changes, then borders, then a merge or
/// unmerge (`Some(true)` / `Some(false)`).
pub struct FormatPlan {
    pub range: CellRange,
    pub changes: Vec<StyleChange>,
    pub borders: Option<BorderPreset>,
    pub merge: Option<bool>,
}

/// `#RRGGBB` (or `RRGGBB`), or `none` / `auto` for no colour.
fn color_arg(args: &Value, key: &str) -> Result<Option<Option<Rgb>>> {
    let Some(v) = opt_str(args, key)? else {
        return Ok(None);
    };
    if v.eq_ignore_ascii_case("none") || v.eq_ignore_ascii_case("auto") {
        return Ok(Some(None));
    }
    let hex = v.strip_prefix('#').unwrap_or(v);
    match (hex.len(), u32::from_str_radix(hex, 16)) {
        (6, Ok(c)) => Ok(Some(Some(c))),
        _ => bail!("`{key}` must be #RRGGBB, none or auto"),
    }
}

pub fn plan_format(args: &Value) -> Result<FormatPlan> {
    let range = range_arg(args, "range")?;
    let mut changes = Vec::new();
    for (key, make) in [
        ("bold", StyleChange::Bold as fn(bool) -> StyleChange),
        ("italic", StyleChange::Italic),
        ("underline", StyleChange::Underline),
        ("strikethrough", StyleChange::Strikethrough),
        ("wrap", StyleChange::Wrap),
    ] {
        if let Some(on) = opt_bool(args, key)? {
            changes.push(make(on));
        }
    }
    if let Some(a) = opt_str(args, "align")? {
        changes.push(StyleChange::Align(match a {
            "general" => HAlign::General,
            "left" => HAlign::Left,
            "center" => HAlign::Center,
            "right" => HAlign::Right,
            _ => bail!("`align` must be general, left, center or right"),
        }));
    }
    if let Some(f) = opt_str(args, "number_format")? {
        changes.push(StyleChange::NumberFormat(match f {
            "general" => NumberFormat::General,
            "number" => NumberFormat::Number,
            "percent" => NumberFormat::Percent,
            "rupiah" => NumberFormat::CurrencyIdr,
            "dollar" => NumberFormat::CurrencyUsd,
            "date" => NumberFormat::Date,
            _ => bail!("`number_format` must be general, number, percent, rupiah, dollar or date"),
        }));
    }
    if let Some(a) = opt_str(args, "valign")? {
        changes.push(StyleChange::VAlign(match a {
            "top" => VAlign::Top,
            "center" => VAlign::Center,
            "bottom" => VAlign::Bottom,
            _ => bail!("`valign` must be top, center or bottom"),
        }));
    }
    if let Some(c) = color_arg(args, "fill")? {
        changes.push(StyleChange::Fill(c));
    }
    if let Some(c) = color_arg(args, "font_color")? {
        changes.push(StyleChange::FontColor(c));
    }
    if let Some(d) = crate::agent::opt_int(args, "font_size_delta")? {
        if d == 0 || d.abs() > 100 {
            bail!("`font_size_delta` must be a non-zero number of points");
        }
        changes.push(StyleChange::FontSizeDelta(d as i32));
    }
    let borders = match opt_str(args, "borders")? {
        None => None,
        Some("all") => Some(BorderPreset::All),
        Some("outline") => Some(BorderPreset::Outline),
        Some("none") => Some(BorderPreset::None),
        Some(_) => bail!("`borders` must be all, outline or none"),
    };
    let merge = opt_bool(args, "merge")?;
    if changes.is_empty() && borders.is_none() && merge.is_none() {
        bail!(
            "give at least one of bold, italic, underline, strikethrough, align, valign, wrap, \
             number_format, fill, font_color, font_size_delta, borders, merge"
        );
    }
    Ok(FormatPlan {
        range,
        changes,
        borders,
        merge,
    })
}

pub fn selection(book: &Workbook, sheet: u32, range: CellRange, active: (u32, u32)) -> Value {
    json!({
        "sheet": book.sheet_names().get(sheet as usize).cloned().unwrap_or_default(),
        "range": range.name(),
        "active_cell": cell_name(active.0, active.1),
        "active_input": book.input(sheet, active.0, active.1),
        "active_value": book.display_value(sheet, active.0, active.1),
    })
}

/// `C:D` for columns `2..=3`.
pub fn columns_name(first: u32, last: u32) -> String {
    if first == last {
        column_name(first)
    } else {
        format!("{}:{}", column_name(first), column_name(last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book() -> Workbook {
        let mut b = Workbook::new_empty().unwrap();
        let plan = plan_write(&json!({
            "top_left": "A1",
            "values": [["Item", "Price"], ["Tea", 12000], ["Coffee", 1.5], ["Total", "=SUM(B2:B3)"]],
        }))
        .unwrap();
        b.set_inputs(0, &plan.cells).unwrap();
        b
    }

    #[std::prelude::v1::test]
    fn writes_and_reads_back() {
        let b = book();
        let r = read_range(&b, 0, &json!({})).unwrap();
        assert_eq!(r["range"], "A1:B4");
        assert_eq!(r["inputs"][3][1], "=SUM(B2:B3)");
        assert_eq!(r["values"][3][1], "12001.5");
        assert_eq!(r["values"][1][0], "Tea");
        let r = read_range(
            &b,
            0,
            &json!({"range": "A1:B4", "include": "values", "max_cells": 4}),
        )
        .unwrap();
        assert_eq!(r["range"], "A1:B2");
        assert_eq!(r["next_range"], "A3:B4");
        assert!(r.get("inputs").is_none());
        assert!(read_range(&b, 0, &json!({"range": "A1:Z1", "max_cells": 3})).is_err());
        let list = list_sheets(&b, 0);
        assert_eq!(list["sheets"][0]["used_range"], "A1:B4");
        assert_eq!(sheet_index(&b, &json!({"sheet": 1}), 0).unwrap(), 0);
        assert_eq!(sheet_index(&b, &json!({"sheet": "sheet1"}), 0).unwrap(), 0);
        assert!(sheet_index(&b, &json!({"sheet": 2}), 0).is_err());
    }

    #[std::prelude::v1::test]
    fn plans_writes() {
        let p = plan_write(&json!({"top_left": "C3", "values": [[null, true], ["x"]]})).unwrap();
        assert_eq!(p.range.name(), "C3:D4");
        assert_eq!(p.cells, vec![(2, 3, "TRUE".into()), (3, 2, "x".into())]);
        for bad in [
            json!({"top_left": "C3", "values": []}),
            json!({"top_left": "C3:D4", "values": [[1]]}).clone(),
            json!({"top_left": "Q", "values": [[1]]}),
            json!({"top_left": "A1", "values": [[{"a": 1}]]}),
            json!({"top_left": "A1048576", "values": [[1], [2]]}),
        ] {
            let ok = plan_write(&bad).is_ok();
            // A range as top_left is accepted (its corner is used).
            assert_eq!(ok, bad["top_left"] == "C3:D4", "{bad}");
        }
        let big: Vec<Vec<i32>> = vec![vec![1; 101]; 100];
        assert!(plan_write(&json!({"top_left": "A1", "values": big})).is_err());
    }

    #[std::prelude::v1::test]
    fn plans_formats() {
        let (range, changes) =
            plan_format(&json!({"range": "B2:C3", "bold": true, "number_format": "rupiah"}))
                .map(|p| (p.range, p.changes))
                .unwrap();
        assert_eq!(range.name(), "B2:C3");
        assert_eq!(changes.len(), 2);
        assert!(plan_format(&json!({"range": "B2"})).is_err());
        assert!(plan_format(&json!({"range": "B2", "align": "middle"})).is_err());
        let p = plan_format(&json!({
            "range": "A1:C1", "fill": "#FFF2CC", "font_color": "auto", "wrap": true,
            "valign": "top", "font_size_delta": 2, "borders": "outline", "merge": true,
        }))
        .unwrap();
        assert_eq!(p.changes.len(), 5);
        assert!(p.changes.contains(&StyleChange::Fill(Some(0xFFF2CC))));
        assert!(p.changes.contains(&StyleChange::FontColor(None)));
        assert_eq!(
            (p.borders, p.merge),
            (Some(BorderPreset::Outline), Some(true))
        );
        assert!(plan_format(&json!({"range": "B2", "fill": "#FFF"})).is_err());
        assert!(plan_format(&json!({"range": "B2", "borders": "thick"})).is_err());
        assert!(plan_format(&json!({"range": "B2", "font_size_delta": 0})).is_err());
        assert_eq!(column_arg(&json!({"c": "aa"}), "c").unwrap(), Some(26));
        assert!(column_arg(&json!({"c": "A1"}), "c").is_err());
        assert_eq!(row_arg(&json!({"r": 5}), "r").unwrap(), Some(4));
        assert!(row_arg(&json!({"r": 0}), "r").is_err());
    }
}
