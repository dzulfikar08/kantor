# Changelog

All notable changes to Kantor are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, minor versions may break things).

The release workflow (`.github/workflows/release.yml`) uses the section for a tag as
that GitHub Release's notes, so every `v*` tag needs a `## [x.y.z]` heading here.

## [Unreleased]

### Added

#### Packaging
- Every download (dmg, AppImage, tarballs) ships `THIRD_PARTY_LICENSES.txt`: the licence texts of
  all ~610 crates linked into Kantor, generated at packaging time by
  `scripts/third-party-licenses.py`. On macOS it is inside `Kantor.app/Contents/Resources`.

#### Sheets
- Formatted workbooks look as they do in Excel: cell fills (theme colours with tints and indexed
  colours resolved), text colour, font size and family (Calibri → Carlito, Arial → Liberation
  Sans when the original isn't installed), vertical alignment, wrapped text, borders (thin, medium,
  thick, double, dotted, medium dashed; shared edges resolved like Excel), merged cells (drawn as
  one cell; clicking or arrowing into one selects it as a whole) and gridlines off per sheet.
- Toolbar and command palette: fill and text colour, larger / smaller font, all / outside / no
  borders, wrap text, merge & center / unmerge, and vertical alignment (palette). Each is one
  undo step.
- MCP `sheets_format` also sets fill, text colour, font size, wrap, vertical alignment, borders
  and merging.
- Number formats show values as Excel does (`crates/kantor_numfmt`, adapted from GridCraft):
  fractions (`# ?/?`), elapsed time (`[h]:mm:ss`), conditions (`[>=1000]#,##0,"K";0`),
  colours (`[Red]` draws the value red), text sections (`"Name: "@`), fill (`* ` spreads
  accounting formats across the cell), spacing (`_)`), engineering notation, built-in format 14
  as `m/d/yyyy`, and halves rounded away from zero (`0` shows 1234.5 as `1235`).
- Locale-tagged formats work: `[$Rp-421]#,##0` shows `Rp1.500.000` (Indonesian separators from
  the tag) and `[$-421]d mmmm yyyy` shows `17 Agustus 2025`. They showed `#VALUE!` before.

#### Updates
- In-app updates for the DMG, AppImage and install-script installs: a check at most once a day
  (and *Check for Updates…* in the palette) fetches a signed manifest, sending only Kantor's
  version and OS. The new version downloads in the background, is verified (Ed25519 manifest
  signature, SHA-256, and on macOS codesign / Gatekeeper) and installs on *Restart to update* or
  at quit, all or nothing. Package and source installs only get a notice. Turn it off with
  `[update] enabled = false` in `settings.toml` (README "Updates").
- `kantor --version` names the nightly build (`kantor 0.2.0 (nightly 2026-11-10 14:30)`).

### Changed
- Sheets: big workbooks use about a third less memory (a 180k-row, 32 MB `.xlsx`: 410 → 323 MB
  with the app open, 467 → 377 MB after F9), and closing one gives its memory back to the system
  (it stayed allocated before).
- Linux: the automatic UI font skips variable fonts (such as a single-file Inter or Adwaita Sans)
  when a family with static bold faces is installed, since GPUI draws only a variable font's
  default weight there and bold text looked regular. macOS is unchanged.

### Performance
- Sheets resolves each cell style once per workbook state instead of cloning it for every
  visible cell on every frame.

## [0.2.0] - 2026-11-10

Docs and Slides join Sheets: all three apps open, edit and save their Office formats, and Docs and
Slides export PDF. New: macOS builds (a notarised `Kantor.dmg`), Linux AppImages, `kantor convert`
and `kantor pdf` on the command line, opt-in AI features, and an MCP server for AI agents.

### Added

#### Docs
- **Docs** (first slice): open, edit and save `.docx` on A4 pages laid out with parley — typing,
  selection, Bold/Italic/Underline, headings (Ctrl+Alt+0–3), alignment, undo/redo, zoom, plain-text
  clipboard, IME input. New Document is Ctrl+Shift+N. Saving an opened `.docx` rewrites only the
  body and copies every other part unchanged.
- **Docs: lists, tables and pictures.** Bulleted and numbered lists (Ctrl+Shift+8 / Ctrl+Shift+7,
  toolbar; Enter continues, Enter on an empty item ends the list, Tab / Shift+Tab change level,
  Backspace at the start removes the number) with markers drawn in the hanging indent. Tables with
  merged cells and borders, laid out row by row across pages; Tab moves to the next cell (and adds a
  row at the end), toolbar grid to insert a table, insert/delete rows and columns, delete table.
  Inline PNG/JPEG pictures, shown at their size, selectable and deletable like a character; insert a
  picture from a file. All of it is undoable and saves in place.
- **Docs: keeps what it can't show yet.** Floating pictures, shapes and text boxes, content
  controls, page-number and date fields, equations, footnote references, bookmarks, nested tables,
  headers/footers and unmodelled formatting are carried byte for byte and shown as placeholders, so
  ordinary Word files now open as fully supported and save in place. Only tracked changes, comments
  and fields that depend on the document (tables of contents, cross-references) still make Save go
  to a new file. Page breaks and hyperlinks (editable text) are supported too.
- **Docs: Export as PDF…** (command palette) writes the document with the same layout as the
  screen: fonts subset and embedded (text stays selectable and searchable), pictures, colours,
  underline, tables and list markers. Headers and footers are not in the PDF yet.
- **Docs: page setup and ruler.** Paper size (A4, Letter, Legal, A5, F4), orientation and margins
  from the toolbar's Page control or the palette; a centimetre ruler with the margins shaded and
  indent markers you can drag. Saving changes only those settings in the file.
- **Docs: word and character count** in the status bar, for the selection when there is one
  ("12 of 1,234 words · 60 characters").
- **Docs: Rewrite / Translate selection.** Fix grammar, More formal, Shorter, or translate between
  Indonesian and English. Only the selected paragraphs' text is sent; the result is shown first and
  *Replace* applies it as one undo step, keeping paragraph styles (formatting inside a paragraph
  becomes that of its first word).

#### Slides
- **Slides** (first slice): new presentation (Ctrl+Alt+N, Home, sidebar, palette) and open/save
  `.pptx`. Slides look like the file: placeholder positions and text formatting come from the
  layout, master and theme, and master/layout decorations, backgrounds and pictures are drawn.
  Thumbnail strip (drag to reorder, Ctrl+Up/Down), new/duplicate/delete slide (Ctrl+M / Ctrl+D),
  select, move (drag or arrows) and resize shapes, edit text in text boxes and placeholders
  (Bold/Italic/Underline, IME), insert text box, rectangle, ellipse or PNG/JPEG picture, undo/redo,
  and a simple presenter mode (F5 / Shift+F5; arrows, space, Esc). Saving copies every part it
  doesn't rewrite byte for byte, keeps unchanged shapes verbatim and, for edited ones, replaces
  only the position or the paragraphs; tables, charts, groups and SmartArt are written back
  untouched. When slides are added, removed or reordered in a file with slide sections or
  custom shows, those can't be kept, so Save goes to a new file and the original is never
  overwritten.
- **Slides: lists, rotation, notes, fill and outline, copy/paste.** Bullets and numbering (from the
  master and layout, or the toolbar's list buttons), indent levels (Tab / Shift+Tab) and paragraph
  spacing are shown and edited; unmodelled paragraph settings are kept. Rotated and flipped shapes,
  pictures and text are drawn as in the file, with a rotate handle (Shift snaps to 15°). A
  resizable speaker-notes pane under the slide (notes are created with their part, relationships
  and notes master when needed) and notes under the slide while presenting (S hides them). Fill and
  outline popovers with the theme's colours, no fill / no outline and outline widths. Copy, cut and
  paste shapes within and between presentations (pictures take their image along) and Ctrl+D to
  duplicate the selected shape. All undoable.
- **Slides: canvas.** The slide fits the pane and stays centred; zoom with Ctrl+= / Ctrl+- /
  Ctrl+wheel and back to fit with Ctrl+0. Empty placeholders show a dashed outline and a grey
  prompt in the editor only. Resize, move and text cursors over shapes.
- **Slides: shapes keep their real outlines.** All 187 PowerPoint preset shapes (arrows, chevrons,
  stars, callouts, flowchart symbols, brackets, clouds, hearts, connectors…), their adjustments
  (handle positions such as an arrow's head size) and custom freeform shapes are drawn with their
  own outline instead of a box; 3D-style faces (cans, cubes, bevels) get their lighter and darker
  shades. Connectors are now shapes: they can be selected, moved and recoloured. Geometry XML is
  still written back unchanged. The evaluator (`slides_engine::geom`, no GPUI) follows ECMA-376
  shape guides and paths.
- **Slides: slides look like PowerPoint's.** Gradient and picture fills, transparency, dashed
  lines, line caps and arrowheads, and slide, layout and master backgrounds are drawn. Groups,
  tables (merged cells, borders, PowerPoint's table styles and banding) and SmartArt (from the
  drawing PowerPoint saves with it) are drawn instead of grey boxes. Charts are drawn from the
  values saved in the file: column and bar (clustered, stacked, 100 %), line, area, pie,
  doughnut, scatter and combinations, with titles, legends, axes, gridlines and data labels;
  other chart types still show a placeholder. All of this is display only: the XML of these
  objects is written back unchanged.
- **Slides: Export as PDF…** (command palette), one page per slide at the slide's size, hidden
  slides left out: real gradients, dashes, rotated text and pictures, embedded fonts. Exporting
  runs in the background.

#### Sheets
- **Sheets: big workbooks stay responsive.** Spreadsheets open in the background: the pane appears
  at once with the file name, size, elapsed time and a Cancel button. `.xlsx` files show the values
  Excel saved instead of recalculating everything on open. Workbooks with more than 50,000 formulas
  switch to **manual calculation** (status bar: "Manual calculation"): edits don't recalculate the
  workbook, F9 (or Shift+F9, or the palette's "Calculate Now") does, in the background with a
  "Calculating…" pane. Palette: "Automatic Calculation" / "Manual Calculation". A workbook saved
  with out-of-date values asks Excel to recalculate when opened. Status-bar sums and the used range
  are no longer recomputed over every cell on each frame.
- **Sheets: faster recalculation of lookup-heavy workbooks.** Kantor now carries a patched copy of
  IronCalc that caches lookups during a recalculation: a real 767,000-formula workbook went from
  more than 10 minutes to about 3 seconds.
- **Sheets: gridlines and headers.** Gridlines, a 2 px outline around the active cell and
  highlighted headers for the selected rows and columns.
- **Sheets: formula point mode.** While typing a formula with the caret after `=`, `(`, `,` or an
  operator, clicking a cell inserts its reference (drag or Shift+click for a range; clicking again
  replaces it), and arrows / Shift+arrows point too; a click elsewhere in the text still commits as
  before. Clicking another sheet's tab keeps the edit in the formula bar and inserts `'Sheet2'!A1`.
  References are coloured in the editor and outlined in the same colour on the grid; F4 cycles
  `A1 → $A$1 → A$1 → $A1`; typing a function name lists matching IronCalc functions (Tab/Enter
  inserts `SUM(`).
- **Sheets: grid manipulation.** Click a column letter or row number to select it (Shift+click or
  drag across headers for several), the corner or Ctrl+A for everything; Ctrl+arrows jump to the
  edge of the data and Ctrl+Shift+arrows extend to it. Drag the fill handle to fill down, up, left
  or right (series such as 1, 2 → 3, 4 and dates continue, other values repeat, relative references
  in formulas shift), or use Ctrl+D / Ctrl+R; each fill is one undo step. Double-click a column
  border to fit it to its content. Right-click a header to insert or delete rows and columns,
  autofit or reset widths, or freeze panes; the palette has the same commands plus Freeze Top Row /
  First Column / Panes and Unfreeze (saved in the `.xlsx`; frozen columns stay in view, frozen rows
  are marked with a line but still scroll). Backspace clears a selected range. The status bar shows
  Average, Count and Sum for a selected range.
- **Sheets: Ask AI for a formula.** Describe what you want in English or Indonesian ("jumlah penjualan
  untuk region Jakarta"); Kantor sends only the column headers, up to 20 sample rows near the
  selection and the active cell's address, checks the suggested formula with IronCalc (it must parse
  and calculate) and shows it with a one-line explanation; *Insert* puts it in the active cell as one
  undo step. *Explain This Formula* explains the active cell's formula. Sparkle button in the
  toolbar while AI is on.

#### All apps
- **Find & Replace in Docs, Sheets and Slides.** Ctrl+F opens a small find bar at the top right
  (match count "3 of 12", Enter / Shift+Enter or F3 / Shift+F3 for next / previous, Match case and
  Whole word toggles, Esc closes); Ctrl+H adds the replace row, and Replace All is one undo step.
  Docs highlights every match on the page and selects the current one; Sheets searches the active
  sheet's displayed values (or formulas, with "Look in formulas") and selects the cell; Slides
  searches every slide's text and jumps to it. Also in the palette (Find, Replace).
- **Untitled documents and tooltips.** New documents are named Untitled.xlsx, Untitled 2.xlsx …
  per app, and every toolbar button has a tooltip with its shortcut ("Bold · Ctrl+B").
- **AI features, opt-in.** *AI Settings* (command palette) has a master switch (off by default),
  a provider picker (Anthropic, OpenAI, OpenRouter, Groq, DeepSeek, Google Gemini, Ollama running
  locally, or any OpenAI-compatible server), the API key (stored in the system keychain, never in the
  settings file), base URL, model and *Test connection*. Requests go only to the provider you chose;
  Kantor runs no AI service of its own.
- **AI agents can edit open documents (`kantor mcp`).** An MCP server on stdio for Claude Code,
  Claude Desktop, Cursor and other agents (`claude mcp add kantor -- kantor mcp`; JSON setup via
  `kantor mcp --print-config` or the palette's *Copy MCP Setup for Claude Code*). It talks to the
  running Kantor over a private Unix socket (started on demand) and offers 26 tools: list, open,
  create, save and show documents; read and write cell ranges, select, format, insert and delete
  rows/columns in Sheets; read, insert, replace and delete paragraphs and find & replace in Docs;
  list slides, add a slide with title and bullets, set a shape's text or the speaker notes, delete
  and show slides in Slides. Each change is one undo step, selects what changed and shows
  "Agent: …" in the status bar. Speaks MCP 2026-07-28 (stateless) and the 2025-11-25 … 2024-11-05
  handshake revisions.

#### Command line
- **`kantor convert`** converts documents without opening a window (no display needed):
  `.docx` → `.pdf`, `.pptx` → `.pdf`, and spreadsheets (`.xlsx`, `.xlsm`, `.xls`, `.xlsb`, `.ods`,
  `.csv`, `.tsv`) → `.xlsx`, `.csv` or `.tsv` (`--sheet` picks the sheet). `kantor convert IN OUT`,
  or `-o DIR --to FORMAT FILES…` for many. It never overwrites without `-f` and never writes over
  its input. Spreadsheet → PDF is not supported yet.
- **`kantor pdf`** tools for PDF files: `merge` (with page ranges per file), `split`, `extract`,
  `delete`, `reorder`, `rotate`, `info`, `images` (JPEG/PNG to PDF), `watermark`, `page-numbers`
  and lossless `compress`. Password-protected PDFs open with `--password`. Outputs are never
  overwritten without `-f`.

#### Downloads and install
- **macOS.** Kantor runs on macOS: `Kantor.dmg` is a universal app (Apple Silicon and Intel,
  macOS 11 or later), signed with a Developer ID and notarised by Apple, so it opens with a
  double-click. Shortcuts use ⌘ instead of Ctrl (Ctrl+Tab stays), and shortcut labels show ⌘⇧⌥.
- **Linux AppImages** for x86_64 and aarch64 (`Kantor-x86_64.AppImage`,
  `Kantor-aarch64.AppImage`): download, make executable, run. They need glibc 2.39 or newer
  (Ubuntu 24.04, Debian 13, Fedora 40 or later), like the tarballs.
- **Stable download links.** Every release has `Kantor.dmg`, `Kantor-x86_64.AppImage` and
  `Kantor-aarch64.AppImage` under the same names (plus versioned copies), so
  `https://github.com/dzulfikar08/kantor/releases/latest/download/<name>` always points at the
  newest release.
- **Install script:** `--uninstall` removes what it installed (binary or link, Kantor.app,
  launchers, icons; settings are kept); when `~/.local/bin` is not on `PATH` it prints the line to
  add for zsh, bash or fish; macOS installs from tagged releases (v0.2.0 on), with a clear message
  for releases that have no macOS build.

### Changed
- Release builds use fat LTO and one codegen unit: the Linux binary shrank from 60.3 MB to
  51.0 MB.
- The command palette reacts faster: typing redraws only the palette, and the list of system fonts
  is read once (on macOS each letter took about 190 ms before).

### Fixed
- Sheets: approximate lookups (`VLOOKUP`/`HLOOKUP`/`LOOKUP` with sorted data, `MATCH` with match
  type 1 or -1) stop at the last non-empty cell, as Excel does. Before, a lookup over whole
  columns could land on a blank row and return 0.
- Docs: the ruler is drawn in the theme's chrome colours instead of paper white.
- Release notes and README no longer claim a `kantor-bin` AUR package: it was prepared
  (`packaging/aur/`) but never published.

### Known limitations
- Not yet checked against a large set of real-world files; expect layout differences from Word
  and PowerPoint.
- Docs: headers and footers, footnotes, fields and floating objects are kept in the file but shown
  only as placeholders, and are not in the PDF; tracked changes, comments and tables of contents make Save go to a new file.
- Slides: charts other than the types above show a placeholder; the content of charts, tables,
  groups and SmartArt can't be edited yet. On screen, rotated text stays upright (the PDF turns it).
- Sheets: no sort, filter, charts, cell colours/borders editing, printing or autosave yet; `.ods`,
  `.xls` and `.xlsb` are import-only.
- `kantor convert` can't turn spreadsheets into PDF yet.
- The Linux builds need glibc 2.39 or newer.

## [0.1.0] - 2026-11-07

First tagged release: the **Sheets MVP**. Docs and Slides are not started yet.
Prebuilt for x86_64 and aarch64 Linux (built on Ubuntu 24.04, glibc 2.39; runs on Arch
and on Asahi's 16K-page kernel).

### Added

#### Suite shell
- One binary for the whole suite: `kantor` opens the Home screen (Docs · Sheets · Slides),
  `kantor file.xlsx` or `kantor --sheets` goes straight to Sheets.
- Document tabs in the titlebar, split panes (`Ctrl \` / `Ctrl Alt \`) with draggable
  dividers, a resizable sidebar (open, recent, create) and inspector (cell and workbook
  details), and a status bar.
- Command palette (`Ctrl K` / `Ctrl Shift P`) with fuzzy search.
- Themes: follows the Omarchy theme by default and live-reloads on `omarchy theme set`;
  the theme picker also offers the Omarchy themes and 907 Ghostty themes with live preview.
- UI font picker (SF Pro when installed, never bundled; else Inter / Adwaita Sans).
- Open tabs, splits, panel sizes and recent files are restored on the next launch
  (`~/.local/state/kantor/workspace.json`; settings in `~/.config/kantor/settings.toml`).
- Asks to Save / Don't Save / Cancel before closing a pane or tab, quitting (`Ctrl Q`) or
  closing the window with unsaved changes.
- Renderer memory patch (vendored `gpui-pre-wgpu`): path textures are allocated only when
  used and path MSAA is off by default, which fixes wgpu *Out of Memory* crashes on 8 GB
  machines with a browser open.

#### Sheets
- Open, edit and save `.xlsx` through IronCalc 0.9.1: formulas recalculate, multiple
  sheets, atomic save.
- CSV/TSV open and save: delimiter detection, UTF-8 BOM / UTF-16 / Windows-1252 input; one
  sheet of displayed values; a leading `=` stays text.
- Read-only import of `.ods`, `.xls` and `.xlsb` (values, dates, formulas). Formulas that
  don't reproduce the file's cached result are kept as values. Saving an imported file
  goes to Save As `name.xlsx`.
- Undo/redo (`Ctrl Z`, `Ctrl Shift Z` / `Ctrl Y`); the unsaved marker follows the undo
  history, so undoing back to the saved state makes the document clean again.
- Range selection (Shift+arrows, Shift+click, drag) and copy/cut/paste of ranges as TSV
  (`Ctrl C`/`X`/`V`), to and from other apps; each paste or clear is one undo step.
- Excel-style editing: type to replace, `Enter`/`F2`/double-click to edit, `Esc` to
  cancel, `Delete` to clear; name box and formula bar.
- In-cell editor over the active cell, synced live with the formula bar (IME input goes
  through GPUI); Enter/Tab and their Shift variants commit and move.
- Formatting: bold, italic, underline, strikethrough (`Ctrl B`/`I`/`U`), horizontal
  alignment, number formats (General, Number, Percent, Rupiah, Dollar, Date) and column
  widths, all saved to `.xlsx`. A toolbar above the formula bar collapses into a "…" menu
  when the window is narrow.

#### Project
- Install script: `curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh`
  (nightly), or `KANTOR_VERSION=v0.1.0` for this release.
- A PKGBUILD for an AUR package `kantor-bin` (`packaging/aur/`; not published).
- xlsx fidelity corpus (12 generated files) with a round-trip test and a Fidelity CI job.
- Round-trip safety for `.xlsx`: comments, custom properties, drawings/charts/images, data
  validation, print setup and other parts IronCalc doesn't model are carried over from the
  original file on save.

### Known limitations
- **Docs and Slides** are placeholders ("coming soon"). A text-layout prototype
  (`docs_layout`, parley) exists but has no UI yet.
- **Row heights** are read and saved but not drawn: every row has the same height.
- Cell colours, borders and merged cells are kept in the file but can't be edited yet.
  No sort, filter, find and replace, fill handle, frozen panes, printing or autosave.
- Pasted formulas are not reference-shifted (`=A1` stays `=A1`).
- The in-cell editor has no Alt+Enter line breaks and hides if the cell is scrolled away.
- Splitting a pane opens a new blank sheet; the same workbook can't be shown in two panes.
- `.ods`, `.xls` and `.xlsb` are import-only; Kantor never writes them.
- IronCalc 0.9.1 file-fidelity gaps (full list in `tests/corpus/xlsx/known-gaps.tsv`):
  - 3-D references (`SUM(Jan:Mar!B2:B4)`) evaluate to `#ERROR!` in Kantor; the file is saved
    with the formula and no cached value, so Excel/LibreOffice recalculate it on open.
  - Parts Kantor doesn't carry over on save (each reported, never silently): tables, pivot
    tables, external links, VBA macros, chart sheets, embedded objects/form controls, slicers.
  - Defined names holding a constant (`Ten = 10`) give `#NAME?`.
  - Dashed and hair borders become thin borders.
  - Number formats: locale-tagged currency (`[$Rp-421]#,##0`) shows `#VALUE!`; fraction
    formats are unsupported; built-in date format 14 shows as `mm-dd-yy`; times can lose a
    minute to rounding; `0` rounds halves to even (1234.5 → 1234; Excel gives 1235).
  - `LEN` counts Unicode scalar values, not UTF-16 code units as Excel does (emoji differ).
  - `INDEX(...)` formulas display as `@INDEX(...)`; quotes around non-ASCII sheet names are
    dropped from formula text; shared strings lose leading/trailing spaces in strict readers.
- Not yet verified on the M1 Air: fractional scale 1.6, fcitx5 composition, memory use
  with a 10k-cell sheet, Hyprland's close-window path for the unsaved-changes prompt.

[Unreleased]: https://github.com/dzulfikar08/kantor/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/dzulfikar08/kantor/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/dzulfikar08/kantor/releases/tag/v0.1.0
