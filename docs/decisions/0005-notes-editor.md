# 0005: Notes is a drawn editor over the Markdown text

- **Status:** accepted (2026-11-27), phases 1 (live preview editor) and 2 (reading view, conversions) of Notes; phase 3 (a folder of notes: wiki links, backlinks, renames, search, graph view) added 2026-11-28 (see "Vaults" below). Plugins build on it.
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
- Known gaps: no autoscroll while drag-selecting past the edge; tables are edited as text (revealed) rather than cell by cell; images from the web aren't fetched; no spell check yet. (Italics used to show upright when the UI font has no italic face: GPUI can't slant glyphs, so italic runs now take an installed family that has a real italic, `NotesView::refresh_italic`.)

## Vaults (phase 3)

**Index** (`notes_engine::vault`, no GPUI): a `Vault` is a folder's notes (hidden folders such as `.obsidian` and `.git` skipped), each with its text and the links `parse` finds: `[[Note]]`, `[[Note|alias]]`, `[[Note#Heading]]`, embeds `![[Note]]`, and Markdown links to `.md` files. `wiki_parts` splits a wiki link into byte ranges (name, heading, alias). Names resolve as Obsidian does: file name without extension, any case; a name with `/` matches a path's end; among notes of one name the linking note's folder wins, then the shortest path. On it: backlinks, unlinked mentions (the name as a whole word outside links and code), search (every word of the query on one line), `[[` completion (prefix, word start, substring, then letters in order; `Note#` lists headings) and the edges for the graph.

**Renames** are plans of byte splices (`Vault::rename_splices`): only the bytes of each link's target change, in the style it was written (bare name, path, `.md`, `%20` in Markdown links). The note's file is moved, open notes take the splices through their editor (one undo step each, then saved), closed files are spliced on disk. The vault keeps the inverse splices and a hash of each file's new text, so Undo Rename puts back the file name and links where a file is still as the rename left it. The rename dialog says how many notes change before anything is written.

**UI** (`notes_ui::vault`, `panels`, `graph_view`, `complete`): one `VaultState` entity per folder, shared by the note views in it (a note opened in an open vault, or under a folder with `.obsidian`, joins it; the toolbar's Files / Search / Backlinks / Graph buttons open the note's own folder as one). The folder is read on a background thread; saves update the index. A plain click follows a link where its line shows formatted, Ctrl/Cmd+click anywhere; a link to a note that doesn't exist yet makes it (in the linking note's folder, or under the vault's root for `Folder/Note`) and is drawn faded until then. The graph is `notes_engine::graph`: d3-force's model (springs on links, a Barnes–Hut push between notes, a weak pull to the middle, cooling to rest), stepped on a 16 ms timer only while it moves, painted with one GPUI path per few thousand links and a quad per note; pan, zoom about the pointer, drag a note (the rest follow), click to open; whole vault or the current note's neighbourhood (depth 1–3).

- The index lives in memory (each note's text): fine for thousands of notes; a file watcher isn't used, so changes made outside Kantor show after "Read the folder again" or a save.
- Block references (`[[Note#^id]]`), aliases from front matter and tags aren't indexed yet.
