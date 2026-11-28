# 0008: Equations are laid out by Kantor's own OpenType MATH engine, not Typst

- **Status:** accepted (2026-11-28). The owner chose Typst's math layout and MiTeX on 2026-11-28, with the condition that Typst's cost be measured first and an own engine considered if it was unreasonable. It was (below), so this records the switch.
- **Decides:** how Kantor reads, draws, edits and writes equations in Docs and Slides (`crates/kantor_math`, used by `docs_engine`, `docs_layout`, `docs_ui`, `slides_engine`, `slides_ui` and the docs/slides JSON).

## Context

Indonesian students and lecturers write theses in Word format, full of equations. Kantor showed Word equations as flat linear text (`a/b`, `x^2`) and kept them as opaque XML. They need real math layout on screen and in PDF, an editor (LaTeX in, Word equation out), and equations in the docs and slides JSON for the CLI and Paparan AI. The install has a 60 MB budget (release profile notes; v0.28.1 ships a 41 MB AppImage, a 53 MB dmg).

## Options

- **Typst** (`typst` + `typst-layout` 0.15, Apache-2.0): high-quality math layout, but only through the whole compiler: math layout needs `typst-library`'s content, styles and engine, `typst-eval` to turn source into content, and a `World`. Measured on a GitHub runner (ubuntu-24.04, fat LTO, one codegen unit, stripped, opt-level 3; `tools/eq-measure`, a CI job since removed): baseline binary 0.34 MB → **31.3 MB with Typst (+30.9 MB)**, 346 crates in the tree (75 not yet in Kantor's: wasmi, syntect and two-face, icu collator data, biblatex, serde_yaml, palette, typst-svg, typst-html…), **279 s** to build that one binary, 2.2 GB peak RAM. Some crates are shared with Kantor (image, rustybuzz, ttf-parser), so the real delta would be a little smaller, still over 20 MB: half the remaining budget for one feature.
- **Typst's math layout pieces alone**: `typst-layout`'s math module is written against `typst-library`'s `Content`, `StyleChain` and `Engine`; cutting it out means forking and maintaining most of the library.
- **MiTeX** (`mitex` 0.2, Apache-2.0) for LaTeX → Typst: +0.28 MB, 61 crates (rkyv 0.7, logos, rowan…). Its output is Typst source that calls MiTeX's own Typst prelude (`mitexsqrt(…)` in the measurement), so it only helps with Typst linked; its parser alone would still need Kantor's own tables from LaTeX names to Unicode.
- **An own engine on the font's OpenType MATH table** (chosen): the TeX rules as W3C MathML Core restates them for OpenType MATH fonts (fraction, script, radical, limit and accent placement from `MathConstants`, size variants and glyph assemblies for stretchy delimiters, radicals, big operators, braces), with ttf-parser (already in the tree; MIT/Apache-2.0) reading the MATH table. Output is glyph ids at points plus filled rectangles, which Docs' existing glyph-run hand-off already draws on screen (`paint_glyph`) and in PDF (krilla `draw_glyphs`). An own LaTeX parser for the math people type (commands, groups, scripts, `\left…\right`, amsmath environments) is a few hundred lines.

## Decision

`kantor_math` (no GPUI; deps roxmltree and ttf-parser) holds:

- **One model** (`ast`): close to OMML (fractions of four kinds, scripts, pre-scripts, radicals, n-ary operators with limit placement, delimiters with separators, matrices, equation arrays with `&` alignment and `#(n)` numbers, accents, bars, boxes, group characters, limits, functions, phantoms, text runs with OMML's alphabets, weights and normal text), display or inline, with `m:oMathPara` justification.
- **Converters**: OMML ⇄ model (lenient reading: tracked deletions out, insertions kept; writing as Word's equation editor does, children in schema order, Word or DrawingML run properties), LaTeX ⇄ model, the linear format (UnicodeMath basics in, everything out), model → Typst source (export only).
- **Layout** (`layout`): the engine above, with **STIX Two Math** (SIL OFL 1.1, unmodified, 1.5 MB; about 0.8 MB compressed in the installers) bundled as the math font. It matches the Times New Roman theses are set in; Word's Cambria Math isn't free to ship. Letters are drawn from the font's math italic alphabet; characters the font lacks can come from a fallback text font. Layouts are cached by OMML, size and width.

Unedited equations keep their XML byte for byte (they stay opaque objects in Docs and raw `a14:m` in Slides, laid out from that XML); new and edited ones are written from the model.

In **Docs**, an equation is an inline box (`ObjectKind::Math`) with its own baseline (parley's `InlineBox::baseline`); display equations take the line's width and place their numbers at the right margin. **Slides** draws `a14:m` over the run of linear text it keeps (hidden), through the same docs_layout path; text with equations stays locked against typing, and the equation itself is edited through Insert Equation.

## Consequences

- Binary cost: the engine is a few thousand lines of Rust and two crates already in the tree; the font adds 1.5 MB. No Typst, no MiTeX, no WebAssembly runtime.
- Layout quality follows the font's MATH table and TeX's rules; small differences from Word remain (no `ssty` script-size glyph forms, no math kerning tables, no line breaking inside long inline equations, matrix and equation-array spacing approximated).
- Typst source is still available as an export (`kantor_math::typst`), and a Typst-based renderer could come back behind `kantor_math::layout` if the budget ever allows it.
- What LaTeX Kantor reads is listed in `docs/docs-json.md` (Equations); unknown commands are errors that name the command.
