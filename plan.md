# Kantor — a native Rust office suite on GPUI

> **Kantor** is a working name (Indonesian for "office"). Rename freely.

A small, fast, native office suite for Linux and macOS (Linux on x86_64 and aarch64, made with Omarchy in mind; macOS on Apple Silicon), built in Rust on [GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui), the UI framework behind the Zed editor. Three apps: **Docs** (Word), **Sheets** (Excel), **Slides** (PowerPoint). They share one binary and one core.

Status: **M0 done; Sheets MVP (M1) feature-complete pending Mac checks; Docs (M2) and Slides (M3) first slices in**. Written 2026-11-07, last updated 2026-11-10.

> **Platform change (2026-11-08):** the M1 Air no longer runs Asahi Linux; it runs macOS, and Kantor ships a macOS build (`Kantor.app`, Metal renderer). Linux stays a first-class platform on x86_64 and aarch64. Notes below about Asahi, 16K pages and the M1 under Linux are history from the first week.

---

## 0. Where we are / pick up here

**Repo:** https://github.com/dzulfikar08/kantor (public, AGPL-3.0-or-later). `main` is always green; every push publishes a **nightly** build for Linux (x86_64, aarch64) and macOS (Apple Silicon).

**Windows (2026-11-12, branch `windows-port`, not merged yet):** builds on windows-latest
(x64) and windows-11-arm with GPUI's DirectX backend. Platform code: `ipc.rs` (loopback TCP plus
a token file in place of the Unix socket), `kantor_core::dirs` (%APPDATA%, %LOCALAPPDATA%),
Credential Manager for AI keys, `kantor_mem` HeapCompact, `crates/kantor/build.rs` (exe icon),
`crates/kantor_console` (`kantor.com`, the terminal front for the GUI-subsystem `kantor.exe`).
Packaging: `packaging/windows/kantor.iss` (per-user Inno Setup, Open with entries),
`scripts/package-windows.sh`, `scripts/windows-smoke.sh`. Updates: `InstallKind::WindowsInstaller`
(silent installer after quit) and `WindowsPortable` (file swap). Signing is off until a
certificate is chosen (docs/windows-signing.md). The site's Windows card is behind
`WINDOWS_RELEASED` in site/build.mjs. ☐ Needs a real Windows desktop: rendering and DPI scaling
on mixed-DPI monitors, IME (Indonesian/CJK input), file dialogs, the Open with entries in
Explorer, SmartScreen flow, an update from one real release to the next.

**What works today** (nightly; first verified on the M1 Air under Asahi, since 2026-11-08 on the same Air under macOS):
- One binary, suite shell modelled on Voltrus Tools (`docs/reference-voltrus-tools.md`): Home screen, document **tabs** (tab strip = titlebar), **split panes** with draggable dividers, resizable **sidebar** (open/recent/create) and **inspector** (cell + workbook details), **status bar**, **Ctrl+K command palette** (fuzzy), **theme picker** (Follow Omarchy / 23 Omarchy / 907 Ghostty themes, live preview), **UI font picker** (SF Pro when installed). Tabs, layouts and panel sizes **persist** across runs.
- **Sheets**: open/edit/save `.xlsx` via IronCalc (formulas recalc, atomic save), Excel-style typing into cells (type / Enter / F2 / Delete / Esc), name box + formula bar, multiple sheets. Formula help: point mode (click/drag/arrows insert references, other sheets as `'Sheet2'!A1`), coloured references in the editor and on the grid, F4 `$` cycling, function-name autocomplete (`crates/sheets_engine/src/formula.rs`, `crates/sheets_ui/src/point.rs`). ☐ Try in a window: reference text colouring is painted over the input's glyphs (single-line `InputState` has no styled-runs hook), check it lines up; arrow pointing in the cell editor doesn't scroll (scrolling the edited cell away would commit).
- **Docs** (M2, not yet tried on the Mac): new document (Ctrl+Shift+N, Home, sidebar, palette) and open/save `.docx`: paragraphs, runs (font, size, bold/italic/underline, colour, character styles), paragraph styles (Normal, Heading 1–3, the file's own), alignment, spacing and indents, **lists** (bulleted/numbered, 9 levels, `numbering.xml`; Enter/Tab/Shift+Tab/Backspace behave like Word; Ctrl+Shift+7/8), **tables** (grid, merged cells, borders, rows paginate; Tab between cells, insert table grid, insert/delete rows/columns), **inline pictures** (PNG/JPEG, insert from file), page breaks, hyperlinks. **Page setup** (toolbar "Page" control + palette: A4/Letter/Legal/A5/F4 215×330 mm, portrait/landscape, Normal/Narrow/Moderate/Wide or custom margins in cm; one undo step; saved by rewriting only the changed `w:pgSz`/`w:pgMar` attributes inside the original `w:sectPr`, `crates/docs_engine/src/page.rs`; new documents are A4 portrait with 2.54 cm margins, since Word's own default follows the Windows region: Letter on US-English installs, A4 on Indonesian ones). **Ruler** in cm above the page (margins shaded, caret paragraph's first-line/left/right indent markers; drag margins or indents, one undo step each; "Toggle Ruler" in the palette, saved as `docs_ruler` in settings). Paged view (parley layout, GPUI paints the glyphs, only visible pages), zoom, caret/selection, typing via GPUI's input handler (IME), Ctrl+B/I/U, undo/redo for every edit, plain-text clipboard. **Round-trip safety:** saving copies every package part byte for byte and regenerates only the body (plus `numbering.xml`/media/rels when Kantor added lists or pictures, originals untouched). Unmodelled elements are carried byte for byte as **opaque objects** with a placeholder (floating images, shapes, content controls, simple fields, equations, footnote refs, bookmarks, nested tables) or as verbatim property extras; only tracked changes, comments and content-dependent fields (TOC, REF…) still mark the file unsupported, which sends Save to Save As (rules in `crates/docs_engine/src/docx.rs`).
- **Slides** (M3 first slice, not yet tried in a window): new presentation (Ctrl+Alt+N, Home, sidebar, palette; 16:9 with "Title Slide" and "Title and Content" layouts) and open/save `.pptx`. Display inheritance (read-only): placeholder positions, list styles and body settings from layout/master, theme colours (incl. lumMod/lumOff) and fonts, backgrounds, master/layout decorations, pictures. Editing: thumbnail strip (virtualised, drag to reorder, Ctrl+Up/Down), new/duplicate/delete slide (Ctrl+M / Ctrl+D), select/move (drag, arrows)/resize (handles), text editing in text boxes and placeholders (caret, selection, Enter/Shift+Enter, Ctrl+B/I/U, IME via `EntityInputHandler`), insert text box/rectangle/ellipse/picture, Delete, undo/redo (typing/nudges coalesce), Save/Save As, F5 presenter mode in the pane. Text is laid out by `docs_layout` and painted through Docs' font bridge. **Round-trip safety** (`crates/slides_engine/src/presentation.rs`): every part not rewritten is raw-copied; unchanged slides are copied, unchanged shapes written verbatim, moved shapes get only `a:xfrm` replaced, edited text only its paragraphs (each run's/paragraph's unmodelled attributes and children kept); tables, charts, groups, SmartArt, transitions and timing stay untouched. Text with fields/equations is shown but locked; slides with unusual XML prefixes are kept verbatim and locked. Deleting a slide also drops its notes. Unsupported content (slide sections / custom shows after slides are added, removed or reordered) forces Save As.
- **Find & Replace** in all three apps (2026-11-08): Ctrl+F / Ctrl+H open a shared bar (`kantor_ui::find_bar`) over the document area; matching (case, whole word, Unicode-safe) is pure code in `kantor_core::find`. Docs highlights all matches and selects the current one, Sheets searches the active sheet's displayed values or formulas and selects the cell, Slides searches every slide's shapes. Replace All is one undo step in each (`Document::replace_ranges`, `Workbook::set_inputs`, `Presentation::replace_matches`). Docs status bar shows word and character counts, for the selection when there is one. ☐ Try in a window / on the Mac: keys inside the bar (Enter, Shift+Enter, F3, Esc, Tab), highlight colours on the page, Sheets bar placement over the grid. Gaps: no regex, Sheets searches only the active sheet, Slides doesn't highlight the other matches.
- **Big workbooks** (2026-11-08, a real 28 MB `.xlsx`: 221 MB unzipped, 92k-row sheet, 767k formula cells): files open on the background executor (the pane shows name, size, elapsed time and Cancel; first paint 13 ms after the open starts, parse ~8 s); `.xlsx` shows Excel's cached values instead of recalculating on open (that took >10 min); workbooks with > 50k formulas switch to **Manual calculation** (F9 / Shift+F9 / palette "Calculate Now", "Automatic/Manual Calculation"; status bar says so; a formula typed meanwhile shows "…"; a stale save sets `fullCalcOnLoad`). F9 and fills on big workbooks move the whole `Workbook` (it is `Send`) to the background with a "Calculating…" pane; IronCalc auto-fill recalculates in any mode, hence fills too. Used range and status-bar sums are cached per revision (`dimension()` walks every cell). Peak RSS opening that file: ~315 MB engine-only, ~445 MB in the app (before the memory work below). Timing harness: `KANTOR_BIG_XLSX=… cargo test --release -p sheets_engine big_open -- --ignored --nocapture`. **Memory** (2026-11-10, generated workbooks from `scripts/corpus/gen_big_xlsx.py`; the app idles at ~138 MB on Linux): steady state after open is real data, not load leftovers (live heap 211 MB of 257 MB RSS for 180k rows; the load peak is only ~2% above it), and 87% of it was IronCalc's rows at 56 bytes a cell. Vendored IronCalc now packs plain rows at 32 bytes a cell and records reads only for dynamic arrays (`vendor/ironcalc_base/KANTOR_PATCHES.md` §4–5); `.xlsx` bytes are kept for save only up to 4 MB (was 16); `kantor_mem::release_free_memory` (glibc `malloc_trim`, macOS `malloc_zone_pressure_relief`; the workspace's only unsafe code) runs after an open, after a background recalculation and 0.5 s after a workbook is dropped. 180k rows / 32 MB: engine 257 → 182 MB after open, app 410 → 323 MB, after F9 467 → 377 MB, after closing the tab 428 → 164 MB; recalculation time unchanged (~1.9 s), every value identical. Measure: `cargo run --release -p sheets_engine --example open_memory -- FILE --recalc [--dhat]`. Guard: `KANTOR_MEM_XLSX=big90k.xlsx cargo test --release -p sheets_engine --test memory -- --ignored --nocapture` (90k rows: 92 MB, limit 115 MB). ☐ Check in Activity Monitor on the Mac. ☐ Try on the Mac. Gaps: a recalculation can't be cancelled; IronCalc can only recalculate the whole workbook; Find over 2M cells takes ~1 s per search.
- **AI agents over MCP** (2026-11-08): `kantor mcp` (`crates/kantor/src/mcp/`) is an MCP stdio server (hand-written JSON-RPC, dual-era: 2026-07-28 stateless requests with `server/discover`, and the `initialize` handshake for 2025-11-25 and older) that forwards tool calls to the running app over a 0600 Unix socket (`crates/kantor/src/ipc.rs`; `$XDG_RUNTIME_DIR/kantor/kantor-<pid>.sock`, macOS `~/Library/Application Support/Kantor/run/`; `KANTOR_SOCKET_DIR` overrides). Socket I/O is on threads; requests reach the GPUI main thread through a channel (`main.rs::start_agent_endpoint`) and run in `workspace/agent.rs`, through additive `agent_*` hooks in each view (`{sheets,docs,slides}_ui/src/agent.rs`) so every write is one undo step with redraw, selection highlight and an "Agent: …" status notice. Tool logic against the engines is in `crates/kantor/src/agent/` (unit-tested); engine helpers: `Workbook::batch`, `formula::parse_range`, `docs_engine::agent` (insert/replace/delete paragraphs), `Presentation::transaction` / `add_slide_with_text` / `set_shape_text`. Launches Kantor if none runs. ☐ Try in a real window with Claude Code (not done: no virtual display on the dev box), incl. on the Mac. Gaps: undo steps aren't labelled "Agent edit" (no engine has step labels), the highlight is a selection rather than a timed flash, no tables/pictures/charts tools, number formats are the six presets.
- **AI, opt-in and local-first** (2026-11-08, `crates/kantor_ai`, panels in `crates/kantor/src/ai/`): off until the user turns on the master switch in **AI Settings** (palette) and picks a provider; there is no Kantor-hosted service. Providers: Anthropic (Messages API, default `claude-haiku-5-5`), OpenAI / OpenRouter / Groq / DeepSeek / any OpenAI-compatible base URL, Google Gemini (`gemini-2.5-flash`), Ollama (local, `http://localhost:11434`); any model id can be typed. All four wire formats stream (SSE / NDJSON) on a worker thread over `ureq` + rustls (blocking, no async runtime; GPUI has no native HTTP client); per-read timeout 120 s, 300 s overall, Cancel between chunks; errors say what to do (401 → check the key, 404 → model/URL, refused → is the server running). API keys go to the OS keychain (`keyring-core`: Secret Service over the zbus GPUI already links, macOS Keychain), with a 0600 `~/.config/kantor/ai-keys.toml` fallback and a warning when no keychain exists; never in `settings.toml` (`[ai]` holds only switch/provider/model/base URL), never logged (`Debug` impls redact). A hosted "Paparan AI" credits provider later = one more `ProviderId` + `Api` arm in `kantor_ai::client`. **Sheets formula assistant** (sparkle button in the toolbar when AI is on, palette *Ask AI for a Formula…*): sends only the header row, ≤ 20 sample rows near the selection (≤ 26 columns, cells cut at 40 chars) and the active cell address (`kantor_ai::sample`); the reply is strict JSON `{formula, explanation}` in the user's language; the formula is parsed with IronCalc and evaluated in a scratch copy (`sheets_engine::check`; `#NAME?`/`#ERROR!`/circular are rejected, `#DIV/0!` etc. shown as a warning) before Insert, which is one undo step. *Explain This Formula* sends the formula and the headers only, answering in the system locale's language (id_* → Indonesian). **Docs rewrite** (sparkle button / palette *Rewrite / Translate Selection…*): Fix grammar, More formal, Shorter, Indonesian ↔ English on the selected paragraphs' text only (≤ 12,000 characters); preview, then Replace as one undo step (`Document::replace_paragraph_texts`) that keeps paragraph styles, lists and bookmarks but gives each paragraph its first run's formatting (bold words and links inside are lost); paragraphs with pictures/objects/page breaks are refused; inside tables the paragraph count must not change. ☐ Try in a window with a real key and with Ollama: streaming status, keychain prompt on a locked GNOME keyring, the Secret Service fallback on bare Hyprland, macOS Keychain on the Mac.
- **In-app updates** (2026-11-10, `crates/kantor_update`, UI in `crates/kantor/src/workspace/update.rs`; README "Updates"): per-channel manifest `update-{stable,nightly}.json` + detached Ed25519 `.sig` (base64 of 64 bytes over the exact bytes), written and signed in CI by `scripts/update-manifest.sh` (openssl 3, key from the `release` environment secret `UPDATE_SIGNING_KEY` = base64 32-byte seed; it refuses a key whose public half isn't the embedded one) in `release.yml` (stable, release assets) and `build.yml` (nightly, Linux only, uploaded after the assets). The app fetches `kantor.paparanai.id/update/<channel>.json` (302s in `site/static/_redirects` to the release assets) over the `ureq` + rustls stack; Ed25519 and SHA-256 come from `ring`, already linked by rustls (no new crypto crate). Versions are semver plus a nightly build stamp (`KANTOR_BUILD`, UTC commit time, set with `KANTOR_CHANNEL=nightly` by build.yml) so nightlies of one version order and never offer themselves. Install kinds: macOS bundle (codesign `-R` with Developer ID anchor + team `F6Q6UFUK69` + bundle id, `codesign -dv` team check, `spctl --assess`), AppImage (`$APPIMAGE` with the exe inside `$APPDIR`), install.sh tarball (`<prefix>/bin/kantor` whose launcher says `Exec=<prefix>/bin/kantor`), else notice only. Staging (download beside the install, size + SHA-256, system `tar`, `--version` probe) runs in the background; the swap is a rename transaction with rollback, on *Restart to Update* (`Closing::Restart`, same unsaved-changes check as quit) or at quit (`on_app_quit`). Check ≤ 1/24 h (`~/.local/state/kantor/update.json`), `[update] enabled/channel` in settings.toml, `KANTOR_UPDATE_URL` for tests, `KANTOR_UPDATE_PUBKEY` (compile time) for test keys. ☐ First real run: needs `UPDATE_SIGNING_KEY` in the `release` environment and a release after this one (v0.2.0 has no manifest, and no updater). ☐ Try the macOS path on the Air.
- Design system in `kantor_ui` (metrics + colour tokens derived from the palette; "views never use raw colours or sizes").
- Renderer memory patch (`vendor/gpui-pre-wgpu`) — fixed the wgpu *Out of Memory* crash on 8 GB with a browser open.

**Machines and rules** (details in §8):
- **M1 Air** (macOS, shared machine): test target only. Never build or install toolchains there; install the nightly (it puts `Kantor.app` in `~/Applications` and links `~/.local/bin/kantor`) with `curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh`.
- **Linux box** (x86_64, 30 GB RAM): fine for local `cargo build`/`cargo test`/`cargo run`. Setup: `rustup` stable, then on Arch `sudo pacman -S --needed base-devel clang cmake wayland libxkbcommon vulkan-icd-loader fontconfig freetype2 alsa-lib openssl zstd dbus` (Ubuntu: `./scripts/ci-deps.sh`). `cargo run -p kantor` / `cargo test --workspace`.
- **GitHub Actions** does the real builds and releases (Linux x86_64 and aarch64, macOS on `macos-15`). `housekeeping.yml` auto-applies `cargo fmt` and refreshes `Cargo.lock` on `main` (it commits as github-actions — `git pull --rebase` before pushing). PRs are checked strictly (fmt + clippy `-D warnings` + tests on both arches).

**Next up, in order** (each item ends with a green CI and a nightly to try on the Mac):
1. ◐ **Confirm on the Mac:** ✅ 2026-11-07, nightly `a395ecf` on the M1 Air (Arch Linux ARM, 16K pages): Sheets, Docs and Slides open, edit and render correctly; **~120 MB RAM with a sheet, a document and two presentations open** (target < 150 MB). ☐ Still to check: fractional scale 1.6 looks sharp, fcitx5 typing in the formula bar, RSS with a 10k-cell sheet (< 150 MB target), long session with Helium open (memory patch).
2. ✅ **Unsaved-changes prompt** (Save / Don't Save / Cancel) when closing a pane or tab, quitting (Ctrl+Q), or closing the window from Hyprland. Save stores each dirty document in turn, untitled ones via Save As, and closes only if all were saved. ☐ Try it on the Mac (Hyprland's close-window path is untested).
3. ✅ **Sheets undo/redo**: `sheets_engine` runs on IronCalc's `UserModel`; Ctrl+Z / Ctrl+Shift+Z / Ctrl+Y in the grid (the formula bar keeps its own text undo), plus palette commands. "Unsaved" follows the undo history, so undoing back to the saved state is clean again.
4. ✅ **Copy/paste** of ranges as TSV (Ctrl+C/X/V in the grid; one undo step per paste/clear), range selection with Shift+arrows, Shift+click and drag. ✅ **Grid manipulation** (`sheets_ui/src/grid_ops.rs`, engine logic in `sheets_engine/src/grid.rs`): header click/drag/Shift selection, corner and Ctrl+A, Ctrl(+Shift)+arrows to the data edge, fill handle drag and Ctrl+D/R via IronCalc auto-fill (plus a Kantor pass so formatted numbers such as dates continue as series), insert/delete rows and columns from a header right-click menu and the palette, autofit (double-click a column border), freeze panes (palette, header menus, toolbar View menu; round-trips in xlsx; not undoable, as in Excel; frozen columns pinned via DataTable fixed columns, frozen rows pinned in a block above the scrolling row list, `table/row_list.rs` `skip`; drawings in up to four panes), status-bar Average/Count/Sum, Backspace clears a range. One undo step each (`Revisions` now records multi-step entries). ☐ Try on the Mac: header drags, fill handle feel, header context menu. ☐ Try on the Mac: mouse selection, clipboard to/from browser/LibreOffice. Pasted formulas are not reference-shifted yet (`=A1` stays `=A1`).
5. ✅ **In-cell editor** over the active cell (gpui-component `Input`, so IME goes through GPUI), synced live with the formula bar; Enter/Tab/Shift variants commit and move, typed edits commit on arrows, F2 edits switch arrows to caret mode. ☐ Try on the Mac: alignment while scrolling/resizing, fcitx5 composition and popup position. Gaps: no Alt+Enter line breaks; editor hides if the cell is wheel-scrolled away.
6. ✅ **Formatting basics**: bold/italic/underline/strike (Ctrl+B/I/U), alignment, number formats (General/Number/Percent/Rupiah/Dollar/Date), column widths saved to xlsx (resize is undoable); toolbar above the formula bar that collapses into a "…" menu when narrow. ☐ Try on the Mac. Row heights ✅ (2026-11-11, `sheets_ui::row_layout`: file heights, hidden rows, lazy autofit of visible rows, drag/double-click/Row Height…, saved; the grid's table is a fork of GPUI Kit's `DataTable` with a variable-height row list, `sheets_ui/src/table/`). ☐ Try row resizing on the Mac. Hide/Unhide rows and columns ✅ (2026-11-12, `sheets_engine/src/hide.rs`: one undo step each, Ctrl+9/0 and Shift variants, header double-line marks, Unhide skips filtered-out rows as Excel does). Display of any format code (2026-11-10) goes through `crates/kantor_numfmt` (GridCraft's numfmt, adapted; `PROVENANCE.md`): Excel rounding, fractions, elapsed time, conditions, `[Red]`, `@`, `*` fill, `_` spacing, `[$…-421]` locale tags (Indonesian names, separators from the tag, a deliberate difference from en-US Excel). Gaps: only Indonesian month/day names; `:` and AM/PM not localised; no `####` for numbers wider than the cell.
7. ✅ **CSV/TSV open and save** (delimiter detection, BOM/UTF-16/Windows-1252; one sheet, displayed values; a leading `=` stays text) and **read-only `.ods`/`.xls`/`.xlsb` import** via calamine (values, dates, formulas; formulas that don't reproduce the file's cached result are kept as values; Save on an import goes to Save As `name.xlsx`). See `crates/sheets_engine/src/import.rs`. ☐ Open the same workbook in two panes (needs a shared document model; today a split opens a new blank sheet).
8. ✅ **Text layout spike** → [`docs/decisions/0001-text-engine.md`](docs/decisions/0001-text-engine.md): **parley** lays out (GPUI-free `crates/docs_layout`: pages, widow/orphan, hit-testing, krilla PDF), GPUI only paints its glyphs via `Window::paint_glyph` through a verified fontique → `FontId` bridge. ~110 ms per 20 A4 pages full layout, ~50 µs per paragraph edit. Biggest risk: GPUI's rasterizer ignores variable-font axes and synthetic bold/italic.
9. ◐ **Corpus + fidelity CI**: ✅ `.xlsx`: 17 generated CC0 files in `tests/corpus/xlsx/` (`scripts/corpus/gen_xlsx.py`; `13`–`17` feature-rich ones from `scripts/corpus/gen_features_xlsx.py` with XlsxWriter, each feature an XML probe in `features.tsv` checked after open → save and a second save), round-trip test `crates/sheets_engine/tests/corpus.rs` with known IronCalc gaps in `known-gaps.tsv`, `fidelity.yml` (report + LibreOffice-headless PDFs as an artifact). ☐ `.docx`/`.pptx` corpora with Docs/Slides; ☐ pixel-diff score.
   ✅ **xlsx part preservation** (§5): `Workbook::save_as` post-processes IronCalc's package (`crates/sheets_engine/src/preserve.rs`) and carries what IronCalc drops from the original: comments + VML, custom properties, drawings/charts/images, printer settings, custom XML, threaded comments, and sheet elements (`sheetPr`, `sheetFormatPr`, data validation, print setup, `legacyDrawing`...), matched to sheets by `sheetId`; tab colours are written from the model; non-OOXML error values (`#ERROR!` from 3-D refs) are no longer written. Anything ambiguous is skipped and listed in `Workbook::save_notes()`; the result is checked with `package_problems` and falls back to IronCalc's output. ✅ 2026-11-10: tables (header edits rename the column), chart sheets (back in their place in the sheet list), sparklines and other `extLst` extensions, hidden/built-in defined-name attributes (`_xlnm._FilterDatabase`); every border style, indent, text rotation, shrink-to-fit, cell protection and row/column outlines via the vendored `ironcalc` reader/writer (`vendor/ironcalc/KANTOR_PATCHES.md`); carried parts follow inserted/deleted rows and columns (`crates/sheets_engine/src/shift.rs`; what loses all its cells is dropped with a note; a table split by a column insert/delete is dropped), and so do defined names in the model (`vendor/ironcalc_base` §8). Big files: the merge reads sheets without their cells and the writer copies them once, saving the 90k-row workbook peaks at 310 MB (was 1849 MB). Not carried yet: pivots, external links, VBA, OLE/controls, slicers; still lost: fill patterns, underline styles, super/subscript (not in IronCalc's model). ✅ 2026-11-11: Excel tables shown with their styles, structured references kept and evaluated against the table's current range (`vendor/ironcalc_base` §12), tables grow with inserted/typed rows and columns (a column inserted inside is kept, not dropped), sort (`crates/sheets_engine/src/sort.rs`) and AutoFilter (`filter.rs`; state = the `<autoFilter>` in the model, filtered rows = hidden rows); grid side in `crates/sheets_ui/src/filter_menu.rs`. Not yet: creating tables (Format as Table), editing total-row functions, sortState. ✅ 2026-11-11: pivot tables made, rearranged and deleted in Kantor (`sheets_engine/src/pivot_edit.rs`: Insert PivotTable, a field layout API that builds a definition and refreshes it with `pivot.rs`'s engine; `preserve::add_pivots` writes new pivots' parts after the merge; `sheets_ui/src/pivot.rs`: the Insert card and the PivotTable Fields panel with GPUI drag and drop). ☐ Open Kantor-made pivots in Excel (not available on the dev box; LibreOffice neither).
10. ◐ First tagged release `v0.1.0` (Sheets MVP) and an AUR `kantor-bin`: ✅ prepared — `release.yml` (tag `v*` must match the Cargo.toml version; tarballs + `SHA256SUMS`, notes from `CHANGELOG.md`), version 0.1.0, `packaging/aur/` (`update.sh`, publish steps in its README). ✅ Released 2026-11-07: https://github.com/dzulfikar08/kantor/releases/tag/v0.1.0 (both arches + `SHA256SUMS`); PKGBUILD checksums filled. ☐ Publish `kantor-bin` to the AUR (needs the user's AUR SSH key; steps in `packaging/aur/README.md`).
11. ◐ **Docs M2, first slice** (`docs_engine`, `docs_ui`, shell wiring): see *What works today*. ✅ Lists, tables, inline pictures and opaque passthrough (2026-11-07). ☐ Try on the Mac: glyphs land where parley put them in a real window (ADR 0001 open risk 6), fcitx5 composition and candidate window, scrolling/zoom feel, a few real `.docx` files; and the new parts: list markers and hanging indents, table borders/cell padding vs Word, pictures (GPUI image painting), the insert-table grid popover, Ctrl+Shift+7/8 on a real keyboard layout. Next: palette entries for the list/table/picture commands (exposed as `docs_ui` actions and `DocsView` methods; the shell's palette wasn't touched while Slides lands), editing cell merges, header-row repeat, floating images, tab stops, rich clipboard, `.docx` corpus + fidelity CI.
12. ◐ **Slides M3, first slice** (`slides_engine`, `slides_ui`, shell wiring): see *What works today*. ☐ Try in a window / on the Mac: canvas and thumbnails render where expected, drag/resize feel, text caret placement in placeholders, fcitx5, presenter mode, a few real `.pptx` decks (open → edit → save → open in PowerPoint/LibreOffice). ✅ 2026-11-08: bullets/numbering, indents and paragraph spacing (inherited from master/layout/placeholder list styles; Tab/Shift+Tab levels, list toggles), rotation/flip display with a rotate handle (Shift snaps to 15°), speaker-notes pane + notes in presenter mode (S), shape fill/outline popover (theme colours, no fill, widths), copy/paste/duplicate of shapes within and between presentations; verified with python-pptx. ☐ Try in a window: rotate-handle and rotated-resize feel, notes pane resize, list toggles on real decks. ✅ 2026-11-10: renderer-neutral scenes (`slides_scene`) painted by GPUI and written to PDF (`slides_pdf`; "Export as PDF…" in the palette, `kantor convert deck.pptx deck.pdf`, hidden slides left out): gradients (linear/radial/path, stops with alpha, theme fill styles), picture fills and backgrounds, transparency, dashes, caps, joins, arrowheads, connectors, picture cropping; the PDF turns text and pictures with their shapes. Next: tables and charts (display), the preset-geometry module in `shape_outline`, a `.pptx` corpus + fidelity CI.

**Known gaps / caveats:** Slides gaps: tables/charts/groups/SmartArt shown as boxes (kept; they can't be copied yet; left out of the PDF), presets other than rect/roundRect/ellipse/line/connectors drawn as their box, radial gradients on screen are 32 steps, picture bullets drawn as "•", EMF/WMF/SVG pictures not in the PDF; text in a rotated shape follows the rotation but its glyphs stay upright (GPUI can't turn glyphs); text with fields (slide numbers, dates) can't be edited; the notes pane edits plain text (formatting outside the changed part is kept). Docs gaps: headers/footers, floating images, shapes, content controls, fields and equations are kept but shown only as placeholders (or not at all, for headers); table rows never split across pages and header rows don't repeat; merges can't be edited yet; tabs render as one space (no tab stops), "at least" line spacing is approximated, the clipboard is plain text, no spell check, PDF export or print yet. AI gaps: the panels are centred cards, not anchored next to the cell/selection; streamed text shows as a character count (replies are JSON); no per-request model override; OpenAI-compatible servers get JSON by prompt only (no `response_format`). Split-right uses the dashboard icon (GPUI Kit's default icon set has no columns icon). Window title/decorations come from Hyprland (server-side); no in-app window buttons, by design.

## 1. Why

On an 8 GB M1 MacBook Air (first under Omarchy on Asahi Linux, now macOS), every option today trades something away:

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

### Goals (targets, measured on the M1 Air, now under macOS)

| Goal | Target |
|---|---|
| Install size | **< 60 MB** (one binary plus assets for all three apps; fonts and dictionaries come from the system). Raised from 40 MB on 2026-11-07: ~53 MB with Sheets, Docs and Slides is fine |
| Cold start | **< 300 ms** to an empty document |
| Memory | **< 150 MB** with a typical 20-page document or a 10k-cell sheet open |
| Office files | Open and save `.docx`, `.xlsx`, `.pptx` as **first-class formats**, not "import" |
| Round-trip safety | Opening and saving a file **never drops content we don't understand** (see §5) |
| Look and feel | Native GPUI, keyboard-first, follows the Omarchy theme, Mac-style shortcuts |
| Platform | Linux on x86_64 and aarch64 (Wayland, Omarchy/Hyprland in mind), macOS on Apple Silicon, and (since 2026-11-12) Windows 10/11 on x64 and arm64. Asahi (16K pages) was the first target until 2026-11-08 and should keep working, but isn't tested any more |
| Output | Accurate PDF export and printing |

### Non-goals for 1.0

- Macros, VBA or scripting
- Real-time collaboration (keep the model CRDT-friendly, but don't build sync)
- Full track changes and comments UI (preserve them in files, read-only display at most)
- Mail merge, advanced charts, embedded OLE objects, SmartArt editing
- Legacy binary formats (`.doc`, `.xls`, `.ppt`) beyond, at most, read-only
- ~~Windows builds~~: added 2026-11-12 (branch `windows-port`), see §0 "Windows". macOS builds were added on 2026-11-08

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
│   ├── slides_scene      # ✅ no GPUI: slide → display list (paths, gradients, dashes, arrowheads, pictures,
│   │                     #    glyph runs); geometry behind `geom::shape_outline`
│   ├── slides_pdf        # ✅ no GPUI: slide scenes → PDF with krilla (reuses docs_pdf's fonts/glyphs/pictures)
│   └── slides_ui         # ✅ slide canvas, thumbnail strip, text editing, presenter mode (paints scenes)
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
- [~] Grid: virtual scrolling ✅, multiple sheets ✅, resize columns ✅ (saved, undoable, double-click autofit), insert/delete rows and columns ✅; frozen panes ✅ (saved to xlsx, frozen rows and columns pinned), row heights ✅ (custom, hidden, autofit; drag/double-click/Row Height…, undoable, saved)
- [~] Cell editing ✅ (Excel-style keys), formula bar ✅; in-cell editor ☐, function autocomplete ☐, error display ☐
- [~] Selection ✅ (ranges, whole rows/columns from the headers, select all, Ctrl/Ctrl+Shift+arrows), copy/paste as TSV ✅, fill handle + Ctrl+D/R ✅, undo/redo ✅, status-bar Sum/Average/Count ✅; ☐ paste from Helium tried on the Mac, ☐ pasted formulas reference-shifted
- [ ] Formatting: number formats, bold/italic/colors, borders, alignment, merge cells
- [ ] Sort, filter, find and replace
- [ ] Print / PDF export of a selected range
- [x] Autosave plus crash recovery (2026-11-12: recovery copies + "Recover unsaved work" for all three apps, opt-in autosave; `kantor_core::recovery`, `crates/kantor/src/workspace/recovery.rs`, README "Your work is safe")

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

The M1 Air (macOS since 2026-11-08) has **8 GB RAM** and is the *test machine*, not a build machine. **Nothing is compiled or installed on the Mac** — not even `rustup`. The project is public so GitHub's free runners do the release work:

| Where | Use for |
|---|---|
| **M1 Air** (macOS, Apple Silicon) | Running the prebuilt nightly, manual testing (shared machine: touch only Kantor's install) |
| **Linux box** (x86_64, 30 GB RAM) | Local development: `cargo run -p kantor`, `cargo test --workspace`, `cargo clippy`. Fast iteration on UI; still push to get the aarch64 build |
| **GitHub Actions `ubuntu-24.04-arm`** | aarch64 Linux tests and release builds (native ARM, free for public repos) |
| **GitHub Actions `macos-15`** | macOS builds: nightly `Kantor.app` + DMG, ad-hoc signed (Apple Silicon); tagged releases a universal, Developer ID signed, notarised `Kantor.dmg` (`scripts/build-dmg.sh`, `release.yml`, environment `release`) |
| **GitHub Actions `ubuntu-24.04`** | x86_64 tests and release builds, fmt, clippy, cargo-deny, `Cargo.lock` upkeep, later LibreOffice-headless fidelity diffs |

Workflow:

1. Edit (on the Mac or the Linux box), push a branch (or `main`).
2. `ci.yml` runs clippy and tests on both architectures (fmt is checked on PRs); `housekeeping.yml` applies `cargo fmt` and refreshes `Cargo.lock` on `main` and commits the result — always `git pull --rebase` before pushing.
3. `build.yml` publishes a rolling **nightly** release from `main` (and versioned releases from `v*` tags).
4. On the Mac: `scripts/install.sh` downloads the macOS build, installs `~/Applications/Kantor.app` and links `~/.local/bin/kantor` (sha256-checked). On Linux it installs the matching Linux build into `~/.local/bin`. `gh run watch` shows CI progress.

Linux binaries are built on Ubuntu 24.04 (glibc 2.39), so they run on Arch. GNU ld's default 64K max page size on aarch64 keeps them compatible with 16K-page kernels such as Asahi's. The macOS build targets macOS 11 and later.

## 9. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| **Paged text layout is very hard** (tables, floats, bidi, CJK) | High | Sheets first; dedicated M0 spike; scope tables and floats tightly in M2; study Typst's layout design |
| **GPUI is pre-1.0** and changes often | Medium | Pin through `gpui-kit`; isolate GPUI behind the view layer (model and layout don't import it); upgrade on a schedule |
| **GPUI on Asahi** (Vulkan driver maturity, 16K pages; no longer the main target since 2026-11-08) | Low | 16K pages and Vulkan verified working (2026-11-07). Under memory pressure (≈1 GB free, browser open) wgpu reports *Out of Memory* / device lost and GPUI panics after 3 recoveries; with the browser closed it runs fine. **Mitigated 2026-11-07** by `vendor/gpui-pre-wgpu` (path textures only when used, no path MSAA by default): no crash in repeated launches with the browser open. Still to do: upstream the patch; ask GPUI to recover instead of panicking |
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
9. ◐ Corpus → §0 item 9. `.xlsx` done; IronCalc 0.9.1 drops comments and custom properties on save, writes a non-OOXML `#ERROR!` for 3-D refs (full list: `tests/corpus/xlsx/known-gaps.tsv`; border styles fixed 2026-11-10).
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
20. ✅ Docs lists, tables, pictures and opaque passthrough: `numbering.xml` model + markers (originals kept, new definitions appended), tables in the flat paragraph list (`CellRef`) with structure/props kept verbatim and paginated by row in `docs_layout`, inline pictures as parley inline boxes (insert adds media + rel + content type), opaque objects for unmodelled elements written back byte for byte, hand-written body writer (docx-rs now only packages new documents); unsupported narrowed to tracked changes, comments and content-dependent fields.
21. ✅ First run of all three apps on the M1 Air (2026-11-07): everything renders; ~120 MB RAM with four documents open.
22. ✅ xlsx save fidelity (2026-11-10): feature corpus `13`–`17` (tables, charts + chart sheet, sparklines, x14 CF, validation, notes, links, names, print setup, protection, outlines, images, a form control, all border and alignment options) with per-feature probes; tables, chart sheets, sparklines/`extLst`, hidden names carried; vendored `ironcalc` keeps borders, alignment, protection, outlines; carried refs and defined names follow row/column inserts and deletes; big-file save peak 1849 → 310 MB.

## 12. Open questions

- **Name**: keep "Kantor"?
- ~~**Licence**~~: decided **AGPL-3.0-or-later** (2026-11-07) so forks and hosted versions stay open. Dependencies are MIT/Apache-2.0/MPL-2.0, all AGPLv3-compatible; check new ones with `cargo deny`.
- **Which app matters most to you day to day?** If it's Docs, it may be worth starting Docs in parallel with Sheets, accepting a slower first release.
- **Collaboration**: confirm it stays out of 1.0.
