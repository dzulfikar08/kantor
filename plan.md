# Kantor — a native Rust office suite on GPUI

> **Kantor** is a working name (Indonesian for "office"). Rename freely.

A small, fast, native office suite for Linux (first target: Omarchy on Apple Silicon), built in Rust on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), the UI framework behind the Zed editor. Three apps: **Docs** (Word), **Sheets** (Excel), **Slides** (PowerPoint). They share one binary and one core.

Status: **M0 done; Sheets MVP (M1) feature-complete pending Mac checks; Docs (M2) and Slides (M3) first slices in**. Written 2026-11-07, last updated 2026-11-07.

---

## 0. Where we are / pick up here

**Repo:** https://github.com/dzulfikar08/kantor (public, AGPL-3.0-or-later). `main` is always green; every push publishes a **nightly** aarch64 + x86_64 build.

**What works today** (nightly, verified running on the M1 Air under Asahi):
- One binary, suite shell modelled on Voltrus Tools (`docs/reference-voltrus-tools.md`): Home screen, document **tabs** (tab strip = titlebar), **split panes** with draggable dividers, resizable **sidebar** (open/recent/create) and **inspector** (cell + workbook details), **status bar**, **Ctrl+K command palette** (fuzzy), **theme picker** (Follow Omarchy / 23 Omarchy / 907 Ghostty themes, live preview), **UI font picker** (SF Pro when installed). Tabs, layouts and panel sizes **persist** across runs.
- **Sheets**: open/edit/save `.xlsx` via IronCalc (formulas recalc, atomic save), Excel-style typing into cells (type / Enter / F2 / Delete / Esc), name box + formula bar, multiple sheets.
- **Docs** (M2 first slice, not yet tried on the Mac): new document (Ctrl+Shift+N, Home, sidebar, palette) and open/save `.docx`: paragraphs, runs (font, size, bold/italic/underline, colour), paragraph styles (Normal, Heading 1–3, the file's own), alignment and spacing. Paged A4 view (parley layout, GPUI paints the glyphs, only visible pages), zoom, caret/selection, typing via GPUI's input handler (IME), Enter/Backspace/Delete, arrows/Home/End/PageUp/PageDown, Ctrl+B/I/U, undo/redo, plain-text clipboard. **Round-trip safety:** saving copies every package part byte for byte and regenerates only the body; a file with content the model doesn't hold (tables, images, fields, comments, tracked changes, headers/footers, lists…) is marked and its Save goes to Save As: the original is never overwritten (`crates/docs_engine/src/docx.rs`).
- **Slides** (M3 first slice, not yet tried in a window): new presentation (Ctrl+Alt+N, Home, sidebar, palette; 16:9 with "Title Slide" and "Title and Content" layouts) and open/save `.pptx`. Display inheritance (read-only): placeholder positions, list styles and body settings from layout/master, theme colours (incl. lumMod/lumOff) and fonts, backgrounds, master/layout decorations, pictures. Editing: thumbnail strip (virtualised, drag to reorder, Ctrl+Up/Down), new/duplicate/delete slide (Ctrl+M / Ctrl+D), select/move (drag, arrows)/resize (handles), text editing in text boxes and placeholders (caret, selection, Enter/Shift+Enter, Ctrl+B/I/U, IME via `EntityInputHandler`), insert text box/rectangle/ellipse/picture, Delete, undo/redo (typing/nudges coalesce), Save/Save As, F5 presenter mode in the pane. Text is laid out by `docs_layout` and painted through Docs' font bridge. **Round-trip safety** (`crates/slides_engine/src/presentation.rs`): every part not rewritten is raw-copied; unchanged slides are copied, unchanged shapes written verbatim, moved shapes get only `a:xfrm` replaced, edited text only its paragraphs (each run's/paragraph's unmodelled attributes and children kept); tables, charts, groups, SmartArt, transitions and timing stay untouched. Text with fields/equations is shown but locked; slides with unusual XML prefixes are kept verbatim and locked. Deleting a slide also drops its notes. Unsupported content (slide sections / custom shows after slides are added, removed or reordered) forces Save As.
- Design system in `kantor_ui` (metrics + colour tokens derived from the palette; "views never use raw colours or sizes").
- Renderer memory patch (`vendor/gpui-pre-wgpu`) — fixed the wgpu *Out of Memory* crash on 8 GB with a browser open.

**Machines and rules** (details in §8):
- **M1 Air**: target only. Never build or install toolchains there; install the nightly with `curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh`.
- **Linux box** (x86_64, 30 GB RAM): fine for local `cargo build`/`cargo test`/`cargo run`. Setup: `rustup` stable, then on Arch `sudo pacman -S --needed base-devel clang cmake wayland libxkbcommon vulkan-icd-loader fontconfig freetype2 alsa-lib openssl zstd dbus` (Ubuntu: `./scripts/ci-deps.sh`). `cargo run -p kantor` / `cargo test --workspace`.
- **GitHub Actions** does the real aarch64 builds and releases. `housekeeping.yml` auto-applies `cargo fmt` and refreshes `Cargo.lock` on `main` (it commits as github-actions — `git pull --rebase` before pushing). PRs are checked strictly (fmt + clippy `-D warnings` + tests on both arches).

**Next up, in order** (each item ends with a green CI and a nightly to try on the Mac):
1. **Confirm on the Mac:** fractional scale 1.6 looks sharp, fcitx5 typing in the formula bar, RSS with a 10k-cell sheet (< 150 MB target), long session with Helium open (memory patch).
2. ✅ **Unsaved-changes prompt** (Save / Don't Save / Cancel) when closing a pane or tab, quitting (Ctrl+Q), or closing the window from Hyprland. Save stores each dirty document in turn, untitled ones via Save As, and closes only if all were saved. ☐ Try it on the Mac (Hyprland's close-window path is untested).
3. ✅ **Sheets undo/redo**: `sheets_engine` runs on IronCalc's `UserModel`; Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y in the grid (the formula bar keeps its own text undo), plus palette commands. "Unsaved" follows the undo history, so undoing back to the saved state is clean again.
4. ✅ **Copy/paste** of ranges as TSV (Ctrl+C/X/V in the grid; one undo step per paste/clear), range selection with Shift+arrows, Shift+click and drag. ☐ Try on the Mac: mouse selection, clipboard to/from browser/LibreOffice. Pasted formulas are not reference-shifted yet (`=A1` stays `=A1`).
5. ✅ **In-cell editor** over the active cell (gpui-component `Input`, so IME goes through GPUI), synced live with the formula bar; Enter/Tab/Shift variants commit and move, typed edits commit on arrows, F2 edits switch arrows to caret mode. ☐ Try on the Mac: alignment while scrolling/resizing, fcitx5 composition and popup position. Gaps: no Alt+Enter line breaks; editor hides if the cell is wheel-scrolled away.
6. ✅ **Formatting basics**: bold/italic/underline/strike (Ctrl+B/I/U), alignment, number formats (General/Number/Percent/Rupiah/Dollar/Date), column widths saved to xlsx (resize is undoable); toolbar above the formula bar that collapses into a "…" menu when narrow. ☐ Try on the Mac. Gaps: row heights stored but not drawn (DataTable has one row height); colours/borders not wired; IronCalc rounds halves to even.
7. ✅ **CSV/TSV open and save** (delimiter detection, BOM/UTF-16/Windows-1252; one sheet, displayed values; a leading `=` stays text) and **read-only `.ods`/`.xls`/`.xlsb` import** via calamine (values, dates, formulas; formulas that don't reproduce the file's cached result are kept as values; Save on an import goes to Save As `name.xlsx`). See `crates/sheets_engine/src/import.rs`. ☐ Open the same workbook in two panes (needs a shared document model; today a split opens a new blank sheet).
8. ✅ **Text layout spike** → [`docs/decisions/0001-text-engine.md`](docs/decisions/0001-text-engine.md): **parley** lays out (GPUI-free `crates/docs_layout`: pages, widow/orphan, hit-testing, krilla PDF), GPUI only paints its glyphs via `Window::paint_glyph` through a verified fontique → `FontId` bridge. ~110 ms per 20 A4 pages full layout, ~50 µs per paragraph edit. Biggest risk: GPUI's rasterizer ignores variable-font axes and synthetic bold/italic.
9. ◐ **Corpus + fidelity CI**: ✅ `.xlsx`: 12 generated CC0 files in `tests/corpus/xlsx/` (`scripts/corpus/gen_xlsx.py`), round-trip test `crates/sheets_engine/tests/corpus.rs` with known IronCalc gaps in `known-gaps.tsv`, `fidelity.yml` (report + LibreOffice-headless PDFs as an artifact). ☐ `.docx`/`.pptx` corpora with Docs/Slides; ☐ pixel-diff score.
   ✅ **xlsx part preservation** (§5): `Workbook::save_as` post-processes IronCalc's package (`crates/sheets_engine/src/preserve.rs`) and carries what IronCalc drops from the original: comments + VML, custom properties, drawings/charts/images, printer settings, custom XML, threaded comments, and sheet elements (`sheetPr`, `sheetFormatPr`, data validation, print setup, `legacyDrawing`...), matched to sheets by `sheetId`; tab colours are written from the model; non-OOXML error values (`#ERROR!` from 3-D refs) are no longer written. Anything ambiguous is skipped and listed in `Workbook::save_notes()`; the result is checked with `package_problems` and falls back to IronCalc's output. Not carried yet: tables, pivots, external links, VBA, OLE/controls.
10. ◐ First tagged release `v0.1.0` (Sheets MVP) and an AUR `kantor-bin`: ✅ prepared — `release.yml` (tag `v*` must match the Cargo.toml version; tarballs + `SHA256SUMS`, notes from `CHANGELOG.md`), version 0.1.0, `packaging/aur/` (`update.sh`, publish steps in its README). ✅ Released 2026-11-07: https://github.com/dzulfikar08/kantor/releases/tag/v0.1.0 (both arches + `SHA256SUMS`); PKGBUILD checksums filled. ☐ Publish `kantor-bin` to the AUR (needs the user's AUR SSH key; steps in `packaging/aur/README.md`).
11. ◐ **Docs M2, first slice** (`docs_engine`, `docs_ui`, shell wiring): see *What works today*. ☐ Try on the Mac: glyphs land where parley put them in a real window (ADR 0001 open risk 6), fcitx5 composition and candidate window, scrolling/zoom feel, a few real `.docx` files. Next: tables (read-only display first), lists, page breaks, tab stops, rich clipboard, `.docx` corpus + fidelity CI.
12. ◐ **Slides M3, first slice** (`slides_engine`, `slides_ui`, shell wiring): see *What works today*. ☐ Try in a window / on the Mac: canvas and thumbnails render where expected, drag/resize feel, text caret placement in placeholders, fcitx5, presenter mode, a few real `.pptx` decks (open → edit → save → open in PowerPoint/LibreOffice). Next: rotation/flip display, bullets and paragraph spacing, tables and charts (display), shape fill/line editing, copy/paste of shapes, speaker notes, PDF export, a `.pptx` corpus + fidelity CI.

**Known gaps / caveats:** Slides gaps: no rotation/flip display (kept in the file), no bullets/numbering or paragraph spacing in the display, tables/charts/groups/SmartArt shown as boxes (kept), gradients drawn as their first colour, no shape copy/paste, fill/line editing, notes view or PDF export; text with fields (slide numbers, dates) can't be edited. Docs gaps: no tables/images/lists/headers yet (such files open with a warning banner and save as a copy), tabs render as one space (no tab stops), "at least" line spacing is approximated, the clipboard is plain text, no spell check, PDF export or print yet. Split-right uses the dashboard icon (GPUI Kit's default icon set has no columns icon). Window title/decorations come from Hyprland (server-side); no in-app window buttons, by design.

## 1. Why

On an 8 GB M1 MacBook Air running Omarchy (aarch64, 16K-page kernel), every option today trades something away:

| Option | Problem |
|---|---|
| LibreOffice | 466 MB installed, classic UI |
| OnlyOffice | ~400 MB, embedded Chromium, uses a lot of RAM |
| WPS Office | x86-only |
| AbiWord + Gnumeric | Small and native, but no slides app, dated UX, weaker DOCX fidelity |
| Web apps (M365 / Google) | Need internet and an account |
| [rust-office](https://github.com/rsasaki0109/rust-office) (egui) | ★9, experimental, no ARM build |
| [Off-Ice](https://github.com/yugaaank/off-ice) (GPUI) | ★0, Writer MVP only, Sheets and Slides are stubs |

Nobody has built a serious office suite on GPUI. The building blocks now exist in Rust: GPUI and GPUI Kit for the UI, IronCalc for spreadsheets, docx-rs for Word files, krilla for PDF, parley and swash for text. So the gap can be filled.

## 2. Goals and non-goals

### Goals (targets, measured on the M1 Air)

| Goal | Target |
|---|---|
| Install size | **< 40 MB** (one binary plus assets; fonts and dictionaries come from the system) |
| Cold start | **< 300 ms** to an empty document |
| Memory | **< 150 MB** with a typical 20-page document or a 10k-cell sheet open |
| Office files | Open and save `.docx`, `.xlsx`, `.pptx` as **first-class formats**, not "import" |
| Round-trip safety | Opening and saving a file **never drops content we don't understand** (see §5) |
| Look and feel | Native GPUI, keyboard-first, follows the Omarchy theme, Mac-style shortcuts |
| Platform | aarch64 Linux first (Asahi/Aurora, 16K pages); x86_64 Linux second |
| Output | Accurate PDF export and printing |

### Non-goals for 1.0

- Macros, VBA or scripting
- Real-time collaboration (keep the model CRDT-friendly, but don't build sync)
- Full track changes and comments UI (preserve them in files, read-only display at most)
- Mail merge, pivot tables, advanced charts, embedded OLE objects, SmartArt editing
- Legacy binary formats (`.doc`, `.xls`, `.ppt`) beyond, at most, read-only
- Windows and macOS builds (GPUI supports them, so this stays possible later)

## 3. Strategy: Sheets first, then Docs, then Slides

The order follows **how much existing Rust code carries the load**:

1. **Sheets first.** [IronCalc](https://github.com/ironcalc/IronCalc) (`ironcalc_base`, Apache-2.0) already provides an Excel-compatible formula engine **and** an `.xlsx` reader and writer. GPUI Kit's data table already does virtual scrolling across hundreds of thousands of rows. Sheets is mostly UI work on proven parts, so it's the fastest route to something useful.
2. **Docs second.** This is the hardest part: a **paged rich-text layout engine** (line breaking, pagination, tables, headers and footers, floating images) has to be written. `docx-rs` (MIT, read and write) covers the file format. Building it second means it can reuse the core, theming, file handling and PDF export proven in Sheets.
3. **Slides last.** There's no mature Rust PPTX library (`ppt-rs` 0.2 is young), so the format layer is ours. But slides reuse the Docs text engine (text boxes) and a simple scene model (shapes and images), so it's mostly composition.

Each app is **released on its own** as soon as it reaches its MVP. There's no waiting for all three.

## 4. Architecture

### 4.1 Workspace layout

✅ = exists today; the rest arrive with their milestone.

```
kantor/
├── crates/
│   ├── kantor            # ✅ binary: CLI, suite shell (workspace/: tabs, splits, panels, palette, home), actions
│   ├── kantor_ui         # ✅ design system: metrics, Omarchy-derived tokens, CachedView, fonts, small components
│   ├── kantor_core       # ✅ no GPUI: fuzzy search, split layout tree, session + settings persistence
│   │                     #    (later: units, styles, document trait, undo/redo command log, autosave)
│   ├── kantor_theme      # ✅ Omarchy palette parser + theme catalog (Omarchy themes, 907 Ghostty themes)
│   ├── kantor_text       # rich-text model + paragraph layout (shared by Docs and Slides)
│   ├── kantor_ooxml      # shared OOXML plumbing: zip, [Content_Types], relationships,
│   │                     #   theme/style parts, unknown-part preservation
│   ├── kantor_pdf        # PDF export via krilla
│   ├── sheets_engine     # ✅ thin wrapper over ironcalc (open/save xlsx, cells, formulas)
│   ├── sheets_ui         # ✅ grid, formula bar, sheet tabs (later: in-cell editor, selection, fill handle)
│   ├── docs_engine       # ✅ no GPUI: document model, styles, undo/redo, .docx read/write + preservation
│   │                     #    (later: sections, tables, images; may split into docs_model / docs_docx)
│   ├── docs_layout       # ✅ no GPUI: parley paragraphs → pages (later: tables, headers/footers)
│   ├── docs_ui           # ✅ page view, caret, selection, IME, toolbar (later: styles panel)
│   ├── slides_engine     # ✅ no GPUI: slides/shapes/text model, undo/redo, .pptx read/write + preservation,
│   │                     #    layout/master/theme inheritance for display (may split into slides_model / slides_pptx)
│   └── slides_ui         # ✅ slide canvas, thumbnail strip, text editing, presenter mode
├── vendor/gpui-pre-wgpu  # ✅ GPUI's wgpu renderer + Kantor memory patches (KANTOR_PATCHES.md)
├── assets/               # ✅ icons (suite K, Sheets), themes/ghostty.tsv (MIT)
├── docs/                 # ✅ reference-voltrus-tools.md, decisions/
├── tests/corpus/         # real-world .docx/.xlsx/.pptx samples (see §7)
└── packaging/            # ✅ .desktop files (later: PKGBUILD, MIME types)
```

One binary, `kantor`, holds all three apps: **an all-in-one suite**. `kantor` with no file opens a Home screen (Docs · Sheets · Slides · Open file); a file or `--docs/--sheets/--slides` jumps straight into an app. A main `Kantor` launcher plus per-app launchers (`kantor-sheets.desktop` now, Docs/Slides as they land) all start the same binary. Icons live in `assets/icons/` (the K is Docs-blue stem, Sheets-green arm, Slides-orange leg).

### 4.2 Key libraries

| Need | Crate | Notes |
|---|---|---|
| UI framework | `gpui` 0.2.x (Apache-2.0) | Pre-1.0, so the API changes. Pin it through `gpui-kit`. |
| Components | `gpui-kit` / `gpui-component` 0.9 | 75+ components, data table, inputs, menus, dialogs. Check its license before depending on it. |
| Spreadsheet engine + xlsx | `ironcalc_base`, `ironcalc` 0.9.1 (Apache-2.0) | Formulas, functions, xlsx read/write. Pre-1.0. |
| Extra xlsx reading | `calamine` 0.38 | Fallback for odd files, `.ods`, and read-only `.xls` |
| DOCX | `docx-rs` 0.4.22 (MIT) | Reads (`read_docx`) and writes to one model. Extend it, or fork if needed. |
| PPTX | own code on `quick-xml` + `zip` | Study `ppt-rs` and `ooxmlsdk` as references |
| Text shaping and layout | `parley` 0.12 + `swash` (Apache/MIT) | Decide in the M0 spike (§6): parley, or GPUI's own text system, for paged layout |
| Text editing buffer | `ropey` | For long documents |
| Unicode | `unicode-segmentation`, `hyphenation` | Word and line breaks, hyphenation |
| PDF export | `krilla` 0.8 (Apache-2.0) | High-level PDF writer from the Typst ecosystem |
| PDF rendering (tests) | `hayro` | Rasterize PDFs for visual diff tests |
| Spell check | `spellbook` 0.4 | Uses the system's Hunspell dictionaries |
| Collaboration-ready model (later) | `loro` or `yrs` | Not in 1.0. Keep the model's operations CRDT-shaped. |

### 4.3 Core design rules

1. **Model / layout / view are separate.** The model is plain data. Layout is a pure function of model plus fonts, giving positioned boxes. The GPUI view only paints boxes and handles input. That makes layout testable without a window.
2. **Every edit is a command.** Every change goes through `kantor_core::Command`, which gives undo/redo, autosave journaling and a future CRDT path for free.
3. **Preserve what we don't understand** (§5).
4. **Incremental everything.** Re-lay out only dirty paragraphs and pages; recalculate only dirty cells (IronCalc handles that); paint only the visible viewport.
5. **No Chromium, no webview, no Electron.** One native binary.

## 5. File format fidelity (the make-or-break part)

Office Open XML (ECMA-376) runs to thousands of pages, and no app implements all of it. Users only forgive "it looked slightly different". They never forgive "it **deleted** something".

- **Preservation layer.** `kantor_ooxml` keeps every package part and every XML element or attribute the model doesn't map, attached to the nearest model node. On save, mapped content is regenerated and unmapped content is written back untouched. That keeps comments, tracked changes, SmartArt, charts and custom XML alive through an edit-and-save cycle even before they're supported.
- **Read-only fallback.** If a file contains things we can't preserve safely, open it read-only with a clear banner ("Saving would lose: charts on page 4"), so data is never silently dropped.
- **Fonts.** Map Microsoft fonts to metric-compatible free ones so pagination matches: Calibri → **Carlito**, Cambria → **Caladea** (both in Arch's `extra` repo), and Arial/Times/Courier → Liberation. Ship the mapping table; recommend installing the fonts.
- **Fidelity is measured, not guessed** (§7): render our PDF and LibreOffice's PDF of the same file, diff them pixel by pixel, and track a score per corpus file in CI.

## 6. Milestones

Time estimates assume **one person, part-time (~10–15 h/week), with AI-assisted coding**. They're rough planning numbers, not commitments.

### M0 — Foundations (2–3 weeks)

- [x] Repo, workspace, AGPL-3.0-or-later licence, CONTRIBUTING, issue templates
- [x] CI: build and test on **x86_64** and **`ubuntu-24.04-arm`** (GitHub's free ARM runners for public repos), clippy, fmt, `cargo deny`
- [~] GPUI window on the M1 Air under Hyprland/Asahi: ✅ Vulkan, ✅ 16K-page kernel, ✅ ~210 ms to GPU-ready, ✅ 39 MB binary; ☐ fractional scale 1.6, ☐ fcitx5 input
- [x] `kantor_theme`: read Omarchy theme colors, live-reload on `omarchy theme set`
- [x] **Text layout spike** (decides the hardest technical question): lay out and paint 50 pages of mixed-font text with parley vs GPUI's text system; measure speed, memory, caret/selection quality and fcitx5 IME support. Pick one and write the decision down. → **parley**, see `docs/decisions/0001-text-engine.md` (painting in a real window and fcitx5 still to confirm with the first `docs_ui` element).
- [ ] Corpus started: 30+ real `.docx/.xlsx/.pptx` files (see §7)

**Exit:** a GPUI window on the Mac, themed, CI green on both architectures, text-engine decision recorded.

### M1 — Sheets MVP (6–10 weeks)

- [~] Open, edit and save `.xlsx` through IronCalc ✅; `.csv` import/export ☐; `.ods`/`.xls` read via calamine ☐
- [~] Grid: virtual scrolling ✅, multiple sheets ✅, resize columns ✅ (not saved yet); frozen panes ☐, row heights ☐
- [~] Cell editing ✅ (Excel-style keys), formula bar ✅; in-cell editor ☐, function autocomplete ☐, error display ☐
- [ ] Selection, copy/paste (including from Helium/web as TSV), fill handle, undo/redo
- [ ] Formatting: number formats, bold/italic/colors, borders, alignment, merge cells
- [ ] Sort, filter, find and replace
- [ ] Print / PDF export of a selected range
- [ ] Autosave plus crash recovery

**Exit:** you can do a real budget or invoice sheet start to finish and send the `.xlsx` to an Excel user, and it opens correctly. First tagged release; an AUR `kantor-bin` with an aarch64 binary.

### M2 — Docs MVP (3–5 months; the hardest milestone)

- [ ] `docs_model` + `docs_layout`: paragraphs, character and paragraph styles, lists, page size and margins, pagination, widow/orphan control
- [ ] Tables (merged cells, borders, auto/fixed widths), inline images, page breaks
- [ ] Headers, footers, page numbers, sections with different orientation
- [ ] Caret, selection, IME input (fcitx5), clipboard with rich text, undo/redo
- [ ] DOCX read/write via docx-rs plus the preservation layer; ODT read (stretch)
- [ ] Spell check (spellbook + Hunspell), word count, find and replace
- [ ] PDF export via krilla; printing
- [ ] Style panel, zoom, outline/navigation pane

**Exit:** open a typical 20-page report (CV, letter, thesis chapter), edit it, save it, and it reopens identically in Word or LibreOffice. The fidelity score on the corpus passes the threshold set in M0.

### M3 — Slides MVP (2–3 months)

- [~] PPTX read/write: ✅ placeholders, shapes, text boxes, images, verbatim preservation (own reader/writer in `slides_engine`, not yet a shared `kantor_ooxml`); ✅ masters/layouts read-only for display; ☐ notes view, ☐ editing masters/layouts
- [~] Slide canvas: ✅ move, resize, thumbnail strip with reorder, theme colours/fonts from the file; ☐ align guides, ☐ grid sorter view
- [x] Text boxes reuse the Docs text engine (`docs_layout` + `docs_ui`'s font bridge; no separate `kantor_text` yet)
- [~] Presenter mode: ✅ in-pane slide show (F5); ☐ fullscreen on one display, notes and timer on the other
- [ ] PDF export (one slide per page)

**Exit:** open a colleague's deck, fix a few slides, present it, save the `.pptx`, and it looks right in PowerPoint.

### M4 — 1.0 polish (ongoing)

- [ ] Accessibility (GPUI's a11y support), keyboard map docs, Mac-style shortcuts (Cmd = Super, matching our Hyprland bindings)
- [ ] Packaging: AUR (`kantor-bin` for aarch64 and x86_64), `.desktop` files, MIME associations, icons, Omarchy plugin or menu entries
- [ ] Performance budget checks in CI (§2 targets)
- [ ] Charts (read and display first, then basic editing), comments display, track-changes display

**Realistic total to a usable 1.0:** about **9–15 months** part-time. Sheets alone could be useful in about 2–3 months.

## 7. Testing and quality

| Layer | How |
|---|---|
| Model / format | Unit tests; **round-trip tests**: open → save → reopen must give an identical model *and* identical preserved parts |
| Layout | Golden tests: model in, page boxes out as JSON snapshots (`insta`) |
| Visual fidelity | Our PDF vs **LibreOffice headless** PDF of the same file (run in CI), rasterized with `hayro`, pixel-diff score per file, tracked over time |
| Formulas | IronCalc's own test suite, plus our sheets from the corpus evaluated against Excel-computed cached values stored in the xlsx |
| UI | GPUI Kit headless UI tests (drive keyboard/mouse, assert state) |
| Performance | `criterion` benchmarks for layout and recalculation; startup time and RSS checked in CI against §2 targets |
| Fuzzing | `cargo fuzz` on the three format parsers; malformed files must never crash or hang |

**Corpus:** collect real files (your own, public government and university templates, the ECMA-376 sample files, plus LibreOffice's and Apache POI's public test documents, after checking each one's licence). Never commit private documents. Keep a `corpus/README.md` listing each file's source and licence.

## 8. Development environment and resources

The M1 Air has **8 GB RAM** and is the *target*, not a build machine. **Nothing is compiled or installed on the Mac** — not even `rustup`. The project is public so GitHub's free runners do the release work:

| Where | Use for |
|---|---|
| **M1 Air** (aarch64, the real target) | Editing code, running prebuilt binaries, manual testing |
| **Linux box** (x86_64, 30 GB RAM) | Local development: `cargo run -p kantor`, `cargo test --workspace`, `cargo clippy`. Fast iteration on UI; still push to get the aarch64 build |
| **GitHub Actions `ubuntu-24.04-arm`** | aarch64 tests and release builds (native ARM, free for public repos) |
| **GitHub Actions `ubuntu-24.04`** | x86_64 tests and release builds, fmt, clippy, cargo-deny, `Cargo.lock` upkeep, later LibreOffice-headless fidelity diffs |

Workflow:

1. Edit (on the Mac or the Linux box), push a branch (or `main`).
2. `ci.yml` runs clippy and tests on both architectures (fmt is checked on PRs); `housekeeping.yml` applies `cargo fmt` and refreshes `Cargo.lock` on `main` and commits the result — always `git pull --rebase` before pushing.
3. `build.yml` publishes a rolling **nightly** release from `main` (and versioned releases from `v*` tags).
4. On the Mac: `scripts/install.sh` downloads the aarch64 build into `~/.local/bin` (sha256-checked). `gh run watch` shows CI progress.

Binaries are built on Ubuntu 24.04 (glibc 2.39), so they run on Arch. GNU ld's default 64K max page size on aarch64 keeps them compatible with the Asahi 16K-page kernel.

## 9. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| **Paged text layout is very hard** (tables, floats, bidi, CJK) | High | Sheets first; dedicated M0 spike; scope tables and floats tightly in M2; study Typst's layout design |
| **GPUI is pre-1.0** and changes often | Medium | Pin through `gpui-kit`; isolate GPUI behind the view layer (model and layout don't import it); upgrade on a schedule |
| **GPUI on Asahi** (Vulkan driver maturity, 16K pages) | Medium | 16K pages and Vulkan verified working (2026-11-07). Under memory pressure (≈1 GB free, browser open) wgpu reports *Out of Memory* / device lost and GPUI panics after 3 recoveries; with the browser closed it runs fine. **Mitigated 2026-11-07** by `vendor/gpui-pre-wgpu` (path textures only when used, no path MSAA by default): no crash in repeated launches with the browser open. Still to do: upstream the patch; ask GPUI to recover instead of panicking |
| **OOXML breadth** | High | Preservation layer from day one; read-only fallback; a fidelity score drives priorities |
| **Microsoft font metrics** | Medium | Carlito/Caladea/Liberation mapping; warn when a document font is missing |
| **IronCalc is pre-1.0** | Low–medium | Keep it behind `sheets_engine`; contribute fixes upstream |
| **Solo-developer scope creep** | High | Ship each app's MVP on its own; strict non-goals list; milestones end with releases |
| **IME / input methods** (fcitx5) | Medium | Test in M0; GPUI has IME support but needs checking on Linux/Wayland |

## 10. Prior art to study

- **[IronCalc](https://github.com/ironcalc/IronCalc)**: engine plus its own web UI (grid interactions)
- **[rust-office](https://github.com/rsasaki0109/rust-office)** (egui): one shared core for Writer/Calc/Impress and its DOCX/XLSX/PPTX profiles
- **[Off-Ice](https://github.com/yugaaank/off-ice)**: GPUI office structure (and its use of Zed's `ui` crate)
- **[Zed](https://github.com/zed-industries/zed)**: how a large GPUI app handles text, buffers, IME and performance
- **[GPUI Kit](https://github.com/longbridge/gpui-kit)**: data table, editor and app recipes
- **[Typst](https://github.com/typst/typst)**: modern layout engine design in Rust, krilla PDF output
- **LibreOffice** (`sw` Writer, `sc` Calc, `sd` Impress) and **OnlyOffice** (native OOXML model): reference behaviour for format edge cases

## 11. Progress log (first sessions, 2026-11-07)

1. ✅ Public GitHub repo (AGPL-3.0-or-later), this plan pushed.
2. ✅ Workspace: `kantor`, `kantor_theme`, `sheets_engine`, `sheets_ui` on `gpui-kit` 0.9 (pins GPUI) and IronCalc 0.9.1.
3. ✅ CI on x86_64 and `ubuntu-24.04-arm`; nightly release with downloadable aarch64 tarball; `scripts/install.sh` for the Mac.
4. ✅ `kantor_theme`: reads Omarchy's `colors.toml`, live-reloads when `theme.generation` changes.
5. ✅ First Sheets pixel: open `.xlsx` with IronCalc, show it in GPUI Kit's data table, edit via formula bar, atomic save.
6. ◐ Run the nightly on the Mac: ✅ Vulkan on Asahi, ✅ 16K pages, ✅ GPU ready in ~210 ms, ✅ 39 MB binary; ⚠ wgpu OOM under memory pressure; ☐ fractional scale 1.6, ☐ fcitx5, ☐ RSS.
7. ☐ Open issues for every remaining M0 checkbox (see §0 *Next up*).
8. ✅ Text layout spike → §0 item 8.
9. ◐ Corpus → §0 item 9. `.xlsx` done; IronCalc 0.9.1 drops comments and custom properties on save, writes a non-OOXML `#ERROR!` for 3-D refs, loses dashed/hair borders (full list: `tests/corpus/xlsx/known-gaps.tsv`).
18. ✅ xlsx part preservation on save (`sheets_engine::preserve`): comments, custom properties, drawings, print setup and other unmodelled parts survive open → edit → save; tab colours written; no more non-OOXML `#ERROR!` (formula saved without a cached value, `fullCalcOnLoad`). Corpus test also checks package validity and dropped sheet elements.
10. ✅ Shell and styling modelled on Voltrus Tools (`docs/reference-voltrus-tools.md`): `kantor_ui` metrics + Omarchy-derived tokens, tab strip as titlebar, status bar, Ctrl+K command palette with fuzzy search, cached chrome views. Then: sidebar + inspector (resizable), split panes with draggable dividers, saved workspace (tabs, layouts, panel sizes, recent files), responsive tab strip and formula bar, theme picker (Omarchy + 907 Ghostty themes, live preview) and UI font picker (SF Pro when installed).
11. ✅ Renderer memory patch: vendored `gpui-pre-wgpu` allocates path textures only when used, no path MSAA by default.
12. ✅ Fixes after first use on the Mac: typing directly into cells; palette rows fill the panel width.
13. ✅ Unsaved-changes prompt on close and quit (`crates/kantor/src/workspace/closing.rs`).
14. ✅ Undo/redo for cell edits (IronCalc `UserModel`); dirty state tracked as a position in the history.
15. ✅ Range selection and TSV copy/paste (`sheets_engine::tsv`, `CellRange`).
16. ✅ In-cell editor (`sheets_ui::cell_editor`), CSV/ODS/XLS import, formatting + toolbar (`sheets_engine::format`, `sheets_ui::toolbar`).
17. ✅ Text engine decided: parley for Docs layout, GPUI paints the glyphs (ADR 0001); prototype `crates/docs_layout` with pagination, caret hit-testing, benchmark and krilla PDF examples.
18. ✅ Docs first slice: `docs_engine` (model, undo stack, `.docx` via a quick-xml reader of the modelled subset + docx-rs writer, unsupported content → Save As only), `docs_ui` (`DocsView`: pages on a desk, zoom, caret/selection, keyboard, `EntityInputHandler`, per-paragraph layout cache, verified font bridge incl. the Arabic fallback case, with a headless test), and the shell's `Doc` generalised to Sheets or Docs (tabs, splits, session, recent, close prompt, palette, Home, inspector, status bar).
19. ✅ Slides first slice: `slides_engine` (model, roxmltree reader with byte ranges + string writer that splices only changed `a:xfrm`/paragraphs, raw-copy of every other part, read-only layout/master/theme inheritance for display, new-presentation template, undo/redo, package-validity checks; verified with python-pptx), `slides_ui` (`SlidesView`: fit-to-pane canvas, virtualised thumbnail strip, move/resize/nudge, text editing with IME, insert shapes/pictures, presenter mode; reuses Docs' parley engine and font bridge, now `pub` in `docs_ui`), and the shell's `Doc` extended to a third kind (AppKind::Slides for `.pptx`, Home, sidebar, palette, Ctrl+Alt+N, status "Slide x of n", inspector, session, close prompt).

## 12. Open questions

- **Name**: keep "Kantor"?
- ~~**Licence**~~: decided **AGPL-3.0-or-later** (2026-11-07) so forks and hosted versions stay open. Dependencies are MIT/Apache-2.0/MPL-2.0, all AGPLv3-compatible; check new ones with `cargo deny`.
- **Which app matters most to you day to day?** If it's Docs, it may be worth starting Docs in parallel with Sheets, accepting a slower first release.
- **Collaboration**: confirm it stays out of 1.0.
