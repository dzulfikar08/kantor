# 0007: Citations come from Zotero; Kantor keeps every manager's citations intact

- **Status:** accepted (2026-11-27).
- **Decides:** how Kantor Docs reads, keeps, formats and inserts citations and bibliographies (`crates/kantor_cite`, `docs_engine::citations`, the Docs References commands, `kantor docs citations`).

## Context

Indonesian students and lecturers write theses (skripsi, tesis, disertasi) and papers in Word with a reference manager: Zotero and Mendeley above all, some with Word's own Source Manager or EndNote. Their `.docx` files carry citations as fields or content controls whose code holds the cited items' data; the manager's Word plug-in re-renders them and the bibliography. If Kantor breaks those codes, the student's file stops working with their plug-in, which is worse than not supporting citations at all.

What each manager writes (from the files the plug-ins produce and their public documentation):

| Manager | Citation | Bibliography | Document settings |
|---|---|---|---|
| Zotero (Word plug-in) | complex field `ADDIN ZOTERO_ITEM CSL_CITATION {json}`: CSL citation JSON with each item's CSL-JSON `itemData`, `uris` into the Zotero library, and the last `formattedCitation` | `ADDIN ZOTERO_BIBL {json} CSL_BIBLIOGRAPHY` (uncited, omitted and custom entries) | `docProps/custom.xml` `ZOTERO_PREF_1…n`: a `<data>` XML split in 255-character chunks with the style id and locale |
| Mendeley Desktop (legacy plug-in) | `ADDIN CSL_CITATION {json}` (same CSL citation JSON, `mendeley.formattedCitation`) | `ADDIN Mendeley Bibliography CSL_BIBLIOGRAPHY` | `custom.xml` `Mendeley Citation Style_1`, `Mendeley Recent Style Id n_1`… |
| Mendeley Cite (Office add-in) | inline content control, `w:tag` = `MENDELEY_CITATION_v3_` + base64 of the CSL citation JSON | block content control tagged `MENDELEY_BIBLIOGRAPHY` | the add-in's web extension part |
| Word | content control with `w:citation` around a `CITATION Tag \l 1033` field | content control (`docPartGallery` "Bibliographies") around a `BIBLIOGRAPHY` field | sources as `b:Sources` in a custom XML part |
| EndNote | `ADDIN EN.CITE <EndNote>…</EndNote>` (often a nested `ADDIN EN.CITE.DATA` with base64) | `ADDIN EN.REFLIST` | `custom.xml` / document variables |

## Options

1. **A reference manager inside Kantor** (Mendeley-like library, browser capture, PDF annotation, sync). Years of work to match Zotero, and users already have their libraries in Zotero or Mendeley.
2. **Integrate with Zotero** (open source, free, runs well on macOS, Windows and Linux, has a local HTTP connector), and **keep everyone else's citations intact** (chosen).
3. Mendeley as the live integration: Mendeley Desktop is retired and Mendeley Cite runs only inside Word (an Office web add-in), so there is nothing on the user's machine to talk to.

## Decision

**Round trip first.** `.docx` files with citations from any of the five managers open, show their last result text, survive editing elsewhere and save with the field codes and content controls byte for byte (Docs' round-trip rules: fields keep their code XML, inline content controls are opaque objects). Inline citation content controls (Mendeley Cite, Word) show their text in the run's formatting rather than a grey placeholder; bibliography content controls (Mendeley, Word) are unwrapped like a table of contents' so their paragraphs read as text. `docs_engine::citations` finds them all and reads their data: the CSL JSON of Zotero, Mendeley Desktop and Mendeley Cite citations, Word's `b:Sources`, EndNote's `<Cite>` records.

**Formatting with CSL through hayagriva** (Typst's citation engine, MIT/Apache-2.0, `csl-json` feature): CSL-JSON items in, citation and bibliography text out, with italics and other formatting kept as runs. Its archive bundles the CSL styles (APA 7th, IEEE, Harvard Cite Them Right, Chicago author-date, Vancouver and about 80 more; the style files are CC BY-SA 3.0 from the CSL project) and the CSL locales, `id-ID` included ("dkk.", "Diakses"). Styles not in the archive are looked up by id in Zotero's own `styles` folder, so a university's custom style installed in Zotero works too. `kantor_cite` wraps it (no GPUI, no document model) together with the importers and the Zotero client.

- **Refresh Citations** re-renders every CSL citation in document order (so numeric styles number by first citation, author-date styles disambiguate) and every bibliography. Zotero and Mendeley citations get the new text as their result; the field code is rewritten only to update `formattedCitation`/`plainCitation` (Zotero) or `mendeley.formattedCitation` (Mendeley Desktop), so the plug-in doesn't report the citation as edited by hand. Citations the user edited by hand in Zotero (`dontUpdate`, Mendeley's `manualOverride`) keep their text.
- **The document's style** is Zotero's document preference (`ZOTERO_PREF_n`), else Mendeley Desktop's, else APA 7th in the interface language's locale. Changing the style writes it back the same way (Zotero's prefs chunked at 255 characters) and refreshes.
- Word `CITATION` and EndNote fields are kept and listed, not re-rendered: their data isn't CSL and their own tools format them (Word's XSL styles, EndNote's Cite While You Write).

**Insert Citation talks to a running Zotero** over its local connector (`http://127.0.0.1:23119`):

1. with Better BibTeX installed, its Cite-as-you-Write picker (`/better-bibtex/cayw?format=json`, Zotero's own quick-format window) chooses the items, then their CSL-JSON is read from the local API (or Better BibTeX's export);
2. otherwise Zotero 7's local API (`/api/users/0/items?q=…&format=json&include=csljson`, enabled in Zotero's Settings ▸ Advanced) feeds Kantor's own search popover.

Kantor writes **Zotero's field format** (`ADDIN ZOTERO_ITEM CSL_CITATION`, a fresh `citationID`, each item's `itemData` and its Zotero URI when the library id is known), so the file keeps working with Zotero's Word plug-in afterwards; in a document that already uses Mendeley Desktop fields, new citations use Mendeley's format instead so its plug-in sees one kind. Insert Bibliography writes `ADDIN ZOTERO_BIBL {"uncited":[],"omitted":[],"custom":[]} CSL_BIBLIOGRAPHY` (or Mendeley's). Each citation carries its items' full data, as Zotero's do: documents are self-contained.

**Without Zotero**, a `.bib` (BibLaTeX/BibTeX, parsed with the `biblatex` crate), RIS or CSL-JSON file is imported into **the document's own source list**, kept as CSL-JSON in a custom XML part (`customXml/itemN.xml`, root `<kantor:sources xmlns:kantor="urn:kantor:citations:1">`, with its `itemProps` part). Chosen over Word's `b:Sources` because:

- it's lossless: the list holds exactly what citation fields embed (CSL-JSON), while `b:Sources` has fewer types and fields (no DOI or container type distinctions for many types) and would need converting back and forth;
- the citations Kantor inserts from it are Zotero fields, which Word's Source Manager doesn't read anyway, while Zotero's plug-in accepts them as embedded items;
- Word keeps custom XML parts it doesn't know on save, so the list survives a trip through Word.

Word's `b:Sources` is still read, for listing Word's own citations.

**CLI and docs JSON:** `kantor docs citations list|refresh|style` work on files; the docs JSON gets `bibliography` sources (a `.bib`/CSL-JSON/RIS path or inline CSL-JSON), inline `cite` runs and a `bibliography` block, so the paparan-doc skill writes cited documents in Zotero's format.

## Consequences

- A thesis written with Zotero or Mendeley can be opened, edited and saved in Kantor and finished in Word with the same plug-in; Kantor can restyle and refresh it on its own.
- hayagriva passes most but not all of the CSL test suite (its `citeproc-pass.txt` lists what passes): known gaps are citation collapsing details, some disambiguation rules, `subsequent-author-substitute` edge cases and note styles (footnote citations are rendered as in-text). Results can differ from Zotero's citeproc-js in punctuation details, so Refresh Citations is a command, never automatic on open.
- Word and EndNote citations are not re-rendered; Mendeley Cite's style (in its web extension part) is not read, so Mendeley Cite documents use the Kantor default until a style is chosen.
- New citations written while Zotero's local API doesn't give the library id have no `uris`: Zotero's plug-in treats them as embedded items (it shows them and can relink them), which needs a check against a real Zotero on the owner's machine.
- The CSL styles and locales ship inside the binary (about 3.5 MB of CBOR from hayagriva's archive).
