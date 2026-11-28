//! The JSON Schema of documents and blocks ([`super::docs_author`]):
//! `kantor docs schema` prints [`document`] (kept as
//! docs/docs-json.schema.json); the MCP `docs_build` and `docs_edit` tools
//! describe their arguments with it.

use serde_json::{Map, Value, json};

fn r(name: &str) -> Value {
    json!({"$ref": format!("#/$defs/{name}")})
}

fn obj(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn length() -> Value {
    json!({"type": ["number", "string"], "description": "Points (12), or a length with its unit: \"2.5cm\", \"18mm\", \"1in\", \"12pt\"."})
}

fn width() -> Value {
    json!({"type": ["number", "string"], "description": "A length, or a share of the text width: \"60%\" (default 100%)."})
}

fn color() -> Value {
    json!({"type": "string", "description": "#RRGGBB or a palette name: accent, ink, muted, rule, tint, bullet, link, accent2-accent6, white, black."})
}

fn text() -> Value {
    json!({
        "description": "Paragraph text: a string with light markdown (**bold**, *italic*, `code`, [text](url), [^footnote], {ref:id}, {ref:id:page}, $inline math$, $$display math$$ (LaTeX); \\ escapes; \\n is a line break), or a list of runs: strings and {text, bold, italic, underline, strike, caps, small_caps, color, highlight, size, font, style, superscript, subscript, code, link, footnote, ref, show, field, result, math, display}.",
        "anyOf": [{"type": "string"}, {"type": "array", "items": {"anyOf": [{"type": "string"}, r("run")]}}]
    })
}

fn run() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "text": {"type": "string"},
            "bold": {"type": "boolean"}, "italic": {"type": "boolean"}, "underline": {"type": "boolean"},
            "strike": {"type": "boolean"}, "caps": {"type": "boolean"}, "small_caps": {"type": "boolean"},
            "color": color(), "highlight": {"type": "string"},
            "size": {"type": ["number", "string"], "description": "Points."},
            "font": {"type": "string", "description": "heading, body, mono, or a font name."},
            "style": {"type": "string", "description": "A character style (Strong, Emphasis, Code, Hyperlink, or one of the document's)."},
            "superscript": {"type": "boolean"}, "subscript": {"type": "boolean"}, "code": {"type": "boolean"},
            "link": {"type": "string", "description": "A URL, or #id for a heading, figure or table of the document."},
            "footnote": {"description": "A footnote placed after the text.", "anyOf": [{"type": "string"}, {"type": "array"}]},
            "ref": {"type": "string", "description": "A cross-reference to the heading, figure or table with this id (a REF / PAGEREF field)."},
            "show": {"type": "string", "enum": ["number", "text", "page", "label", "full"], "description": "number: the heading's number (2.3); text: its words; page: its page; label: \"Figure 2\"; full: \"Figure 2: caption\". Default: number for numbered headings, text for others, label for figures and tables."},
            "field": {"type": "string", "description": "A Word field instruction, e.g. DATE \\@ \"d MMMM yyyy\"."},
            "result": {"type": "string", "description": "What the field shows until updated."},
            "math": {"type": "string", "description": "LaTeX math, e.g. \\frac{a}{b}: a Word equation (Office Math), inline unless display."},
            "display": {"type": "boolean", "description": "With math: a display equation on a line of its own."}
        }
    })
}

fn style_def() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "description": "A style's settings. Built-in names (Normal, Title, Subtitle, Heading 1-4, Quote, Caption, Figure, List Bullet, List Number, Table Text, Table Header, Callout, Callout Title, Cover Label, Cover Meta, TOC Heading, TOC 1-3, Header, Footer, Footnote Text, No Spacing, Strong, Emphasis, Code, Hyperlink) change those; other names add a style.",
        "properties": {
            "type": {"type": "string", "enum": ["paragraph", "character"]},
            "based_on": {"type": "string"}, "next": {"type": "string"},
            "font": {"type": "string", "description": "heading, body, mono, or a font name."},
            "size": {"type": ["number", "string"]}, "bold": {"type": "boolean"}, "italic": {"type": "boolean"},
            "underline": {"type": "boolean"}, "strike": {"type": "boolean"}, "caps": {"type": "boolean"},
            "small_caps": {"type": "boolean"}, "color": color(), "highlight": {"type": "string"},
            "letter_spacing": {"type": ["number", "string"], "description": "Points."},
            "align": {"type": "string", "enum": ["left", "center", "right", "justify"]},
            "space_before": {"type": ["number", "string"], "description": "Points."},
            "space_after": {"type": ["number", "string"], "description": "Points."},
            "line_spacing": {"type": "number", "description": "A multiple of single spacing (1.15)."},
            "indent": length(), "indent_right": length(),
            "first_line": {"type": ["number", "string"], "description": "First-line indent; negative for hanging."},
            "keep_with_next": {"type": "boolean"}, "keep_together": {"type": "boolean"},
            "page_break_before": {"type": "boolean"},
            "outline_level": {"type": "integer", "minimum": 1, "maximum": 9}
        }
    })
}

fn hf() -> Value {
    json!({
        "description": "A header or footer: {left, center, right} texts with tokens {page}, {pages}, {section_pages}, {title}, {subject}, {author}, {company}, {heading} (the page's Heading 1), {date}, {version}, {status}; a string (right-aligned); or \"none\" / null.",
        "anyOf": [
            {"type": "object", "additionalProperties": false, "properties": {"left": {"type": "string"}, "center": {"type": "string"}, "right": {"type": "string"}}},
            {"type": "string"}, {"type": "null"}, {"type": "boolean", "const": false}
        ]
    })
}

fn page() -> Value {
    json!({
        "type": "object", "additionalProperties": false,
        "properties": {
            "size": {"description": "A4 (default), Letter, Legal, A5, A3, F4, or {w, h}.",
                     "anyOf": [{"type": "string"}, {"type": "object", "properties": {"w": length(), "h": length()}, "required": ["w", "h"], "additionalProperties": false}]},
            "orientation": {"type": "string", "enum": ["portrait", "landscape"]},
            "margins": {"description": "One length for all, or {top, bottom, left, right}.",
                        "anyOf": [length(), {"type": "object", "additionalProperties": false, "properties": {"top": length(), "bottom": length(), "left": length(), "right": length()}}]},
            "header": {"description": "Distance from the page top to the header.", "type": ["number", "string"]},
            "footer": {"description": "Distance from the page bottom to the footer.", "type": ["number", "string"]}
        }
    })
}

fn block_of(ty: &str, desc: &str, props: Value, required: &[&str]) -> Value {
    let mut p = obj(json!({"type": {"const": ty}}));
    p.extend(obj(props));
    let mut req = vec!["type"];
    req.extend_from_slice(required);
    json!({"type": "object", "description": desc, "additionalProperties": false, "properties": p, "required": req})
}

fn para_fmt() -> Map<String, Value> {
    obj(json!({
        "style": {"type": "string", "description": "A paragraph style by name (Normal, Quote, Caption, or one in `styles`)."},
        "align": {"type": "string", "enum": ["left", "center", "right", "justify"]},
        "space_before": {"type": ["number", "string"]}, "space_after": {"type": ["number", "string"]},
        "line_spacing": {"type": "number"}, "indent": length(), "indent_right": length(), "first_line": length(),
        "keep_with_next": {"type": "boolean"}, "keep_together": {"type": "boolean"}, "page_break_before": {"type": "boolean"},
        "bold": {"type": "boolean"}, "italic": {"type": "boolean"}, "underline": {"type": "boolean"},
        "color": color(), "size": {"type": ["number", "string"]}, "font": {"type": "string"},
        "highlight": {"type": "string"}, "caps": {"type": "boolean"}, "small_caps": {"type": "boolean"}
    }))
}

fn blocks_defs() -> Vec<(&'static str, Value)> {
    let mut para = obj(json!({"text": text()}));
    para.extend(para_fmt());
    let item = json!({"anyOf": [text(), {"type": "object", "additionalProperties": false, "properties": {
        "text": text(), "items": {"type": "array"}, "bullets": {"type": "array"}, "numbers": {"type": "array"}}}]});
    let cell = json!({"anyOf": [{"type": "string"}, {"type": "number"}, {"type": "null"}, {"type": "array"},
        {"type": "object", "additionalProperties": false, "properties": {
            "text": text(), "col_span": {"type": "integer", "minimum": 1}, "row_span": {"type": "integer", "minimum": 1},
            "align": {"type": "string", "enum": ["left", "center", "right", "justify"]},
            "valign": {"type": "string", "enum": ["top", "middle", "bottom"]},
            "bold": {"type": "boolean"}, "italic": {"type": "boolean"}, "color": color(), "fill": color(),
            "bullets": {"type": "array"}}}],
        "description": "A cell: text, a number, null (under a span), or {text, col_span, row_span, align, valign, bold, italic, color, fill, bullets}."});
    let caption = json!({"description": "A numbered caption (Figure 3. …) below a figure, above a table.", "anyOf": [{"type": "string"}, {"type": "array"}]});
    let id = json!({"type": "string", "description": "Name it for cross-references ({ref:id}); needs a caption on figures and tables."});
    let mut chart = super::slides_schema::def("chart");
    if let Some(p) = chart.get_mut("properties").and_then(Value::as_object_mut) {
        for k in ["x", "y", "w", "h", "name"] {
            p.remove(k);
        }
        p.insert("type".into(), json!({"const": "chart"}));
        p.insert("width".into(), width());
        p.insert(
            "height".into(),
            json!({"type": ["number", "string"], "description": "Default: 55% of the width."}),
        );
        p.insert("caption".into(), caption.clone());
        p.insert("id".into(), id.clone());
        p.insert(
            "alt".into(),
            json!({"type": "string", "description": "Alt text (default: the caption)."}),
        );
        p.insert(
            "align".into(),
            json!({"type": "string", "enum": ["left", "center", "right"]}),
        );
    }
    if let Some(req) = chart.get_mut("required").and_then(Value::as_array_mut) {
        req.retain(|k| !matches!(k.as_str(), Some("x" | "y" | "w" | "h")));
    }
    chart["description"] = json!(
        "A real Word chart with its data in an embedded workbook (the slides chart JSON: chart, categories, series, title, legend, data_labels, number_format, axis, …)."
    );
    vec![
        (
            "heading",
            block_of(
                "heading",
                "A heading: real Heading 1-4 style, numbered (1, 1.1, 1.1.1) unless numbered is false. Shorthand {\"h2\": \"Text\"}.",
                json!({
            "text": text(), "level": {"type": "integer", "minimum": 1, "maximum": 4, "default": 1},
            "id": id.clone(), "numbered": {"type": "boolean"}, "page_break_before": {"type": "boolean"}}),
                &["text"],
            ),
        ),
        (
            "paragraph",
            block_of(
                "paragraph",
                "A paragraph. A plain string is one too; shorthand {\"p\": …}.",
                Value::Object(para),
                &["text"],
            ),
        ),
        (
            "list",
            block_of(
                "list",
                "A bullet or numbered list (real Word numbering), nested with items. Shorthands {\"bullets\": […]}, {\"numbers\": […]}.",
                json!({
            "kind": {"type": "string", "enum": ["bullet", "number"]}, "items": {"type": "array", "items": item},
            "start": {"type": "integer", "minimum": 0}, "continue": {"type": "boolean", "description": "Numbers go on from the previous numbered list."}}),
                &["items"],
            ),
        ),
        (
            "table",
            block_of(
                "table",
                "A table in the document's table style: header row repeated on each page, numbers right-aligned, merged cells.",
                json!({
            "columns": {"description": "Column widths: weights [3, 1, 1], lengths or percentages; or a count.", "anyOf": [{"type": "integer"}, {"type": "array"}]},
            "header": {"type": "array", "description": "The header row (or rows: an array of rows)."},
            "rows": {"type": "array", "items": {"type": "array", "items": cell}},
            "align": {"description": "Per column (or all): left, center, right.", "anyOf": [{"type": "string"}, {"type": "array"}]},
            "caption": caption.clone(), "id": id.clone(), "width": width(),
            "look": {"type": "string", "enum": ["rules", "grid", "header", "plain"]},
            "total": {"type": "boolean", "description": "The last row is a total (bold, rule above)."},
            "header_rows": {"type": "integer", "minimum": 0}, "repeat_header": {"type": "boolean"}}),
                &["rows"],
            ),
        ),
        (
            "picture",
            block_of(
                "picture",
                "A PNG or JPEG picture in line, centred, with an optional caption.",
                json!({
            "path": {"type": "string", "description": "Relative to the JSON file (MCP: absolute)."},
            "width": width(), "height": length(), "alt": {"type": "string"}, "caption": caption.clone(), "id": id.clone(),
            "align": {"type": "string", "enum": ["left", "center", "right"]}}),
                &["path"],
            ),
        ),
        ("chart", chart),
        (
            "callout",
            block_of(
                "callout",
                "A boxed note: tinted, with a bar down the left.",
                json!({
            "title": text(), "text": {"description": "Text, or an array of paragraphs (text, runs, {\"bullets\": […]})."},
            "fill": color(), "bar": color()}),
                &["text"],
            ),
        ),
        (
            "quote",
            block_of(
                "quote",
                "A quotation in the Quote style, with its source below.",
                json!({"text": text(), "source": {"type": "string"}}),
                &["text"],
            ),
        ),
        (
            "equation",
            block_of(
                "equation",
                "A display equation from LaTeX, saved as a Word equation (editable in Word); number puts Word's equation number at the right margin.",
                json!({
            "latex": {"type": "string", "description": "LaTeX math (amsmath environments too): \\frac, \\sqrt, \\sum, \\int, matrices, cases, aligned."},
            "display": {"type": "boolean", "default": true, "description": "false: an inline equation in a paragraph of its own."},
            "number": {"type": "string", "description": "Equation number, e.g. \"1\" or \"3.2\" (shown as (1))."},
            "id": id.clone()}),
                &["latex"],
            ),
        ),
        (
            "page_break",
            block_of(
                "page_break",
                "The next block starts a new page.",
                json!({}),
                &[],
            ),
        ),
        (
            "section",
            block_of(
                "section",
                "A new section from here: page size and orientation (a landscape appendix), margins, header and footer, page numbers, columns. Unset keys keep the previous section's.",
                json!({
            "orientation": {"type": "string", "enum": ["portrait", "landscape"]}, "size": {}, "margins": {},
            "header": hf(), "footer": hf(),
            "page_numbers": {"type": "object", "additionalProperties": false, "properties": {
                "start": {"type": "integer"}, "format": {"type": "string", "enum": ["decimal", "lowerRoman", "upperRoman", "lowerLetter", "upperLetter"]}}},
            "columns": {"type": "integer", "minimum": 1, "maximum": 6},
            "start": {"type": "string", "enum": ["next_page", "continuous", "odd_page", "even_page"]},
            "first_page_header": {"type": "boolean"}}),
                &[],
            ),
        ),
        (
            "toc",
            block_of(
                "toc",
                "A table of contents (a TOC field, filled in with page numbers when built).",
                json!({
            "title": {"anyOf": [{"type": "string"}, {"const": false}]}, "levels": {"type": "integer", "minimum": 1, "maximum": 4}}),
                &[],
            ),
        ),
        (
            "list_of_figures",
            block_of(
                "list_of_figures",
                "A list of the figures' captions with pages.",
                json!({"title": {"anyOf": [{"type": "string"}, {"const": false}]}}),
                &[],
            ),
        ),
        (
            "list_of_tables",
            block_of(
                "list_of_tables",
                "A list of the tables' captions with pages.",
                json!({"title": {"anyOf": [{"type": "string"}, {"const": false}]}}),
                &[],
            ),
        ),
    ]
}

fn defs() -> Map<String, Value> {
    let mut d = Map::new();
    d.insert("run".into(), run());
    d.insert("text".into(), text());
    d.insert("style".into(), style_def());
    d.insert("header_footer".into(), hf());
    d.insert("page".into(), page());
    let blocks = blocks_defs();
    let mut any: Vec<Value> = vec![json!({"type": "string", "description": "A paragraph."})];
    any.push(json!({"type": "object", "description": "A shorthand: {\"h1\"…\"h4\": text}, {\"p\": text}, {\"bullets\": […]}, {\"numbers\": […]} (with id, numbered, style… beside it).",
        "minProperties": 1, "not": {"required": ["type"]}}));
    for (name, v) in blocks {
        any.push(r(name));
        d.insert(name.into(), v);
    }
    d.insert("block".into(), json!({"anyOf": any}));
    d
}

/// The document schema (`kantor docs build`).
pub fn document() -> Value {
    let props = json!({
        "preset": {"type": "string", "enum": super::docs_look::PRESETS, "description": "report (serif headings, calm), proposal (sans, sections on new pages, cover band), letter (no cover or numbering). Default report."},
        "lang": {"type": "string", "description": "Language tag (en-US default; id-ID gives Indonesian labels)."},
        "accent": {"type": "string", "description": "The brand colour #RRGGBB: headings, numbers, rules, chart series; tints follow it."},
        "fonts": {"type": "object", "additionalProperties": false, "properties": {"heading": {"type": "string"}, "body": {"type": "string"}, "mono": {"type": "string"}}},
        "palette": {"type": "object", "additionalProperties": {"type": "string"}, "description": "Named colours over the preset's (accent, ink, muted, rule, tint, bullet, link, accent2-accent6)."},
        "styles": {"type": "object", "additionalProperties": r("style"), "description": "Style changes by name, and new styles."},
        "table": {"type": "object", "additionalProperties": false, "properties": {
            "look": {"type": "string", "enum": ["rules", "grid", "header", "plain"]}, "header_fill": color(), "rule": color(), "edge": color(), "banded": color()}},
        "callout": {"type": "object", "additionalProperties": false, "properties": {"fill": color(), "bar": color()}},
        "cover_style": {"type": "object", "additionalProperties": false, "properties": {"kind": {"type": "string", "enum": ["bar", "band", "plain", "none"]}, "color": color()}},
        "header": r("header_footer"),
        "footer": r("header_footer"),
        "page": r("page"),
        "labels": {"type": "object", "additionalProperties": false, "properties": {
            "figure": {"type": "string"}, "table": {"type": "string"}, "contents": {"type": "string"}, "figures": {"type": "string"}, "tables": {"type": "string"}}},
        "numbered_headings": {"type": "boolean"},
        "first_page_header": {"type": "boolean", "description": "false: no header or footer on the body's first page (letters)."},
        "markdown": {"type": "boolean", "description": "Read **bold**, *italic*, [link](url), [^note], {ref:id} in strings (default true)."},
        "properties": {"type": "object", "additionalProperties": false, "properties": {
            "title": {"type": "string"}, "subject": {"type": "string"}, "author": {"type": "string"}, "company": {"type": "string"},
            "keywords": {"type": "string"}, "description": {"type": "string"},
            "date": {"type": "string", "description": "YYYY-MM-DD."}, "version": {"type": "string"}, "status": {"type": "string"}}},
        "cover": {"type": "object", "additionalProperties": false, "description": "A cover page (front matter: no header or footer; the body starts at page 1).", "properties": {
            "label": text(), "title": text(), "subtitle": text(),
            "meta": {"description": "[[label, value], …] lines, e.g. [[\"Prepared for\", \"PT Example\"]].", "anyOf": [{"type": "array"}, {"type": "object"}]},
            "note": text(), "logo": {"type": "string"}, "logo_width": width()}},
        "toc": {"description": "true or {title, levels}: a table of contents after the cover.", "anyOf": [{"type": "boolean"}, {"type": "object", "additionalProperties": false, "properties": {"title": {"type": "string"}, "levels": {"type": "integer", "minimum": 1, "maximum": 4}}}]},
        "blocks": {"type": "array", "items": r("block")},
    });
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://kantor.paparanai.id/schema/docs-json.schema.json",
        "title": "Kantor document",
        "description": "A Word document for `kantor docs build doc.json -o doc.docx`; blocks also go to `kantor docs edit … insert_blocks`.",
        "type": "object",
        "additionalProperties": false,
        "properties": props,
        "$defs": defs(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn the_schema_file_is_current() {
        let file = include_str!("../../../../docs/docs-json.schema.json");
        let now = serde_json::to_string_pretty(&document()).unwrap() + "\n";
        assert!(
            file == now,
            "docs/docs-json.schema.json is out of date: run `kantor docs schema > docs/docs-json.schema.json`"
        );
    }

    #[std::prelude::v1::test]
    fn every_key_the_builder_reads_is_described() {
        let s = document();
        let props = s["properties"].as_object().unwrap();
        for k in super::super::docs_author::doc_keys() {
            assert!(props.contains_key(k), "{k}");
        }
        for t in super::super::docs_author::BLOCK_TYPES {
            assert!(s["$defs"][t].is_object(), "{t}");
        }
    }
}
