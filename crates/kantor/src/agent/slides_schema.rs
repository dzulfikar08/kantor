//! The JSON Schema of slide elements and decks ([`super::slides_author`]):
//! `kantor slides schema` prints [`deck`] (kept as
//! docs/slides-deck.schema.json), and the MCP slides tools use the same
//! definitions with the references written out ([`inline`]).

use serde_json::{Map, Value, json};

fn r(name: &str) -> Value {
    json!({"$ref": format!("#/$defs/{name}")})
}

fn with(mut base: Map<String, Value>, more: Value) -> Map<String, Value> {
    if let Value::Object(m) = more {
        base.extend(m);
    }
    base
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// Character formatting keys (elements, paragraphs, runs, cells).
fn style_props() -> Map<String, Value> {
    obj(json!({
        "font": {"type": "string", "description": "Typeface, e.g. \"Inter\" (default: the theme's body font)."},
        "size": {"type": ["number", "string"], "description": "Font size in points, e.g. 18."},
        "bold": {"type": "boolean"},
        "italic": {"type": "boolean"},
        "underline": {"type": "boolean"},
        "strike": {"type": "boolean", "description": "Strikethrough."},
        "color": r("color"),
        "caps": {"type": "boolean", "description": "All capitals (the text is written in capitals)."},
        "letter_spacing": {"type": ["number", "string"], "description": "Character spacing in points, e.g. 1."},
    }))
}

/// A named text style ([`super::slides_style`]).
fn style_ref() -> Value {
    json!({"type": "string", "description": "A text style of the deck (eyebrow, title, subtitle, body, small, label, card_title, stat, ...); keys given beside it win."})
}

/// Paragraph settings (a whole text, or one paragraph).
fn para_props() -> Map<String, Value> {
    obj(json!({
        "align": {"type": "string", "enum": ["left", "center", "right", "justify"]},
        "level": {"type": "integer", "minimum": 0, "maximum": 8, "description": "Outline level (indents bullets)."},
        "bullet": {"type": ["boolean", "string"], "description": "true (•), false/\"none\", \"number\" (1. 2. 3.), one character such as \"–\", or a numbering scheme such as \"alphaLcParenR\"."},
        "line_spacing": {"type": ["number", "string"], "description": "A multiple of single spacing (1.2) or points (\"20pt\")."},
        "space_before": {"type": ["number", "string"], "description": "Points before the paragraph."},
        "space_after": {"type": ["number", "string"], "description": "Points after the paragraph."},
    }))
}

/// How text sits in its shape.
fn text_layout_props() -> Map<String, Value> {
    obj(json!({
        "text": r("text"),
        "anchor": {"type": "string", "enum": ["top", "middle", "bottom"], "description": "Vertical position of the text (shapes: middle, text boxes: top)."},
        "insets": {"description": "Space between the edge and the text: one length or [left, top, right, bottom] (default 0.1in left/right, 0.05in top/bottom).", "anyOf": [r("length"), {"type": "array", "items": r("length"), "minItems": 4, "maxItems": 4}]},
        "autofit": {"type": "string", "enum": ["none", "shrink", "resize"], "description": "none: text may overflow (default); shrink: PowerPoint shrinks text that overflows; resize: the shape grows to fit."},
        "wrap": {"type": "boolean", "description": "Wrap lines at the shape's width (default true)."},
        "vertical": {"type": "string", "enum": ["horizontal", "down", "up"], "description": "Text direction: turned to read downwards or upwards."},
    }))
}

fn box_props(what: &str) -> Map<String, Value> {
    obj(json!({
        "name": {"type": "string", "description": format!("The {what}'s name (shown in the selection pane).")},
        "x": r("length"),
        "y": r("length"),
        "w": r("length"),
        "h": r("length"),
        "rotation": {"type": "number", "description": "Degrees clockwise."},
        "flip_h": {"type": "boolean"},
        "flip_v": {"type": "boolean"},
    }))
}

fn geometry_props() -> Map<String, Value> {
    obj(json!({
        "geometry": {"type": "string", "description": "A DrawingML preset shape: rect (default), roundRect, ellipse (or oval), triangle, diamond, chevron, homePlate (arrow pentagon), pentagon, hexagon, rightArrow, leftArrow, star5, flowChartProcess, ... (all 187 ECMA-376 presets)."},
        "radius": {"description": "Corner radius of a rounded rectangle (sets geometry roundRect when no geometry is given).", "allOf": [r("length")]},
        "adjust": {"type": "object", "additionalProperties": {"type": "number"}, "description": "The preset's adjust values as written in the file, e.g. {\"adj\": 25000} (chevron point depth, 1/1000 % of the shorter side)."},
    }))
}

/// Keys of a shape or text box.
pub fn shape_props() -> Map<String, Value> {
    let mut m = box_props("shape");
    m.extend(geometry_props());
    m.extend(obj(json!({
        "fill": r("fill"),
        "line": r("line"),
    })));
    m.extend(text_layout_props());
    m.extend(para_props());
    m.extend(style_props());
    m.insert("style".into(), style_ref());
    m
}

/// Keys of a line.
pub fn line_props() -> Map<String, Value> {
    obj(json!({
        "name": {"type": "string"},
        "x1": r("length"),
        "y1": r("length"),
        "x2": r("length"),
        "y2": r("length"),
        "line": r("line"),
        "color": r("color"),
        "width": r("length"),
        "dash": {"type": "string", "enum": slides_engine::author::DASHES},
        "head": r("arrow"),
        "tail": r("arrow"),
        "cap": {"type": "string", "enum": ["flat", "round"]},
    }))
}

/// Keys of a picture.
pub fn picture_props() -> Map<String, Value> {
    let mut m = box_props("picture");
    m.extend(geometry_props());
    m.extend(obj(json!({
        "path": {"type": "string", "description": "A PNG or JPEG file: absolute (~ allowed) or, in a deck file, relative to it."},
        "crop": {"type": "object", "additionalProperties": false, "description": "Fractions (0.1) or percentages (\"10%\") cut off each side.",
            "properties": {"left": r("fraction"), "top": r("fraction"), "right": r("fraction"), "bottom": r("fraction")}},
        "fit": {"type": "string", "enum": ["stretch", "contain", "cover"], "description": "With both w and h: stretch to the box (default), fit inside it keeping the aspect (contain), or fill it and crop the rest (cover). With one of w/h the other follows the picture's aspect; with neither, its size at 96 dpi."},
        "keep_aspect": {"type": "boolean", "description": "Same as fit: contain."},
        "line": r("line"),
        "alt": {"type": "string", "description": "Alternative text."},
    })));
    m
}

/// Keys of a table.
pub fn table_props() -> Map<String, Value> {
    let mut m = obj(json!({
        "name": {"type": "string"},
        "x": r("length"),
        "y": r("length"),
        "w": {"description": "Table width: columns are scaled to it.", "allOf": [r("length")]},
        "h": {"description": "Table height when no row heights are given (split evenly).", "allOf": [r("length")]},
        "columns": {"description": "Column widths, or a number of equal columns (needs w). Default: as many as the first row's cells, splitting w.",
            "anyOf": [{"type": "array", "items": r("length")}, {"type": "integer", "minimum": 1}]},
        "rows": {"type": "array", "description": "Rows of cells: text, numbers, cell objects, or null for a cell covered by a span.",
            "items": {"type": "array", "items": r("cell")}},
        "row_height": r("length"),
        "row_heights": {"type": "array", "items": r("length")},
        "header": {"type": "boolean", "description": "The first row is a header: bold, header_color on header_fill (default true)."},
        "banded": {"type": "boolean", "description": "Every other body row gets band_fill (default false)."},
        "header_fill": r("fill"),
        "header_color": r("color"),
        "fill": {"description": "Fill of body cells (default none).", "allOf": [r("fill")]},
        "band_fill": r("fill"),
        "border": {"description": "Every cell's borders (default 1 pt #BFBFBF; \"none\" for none).", "allOf": [r("line")]},
        "cell_margins": {"description": "One length or [left, top, right, bottom].", "anyOf": [r("length"), {"type": "array", "items": r("length"), "minItems": 4, "maxItems": 4}]},
        "style_id": {"type": "string", "description": "A PowerPoint table style GUID, e.g. {5C22544A-7EE6-4342-B048-85BDC9FD1C3A}."},
        "align": {"type": "string", "enum": ["left", "center", "right", "justify"]},
        "anchor": {"type": "string", "enum": ["top", "middle", "bottom"]},
    }));
    m.extend(style_props());
    m.remove("strike");
    m.insert("style".into(), style_ref());
    m.insert("preset".into(), json!({"type": "string", "enum": ["clean"], "description": "clean: a rounded header bar, soft banded rows, no grid lines, colours from the palette; x, y, w default to the content area."}));
    m.insert("first_bold".into(), json!({"type": "boolean", "description": "With preset clean: the first column bold (default true)."}));
    m
}

/// Keys of a chart (see [`super::slides_chart`]).
pub fn chart_props() -> Map<String, Value> {
    let labels = json!({
        "description": "Data labels: true (values; pie/doughnut: percentages), false, or {show: value|percent|category|series or a list, position: center|inside_end|inside_base|outside_end|left|right|above|below|best_fit, format (e.g. \"0.0%\"), size, bold, italic, color (#RRGGBB)}.",
        "anyOf": [{"type": "boolean"}, {"type": "object", "additionalProperties": false, "properties": {
            "show": {"anyOf": [{"type": "string", "enum": ["value", "percent", "category", "series"]}, {"type": "array", "items": {"type": "string", "enum": ["value", "percent", "category", "series"]}}]},
            "position": {"type": "string", "enum": ["center", "inside_end", "inside_base", "outside_end", "left", "right", "above", "below", "best_fit"]},
            "format": {"type": "string"},
            "size": {"type": ["number", "string"]},
            "bold": {"type": "boolean"},
            "italic": {"type": "boolean"},
            "color": {"type": "string"},
        }}],
    });
    let axis = |what: &str| {
        json!({"type": "object", "additionalProperties": false, "description": what, "properties": {
            "title": {"type": "string"},
            "min": {"type": "number"},
            "max": {"type": "number"},
            "major_unit": {"type": "number", "description": "Distance between gridlines / tick labels."},
            "gridlines": {"type": "boolean", "description": "Value axis: major gridlines (default true)."},
            "number_format": {"type": "string", "description": "Tick label format, e.g. \"#,##0\" or \"0%\" (default: the values' number_format)."},
            "reverse": {"type": "boolean", "description": "Values (categories) in reverse order."},
        }})
    };
    let mut m = box_props("chart");
    for k in ["rotation", "flip_h", "flip_v"] {
        m.remove(k);
    }
    m.extend(obj(json!({
        "chart": {"type": "string", "enum": super::slides_chart::CHART_TYPES, "description": "Chart type. combo: columns with lines/areas (series `type`, `axis`)."},
        "stacked": {"anyOf": [{"type": "boolean"}, {"type": "string", "enum": ["stacked", "percent"]}], "description": "Column, bar, line, area: stack the series (true / \"stacked\") or stack to 100% (\"percent\")."},
        "categories": {"type": "array", "items": {"type": ["string", "number"]}, "description": "Labels along the category axis (scatter: the x values, numbers)."},
        "series": {"type": "array", "minItems": 1, "items": {"type": "object", "additionalProperties": false, "required": ["values"], "properties": {
            "name": {"type": "string"},
            "values": {"type": "array", "items": {"type": ["number", "null"]}, "description": "One per category; null leaves a gap."},
            "color": r("color"),
            "colors": {"type": "array", "items": {"anyOf": [r("color"), {"type": "null"}]}, "description": "Bars or slices of their own colour, one per category (null: the series colour)."},
            "type": {"type": "string", "enum": ["column", "line", "area"], "description": "Combo (or column/line/area chart): this series' type (combo default: first column, others line)."},
            "axis": {"type": "string", "enum": ["primary", "secondary"]},
            "line": {"description": "A line series' line: width, dash, colour.", "allOf": [r("line")]},
            "data_labels": labels.clone(),
        }}},
        "title": {"type": "string"},
        "legend": {"type": "string", "enum": ["none", "bottom", "top", "right", "left"], "description": "Default: bottom with several series or a pie, else none."},
        "data_labels": labels,
        "number_format": {"type": "string", "description": "Excel number format of the values (labels and value axis), written as Excel stores it: \"#,##0.0\", \"0%\", \"\\\"Rp\\\" #,##0\". `,` is the thousands and `.` the decimal separator in the code; PowerPoint and Excel show them in the viewer's locale (Indonesian: 1.234,5)."},
        "axis": axis("The value axis (horizontal in a bar chart)."),
        "category_axis": axis("The category axis: title, reverse (scatter: the x axis, with min/max/major_unit)."),
        "gap_width": {"type": "number", "minimum": 0, "maximum": 500, "description": "Column/bar: gap between categories, % of a bar (default 150)."},
        "overlap": {"type": "number", "minimum": -100, "maximum": 100, "description": "Column/bar: overlap of a category's bars in % (stacked: 100)."},
        "hole": {"type": "number", "minimum": 10, "maximum": 90, "description": "Doughnut hole size in % (default 50)."},
        "font": {"type": "object", "additionalProperties": false, "description": "The chart's text (default: the deck's typeface and colour).", "properties": {
            "font": {"type": "string"}, "size": {"type": ["number", "string"]}, "color": r("color"), "bold": {"type": "boolean"}, "italic": {"type": "boolean"}}},
        "colors": {"type": "array", "items": {"anyOf": [r("color"), {"type": "null"}]}, "description": "A one-series column, bar, pie or doughnut chart: each bar's or slice's colour (null: automatic), e.g. to highlight bars below target."},
        "target_line": {"description": "A dashed horizontal line at a value (column, line, area, combo): a number or {value, label (default Target), color, width, dash}. It is a line series, so it shows in the legend and in Edit Data.",
            "anyOf": [{"type": "number"}, {"type": "object", "additionalProperties": false, "required": ["value"], "properties": {
                "value": {"type": "number"}, "label": {"type": "string"}, "color": r("color"), "width": r("length"), "dash": {"type": "string", "enum": slides_engine::author::DASHES}}}]},
    })));
    m
}

fn closed(ty: &str, props: Map<String, Value>, required: &[&str], description: &str) -> Value {
    let mut props = props;
    props.insert("type".into(), json!({"const": ty}));
    let mut req = vec!["type"];
    req.extend_from_slice(required);
    json!({
        "type": "object",
        "description": description,
        "properties": props,
        "required": req,
        "additionalProperties": false,
    })
}

fn defs() -> Value {
    let mut para = with(
        obj(json!({
            "text": {"type": "string"},
            "runs": {"type": "array", "items": r("run")},
        })),
        Value::Object(para_props()),
    );
    para.extend(style_props());
    para.insert("style".into(), style_ref());
    let mut run = obj(json!({
        "text": {"type": "string", "description": "A new line is a line break in the paragraph."},
        "field": {"type": "string", "enum": ["slide_number"], "description": "A field PowerPoint keeps up to date (text optional)."},
    }));
    run.extend(style_props());
    run.insert("style".into(), style_ref());
    let mut cell = obj(json!({
        "text": r("text"),
        "fill": r("fill"),
        "border": r("line"),
        "margins": {"anyOf": [r("length"), {"type": "array", "items": r("length"), "minItems": 4, "maxItems": 4}]},
        "col_span": {"type": "integer", "minimum": 1},
        "row_span": {"type": "integer", "minimum": 1},
        "anchor": {"type": "string", "enum": ["top", "middle", "bottom"]},
    }));
    cell.extend(para_props());
    cell.extend(style_props());
    cell.insert("style".into(), style_ref());
    let mut defs = json!({
        "length": {
            "description": "A length: a number in `units` (default emu: 914400 per inch, 12700 per point, 360000 per cm) or a string with a unit: \"1.5in\", \"2.5cm\", \"40mm\", \"18pt\", \"120px\", \"914400emu\".",
            "type": ["number", "string"],
        },
        "fraction": {"type": ["number", "string"], "description": "0 to 1, or a percentage such as \"25%\"."},
        "color": {
            "description": "#RRGGBB (also #RGB, or #RRGGBBAA with opacity), a theme colour (accent1-accent6, tx1, tx2, bg1, bg2, dk1, lt1, dk2, lt2, hlink, folHlink), or {color, opacity}.",
            "anyOf": [
                {"type": "string"},
                {"type": "object", "properties": {"color": {"type": "string"}, "opacity": r("fraction")}, "required": ["color"], "additionalProperties": false},
            ],
        },
        "fill": {
            "description": "\"none\", a colour, or a gradient.",
            "anyOf": [
                r("color"),
                {"type": "object", "additionalProperties": false, "required": ["gradient"], "properties": {"gradient": {
                    "type": "object", "additionalProperties": false, "required": ["stops"],
                    "properties": {
                        "type": {"type": "string", "enum": ["linear", "radial"]},
                        "angle": {"type": "number", "description": "Degrees: 0 left to right, 90 top to bottom (default)."},
                        "stops": {"type": "array", "minItems": 2, "maxItems": 10, "items": {"anyOf": [
                            r("color"),
                            {"type": "object", "additionalProperties": false, "required": ["color", "position"],
                             "properties": {"color": {"type": "string"}, "opacity": r("fraction"), "position": r("fraction")}},
                        ]}, "description": "Colours spread evenly, or {color, position}."},
                    },
                }}},
            ],
        },
        "arrow": {
            "description": "An arrowhead: a type, or {type, width, length} (small, medium, large).",
            "anyOf": [
                {"type": "string", "enum": slides_engine::author::ARROW_KINDS},
                {"type": "object", "additionalProperties": false, "properties": {
                    "type": {"type": "string", "enum": slides_engine::author::ARROW_KINDS},
                    "width": {"type": "string", "enum": ["small", "medium", "large"]},
                    "length": {"type": "string", "enum": ["small", "medium", "large"]},
                }},
            ],
        },
        "line": {
            "description": "An outline: \"none\", a colour, or {color, width, dash, head, tail, cap}. Shapes without one get the theme's (accent 1, darker).",
            "anyOf": [
                r("color"),
                {"type": "object", "additionalProperties": false, "properties": {
                    "color": r("color"),
                    "opacity": r("fraction"),
                    "width": r("length"),
                    "dash": {"type": "string", "enum": slides_engine::author::DASHES},
                    "head": {"description": "Arrowhead at the start (x1, y1).", "allOf": [r("arrow")]},
                    "tail": {"description": "Arrowhead at the end (x2, y2).", "allOf": [r("arrow")]},
                    "cap": {"type": "string", "enum": ["flat", "round"]},
                }},
            ],
        },
        "run": {
            "anyOf": [
                {"type": "string"},
                {"type": "object", "additionalProperties": false, "properties": run},
            ],
        },
        "paragraph": {
            "anyOf": [
                {"type": "string"},
                {"type": "object", "additionalProperties": false, "properties": para},
            ],
        },
        "text": {
            "description": "A string (a new line starts a paragraph), or paragraphs: strings or {text | runs, align, bullet, level, line_spacing, space_before, space_after, font, size, bold, italic, underline, strike, color}.",
            "anyOf": [
                {"type": "string"},
                {"type": "array", "items": r("paragraph")},
            ],
        },
        "cell": {
            "anyOf": [
                {"type": ["string", "number", "null"]},
                {"type": "object", "additionalProperties": false, "properties": cell},
            ],
        },
        "shape": closed("shape", shape_props(), &["w", "h"], "A shape: geometry, fill, outline and text. Default look as PowerPoint's: accent 1 fill, darker outline, white centred text."),
        "text_box": closed("text", shape_props(), &["w", "h"], "A text box: no fill or outline unless given, text from the top left."),
        "line_element": closed("line", line_props(), &["x1", "y1", "x2", "y2"], "A straight line from (x1, y1) to (x2, y2), optionally with arrowheads."),
        "picture": closed("picture", picture_props(), &["path"], "A picture from a PNG or JPEG file, optionally cropped and cut to a shape."),
        "table": closed("table", table_props(), &["rows"], "A table."),
        "chart": closed("chart", chart_props(), &["chart", "w", "h", "categories", "series"], "A real, editable chart (PowerPoint's Edit Data opens its data): column, bar, line, area, pie, doughnut, scatter or combo."),
        "icon": super::slides_icons::schema(),
        "element": {
            "description": "Something on a slide, by `type`: an element, or a component that expands into several.",
            "oneOf": [r("shape"), r("text_box"), r("line_element"), r("picture"), r("table"), r("chart"), r("icon"),
                r("stat_cards"), r("cards"), r("steps"), r("timeline"), r("callout"), r("numbered_list"), r("comparison"), r("badge")],
        },
        "background": {
            "description": "A colour, a gradient, {\"picture\": path}, or \"inherit\" (the layout's).",
            "anyOf": [
                r("fill"),
                {"type": "object", "additionalProperties": false, "required": ["picture"], "properties": {"picture": {"type": "string"}}},
            ],
        },
        "slide": {
            "type": "object", "additionalProperties": false,
            "properties": {
                "layout": {"type": ["string", "integer"], "description": "Title Slide, Title and Content, Title Only or Blank (name or number). Default: Title and Content with bullets, Title Only with a title, else Blank."},
                "title": {"type": "string"},
                "bullets": {"type": "array", "items": {"anyOf": [{"type": "string"}, {"type": "object", "additionalProperties": false, "required": ["text"], "properties": {"text": {"type": "string"}, "level": {"type": "integer", "minimum": 0, "maximum": 8}}}]}},
                "background": r("background"),
                "elements": {"type": "array", "items": r("element"), "description": "Drawn in order: later ones on top."},
                "notes": {"type": "string", "description": "Speaker notes."},
            },
        },
    });
    for (k, v) in super::slides_components::schema_defs()
        .into_iter()
        .chain(super::slides_style::schema_defs())
    {
        defs[k] = v;
    }
    if let Some(Value::Object(props)) = defs.pointer_mut("/slide/properties") {
        props.extend(super::slides_style::slide_props());
    }
    defs
}

/// The deck schema (`kantor slides build`).
pub fn deck() -> Value {
    let mut d = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://kantor.paparanai.id/schema/slides-deck.schema.json",
        "title": "Kantor slide deck",
        "description": "A presentation for `kantor slides build deck.json -o deck.pptx`. The same elements go to the MCP slides tools.",
        "type": "object",
        "additionalProperties": false,
        "required": ["slides"],
        "properties": {
            "size": {"description": "Slide size: 16:9 (default, 13.333 × 7.5 in), 4:3, 16:10, a4, letter, or {w, h}.",
                "anyOf": [{"type": "string", "enum": ["16:9", "widescreen", "4:3", "16:10", "a4", "letter"]},
                          {"type": "object", "additionalProperties": false, "required": ["w", "h"], "properties": {"w": r("length"), "h": r("length")}}]},
            "units": units(),
            "theme": {"type": "object", "additionalProperties": false, "properties": {
                "name": {"type": "string"},
                "fonts": {"type": "object", "additionalProperties": false, "properties": {
                    "heading": {"type": "string", "description": "Headings (titles) typeface (default Calibri Light)."},
                    "body": {"type": "string", "description": "Body text typeface (default Calibri)."}}},
                "colors": {"type": "object", "additionalProperties": false, "description": "Theme colours as #RRGGBB.",
                    "properties": slides_engine::author::THEME_COLOR_NAMES.iter().map(|n| ((*n).to_owned(), json!({"type": "string"}))).collect::<Map<String, Value>>()},
            }},
            "text": {"type": "object", "additionalProperties": false, "description": "Default text formatting of every element (colours may be palette names, the font heading or body).", "properties": style_props()},
            "background": {"description": "Every slide's background unless the slide has its own.", "allOf": [r("background")]},
            "slides": {"type": "array", "minItems": 1, "items": r("slide")},
        },
        "$defs": defs(),
    });
    if let Some(Value::Object(props)) = d.get_mut("properties") {
        props.extend(super::slides_style::deck_props());
    }
    d
}

/// The `units` argument.
pub fn units() -> Value {
    json!({"type": "string", "enum": ["emu", "pt", "in", "cm", "mm", "px"], "description": "What bare numbers in lengths mean (default emu). Strings such as \"1in\" always say their unit."})
}

/// `v` with every `{"$ref": "#/$defs/x"}` replaced by the definition
/// (merged with any keys beside the reference).
///
/// For MCP tool lists the shared definitions are short (their type and a
/// description; the full schema is `kantor slides schema`): written out
/// in full they would make the tool list too long for an agent's context.
pub fn inline(v: &Value) -> Value {
    let mut defs = defs();
    for (name, short) in compact() {
        defs[name] = short;
    }
    inline_with(v, &defs, 0)
}

/// Short forms of the definitions repeated across the MCP tools.
fn compact() -> Vec<(&'static str, Value)> {
    let full = defs();
    let desc = |name: &str| full[name]["description"].clone();
    vec![
        (
            "color",
            json!({"type": ["string", "object"], "description": desc("color")}),
        ),
        (
            "fill",
            json!({"type": ["string", "object"], "description": "\"none\", a colour, or {\"gradient\": {\"type\": linear|radial, \"angle\": degrees, \"stops\": [colours, or {color, position}]}}."}),
        ),
        (
            "line",
            json!({"type": ["string", "object"], "description": "\"none\", a colour, or {color, opacity, width, dash (solid, dash, sysDash, dot, lgDash, dashDot, ...), head, tail (arrowheads: triangle, stealth, diamond, oval, arrow, or {type, width, length} small|medium|large), cap (flat|round)}."}),
        ),
        (
            "arrow",
            json!({"type": ["string", "object"], "description": desc("arrow")}),
        ),
        (
            "text",
            json!({"type": ["string", "array"], "description": "A string (a new line starts a paragraph), or paragraphs: strings or {text | runs, align, bullet, level, line_spacing, space_before, space_after, font, size, bold, italic, underline, strike, color}; runs are strings or {text, font, size, bold, italic, underline, strike, color}."}),
        ),
        (
            "cell",
            json!({"type": ["string", "number", "null", "object"], "description": "Text, a number, null (covered by a span), or {text, fill, border, margins, col_span, row_span, align, anchor, font, size, bold, italic, underline, color, ...}."}),
        ),
        (
            "element",
            json!({"type": "object", "description": "An element with its `type`: shape, text, line (keys as slides_add_shape), picture (as slides_add_picture), table (as slides_add_table), chart (as slides_add_chart), icon {icon, x, y, size, color}, or a component: stat_cards {items: [{value, label, detail, highlight}]}, cards {items: [{title, text, bullets, icon}], columns}, steps {items, current}, timeline {items: [{label, title, text}], current, style: dots|chevrons}, callout {tone: info|warning|success|danger|note, title, text, icon}, numbered_list {items, columns, marker: badge|number}, comparison {columns: [{title, tag, items, highlight}]}, badge {text|icon, x, y, size}. Components take x, y, w, h (default: the content area)."}),
        ),
        (
            "background",
            json!({"type": ["string", "object"], "description": desc("background")}),
        ),
    ]
}

fn inline_with(v: &Value, defs: &Value, depth: usize) -> Value {
    match v {
        Value::Object(m) => {
            if let Some(Value::String(path)) = m.get("$ref")
                && let Some(name) = path.strip_prefix("#/$defs/")
                && depth < 16
            {
                let mut out = match inline_with(&defs[name], defs, depth + 1) {
                    Value::Object(o) => o,
                    other => return other,
                };
                for (k, x) in m {
                    if k != "$ref" {
                        out.insert(k.clone(), inline_with(x, defs, depth));
                    }
                }
                return Value::Object(out);
            }
            Value::Object(
                m.iter()
                    .map(|(k, x)| (k.clone(), inline_with(x, defs, depth)))
                    .collect(),
            )
        }
        Value::Array(a) => Value::Array(a.iter().map(|x| inline_with(x, defs, depth)).collect()),
        other => other.clone(),
    }
}

/// A definition with its references written out.
pub fn def(name: &str) -> Value {
    inline(&r(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn the_schema_file_is_current() {
        let file = include_str!("../../../../docs/slides-deck.schema.json");
        let now = serde_json::to_string_pretty(&deck()).unwrap() + "\n";
        assert!(
            file == now,
            "docs/slides-deck.schema.json is out of date: run `kantor slides schema > docs/slides-deck.schema.json`"
        );
    }

    #[std::prelude::v1::test]
    fn inlined_definitions_have_no_references() {
        let e = def("table");
        assert!(!e.to_string().contains("$ref"));
        assert!(e["properties"]["rows"].is_object());
        // The tool list stays small enough for an agent's context.
        let all = serde_json::to_string(&crate::mcp::tools::list()).unwrap();
        assert!(all.len() < 80_000, "{} bytes", all.len());
    }
}
