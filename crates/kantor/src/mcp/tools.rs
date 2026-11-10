//! The tools `kantor mcp` offers: names, descriptions and argument schemas.
//! The running app implements them (`workspace/agent.rs`).

use serde_json::{Value, json};

/// `(name, title, description, properties, required, read_only)`.
type Spec = (
    &'static str,
    &'static str,
    &'static str,
    fn() -> Value,
    &'static [&'static str],
    bool,
);

fn document() -> Value {
    json!({"type": "integer", "description": "Document id from list_documents. Default: the document in front."})
}

fn sheet() -> Value {
    json!({"type": ["string", "integer"], "description": "Sheet name, or its number counting from 1. Default: the sheet on screen."})
}

const SPECS: &[Spec] = &[
    (
        "list_documents",
        "List open documents",
        "List the documents open in Kantor: id, kind (sheets/docs/slides), name, file path, unsaved changes, and which one is in front. Call this first.",
        || json!({}),
        &[],
        true,
    ),
    (
        "open_document",
        "Open a file",
        "Open a file in Kantor in a new tab (or bring it to front if already open) and return its id. Sheets opens .xlsx/.xlsm/.csv/.tsv (and .ods/.xls/.xlsb read-only, saved as .xlsx), Docs .docx, Slides .pptx.",
        || json!({"path": {"type": "string", "description": "Absolute path (~ allowed)."}}),
        &["path"],
        false,
    ),
    (
        "new_document",
        "New document",
        "Create a new, untitled document in a new tab and bring it to front.",
        || json!({"kind": {"type": "string", "enum": ["sheets", "docs", "slides"]}}),
        &["kind"],
        false,
    ),
    (
        "save_document",
        "Save a document",
        "Save a document. Without `path` it saves to its own file; new, imported or partly unsupported documents need a `path`. An existing file other than the document's own is only replaced with overwrite=true.",
        || {
            json!({
                "document": document(),
                "path": {"type": "string", "description": "Absolute path to save to (Save As); the extension is added when missing."},
                "overwrite": {"type": "boolean", "description": "Allow replacing another existing file at `path`.", "default": false},
            })
        },
        &[],
        false,
    ),
    (
        "activate_document",
        "Show a document",
        "Bring a document's tab to front so the user sees it.",
        || json!({"document": {"type": "integer", "description": "Document id from list_documents."}}),
        &["document"],
        false,
    ),
    (
        "get_selection",
        "Get the user's selection",
        "What the user has selected in a document: cells (Sheets), text (Docs) or slide and shape (Slides).",
        || json!({"document": document()}),
        &[],
        true,
    ),
    // ── Sheets ─────────────────────────────────────────────────────────────
    (
        "sheets_list_sheets",
        "List sheets",
        "List a workbook's sheets with the used range of each (from A1) and which one is on screen.",
        || json!({"document": document()}),
        &[],
        true,
    ),
    (
        "sheets_read_range",
        "Read cells",
        "Read a range of cells as rows: `values` is what the cells show (formatted), `inputs` what was typed (formulas such as =SUM(A1:A3), raw numbers). Large ranges are cut to max_cells whole rows; `next_range` then says where to continue.",
        || {
            json!({
                "document": document(),
                "sheet": sheet(),
                "range": {"type": "string", "description": "A1-style range such as A1:D20 or B7. Default: the used range."},
                "include": {"type": "string", "enum": ["values", "inputs", "both"], "default": "both"},
                "max_cells": {"type": "integer", "minimum": 1, "maximum": 20000, "default": 2000},
            })
        },
        &[],
        true,
    ),
    (
        "sheets_write_range",
        "Write cells",
        "Write a block of cells from its top-left cell, row by row, as if typed: text, numbers, booleans, or formulas starting with \"=\". null leaves a cell as it is; \"\" clears it. One undo step; the written range is selected. At most 10,000 cells per call.",
        || {
            json!({
                "document": document(),
                "sheet": sheet(),
                "top_left": {"type": "string", "description": "Top-left cell, e.g. B2."},
                "values": {
                    "type": "array",
                    "description": "Rows of cells, e.g. [[\"Item\", \"Price\"], [\"Tea\", 12000], [\"Total\", \"=SUM(B3:B3)\"]].",
                    "items": {"type": "array", "items": {"type": ["string", "number", "boolean", "null"]}},
                },
            })
        },
        &["top_left", "values"],
        false,
    ),
    (
        "sheets_select",
        "Select cells",
        "Select a range on a sheet (switching to that sheet) so the user sees it.",
        || json!({"document": document(), "sheet": sheet(), "range": {"type": "string", "description": "A1-style range such as B2:D9."}}),
        &["range"],
        false,
    ),
    (
        "sheets_insert_rows",
        "Insert rows",
        "Insert empty rows before a row (formulas below adjust). One undo step.",
        || {
            json!({
                "document": document(), "sheet": sheet(),
                "before_row": {"type": "integer", "minimum": 1, "description": "Row number the new rows go above."},
                "count": {"type": "integer", "minimum": 1, "maximum": 10000, "default": 1},
            })
        },
        &["before_row"],
        false,
    ),
    (
        "sheets_delete_rows",
        "Delete rows",
        "Delete rows first_row..last_row (inclusive). One undo step.",
        || {
            json!({
                "document": document(), "sheet": sheet(),
                "first_row": {"type": "integer", "minimum": 1},
                "last_row": {"type": "integer", "minimum": 1, "description": "Default: first_row."},
            })
        },
        &["first_row"],
        false,
    ),
    (
        "sheets_insert_columns",
        "Insert columns",
        "Insert empty columns before a column. One undo step.",
        || {
            json!({
                "document": document(), "sheet": sheet(),
                "before_column": {"type": "string", "description": "Column letter the new columns go left of, e.g. C."},
                "count": {"type": "integer", "minimum": 1, "maximum": 1000, "default": 1},
            })
        },
        &["before_column"],
        false,
    ),
    (
        "sheets_delete_columns",
        "Delete columns",
        "Delete columns first_column..last_column (inclusive, letters). One undo step.",
        || {
            json!({
                "document": document(), "sheet": sheet(),
                "first_column": {"type": "string"},
                "last_column": {"type": "string", "description": "Default: first_column."},
            })
        },
        &["first_column"],
        false,
    ),
    (
        "sheets_format",
        "Format cells",
        "Set formatting on a range: font flags, alignment, wrapping, number format, fill and text colour, font size, borders, merging. Only the given properties change. One undo step.",
        || {
            json!({
                "document": document(), "sheet": sheet(),
                "range": {"type": "string", "description": "A1-style range."},
                "bold": {"type": "boolean"},
                "italic": {"type": "boolean"},
                "underline": {"type": "boolean"},
                "strikethrough": {"type": "boolean"},
                "align": {"type": "string", "enum": ["general", "left", "center", "right"]},
                "number_format": {"type": "string", "enum": ["general", "number", "percent", "rupiah", "dollar", "date"],
                    "description": "number = #,##0.00; percent = 0.00%; rupiah = Rp#,##0; dollar = $#,##0.00; date = yyyy-mm-dd."},
                "valign": {"type": "string", "enum": ["top", "center", "bottom"]},
                "wrap": {"type": "boolean", "description": "Wrap text onto several lines."},
                "fill": {"type": "string", "description": "Background colour #RRGGBB, or none."},
                "font_color": {"type": "string", "description": "Text colour #RRGGBB, or auto."},
                "font_size_delta": {"type": "integer", "description": "Points to grow (or, negative, shrink) the font by."},
                "borders": {"type": "string", "enum": ["all", "outline", "none"], "description": "Thin borders on every edge, around the range, or removed."},
                "merge": {"type": "boolean", "description": "true: merge and centre the range (at most one cell may have content); false: unmerge merged cells in it."},
            })
        },
        &["range"],
        false,
    ),
    // ── Docs ───────────────────────────────────────────────────────────────
    (
        "docs_get_paragraphs",
        "Read paragraphs",
        "Read a document's paragraphs in order: index (from 0), text, style, list kind and level, and whether it's in a table. Pictures and other objects show as [object]. Also lists the paragraph styles available.",
        || {
            json!({
                "document": document(),
                "start": {"type": "integer", "minimum": 0, "default": 0},
                "limit": {"type": "integer", "minimum": 1, "maximum": 2000, "default": 300},
            })
        },
        &[],
        true,
    ),
    (
        "docs_insert_paragraphs",
        "Insert paragraphs",
        "Insert paragraphs after a paragraph (after_index -1 = at the start; omitted = at the end). Each has text and optionally a style (\"Heading 1\", \"Title\", any listed by docs_get_paragraphs) and a list (bullet/number, level 0-8); a new line inside text starts another paragraph like it. One undo step; the new paragraphs are selected.",
        || {
            json!({
                "document": document(),
                "after_index": {"type": "integer", "minimum": -1},
                "paragraphs": {
                    "type": "array", "minItems": 1, "maxItems": 2000,
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": {"type": "string"},
                            "style": {"type": "string"},
                            "list": {"type": "string", "enum": ["bullet", "number"]},
                            "level": {"type": "integer", "minimum": 0, "maximum": 8},
                        },
                        "required": ["text"],
                    },
                },
            })
        },
        &["paragraphs"],
        false,
    ),
    (
        "docs_replace_paragraph",
        "Replace a paragraph's text",
        "Replace the text of one paragraph, keeping its style, list and the formatting of its first character. Refused for paragraphs holding pictures (use docs_find_replace). One undo step.",
        || json!({"document": document(), "index": {"type": "integer", "minimum": 0}, "text": {"type": "string"}}),
        &["index", "text"],
        false,
    ),
    (
        "docs_delete_paragraphs",
        "Delete paragraphs",
        "Delete `count` paragraphs starting at `index` (not tables). One undo step.",
        || {
            json!({
                "document": document(),
                "index": {"type": "integer", "minimum": 0},
                "count": {"type": "integer", "minimum": 1, "default": 1},
            })
        },
        &["index"],
        false,
    ),
    (
        "docs_find_replace",
        "Find and replace",
        "Replace every occurrence of `find` with `replace` in the document's text (within paragraphs; formatting of each match is kept). Returns how many were replaced. One undo step.",
        || {
            json!({
                "document": document(),
                "find": {"type": "string"},
                "replace": {"type": "string"},
                "match_case": {"type": "boolean", "default": false},
                "whole_word": {"type": "boolean", "default": false},
            })
        },
        &["find", "replace"],
        false,
    ),
    // ── Slides ─────────────────────────────────────────────────────────────
    (
        "slides_list",
        "List slides",
        "List a presentation's slides (numbered from 1): layout, title, each shape's id, name, kind, placeholder type and text, and the speaker notes. Also the layouts new slides can use.",
        || json!({"document": document()}),
        &[],
        true,
    ),
    (
        "slides_add_slide",
        "Add a slide",
        "Add a slide with a title and bullet points. Bullets are strings or {text, level} objects (level 0-8). One undo step; the new slide is shown.",
        || {
            json!({
                "document": document(),
                "after_slide": {"type": "integer", "minimum": 0, "description": "Slide number it goes after; 0 = first. Default: at the end."},
                "layout": {"type": ["string", "integer"], "description": "Layout name or number from slides_list. Default: Title and Content (or like the slide before)."},
                "title": {"type": "string"},
                "bullets": {
                    "type": "array",
                    "items": {"anyOf": [
                        {"type": "string"},
                        {"type": "object", "properties": {"text": {"type": "string"}, "level": {"type": "integer", "minimum": 0, "maximum": 8}}, "required": ["text"]},
                    ]},
                },
            })
        },
        &[],
        false,
    ),
    (
        "slides_set_shape_text",
        "Set a shape's text",
        "Replace all text of a shape (title, body placeholder, text box); a new line starts a paragraph. Shape ids come from slides_list. One undo step.",
        || {
            json!({
                "document": document(),
                "slide": {"type": "integer", "minimum": 1},
                "shape_id": {"type": "integer"},
                "text": {"type": "string"},
            })
        },
        &["slide", "shape_id", "text"],
        false,
    ),
    (
        "slides_delete_slide",
        "Delete a slide",
        "Delete a slide (and its notes). One undo step.",
        || json!({"document": document(), "slide": {"type": "integer", "minimum": 1}}),
        &["slide"],
        false,
    ),
    (
        "slides_set_notes",
        "Set speaker notes",
        "Replace a slide's speaker notes (plain text). One undo step. slides_list shows the current notes.",
        || {
            json!({
                "document": document(),
                "slide": {"type": "integer", "minimum": 1},
                "text": {"type": "string"},
            })
        },
        &["slide", "text"],
        false,
    ),
    (
        "slides_go_to",
        "Show a slide",
        "Show a slide in the editor so the user sees it.",
        || json!({"document": document(), "slide": {"type": "integer", "minimum": 1}}),
        &["slide"],
        false,
    ),
];

/// Tool descriptions for `tools/list`.
pub fn list() -> Vec<Value> {
    SPECS
        .iter()
        .map(|(name, title, description, props, required, read_only)| {
            let mut annotations =
                json!({"title": title, "readOnlyHint": read_only, "openWorldHint": false});
            if !read_only {
                // Edits can be undone in the app; deleting and saving can't be.
                let destructive = name.contains("delete") || *name == "save_document";
                annotations["destructiveHint"] = json!(destructive);
            }
            json!({
                "name": name,
                "title": title,
                "description": description,
                "inputSchema": {
                    "type": "object",
                    "properties": props(),
                    "required": required,
                    "additionalProperties": false,
                },
                "annotations": annotations,
            })
        })
        .collect()
}

pub fn exists(name: &str) -> bool {
    SPECS.iter().any(|s| s.0 == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn schemas_are_consistent() {
        let mut names = std::collections::HashSet::new();
        for tool in list() {
            let name = tool["name"].as_str().unwrap();
            assert!(names.insert(name.to_owned()), "duplicate {name}");
            let props = tool["inputSchema"]["properties"].as_object().unwrap();
            for req in tool["inputSchema"]["required"].as_array().unwrap() {
                assert!(props.contains_key(req.as_str().unwrap()), "{name}: {req}");
            }
        }
    }
}
