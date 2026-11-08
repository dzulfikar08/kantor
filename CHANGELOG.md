# Changelog

All notable changes to Kantor are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, minor versions may break things).

The release workflow (`.github/workflows/release.yml`) uses the section for a tag as
that GitHub Release's notes, so every `v*` tag needs a `## [x.y.z]` heading here.

## [Unreleased]

### Added
- **Docs** (first slice): open, edit and save `.docx` on A4 pages laid out with parley — typing,
  selection, Bold/Italic/Underline, headings (Ctrl+Alt+0–3), alignment, undo/redo, zoom, plain-text
  clipboard, IME input. New Document is Ctrl+Shift+N. Saving an opened `.docx` rewrites only the
  body and copies every other part unchanged; files with content Docs doesn't model yet (tables,
  images, lists, headers/footers, comments, tracked changes…) show a banner and save to a new file.
- **Slides** (first slice): new presentation (Ctrl+Alt+N, Home, sidebar, palette) and open/save
  `.pptx`. Slides look like the file: placeholder positions and text formatting come from the
  layout, master and theme, and master/layout decorations, backgrounds and pictures are drawn.
  Thumbnail strip (drag to reorder, Ctrl+Up/Down), new/duplicate/delete slide (Ctrl+M / Ctrl+D),
  select, move (drag or arrows) and resize shapes, edit text in text boxes and placeholders
  (Bold/Italic/Underline, IME), insert text box, rectangle, ellipse or PNG/JPEG picture, undo/redo,
  and a simple presenter mode (F5 / Shift+F5; arrows, space, Esc). Saving copies every part it
  doesn't rewrite byte for byte, keeps unchanged shapes verbatim and, for edited ones, replaces
  only the position or the paragraphs; tables, charts, groups and SmartArt are kept untouched
  (shown as boxes). When slides are added, removed or reordered in a file with slide sections or
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
- AUR package `kantor-bin`.
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

[Unreleased]: https://github.com/dzulfikar08/kantor/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/dzulfikar08/kantor/releases/tag/v0.1.0
