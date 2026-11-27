//! The tools `kantor mcp` offers: names, descriptions and argument schemas.
//! The running app implements them (`workspace/agent.rs`).

use serde_json::{Value, json};

use crate::agent::slides_schema as schema;

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

fn slide_no() -> Value {
    json!({"type": "integer", "minimum": 1, "description": "Slide number from 1."})
}

fn shape_id() -> Value {
    json!({"type": "integer", "description": "Shape id from slides_list."})
}

fn length() -> Value {
    schema::def("length")
}

/// `document`, `slide` and `units`, then `more` (inlined).
fn on_slide(more: serde_json::Map<String, Value>) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("document".into(), document());
    m.insert("slide".into(), slide_no());
    m.insert("units".into(), schema::units());
    for (k, v) in more {
        m.insert(k, schema::inline(&v));
    }
    Value::Object(m)
}

macro_rules! elements_help {
    () => {
        "Lengths are EMU (914400 per inch, 12700 per point) unless `units` says otherwise or a string gives its unit (\"1.5in\", \"2cm\", \"18pt\"); x/y from the slide's top left (slides_list gives the size of shapes; a 16:9 slide is 13.333 × 7.5 in). Colours are #RRGGBB or theme colours (accent1, tx1, bg1, ...). One undo step; returns the new shapes' ids."
    };
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
        "Open a file in Kantor in a new tab (or bring it to front if already open) and return its id. Sheets opens .xlsx/.xlsm/.csv/.tsv (and .ods/.xls/.xlt/.xlsb read-only, saved as .xlsx), Docs .docx (and .rtf/.doc/.dot/.odt/.ott read-only, saved as .docx), Slides .pptx (and .ppt/.pps/.pot/.odp/.otp read-only, saved as .pptx). A password-protected file needs `password` (ask the user for it); it stays protected when saved.",
        || {
            json!({
                "path": {"type": "string", "description": "Absolute path (~ allowed)."},
                "password": {"type": "string", "description": "The password of a password-protected .docx/.xlsx/.pptx/.odt/.ods/.odp file."}
            })
        },
        &["path"],
        false,
    ),
    (
        "new_document",
        "New document",
        "Create a new, untitled document in a new tab and bring it to front.",
        || json!({"kind": {"type": "string", "enum": ["sheets", "docs", "slides", "notes"]}}),
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
    // ── Notes ──────────────────────────────────────────────────────────────
    (
        "notes_read",
        "Read a note",
        "Read a Markdown note (kind notes): its whole text as in the file, its headings with line numbers (from 1), its name and path.",
        || json!({"document": document()}),
        &[],
        true,
    ),
    (
        "notes_write",
        "Rewrite a note",
        "Replace a note's whole text with `text` (Markdown). Only the part that differs changes; the note keeps its line endings. One undo step; the user saves.",
        || {
            json!({
                "document": document(),
                "text": {"type": "string", "description": "The note's new Markdown text."},
            })
        },
        &["text"],
        false,
    ),
    (
        "notes_replace",
        "Replace text in a note",
        "Replace `find` (exact text, case-sensitive) with `replace` in a note: every occurrence, or only the first with all=false. One undo step; returns how many were replaced.",
        || {
            json!({
                "document": document(),
                "find": {"type": "string"},
                "replace": {"type": "string"},
                "all": {"type": "boolean", "default": true},
            })
        },
        &["find", "replace"],
        false,
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
    (
        "sheets_list",
        "Describe a workbook",
        "A workbook as JSON (as `kantor sheets list`): each sheet's used range, frozen panes, merged cells, column widths, tables, charts with their series' ranges, conditional formats, validation, pivot tables, a summary of each column (header, numbers/text/dates/formulas/blanks, min, max, number format) and one page of cells: `values` row by row, `formulas` and `number_formats` beside them, `next` where the next page starts. The open workbook (`document`), or a file without opening it (`path`).",
        || {
            json!({
                "document": document(),
                "path": {"type": "string", "description": "Absolute path of a workbook to read without opening it (~ allowed); instead of document."},
                "sheet": {"type": ["string", "integer"], "description": "Only this sheet (name or number from 1)."},
                "range": {"type": "string", "description": "The cells to show, e.g. A1:H200 or Data!A31:H60 (default: the used range's first rows)."},
                "formulas": {"type": "boolean", "description": "Formulas in the grid in place of their values."},
                "max_rows": {"type": "integer", "minimum": 0, "maximum": 5000, "description": "Rows of cells per sheet (default 30, or 500 of a `range`; 0: summaries only). Cut listings say \"truncated\": true and \"next\"."},
            })
        },
        &[],
        true,
    ),
    (
        "sheets_check",
        "Audit a workbook",
        "Check a workbook for the mistakes reviewers look for (as `kantor sheets check`): formula errors (#REF!, #DIV/0!, #N/A, #VALUE!, #NAME?), circular references, numbers and dates stored as text, a formula that differs from its neighbours, numbers typed into formulas (=B2*1.11), totals typed as numbers, SUMs that miss rows, formulas using empty cells, mixed number formats or units in a column, duplicate rows, extra spaces, merged cells in data, volatile functions, external links, broken names, hidden data, text cut off or shown as #### where it prints (measured with the fonts the PDF and PNG use), content outside the print area or split by page breaks, bar charts not sorted by value. Each finding has a severity, check, sheet, range, message and details. The open workbook (`document`) or a file (`path`). Fix with sheets_edit (to_numbers, to_dates, trim, remove_duplicates, write_range...).",
        || {
            json!({
                "document": document(),
                "path": {"type": "string", "description": "Absolute path of a workbook to check without opening it (~ allowed); instead of document."},
                "sheet": {"type": ["string", "integer"], "description": "Only this sheet."},
            })
        },
        &[],
        true,
    ),
    (
        "sheets_edit",
        "Edit a workbook (tools)",
        "Run editing tools on a workbook, as one undo step (the same JSON as `kantor sheets edit FILE --ops`): `ops` is a list of {tool, args}. Tools: write_range, write_columns, clear, fill_down, fill_right (a formula down a column), format (font, size, colours, fill, alignment, wrap, any number format code, indent, rotation, borders {edges, style, color}, merge), column_width, autofit, row_height, freeze, insert_rows, delete_rows, insert_columns, delete_columns, hide, add_sheet, delete_sheet, rename_sheet, move_sheet, copy_sheet, hide_sheet, sort, remove_duplicates, trim, to_numbers, to_dates, split_column, combine_columns, find_replace, add_table (with style and totals), add_chart (column, bar, line, area, pie, doughnut, scatter, combo of cell ranges: `data` or categories/series ranges, placed `at`/`to` cells; the slides chart options), add_conditional_format (data_bar, color_scale, icon_set, cell_value, text, top, bottom, above_average, below_average, duplicates, unique, blanks, formula, dates), clear_conditional_formats, add_validation (list, whole, decimal, date, time, text_length, custom), clear_validation, define_name, delete_name, add_pivot (rows, columns, values by field name), page_setup, recalculate. Cells are A1-style (Data!A1:D20, C:E, 3:5); `sheet` is a name or number. Each tool's arguments are in `kantor sheets schema` and docs/sheets-json.md. Returns each tool's result (written formulas' values and errors).",
        || {
            json!({
                "document": document(),
                "ops": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {
                    "tool": {"type": "string", "description": "A tool name, e.g. fill_down."},
                    "args": {"type": "object", "description": "Its arguments, e.g. {\"range\": \"E2:E200\", \"formula\": \"=C2*D2\"}."}},
                    "required": ["tool"]}},
            })
        },
        &["ops"],
        false,
    ),
    (
        "sheets_build",
        "Build a .xlsx from JSON",
        "Write a new .xlsx from a workbook description without opening it (as `kantor sheets build`): sheets with rows or columns of values and formulas, cells, formats, widths, frozen panes, tables, charts placed by cell, conditional formats, validation, pivot tables, names, and any editing tools (`ops`). The same JSON always gives the same file. open=true then opens it in Kantor.",
        || {
            json!({
                "workbook": {"type": "object", "description": "{sheets: [{name, rows | columns, cells, formats, widths, freeze, tables, charts, conditional_formats, validations, pivots, ops}], names, active, ops}; see `kantor sheets schema` or docs/sheets-json.md."},
                "path": {"type": "string", "description": "Absolute path of the .xlsx to write (~ allowed)."},
                "overwrite": {"type": "boolean", "default": false, "description": "Replace an existing file."},
                "open": {"type": "boolean", "default": false, "description": "Open the new file in Kantor."},
            })
        },
        &["workbook", "path"],
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
        "Replace every occurrence of `find` with `replace` in the document's text, in the body, every header and footer, text boxes and footnotes/endnotes by default (within paragraphs; formatting of each match is kept; a line break in `replace` is a line break in the paragraph). Returns how many were replaced.",
        || {
            json!({
                "document": document(),
                "find": {"type": "string"},
                "replace": {"type": "string"},
                "match_case": {"type": "boolean", "default": false},
                "whole_word": {"type": "boolean", "default": false},
                "in": {"type": "string", "enum": ["all", "body", "headers", "notes"], "default": "all", "description": "Where to look: everywhere; the body and its text boxes; the headers and footers and their text boxes; or the footnotes and endnotes."},
            })
        },
        &["find", "replace"],
        false,
    ),
    (
        "docs_build",
        "Build a .docx from JSON",
        "Write a new Word document from JSON without opening it (as `kantor docs build`): a preset look (report, proposal, letter) with `accent` and `fonts` for the client's brand, real styles, numbered headings, cover, table of contents (filled in with page numbers), lists, tables, pictures, charts, callouts, captions, cross-references ({ref:id}), footnotes ([^text]), sections (landscape pages), headers and footers. Blocks: heading, paragraph (or a string), list, table, picture, chart, callout, quote, page_break, section, toc, list_of_figures, list_of_tables; shorthands {\"h1\": …}, {\"bullets\": […]}. See `kantor docs schema` or docs/docs-json.md. The same JSON gives the same file. Returns pages and notes; check it with docs_check and look at it with `kantor convert FILE --to png`.",
        || {
            json!({
                "doc": {"type": "object", "description": "The document: {preset, accent, fonts, properties: {title, company, date}, cover: {label, title, subtitle, meta}, toc: true, header, footer, page, styles, blocks: [...]}."},
                "path": {"type": "string", "description": "Absolute path of the .docx to write (~ allowed)."},
                "base_dir": {"type": "string", "description": "Folder relative picture paths start from (default: they must be absolute)."},
                "overwrite": {"type": "boolean", "default": false},
                "open": {"type": "boolean", "default": false, "description": "Open the new file in Kantor."},
            })
        },
        &["doc", "path"],
        false,
    ),
    (
        "docs_edit",
        "Revise a .docx file",
        "Run editing tools on a Word file and save it, without opening it (as `kantor docs edit`). `ops` is [{tool, args}]: get_paragraphs, insert_paragraphs, replace_paragraph, delete_paragraphs, find_replace (with bold/italic/color/style to format the new text), format, insert_blocks (blocks as in docs_build, placed after/before a paragraph id, after_heading, before_heading, end_of_section), replace_blocks, delete_blocks (from/to ids or a heading's whole section), move_section, set_style, restyle (change a style definition), add_comment, resolve_comment, accept_changes, reject_changes, update_fields (TOC, numbers, references), set_header, set_footer, page_setup. `track` makes the edits tracked changes by that author. Paragraph ids come from docs_list (`kantor docs list FILE --text`). Returns each tool's result.",
        || {
            json!({
                "path": {"type": "string", "description": "Absolute path of the .docx (~ allowed)."},
                "output": {"type": "string", "description": "Write here instead of over the file."},
                "ops": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {"tool": {"type": "string"}, "args": {"type": "object"}}, "required": ["tool"]}},
                "track": {"type": "string", "description": "Author name: the edits become tracked changes."},
                "base_dir": {"type": "string", "description": "Folder relative picture paths start from."},
                "overwrite": {"type": "boolean", "default": false, "description": "Replace an existing `output`."},
            })
        },
        &["path", "ops"],
        false,
    ),
    (
        "docs_check",
        "Check a .docx file",
        "Lay a Word file out as Kantor draws it and report problems with paragraph ids and pages (as `kantor docs check --json`): empty or skipped heading levels, headings made by hand, direct formatting, broken cross-references, a stale table of contents, caption numbers, pictures without alt text, tables without a header row, text past cells or margins, headings alone at a page bottom, mixed fonts, placeholder text (TBD, [Name], Lorem), double spaces, open comments and tracked changes, low contrast, fonts not installed. With `outline`, also the document's outline (as `kantor docs list`).",
        || {
            json!({
                "path": {"type": "string", "description": "Absolute path of the .docx (~ allowed)."},
                "outline": {"type": "boolean", "default": false, "description": "Also return the outline: sections, headings with ids, tables, pictures, fields, comments, changes, pages."},
            })
        },
        &["path"],
        true,
    ),
    // ── Slides ─────────────────────────────────────────────────────────────
    (
        "slides_list",
        "List slides",
        "List a presentation's slides (numbered from 1): layout, title, each shape's id, name, kind, placeholder type and text, and the speaker notes (`notes`, null when the slide has none). Also the layouts new slides can use.",
        || json!({"document": document()}),
        &[],
        true,
    ),
    (
        "slides_check",
        "Check slides for layout problems",
        "Lay a presentation's slides out as Kantor draws them and report problems an author can't see without looking: text overflowing its box or wrapping in a one-line box (lines needed vs lines that fit), shrink-to-fit boxes and the scale they need, shapes off the slide, text running into other text or crossing a shape's edge, low contrast, fonts not installed, text under 10 pt, empty placeholders, slides without a title or notes. Each finding has a severity (error, warning, info), the slide, the shape's id and name, and a rect in EMU and inches. Checks the open presentation (`document`), or a .pptx file without opening it (`path`). Run it after building or editing a deck and fix the errors.",
        || {
            json!({
                "document": document(),
                "path": {"type": "string", "description": "Absolute path of a .pptx to check without opening it (~ allowed); instead of document."},
                "slide": {"type": "integer", "minimum": 1, "description": "Only this slide."},
            })
        },
        &[],
        true,
    ),
    (
        "slides_add_slide",
        "Add a slide",
        "Add a slide with a title and bullet points (bullets are strings or {text, level} objects, level 0-8), and optionally a background, elements (shapes, text boxes, lines, pictures, tables, charts; as slides_add_elements) and speaker notes. One undo step; the new slide is shown.",
        || {
            let mut v = json!({
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
                "background": schema::def("background"),
                "elements": {"type": "array", "items": schema::def("element")},
                "notes": {"type": "string", "description": "Speaker notes."},
                "units": schema::units(),
            });
            v["layout"]["description"] = json!(
                "Layout name or number from slides_list. Default: Title and Content (or like the slide before)."
            );
            v
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
        "Replace a slide's speaker notes (plain text, as `text` or `notes`). One undo step. slides_list shows the current notes.",
        || {
            json!({
                "document": document(),
                "slide": {"type": "integer", "minimum": 1},
                "text": {"type": "string"},
                "notes": {"type": "string", "description": "Same as text (the key deck slides use)."},
            })
        },
        &["slide"],
        false,
    ),
    (
        "slides_add_shape",
        "Add a shape, text box or line",
        concat!(
            "Add a shape (type shape: any preset geometry such as rect, roundRect with radius, ellipse, chevron, homePlate; fill, outline, text), a text box (type text) or a line (type line: x1/y1 to x2/y2, dash, arrowheads) to a slide. Text is a string or paragraphs with runs (font, size, bold, italic, colour), alignment, vertical anchor, line spacing, insets, autofit. Shapes look like PowerPoint's new shapes unless given fill/line. ",
            elements_help!()
        ),
        || {
            let mut more = schema::shape_props();
            more.extend(schema::line_props());
            more.insert(
                "type".into(),
                json!({"type": "string", "enum": ["shape", "text", "line"], "default": "shape"}),
            );
            on_slide(more)
        },
        &["slide"],
        false,
    ),
    (
        "slides_add_picture",
        "Add a picture",
        concat!(
            "Add a PNG or JPEG picture from a file at x/y/w/h (one of w/h keeps its aspect; fit contain or cover with both), with cropping (crop left/top/right/bottom as fractions), cut to a shape (geometry such as ellipse, or radius for rounded corners), an outline and alternative text. ",
            elements_help!()
        ),
        || on_slide(schema::picture_props()),
        &["slide", "path"],
        false,
    ),
    (
        "slides_add_table",
        "Add a table",
        concat!(
            "Add a table: rows of cells (text, numbers, or {text, fill, bold, color, align, col_span, row_span, ...}; null for cells covered by a span), column widths, a header row (bold, header_fill) and banded rows, fills and borders. ",
            elements_help!()
        ),
        || on_slide(schema::table_props()),
        &["slide", "rows"],
        false,
    ),
    (
        "slides_add_chart",
        "Add a chart",
        concat!(
            "Add a real, editable chart from data (PowerPoint's Edit Data and Kantor's chart panel open its data): chart column, bar, line, area, pie, doughnut, scatter or combo; categories and series [{name, values, color, type, axis}]; stacked (true or \"percent\"); title, legend, data_labels, number_format (Excel codes such as \"#,##0.0\"), axis {min, max, major_unit, title, gridlines, number_format}, gap_width, overlap, font, per-bar colors, target_line. Never draw charts from shapes. ",
            elements_help!()
        ),
        || on_slide(schema::chart_props()),
        &["slide", "chart", "w", "h", "categories", "series"],
        false,
    ),
    (
        "slides_add_elements",
        "Add several elements",
        concat!(
            "Add several shapes, text boxes, lines, pictures, tables and charts to a slide at once (each with its `type`; the same objects as slides_add_shape, slides_add_picture, slides_add_table and slides_add_chart, and as the elements of slides_build). Also icons (type icon: a Lucide `icon` name, size, color) and components that expand into editable shapes: stat_cards, cards, steps, timeline, callout, numbered_list, comparison, badge, and tables with preset clean (see `kantor slides schema`). `style` names a text style (eyebrow, title, subtitle, body, small, label, card_title, stat, ...); colours may be palette names (primary, highlight, ink, muted, divider, paper, tint, ...). Later ones are on top. ",
            elements_help!()
        ),
        || {
            let mut more = serde_json::Map::new();
            more.insert(
                "elements".into(),
                json!({"type": "array", "minItems": 1, "items": {"$ref": "#/$defs/element"}}),
            );
            on_slide(more)
        },
        &["slide", "elements"],
        false,
    ),
    (
        "slides_set_background",
        "Set a slide's background",
        "Set a slide's background: a colour, a gradient, {\"picture\": absolute path}, or \"inherit\" (back to the layout's). One undo step.",
        || {
            let mut more = serde_json::Map::new();
            more.insert("background".into(), json!({"$ref": "#/$defs/background"}));
            on_slide(more)
        },
        &["slide", "background"],
        false,
    ),
    (
        "slides_update_shape",
        "Move or resize a shape",
        "Move, resize, turn or flip a shape: only the given values change (lengths as in slides_add_shape). Tables keep their rows in proportion. One undo step.",
        || {
            json!({
                "document": document(), "slide": slide_no(), "shape_id": shape_id(), "units": schema::units(),
                "x": length(), "y": length(), "w": length(), "h": length(),
                "rotation": {"type": "number", "description": "Degrees clockwise."},
                "flip_h": {"type": "boolean"}, "flip_v": {"type": "boolean"},
            })
        },
        &["slide", "shape_id"],
        false,
    ),
    (
        "slides_delete_shape",
        "Delete a shape",
        "Delete a shape from a slide. One undo step.",
        || json!({"document": document(), "slide": slide_no(), "shape_id": shape_id()}),
        &["slide", "shape_id"],
        false,
    ),
    (
        "slides_order_shape",
        "Bring forward or send backward",
        "Change a shape's stacking order: front, back, forward or backward one step, or a position counting from 1 at the back. One undo step.",
        || {
            json!({
                "document": document(), "slide": slide_no(), "shape_id": shape_id(),
                "to": {"anyOf": [{"type": "string", "enum": ["front", "back", "forward", "backward"]}, {"type": "integer", "minimum": 1}]},
            })
        },
        &["slide", "shape_id", "to"],
        false,
    ),
    (
        "slides_group_shapes",
        "Group shapes",
        "Group two or more shapes of a slide into one group (it moves and resizes as one). One undo step; returns the group's id.",
        || {
            json!({
                "document": document(), "slide": slide_no(),
                "shape_ids": {"type": "array", "minItems": 2, "items": {"type": "integer"}},
            })
        },
        &["slide", "shape_ids"],
        false,
    ),
    (
        "slides_build",
        "Build a .pptx from JSON",
        "Write a new .pptx from a deck description without opening it: slide size, theme fonts and colours, then each slide's layout, title, bullets, background, elements (as slides_add_elements) and notes. The same JSON as `kantor slides build deck.json -o deck.pptx`. The same deck always gives the same file. open=true then opens it in Kantor.",
        || {
            json!({
                "deck": {"type": "object", "description": "The deck: {size, units, theme, text, background, slides: [{layout, title, bullets, background, elements, notes}]}; see the schema (kantor slides schema) or docs/slides-json.md."},
                "path": {"type": "string", "description": "Absolute path of the .pptx to write (~ allowed)."},
                "base_dir": {"type": "string", "description": "Folder that relative picture paths start from (default: they must be absolute)."},
                "overwrite": {"type": "boolean", "default": false, "description": "Replace an existing file."},
                "open": {"type": "boolean", "default": false, "description": "Open the new file in Kantor."},
            })
        },
        &["deck", "path"],
        false,
    ),
    (
        "mail_merge",
        "Mail merge",
        "Fill a template in once per row of a workbook or CSV file and write the files, without opening them (as `kantor docs merge` / `kantor slides merge` / `kantor sheets merge`). A .docx template holds Word merge fields (MERGEFIELD, IF, NEXT, SKIPIF, MERGEREC… as Word's Mailings tab writes them; without `data` its own linked data source is used) and gives one .docx and/or .pdf per record, or one merged .docx with `single`. A .pptx or .xlsx template holds {{Field}} placeholders and gives one .pptx/.xlsx and/or .pdf per record. The data's first row (or `header_row`) names the fields. Returns the files written.",
        || {
            json!({
                "template": {"type": "string", "description": "Absolute path of the .docx, .pptx or .xlsx template (~ allowed)."},
                "data": {"type": "string", "description": "Absolute path of the data: .xlsx, .xls, .xlsb, .ods, .csv or .tsv. Optional for a .docx with its own data source."},
                "sheet": {"type": ["string", "integer"], "description": "The data's sheet: name or number from 1. Default: the first."},
                "header_row": {"type": "integer", "minimum": 1, "description": "The row naming the fields. Default 1."},
                "where": {"type": "array", "items": {"type": "string"}, "description": "Only rows where every condition holds: \"Field=Value\", \"Field!=Value\", <, >, <=, >=, \"Field~Text\" (contains), \"Field=\" (blank), \"Field!=\" (not blank)."},
                "sort": {"type": "array", "items": {"type": "string"}, "description": "Sort keys: \"Field\" or \"Field:desc\"."},
                "output_dir": {"type": "string", "description": "Absolute path of the folder for one file per record (made when missing)."},
                "formats": {"type": "array", "items": {"type": "string", "enum": ["docx", "pptx", "xlsx", "pdf"]}, "description": "What to write per record: the template's format and/or pdf. Default: the template's format."},
                "name": {"type": "string", "description": "File name pattern: {Field} is the row's value, {#} its number; the extension is added. Default \"TEMPLATE {#}\"."},
                "single": {"type": "string", "description": "Docs only: absolute path of one .docx (or .pdf) holding every record, a section each."},
                "overwrite": {"type": "boolean", "default": false, "description": "Replace existing files."},
            })
        },
        &["template"],
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

/// Whether tool `name` only reads (unknown tools count as editing).
pub fn is_read_only(name: &str) -> bool {
    SPECS.iter().any(|s| s.0 == name && s.5)
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
