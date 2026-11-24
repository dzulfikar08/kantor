# Slides from JSON: `kantor slides` and the MCP slides tools

Kantor builds and edits presentations from JSON, with no window and without
python-pptx:

```sh
kantor slides build deck.json -o deck.pptx     # a whole deck (-f to overwrite)
kantor slides list deck.pptx                   # slides and shapes as JSON
kantor slides edit deck.pptx add_shape '{"slide": 2, "type": "shape", "x": "1in", "y": "1in", "w": "3in", "h": "1in", "text": "Hello"}'
kantor slides edit deck.pptx -o out.pptx --ops ops.json   # [{"tool": "add_table", "args": {...}}, ...]
kantor slides schema > slides-deck.schema.json
kantor slides check deck.pptx                  # layout problems (see "Checking your deck")
kantor convert deck.pptx -o shots/ --to png    # each slide as a PNG
```

The MCP server (`kantor mcp`) offers the same as tools on the presentation
open in Kantor: `slides_add_shape`, `slides_add_picture`, `slides_add_table`,
`slides_add_chart`, `slides_add_elements`, `slides_set_background`, `slides_update_shape`,
`slides_delete_shape`, `slides_order_shape`, `slides_group_shapes`, and
`slides_add_slide` with `background`, `elements` and `notes`. `slides_build`
takes a whole deck and writes a `.pptx` (`open: true` opens it). An element is
the same object everywhere: in a deck's `elements`, in `slides_add_elements`,
and as the arguments of the single-element tools (plus `slide`).

The full schema is [slides-deck.schema.json](slides-deck.schema.json).
Two complete examples (their pictures come from
`examples/slides/make_pictures.py`): [deck.json](examples/slides/deck.json)
shows most features, [proposal.json](examples/slides/proposal.json) is an
11-slide consultancy-style proposal in 8 KB of JSON:

```sh
kantor slides build docs/examples/slides/proposal.json -o /tmp/proposal.pptx
kantor convert /tmp/proposal.pptx /tmp/proposal.pdf
```

The short way to a good-looking deck: pick a `preset`, give each slide an
`eyebrow`, `title` and `subtitle`, and fill it with components:

```json
{
  "units": "in",
  "preset": "editorial",
  "footer": {"text": "Example Co"},
  "transition": "fade",
  "slides": [
    {"kind": "cover", "eyebrow": "Proposal · November 2026", "title": "Reliable uptime for every line",
     "mark": "96%", "mark_label": "Uptime on the pilot line"},
    {"eyebrow": "At a glance", "title": "Three numbers that matter", "footer_note": "Source: 2026 annual report",
     "elements": [{"type": "stat_cards", "items": [
       {"value": "96%", "label": "On time"}, {"value": "−18%", "label": "Cost per parcel", "highlight": true}, {"value": "4.7", "label": "Rating"}]}]}
  ]
}
```

The same deck always gives a byte-identical `.pptx`.

## Units

Lengths (`x`, `y`, `w`, `h`, `radius`, line `width`, `insets`, ...) are

- a number in the deck's (or tool call's) `units`: `emu` (default; 914 400 per
  inch, 12 700 per point, 360 000 per cm), `pt`, `in`, `cm`, `mm` or `px`
  (96 per inch);
- or a string with its unit: `"1.5in"`, `"2.5cm"`, `"18pt"`, `"914400emu"`.

Positions are from the slide's top left. A 16:9 slide is 13.333 × 7.5 in.
Font sizes, `space_before` and `space_after` are points; `line_spacing` is a
multiple of single spacing (`1.2`) or points (`"20pt"`). Angles are degrees
clockwise. Crop values are fractions (`0.1`) or percentages (`"10%"`).

## Colours and fills

`"#1F3864"` (also `#RGB`, or `#RRGGBBAA` with opacity), a theme colour
(`accent1`…`accent6`, `tx1`, `tx2`, `bg1`, `bg2`, `dk1`, `lt1`, `dk2`, `lt2`,
`hlink`, `folHlink`), or `{"color": "#000000", "opacity": 0.5}`.

A fill is `"none"`, a colour, or a gradient:

```json
{"gradient": {"type": "linear", "angle": 90, "stops": ["accent1", {"color": "#FFFFFF", "position": 0.8}]}}
```

A line (`line`, `border`) is `"none"`, a colour, or
`{"color", "width", "dash", "head", "tail", "cap"}`; `dash` is a preset
(`solid`, `dash`, `sysDash`, `dot`, `lgDashDot`, ...), arrowheads are
`triangle`, `stealth`, `diamond`, `oval`, `arrow` or
`{"type", "width", "length"}` (small, medium, large).

## Elements

Every element has a `type` and optionally a `name`.

- **shape** — `x`, `y`, `w`, `h`, `rotation`, `flip_h`, `flip_v`; `geometry`
  (any of the 187 DrawingML presets: `rect`, `roundRect`, `ellipse`,
  `chevron`, `homePlate`, `rightArrow`, `wedgeRoundRectCallout`, ...),
  `radius` (corner radius of rounded rectangles), `adjust` (raw adjust values,
  e.g. `{"adj": 25000}`); `fill`, `line`; text. Without `fill`/`line` a
  shape looks like a new PowerPoint shape (accent 1, darker outline, white
  centred text).
- **text** — a text box: like a shape, but no fill or outline unless given,
  and text from the top left.
- **line** — from (`x1`, `y1`) to (`x2`, `y2`), with `color`, `width`,
  `dash`, `head`, `tail`, `cap` (or the same inside `line`).
- **picture** — `path` (PNG or JPEG; in a deck file relative to the file, in
  MCP tools absolute), `x`, `y`, `w`/`h` (give one and the other keeps the
  picture's aspect), `fit` (`stretch`, `contain`, `cover`), `crop`
  (`left`/`top`/`right`/`bottom`), `geometry`/`radius`/`adjust` to cut it to
  a shape, `line`, `alt`.
- **icon** — `icon` (a [Lucide](https://lucide.dev/icons) name such as
  `shield-check`, `rocket`, `chart-line`), `x`, `y`, `size` (or `w`/`h`),
  `color` (default accent 1), `stroke`. Written as an outline shape (custom
  geometry, no fill), so it stays sharp and recolours like any line. (The
  key is `icon` because `name` is every element's shape name.)
- **table** — `x`, `y`, `w`, `columns` (widths, or a count with `w`), `rows`
  (arrays of cells: text, numbers, `null` under a span, or
  `{text, fill, border, bold, color, align, anchor, col_span, row_span, ...}`),
  `row_height`/`row_heights`, `header` (default true: bold `header_color` on
  `header_fill`), `banded` with `band_fill`, `fill`, `border` (default 1 pt
  grey), `cell_margins`, `style_id`, and text formatting for every cell.
  `"preset": "clean"` draws it in the deck's card style (below): `rule`
  has no fills, an ink rule above and below, a thinner one under the
  header and hairlines between rows (lines drawn over the table); `fill`
  has an ink header row and banded panel rows. No grid lines either way,
  the first column bold (`first_bold: false` to turn off); `x`, `y`, `w`
  default to the content area. It stays a real table.
- **chart** — a real PowerPoint chart made from data (see [Charts](#charts)).

### Text

`text` is a string (each line a paragraph) or a list of paragraphs. A
paragraph is a string or
`{"text" | "runs", "align", "level", "bullet", "line_spacing", "space_before", "space_after", ...formatting}`;
a run is a string or `{"text", "font", "size", "bold", "italic", "underline", "strike", "color", "caps", "letter_spacing", "field"}`.
`caps: true` writes the text in capitals (and marks it all caps for
PowerPoint); `letter_spacing` is character spacing in points (PowerPoint
and LibreOffice show it; Kantor's own drawing doesn't yet). A run with
`"field": "slide_number"` is a slide number field that PowerPoint keeps
right when slides move (Kantor shows the number it had when built).
Formatting given on the element applies to all its text, on a paragraph to its
runs. On shapes and text boxes: `anchor` (top, middle, bottom), `insets`
(one length or `[left, top, right, bottom]`), `autofit` (`none`, `shrink`,
`resize`), `wrap`, `vertical` (`down`, `up`).

`autofit: "shrink"` is written as PowerPoint's “shrink text on overflow”;
PowerPoint works out the scale when it lays the text out, Kantor shows the
text at full size until then.

## Charts

A `chart` element is a real, editable chart: a chart part with its data in an
embedded workbook, as PowerPoint makes it. PowerPoint's *Edit Data* and
Kantor's chart data panel open the data; `kantor convert` and Kantor draw it.
Don't draw charts from shapes.

```json
{"type": "chart", "chart": "column", "x": 0.6, "y": 1.6, "w": 6, "h": 5.4,
 "title": "Revenue, IDR bn",
 "categories": ["Q1", "Q2", "Q3", "Q4"],
 "series": [{"name": "2025", "values": [41.2, 44.8, 39.5, 52.1], "color": "accent1"},
            {"name": "2026", "values": [45.0, 49.3, 47.7, null]}],
 "number_format": "#,##0.0",
 "data_labels": {"show": "value", "position": "outside_end"},
 "axis": {"min": 0, "max": 60, "major_unit": 10, "title": "IDR bn"},
 "legend": "bottom", "target_line": {"value": 45, "label": "Target"}}
```

| Key | Meaning |
|---|---|
| `chart` | `column`, `bar` (horizontal), `line`, `area`, `pie`, `doughnut`, `scatter`, `combo` |
| `x`, `y`, `w`, `h` | position and size (`w`, `h` required) |
| `categories` | labels along the category axis: strings or numbers (years). Scatter: the x values (numbers) |
| `series` | `[{name, values, color, colors, type, axis, line, data_labels}]`: one value per category, `null` for a gap. `color` is the series' fill (bars, areas, slices) or line colour; `colors` colours single bars or slices; `line` (`{width, dash, color}`) a line series' line |
| `stacked` | `true`/`"stacked"`, or `"percent"` (100 % stacked; value axis `0%`): column, bar, line, area |
| `type`, `axis` (per series) | a combo's series: `column`, `line` or `area`, on the `primary` or `secondary` axis. In a column, line or area chart they make it a combo. Combo default: the first series column, the rest lines |
| `title` | the chart title (none by default) |
| `legend` | `none`, `bottom`, `top`, `right`, `left` (default: bottom with several series or a pie, else none) |
| `data_labels` | `true` (values; pie and doughnut: percentages) or `{show, position, format, size, bold, italic, color}`: `show` is `value`, `percent` (pie, doughnut), `category`, `series` or a list; `position` `center`, `inside_end`, `inside_base`, `outside_end` (bars, pies), `left`, `right`, `above`, `below` (lines, scatter), `best_fit` (pies); `format` a number format; `color` #RRGGBB. Per series too |
| `number_format` | the values' Excel number format (labels and value axis follow it), see below |
| `axis` | the value axis (horizontal in a bar chart): `title`, `min`, `max`, `major_unit`, `gridlines` (default true), `number_format`, `reverse` |
| `category_axis` | `title`, `reverse`; scatter (its x axis): also `min`, `max`, `major_unit`, `number_format` |
| `gap_width`, `overlap` | column/bar/combo: the gap between categories (% of a bar, default 150) and the overlap of a category's bars (-100 to 100) |
| `hole` | doughnut: hole size in % (default 50) |
| `font` | `{font, size, color, bold, italic}` for all the chart's text; the title is 1.2 × `size` (default: the deck's `text` typeface and colour, PowerPoint's size) |
| `colors` | a one-series column, bar, pie or doughnut chart: each bar's or slice's colour, `null` for automatic, e.g. `[null, "#C00000", null]` to mark the one below target |
| `target_line` | column, line, area, combo: a number or `{value, label, color, width, dash}`: a dashed line across the chart at that value (default label "Target", colour `tx1`). It is a line series of the chart, so it shows in the legend and in Edit Data |

Colours are `#RRGGBB` or theme colours (`accent1`, ...) as everywhere; without
one, series take the theme's accents in order.

**Number formats** are written as Excel stores them, whatever the language:
`,` groups thousands and `.` is the decimal point in the code (`"#,##0.0"`,
`"0%"`, `"0.0%"`, `"\"Rp\" #,##0"`). PowerPoint and Excel show the numbers
in the viewer's locale, so on an Indonesian system `"#,##0.0"` shows
`1.234,5`.

The embedded workbook has one sheet, `Sheet1`: series names in row 1 from
column B, categories in column A from row 2, each series' values under its
name. Same JSON, same bytes.

`kantor slides list` shows a chart as `"kind": "chart"` with `"chart"` (its
type: `column`, `stacked bar`, `100% stacked column`, `doughnut`, `combo`...),
`"series"` (names) and `"categories"` (how many). Add one to an existing file
with `kantor slides edit deck.pptx add_chart '{"slide": 2, ...}'` (or the MCP
tool `slides_add_chart`, or in `add_elements`). A complete example with every
type is [examples/slides/charts.json](examples/slides/charts.json).

Limits: pie and doughnut charts show one series; a horizontal bar chart can't
mix series types; the secondary axis takes its numbers' format from
`number_format` too (no options of its own); no 3-D, radar, stock or bubble
charts, trendlines or error bars.

## Decks

```json
{
  "size": "16:9",
  "units": "in",
  "theme": {"fonts": {"heading": "Inter", "body": "Inter"}, "colors": {"accent1": "#2F5597"}},
  "text": {"color": "#1B2A4A"},
  "background": "#FFFFFF",
  "slides": [
    {"title": "Agenda", "bullets": ["One", {"text": "Detail", "level": 1}], "notes": "..."},
    {"layout": "Blank", "background": {"picture": "cover.jpg"}, "elements": [ ... ]}
  ]
}
```

- `size`: `16:9` (default), `4:3`, `16:10`, `a4`, `letter`, or `{"w", "h"}`.
- `theme`: `name`, `fonts` (`heading`, `body`) and `colors`
  (`dk1`, `lt1`, `dk2`, `lt2`, `accent1`–`accent6`, `hlink`, `folHlink`).
- `text`: default formatting for every element's text.
- `background`: every slide's, unless a slide has its own.
- A slide: `layout` (Title Slide, Title and Content, Title Only, Blank; by
  default Title and Content with `bullets`, Title Only with only a `title`,
  else Blank), `title`, `bullets`, `background` (a fill, `{"picture": path}`
  or `"inherit"`), `elements` (drawn in order, later on top), `notes`.
  With a deck `header` (or a preset): `eyebrow`, `subtitle`, `kind`
  (`content`, `cover`, `section`), `footer`, `footer_note` and `transition`;
  on covers `meta`, `mark` and `mark_label` (below); a slide with its own
  `layout` keeps the layout's placeholders.
- Deck keys for the look: `preset`, `card_style`, `palette`, `styles`,
  `header`, `footer`, `cover`, `content`, `transition` (below).

## Presets, styles and the palette

`"preset"` picks a built-in look:

- `editorial`: an annual-report feel. Warm paper (`#F6F1E9`) on every
  slide, titles and big numbers in Instrument Serif, body and labels in
  Inter, one terracotta accent. The header is a short accent rule with the
  section label beside it in sentence case, then the title; the footer a
  hairline with the source note at the left and the deck's name and page
  number (`02`) at the right. Cards and stats have no boxes, only a rule on
  top; tables only rules; the callout is a serif pull quote beside an accent
  rule. The cover has a full-height accent block on the right carrying one
  big number (`mark`); sections are dark with serif numerals in the accent.
- `swiss`: a modernist grid. White, Inter bold titles under a 3 pt ink rule
  across the content width, labels in JetBrains Mono capitals (`02 /
  SECTION`, the number in the accent), one strong blue. Cards and stats are
  solid square panels, the highlighted one blue with white text; tables
  have an ink header row and banded rows; the callout is a blue bar. The
  footer is the source note and a small blue square. The cover is all blue
  with a big white title and a 4×4 grid of squares.
- `mono`: `swiss` in black, white and one red.

Corners are square everywhere. A preset sets the theme colours and fonts, the
palette, the text styles, the card style, the header, footer and covers, and
the slide background; the deck's own `theme`, `palette`, `styles`,
`header`, `footer`, `cover`, `card_style` and `background` go over it key by
key. A deck that sets its own `theme.colors.accent1` (its brand colour) gets
the preset's tints (`accent_soft`, `tint`, `on_dark_accent`, `accent_light`)
mixed from it instead.

`"card_style"` is how cards, stats, steps, numbered lists, comparisons,
callouts and clean tables are drawn: `rule` (editorial) or `fill` (swiss,
mono, and decks without a preset).

Fonts are not embedded in the `.pptx` (Kantor doesn't write embedded fonts):
whoever opens a deck needs its fonts installed (Instrument Serif, Inter and
JetBrains Mono are free, under the SIL Open Font License), or the viewer
substitutes another. A deck may name any font in `theme.fonts`; `styles`
change the label font (`"label": {"font": "..."}` and the others that name
JetBrains Mono).

**Text styles.** `"style": "eyebrow"` on an element, a paragraph, a run or a
table cell applies a named style; keys given beside it win. Every deck has
`eyebrow`, `page_number`, `title`, `subtitle`, `body`, `small`, `note`,
`label`, `card_title`, `stat`, `number` (card numbers), `numeral` (step and
list numerals), `badge`, `callout`, `table`, `table_header`, `footer`,
`cover_brand`, `cover_eyebrow`, `cover_title`, `cover_subtitle`,
`cover_meta`, `cover_mark`, `cover_mark_label`, `section_eyebrow`,
`section_title` and `section_subtitle`; change them or add your own in
`styles`:

```json
"styles": {
  "eyebrow": {"size": 11, "color": "muted"},
  "quote": {"font": "heading", "size": 22, "italic": true, "color": "dark", "line_spacing": 1.2}
}
```

A style has character keys (`font`, `size`, `bold`, `italic`, `underline`,
`strike`, `color`, `caps`, `letter_spacing`) and paragraph keys (`align`,
`line_spacing`, `space_before`, `space_after`); on a run only the character
keys apply. `"font": "heading"` / `"body"` are the theme's fonts (they follow
a theme change in PowerPoint).

**Palette.** Wherever a colour goes (`color`, `fill`, `line`, `border`,
`header_fill`, `band_fill`, `background`, gradient stops) a palette name
works too: `accent` (accent 1; also `primary`), `accent_soft` (also
`tint`), `ink` (text), `muted`, `line` (hairlines; also `divider`), `panel`
(card fills; also `paper`), `bar`, `background` (the slide background),
`dark`, `good` / `success`, `bad` / `danger`, `warning`, `highlight`,
`on_accent`, `on_dark`, `on_dark_muted`, `on_dark_accent`, `accent_light`,
`info`, `white`, `black`, `on_primary`. Those that are theme colours are
written as theme colours; the others as RGB. Override them in
`"palette": {"muted": "#5B6676"}`. On a dark section (or cover) slide the
text colours swap: `ink` is `on_dark`, `muted` `on_dark_muted`, `accent` is
`on_dark_accent` (on a dark section, not a cover on the accent itself), the
tones (`highlight`, `warning`, `good`, `bad`, …) are lightened until they
read on the dark at 4.5:1, `on_accent` is the dark background, and rules
and panels are mixed from the dark background, so components work there
unchanged. The presets' accents read at 4.5:1 or more on their paper.

## Slide furniture

With a `header` (every preset has one), a slide takes `eyebrow`, `title` and
`subtitle`, and they go where the header says, one under the other (a long
title takes two lines and pushes the rest down). The title is the slide's
title placeholder, so `slides list`, the outline and screen readers see it as
the title. Bullets go in a text box in the content area.

```json
"header": {"x": "0.75in", "y": "0.52in", "w": "11.8in", "gap": "0.12in", "content_gap": "0.5in",
           "rule": {"color": "accent", "w": "0.35in", "h": "1.5pt", "position": "left"},
           "page_number": false, "separator": " / ", "text": false, "align": "left"}
```

`rule` is a short bar above the eyebrow, or beside it with `"position":
"left"`; `"w": "full"` spans the header's width (swiss: a 3 pt ink rule).
`page_number: true` puts the page number before the eyebrow (`02 / ...`, in
the `page_number` style); `text` puts text at the right of the eyebrow row
(`true`: the footer's `text`, which then isn't repeated in the footer).

`footer` is drawn on every slide but covers and sections:

```json
"footer": {"text": "Example Co", "logo": "logo.png", "logo_h": "0.2in", "page_numbers": true,
           "rule": {"color": "line", "width": "0.75pt"}, "mark": false, "bar": false, "note_field": true}
```

Left to right: the logo, the slide's `"footer_note": "Source: …"`, the
footer's `text`, the page number (`02`, a slide number field; Kantor shows
the number at build time and PowerPoint renumbers it as `2` when slides
move). `rule` is a hairline across the content width above it, `mark` a
small square at the bottom right (`{color, size}`), `bar` a bar along the
bottom edge. `"footer": false` hides the footer on one slide (`true` shows
it on a cover); `"header": false` or `"footer": false` on the deck turns a
preset's off. Its boxes are named `Footer`, `Footer note` and `Slide
number`, and `kantor slides check` treats them as footers (9 pt is fine
there). Text boxes for one line (titles, numbers, numerals, stat values)
are as tall as the font's own line height at their size and line spacing,
so a deck built from components checks clean.

`"kind": "cover"` makes a cover: the footer's `text` at the top left
(`cover.brand`) or the cover logo, the eyebrow (beside or under the cover's
`rule`), a big title, the subtitle and the slide's `meta` line at the
bottom, with the cover's `motif`: `block` (an accent block down the right,
`block_w` wide, carrying the slide's `mark`, a big number, and
`mark_label`) or `grid` (a 4×4 grid of squares). `"kind": "section"` is a
dark slide (`cover.section_background`, default the palette's `dark`) with
the eyebrow, title and subtitle at `section_y` (or centred), and the slide's
components under them, drawn in the light colours. Set them in the deck's
`cover`: `{"background", "x", "y", "w", "rule", "logo", "logo_h", "align",
"motif", "block_w", "brand", "section_background", "section_y"}`. Without a
`y` the text is centred on the slide's height. A slide's own `background`
wins (e.g. a photo).

`content` sets the area under the header that components use when they
have no box (default: 0.75 in margins, from 2.2 in down to 0.85 in above the
bottom), and `gap`, the space between cards:
`"content": {"x": 0.75, "y": 2.2, "w": 11.83, "h": 4.45, "gap": 0.24}`.

## Components

Components are elements that expand into ordinary shapes and text boxes when
the deck is built (nothing special is left in the file). Each takes a box
(`x`, `y`, `w`, `h`; by default the slide's content area, so most need none)
and an optional `name` (prefix of the shapes' names). Text sizes come from the
styles, colours from the palette.

Each is drawn in the deck's `card_style`: `rule` (no boxes, a 1 pt ink rule
on top, 2.5 pt accent on the highlighted one, numbers in the heading font)
or `fill` (square panels, the highlighted one in the accent colour with
white text).

- **stat_cards** — a row of stats: `items: [{value, label, detail,
  highlight}]`.
- **cards** — a grid: `items: [{title, text | bullets, icon}]` (or strings),
  `columns` (default from the count), a number on each (`numbered: false`
  for none; an item's `icon` replaces the number), `highlight: n`, `fill`
  (fill style). Short cards put the number beside the text.
- **steps** — numbered steps down the box: big numerals with hairlines
  between (rule) or square number badges (fill): `items`, `current` (the
  highlighted step, from 1).
- **timeline** — phases across the box: `items: [{label, title, text}]`,
  `current`, `style`: `track` (square markers on a line: done ink, current
  accent, later outlined; the line in the accent up to `current`) or
  `chevrons`.
- **callout** — the one sentence to remember: `tone` (`info`, `warning`,
  `success`, `danger`, `note`), `title` (the lead-in), `text`, `icon`; a
  pull quote beside a rule in the tone's colour (rule) or a solid bar
  (fill); by default across the bottom of the content area, as tall as its
  text.
- **numbered_list** — rows of a number and text centred on each other:
  `items`, `columns`, `marker` (`badge`: a numeral (rule) or a square badge
  (fill); `number`: a big `01`), `boxed` (a hairline above each row, or a
  panel behind it).
- **comparison** — 2 to 4 columns: `columns: [{tag, title, text, items,
  icon, highlight}]`.
- **badge** — a square badge: `text` or `icon`, `x`, `y`, `size`, `fill`,
  `color`, `geometry` (`ellipse` for a circle).
- **table** with `"preset": "clean"` (see Elements).

Numbers are `01`, `02`, ... (`"numbers": "1"` for `1`, `2`; `start`).
Components work in `slides_add_elements` too (with the presentation's own
theme colours and the default styles).

## Transitions

`"transition"` on a slide, or on the deck for every slide: a type (`fade`,
`push`, `wipe`, `split`, `cover`, `uncover`, `zoom`, `cut`, or `none`) or
`{"type", "duration" (seconds, or "700ms"), "direction" (left, right, up,
down: the way the slides move; in / out for split and zoom), "orientation"
(horizontal / vertical, split), "through_black" (fade, cut), "advance_after"
(seconds)}`. Kantor's slide show plays them; a duration is written as
PowerPoint 2010's `p14:dur` with `spd` for older readers.

## Editing an open presentation (MCP)

`slides_list` gives each shape's `id`, `name`, `kind` (shape, text box, line,
picture, table, group, ...), `geometry`, `x`/`y`/`w`/`h` in EMU, `rotation`
and text. Use the ids with `slides_update_shape` (move, resize, turn),
`slides_delete_shape`, `slides_order_shape` (`front`, `back`, `forward`,
`backward`, or a position from 1 at the back), `slides_group_shapes` and
`slides_set_shape_text`; `slides_set_notes` takes `text` or `notes`. Each
call is one undo step in Kantor.

## Checking your deck

A deck can look right in JSON and wrong on the slide: a title that wraps to
a second line runs into the text below it, a box placed past the edge is
cut off, pale text vanishes on a pale card. Two tools show what the slides
really look like, laid out with Kantor's own text layout (the same as the
app, the PDF and the PNGs):

```sh
kantor slides check deck.pptx                  # findings for people; exit 1 on errors
kantor slides check deck.pptx --json           # for programs and agents
kantor slides check deck.pptx --slide 3        # one slide
kantor slides build deck.json -o deck.pptx --check    # build, then check
kantor convert deck.pptx -o shots/ --to png    # shots/deck-01.png, deck-02.png, …
kantor convert deck.pptx -o shots/ --to png --contact-sheet   # shots/deck-sheet.png
kantor convert deck.pptx slide3.png --slide 3 --width 1920
```

`check` reports, per slide and shape (id and name as in `slides_list`):

| check | severity | what |
|---|---|---|
| `text_overflow` | error | text taller (or a line wider) than its box: lines needed, lines that fit, how much spills out |
| `text_wraps` | error | text wrapping in a box made for one line (and spilling out) |
| `title_wraps` | info | a title-like single paragraph (title placeholder, a name with "title", or 24 pt and up) that wraps but fits |
| `autofit_shrink` | warning (error under 60 %) | a shrink-on-overflow box (`"autofit": "shrink"`) whose text doesn't fit, and the scale it needs; the file, PDF and PNG show it at full size until PowerPoint shrinks it |
| `autofit_resize` | info | a box that grows to fit its text (`"autofit": "resize"`): how tall it gets |
| `off_slide` | error / warning / info | entirely off the slide, or its text past the edge (error); its box but not its text past the edge (warning); a shape without text bleeding off (info) |
| `text_overlap` | error | text running into another shape's text |
| `covers_text` | warning | a shape drawn on top of part of a text |
| `text_crosses_edge` | warning | text partly over the shape behind it (text wholly inside a card, picture or band is fine) |
| `low_contrast` | warning (error under 2:1) | text colour against the colour behind it under 4.5:1 (3:1 for 18 pt bold or 24 pt and up); text over pictures isn't checked |
| `font_missing` | warning / info | a font not installed here: text falls back to another font, so line breaks can differ (info when a font with the same metrics stands in, e.g. Carlito for Calibri) |
| `small_text` | warning | text under 10 pt (footers excluded: footer placeholders, and the `Footer`, `Footer note` and `Slide number` boxes a deck `footer` draws along the bottom) |
| `empty_placeholder` | warning | a placeholder with no text (shows a prompt in editors, nothing in the show) |
| `no_title` | warning / info | a slide without a title placeholder (info when big text near the top looks like one: give the slide a `title`) |
| `no_notes` | info | no speaker notes |

The text output lists the findings by slide. `--json` gives
`{"slides", "checked", "size", "errors", "warnings", "info", "findings": [...]}`;
each finding has `severity`, `check`, `slide` (from 1; `null` for the whole
deck), `shape` and `other` (`{"id", "name", "kind"}`), `message`, `rect`
(where the problem is: `x`, `y`, `w`, `h` in EMU and the same in inches
under `in`) and the numbers behind it (`lines`, `lines_that_fit`,
`overflow`/`overflow_in`, `scale_needed`, `ratio`, `color`, `background`,
`font`…). The exit status is 0 without errors, 1 with errors and 2 for
usage errors. The MCP tool `slides_check` returns the same JSON for the
presentation open in Kantor, or for a file (`path`).

`kantor convert … --to png` draws the slides through the PDF export, so the
PNGs show what the PDF shows (pictures, gradients, tables, charts, text).
`--width` sets each picture's width in pixels (default 1600; 480 per slide
in a contact sheet), `--slide` picks slides (`3`, `2-5`, `1,3,7-9`, `4-`;
hidden slides are left out unless named). With `-o DIR` the files are
`DIR/NAME-01.png`…; `INPUT OUTPUT.png` with one slide writes `OUTPUT.png`.
`--contact-sheet` writes one picture of all the slides in a numbered grid.

A good loop for an agent: build, `check --json`, fix the errors, then look
at the contact sheet (or single slides) for what checks can't judge:
balance, alignment, whether a picture fits the story. Not checked yet: text
in tables, groups, charts and SmartArt, and turned (vertical) text's
overflow.
