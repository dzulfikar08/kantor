# Word documents from JSON: `kantor docs` and the MCP docs tools

Kantor writes and revises Word documents from JSON, with no window and
without python-docx. What it writes is ordinary Word: real paragraph and
character styles, multilevel heading numbers, a table of contents field,
SEQ captions, REF and PAGEREF cross-references, footnotes, sections,
headers and footers with PAGE and NUMPAGES fields, tables with repeating
header rows, inline pictures, and charts with their data in an embedded
workbook (Word's *Edit Data* opens it). Presentations and workbooks have the
same kind of commands: [slides-json.md](slides-json.md),
[sheets-json.md](sheets-json.md).

```sh
kantor docs build doc.json -o doc.docx          # a whole document (-f to overwrite)
kantor docs build doc.json -o doc.docx --check  # build, then check
kantor docs list doc.docx                       # outline as JSON
kantor docs list doc.docx --text [--start N --limit N]   # paragraphs with ids
kantor docs check doc.docx [--json]             # problems, with paragraph ids and pages
kantor docs edit doc.docx TOOL '{...}'          # one tool, saved in place
kantor docs edit doc.docx -o out.docx --ops ops.json [--track "Reviewer"]
kantor docs schema > docs-json.schema.json
kantor convert doc.docx -o shots/ --to png [--contact-sheet] [--page 2-4]
```

The MCP server (`kantor mcp`) offers the same as `docs_build` (a document
from JSON, written to a path), `docs_edit` (a list of tools run on a file),
and `docs_check` (findings, and the outline with `outline: true`). The tools
that edit the document open in Kantor (`docs_get_paragraphs`,
`docs_insert_paragraphs`, …) are unchanged.

The full schema is [docs-json.schema.json](docs-json.schema.json). A
complete example is [proposal.json](examples/docs/proposal.json): an
eleven-page proposal with a cover, contents, numbered sections, tables, a
chart, a figure, footnotes, cross-references and a landscape appendix.

The same JSON always gives a byte-identical `.docx`.

## A document

```json
{
  "preset": "proposal",
  "accent": "#1F5E5B",
  "lang": "en-US",
  "properties": {"title": "Maintenance platform proposal", "company": "Example Engineering", "date": "2026-11-25"},
  "cover": {"label": "Proposal", "subtitle": "For the Cikarang plant",
            "meta": [["Prepared for", "PT Example Manufacturing"], ["Date", "25 November 2026"]]},
  "toc": {"levels": 2},
  "blocks": [
    {"h1": "Summary", "id": "summary"},
    "We propose a **twelve-week** rollout in three phases (see {ref:plan}).[^Based on the 2026 site visit.]",
    {"bullets": ["Fewer stoppages", {"text": "Faster repairs", "items": ["Spare parts on hand"]}]},
    {"h1": "Plan", "id": "plan"},
    {"type": "table", "caption": "Phases and dates", "id": "phases", "columns": [3, 1, 1],
     "header": ["Phase", "Weeks", "Cost"], "rows": [["Pilot", "4", "$40,000"], ["Rollout", "8", "$95,000"]]}
  ]
}
```

Top-level keys:

- `preset`: `report` (default), `proposal` or `letter` (below).
- `accent`: the client's brand colour. Headings, numbers, bullets, links,
  table rules and the first chart series follow it; the preset's tints are
  mixed from it.
- `fonts`: `{"heading", "body", "mono"}` font names. The mono font (inline
  code, the `Code` character style) is Courier New by default: an Office
  font with a metric-compatible stand-in (Liberation Mono) where it isn't
  installed.
- `lang`: a language tag (`en-US` default). `id-ID` (Indonesian) also
  sets the labels (Gambar, Tabel, Daftar Isi) and the decimal comma.
- `properties`: `title`, `subject`, `author`, `company`, `keywords`,
  `description`, `date` (YYYY-MM-DD; used for the file's dates and date
  fields), `version`, `status`. Header and footer tokens show them.
- `cover`: `label` (a small line above the title), `title` (default: the
  properties' title), `subtitle`, `meta` (`[[label, value], …]` lines),
  `note` (e.g. a confidentiality line), `logo` (a picture path) and
  `logo_width`. The cover and the table of contents are front matter: no
  header or footer, and the body's pages are numbered from 1.
- `toc`: `true` or `{"title", "levels"}` (1–4, default 2): a table of
  contents after the cover, filled in with page numbers when built.
- `page`: `size` (`A4` default, `Letter`, `Legal`, `A5`, `A3`, `F4`, or
  `{"w", "h"}`), `orientation`, `margins` (one length, or `{top, bottom,
  left, right}`), `header` and `footer` distances.
- `header`, `footer`: `{"left", "center", "right"}` texts (or a string:
  right-aligned; `null` or `"none"`: none). Tokens: `{page}`, `{pages}`,
  `{section_pages}`, `{title}`, `{subject}`, `{author}`, `{company}`,
  `{heading}` (the page's Heading 1, a STYLEREF field), `{date}`,
  `{version}`, `{status}`. Page numbers and the title are fields.
  `{pages}` counts the pages that are numbered: with a cover or contents
  the body's pages (numbered from 1), so the last page reads "8 / 8", not
  "8 / 10". It is a `NUMPAGES` field without front matter, `SECTIONPAGES`
  when the body is one section (the front matter is a section of its
  own), and with `section` blocks a `PAGEREF` to the hidden bookmark
  `_KantorEnd` at the end of the document (the last page's number;
  Kantor fills it in when building, Word updates it as it lays out the
  pages, or with F9). Word keeps all three right as the text changes.
- `first_page_header`: `false` leaves the header and footer off the body's
  first page (letters).
- `numbered_headings`: `false` turns heading numbers off.
- `styles`, `palette`, `table`, `callout`, `cover_style`, `labels`: change
  the look (below).
- `markdown`: `false` reads strings as plain text.
- `blocks`: the content.

Lengths are points (`12`) or strings with a unit: `"2.5cm"`, `"18mm"`,
`"1in"`, `"12pt"`, `"96px"`. Widths may be a share of the text width:
`"60%"`.

## Text

A paragraph's `text` is a string or a list of runs. Strings read light
markdown:

| write | get |
|---|---|
| `**bold**`, `*italic*`, `` `code` `` | the character styles Strong, Emphasis and Code (the mono font) |
| `[label](https://example.com)` | a hyperlink (`[label](#id)` links inside the document) |
| `[^Source: annual report 2025.]` | a footnote (the note's own text may use markdown) |
| `{ref:id}` | a cross-reference to the heading, figure or table with that `id` |
| `{ref:id:page}` / `:text` / `:number` / `:label` / `:full` | its page, its words, its number, "Figure 2", "Figure 2: caption" |
| `\n` | a line break |
| `\*`, `\[`, `\{` | the character itself |

A cross-reference is a REF (or PAGEREF) field: Word keeps it right when
the document changes (F9). By default a numbered heading shows its number
(`see section {ref:plan}` → "see section 2"), an unnumbered one its words,
a figure or table its label ("Table 1").

A run is a string or an object: `text`, `bold`, `italic`, `underline`,
`strike`, `caps`, `small_caps`, `color`, `highlight`, `size`, `font`,
`style` (a character style: `Strong`, `Emphasis`, `Code`, or your own),
`superscript`, `subscript`, `code`, `link`, `footnote`, `ref` with `show`,
or `field` (any Word field instruction, e.g. `DATE \@ "d MMMM yyyy"`) with
`result`. Prefer styles to run formatting: `check` counts direct formatting.

## Blocks

A plain string is a paragraph. Every other block is an object with `type`,
or a shorthand: `{"h1": "…"}` to `{"h4": "…"}`, `{"p": "…"}`,
`{"bullets": […]}`, `{"numbers": […]}` (other keys go beside it:
`{"h2": "Scope", "id": "scope"}`).

- **heading**: `text`, `level` (1–4), `id`, `numbered` (`false` for an
  unnumbered heading such as an appendix title), `page_break_before`.
  Heading 1–3 are numbered 1, 1.1, 1.1.1 with Word's multilevel list
  linked to the styles; Heading 4 isn't.
- **paragraph**: `text`, `style` (any paragraph style: `Quote`, `No
  Spacing`, or one in `styles`), and paragraph or run settings (`align`,
  `space_before`, `keep_with_next`, `bold`, `color`, …).
- **list**: `kind` (`bullet` or `number`), `items` (text, or `{"text",
  "items" | "bullets" | "numbers"}` for nested levels), `start`,
  `continue` (numbers go on from the previous numbered list). Uses the
  List Bullet / List Number styles and real Word numbering; each numbered
  list starts again at 1.
- **table**: `header` (a row, or several rows), `rows` (arrays of cells),
  `columns` (weights such as `[3, 1, 1]`, lengths, percentages, or a
  count), `width`, `align` (per column), `caption`, `id`, `total` (the last
  row is a total: bold, rule above), `look` (`rules`, `grid`, `header`,
  `plain`), `header_rows`, `repeat_header` (default true). A cell is text, a
  number, `null` under a span, or `{"text", "col_span", "row_span",
  "align", "valign", "bold", "italic", "color", "fill", "bullets"}`.
  Columns of numbers (`1,200`, `45%`, `$3.2M`, `Rp 12.000`) are
  right-aligned. Header rows repeat on every page and rows don't break
  across pages. The caption goes above the table.
- **picture**: `path` (PNG or JPEG; relative to the JSON file), `width`
  (default: its size at 96 dpi, at most the text width), `height`, `alt`,
  `caption`, `id`, `align`. The caption goes below.
- **chart**: the chart keys of the slides JSON (`chart`: column, bar,
  line, area, pie, doughnut, scatter, combo; `categories`, `series`,
  `stacked`, `title`, `legend`, `data_labels`, `number_format`, `axis`,
  `category_axis`, `colors`, `target_line`, `font`, …; see
  [slides-json.md](slides-json.md#charts)) plus `width`, `height` (default
  55 % of the width), `caption`, `id`, `alt`. Series take the accent
  colours; palette names work in colours.
- **callout**: `title`, `text` (text, or an array of paragraphs and
  `{"bullets": […]}`), `fill`, `bar`. A tinted box with a bar down the left
  (a one-cell table, so it reads the same everywhere).
- **quote**: `text`, `source`.
- **page_break**: the next block starts a new page.
- **section**: a new section from here: `orientation`, `size`, `margins`,
  `header`, `footer`, `page_numbers` (`{"start", "format"}`), `columns`,
  `start` (`next_page`, `continuous`, `odd_page`, `even_page`),
  `first_page_header`. Keys left out keep the previous section's, so a
  landscape appendix is `{"type": "section", "orientation": "landscape"}`
  and the way back `{"type": "section", "orientation": "portrait"}`.
  Headers and footers are made again for a section of another width, so
  their right-aligned parts stay at the right margin.
- **toc**, **list_of_figures**, **list_of_tables**: `title` (or `false`),
  `levels`. Placed where they are in the blocks (the top-level `toc` puts
  one after the cover).

`id`s are on headings and on captioned tables, pictures and charts; two
blocks can't share one, and a reference to an id that doesn't exist is an
error.

## Presets and the look

- `report`: calm and analytical. Cambria headings over Calibri text, the
  first level in a slate blue, the rest in ink; numbers 1, 1.1, 1.1.1 in a
  hanging column. Tables have rules only (a heavier rule above and below,
  hairlines between rows, none down). The cover is text with a narrow
  accent strip down the left edge of the page. The header shows the title
  and the current section, the footer the page number.
- `proposal`: Calibri throughout, bolder. Each Heading 1 starts a new page,
  Heading 2 is in the accent (a deep green by default). Table headers are
  tinted. The cover has an accent band across the top. The header shows
  the title and the company, the footer the current section and
  the page number.
- `letter`: no cover, no numbering, no header; page numbers from page 2.

Every look keeps to one accent with neutral inks; corners and rules are
plain. Fonts are Office fonts (Calibri, Cambria) so the file looks the same
for whoever opens it; Kantor draws Calibri with its metric twin Carlito
where Calibri isn't installed.

Any part goes over the preset key by key:

```json
{
  "preset": "report",
  "accent": "#7A1F3D",
  "fonts": {"heading": "Georgia", "body": "Arial"},
  "palette": {"muted": "#5B6676"},
  "styles": {
    "Normal": {"size": 11, "line_spacing": 1.15, "align": "justify"},
    "Heading 1": {"size": 20, "page_break_before": true},
    "Lead": {"size": 13, "color": "muted", "space_after": 12},
    "Key term": {"type": "character", "bold": true, "color": "accent"}
  },
  "table": {"look": "header", "header_fill": "tint", "banded": "#F7F7F7"},
  "callout": {"fill": "tint", "bar": "accent"},
  "cover_style": {"kind": "band"},
  "labels": {"figure": "Exhibit"},
  "header": {"left": "{company}", "right": "{title}"},
  "footer": {"center": "Page {page} of {pages}"}
}
```

Style settings: `font` (`heading`, `body`, `mono`, or a name), `size`,
`bold`, `italic`, `underline`, `strike`, `caps`, `small_caps`, `color`,
`highlight`, `letter_spacing`, `align`, `space_before`, `space_after`,
`line_spacing` (a multiple), `indent`, `indent_right`, `first_line`
(negative: hanging), `keep_with_next`, `keep_together`,
`page_break_before`, `outline_level`, and for new styles `type`
(`paragraph` or `character`), `based_on`, `next`.

The built-in styles are Word's own (Word shows them under their usual,
translated names): Normal, Title, Subtitle, Heading 1–4, Quote, Caption,
List Bullet (2, 3), List Number (2, 3), TOC Heading, TOC 1–3, Table of
Figures, Header, Footer, Footnote Text, No Spacing; the characters styles
Strong, Emphasis, Hyperlink, Footnote Reference; and Kantor's Figure,
Table Text, Table Header, Callout, Callout Title, Cover Label, Cover Meta
and Code. The table style is "Document Table". Inline code (`` `x` `` and
`"code": true` runs) takes the Code style, so `"styles": {"Code": {"size":
9.5}}` changes all of it. Header rows of tables keep with the first body
row, and rows don't break across pages.

Palette names (anywhere a colour goes): `accent`, `ink`, `muted`, `rule`,
`tint`, `bullet`, `link`, `accent2`–`accent6` (chart series), `white`,
`black`.

**Making it the client's.** Give `accent` (their brand colour), `fonts`
(their typefaces, if the readers have them), `properties.company`, a
`cover.logo`, and `labels` in their language (`lang: "id-ID"` does it for
Indonesian). Keep the preset's structure; change `styles` only where their
own style guide says so.

## Reading a document

`kantor docs list FILE` prints the outline: `paragraphs`, `pages`,
`words`, `sections` (paragraph range, page size, orientation, margins,
columns), `headings` (a tree: `id`, `level`, `number`, `text`, `style`,
`page`), `styles` (paragraphs per style and how many have direct
formatting), `direct_formatting`, `tables` (`id`, rows, columns, header
rows, caption, page), `pictures` and `charts` (alt text, page),
`field_counts` and the `fields` that reference (TOC, REF, PAGEREF, SEQ,
NOTEREF), `comments`, `tracked_changes` (the body's, the first 500;
`tracked_change_count` counts them all, headers, footers and notes
included), `notes`, `bookmarks` (the names
headings and captions carry: blocks inserted later can point at them with
`{ref:_Ref100000003}`), `track_changes_on`.

`--text` prints the paragraphs: `id`, `style`, `text` (pictures as
`[picture]`, charts as `[chart]`), `number` (a list or heading number),
`heading` level, `table` (row and cell), `page`. Tracked deletions show
as `[-deleted text-]`. Long documents come in
pages of 500 (`--start`, `--limit`; `next_start` says where to go on).

A paragraph id is its index from 0, counting table cells' paragraphs in
order (the MCP docs tools' `index`). Ids after an edit point move when
paragraphs are added or removed: list again after edits.

## Editing

```sh
kantor docs edit report.docx find_replace '{"find": "ACME", "replace": "Acme Corp"}'
kantor docs edit report.docx -o revised.docx --track "Dzul" --ops ops.json
```

`ops.json` is `[{"tool": "…", "args": {…}}, …]`, run in order. With
`--track AUTHOR` (MCP: `track`) text changes, inserted blocks and
paragraphs, deletions and paragraph style changes are tracked changes by
that author; the file keeps Track Changes as it had it.
`replace_paragraph` leaves the old text struck through before the new;
`insert_paragraphs` marks the new paragraphs' marks inserted, so
rejecting removes them whole; `delete_paragraphs` strikes the text and
the paragraph marks, so accepting removes the paragraphs.

`find_replace` and `format` look where `in` says: `all` (the default)
is the body and every header and footer (first-page, even and default
ones, in every section), as Word's Replace All; `body`; `headers` (the
headers and footers only). They search paragraphs and table cells, not
text boxes, footnotes or endnotes; a header Kantor can't edit is
skipped.

| tool | args | does |
|---|---|---|
| `get_paragraphs` | `start`, `limit` | the paragraphs (as the MCP tool) |
| `insert_paragraphs` | `after_index`, `paragraphs` | plain paragraphs with a style or list |
| `replace_paragraph` | `index`, `text` | one paragraph's text, keeping its style |
| `delete_paragraphs` | `index`, `count` | paragraphs outside tables |
| `insert_blocks` | `blocks`, and `after` / `before` (ids), `after_heading`, `before_heading`, `end_of_section` (heading text or number), `at_start`; `preset`, `accent`, `fonts`, `lang` | build-JSON blocks (tables, charts, pictures, lists, callouts, captions, references, footnotes) placed in the document. Styles the document lacks are added from the preset; the document's own heading styles are used. Captions and references are updated. |
| `replace_blocks` | `from`, `to` (ids) or `heading`; `blocks` | the range (whole tables) replaced by blocks |
| `delete_blocks` | `from`, `to` or `heading` | a range, or a heading's whole section |
| `move_section` | `heading`; `before_heading`, `after_section` or `to_end` | a heading and everything under it (not tracked) |
| `set_style` | `from`, `to` (or `heading`), `style` | apply a paragraph style (a built-in one is added if missing) |
| `restyle` | `style`, style settings (`size`, `color`, `space_before`, …); `type`, `based_on` for a new one | change a style's definition (every paragraph in it follows); makes the style if there is none |
| `find_replace` | `find`, `replace`, `match_case`, `whole_word`, `in`, and optionally `bold`, `italic`, `underline`, `color`, `size`, `font`, `highlight`, `style` (character) | replace text; with formatting keys the new text is formatted. A `\n` in `replace` is a line break in the same paragraph (`w:br`) |
| `format` | `find`, formatting keys, `in` | format every match |
| `add_comment` | `paragraph`, `text`, `find` (the words to comment on), `author` | a comment |
| `resolve_comment` | `id`, `done` | mark resolved |
| `accept_changes`, `reject_changes` | nothing (all), or `from`/`to`, `heading` | resolve tracked changes |
| `update_fields` | | the table of contents, page references, cross-references, caption numbers (F9 on everything) |
| `set_header`, `set_footer` | `left`, `center`, `right` (tokens as in build), `none`, `section` | a header or footer for every section (or one) |
| `page_setup` | `size`, `orientation`, `margins`, `header`, `footer`, `section` | page setup of every section (or one) |

Each tool prints a JSON line with what it did; `saved FILE` ends the run.

## Checking

```sh
kantor docs check doc.docx            # for people; exit status 1 on errors
kantor docs check doc.docx --json     # for programs and agents
kantor convert doc.docx -o shots/ --to png --contact-sheet   # shots/doc-sheet.png
```

The document is laid out as Kantor draws it (the app, the PDF and the
PNGs use the same layout). Findings name the paragraph id and the page:

| check | severity | what |
|---|---|---|
| `empty_heading` | error | a heading with no text |
| `heading_skip` | warning | a heading more than one level below the one before (Heading 1 then Heading 3) |
| `fake_heading` | warning | a short bold or large paragraph in a body style: a heading made by hand instead of with a Heading style |
| `direct_formatting` | warning / info | text formatted by hand (fonts, sizes, colours, bold) instead of with styles: warning when many paragraphs have it |
| `broken_reference` | error | a cross-reference (REF, PAGEREF, NOTEREF) to a bookmark that isn't there, or showing Word's "Error! Reference source not found" |
| `toc_stale` | warning | a table of contents that doesn't match the headings and pages (update fields), or has no entries |
| `caption_numbers` | warning | figure or table numbers (SEQ fields) that repeat, skip or are out of order |
| `missing_caption` | info | a picture or chart without a caption |
| `missing_alt_text` | warning | a picture or chart without alt text |
| `table_header` | warning | a table (of two rows or more) whose first row isn't a header row repeated on each page |
| `cell_overflow` | error | text wider than its table cell (a long word, number or link that can't wrap) |
| `past_margin` | warning | text, a table or a picture reaching past the right margin |
| `heading_at_page_bottom` | warning / info | a heading alone at the bottom of a page, its text on the next (info when the heading keeps with the next paragraph: Word moves it) |
| `table_at_page_bottom` | warning / info | a table whose header row sits alone at the bottom of a page, its body rows on the next (warning), or that starts with one body row before a page break (info) |
| `mixed_fonts` | warning | body text in more than one font family |
| `placeholder_text` | error | text left from a template: Lorem ipsum, TBD, TODO, XXX, [Client name], <insert …>, {{field}} |
| `double_space` | info | two spaces in a row inside a sentence |
| `open_comments` | warning | comments not marked resolved |
| `tracked_changes` | warning | tracked changes not accepted or rejected |
| `low_contrast` | warning / error | text colour against its background (the page, or the cell's shading) under 4.5:1 (3:1 for large text); error under 2:1 |
| `font_missing` | warning / info | a font not installed here: line breaks and pages can differ where it is (info when a metric-compatible substitute stands in) |

## The loop for agents

1. Write the JSON: facts from the brief, a preset, the client's accent.
2. `kantor docs build doc.json -o doc.docx --check`: fix the errors.
3. `kantor convert doc.docx -o shots/ --to png --contact-sheet` and look
   at every page: covers, tables that break badly, charts, the landscape
   pages.
4. Change the JSON (or `kantor docs edit` an existing file), build again.

## What isn't supported yet

- Floating pictures and text wrapping around them (pictures and charts are
  in line with the text); text boxes; SmartArt; equations.
- Endnotes, an index, citations and a bibliography.
- Tracked moves (`move_section` can't be tracked) and tracked style
  definition changes (`restyle` isn't tracked).
- Inserting a section break with `insert_blocks` (use `page_setup` on an
  existing section).
- Paragraph borders and shading aren't drawn in Kantor's PDF and PNG
  (Word shows them); the presets don't use them.
