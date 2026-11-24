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

The full schema is [slides-deck.schema.json](slides-deck.schema.json); a
complete example is [examples/slides/deck.json](examples/slides/deck.json)
(its pictures come from `examples/slides/make_pictures.py`):

```sh
kantor slides build docs/examples/slides/deck.json -o /tmp/example.pptx
kantor convert /tmp/example.pptx /tmp/example.pdf
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
- **table** — `x`, `y`, `w`, `columns` (widths, or a count with `w`), `rows`
  (arrays of cells: text, numbers, `null` under a span, or
  `{text, fill, border, bold, color, align, anchor, col_span, row_span, ...}`),
  `row_height`/`row_heights`, `header` (default true: bold `header_color` on
  `header_fill`), `banded` with `band_fill`, `fill`, `border` (default 1 pt
  grey), `cell_margins`, `style_id`, and text formatting for every cell.
- **chart** — a real PowerPoint chart made from data (see [Charts](#charts)).

### Text

`text` is a string (each line a paragraph) or a list of paragraphs. A
paragraph is a string or
`{"text" | "runs", "align", "level", "bullet", "line_spacing", "space_before", "space_after", ...formatting}`;
a run is a string or `{"text", "font", "size", "bold", "italic", "underline", "strike", "color"}`.
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

## Editing an open presentation (MCP)

`slides_list` gives each shape's `id`, `name`, `kind` (shape, text box, line,
picture, table, group, ...), `geometry`, `x`/`y`/`w`/`h` in EMU, `rotation`
and text. Use the ids with `slides_update_shape` (move, resize, turn),
`slides_delete_shape`, `slides_order_shape` (`front`, `back`, `forward`,
`backward`, or a position from 1 at the back), `slides_group_shapes` and
`slides_set_shape_text`. Each call is one undo step in Kantor.

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
| `small_text` | warning | text under 10 pt (footers excluded) |
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
