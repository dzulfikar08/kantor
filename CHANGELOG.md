# Changelog

All notable changes to Kantor are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, minor versions may break things).

The release workflow (`.github/workflows/release.yml`) uses the section for a tag as
that GitHub Release's notes, so every `v*` tag needs a `## [x.y.z]` heading here.

## [Unreleased]

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
  - **Data loss on save:** cell comments and custom document properties are dropped.
  - 3-D references (`SUM(Jan:Mar!B2:B4)`) evaluate to `#ERROR!`, and that result is saved as
    a non-OOXML error code that Excel may report as damage.
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
