# Kantor — a native Rust office suite on GPUI

> **Kantor** is a working name (Indonesian for "office"). Rename freely.

A small, fast, native office suite for Linux (first target: Omarchy on Apple Silicon), built in Rust on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), the UI framework behind the Zed editor. Three apps: **Docs** (Word), **Sheets** (Excel), **Slides** (PowerPoint). They share one binary and one core.

Status: **M0 in progress**. Written 2026-11-07.

---

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

```
kantor/
├── crates/
│   ├── kantor            # binary: app shell, window/tab management, CLI (`kantor file.xlsx`)
│   ├── kantor_core       # shared model: units, colors, fonts, styles, document trait,
│   │                     #   undo/redo command log, autosave/recovery, settings
│   ├── kantor_theme      # Omarchy theme bridge (reads ~/.local/state/omarchy/current/theme/colors.toml)
│   ├── kantor_text       # rich-text model + paragraph layout (shared by Docs and Slides)
│   ├── kantor_ooxml      # shared OOXML plumbing: zip, [Content_Types], relationships,
│   │                     #   theme/style parts, unknown-part preservation
│   ├── kantor_pdf        # PDF export via krilla
│   ├── sheets_engine     # thin wrapper over ironcalc_base
│   ├── sheets_ui         # grid, formula bar, cell editor, selection, fill handle
│   ├── docs_model        # document tree: sections, paragraphs, runs, tables, images
│   ├── docs_layout       # pagination engine (pages, columns, tables, headers/footers)
│   ├── docs_docx         # DOCX <-> docs_model (docx-rs + preserved extras)
│   ├── docs_ui           # page view, caret, selection, toolbar, styles panel
│   ├── slides_model      # slides, layouts, masters, shapes, text boxes, images
│   ├── slides_pptx       # PPTX <-> slides_model (own, on kantor_ooxml)
│   └── slides_ui         # slide canvas, sorter, presenter mode
├── tests/corpus/         # real-world .docx/.xlsx/.pptx samples (see §7)
└── packaging/            # PKGBUILD, .desktop files, MIME types, icons
```

One binary, `kantor`, holds all three apps. `kantor-docs`, `kantor-sheets` and `kantor-slides` `.desktop` entries launch it in the right mode, so each one shows up in omacast.

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

- [ ] Repo, workspace, AGPL-3.0-or-later licence, CONTRIBUTING, issue templates
- [ ] CI: build and test on **x86_64** and **`ubuntu-24.04-arm`** (GitHub's free ARM runners for public repos), clippy, fmt, `cargo deny`
- [ ] GPUI "hello window" on the M1 Air under Hyprland/Asahi: confirm Vulkan rendering, fractional scale 1.6, fcitx5 input, and the 16K-page kernel
- [ ] `kantor_theme`: read Omarchy theme colors, live-reload on `omarchy theme set`
- [ ] **Text layout spike** (decides the hardest technical question): lay out and paint 50 pages of mixed-font text with parley vs GPUI's text system; measure speed, memory, caret/selection quality and fcitx5 IME support. Pick one and write the decision down.
- [ ] Corpus started: 30+ real `.docx/.xlsx/.pptx` files (see §7)

**Exit:** a GPUI window on the Mac, themed, CI green on both architectures, text-engine decision recorded.

### M1 — Sheets MVP (6–10 weeks)

- [ ] Open, edit and save `.xlsx` through IronCalc; `.csv` import/export; `.ods`/`.xls` read via calamine
- [ ] Grid: virtual scrolling, frozen panes, resize rows/columns, multiple sheets (tabs)
- [ ] Cell editing, formula bar, autocomplete for function names, error display
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

- [ ] PPTX read/write on `kantor_ooxml`: masters, layouts, placeholders, shapes, text boxes, images, notes
- [ ] Slide canvas (move, resize, align guides), slide sorter, themes from the file
- [ ] Text boxes reuse `kantor_text`
- [ ] Presenter mode: fullscreen on one display, notes and timer on the other
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

The M1 Air has **8 GB RAM** and is the *target*, not a build machine. **Nothing is compiled or installed on the Mac** — not even `rustup`. The project is public so GitHub's free runners do all the work:

| Where | Use for |
|---|---|
| **M1 Air** (aarch64, the real target) | Editing code, running prebuilt binaries, manual testing |
| **GitHub Actions `ubuntu-24.04-arm`** | aarch64 tests and release builds (native ARM, free for public repos) |
| **GitHub Actions `ubuntu-24.04`** | x86_64 tests and release builds, fmt, clippy, cargo-deny, `Cargo.lock` upkeep, later LibreOffice-headless fidelity diffs |

Workflow:

1. Edit on the Mac, push a branch (or `main`).
2. `ci.yml` runs fmt, clippy, tests on both architectures; `lockfile.yml` keeps `Cargo.lock` committed so nobody runs cargo locally.
3. `build.yml` publishes a rolling **nightly** release from `main` (and versioned releases from `v*` tags).
4. On the Mac: `scripts/install.sh` downloads the aarch64 build into `~/.local/bin` (sha256-checked). `gh run watch` shows CI progress.

Binaries are built on Ubuntu 24.04 (glibc 2.39), so they run on Arch. GNU ld's default 64K max page size on aarch64 keeps them compatible with the Asahi 16K-page kernel.

## 9. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| **Paged text layout is very hard** (tables, floats, bidi, CJK) | High | Sheets first; dedicated M0 spike; scope tables and floats tightly in M2; study Typst's layout design |
| **GPUI is pre-1.0** and changes often | Medium | Pin through `gpui-kit`; isolate GPUI behind the view layer (model and layout don't import it); upgrade on a schedule |
| **GPUI on Asahi** (Vulkan driver maturity, 16K pages) | Medium | Verify in M0; report driver bugs to linux-aurora / Mesa; CI tests on generic ARM |
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

## 11. First two weeks: concrete steps

1. ✅ Public GitHub repo (AGPL-3.0-or-later), this plan pushed.
2. ✅ Workspace: `kantor`, `kantor_theme`, `sheets_engine`, `sheets_ui` on `gpui-kit` 0.9 (pins GPUI) and IronCalc 0.9.1.
3. ✅ CI on x86_64 and `ubuntu-24.04-arm`; nightly release with downloadable aarch64 tarball; `scripts/install.sh` for the Mac.
4. ✅ `kantor_theme`: reads Omarchy's `colors.toml`, live-reloads when `theme.generation` changes.
5. ✅ First Sheets pixel: open `.xlsx` with IronCalc, show it in GPUI Kit's data table, edit via formula bar, atomic save.
6. ☐ Run the nightly on the Mac under Hyprland/Asahi: Vulkan, fractional scale 1.6, fcitx5 input. Record RSS and startup time.
7. ☐ Open issues for every remaining M0 checkbox.
8. ☐ Text layout spike (parley vs GPUI text), built and benchmarked in CI; numbers and decision in `docs/decisions/0001-text-engine.md`.
9. ☐ Corpus: 10 files of each format; a CI job producing LibreOffice-headless reference PDFs.

## 12. Open questions

- **Name**: keep "Kantor"?
- ~~**Licence**~~: decided **AGPL-3.0-or-later** (2026-11-07) so forks and hosted versions stay open. Dependencies are MIT/Apache-2.0/MPL-2.0, all AGPLv3-compatible; check new ones with `cargo deny`.
- **Which app matters most to you day to day?** If it's Docs, it may be worth starting Docs in parallel with Sheets, accepting a slower first release.
- **Collaboration**: confirm it stays out of 1.0.
