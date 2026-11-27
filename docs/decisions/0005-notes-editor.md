# 0005: Notes is a drawn editor over the Markdown text

- **Status:** accepted (2026-11-27), phases 1 (live preview editor) and 2 (reading view, conversions) of Notes. Phase 3 (a folder of notes: wiki links, backlinks, graph view) and plugins build on it.
- **Decides:** how Kantor edits Markdown notes (`crates/notes_engine`, `crates/notes_ui`).

## Context

Notes should be a light, native alternative to Obsidian inside Kantor: the `.md` file is the document, shown formatted while it is edited ("live preview": the markup of a line shows only while the caret is on it), with a source mode and a reading view. Files must round-trip byte for byte.

## Options

- **Kantor's text `Input`** (gpui-component's `InputState`): single-line state can't colour runs, and the multi-line editor has no way to hide bytes or draw check boxes and tables.
- **The Docs model** (convert Markdown to paragraphs and back): needs a serialiser, so saving rewrites the file; byte-exact round trips are out.
- **A web view**: not allowed (no JS runtime in Kantor).
- **A drawn element over the text** (chosen).

## Decision

**Buffer** (`notes_engine::buffer`): the file's text as a `String` (a byte-order mark set aside and written back), line starts indexed, line endings kept as found; a line typed or pasted gets the note's own ending (the first one found). Notes are small enough that a `String` with an O(n) splice per edit is fine; the API (`replace`, `line_range`, `line_of`) hides it, so a rope can replace it later.

**Parser** (`notes_engine::parse`): pulldown-cmark 0.13 (MIT) with offsets, CommonMark plus GFM tables, task lists, strikethrough, footnotes, YAML front matter and `[[wiki links]]` (`Options::ENABLE_WIKILINKS`). Its events become one `LineInfo` per buffer line: the line's kind (paragraph, heading, code, table row, rule, front matter…), its quote depth, list depth and list marker, the lines revealed with it (a table, a code block, a setext heading), the **visible** byte ranges and the inline **spans** (strong, emphasis, strike, code, link, image). The rule that hides markup is: inside a recognised block, bytes covered by content events (text, code without its backticks, HTML, code-block text) are visible and every other byte is markup. A line in no block (a link reference definition) shows as typed. Nothing is ever re-serialised from the parse.

**Editor** (`notes_engine::editor`): selection (byte offsets), undo/redo as lists of replacements (typing and deleting merge into word-sized steps, input-method updates into one), a state id for "is it saved?" that undo brings back, and the Markdown helpers: Enter continues lists, quotes and task lists (and ends or outdents an empty item), Tab / Shift+Tab indent list items by the parent marker's width, `**` / `*` / `~~` / `` ` `` wrapping, links, headings, task check boxes, list and quote buttons. Each is a set of text replacements.

**Layout** (`notes_ui::layout`): one box per buffer line, shaped with GPUI's `WindowTextSystem::shape_text` (soft wrap, styled runs). A line is *formatted* (only its visible bytes shaped; list markers, check boxes and quote bars drawn as decorations before the text, which wraps with a hanging indent) or *revealed* (every byte shaped, markup muted, the content kept where the formatted line has it, its markup in the gutter). A `DisplayMap` turns offsets in the shaped text back into buffer offsets, so carets, clicks and selections always land on bytes of the file. A formatted table is one grid box on its first line (its other lines take no height); pictures alone on a line are drawn under it at their size (read from the file header). Boxes are cached by a hash of what shapes them, with offsets relative to the line start: an edit reshapes only the lines it touched and those whose reveal state changed.

**View** (`notes_ui`): a canvas paints the visible boxes; typing and IMEs go through `EntityInputHandler` (the caret's line is the IME's text, as in Docs); keys are actions in the `NotesEditor` context. Modes: live preview (the selection's lines revealed), source (all revealed, one size), reading (none, read-only).

**Conversions** (`kantor::agent::notes_convert`): the parse becomes the docs JSON (`docs_author::build`, runs from the spans, not markdown strings) or a slides deck (`slides_author::build`, a slide per heading); PDF goes through the `.docx` and Docs' layout.

## Consequences

- Saving an unedited note writes the same bytes; every edit is a splice of what was typed.
- Re-parsing the whole note on each edit (pulldown-cmark, about a millisecond per 100 KB) keeps the model simple; incremental parsing can come later behind the same `Parsed` API.
- Phase 3 builds on `Parsed::links` (`LinkKind::Wiki` with the target name, `[[Note|label]]` and `#heading` parts in `dest`) and `Parsed::headings`; following a wiki link today opens `Name.md` in the note's folder (`NotesEvent::Open`). Backlinks and a graph need a folder index over these.
- Known gaps: italics show upright when the UI font has no italic face (GPUI doesn't slant); no autoscroll while drag-selecting past the edge; tables are edited as text (revealed) rather than cell by cell; images from the web aren't fetched; no spell check yet.
