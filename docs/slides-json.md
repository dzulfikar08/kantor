# Slides from JSON: `kantor slides` and the MCP slides tools

Kantor builds and edits presentations from JSON, with no window and without
python-pptx:

```sh
kantor slides build deck.json -o deck.pptx     # a whole deck (-f to overwrite)
kantor slides list deck.pptx                   # slides and shapes as JSON
kantor slides edit deck.pptx add_shape '{"slide": 2, "type": "shape", "x": "1in", "y": "1in", "w": "3in", "h": "1in", "text": "Hello"}'
kantor slides edit deck.pptx -o out.pptx --ops ops.json   # [{"tool": "add_table", "args": {...}}, ...]
kantor slides schema > slides-deck.schema.json
```

The MCP server (`kantor mcp`) offers the same as tools on the presentation
open in Kantor: `slides_add_shape`, `slides_add_picture`, `slides_add_table`,
`slides_add_elements`, `slides_set_background`, `slides_update_shape`,
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
  "preset": "consulting-navy",
  "footer": {"text": "Prepared for Example Co", "logo": "logo.png"},
  "transition": "fade",
  "slides": [
    {"kind": "cover", "eyebrow": "Proposal · November 2026", "title": "Reliable uptime for every line"},
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
  `"preset": "clean"` gives the consultancy look: a rounded header bar in
  the primary colour behind a borderless header row (small capitals), soft
  banded rows, no grid lines, the first column bold (`first_bold: false`
  to turn off); `x`, `y`, `w` default to the content area. It stays a real
  table (the header bar is a shape behind it).

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
  (`content`, `cover`, `section`), `footer`, `footer_note` and `transition`
  (below); a slide with its own `layout` keeps the layout's placeholders.
- Deck keys for the look: `preset`, `palette`, `styles`, `header`, `footer`,
  `cover`, `content`, `transition` (below).

## Presets, styles and the palette

`"preset"` picks a built-in look: `consulting-navy` (navy and amber,
Calibri), `warm` (terracotta and mustard, Georgia headings, Calibri body),
`mono` (black, white and one red, Arial) or `teal` (teal and amber,
Calibri). A preset sets the theme colours and fonts, the palette, the text
styles, a header (eyebrow, title, subtitle), a footer (bottom bar, slide
numbers) and cover slides; the deck's own `theme`, `palette`, `styles`,
`header`, `footer` and `cover` go over it key by key.

Fonts are not embedded in the `.pptx` (Kantor doesn't write embedded fonts),
so the presets use fonts PowerPoint has everywhere. On Linux, Kantor and
LibreOffice show Calibri with its metric twin Carlito. A deck may name any
font in `theme.fonts`; whoever opens it needs that font installed.

**Text styles.** `"style": "eyebrow"` on an element, a paragraph, a run or a
table cell applies a named style; keys given beside it win. Every deck has
`eyebrow`, `title`, `subtitle`, `body`, `small`, `note`, `label`,
`card_title`, `stat`, `number`, `badge`, `callout`, `table`,
`table_header`, `footer`, `cover_eyebrow`, `cover_title` and
`cover_subtitle`; change them or add your own in `styles`:

```json
"styles": {
  "eyebrow": {"size": 11, "bold": true, "caps": true, "letter_spacing": 1, "color": "primary"},
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
works too: `primary` (accent 1), `highlight` (accent 2), `ink` (text), `dark`
(dark 2), `paper` (light 2), `white`, `muted`, `divider`, `tint`,
`accent_light`, `on_dark_muted`, `info`, `success`, `warning`, `danger`,
`black`, `on_primary`. Those that are theme colours are written as theme
colours; the mixed ones (`muted`, `divider`, `tint`, ...) as RGB. Override
them in `"palette": {"muted": "#5B6676"}`.

## Slide furniture

With a `header` (every preset has one), a slide takes `eyebrow`, `title` and
`subtitle`, and they go where the header says, one under the other (a long
title takes two lines and pushes the rest down). The title is the slide's
title placeholder, so `slides list`, the outline and screen readers see it as
the title. Bullets go in a text box in the content area.

```json
"header": {"x": "0.75in", "y": "0.55in", "w": "11.8in", "gap": "0.06in", "content_gap": "0.4in",
           "rule": {"color": "highlight", "w": "0.9in", "h": "0.06in"}, "align": "left"}
```

`footer` is drawn on every slide but covers and sections:

```json
"footer": {"text": "Prepared for Example Co", "logo": "logo.png", "logo_h": "0.2in",
           "page_numbers": true, "bar": {"color": "primary", "height": "0.08in"}, "note_field": true}
```

A slide's `"footer_note": "Source: …"` goes in the footer, right-aligned;
`"footer": false` hides the footer on one slide (`true` shows it on a
cover). Slide numbers are slide number fields. `"header": false` or
`"footer": false` on the deck turns a preset's off.

`"kind": "cover"` (or `"section"`) makes a cover: a dark background (the
palette's `dark`; `primary` for sections), an accent rule, then the
eyebrow, a big title and subtitle centred on the slide's height, and the
cover logo. Set them in the deck's `cover`:
`{"background", "x", "w", "rule", "logo", "logo_h", "align"}`. A slide's own
`background` wins (e.g. a photo).

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

- **stat_cards** — a row of cards: `items: [{value, label, detail,
  highlight}]` (one highlighted card is filled with the primary colour).
- **cards** — a grid: `items: [{title, text | bullets, icon}]` (or strings),
  `columns` (default from the count), a numbered badge on each (`numbered:
  false` for none; an item's `icon` replaces the number), `highlight: n`,
  `fill`. Short cards put the badge beside the text.
- **steps** — numbered steps down the box joined by a line: `items`,
  `current` (steps up to it are filled).
- **timeline** — phases across the box: `items: [{label, title, text,
  badge}]`, `current`, `style`: `dots` (dots on a line, progress up to
  `current`) or `chevrons`.
- **callout** — a tinted box with a bold lead-in: `tone` (`info`, `warning`,
  `success`, `danger`, `note`), `title`, `text`, `icon`; by default across the
  bottom of the content area, as tall as its text.
- **numbered_list** — rows of a number and text centred on each other:
  `items`, `columns`, `marker` (`badge` or a big `number`), `boxed`.
- **comparison** — 2 to 4 columns: `columns: [{tag, title, text, items,
  icon, highlight}]`.
- **badge** — a round badge: `text` or `icon`, `x`, `y`, `size`, `fill`,
  `color`.
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
