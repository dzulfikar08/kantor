//! Workbooks written from JSON: the editing tools of `kantor sheets edit`
//! (and the MCP tool `sheets_edit`) and the workbook descriptions of
//! `kantor sheets build` (see docs/sheets-json.md and
//! docs/sheets-json.schema.json). Every tool is a function of a
//! [`Workbook`], so the CLI and the app run the same code.
//!
//! Cells are A1-style (`B7`, `A1:D20`, `Data!A1:D20`, whole columns `C:E`,
//! whole rows `3:5`); sheets are named or numbered from 1; rows and columns
//! are 0-based inside.

use std::path::Path;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Map, Value, json};
use sheets_engine::{
    BorderEdges, CellRange, Fill, HAlign, LineStyle, MAX_COLS, MAX_ROWS, NumberFormat, RowHeight,
    StyleChange, VAlign, ValueKind, Workbook, cell_name, column_name, quote_sheet_name,
};

use super::sheets as basic;
use super::{MAX_TEXT, sheets_chart, sheets_clean, sheets_objects};

/// Sheets one workbook description may have.
pub const MAX_SHEETS: usize = 255;
/// Cells one description (or one write) may give.
pub const MAX_CELLS: usize = 2_000_000;

/// One editing tool: name (without `sheets_`), what it does, its keys.
pub struct ToolSpec {
    pub name: &'static str,
    pub summary: &'static str,
    pub keys: &'static [&'static str],
    pub required: &'static [&'static str],
}

macro_rules! tool {
    ($name:literal, $summary:literal, [$($k:literal),*], [$($r:literal),*]) => {
        ToolSpec { name: $name, summary: $summary, keys: &[$($k),*], required: &[$($r),*] }
    };
}

/// Every editing tool, in the order the help and the schema list them.
pub const TOOLS: &[ToolSpec] = &[
    tool!(
        "write_range",
        "Write rows of values and formulas (\"=...\") from a top-left cell, as typed. null leaves a cell, \"\" clears it.",
        ["sheet", "top_left", "values"],
        ["top_left", "values"]
    ),
    tool!(
        "write_columns",
        "Write columns of values from a top-left cell: each inner array is one column, top to bottom.",
        ["sheet", "top_left", "columns"],
        ["top_left", "columns"]
    ),
    tool!(
        "clear",
        "Clear the contents of a range (formats stay).",
        ["sheet", "range"],
        ["range"]
    ),
    tool!(
        "fill_down",
        "Copy the top row of a range down through it, formulas adjusting their relative references (Ctrl+D). With `formula`, that formula is written to the top row first. A single cell fills down to the last row of the data beside it.",
        ["sheet", "range", "formula"],
        ["range"]
    ),
    tool!(
        "fill_right",
        "Copy the left column of a range across it (Ctrl+R); `formula` is written to it first.",
        ["sheet", "range", "formula"],
        ["range"]
    ),
    tool!(
        "format",
        "Format a range: font, size, bold, colours, fill, alignment, wrap, number format (a preset or any Excel code), indent, rotation, borders, merge.",
        [
            "sheet",
            "range",
            "bold",
            "italic",
            "underline",
            "strikethrough",
            "font",
            "size",
            "font_size_delta",
            "font_color",
            "color",
            "fill",
            "align",
            "valign",
            "wrap",
            "number_format",
            "indent",
            "rotation",
            "borders",
            "merge"
        ],
        ["range"]
    ),
    tool!(
        "column_width",
        "Set column widths in characters (Excel's unit; 8.43 is the default), or \"auto\" to fit their contents.",
        ["sheet", "columns", "width", "widths"],
        []
    ),
    tool!(
        "autofit",
        "Widen columns to fit their contents (estimated from the text, as Excel's double-click does).",
        ["sheet", "columns", "min", "max"],
        []
    ),
    tool!(
        "row_height",
        "Set row heights in points (15 is the default for 11 pt text).",
        ["sheet", "rows", "height"],
        ["rows", "height"]
    ),
    tool!(
        "freeze",
        "Freeze panes: the rows above and the columns left of `cell` stay in view (or `rows` / `columns` counts; 0 / \"A1\" unfreezes).",
        ["sheet", "cell", "rows", "columns"],
        []
    ),
    tool!(
        "insert_rows",
        "Insert empty rows before row `before_row` (from 1).",
        ["sheet", "before_row", "count"],
        ["before_row"]
    ),
    tool!(
        "delete_rows",
        "Delete rows `first_row` to `last_row` (from 1).",
        ["sheet", "first_row", "last_row"],
        ["first_row"]
    ),
    tool!(
        "insert_columns",
        "Insert empty columns before column `before_column` (a letter).",
        ["sheet", "before_column", "count"],
        ["before_column"]
    ),
    tool!(
        "delete_columns",
        "Delete columns `first_column` to `last_column` (letters).",
        ["sheet", "first_column", "last_column"],
        ["first_column"]
    ),
    tool!(
        "hide",
        "Hide or show rows (`rows`: \"5:9\") or columns (`columns`: \"C:D\").",
        ["sheet", "rows", "columns", "hidden"],
        []
    ),
    tool!(
        "add_sheet",
        "Add a worksheet (after the last one, or at `position` from 1).",
        ["name", "position"],
        ["name"]
    ),
    tool!("delete_sheet", "Delete a worksheet.", ["sheet"], ["sheet"]),
    tool!(
        "rename_sheet",
        "Rename a worksheet (formulas follow).",
        ["sheet", "name"],
        ["sheet", "name"]
    ),
    tool!(
        "move_sheet",
        "Move a worksheet's tab to `position` (from 1).",
        ["sheet", "position"],
        ["sheet", "position"]
    ),
    tool!(
        "copy_sheet",
        "Copy a worksheet with its formats, tables and charts.",
        ["sheet", "name", "position"],
        ["sheet"]
    ),
    tool!(
        "hide_sheet",
        "Hide (or show, with hidden false) a worksheet.",
        ["sheet", "hidden"],
        ["sheet"]
    ),
    tool!(
        "sort",
        "Sort the rows of a range by one or more columns. The header row stays on top (`header`, default: guessed).",
        ["sheet", "range", "by", "descending", "header"],
        ["range", "by"]
    ),
    tool!(
        "remove_duplicates",
        "Delete rows of a range that repeat an earlier row (in `columns`, default all), keeping the first. Compares values, ignoring case and surrounding spaces.",
        ["sheet", "range", "columns", "header"],
        ["range"]
    ),
    tool!(
        "trim",
        "Remove leading, trailing and repeated spaces (and non-printing characters, non-breaking spaces) from text cells.",
        ["sheet", "range", "collapse"],
        ["range"]
    ),
    tool!(
        "to_numbers",
        "Turn numbers stored as text (\"1,234.50\", \"Rp 12.000\", \"(500)\", \"15%\") into numbers. `decimal`: \".\", \",\" or \"auto\".",
        ["sheet", "range", "decimal", "number_format"],
        ["range"]
    ),
    tool!(
        "to_dates",
        "Turn dates stored as text (\"15/01/2026\", \"2026-01-15\", \"15 Jan 2026\", \"15 Januari 2026\") into dates. `order`: dmy, mdy, ymd or auto.",
        ["sheet", "range", "order", "number_format"],
        ["range"]
    ),
    tool!(
        "split_column",
        "Split the text of a one-column range at `delimiter` into the columns from `to` (default: the next column; they must be empty unless overwrite).",
        ["sheet", "range", "delimiter", "to", "overwrite"],
        ["range", "delimiter"]
    ),
    tool!(
        "combine_columns",
        "Join the columns of a range into one column at `to` with `separator`, as formulas (=A2&\" \"&B2) unless formulas is false.",
        ["sheet", "range", "to", "separator", "formulas"],
        ["range", "to"]
    ),
    tool!(
        "find_replace",
        "Replace text in cells of a range, a sheet or every sheet (`all_sheets`). Formulas are left alone unless in_formulas.",
        [
            "sheet",
            "range",
            "all_sheets",
            "find",
            "replace",
            "match_case",
            "whole_cell",
            "in_formulas"
        ],
        ["find", "replace"]
    ),
    tool!(
        "add_table",
        "Make a range an Excel table (a ListObject with a style, filter buttons and an optional total row).",
        [
            "sheet",
            "range",
            "name",
            "style",
            "header",
            "totals",
            "banded_rows",
            "first_column",
            "last_column",
            "filter_buttons"
        ],
        ["range"]
    ),
    tool!(
        "add_chart",
        "Add a chart of cell ranges: column, bar, line, area, pie, doughnut, scatter or combo, placed by cell anchor (see `kantor sheets schema`).",
        [
            "sheet",
            "at",
            "to",
            "w",
            "h",
            "units",
            "data",
            "series_in",
            "chart",
            "stacked",
            "categories",
            "series",
            "title",
            "legend",
            "data_labels",
            "number_format",
            "axis",
            "category_axis",
            "gap_width",
            "overlap",
            "hole",
            "font",
            "colors",
            "target_line"
        ],
        ["chart"]
    ),
    tool!(
        "add_conditional_format",
        "Add a conditional format to a range: data_bar, color_scale, icon_set, cell_value, text, top, bottom, above_average, below_average, duplicates, unique, blanks, not_blanks, formula or dates.",
        [
            "sheet",
            "range",
            "type",
            "operator",
            "value",
            "value2",
            "text",
            "formula",
            "rank",
            "percent",
            "period",
            "color",
            "colors",
            "gradient",
            "show_value",
            "icons",
            "reverse",
            "format",
            "stop_if_true"
        ],
        ["range", "type"]
    ),
    tool!(
        "clear_conditional_formats",
        "Remove the conditional formats of a range (or the whole sheet).",
        ["sheet", "range"],
        []
    ),
    tool!(
        "add_validation",
        "Add data validation to a range: a drop-down `list`, whole, decimal, date, time, text_length or custom, with an input message and error alert.",
        [
            "sheet",
            "range",
            "type",
            "values",
            "source",
            "operator",
            "value",
            "value2",
            "formula",
            "allow_blank",
            "dropdown",
            "prompt",
            "error",
            "error_style"
        ],
        ["range", "type"]
    ),
    tool!(
        "clear_validation",
        "Remove data validation from a range.",
        ["sheet", "range"],
        ["range"]
    ),
    tool!(
        "define_name",
        "Define a name for a range or value (`refers_to`, e.g. \"Data!$B$2:$B$100\" or \"0.11\"), for the workbook or one `sheet`.",
        ["name", "refers_to", "sheet"],
        ["name", "refers_to"]
    ),
    tool!(
        "delete_name",
        "Delete a defined name.",
        ["name", "sheet"],
        ["name"]
    ),
    tool!(
        "add_pivot",
        "Add a pivot table of a range or table (its header row names the fields) at a cell or on a new sheet, with rows, columns, values and filters by field name.",
        [
            "source",
            "at",
            "new_sheet",
            "rows",
            "columns",
            "values",
            "filters",
            "layout",
            "grand_totals",
            "style"
        ],
        ["source", "values"]
    ),
    tool!(
        "page_setup",
        "Set how a sheet prints (and draws as PDF/PNG): orientation, paper, fit to pages, scale, print area, title rows, gridlines, centring.",
        [
            "sheet",
            "landscape",
            "paper",
            "fit",
            "scale",
            "print_area",
            "title_rows",
            "gridlines",
            "center"
        ],
        []
    ),
    tool!("recalculate", "Recalculate every formula.", [], []),
];

pub fn spec(tool: &str) -> Option<&'static ToolSpec> {
    let name = tool.strip_prefix("sheets_").unwrap_or(tool);
    TOOLS.iter().find(|t| t.name == name)
}

// ── reading arguments ──────────────────────────────────────────────────────

pub(super) fn get<'a>(o: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    o.get(key).filter(|v| !v.is_null())
}

pub(super) fn opt_str<'a>(o: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>> {
    match get(o, key) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => bail!("`{key}` must be a string"),
    }
}

pub(super) fn req_str<'a>(o: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    opt_str(o, key)?.ok_or_else(|| anyhow!("`{key}` is required"))
}

pub(super) fn opt_bool(o: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    match get(o, key) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => bail!("`{key}` must be true or false"),
    }
}

pub(super) fn opt_num(o: &Map<String, Value>, key: &str) -> Result<Option<f64>> {
    match get(o, key) {
        None => Ok(None),
        Some(v) => match v.as_f64() {
            Some(n) if n.is_finite() => Ok(Some(n)),
            _ => bail!("`{key}` must be a number"),
        },
    }
}

pub(super) fn opt_count(o: &Map<String, Value>, key: &str, min: u64) -> Result<Option<u64>> {
    match get(o, key) {
        None => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if n >= min => Ok(Some(n)),
            _ => bail!("`{key}` must be a whole number of at least {min}"),
        },
    }
}

/// `args` as an object with only `spec`'s keys.
fn args_of<'a>(args: &'a Value, spec: &ToolSpec) -> Result<&'a Map<String, Value>> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    let o = match args {
        Value::Null => EMPTY.get_or_init(Map::new),
        Value::Object(o) => o,
        _ => bail!("the arguments of {} must be an object", spec.name),
    };
    for k in o.keys() {
        if !spec.keys.contains(&k.as_str()) {
            bail!(
                "{} has no key \"{k}\" (keys: {})",
                spec.name,
                if spec.keys.is_empty() {
                    "none".to_owned()
                } else {
                    spec.keys.join(", ")
                }
            );
        }
    }
    for k in spec.required {
        if get(o, k).is_none() {
            bail!("{} needs `{k}`", spec.name);
        }
    }
    Ok(o)
}

/// The sheet `o.sheet` names (name or number from 1), else the active one.
pub(super) fn sheet_arg(book: &Workbook, o: &Map<String, Value>) -> Result<u32> {
    let shown = book.active_sheet();
    let mut probe = Map::new();
    if let Some(v) = get(o, "sheet") {
        probe.insert("sheet".into(), v.clone());
    }
    basic::sheet_index(book, &Value::Object(probe), shown)
}

/// A sheet by name (any case), for references like `Data!A1`.
pub(super) fn sheet_named(book: &Workbook, name: &str) -> Result<u32> {
    let names = book.sheet_names();
    names
        .iter()
        .position(|n| n == name)
        .or_else(|| names.iter().position(|n| n.eq_ignore_ascii_case(name)))
        .map(|i| i as u32)
        .ok_or_else(|| anyhow!("there is no sheet \"{name}\"; sheets: {}", names.join(", ")))
}

fn col_letters(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('$');
    if s.is_empty() || s.len() > 3 || !s.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let n = s.chars().fold(0u32, |n, c| {
        n * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1)
    });
    (n >= 1 && n <= MAX_COLS).then_some(n - 1)
}

fn row_digits(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches('$');
    let n: u32 = s.parse().ok()?;
    (n >= 1 && n <= MAX_ROWS).then_some(n - 1)
}

/// `Data!A1:C9`, `'My sheet'!B2`, `A1:C9`, `C:E` (whole columns), `3:5`
/// (whole rows), on `default` when no sheet is named.
pub fn parse_ref(book: &Workbook, text: &str, default: u32) -> Result<(u32, CellRange)> {
    let t = text.trim().trim_start_matches('=');
    let (sheet, cells) = match t.rfind('!') {
        Some(i) => {
            let name = t[..i].trim();
            let name = match name.strip_prefix('\'').and_then(|n| n.strip_suffix('\'')) {
                Some(q) => q.replace("''", "'"),
                None => name.to_owned(),
            };
            (sheet_named(book, &name)?, &t[i + 1..])
        }
        None => (default, t),
    };
    let bad =
        || anyhow!("\"{text}\" is not a cell or range like B7, A1:D20, Data!A1:D20, C:E or 3:5");
    if let Some(r) = sheets_engine::formula::parse_range(&cells.replace('$', "")) {
        return Ok((sheet, r));
    }
    let (a, b) = cells.split_once(':').unwrap_or((cells, cells));
    if let (Some(x), Some(y)) = (col_letters(a), col_letters(b)) {
        return Ok((sheet, CellRange::columns(x.min(y), x.max(y))));
    }
    if let (Some(x), Some(y)) = (row_digits(a), row_digits(b)) {
        return Ok((sheet, CellRange::rows(x.min(y), x.max(y))));
    }
    Err(bad())
}

/// Range `o[key]` on the sheet `o.sheet` (or the one the reference names).
pub(super) fn range_arg(
    book: &Workbook,
    o: &Map<String, Value>,
    key: &str,
) -> Result<(u32, CellRange)> {
    let default = sheet_arg(book, o)?;
    let text = req_str(o, key)?;
    parse_ref(book, text, default).with_context(|| format!("`{key}`"))
}

/// Whole rows or columns cut to the sheet's used area (at least one cell).
pub(super) fn used_part(book: &Workbook, sheet: u32, r: CellRange) -> CellRange {
    let (rows, cols) = book.used_size(sheet);
    CellRange {
        top: r.top,
        left: r.left,
        bottom: if r.is_whole_columns() {
            rows.max(r.top + 1) - 1
        } else {
            r.bottom
        },
        right: if r.is_whole_rows() {
            cols.max(r.left + 1) - 1
        } else {
            r.right
        },
    }
}

/// A column list: `"C"`, `"A:C"`, `["A", "C:D"]`, 0-based and sorted.
pub(super) fn columns_arg(v: &Value, key: &str) -> Result<Vec<u32>> {
    let mut out = Vec::new();
    let mut one = |s: &str| -> Result<()> {
        let (a, b) = s.split_once(':').unwrap_or((s, s));
        match (col_letters(a), col_letters(b)) {
            (Some(x), Some(y)) => {
                out.extend(x.min(y)..=x.max(y));
                Ok(())
            }
            _ => bail!("`{key}`: \"{s}\" is not a column like C or a span like A:C"),
        }
    };
    match v {
        Value::String(s) => one(s)?,
        Value::Array(items) => {
            for i in items {
                one(i
                    .as_str()
                    .with_context(|| format!("`{key}` holds column letters"))?)?;
            }
        }
        _ => bail!("`{key}` must be a column letter, a span like A:C, or a list of them"),
    }
    out.sort_unstable();
    out.dedup();
    if out.len() > MAX_COLS as usize {
        bail!("`{key}`: too many columns");
    }
    Ok(out)
}

/// A row span: `5`, `"5"`, `"5:9"`, 0-based inclusive.
pub(super) fn rows_arg(v: &Value, key: &str) -> Result<(u32, u32)> {
    let text = match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => bail!("`{key}` must be a row number from 1 or a span like \"5:9\""),
    };
    let (a, b) = text.split_once(':').unwrap_or((&text, &text));
    match (row_digits(a), row_digits(b)) {
        (Some(x), Some(y)) => Ok((x.min(y), x.max(y))),
        _ => bail!("`{key}`: \"{text}\" is not a row number from 1 or a span like 5:9"),
    }
}

/// `#RRGGBB`/`RRGGBB`/`#RGB` → `0xRRGGBB`.
pub(super) fn rgb(v: &Value, key: &str) -> Result<u32> {
    let s = v
        .as_str()
        .with_context(|| format!("`{key}` must be a colour like #1F4E79"))?;
    let hex = s.trim().trim_start_matches('#');
    let hex = if hex.len() == 3 {
        hex.chars().flat_map(|c| [c, c]).collect()
    } else {
        hex.to_owned()
    };
    match (hex.len(), u32::from_str_radix(&hex, 16)) {
        (6, Ok(c)) => Ok(c),
        _ => bail!("`{key}` \"{s}\" is not a colour like #1F4E79"),
    }
}

// ── values ─────────────────────────────────────────────────────────────────

/// What a JSON value is typed as: text, numbers, booleans, formulas.
pub(super) fn input_text(v: &Value) -> Result<Option<String>> {
    Ok(match v {
        Value::Null => None,
        Value::String(s) => {
            if s.chars().count() > MAX_TEXT {
                bail!("a cell is longer than {MAX_TEXT} characters");
            }
            Some(s.clone())
        }
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "TRUE" } else { "FALSE" }.to_owned()),
        _ => bail!("cells must be text, numbers, booleans or null, not {v}"),
    })
}

/// Text written so it stays text (an apostrophe before what would read as
/// a number, date, boolean or formula).
pub(super) fn as_text_input(s: &str) -> String {
    let t = s.trim_start();
    let risky = t.starts_with(['=', '+', '-', '\'', '@'])
        || t.chars()
            .next()
            .is_some_and(|c| c.is_ascii_digit() || c == '.')
        || t.eq_ignore_ascii_case("true")
        || t.eq_ignore_ascii_case("false")
        || (t.starts_with('(') && t.ends_with(')'))
        || t.starts_with('$')
        || t.ends_with('%');
    if risky && !s.is_empty() {
        format!("'{s}")
    } else {
        s.to_owned()
    }
}

/// A cell's value as JSON: numbers, text, booleans, errors as their text
/// (`#DIV/0!`), null when empty.
pub fn cell_value(book: &Workbook, sheet: u32, row: u32, col: u32) -> Value {
    match book.value_kind(sheet, row, col) {
        ValueKind::Number => match book.number_at(sheet, row, col) {
            Some((n, _)) => number(n),
            None => json!(book.display_value(sheet, row, col)),
        },
        ValueKind::Boolean => {
            json!(
                book.display_value(sheet, row, col)
                    .eq_ignore_ascii_case("TRUE")
            )
        }
        ValueKind::Error => json!(book.display_value(sheet, row, col)),
        ValueKind::Text => {
            let s = book.display_value(sheet, row, col);
            if s.is_empty() && book.input(sheet, row, col).is_empty() {
                Value::Null
            } else {
                json!(s)
            }
        }
    }
}

/// A number as JSON (whole numbers without `.0`).
pub fn number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() < 9.0e15 {
        json!(n as i64)
    } else {
        json!(n)
    }
}

/// The cells of a 2-D array from `start`, row by row (`by_columns`: each
/// inner array is a column).
pub(super) fn grid_cells(
    start: (u32, u32),
    rows: &Value,
    key: &str,
    by_columns: bool,
) -> Result<(CellRange, Vec<(u32, u32, String)>)> {
    let list = rows
        .as_array()
        .with_context(|| format!("`{key}` must be an array of arrays"))?;
    let mut cells = Vec::new();
    let (mut h, mut w) = (0usize, 0usize);
    for (i, line) in list.iter().enumerate() {
        let line = line
            .as_array()
            .with_context(|| format!("item {} of `{key}` is not an array", i + 1))?;
        for (j, v) in line.iter().enumerate() {
            let (r, c) = if by_columns { (j, i) } else { (i, j) };
            h = h.max(r + 1);
            w = w.max(c + 1);
            if let Some(t) = input_text(v)? {
                cells.push((start.0 as usize + r, start.1 as usize + c, t));
                if cells.len() > MAX_CELLS {
                    bail!("more than {MAX_CELLS} cells in `{key}`");
                }
            }
        }
    }
    if h == 0 || w == 0 {
        bail!("`{key}` has no cells");
    }
    let bottom = start.0 as usize + h - 1;
    let right = start.1 as usize + w - 1;
    if bottom >= MAX_ROWS as usize || right >= MAX_COLS as usize {
        bail!("`{key}` goes past the sheet's last row or column");
    }
    Ok((
        CellRange::from_corners(start, (bottom as u32, right as u32)),
        cells
            .into_iter()
            .map(|(r, c, t)| (r as u32, c as u32, t))
            .collect(),
    ))
}

/// The values of a written range (formulas' results), to show errors such
/// as #NAME? right away; `None` past 400 cells.
fn written_values(book: &Workbook, sheet: u32, r: CellRange) -> Value {
    if u64::from(r.rows_count()) * u64::from(r.cols_count()) > 400 {
        return Value::Null;
    }
    sheets_engine::numbers_invariant(|| {
        json!(
            (r.top..=r.bottom)
                .map(|row| (r.left..=r.right)
                    .map(|c| cell_value(book, sheet, row, c))
                    .collect::<Vec<_>>())
                .collect::<Vec<_>>()
        )
    })
}

/// Formula errors in `r` (`[{cell, error}]`), at most 20.
fn errors_in(book: &Workbook, sheet: u32, r: CellRange) -> Vec<Value> {
    let mut out = Vec::new();
    let r = used_part(book, sheet, r);
    for row in r.top..=r.bottom.min(r.top + 100_000) {
        for c in r.left..=r.right {
            if book.value_kind(sheet, row, c) == ValueKind::Error {
                out.push(
                    json!({"cell": cell_name(row, c), "error": book.display_value(sheet, row, c)}),
                );
                if out.len() >= 20 {
                    return out;
                }
            }
        }
    }
    out
}

// ── formats ────────────────────────────────────────────────────────────────

/// Number format presets (else the text is an Excel format code).
pub const NUMBER_FORMATS: [(&str, &str); 14] = [
    ("general", "General"),
    ("number", "#,##0.00"),
    ("integer", "#,##0"),
    ("decimal1", "#,##0.0"),
    ("percent", "0.0%"),
    ("percent0", "0%"),
    ("percent2", "0.00%"),
    ("rupiah", "\"Rp\"#,##0"),
    ("dollar", "\"$\"#,##0.00"),
    ("euro", "[$€-x-euro2] #,##0.00"),
    ("date", "yyyy-mm-dd"),
    ("datetime", "yyyy-mm-dd hh:mm"),
    ("time", "hh:mm"),
    ("text", "@"),
];

pub(super) fn number_format_code(s: &str) -> String {
    NUMBER_FORMATS
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(s.trim()))
        .map_or_else(|| s.to_owned(), |(_, c)| (*c).to_owned())
}

fn line_style(s: &str) -> Result<LineStyle> {
    let want = s.trim().replace('_', "").to_ascii_lowercase();
    LineStyle::ALL
        .into_iter()
        .find(|l| l.xml().to_ascii_lowercase() == want)
        .ok_or_else(|| {
            anyhow!(
                "border `style` must be one of {}",
                LineStyle::ALL
                    .iter()
                    .map(|l| l.xml())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

/// `borders`: "all", "outline", ... or `{edges, style, color}` or a list.
fn border_list(v: &Value) -> Result<Vec<(BorderEdges, LineStyle, Option<u32>)>> {
    let one = |v: &Value| -> Result<(BorderEdges, LineStyle, Option<u32>)> {
        match v {
            Value::String(s) => Ok((
                BorderEdges::parse(s).with_context(|| {
                    format!("`borders` \"{s}\": use {}", BorderEdges::NAMES.join(", "))
                })?,
                LineStyle::Thin,
                None,
            )),
            Value::Object(o) => {
                for k in o.keys() {
                    if !["edges", "style", "color"].contains(&k.as_str()) {
                        bail!("a border has no key \"{k}\" (keys: edges, style, color)");
                    }
                }
                let edges = opt_str(o, "edges")?.unwrap_or("all");
                Ok((
                    BorderEdges::parse(edges).with_context(|| {
                        format!(
                            "border `edges` \"{edges}\": use {}",
                            BorderEdges::NAMES.join(", ")
                        )
                    })?,
                    match opt_str(o, "style")? {
                        Some(s) => line_style(s)?,
                        None => LineStyle::Thin,
                    },
                    get(o, "color").map(|c| rgb(c, "color")).transpose()?,
                ))
            }
            _ => bail!("`borders` must be a string, an object or a list of them"),
        }
    };
    match v {
        Value::Array(items) => items.iter().map(one).collect(),
        v => Ok(vec![one(v)?]),
    }
}

/// Apply `format` keys of `o` to `range`.
fn apply_format(
    book: &mut Workbook,
    sheet: u32,
    range: CellRange,
    o: &Map<String, Value>,
) -> Result<usize> {
    let mut n = 0;
    for (key, make) in [
        ("bold", StyleChange::Bold as fn(bool) -> StyleChange),
        ("italic", StyleChange::Italic),
        ("underline", StyleChange::Underline),
        ("strikethrough", StyleChange::Strikethrough),
        ("wrap", StyleChange::Wrap),
    ] {
        if let Some(on) = opt_bool(o, key)? {
            book.set_range_style(sheet, range, make(on))?;
            n += 1;
        }
    }
    if let Some(a) = opt_str(o, "align")? {
        let a = match a {
            "general" => HAlign::General,
            "left" => HAlign::Left,
            "center" | "centre" => HAlign::Center,
            "right" => HAlign::Right,
            _ => bail!("`align` must be general, left, center or right"),
        };
        book.set_range_style(sheet, range, StyleChange::Align(a))?;
        n += 1;
    }
    if let Some(a) = opt_str(o, "valign")? {
        let a = match a {
            "top" => VAlign::Top,
            "center" | "middle" => VAlign::Center,
            "bottom" => VAlign::Bottom,
            _ => bail!("`valign` must be top, center or bottom"),
        };
        book.set_range_style(sheet, range, StyleChange::VAlign(a))?;
        n += 1;
    }
    let color_key = |key: &str| -> Result<Option<Option<u32>>> {
        match get(o, key) {
            None => Ok(None),
            Some(Value::String(s)) if s == "none" || s == "auto" => Ok(Some(None)),
            Some(v) => Ok(Some(Some(rgb(v, key)?))),
        }
    };
    if let Some(c) = color_key("fill")? {
        book.set_range_style(sheet, range, StyleChange::Fill(c))?;
        n += 1;
    }
    if get(o, "font_color").is_some() && get(o, "color").is_some() {
        bail!("give `font_color` or `color`, not both");
    }
    if let Some(c) = color_key("font_color")?.or(color_key("color")?) {
        book.set_range_style(sheet, range, StyleChange::FontColor(c))?;
        n += 1;
    }
    if let Some(f) = opt_str(o, "font")? {
        book.set_range_style_value(sheet, range, "font.name", f)?;
        n += 1;
    }
    if let Some(s) = opt_num(o, "size")? {
        if !(1.0..=409.0).contains(&s) {
            bail!("`size` must be 1 to 409 points");
        }
        book.set_range_style_value(sheet, range, "font.size", &(s.round() as i64).to_string())?;
        n += 1;
    }
    if let Some(d) = get(o, "font_size_delta").and_then(Value::as_i64) {
        if d == 0 || d.abs() > 100 {
            bail!("`font_size_delta` must be a non-zero number of points");
        }
        book.set_range_style(sheet, range, StyleChange::FontSizeDelta(d as i32))?;
        n += 1;
    }
    if let Some(f) = opt_str(o, "number_format")? {
        let code = number_format_code(f);
        if code.eq_ignore_ascii_case("general") {
            book.set_range_style(
                sheet,
                range,
                StyleChange::NumberFormat(NumberFormat::General),
            )?;
        } else {
            book.set_range_style_value(sheet, range, "num_fmt", &code)?;
        }
        n += 1;
    }
    if let Some(i) = opt_count(o, "indent", 0)? {
        book.set_range_style_value(sheet, range, "alignment.indent", &i.min(250).to_string())?;
        n += 1;
    }
    if let Some(r) = opt_num(o, "rotation")? {
        // Degrees counter-clockwise (-90 to 90), or 255 stacked.
        let v = if r == 255.0 {
            255
        } else if (0.0..=90.0).contains(&r) {
            r.round() as u32
        } else if (-90.0..0.0).contains(&r) {
            90 + (-r).round() as u32
        } else {
            bail!("`rotation` must be -90 to 90 degrees, or 255 for stacked letters");
        };
        book.set_range_style_value(sheet, range, "alignment.text_rotation", &v.to_string())?;
        n += 1;
    }
    if let Some(b) = get(o, "borders") {
        for (edges, style, color) in border_list(b)? {
            book.set_range_border(sheet, range, edges, style, color)?;
            n += 1;
        }
    }
    match opt_bool(o, "merge")? {
        Some(true) => {
            book.merge_range(sheet, range)?;
            n += 1;
        }
        Some(false) => {
            book.unmerge_range(sheet, range)?;
            n += 1;
        }
        None => {}
    }
    Ok(n)
}

/// Column width in Excel's characters → pixels at 100 %.
pub(super) fn chars_to_px(book: &Workbook, sheet: u32, chars: f64) -> f64 {
    let d = book.column_scale(sheet).digit_px;
    (chars * d).round() + 5.0
}

/// A column's width in Excel's characters.
pub fn px_to_chars(book: &Workbook, sheet: u32, px: f64) -> f64 {
    let d = book.column_scale(sheet).digit_px;
    (((px - 5.0) / d).max(0.0) * 100.0).round() / 100.0
}

/// Display width of `s` in characters (wide East Asian letters count two).
fn text_width(s: &str) -> f64 {
    s.lines()
        .map(|l| {
            l.chars()
                .map(|c| {
                    let u = c as u32;
                    if (0x1100..=0x115F).contains(&u)
                        || (0x2E80..=0xA4CF).contains(&u)
                        || (0xAC00..=0xD7A3).contains(&u)
                        || (0xF900..=0xFAFF).contains(&u)
                        || (0xFF00..=0xFF60).contains(&u)
                    {
                        2.0
                    } else if c.is_ascii_uppercase() || "mwMW@%".contains(c) {
                        1.2
                    } else if ".,:;'|!il1 ".contains(c) {
                        0.6
                    } else {
                        1.0
                    }
                })
                .sum::<f64>()
        })
        .fold(0.0, f64::max)
}

/// Widths (characters) that fit columns `cols`' contents.
pub(super) fn fit_widths(
    book: &Workbook,
    sheet: u32,
    cols: &[u32],
    min: f64,
    max: f64,
) -> Vec<(u32, f64)> {
    let (rows, _) = book.used_size(sheet);
    let last = rows.min(20_000);
    cols.iter()
        .map(|&c| {
            let mut w: f64 = 0.0;
            for r in 0..last {
                if book.merge_at(sheet, r, c).is_some_and(|m| !m.is_single()) {
                    continue;
                }
                let text = book.display_value(sheet, r, c);
                if text.is_empty() {
                    continue;
                }
                let st = book.cell_style(sheet, r, c);
                let size = if st.font_size > 0 { st.font_size } else { 11 };
                let mut k = f64::from(size) / 11.0;
                if st.bold {
                    k *= 1.08;
                }
                let tw = if st.wrap {
                    text_width(&text).min(40.0)
                } else {
                    text_width(&text)
                };
                w = w.max(tw * k + 1.5 + f64::from(st.indent) * 3.0);
            }
            let w = if w == 0.0 { 8.43 } else { w };
            (c, (w.clamp(min, max) * 100.0).round() / 100.0)
        })
        .collect()
}

// ── running tools ──────────────────────────────────────────────────────────

/// Run editing tool `tool` (`write_range` or `sheets_write_range`) on
/// `book`; returns what was done and the tool's answer. `base` is the
/// folder relative paths start from.
pub fn run_tool(
    book: &mut Workbook,
    tool: &str,
    args: &Value,
    base: Option<&Path>,
) -> Result<(String, Value)> {
    let spec = spec(tool).ok_or_else(|| {
        anyhow!(
            "{tool} is not a sheets editing tool (one of {})",
            TOOLS.iter().map(|t| t.name).collect::<Vec<_>>().join(", ")
        )
    })?;
    let o = args_of(args, spec)?;
    let _ = base;
    sheets_engine::numbers_invariant(|| run(book, spec.name, o))
}

fn sheet_name(book: &Workbook, sheet: u32) -> String {
    book.sheet_names()
        .get(sheet as usize)
        .cloned()
        .unwrap_or_default()
}

fn run(book: &mut Workbook, tool: &str, o: &Map<String, Value>) -> Result<(String, Value)> {
    match tool {
        "write_range" | "write_columns" => {
            let (sheet, start) = range_arg(book, o, "top_left")?;
            let key = if tool == "write_range" {
                "values"
            } else {
                "columns"
            };
            let (range, cells) = grid_cells(
                (start.top, start.left),
                get(o, key).context("no values")?,
                key,
                tool == "write_columns",
            )?;
            book.set_inputs(sheet, &cells)?;
            let mut out = json!({"sheet": sheet_name(book, sheet), "range": range.name()});
            let v = written_values(book, sheet, range);
            if !v.is_null() {
                out["values"] = v;
            }
            let errors = errors_in(book, sheet, range);
            if !errors.is_empty() {
                out["errors"] = json!(errors);
            }
            Ok((format!("wrote {}", range.name()), out))
        }
        "clear" => {
            let (sheet, r) = range_arg(book, o, "range")?;
            let r = used_part(book, sheet, r);
            book.clear_range(sheet, r)?;
            Ok((format!("cleared {}", r.name()), json!({"range": r.name()})))
        }
        "fill_down" | "fill_right" => {
            let (sheet, mut r) = range_arg(book, o, "range")?;
            let down = tool == "fill_down";
            if r.is_single() && down {
                // To the last row of the data beside it.
                let region = if r.left > 0 {
                    book.current_region(sheet, r.top, r.left - 1)
                } else {
                    book.current_region(sheet, r.top, r.left + 1)
                };
                let (rows, _) = book.used_size(sheet);
                let last = if region.bottom > r.top {
                    region.bottom
                } else {
                    rows.max(1) - 1
                };
                r.bottom = last.max(r.top);
            }
            if let Some(f) = opt_str(o, "formula")? {
                let cells: Vec<(u32, u32, String)> = if down {
                    (r.left..=r.right)
                        .map(|c| (r.top, c, f.to_owned()))
                        .collect()
                } else {
                    (r.top..=r.bottom)
                        .map(|row| (row, r.left, f.to_owned()))
                        .collect()
                };
                book.set_inputs(sheet, &cells)?;
            }
            let fill = if down { Fill::down(r) } else { Fill::right(r) };
            let fill = fill.context("give a range of more than one row (column) to fill")?;
            let target = book.fill(sheet, &fill)?;
            let mut out = json!({"range": target.name()});
            let errors = errors_in(book, sheet, target);
            if !errors.is_empty() {
                out["errors"] = json!(errors);
            }
            Ok((format!("filled {}", target.name()), out))
        }
        "format" => {
            let (sheet, r) = range_arg(book, o, "range")?;
            let r = if r.is_whole_columns() || r.is_whole_rows() {
                used_part(book, sheet, r)
            } else {
                r
            };
            let n = apply_format(book, sheet, r, o)?;
            if n == 0 {
                bail!("format needs at least one formatting key besides `range`");
            }
            Ok((
                format!("formatted {}", r.name()),
                json!({"range": r.name()}),
            ))
        }
        "column_width" => {
            let sheet = sheet_arg(book, o)?;
            let mut widths: Vec<(u32, Option<f64>)> = Vec::new();
            if let Some(m) = get(o, "widths") {
                let m = m
                    .as_object()
                    .context("`widths` must map columns to widths, e.g. {\"A\": 12, \"B:D\": 9}")?;
                for (cols, w) in m {
                    let w = match w {
                        Value::String(s) if s == "auto" => None,
                        v => Some(v.as_f64().with_context(|| {
                            format!("width of {cols} must be a number or \"auto\"")
                        })?),
                    };
                    for c in columns_arg(&json!(cols), "widths")? {
                        widths.push((c, w));
                    }
                }
            }
            if let Some(cols) = get(o, "columns") {
                let w = match get(o, "width") {
                    Some(Value::String(s)) if s == "auto" => None,
                    Some(v) => Some(v.as_f64().context("`width` must be a number or \"auto\"")?),
                    None => bail!("column_width needs `width` with `columns`"),
                };
                for c in columns_arg(cols, "columns")? {
                    widths.push((c, w));
                }
            }
            if widths.is_empty() {
                bail!("column_width needs `columns` and `width`, or `widths`");
            }
            let auto: Vec<u32> = widths
                .iter()
                .filter(|w| w.1.is_none())
                .map(|w| w.0)
                .collect();
            let fitted = fit_widths(book, sheet, &auto, 2.0, 80.0);
            let mut px = Vec::new();
            for (c, w) in &widths {
                let chars = match w {
                    Some(w) => *w,
                    None => fitted.iter().find(|f| f.0 == *c).map_or(8.43, |f| f.1),
                };
                if !(0.0..=255.0).contains(&chars) {
                    bail!("column widths are 0 to 255 characters");
                }
                px.push((*c, chars_to_px(book, sheet, chars)));
            }
            book.set_column_widths(sheet, &px)?;
            Ok((
                format!("set {} column width(s)", px.len()),
                json!({"columns": px.len()}),
            ))
        }
        "autofit" => {
            let sheet = sheet_arg(book, o)?;
            let cols = match get(o, "columns") {
                Some(v) => columns_arg(v, "columns")?,
                None => (0..book.used_size(sheet).1).collect(),
            };
            let min = opt_num(o, "min")?.unwrap_or(4.0).max(0.5);
            let max = opt_num(o, "max")?.unwrap_or(60.0).min(255.0).max(min);
            let fitted = fit_widths(book, sheet, &cols, min, max);
            let px: Vec<(u32, f64)> = fitted
                .iter()
                .map(|&(c, w)| (c, chars_to_px(book, sheet, w)))
                .collect();
            book.set_column_widths(sheet, &px)?;
            let shown: Map<String, Value> = fitted
                .iter()
                .take(50)
                .map(|(c, w)| (column_name(*c), json!(w)))
                .collect();
            Ok((
                format!("fitted {} column(s)", px.len()),
                json!({"widths": shown}),
            ))
        }
        "row_height" => {
            let sheet = sheet_arg(book, o)?;
            let (a, b) = rows_arg(get(o, "rows").context("no rows")?, "rows")?;
            let h = opt_num(o, "height")?.context("no height")?;
            if !(0.0..=409.0).contains(&h) || h == 0.0 {
                bail!("`height` must be more than 0 and at most 409 points");
            }
            book.set_row_heights(sheet, a, b, RowHeight::Points(h))?;
            Ok((
                format!("set the height of rows {}–{}", a + 1, b + 1),
                json!({}),
            ))
        }
        "freeze" => {
            let sheet = sheet_arg(book, o)?;
            let (rows, cols) = match opt_str(o, "cell")? {
                Some(c) => {
                    let r = basic::range_arg(&json!({"cell": c}), "cell")?;
                    (r.top, r.left)
                }
                None => (
                    opt_count(o, "rows", 0)?.unwrap_or(0) as u32,
                    opt_count(o, "columns", 0)?.unwrap_or(0) as u32,
                ),
            };
            book.set_frozen(sheet, rows, cols)?;
            let what = if rows == 0 && cols == 0 {
                "unfroze the panes".to_owned()
            } else {
                format!("froze {rows} row(s) and {cols} column(s)")
            };
            Ok((what, json!({"rows": rows, "columns": cols})))
        }
        "insert_rows" => {
            let sheet = sheet_arg(book, o)?;
            let row = rows_arg(get(o, "before_row").context("no row")?, "before_row")?.0;
            let count = opt_count(o, "count", 1)?.unwrap_or(1).min(100_000) as u32;
            book.insert_rows(sheet, row, count)?;
            Ok((format!("inserted {count} row(s) at {}", row + 1), json!({})))
        }
        "delete_rows" => {
            let sheet = sheet_arg(book, o)?;
            let first = rows_arg(get(o, "first_row").context("no row")?, "first_row")?.0;
            let last = match get(o, "last_row") {
                Some(v) => rows_arg(v, "last_row")?.0,
                None => first,
            };
            if last < first {
                bail!("`last_row` is before `first_row`");
            }
            book.delete_rows(sheet, first, last)?;
            Ok((
                format!("deleted rows {}–{}", first + 1, last + 1),
                json!({}),
            ))
        }
        "insert_columns" => {
            let sheet = sheet_arg(book, o)?;
            let col = columns_arg(
                get(o, "before_column").context("no column")?,
                "before_column",
            )?[0];
            let count = opt_count(o, "count", 1)?.unwrap_or(1).min(1_000) as u32;
            book.insert_columns(sheet, col, count)?;
            Ok((
                format!("inserted {count} column(s) at {}", column_name(col)),
                json!({}),
            ))
        }
        "delete_columns" => {
            let sheet = sheet_arg(book, o)?;
            let first =
                columns_arg(get(o, "first_column").context("no column")?, "first_column")?[0];
            let last = match get(o, "last_column") {
                Some(v) => columns_arg(v, "last_column")?[0],
                None => first,
            };
            if last < first {
                bail!("`last_column` is before `first_column`");
            }
            book.delete_columns(sheet, first, last)?;
            Ok((
                format!("deleted columns {}", basic::columns_name(first, last)),
                json!({}),
            ))
        }
        "hide" => {
            let sheet = sheet_arg(book, o)?;
            let hidden = opt_bool(o, "hidden")?.unwrap_or(true);
            let mut n = 0;
            if let Some(v) = get(o, "rows") {
                let (a, b) = rows_arg(v, "rows")?;
                if hidden {
                    book.hide_rows(sheet, a, b)?;
                } else {
                    book.unhide_rows(sheet, a, b)?;
                }
                n += 1;
            }
            if let Some(v) = get(o, "columns") {
                let cols = columns_arg(v, "columns")?;
                if let (Some(&a), Some(&b)) = (cols.first(), cols.last()) {
                    book.set_columns_hidden(sheet, a, b, hidden)?;
                }
                n += 1;
            }
            if n == 0 {
                bail!("hide needs `rows` or `columns`");
            }
            Ok((
                if hidden { "hid" } else { "showed" }.to_owned() + " rows/columns",
                json!({}),
            ))
        }
        "add_sheet" => {
            let name = req_str(o, "name")?;
            let n = book.sheet_names().len();
            let at = match opt_count(o, "position", 1)? {
                Some(p) => (p as usize - 1).min(book.tabs().len()),
                None => book.tabs().len(),
            };
            if n >= MAX_SHEETS {
                bail!("a workbook here has at most {MAX_SHEETS} sheets");
            }
            let i = book.insert_sheet(at)?;
            book.rename_sheet(i, name)?;
            Ok((
                format!("added sheet {name}"),
                json!({"sheet": name, "number": i + 1}),
            ))
        }
        "delete_sheet" => {
            let sheet = sheet_arg(book, o)?;
            let name = sheet_name(book, sheet);
            book.delete_sheet(sheet)?;
            Ok((format!("deleted sheet {name}"), json!({})))
        }
        "rename_sheet" => {
            let sheet = sheet_arg(book, o)?;
            let name = req_str(o, "name")?;
            let old = sheet_name(book, sheet);
            book.rename_sheet(sheet, name)?;
            Ok((
                format!("renamed sheet {old} to {name}"),
                json!({"sheet": name}),
            ))
        }
        "move_sheet" => {
            let sheet = sheet_arg(book, o)?;
            let to = opt_count(o, "position", 1)?.context("no position")? as usize - 1;
            book.move_tab(sheets_engine::Tab::Sheet(sheet), to.min(book.tabs().len()))?;
            Ok((
                format!("moved sheet {}", sheet_name(book, sheet)),
                json!({}),
            ))
        }
        "copy_sheet" => {
            let sheet = sheet_arg(book, o)?;
            let at = match opt_count(o, "position", 1)? {
                Some(p) => (p as usize - 1).min(book.tabs().len()),
                None => book.tabs().len(),
            };
            let i = book.copy_sheet(sheet, at)?;
            if let Some(name) = opt_str(o, "name")? {
                book.rename_sheet(i, name)?;
            }
            let name = sheet_name(book, i);
            Ok((
                format!("copied sheet to {name}"),
                json!({"sheet": name, "number": i + 1}),
            ))
        }
        "hide_sheet" => {
            let sheet = sheet_arg(book, o)?;
            let hidden = opt_bool(o, "hidden")?.unwrap_or(true);
            book.set_sheet_hidden(sheet, hidden)?;
            Ok((
                format!(
                    "{} sheet {}",
                    if hidden { "hid" } else { "showed" },
                    sheet_name(book, sheet)
                ),
                json!({}),
            ))
        }
        "sort" | "remove_duplicates" | "trim" | "to_numbers" | "to_dates" | "split_column"
        | "combine_columns" | "find_replace" => sheets_clean::run(book, tool, o),
        "add_chart" => sheets_chart::run(book, o),
        "add_table"
        | "add_conditional_format"
        | "clear_conditional_formats"
        | "add_validation"
        | "clear_validation"
        | "define_name"
        | "delete_name"
        | "add_pivot"
        | "page_setup" => sheets_objects::run(book, tool, o),
        "recalculate" => {
            book.recalculate();
            Ok(("recalculated".to_owned(), json!({})))
        }
        other => bail!("unknown tool {other}"),
    }
}

/// `sheets_edit`'s `ops` ([{tool, args}]) run in order; each one's answer
/// with `done`.
pub fn run_ops(book: &mut Workbook, ops: &Value, base: Option<&Path>) -> Result<Vec<Value>> {
    let list = ops
        .as_array()
        .context("`ops` must be an array of {\"tool\", \"args\"}")?;
    if list.is_empty() {
        bail!("`ops` is empty");
    }
    let mut out = Vec::new();
    for (i, op) in list.iter().enumerate() {
        let tool = op
            .get("tool")
            .and_then(Value::as_str)
            .with_context(|| format!("operation {} has no `tool`", i + 1))?;
        let args = op.get("args").cloned().unwrap_or(Value::Null);
        let (what, mut v) = run_tool(book, tool, &args, base)
            .with_context(|| format!("operation {} ({tool})", i + 1))?;
        if let Some(m) = v.as_object_mut() {
            m.insert("done".into(), json!(what));
        }
        out.push(v);
    }
    Ok(out)
}

/// The reading tools `sheets_list` and `sheets_check` with MCP arguments.
pub fn read_tool(book: &Workbook, tool: &str, args: &Value) -> Result<Value> {
    let sheet = match args.get("sheet") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    };
    match tool {
        "sheets_list" => super::sheets_list::list(
            book,
            &super::sheets_list::ListOpts {
                sheet,
                range: args.get("range").and_then(Value::as_str).map(str::to_owned),
                formulas: args
                    .get("formulas")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                max_rows: args
                    .get("max_rows")
                    .and_then(Value::as_u64)
                    .map(|n| n.min(5000) as u32),
            },
        ),
        "sheets_check" => {
            let only = match sheet {
                Some(s) => {
                    let v = match s.parse::<u64>() {
                        Ok(n) => json!({"sheet": n}),
                        Err(_) => json!({"sheet": s}),
                    };
                    Some(basic::sheet_index(book, &v, 0)?)
                }
                None => None,
            };
            Ok(super::sheets_check::check(book, only).json())
        }
        other => bail!("unknown tool {other}"),
    }
}

/// A sheet's name as formulas write it (`'My sheet'`).
pub fn quoted(book: &Workbook, sheet: u32) -> String {
    quote_sheet_name(&sheet_name(book, sheet))
}

// ── build ──────────────────────────────────────────────────────────────────

/// Keys of a workbook description.
pub const BOOK_KEYS: &[&str] = &["sheets", "names", "active", "ops", "calc"];
/// Keys of one sheet in it.
pub const SHEET_KEYS: &[&str] = &[
    "name",
    "start",
    "rows",
    "columns",
    "cells",
    "widths",
    "autofit",
    "heights",
    "freeze",
    "formats",
    "tables",
    "charts",
    "conditional_formats",
    "validations",
    "pivots",
    "hidden",
    "tab_color",
    "gridlines",
    "page_setup",
    "ops",
];

fn object<'a>(v: &'a Value, what: &str, allowed: &[&str]) -> Result<&'a Map<String, Value>> {
    let o = v
        .as_object()
        .with_context(|| format!("{what} must be an object"))?;
    for k in o.keys() {
        if !allowed.contains(&k.as_str()) {
            bail!(
                "{what} has an unknown key \"{k}\" (allowed: {})",
                allowed.join(", ")
            );
        }
    }
    Ok(o)
}

/// One step of a build: a tool and its arguments.
type Step = (String, Value);

/// `args` with `"sheet": name` added (unless it names one).
fn on_sheet(args: &Value, name: &str) -> Value {
    let mut a = args.as_object().cloned().unwrap_or_default();
    a.entry("sheet").or_insert_with(|| json!(name));
    Value::Object(a)
}

/// The steps that make sheet `s` (named `name`).
fn sheet_steps(s: &Map<String, Value>, name: &str) -> Result<Vec<Step>> {
    let mut steps: Vec<Step> = Vec::new();
    let start = opt_str(s, "start")?.unwrap_or("A1");
    if let Some(rows) = get(s, "rows") {
        steps.push((
            "write_range".into(),
            json!({"sheet": name, "top_left": start, "values": rows}),
        ));
    }
    if let Some(cols) = get(s, "columns") {
        if get(s, "rows").is_some() {
            bail!("sheet {name}: give `rows` or `columns`, not both");
        }
        steps.push((
            "write_columns".into(),
            json!({"sheet": name, "top_left": start, "columns": cols}),
        ));
    }
    if let Some(cells) = get(s, "cells") {
        let m = cells.as_object().with_context(|| {
            format!("sheet {name}: `cells` maps cells to values, e.g. {{\"B9\": \"=SUM(B2:B8)\"}}")
        })?;
        for (cell, v) in m {
            steps.push((
                "write_range".into(),
                json!({"sheet": name, "top_left": cell, "values": [[v]]}),
            ));
        }
    }
    let list = |key: &str, tool: &str, steps: &mut Vec<Step>| -> Result<()> {
        if let Some(v) = get(s, key) {
            let items = v
                .as_array()
                .with_context(|| format!("sheet {name}: `{key}` must be an array"))?;
            for i in items {
                steps.push((tool.into(), on_sheet(i, name)));
            }
        }
        Ok(())
    };
    list("formats", "format", &mut steps)?;
    if let Some(w) = get(s, "widths") {
        steps.push(("column_width".into(), json!({"sheet": name, "widths": w})));
    }
    match get(s, "autofit") {
        None | Some(Value::Bool(false)) => {}
        Some(Value::Bool(true)) => steps.push(("autofit".into(), json!({"sheet": name}))),
        Some(cols) => steps.push(("autofit".into(), json!({"sheet": name, "columns": cols}))),
    }
    if let Some(h) = get(s, "heights") {
        let m = h.as_object().with_context(|| {
            format!("sheet {name}: `heights` maps rows to points, e.g. {{\"1\": 28}}")
        })?;
        for (rows, pt) in m {
            steps.push((
                "row_height".into(),
                json!({"sheet": name, "rows": rows, "height": pt}),
            ));
        }
    }
    list("tables", "add_table", &mut steps)?;
    list("conditional_formats", "add_conditional_format", &mut steps)?;
    list("validations", "add_validation", &mut steps)?;
    list("charts", "add_chart", &mut steps)?;
    if let Some(p) = get(s, "pivots") {
        for i in p
            .as_array()
            .with_context(|| format!("sheet {name}: `pivots` must be an array"))?
        {
            // A pivot table goes on this sheet (its `source` names its data).
            let mut a = i.as_object().cloned().unwrap_or_default();
            if let Some(at) = a.get("at").and_then(Value::as_str)
                && !at.contains('!')
            {
                a.insert(
                    "at".into(),
                    json!(format!("{}!{at}", quote_sheet_name(name))),
                );
            }
            steps.push(("add_pivot".into(), Value::Object(a)));
        }
    }
    if let Some(f) = get(s, "freeze") {
        let a = match f {
            Value::String(c) => json!({"sheet": name, "cell": c}),
            Value::Object(_) => on_sheet(f, name),
            _ => bail!("sheet {name}: `freeze` is a cell like \"B2\" or {{rows, columns}}"),
        };
        steps.push(("freeze".into(), a));
    }
    if let Some(p) = get(s, "page_setup") {
        steps.push(("page_setup".into(), on_sheet(p, name)));
    }
    if let Some(g) = get(s, "gridlines") {
        steps.push(("page_setup".into(), json!({"sheet": name, "gridlines": g})));
    }
    if let Some(v) = get(s, "ops") {
        for (i, op) in v
            .as_array()
            .with_context(|| format!("sheet {name}: `ops` must be an array"))?
            .iter()
            .enumerate()
        {
            let tool = op
                .get("tool")
                .and_then(Value::as_str)
                .with_context(|| format!("sheet {name}: op {} has no `tool`", i + 1))?;
            let args = op.get("args").cloned().unwrap_or(json!({}));
            let args = if spec(tool).is_some_and(|t| t.keys.contains(&"sheet")) {
                on_sheet(&args, name)
            } else {
                args
            };
            steps.push((tool.to_owned(), args));
        }
    }
    if opt_bool(s, "hidden")? == Some(true) {
        steps.push(("hide_sheet".into(), json!({"sheet": name})));
    }
    Ok(steps)
}

/// A new workbook from description `v` (see docs/sheets-json.md). The
/// same description always gives the same workbook (and, written with
/// [`to_bytes`], the same file).
pub fn build(v: &Value, base: Option<&Path>) -> Result<Workbook> {
    let o = object(v, "the workbook", BOOK_KEYS)?;
    let sheets = get(o, "sheets")
        .context("a workbook needs `sheets`: [{\"name\": ..., \"rows\": [...]}, ...]")?
        .as_array()
        .context("`sheets` must be an array")?;
    if sheets.is_empty() || sheets.len() > MAX_SHEETS {
        bail!("a workbook needs 1 to {MAX_SHEETS} sheets");
    }
    let mut book = Workbook::new_empty()?;
    if opt_str(o, "calc")? == Some("manual") {
        book.set_calc_mode(sheets_engine::CalcMode::Manual);
    }
    // The sheets first (formulas may refer to later ones).
    let mut names = Vec::new();
    for (i, s) in sheets.iter().enumerate() {
        let so = object(s, &format!("sheet {}", i + 1), SHEET_KEYS)?;
        let name = opt_str(so, "name")?
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Sheet{}", i + 1));
        if names.iter().any(|n: &String| n.eq_ignore_ascii_case(&name)) {
            bail!("two sheets are named {name}");
        }
        if i > 0 {
            book.insert_sheet(i)?;
        }
        if book.sheet_names()[i] != name {
            book.rename_sheet(i as u32, &name)
                .with_context(|| format!("sheet {}: name {name:?}", i + 1))?;
        }
        names.push(name);
    }
    if let Some(list) = get(o, "names") {
        for (i, n) in list
            .as_array()
            .context("`names` must be an array of {name, refers_to}")?
            .iter()
            .enumerate()
        {
            run_tool(&mut book, "define_name", n, base)
                .with_context(|| format!("name {}", i + 1))?;
        }
    }
    for (i, s) in sheets.iter().enumerate() {
        let so = s.as_object().expect("checked");
        let name = &names[i];
        if let Some(c) = get(so, "tab_color") {
            let c = rgb(c, "tab_color")?;
            book.set_tab_color(i as u32, Some(sheets_engine::TabColor::Rgb(c)))?;
        }
        for (k, (tool, args)) in sheet_steps(so, name)?.into_iter().enumerate() {
            run_tool(&mut book, &tool, &args, base)
                .with_context(|| format!("sheet {name}: step {} ({tool})", k + 1))?;
        }
    }
    if let Some(ops) = get(o, "ops") {
        for (i, op) in ops
            .as_array()
            .context("`ops` must be an array of {tool, args}")?
            .iter()
            .enumerate()
        {
            let tool = op
                .get("tool")
                .and_then(Value::as_str)
                .with_context(|| format!("op {} has no `tool`", i + 1))?;
            run_tool(
                &mut book,
                tool,
                op.get("args").unwrap_or(&Value::Null),
                base,
            )
            .with_context(|| format!("op {} ({tool})", i + 1))?;
        }
    }
    let active = match get(o, "active") {
        None => 0,
        Some(Value::String(s)) => sheet_named(&book, s)?,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=names.len() as u64).contains(n))
            .map(|n| n as u32 - 1)
            .context("`active` is a sheet name or number from 1")?,
    };
    book.set_active_sheet(active)?;
    Ok(book)
}

/// The `.xlsx` bytes of `book`, the same for the same workbook: the zip's
/// file times and the document's dates are fixed.
pub fn to_bytes(book: &Workbook) -> Result<Vec<u8>> {
    let out = book.xlsx_snapshot().render()?;
    normalize_zip(&out.bytes)
}

/// The time written into built files (2000-01-01).
const FIXED_DATE: &str = "2000-01-01T00:00:00Z";

fn normalize_zip(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::io::{Read as _, Write as _};
    let mut zin = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut out = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let time = zip::DateTime::from_date_and_time(2000, 1, 1, 0, 0, 0)
        .map_err(|_| anyhow!("bad zip date"))?;
    for i in 0..zin.len() {
        let mut f = zin.by_index(i)?;
        let name = f.name().to_owned();
        let mut data = Vec::new();
        f.read_to_end(&mut data)?;
        if name == "docProps/core.xml" {
            let text = String::from_utf8_lossy(&data).into_owned();
            data = fix_dates(&text).into_bytes();
        }
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(time);
        out.start_file(name, opts)?;
        out.write_all(&data)?;
    }
    Ok(out.finish()?.into_inner())
}

/// `dcterms:created` / `dcterms:modified` set to [`FIXED_DATE`].
fn fix_dates(xml: &str) -> String {
    let mut s = xml.to_owned();
    for tag in ["dcterms:created", "dcterms:modified"] {
        if let Some(a) = s.find(&format!("<{tag}"))
            && let Some(gt) = s[a..].find('>')
            && let Some(end) = s[a..].find(&format!("</{tag}>"))
        {
            let (from, to) = (a + gt + 1, a + end);
            if from <= to {
                s.replace_range(from..to, FIXED_DATE);
            }
        }
    }
    s
}

/// Cells of a sheet that hold formulas, for tests and summaries.
pub fn formula_cells(book: &Workbook, sheet: u32) -> Vec<(u32, u32)> {
    book.filled_cells(sheet)
        .into_iter()
        .filter(|&(r, c)| book.input(sheet, r, c).starts_with('='))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn run_ok(book: &mut Workbook, tool: &str, args: Value) -> Value {
        run_tool(book, tool, &args, None)
            .unwrap_or_else(|e| panic!("{tool}: {e:#}"))
            .1
    }

    fn sales() -> Workbook {
        let mut b = Workbook::new_empty().unwrap();
        run_ok(
            &mut b,
            "write_range",
            json!({"top_left": "A1", "values": [
                ["Region", "Units", "Price", "Revenue"],
                ["North", 10, 2.5, null],
                ["South", 4, 3, null],
                ["East", 7, 1.25, null],
            ]}),
        );
        b
    }

    #[std::prelude::v1::test]
    fn references_parse() {
        let mut b = sales();
        run_ok(&mut b, "add_sheet", json!({"name": "My data"}));
        assert_eq!(
            parse_ref(&b, "B2:C3", 0).unwrap(),
            (0, CellRange::from_corners((1, 1), (2, 2)))
        );
        assert_eq!(parse_ref(&b, "'My data'!A1", 0).unwrap().0, 1);
        assert_eq!(
            parse_ref(&b, "my data!$A$1:$B$2", 0).unwrap().1.name(),
            "A1:B2"
        );
        assert_eq!(parse_ref(&b, "C:E", 0).unwrap().1, CellRange::columns(2, 4));
        assert_eq!(parse_ref(&b, "3:5", 0).unwrap().1, CellRange::rows(2, 4));
        assert!(parse_ref(&b, "Nope!A1", 0).is_err());
        assert!(parse_ref(&b, "A0", 0).is_err());
        assert_eq!(
            columns_arg(&json!(["A", "C:D"]), "c").unwrap(),
            vec![0, 2, 3]
        );
        assert_eq!(rows_arg(&json!("9:5"), "r").unwrap(), (4, 8));
        assert_eq!(rows_arg(&json!(3), "r").unwrap(), (2, 2));
    }

    #[std::prelude::v1::test]
    fn writes_fills_and_formats() {
        let mut b = sales();
        let out = run_ok(
            &mut b,
            "fill_down",
            json!({"range": "D2:D4", "formula": "=B2*C2"}),
        );
        assert_eq!(out["range"], "D2:D4");
        assert_eq!(b.input(0, 3, 3), "=B4*C4");
        assert_eq!(cell_value(&b, 0, 3, 3), json!(8.75));
        // One cell fills to the end of the data beside it.
        run_ok(
            &mut b,
            "fill_down",
            json!({"range": "E2", "formula": "=D2/B2"}),
        );
        assert_eq!(b.input(0, 3, 4), "=D4/B4");
        let out = run_ok(
            &mut b,
            "write_range",
            json!({"top_left": "A6", "values": [["Total", "=SUM(B2:B4)", null, "=SUM(D2:D4)", "=1/0"]]}),
        );
        assert_eq!(out["values"][0][1], json!(21));
        assert_eq!(out["errors"][0]["cell"], "E6");
        run_ok(
            &mut b,
            "format",
            json!({"range": "A1:D1", "bold": true, "fill": "#1F4E79", "font_color": "#FFFFFF", "font": "Arial", "size": 12, "borders": {"edges": "bottom", "style": "medium", "color": "#000000"}}),
        );
        let st = b.cell_style(0, 0, 0);
        assert!(st.bold);
        assert_eq!(st.fill, Some(0x1F4E79));
        assert_eq!(st.font_name.as_deref(), Some("Arial"));
        assert_eq!(st.font_size, 12);
        assert_eq!(st.borders.bottom.map(|l| l.style), Some(LineStyle::Medium));
        run_ok(
            &mut b,
            "format",
            json!({"range": "C2:D6", "number_format": "integer"}),
        );
        assert_eq!(b.cell_style(0, 1, 2).num_fmt, "#,##0");
        run_ok(
            &mut b,
            "format",
            json!({"range": "B2:B4", "number_format": "0.0 \"kg\"", "indent": 1, "rotation": 45}),
        );
        let st = b.cell_style(0, 1, 1);
        assert_eq!(
            (st.num_fmt.as_str(), st.indent, st.rotation),
            ("0.0 \"kg\"", 1, 45)
        );
        assert!(run_tool(&mut b, "format", &json!({"range": "A1"}), None).is_err());
        assert!(
            run_tool(
                &mut b,
                "format",
                &json!({"range": "A1", "bolt": true}),
                None
            )
            .unwrap_err()
            .to_string()
            .contains("no key \"bolt\"")
        );
        run_ok(
            &mut b,
            "write_columns",
            json!({"top_left": "H1", "columns": [["a", "b"], [1, 2]]}),
        );
        assert_eq!(b.display_value(0, 1, 7), "b");
        assert_eq!(b.display_value(0, 1, 8), "2");
        run_ok(&mut b, "clear", json!({"range": "H1:I2"}));
        assert_eq!(b.input(0, 0, 7), "");
    }

    #[std::prelude::v1::test]
    fn widths_rows_freeze_and_sheets() {
        let mut b = sales();
        run_ok(
            &mut b,
            "column_width",
            json!({"widths": {"A": 20, "B:C": 9}}),
        );
        assert!((px_to_chars(&b, 0, b.column_width(0, 0)) - 20.0).abs() < 0.2);
        let out = run_ok(&mut b, "autofit", json!({"columns": "A:D"}));
        assert!(out["widths"]["A"].as_f64().unwrap() >= 4.0);
        run_ok(&mut b, "row_height", json!({"rows": "1", "height": 28}));
        run_ok(&mut b, "freeze", json!({"cell": "B2"}));
        assert_eq!(b.frozen(0), (1, 1));
        run_ok(&mut b, "insert_rows", json!({"before_row": 2, "count": 2}));
        assert_eq!(b.display_value(0, 3, 0), "North");
        run_ok(
            &mut b,
            "delete_rows",
            json!({"first_row": 2, "last_row": 3}),
        );
        run_ok(&mut b, "insert_columns", json!({"before_column": "B"}));
        run_ok(&mut b, "delete_columns", json!({"first_column": "B"}));
        assert_eq!(b.display_value(0, 1, 1), "10");
        run_ok(&mut b, "hide", json!({"rows": "3"}));
        assert!(b.is_row_hidden(0, 2));
        run_ok(&mut b, "hide", json!({"columns": "C", "hidden": true}));
        assert!(b.is_column_hidden(0, 2));
        run_ok(
            &mut b,
            "add_sheet",
            json!({"name": "Summary", "position": 1}),
        );
        assert_eq!(b.sheet_names()[0], "Summary");
        run_ok(
            &mut b,
            "move_sheet",
            json!({"sheet": "Summary", "position": 2}),
        );
        assert_eq!(b.sheet_names()[1], "Summary");
        run_ok(
            &mut b,
            "rename_sheet",
            json!({"sheet": "Summary", "name": "Totals"}),
        );
        run_ok(&mut b, "copy_sheet", json!({"sheet": 1, "name": "Copy"}));
        assert!(b.sheet_names().contains(&"Copy".to_owned()));
        run_ok(&mut b, "hide_sheet", json!({"sheet": "Copy"}));
        run_ok(&mut b, "delete_sheet", json!({"sheet": "Totals"}));
        assert!(!b.sheet_names().contains(&"Totals".to_owned()));
        assert!(run_tool(&mut b, "no_such_tool", &json!({}), None).is_err());
    }

    #[std::prelude::v1::test]
    fn builds_are_deterministic() {
        let desc = json!({
            "sheets": [
                {"name": "Data", "rows": [["Month", "Sales"], ["Jan", 10], ["Feb", 12], ["Mar", 9]],
                 "cells": {"A5": "Total", "B5": "=SUM(B2:B4)"},
                 "widths": {"A": 12}, "freeze": "A2",
                 "formats": [{"range": "A1:B1", "bold": true}],
                 "tables": [{"range": "A1:B4", "name": "Sales"}]},
                {"name": "Notes", "columns": [["Note", "Checked"]], "hidden": true}
            ],
            "names": [{"name": "TaxRate", "refers_to": "0.11"}],
            "active": "Data"
        });
        let b = build(&desc, None).unwrap();
        assert_eq!(b.sheet_names(), vec!["Data", "Notes"]);
        assert_eq!(cell_value(&b, 0, 4, 1), json!(31));
        assert_eq!(b.frozen(0), (1, 0));
        assert_eq!(b.tables(0)[0].name, "Sales");
        assert!(b.sheet_hidden(1));
        assert!(b.defined_names().iter().any(|n| n.name == "TaxRate"));
        let one = to_bytes(&b).unwrap();
        let two = to_bytes(&build(&desc, None).unwrap()).unwrap();
        assert_eq!(one, two);
        // It opens again.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("b.xlsx");
        std::fs::write(&p, &one).unwrap();
        let again = Workbook::open(&p).unwrap();
        assert_eq!(again.input(0, 4, 1), "=SUM(B2:B4)");
        assert!(build(&json!({"sheets": []}), None).is_err());
        assert!(build(&json!({"sheets": [{"nme": "x"}]}), None).is_err());
        assert!(build(&json!({"sheets": [{"name": "A"}, {"name": "a"}]}), None).is_err());
    }
}
