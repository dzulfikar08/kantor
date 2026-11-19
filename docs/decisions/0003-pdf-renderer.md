# 0003: hayro renders PDFs in the viewer

- **Status:** accepted (2026-11-19), step 1 of the PDF app (viewer). Steps 2 (page tools in the window) and 3 (annotations) build on it.
- **Decides:** which library draws pages, extracts text and reads outlines and links for Kantor's PDF viewer (`crates/pdf_engine`, `crates/pdf_ui`).

## Context

Kantor writes PDFs (krilla) and edits their pages without a window (`kantor pdf`, lopdf), but couldn't open one. The viewer needs a renderer that:

- draws real-world PDFs correctly: embedded and non-embedded fonts (TrueType, CFF/Type 1, CID, Type 3), images (JPEG, JPEG 2000, JBIG2, CCITT), transparency groups, soft masks, blend modes, shadings;
- is fast enough to scroll at 60 fps with a page cache (a page drawn in well under a frame's budget a few times over);
- gives text with positions, for search, selection and copy;
- builds in CI for Linux, macOS and Windows with **no system libraries at runtime** (AppImage, dmg, Windows installer);
- has a licence that fits AGPL-3.0-or-later.

## Options

| | hayro 0.9 | pdfium-render + pdfium | MuPDF |
|---|---|---|---|
| Language / build | pure Rust, `cargo build` | Rust bindings + a prebuilt `pdfium` shared library per platform and CPU (6 binaries to fetch, sign and ship) | C library compiled per platform (cc + its own third-party tree) |
| Licence | MIT / Apache-2.0 | MIT/Apache bindings, BSD pdfium | AGPL-3.0 (compatible, but every fork stays AGPL) |
| Size added | already linked; its embedded standard fonts and CJK CMaps add about 0.55 MB | a pdfium library of several MB per platform | the library plus its fonts, several MB |
| Already in Kantor | yes: `sheets_pdf` uses it for print preview | no | no |
| Fonts | TrueType, CFF, Type 1, CID, Type 3; the 14 standard fonts and Adobe's CJK CMaps embedded | all | all |
| Images | JPEG, JPEG 2000 (`hayro-jpeg2000`), JBIG2 (`hayro-jbig2`), CCITT | all | all |
| Encryption | RC4, AES-128, AES-256 with a user password | all | all |
| Text | glyphs with Unicode (ToUnicode, glyph names) through its `Device` trait; extraction is ours | full text API | full text API |

pdfium and MuPDF are the reference renderers, but each brings a native build or binary per platform into a project that is all Rust today (the Windows port and the notarised dmg are already the slowest parts of a release). hayro is written by krilla's author, is already compiled into Kantor, and covers the features above. pdfium and MuPDF weren't built for this comparison; their columns come from their documentation.

## Measurements

No public PDFs were in the real-world sweep (`~/.cache/realworld` has none yet), so the files were made here: Kantor's own exports (601-page `.docx` → PDF through krilla with embedded CID TrueType subsets; three corpus decks with charts, SmartArt and gradients), the same document rewritten by Ghostscript 10.08 (CFF fonts, xref streams, outline from pdfmarks), a hand-written PDF with non-embedded Helvetica and link annotations, and that file encrypted by qpdf with AES-256. All open, render and give their text, outline and links; the AES file opens with its password.

`cargo run --release -p pdf_engine --example bench -- FILE.pdf QUERY` (Linux, x86_64, release build, one thread):

| file | open | render 100 % (96 dpi), median / max | render 200 % | text + search, all pages |
|---|---|---|---|---|
| `big.pdf`, 601 pages (Kantor export, 1.2 MB) | 2.5 ms | 7.4 / 12.4 ms | 22.8 / 29.3 ms | 558 ms (1.46 MB of text) |
| Ghostscript rewrite, 60 pages + outline + links | 0.5 ms | 9.0 / 10.9 ms | 26.6 / 32.6 ms | 43 ms |
| `charts.pptx` export, 6 slides | 0.1 ms | 5.5 / 7.0 ms | 21.9 / 22.2 ms | 1 ms |
| `smartart.pptx` export, 18 slides | 0.3 ms | 5.7 / 13.8 ms | 23.1 / 26.5 ms | 3 ms |

For scale, poppler's `pdftoppm -r 96` on the same 601-page file takes about 16 ms a page (including writing the PPM), so hayro is at least as fast as the renderer most Linux viewers use. At 100 % a page is drawn in a few milliseconds on a background thread; scrolling paints cached images, so the frame itself does no PDF work.

Memory in the app (debug build, 1550 × 975 window at 125 %, `big.pdf`): 247 MB after opening; 349, 356 and 364 MB after 200, 400 and 600 pages of Page Down at Fit Width; at 300 % zoom it levels off around 420 MB. Page images are cached up to 96 MB (pages on screen are always kept), thumbnails up to 24 MB, and hayro's own cache is replaced every 48 pages.

## Decision

Use **hayro** (with its embedded standard fonts and CMaps) for rendering, text, outlines and links, in `pdf_engine`; lopdf stays for writing (`kantor_pdf`, and later page tools and annotations).

## Consequences

- No native code or downloads in the build; the viewer works the same on the three platforms.
- Text extraction (word and line breaks from glyph positions) is Kantor's: content-stream order, which suits nearly all producers; text drawn out of reading order (some multi-column layouts) is selected in drawing order.
- Pages are drawn whole: at high zooms a page image can reach the 6000-pixel cap (its resolution is lowered to fit), where a tiled renderer would stay sharp. Tiles can be added later without changing the engine.
- If a file renders wrong, the fix goes to hayro upstream (MIT/Apache) or a patch in `vendor/`, as with IronCalc.
