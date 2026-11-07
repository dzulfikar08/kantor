# 0001 — Text engine for Docs: parley for layout, GPUI for painting

- **Status:** accepted (M0 spike, 2026-11-07)
- **Decides:** plan.md §6 M0 "Text layout spike", §0 item 8. Gates Docs (M2).
- **Prototype:** `crates/docs_layout` (library, tests, `examples/bench.rs`, `examples/pdf.rs`, `examples/gpui_compare.rs`)

## Context

Docs needs a **paged** rich-text engine, like Word: paragraphs of mixed runs (font, size, bold/italic, colour, underline), UAX #14 line breaking, justification, pagination onto fixed-size pages with widow/orphan control, and later tables, floats and hyphenation. It must handle Indonesian, CJK and Arabic (bidi). It has to feed three consumers:

1. **GPUI's renderer** (the editor view). Everything on screen is painted by GPUI.
2. **krilla** for PDF export, which needs font bytes, glyph ids and positions.
3. **Editing**: caret placement, hit-testing, selection rectangles, and GPUI's `InputHandler` for IME (fcitx5).

plan.md §4.3 also requires layout to be a pure function of model + fonts, testable without a window.

## Options

### A. GPUI's own text system

`WindowTextSystem::shape_line` / `shape_text`, `LineWrapper`, on Linux backed by `gpui_wgpu::CosmicTextSystem` (cosmic-text 0.19 shaping, swash rasterising). Read from source (gpui-pre 0.3.11, `vendor/gpui-pre-wgpu/src/cosmic_text_system.rs`):

- `PlatformTextSystem::layout_line(text, font_size, &[FontRun { len, font_id }])` shapes **one line at one font size**. `FontRun`/`TextRun` have no size, so a paragraph that mixes 11 pt and 13 pt has to be cut into separately shaped pieces and stitched back together by hand.
- `shape_text` wraps with `LineWrapper`: it adds up per-character advances for **one font**, using a "word character" heuristic. That isn't UAX #14, isn't shaping-aware, and can't do CJK kinsoku or justification. The paragraph's runs aren't wrapped as a unit.
- There's no bidi *line* reordering API, no paragraph-level alignment or justification, and no cluster or caret model beyond `index_for_x`.
- **No access to font bytes**: `FontId` is opaque, and the trait has no way to get the data back. krilla can't embed what it can't read, so PDF export would need a second font system anyway, and the screen and the PDF could then disagree.
- It needs a platform text system, so layout can't run in plain unit tests without instantiating GPUI's backend.

### B. parley (linebender) for layout, GPUI for painting

parley 0.12 (+ fontique for fonts, harfrust for shaping, ICU4X segmenters). It provides `RangedBuilder` style spans (family, size, weight, style, colour brush, underline, locale, letter/word spacing), bidi, UAX #14 breaking with per-line width control (`BreakLines` with `set_line_max_advance`, which floats will need), `Alignment::Justify`, inline boxes (for inline images later), and an editing model (`Cursor::from_point`, `Cursor::geometry`, `Selection::geometry`, visual/logical word movement). Every glyph run exposes `FontData` (an `Arc`'d blob + collection index), the font size, normalized variation coords, synthesis flags and positioned glyph ids.

### C. Something else (cosmic-text directly, or own shaping on harfrust)

cosmic-text is a text-*editor* buffer (one style table, line-oriented), not a paged-layout toolkit. Writing our own line breaker and bidi on harfrust would redo what parley already does. Not pursued.

## The crux: painting parley's glyphs with GPUI

This works with public GPUI API, and it was verified headless (`examples/gpui_compare.rs`):

1. **Paint call.** `Window::paint_glyph(origin, font_id: FontId, glyph_id: GlyphId, font_size: Pixels, color: Hsla)` is public. `origin` is the glyph's pen position on the baseline (that's how GPUI's own `ShapedLine::paint` calls it). Color glyphs go through `Window::paint_emoji`. `docs_layout::PagedDocument::glyph_runs(page)` already yields per-glyph page coordinates (`x`, `y` = baseline + offset) and the run's font size and colour. The Docs element's `paint` loops `paint_glyph(page_origin + (g.x, g.y) * zoom, font_id, GlyphId(g.id), px(size * zoom), color)` for the visible pages only. Underline and strikethrough come from the run metrics and are painted with `paint_quad`.
2. **Font identity (fontique → GPUI `FontId`).** Glyph ids only mean something for the exact font file, so a `FontBridge` maps `(FontData blob id, index)` to a `FontId` once per face and caches it:
   - Read the face's family name (name ID 16, else 1), weight and style with skrifa from parley's bytes, then `TextSystem::resolve_font(&Font { family, weight, style, .. })`.
   - **Verify** it's the same file: compare cmap glyph ids (`glyph_for_char`) and advances (`advance`) for a sample of code points. On this box all 6 faces used by the sample document matched (Carlito Regular/Bold/Italic, Liberation Serif, Noto Sans CJK SC from a `.ttc`, Noto Sans Arabic; 303 code points checked). Both sides enumerate the same fontconfig directories, so system fonts match in practice.
   - **Gotcha:** GPUI's `load_family` refuses a face as a *primary* font when it has no Latin `m` (Arabic, Devanagari or Hebrew-only faces), so `font_id` fails for "Noto Sans Arabic". Those faces only get a `FontId` through GPUI's fallback path. The bridge handles this by shaping a sample of that face's text with `Font { family: "Liberation Sans", fallbacks: [that family] }` and reading the `ShapedRun::font_id`. This works today, but it's a hack (see risks).
   - If verification fails (a font that only fontique knows, or a font embedded in a document), register the exact bytes with `TextSystem::add_fonts` and resolve again. The same applies to fonts embedded in `.docx`: load them into both fontique and GPUI from the same bytes.
3. **IME / caret.** GPUI's `EntityInputHandler` (`bounds_for_range`, `character_index_for_point`, marked text) doesn't depend on the layout engine. It maps onto `PagedDocument::caret` / `hit_test` (with UTF-16 ↔ UTF-8 conversion). The round trip is tested in `caret_round_trips_through_hit_test`. fcitx5 on Wayland is a GPUI platform question (plan §0 item 1) and the same for either option.
4. **PDF.** krilla takes parley's bytes and glyph ids directly (`examples/pdf.rs`: `Font::new(blob.data(), index)`, `KrillaGlyph` per glyph with its cluster range for ToUnicode). Screen and PDF use the **same** layout.

## Measurements

Linux box (x86_64), release build (`lto = "thin"`), A4 at 96 dpi (793.7 × 1122.5 px), 2.54 cm margins, Carlito 11 pt / 1.15 line spacing. Sample: heading + justified Indonesian paragraph with bold, italic-red and 13 pt serif-underlined runs, a CJK paragraph and an Arabic paragraph, repeated. The machine was shared with other builds, so timings vary by about 2×; the best runs are shown with the range.

| | parley (`examples/bench.rs`) |
|---|---|
| Document | 280 paragraphs, 54.6 k chars → **840 lines, 21 pages**; 0 `.notdef`, 6 faces (incl. CJK and Arabic fallback) |
| Font context (system font scan) | 28–35 ms, once per process |
| Full layout + pagination, 20 pages | **~105–125 ms** (noisy runs up to 210 ms) |
| Full layout, 50 pages (680 paragraphs, 2040 lines) | **~270 ms** |
| Edit one paragraph (reshape + repaginate all) | **~50 µs** (20 p) / ~90 µs (50 p) |
| Repaginate only | 26 µs (20 p) / 65 µs (50 p) |
| Glyph runs for all 21 pages (57.9 k glyphs) | 7 ms (≈0.35 ms per page painted) |
| Hit-test | ~0.9–1.7 µs per point |
| RSS after laying out | 22 MB at 21 pages, 39 MB at 50 pages (≈0.6 MB per page, incl. touched font mmaps) |
| krilla PDF export (`examples/pdf.rs`) | 4 pages in 24 ms, 79 KB, fonts subset and embedded with ToUnicode (`pdffonts`: Carlito ×3, Liberation Serif, Noto Sans CJK SC, Noto Sans Arabic) |
| Binary cost | parley + deps ≈ 1.3 MB stripped (bench example 1.6 MB total); krilla with default features ≈ +2.5 MB |

Same 280 paragraphs, shaping + line breaking only (`examples/gpui_compare.rs`):

| | time | lines |
|---|---|---|
| GPUI / cosmic-text `layout_line` + a hand-written greedy breaker (spaces and CJK only, **span sizes dropped**, no justification) | ~43 ms | 840 |
| parley shape + UAX #14 + alignment | ~108 ms | 840 |

So GPUI's shaper is about 2.5× faster per character, but it does less: no bidi or segmentation analysis, no cluster data for editing, and it can't express the document. Since layout is cached per paragraph, the edit path is about 50 µs either way. The difference only shows on full relayout (file open, page-width change).

The PDF of the sample, rasterised, shows justified Indonesian with the bold, italic and serif runs, CJK broken per character with punctuation rules, and right-aligned Arabic with correct joining (`cargo run --release -p docs_layout --example pdf -- /tmp/spike.pdf`).

## Decision

**parley does all Docs (and later Slides text-box) layout, in GPUI-free crates. GPUI only paints the resulting glyph runs with `Window::paint_glyph`, through a cached, verified fontique → `FontId` bridge, and handles input through `EntityInputHandler`.** krilla consumes the same glyph runs for PDF. GPUI's own text system stays in use for UI chrome (labels, inputs, the formula bar), where it's already used.

Why: option A can't represent mixed font sizes in a line, has no real line breaking, justification or bidi layout, and can't provide font bytes for PDF. Building those on top would mean writing a parley anyway. Option B covers the requirements today, keeps layout a pure, window-free function (plan §4.3), and gives screen/PDF parity by construction.

## Consequences

- `crates/docs_layout` (no GPUI) holds the paged engine. The spike's API (`Engine`, `Paragraph`/`Span`, `PageSetup`, `PagedDocument::{paginate, glyph_runs, hit_test, caret}`) is the seed. It will move behind `docs_model` once that exists. Whether paragraph layout lives in `kantor_text` for Slides is decided when Slides starts.
- `docs_ui` owns the `FontBridge` (fontique face → GPUI `FontId`) and the paint loop. Only the bridge touches both worlds.
- Two font databases (fontique and cosmic-text's fontdb) and two shapers (harfrust 0.12 via parley, harfrust 0.5 via cosmic-text) end up in the binary: about 1.3 MB more, and both scan system fonts at start (~30 ms each). Create the parley `FontContext` lazily, the first time a Docs tab opens, so Sheets cold start doesn't pay for it.
- Layouts are cached per paragraph. Edits reshape one paragraph and repaginate (~50 µs). Full relayout happens only on open or page-geometry change.
- `docs_layout` dev-depends on `gpui-pre`/`gpui-pre-wgpu`/`skrifa` only for `examples/gpui_compare.rs`. The library itself has one dependency, `parley`. All new crates are MIT/Apache-2.0/Unicode-3.0/BSD-3-Clause (AGPL-compatible, inside `deny.toml`'s allow list).

## Open risks

1. **Variable fonts and synthesis don't survive GPUI's rasterizer** (biggest risk). `RenderGlyphParams` has no variation coordinates and no embolden/skew, and `CosmicTextSystem::render_glyph_image` scales the default instance. If parley picks a weight from a variable font (e.g. a variable Noto Sans or Inter), or synthesises bold/italic for a family that lacks them, the **layout and PDF are right but the screen draws the default instance**, with advances that don't match. The sample fonts (Carlito, Liberation, Noto CJK/Arabic) are static, so nothing showed up. Mitigations, in order: prefer static faces in the font mapping table (§5); detect `normalized_coords`/synthesis on a run and warn; long-term, upstream variation coords and synthesis to GPUI's `RenderGlyphParams` (the renderer half is already vendored in `vendor/gpui-pre-wgpu`, but the struct lives in `gpui-pre`).
2. **The no-'m' fallback hack** for script-only faces depends on cosmic-text's fallback path assigning a `FontId`. It's undocumented behaviour and could change on a GPUI bump. Fix upstream with a `TextSystem::font_id_for_face(bytes/path, index)` or a "non-primary" lookup. Cover it with a regression test in `docs_ui`.
3. **Memory**: about 0.6 MB RSS per laid-out page, including font mmaps. Fine for a 20-page report (inside the 150 MB budget), but a 300-page thesis would not be. Mitigation: keep only line breaks and heights for off-screen paragraphs and reshape on demand (~0.1–0.2 ms per paragraph). Measure on the M1 Air.
4. **Full-relayout speed** (~5–6 ms per page here; probably a bit slower on the M1 Air) means about 1 s to open a 200-page document. Mitigation: lay out visible pages first and the rest in the background. Paragraphs are independent, so they parallelise across threads, one `LayoutContext`/`FontContext` per thread.
5. **parley is pre-1.0** (0.12, API reshuffled into `parley_engine`/`parlance` in this release). Pin it and keep it behind `docs_layout`.
6. Not covered by the spike: **hyphenation** (parley has no hyphenation; add soft hyphens from `hyphenation` patterns before building, or use a custom break pass), **tables and floats** (variable line widths through `BreakLines`, not tried), **hinting differences** (layout is unhinted, while GPUI rasterises hinted at screen size, so glyphs may look slightly tighter or looser than their advances at small zoom), **fcitx5 on Wayland** (GPUI platform, plan §0 item 1), and a real `paint_glyph` run in a window. The spike opened no window, so the first `docs_ui` element must confirm the glyphs land where `glyph_runs` says.
