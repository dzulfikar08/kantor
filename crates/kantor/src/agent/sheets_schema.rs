//! The JSON Schema of workbook descriptions (`kantor sheets build`) and of
//! the editing tools' arguments (`kantor sheets edit`, the MCP tool
//! `sheets_edit`): `kantor sheets schema` prints [`workbook`], kept as
//! docs/sheets-json.schema.json.

use serde_json::{Map, Value, json};

use super::sheets_author::{BOOK_KEYS, NUMBER_FORMATS, SHEET_KEYS, TOOLS, ToolSpec};

fn r(name: &str) -> Value {
    json!({"$ref": format!("#/$defs/{name}")})
}

fn s(desc: &str) -> Value {
    json!({"type": "string", "description": desc})
}

fn b(desc: &str) -> Value {
    json!({"type": "boolean", "description": desc})
}

fn n(desc: &str) -> Value {
    json!({"type": "number", "description": desc})
}

/// The schema of key `key` of tool `tool`.
fn prop(tool: &str, key: &str) -> Value {
    let colour = "A colour: #RRGGBB.";
    match (tool, key) {
        (_, "sheet") => {
            json!({"type": ["string", "integer"], "description": "Sheet name, or its number from 1 (default: the active sheet). References such as Data!A1:B9 name their own sheet."})
        }
        ("write_range", "values") => {
            json!({"type": "array", "description": "Rows of cells from top_left: text, numbers, booleans, formulas (\"=SUM(B2:B9)\"), null to leave a cell, \"\" to clear it. Text that looks like a number stays text with a leading apostrophe (\"'00123\").", "items": {"type": "array", "items": {"type": ["string", "number", "boolean", "null"]}}})
        }
        ("write_columns", "columns") => {
            json!({"type": "array", "description": "Columns of cells from top_left, each top to bottom.", "items": {"type": "array", "items": {"type": ["string", "number", "boolean", "null"]}}})
        }
        (_, "top_left") => s("The first cell, e.g. A1 or Data!B2."),
        (_, "range") => s("Cells: A1:D20, Data!A1:D20, whole columns C:E or rows 3:5."),
        ("fill_down" | "fill_right", "formula") => {
            s("A formula with =, its references as for the first cell (they adjust as it fills).")
        }
        ("format", "number_format")
        | ("to_numbers", "number_format")
        | ("to_dates", "number_format") => json!({"type": "string", "description": format!(
            "An Excel number format code (\"#,##0.00\", \"0.0%\", \"\\\"Rp\\\"#,##0\", \"dd/mm/yyyy\", \"@\" for text) or a preset: {}.",
            NUMBER_FORMATS.iter().map(|(k, c)| format!("{k} ({c})")).collect::<Vec<_>>().join(", "))}),
        ("format", "borders") => {
            json!({"description": "Borders: all, outline, inside, top, bottom, left, right, inside_horizontal, inside_vertical, none; or {edges, style (thin, medium, thick, double, dotted, dashed, hair, dashDot, ...), color}; or a list of these.", "anyOf": [
            {"type": "string", "enum": sheets_engine::BorderEdges::NAMES},
            r("border"),
            {"type": "array", "items": {"anyOf": [{"type": "string", "enum": sheets_engine::BorderEdges::NAMES}, r("border")]}}]})
        }
        ("format", "align") => {
            json!({"type": "string", "enum": ["general", "left", "center", "right"]})
        }
        ("format", "valign") => json!({"type": "string", "enum": ["top", "center", "bottom"]}),
        ("format", "font") => s("Typeface, e.g. Calibri, Arial, Inter."),
        ("format", "size") => n("Font size in points."),
        ("format", "font_size_delta") => {
            json!({"type": "integer", "description": "Grow (or shrink) the font by this many points."})
        }
        ("format", "fill") | ("format", "font_color") | ("format", "color") => {
            json!({"type": "string", "description": "#RRGGBB, or none / auto."})
        }
        ("format", "indent") => {
            json!({"type": "integer", "minimum": 0, "description": "Indent steps (about three spaces each)."})
        }
        ("format", "rotation") => {
            n("Text angle in degrees, -90 to 90 (counter-clockwise), or 255 for stacked letters.")
        }
        ("format", "merge") => b("true merges the range (Merge & Center), false unmerges it."),
        ("format", k) => b(&format!("{k} on or off.")),
        ("column_width", "columns") | ("autofit", "columns") | ("hide", "columns") => {
            json!({"description": "A column (C), a span (A:F) or a list of them.", "anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]})
        }
        ("column_width", "width") => {
            json!({"description": "Width in characters (Excel's unit; the default column is 8.43), or \"auto\".", "anyOf": [{"type": "number", "minimum": 0, "maximum": 255}, {"const": "auto"}]})
        }
        ("column_width", "widths") => {
            json!({"type": "object", "description": "Columns to widths, e.g. {\"A\": 14, \"B:D\": 10, \"E\": \"auto\"}.", "additionalProperties": {"anyOf": [{"type": "number"}, {"const": "auto"}]}})
        }
        ("autofit", "min") => n("Narrowest width (characters, default 4)."),
        ("autofit", "max") => n("Widest width (characters, default 60)."),
        ("row_height", "rows") | ("hide", "rows") => {
            json!({"type": ["string", "integer"], "description": "A row number from 1, or a span such as \"5:9\"."})
        }
        ("row_height", "height") => n("Height in points."),
        ("freeze", "cell") => {
            s("The first cell that scrolls: B2 freezes row 1 and column A; A1 unfreezes.")
        }
        ("freeze", "rows") => {
            json!({"type": "integer", "minimum": 0, "description": "Rows frozen at the top."})
        }
        ("freeze", "columns") => {
            json!({"type": "integer", "minimum": 0, "description": "Columns frozen at the left."})
        }
        (_, "before_row") | (_, "first_row") | (_, "last_row") => {
            json!({"type": "integer", "minimum": 1, "description": "Row number from 1."})
        }
        (_, "before_column") | (_, "first_column") | (_, "last_column") => {
            s("Column letter, e.g. C.")
        }
        (_, "count") => json!({"type": "integer", "minimum": 1}),
        (_, "hidden") => b("Hidden (true, the default) or shown (false)."),
        (_, "position") => {
            json!({"type": "integer", "minimum": 1, "description": "Tab position from 1."})
        }
        ("add_sheet", "name") | ("rename_sheet", "name") | ("copy_sheet", "name") => {
            s("Sheet name (up to 31 characters, none of : \\ / ? * [ ]).")
        }
        ("sort", "by") => {
            json!({"description": "A column letter, {column, order: asc|desc}, or a list of them (first is the main key).", "anyOf": [{"type": "string"}, r("sort_key"), {"type": "array", "items": {"anyOf": [{"type": "string"}, r("sort_key")]}}]})
        }
        ("sort", "descending") => b("Every key descending (unless it gives its order)."),
        (_, "header") => b(
            "The range's first row is a header (default: sort and remove_duplicates guess, add_table assumes true).",
        ),
        ("remove_duplicates", "columns") => {
            json!({"description": "The columns compared (default: all of the range's).", "anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]})
        }
        ("trim", "collapse") => {
            b("Also make runs of spaces inside the text one space (default true).")
        }
        ("to_numbers", "decimal") => {
            json!({"type": "string", "enum": [".", ",", "auto"], "description": "The decimal separator in the text (auto: from the texts; Rupiah amounts such as \"Rp 12.000\" group with dots either way)."})
        }
        ("to_dates", "order") => {
            json!({"type": "string", "enum": ["dmy", "mdy", "ymd", "auto"], "description": "Order of day, month and year in numeric dates such as 01/02/2026 (auto: from the texts, else day first)."})
        }
        ("split_column", "delimiter") => s("Where to split, e.g. \",\" or \" \"."),
        ("split_column", "to") | ("combine_columns", "to") => s("The first cell to write to."),
        (_, "overwrite") => b("Write over cells that aren't empty."),
        ("combine_columns", "separator") => s("Put between the parts (default a space)."),
        ("combine_columns", "formulas") => {
            b("Write formulas such as =A2&\" \"&B2 (default true) or the joined text.")
        }
        ("find_replace", "find") => s("Text to find."),
        ("find_replace", "replace") => s("Text to put instead."),
        ("find_replace", "all_sheets") => b("Every sheet."),
        ("find_replace", "match_case") => b("Case must match (default false)."),
        ("find_replace", "whole_cell") => b("The whole cell must match (default false)."),
        ("find_replace", "in_formulas") => b("Also change formulas' text (default false)."),
        ("add_table", "name") => s("Table name (letters, digits, _; default Table1, Table2...)."),
        ("add_table", "style") => s(
            "TableStyleLight1-21, TableStyleMedium1-28 (default TableStyleMedium2), TableStyleDark1-11, or none.",
        ),
        ("add_table", "totals") => {
            json!({"description": "true adds a total row; or column names to functions: sum, average, count, count_numbers, max, min, stddev, var, none.", "anyOf": [{"type": "boolean"}, {"type": "object", "additionalProperties": {"type": "string"}}]})
        }
        ("add_table", k) => b(&format!("{k} (Table Design option).")),
        ("add_chart", k) => chart_prop(k),
        ("add_conditional_format", k) => cf_prop(k),
        ("add_validation", k) => dv_prop(k),
        ("define_name", "name") | ("delete_name", "name") => s("The name, e.g. TaxRate."),
        ("define_name", "refers_to") => s(
            "The cells it names, e.g. Data!$B$2:$B$100 (names of constants or formulas aren't supported: name a cell holding the value).",
        ),
        ("add_pivot", k) => pivot_prop(k),
        ("page_setup", "landscape") => b("Landscape (else portrait)."),
        ("page_setup", "paper") => s("a4, letter, legal, f4, a3, a5, or WxHmm / WxHin."),
        ("page_setup", "fit") => {
            json!({"description": "\"width\" (one page wide), \"page\" (one page), \"none\", or [pages wide, pages tall] (0: any).", "anyOf": [{"type": "string", "enum": ["width", "page", "none"]}, {"type": "array", "items": {"type": "integer", "minimum": 0}, "minItems": 2, "maxItems": 2}]})
        }
        ("page_setup", "scale") => json!({"type": "number", "minimum": 10, "maximum": 400}),
        ("page_setup", "print_area") => s("The range printed (\"\" for the whole sheet)."),
        ("page_setup", "title_rows") => {
            json!({"type": ["string", "integer", "null"], "description": "Rows repeated at the top of each page, e.g. \"1\" or \"1:2\"."})
        }
        ("page_setup", "gridlines") => b("Print the gridlines."),
        ("sheet_view", "show_gridlines") => {
            b("false hides the gridlines on screen (true shows them).")
        }
        ("page_setup", "center") => b("Centre the sheet across the page."),
        (_, "color") => s(colour),
        (t, k) => s(&format!("{t}: {k}")),
    }
}

fn chart_prop(k: &str) -> Value {
    let slides = super::slides_schema::chart_props();
    match k {
        "at" => s("Top-left cell of the chart (default: two columns right of `data`)."),
        "to" => s("Bottom-right cell the chart covers (inclusive), e.g. at G2, to N18."),
        "w" | "h" => {
            json!({"type": ["number", "string"], "description": "Width / height when `to` isn't given: a number in `units` or a string such as \"6in\" or \"480px\"."})
        }
        "units" => super::slides_schema::units(),
        "data" => s(
            "A block of cells: series names along the top row, categories down the first column (text or dates; a scatter chart's x values), one series per column. E.g. Data!A1:C13.",
        ),
        "series_in" => {
            json!({"type": "string", "enum": ["columns", "rows"], "description": "The block's series are its columns (default) or rows."})
        }
        "categories" => {
            json!({"description": "With `series`: the category labels' cells (Data!A2:A13), or a list of labels.", "anyOf": [{"type": "string"}, {"type": "array", "items": {"type": ["string", "number"]}}]})
        }
        "series" => {
            let mut item = super::slides_schema::inline(&slides["series"]["items"]);
            item["properties"]["values"] = json!({"description": "The values' cells (Data!B2:B13), or numbers. With `data`, leave it out: the entries then only style the block's series.", "anyOf": [{"type": "string"}, {"type": "array", "items": {"type": ["number", "null"]}}]});
            item["properties"]["name"] = s("The series' name: text, or a cell with = (=Data!B1).");
            item["required"] = json!([]);
            json!({"type": "array", "items": item, "description": "The series: {name, values, color, colors, type, axis, line, data_labels}."})
        }
        other => super::slides_schema::inline(slides.get(other).unwrap_or(&json!({}))),
    }
}

fn cf_prop(k: &str) -> Value {
    match k {
        "type" => {
            json!({"type": "string", "enum": ["data_bar", "color_scale", "icon_set", "cell_value", "text", "top", "bottom", "above_average", "below_average", "duplicates", "unique", "blanks", "not_blanks", "formula", "dates"]})
        }
        "operator" => s(
            "cell_value: >, <, >=, <=, =, !=, between, not_between. text: contains, not_contains, begins_with, ends_with, equals.",
        ),
        "value" | "value2" => {
            json!({"type": ["number", "string"], "description": "cell_value: the bound(s): a number, text or =formula."})
        }
        "text" => s("text: the text looked for."),
        "formula" => s(
            "formula: true for the cells to format, written for the range's first cell (=$D2>100).",
        ),
        "rank" => {
            json!({"type": "integer", "minimum": 1, "description": "top/bottom: how many (default 10)."})
        }
        "percent" => b("top/bottom: rank is a percentage."),
        "period" => {
            json!({"type": "string", "enum": ["yesterday", "today", "tomorrow", "last_7_days", "last_week", "this_week", "next_week", "last_month", "this_month", "next_month"]})
        }
        "color" => s("data_bar: the bar's colour (default #638EC6)."),
        "colors" => {
            json!({"description": "color_scale: red_yellow_green (default), green_yellow_red, red_white_green, green_white_red, red_white_blue, blue_white_red, white_green, white_red, or 2-3 colours, the lowest value's first.", "anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}, "minItems": 2, "maxItems": 3}]})
        }
        "gradient" => b("data_bar: gradient fill (default true)."),
        "show_value" => b("data_bar / icon_set: show the cell's value too (default true)."),
        "icons" => s(
            "icon_set: arrows (default), arrows_gray, triangles, arrows4, arrows5, traffic_lights, traffic_lights4, signs, symbols, flags, stars, ratings (or Excel's names, 3Arrows...).",
        ),
        "reverse" => b("icon_set: reverse the icons."),
        "format" => {
            json!({"description": "What matching cells look like: red (default: light red fill, dark red text), yellow, green, red_fill, red_text, bold, or {fill, font_color, bold, italic, underline, strike}.", "anyOf": [{"type": "string"}, {"type": "object", "additionalProperties": false, "properties": {
            "fill": {"type": "string"}, "font_color": {"type": "string"}, "bold": {"type": "boolean"}, "italic": {"type": "boolean"}, "underline": {"type": "boolean"}, "strike": {"type": "boolean"}}}]})
        }
        "stop_if_true" => b("Later rules don't apply to cells this one matched."),
        other => s(other),
    }
}

fn dv_prop(k: &str) -> Value {
    match k {
        "type" => {
            json!({"type": "string", "enum": ["list", "whole", "decimal", "date", "time", "text_length", "custom", "any"]})
        }
        "values" => {
            json!({"type": "array", "items": {"type": ["string", "number"]}, "description": "list: the choices."})
        }
        "source" => s("list: the cells holding the choices, e.g. Lists!A2:A20."),
        "operator" => {
            json!({"type": "string", "enum": ["between", "not_between", "=", "!=", ">", "<", ">=", "<="], "description": "Default between."})
        }
        "value" | "value2" => {
            json!({"type": ["number", "string"], "description": "The bound(s): a number, a date (2026-01-31), a time (9:30) or =formula."})
        }
        "formula" => s("custom: a formula true for allowed values, e.g. =LEN(A2)=8."),
        "allow_blank" => b("Blank cells pass (default true)."),
        "dropdown" => b("list: show the drop-down arrow (default true)."),
        "prompt" | "error" => {
            json!({"description": "A message: text or {title, text}.", "anyOf": [{"type": "string"}, {"type": "object", "additionalProperties": false, "properties": {"title": {"type": "string"}, "text": {"type": "string"}}}]})
        }
        "error_style" => json!({"type": "string", "enum": ["stop", "warning", "information"]}),
        other => s(other),
    }
}

fn pivot_prop(k: &str) -> Value {
    let names =
        json!({"anyOf": [{"type": "string"}, {"type": "array", "items": {"type": "string"}}]});
    let with = |d: &str| {
        let mut v = names.clone();
        v["description"] = json!(d);
        v
    };
    match k {
        "source" => s("The data: a range with a header row (Data!A1:F500) or a table name."),
        "at" => s("The pivot table's top-left cell, e.g. Summary!H2 (or give new_sheet)."),
        "new_sheet" => s("Put it on a new sheet with this name (the default is a new sheet)."),
        "rows" => with("Field names down the side."),
        "columns" => with("Field names across the top."),
        "filters" => with("Field names as report filters."),
        "values" => {
            json!({"type": "array", "description": "What to add up: field names, or {field, func: sum|count|average|max|min|product|count_numbers|stddev|stddevp|var|varp, name, number_format}.",
            "items": {"anyOf": [{"type": "string"}, {"type": "object", "additionalProperties": false, "required": ["field"], "properties": {
                "field": {"type": "string"}, "func": {"type": "string"}, "name": {"type": "string"}, "number_format": {"type": "string"}}}]}})
        }
        "layout" => json!({"type": "string", "enum": ["compact", "outline", "tabular"]}),
        "grand_totals" => b("Grand total row and column (default true)."),
        "style" => s("PivotStyleLight16 (default), PivotStyleMedium2... or none."),
        other => s(other),
    }
}

/// The schema of `tool`'s arguments.
fn tool_args(t: &ToolSpec) -> Value {
    let props: Map<String, Value> = t
        .keys
        .iter()
        .map(|k| ((*k).to_owned(), prop(t.name, k)))
        .collect();
    json!({
        "type": "object",
        "description": t.summary,
        "additionalProperties": false,
        "properties": props,
        "required": t.required,
    })
}

/// The arguments of editing tool `tool` (with its references written
/// out).
#[cfg(test)]
fn tool_schema(tool: &str) -> Option<Value> {
    super::sheets_author::spec(tool).map(|t| inline(&tool_args(t)))
}

/// `v` with `#/$defs/...` references written out.
#[cfg(test)]
fn inline(v: &Value) -> Value {
    let defs = shared_defs();
    fn go(v: &Value, defs: &Map<String, Value>) -> Value {
        match v {
            Value::Object(m) => {
                if let Some(Value::String(p)) = m.get("$ref")
                    && let Some(d) = p.strip_prefix("#/$defs/").and_then(|n| defs.get(n))
                {
                    return go(d, defs);
                }
                Value::Object(m.iter().map(|(k, x)| (k.clone(), go(x, defs))).collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(|x| go(x, defs)).collect()),
            o => o.clone(),
        }
    }
    go(v, &defs)
}

fn shared_defs() -> Map<String, Value> {
    let mut d = Map::new();
    d.insert("border".into(), json!({"type": "object", "additionalProperties": false, "properties": {
        "edges": {"type": "string", "enum": sheets_engine::BorderEdges::NAMES, "description": "Default all."},
        "style": {"type": "string", "description": "thin (default), medium, thick, double, dotted, dashed, hair, dashDot, dashDotDot, mediumDashed, mediumDashDot, mediumDashDotDot, slantDashDot."},
        "color": {"type": "string", "description": "#RRGGBB (default black)."}}}));
    d.insert("sort_key".into(), json!({"type": "object", "additionalProperties": false, "required": ["column"], "properties": {
        "column": {"type": "string"}, "order": {"type": "string", "enum": ["asc", "desc"]}, "descending": {"type": "boolean"}}}));
    d
}

/// A tool's arguments for a sheet's list (`formats`, `charts`...): without
/// `sheet` (the sheet's own).
fn on_sheet(tool: &str) -> Value {
    let t = super::sheets_author::spec(tool).expect("a tool");
    let mut v = tool_args(t);
    if let Some(p) = v["properties"].as_object_mut() {
        p.remove("sheet");
    }
    v
}

/// The whole schema: a workbook description, every tool's arguments.
pub fn workbook() -> Value {
    let mut defs = shared_defs();
    for t in TOOLS {
        defs.insert(format!("{}_args", t.name), tool_args(t));
    }
    let ops: Vec<Value> = TOOLS
        .iter()
        .map(|t| json!({"type": "object", "additionalProperties": false, "required": ["tool"], "properties": {
            "tool": {"const": t.name}, "args": r(&format!("{}_args", t.name))}}))
        .collect();
    defs.insert("op".into(), json!({"description": "One editing tool and its arguments, as `kantor sheets edit FILE TOOL ARGS` takes them.", "oneOf": ops}));
    let grid = json!({"type": "array", "items": {"type": "array", "items": {"type": ["string", "number", "boolean", "null"]}}});
    let mut sheet_props = Map::new();
    let list =
        |tool: &str, d: &str| json!({"type": "array", "description": d, "items": on_sheet(tool)});
    for k in SHEET_KEYS {
        let v = match *k {
            "name" => s("The sheet's name (default Sheet1, Sheet2...)."),
            "start" => s("Where `rows` or `columns` start (default A1)."),
            "rows" => {
                let mut g = grid.clone();
                g["description"] = json!(
                    "The data row by row from `start`: text, numbers, booleans, formulas (\"=B2*C2\"), null for empty."
                );
                g
            }
            "columns" => {
                let mut g = grid.clone();
                g["description"] =
                    json!("The data column by column from `start` (each inner array is a column).");
                g
            }
            "cells" => {
                json!({"type": "object", "description": "Single cells: {\"B9\": \"=SUM(B2:B8)\", \"A9\": \"Total\"}.", "additionalProperties": {"type": ["string", "number", "boolean", "null"]}})
            }
            "widths" => prop("column_width", "widths"),
            "autofit" => {
                json!({"description": "true fits every used column to its contents; or the columns to fit.", "anyOf": [{"type": "boolean"}, {"type": "string"}, {"type": "array", "items": {"type": "string"}}]})
            }
            "heights" => {
                json!({"type": "object", "description": "Rows to heights in points: {\"1\": 30, \"2:5\": 18}.", "additionalProperties": {"type": "number"}})
            }
            "freeze" => {
                json!({"description": "The first cell that scrolls (\"B2\": row 1 and column A stay), or {rows, columns}.", "anyOf": [{"type": "string"}, {"type": "object", "properties": {"rows": {"type": "integer"}, "columns": {"type": "integer"}}, "additionalProperties": false}]})
            }
            "formats" => list(
                "format",
                "Formats, each {range, ...} as the format tool takes them.",
            ),
            "tables" => list("add_table", "Excel tables, each as add_table takes it."),
            "charts" => list(
                "add_chart",
                "Charts, each as add_chart takes it (`at`/`to` place it on this sheet).",
            ),
            "conditional_formats" => list(
                "add_conditional_format",
                "Conditional formats, each as add_conditional_format takes it.",
            ),
            "validations" => list(
                "add_validation",
                "Data validation, each as add_validation takes it.",
            ),
            "pivots" => {
                json!({"type": "array", "description": "Pivot tables placed on this sheet (`at`: a cell of it), each as add_pivot takes it.", "items": r("add_pivot_args")})
            }
            "hidden" => b("Hide the sheet."),
            "tab_color" => s("The tab's colour, #RRGGBB."),
            "gridlines" => b("Print (and draw in PDF/PNG) the gridlines."),
            "show_gridlines" => b(
                "false hides the gridlines on screen (Excel's View > Gridlines), e.g. for a dashboard.",
            ),
            "page_setup" => on_sheet("page_setup"),
            "ops" => {
                json!({"type": "array", "description": "Any editing tools, run on this sheet after the rest.", "items": r("op")})
            }
            other => s(other),
        };
        sheet_props.insert((*k).to_owned(), v);
    }
    defs.insert(
        "sheet".into(),
        json!({"type": "object", "additionalProperties": false, "properties": sheet_props}),
    );
    let mut props = Map::new();
    for k in BOOK_KEYS {
        let v = match *k {
            "sheets" => {
                json!({"type": "array", "minItems": 1, "items": r("sheet"), "description": "The sheets, in tab order. Each sheet's parts are made in this order: rows/columns/cells, formats, widths, heights, tables, conditional formats, validation, charts, pivots, freeze, page setup, ops."})
            }
            "names" => {
                json!({"type": "array", "description": "Defined names, made before the sheets' contents.", "items": r("define_name_args")})
            }
            "active" => {
                json!({"type": ["string", "integer"], "description": "The sheet shown when the file opens."})
            }
            "ops" => {
                json!({"type": "array", "description": "Editing tools run after every sheet is made.", "items": r("op")})
            }
            "calc" => {
                json!({"type": "string", "enum": ["automatic", "manual"], "description": "Calculation mode (default automatic)."})
            }
            other => s(other),
        };
        props.insert((*k).to_owned(), v);
    }
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://kantor.paparanai.id/schema/sheets-json.schema.json",
        "title": "Kantor workbook (kantor sheets build) and sheets editing tools (kantor sheets edit)",
        "type": "object",
        "additionalProperties": false,
        "required": ["sheets"],
        "properties": props,
        "$defs": defs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn the_schema_file_is_current() {
        let file = include_str!("../../../../docs/sheets-json.schema.json");
        let now = serde_json::to_string_pretty(&workbook()).unwrap() + "\n";
        assert!(
            file == now,
            "docs/sheets-json.schema.json is out of date: run `kantor sheets schema > docs/sheets-json.schema.json`"
        );
    }

    #[std::prelude::v1::test]
    fn every_tool_key_is_described() {
        for t in TOOLS {
            for k in t.keys {
                let p = prop(t.name, k);
                let d = p["description"].as_str().unwrap_or("");
                assert!(
                    !d.starts_with(&format!("{}: ", t.name))
                        || p.get("enum").is_some()
                        || p.get("anyOf").is_some(),
                    "{}.{k} has no description",
                    t.name
                );
            }
            let s = tool_schema(t.name).unwrap();
            assert!(!s.to_string().contains("$ref"), "{}", t.name);
        }
    }
}
