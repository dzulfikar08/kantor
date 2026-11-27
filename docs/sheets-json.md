# Workbooks from JSON: `kantor sheets` and the MCP sheets tools

Kantor builds, reads, edits and audits Excel workbooks from the command line,
with no window and without openpyxl. Formulas stay formulas (Kantor's
engine calculates them, and the saved file carries their values), and the
files open in Excel. Presentations and Word documents have the same kind of
commands: [slides-json.md](slides-json.md), [docs-json.md](docs-json.md).

```sh
kantor sheets build book.json -o book.xlsx        # a whole workbook (-f to overwrite)
kantor sheets list book.xlsx                      # sheets, objects, column summaries, a page of cells
kantor sheets list book.xlsx --sheet Data --range A31:H60 --formulas
kantor sheets edit book.xlsx to_numbers '{"range": "Data!B2:B500"}'
kantor sheets edit book.xlsx -o out.xlsx --ops ops.json   # [{"tool": "fill_down", "args": {...}}, ...]
kantor sheets check book.xlsx [--json] [--sheet Data]     # the audit (see "Checking a workbook")
kantor sheets macros book.xlsm [--module Module1] [--json] # the VBA macros: modules, procedures, code
kantor sheets schema > sheets-json.schema.json
kantor convert book.xlsx -o shots/ --to png       # each printed page as a PNG
```

The MCP server (`kantor mcp`) offers the same: `sheets_edit` runs a list of
these tools on the workbook open in Kantor (one undo step), `sheets_list`
and `sheets_check` read the open workbook or a file (`path`), and
`sheets_build` writes a `.xlsx` (`open: true` opens it). `sheets_macros`
reads a workbook's macros (see "Macros"). The tools' JSON is
the same everywhere. The full schema is
[sheets-json.schema.json](sheets-json.schema.json).

## Cells, sheets, colours

- Cells are A1-style: `B7`, `A1:D20`, with a sheet `Data!A1:D20` or
  `'My data'!A1`, whole columns `C:E`, whole rows `3:5`. Whole columns and
  rows are cut to the used area where that makes sense (formatting, sorting).
- `sheet` is a sheet name or its number from 1; without it, the active
  sheet. A reference that names a sheet wins.
- Columns in `columns`, `widths` and sort keys are letters (`"C"`, `"A:F"`,
  `["A", "C:D"]`); rows are numbers from 1 or spans (`"5:9"`).
- Colours are `#RRGGBB` (`none`/`auto` remove a fill or font colour).
- Values are typed as a user types them: numbers, `true`/`false`, text,
  formulas starting with `=`, dates such as `"2026-01-15"`. Text that would
  read as a number stays text with a leading apostrophe (`"'00123"`).
- Number formats are Excel codes, written as Excel stores them whatever the
  language (`"#,##0.00"`, `"0.0%"`, `"\"Rp\"#,##0"`, `"dd/mm/yyyy"`, `"@"`), or
  presets: `general`, `number` (#,##0.00), `integer` (#,##0), `decimal1`,
  `percent` (0.0%), `percent0`, `percent2`, `rupiah`, `dollar`, `euro`,
  `date` (yyyy-mm-dd), `datetime`, `time`, `text`.

## Building a workbook

```json
{
  "sheets": [
    {"name": "Data",
     "rows": [["Month", "Region", "Units", "Price", "Revenue"],
              ["2026-01-01", "North", 120, 25000, "=C2*D2"],
              ["2026-01-01", "South", 95, 25000, "=C3*D3"]],
     "formats": [{"range": "A1:E1", "bold": true, "fill": "#1F4E79", "font_color": "#FFFFFF"},
                 {"range": "D:E", "number_format": "rupiah"}],
     "widths": {"A": 12, "B": 14, "C:E": 12},
     "freeze": "A2",
     "tables": [{"range": "A1:E3", "name": "Sales", "style": "TableStyleMedium2", "totals": {"Revenue": "sum"}}]},
    {"name": "Dashboard",
     "cells": {"A1": "Sales dashboard", "A3": "Revenue", "B3": "=SUM(Data!E2:E3)"},
     "formats": [{"range": "A1", "size": 18, "bold": true}, {"range": "B3", "number_format": "rupiah"}],
     "charts": [{"chart": "column", "data": "Data!B1:C3", "at": "A5", "to": "H20", "title": "Units by region",
                 "data_labels": true, "legend": "none"}],
     "page_setup": {"landscape": true, "fit": "page"}}
  ],
  "names": [{"name": "Regions", "refers_to": "Data!$B$2:$B$3"}],
  "active": "Dashboard"
}
```

| Key | Meaning |
|---|---|
| `sheets` | the sheets in tab order (required) |
| `names` | defined names `{name, refers_to, sheet}` of cells (a `sheet` makes it sheet-level), made first so formulas can use them. Names of constants (`0.11`) or formulas aren't supported: name a cell holding the value |
| `active` | the sheet shown when the file opens |
| `ops` | editing tools `{tool, args}` run after all sheets are made |
| `calc` | `manual` for manual calculation (default automatic) |

A sheet:

| Key | Meaning |
|---|---|
| `name` | its name (default `Sheet1`, ...) |
| `rows` / `columns` | the data, row by row or column by column, from `start` (default `A1`) |
| `csv` | a CSV/TSV file next to the JSON whose rows are written from `start` (numbers and dates typed, as `kantor convert` reads it), instead of `rows`: the user's data copied without retyping |
| `cells` | single cells: `{"B9": "=SUM(B2:B8)"}` |
| `formats` | `format` tool arguments, each with its `range` |
| `widths` | column widths in characters: `{"A": 14, "B:D": 10, "E": "auto"}` |
| `autofit` | `true` (every used column) or columns to fit to their contents |
| `heights` | row heights in points: `{"1": 30}` |
| `freeze` | the first cell that scrolls (`"B2"`), or `{rows, columns}` |
| `tables`, `charts`, `conditional_formats`, `validations` | `add_table`, `add_chart`, `add_conditional_format`, `add_validation` arguments |
| `pivots` | `add_pivot` arguments; `at` is a cell of this sheet |
| `page_setup`, `gridlines` | `page_setup` arguments; gridlines in print and PNG |
| `show_gridlines` | `false` hides the gridlines on screen (Excel's View > Gridlines), e.g. on a dashboard sheet; `gridlines` is about printing |
| `hidden`, `tab_color` | hide the sheet, colour its tab |
| `ops` | any editing tools, run on this sheet last |

The parts are made in this order: data, formats, widths, heights, tables,
conditional formats, validation, charts, pivot tables, frozen panes, page
setup, ops. Every sheet exists before any is filled, so formulas may refer
to later sheets. The same JSON always gives a byte-identical file.

## Editing tools

`kantor sheets edit FILE TOOL ARGS` (TOOL without or with `sheets_`), or a
list with `--ops`. Each prints a JSON line with what it did; writes show the
written cells' values and formula errors right away.

| Tool | Arguments | What it does |
|---|---|---|
| `write_range` | `top_left`, `values` (rows) | write values and formulas |
| `write_columns` | `top_left`, `columns` | the same, column by column |
| `clear` | `range` | clear contents (formats stay) |
| `fill_down` / `fill_right` | `range`, `formula` | copy the first row (column) through the range, references adjusting; `formula` is written first. A single cell fills down to the last row of the data beside it |
| `format` | `range`, `bold`, `italic`, `underline`, `strikethrough`, `font`, `size`, `font_size_delta`, `font_color`, `fill`, `align`, `valign`, `wrap`, `number_format`, `indent`, `rotation`, `borders`, `merge` | format cells. `borders`: `all`, `outline`, `inside`, `top`, `bottom`, `left`, `right`, `inside_horizontal`, `inside_vertical`, `none`, or `{edges, style, color}` (style `thin`, `medium`, `thick`, `double`, `dotted`, `dashed`, `hair`...), or a list |
| `column_width` | `columns` + `width`, or `widths` | widths in characters, or `"auto"` |
| `autofit` | `columns`, `min`, `max` | fit columns to their text (estimated) |
| `row_height` | `rows`, `height` | heights in points |
| `freeze` | `cell`, or `rows`/`columns` | freeze panes (`A1` unfreezes) |
| `sheet_view` | `show_gridlines` | `false` hides the on-screen gridlines (`true` shows them); printing them is `page_setup`'s `gridlines` |
| `insert_rows`, `delete_rows`, `insert_columns`, `delete_columns` | `before_row`/`first_row`/`last_row`, `before_column`/... , `count` | formulas follow |
| `hide` | `rows` and/or `columns`, `hidden` | hide or show |
| `add_sheet`, `delete_sheet`, `rename_sheet`, `move_sheet`, `copy_sheet`, `hide_sheet` | `name`, `sheet`, `position` | sheets |
| `sort` | `range`, `by` (`"C"`, `{column, order}` or a list), `descending`, `header` | sort rows; the header row stays |
| `remove_duplicates` | `range`, `columns`, `header` | delete repeated rows (case and outer spaces ignored), keep the first |
| `trim` | `range`, `collapse` | TRIM + CLEAN text cells (non-breaking spaces too) |
| `to_numbers` | `range`, `decimal` (`.`, `,`, `auto`), `number_format` | numbers stored as text to numbers: `1,250.50`, `1.250,50`, `Rp 12.000`, `(500)`, `500-`, `15%`. `number_format` is set on the whole range, numbers that were already numbers too, so the column ends up alike |
| `to_dates` | `range`, `order` (`dmy`, `mdy`, `ymd`, `auto`), `number_format` | dates stored as text to dates: `15/01/2026`, `2026-01-15`, `15 Jan 2026`, `15 Januari 2026`, `Jan 15, 2026`, with times. The date format (`number_format`, default `yyyy-mm-dd`) goes on the whole range when given, else on the converted cells and the range's dates that were already dates |
| `split_column` | `range` (one column), `delimiter`, `to`, `overwrite` | text to columns |
| `combine_columns` | `range`, `to`, `separator`, `formulas` | join columns, as formulas (`=A2&" "&B2`) by default |
| `find_replace` | `find`, `replace`, `range`/`sheet`/`all_sheets`, `match_case`, `whole_cell`, `in_formulas` | replace text |
| `add_table` | `range`, `name`, `style`, `header`, `totals`, `banded_rows`, `first_column`, `last_column`, `filter_buttons` | an Excel table; `totals`: `true` (a total row, the last column totalled) or `{"Revenue": "sum", "Units": "average"}` (only those columns; the first column says "Total") |
| `add_chart` | see below | a chart of cells |
| `add_conditional_format` | `range`, `type`, ... | see below |
| `clear_conditional_formats` | `range` (or the sheet) | remove rules |
| `add_validation` | `range`, `type`, `values`/`source`, `operator`, `value`, `value2`, `formula`, `allow_blank`, `dropdown`, `prompt`, `error`, `error_style` | data validation: `list` (a drop-down of `values` or the cells of `source`), `whole`, `decimal`, `date`, `time`, `text_length`, `custom` |
| `clear_validation` | `range` | |
| `define_name`, `delete_name` | `name`, `refers_to` (cells), `sheet` | defined names |
| `add_pivot` | `source` (range or table), `at` or `new_sheet`, `rows`, `columns`, `filters`, `values` (`[{field, func, name, number_format}]`), `layout`, `grand_totals`, `style` | a pivot table Excel refreshes |
| `page_setup` | `landscape`, `paper`, `fit` (`width`, `page`, `[w, h]`), `scale`, `print_area`, `title_rows`, `gridlines`, `center` | printing, PDF and PNG |
| `recalculate` | | recalculate every formula |

Conditional formats (`type`): `data_bar` (`color`, `gradient`,
`show_value`), `color_scale` (`colors`: `red_yellow_green`,
`green_yellow_red`, `red_white_green`, `green_white_red`, `red_white_blue`,
`blue_white_red`, `white_green`, `white_red`, or 2-3 colours, lowest
first), `icon_set` (`icons`: `arrows`, `triangles`, `traffic_lights`,
`signs`, `symbols`, `flags`, `stars`, `ratings`...; `reverse`,
`show_value`), `cell_value` (`operator` `>`, `<`, `>=`, `<=`, `=`, `!=`,
`between`, `not_between`; `value`, `value2`), `text` (`operator`
`contains`, `not_contains`, `begins_with`, `ends_with`, `equals`; `text`),
`top`/`bottom` (`rank`, `percent`), `above_average`, `below_average`,
`duplicates`, `unique`, `blanks`, `not_blanks`, `formula` (`=$D2>100`,
written for the first cell), `dates` (`period`: `today`, `last_7_days`,
`this_month`...). Rules that colour cells take `format`: `red` (the
default), `yellow`, `green`, `red_fill`, `red_text`, `bold`, or `{fill,
font_color, bold, italic, underline, strike}`.

## Charts

A chart takes the slides charts' keys (`chart`, `stacked`, `title`,
`legend`, `data_labels`, `number_format`, `axis`, `category_axis`,
`gap_width`, `overlap`, `hole`, `font`, `colors`, `target_line`; see
[slides-json.md](slides-json.md#charts)) with cells where slides take
values. Its series refer to the cells, so Excel and Kantor redraw it when
they change. Without `number_format` (or `axis.number_format`,
`data_labels.number_format`) the value axis and labels show the cells' own
format; with it they show that format whatever the cells' (`"#,##0"`,
`"\"Rp\"#,##0.0,,"` for millions, `"0.0%"`).

```json
{"chart": "column", "data": "Data!A1:C13", "at": "E2", "to": "L18",
 "title": "Revenue by month", "number_format": "#,##0", "target_line": 50000}

{"chart": "combo", "categories": "Data!A2:A13",
 "series": [{"name": "=Data!B1", "values": "Data!B2:B13"},
            {"name": "Margin", "values": "Data!D2:D13", "type": "line", "axis": "secondary", "color": "#C00000"}],
 "at": "E20", "w": "6in", "h": "3.5in"}
```

- `data` is a block: series names along its top row, categories down its
  first column (text or dates; a scatter chart's x values), one series per
  column (`series_in: "rows"` for one per row). `series` beside `data`
  only style the block's series (`color`, `type`, `axis`, `data_labels`...).
- Or `categories` (a range, or a list) and `series`, each with `values` (a
  range, or numbers) and `name` (text, or `=Data!B1`).
- Types: `column`, `bar`, `line`, `area`, `pie`, `doughnut`, `scatter`,
  `combo`; `stacked`: `true` or `"percent"`.
- Placement: `at` (top-left cell; default two columns right of `data`) and
  `to` (the bottom-right cell it covers), or `w` and `h` (`"6in"`,
  `"480px"`, or numbers in `units`); default 8 columns × 15 rows. A
  dashboard is a sheet of KPI cells with charts placed by `at`/`to`.

`kantor sheets list` shows each chart's type, title, cells and its series'
`name`, `categories` and `values` references.

## Reading a workbook

`kantor sheets list FILE` prints, per sheet: `used_range`, `rows`,
`columns`, `hidden`, `frozen`, `merged`, `hidden_rows`, `hidden_columns`,
`column_widths`, `tables`, `charts`, `conditional_formats`, `validations`,
`pivots`, a `column_summary` (per column: `header`, counts of `numbers`,
`dates`, `text`, `formulas`, `errors`, `blank`, `min`, `max`, the main
`number_format`), and `cells`: `range`, `values` row by row (numbers,
text, booleans, errors as `"#DIV/0!"`, null for empty; trailing empty cells
left out), `formulas` (`{"E2": "=C2*D2"}`) and `number_formats` (runs down
columns, `{"D2:D31": "#,##0"}`). Only 30 rows are shown unless `--max-rows`
(up to 5000; 0 for summaries only); a `--range` shows the whole range up to
500 rows. When rows are left out, `cells` says so: `"truncated": true`,
`next` (the rest, for `--range`) and a `note`. `--formulas` puts formulas in the grid. Defined names are in
`names`.

## Checking a workbook

```sh
kantor sheets check book.xlsx              # findings for people; exit 1 on errors
kantor sheets check book.xlsx --json       # for programs and agents
kantor sheets build book.json -o book.xlsx --check
```

Each finding has a `severity`, `check`, `sheet`, `range`, `message` and the
facts behind it (`cells`, `example`, `typed`, `sum`, `fix`...). Cells with
the same problem down a column are one finding with a range. `--json`
gives `{"sheets", "cells_checked", "errors", "warnings", "info",
"findings": [...]}`; the exit status is 1 when there are errors.

| check | severity | what |
|---|---|---|
| `formula_error` | error (warning for #N/A) | a formula shows #REF!, #DIV/0!, #VALUE!, #NAME?, #NUM!, #NULL!, #SPILL! or #CALC! (#N/A: often a lookup that found nothing) |
| `circular_reference` | error | a formula depends on its own value |
| `error_value` | warning | an error typed as a value |
| `number_as_text` | warning | numbers stored as text (`"1,250.50"`, `"Rp 12.000"`); codes with leading zeros, phone numbers (`+62…`) and digit strings over 15 long are left alone. Fix: `to_numbers` |
| `date_as_text` | warning | dates stored as text. Fix: `to_dates` |
| `inconsistent_formula` | warning | a formula that differs (in R1C1 form) from the formulas above and below, or left and right, which agree with each other |
| `hardcoded_number` | warning | a number typed as an operand next to a reference (`=B2*1.11`, `=C5+500`); 0, 0.5, 1, 2, 10, 12, 24, 60, 100, 1000, 365, comparisons and function arguments (`ROUND(x, 2)`) are left alone. Put the number in a cell or a name |
| `typed_total` | error / warning | a row or column labelled Total, Grand total, Subtotal, Jumlah… holding a typed number below (beside) at least two numbers: error when it isn't their sum, warning when it is (it won't follow changes). The numbers are the column's (row's) whole block up to its header, a subtotal, another total or an empty row, blank cells and numbers stored as text included (`block`). Numbers stored as text are named (`numbers_as_text`, `text_cells`, `sum_with_text`): SUM skips them, so convert them first. `fix` gives the SUM |
| `sum_misses_rows` | error / warning | `=SUM(...)` under (beside) a block of numbers (found as for `typed_total`) that leaves some of it out: error when the cells left out hold numbers (the total is wrong), warning when they are only numbers stored as text |
| `refers_to_empty` | warning | a formula using an empty cell on its own (not in a range), unless it tests for blanks (IF, ISBLANK) |
| `mixed_formats` | warning | a column of 5+ numbers in different kinds of format (plain and percent, two currencies, dates and numbers), or of 3+ dates in different date formats (`yyyy-mm-dd` next to `dd/mm/yyyy`). Fix: `format` the column with one `number_format` |
| `mixed_units` | warning | a column with values like `12 kg` and `3 lb` |
| `duplicate_rows` | warning | rows of a data block (3+ rows, 2+ columns) repeating an earlier row. Fix: `remove_duplicates` |
| `extra_spaces` | warning | text with leading, trailing, doubled or non-breaking spaces. Fix: `trim` |
| `merged_in_data` | warning | merged cells below a data block's first row, or in a table |
| `volatile_function` | warning | NOW, TODAY, RAND, RANDBETWEEN, OFFSET, INDIRECT, CELL or INFO in a workbook of 1000+ formulas |
| `external_link` | warning | a formula or name referring to another workbook |
| `broken_name` | error | a defined name referring to #REF! |
| `hidden_data` | info | a hidden sheet, hidden rows or hidden columns holding data |
| `text_cut_off` | warning | text that doesn't fit where it's shown. Each cell's text is placed as the PDF and PNG (`kantor convert`) draw it and measured with the same fonts: wider than its cell when the cell beside it isn't empty (so it can't spill), wider than its merged cells (merged cells never spill), cut at the printed page's edge (`page_edge`), or taller than its row. `need_pt` and `room_pt` give the sizes. Fix: widen the columns, shorten or wrap the text, make the row taller, or fit the sheet to the page width |
| `number_hashes` | warning | numbers or dates wider than their column, shown as `####`; `width_chars` is about the width the column needs |
| `number_rounded` | info | General numbers wider than their column that show with fewer decimals or in scientific notation (as in Excel), so the cell shows less than it holds: `value` is the stored number, `shown` the text shown, `full` its full General text, `width_chars` about the width the column needs to show it in full. Numbers that show shorter without losing anything (`1E+12`) aren't reported. Fix: widen the column, or `format` it with a `number_format` giving the decimals you want |
| `outside_print_area` | warning | filled cells, charts or pictures outside the print area (they don't print, or print cut) |
| `split_by_page` | warning | a chart or picture split over two or more printed pages by a page break. Fix: `page_setup` `fit` (`width` or `page`), or move it |
| `unsorted_bars` | info | a bar or column chart of one series over 4+ text categories whose values aren't sorted: sort the categories by value so the ranking reads at a glance. Months, days, quarters, years, numbers and other ordered labels (Low/Medium/High, S/M/L), several series and line charts are left alone; an "Other" category at the end doesn't count |

The loop for an agent: `list` to understand the workbook, `check --json`,
fix with `edit` (`to_numbers`, `to_dates`, `trim`, `remove_duplicates`,
`write_range` for formulas), `check` again, then look at the result with
`kantor convert book.xlsx -o shots/ --to png`.

## Pictures of a workbook

`kantor convert book.xlsx -o shots/ --to png` draws each printed page (as
the PDF export prints it: every visible sheet, or `--sheet`) as
`shots/book-01.png`, `-02`…; `--contact-sheet` puts the pages in one
picture, `--page 2-4` draws only those pages, `--width` sets the width in
pixels. The page setup options
(`--fit-page`, `--landscape`, `--paper`...) apply; for a dashboard,
`page_setup` with `"fit": "page"` and `"landscape": true` makes one page.

## Not supported yet

- Sparklines, slicers, shapes and pictures can't be added (those in a file
  are kept and listed as `other_objects`).
- Charts: no 3-D, radar, stock, bubble or Excel 2016 types (waterfall,
  treemap...), trendlines or error bars; a chart's series come from one
  sheet's cells or literal numbers.
- `autofit` estimates widths from the text (character counts), not from
  the fonts' glyphs; row heights aren't fitted.
- Rich text in cells, comments/notes, protection and external links can't
  be written.
- The checks look at values and formulas as they are: they don't follow
  INDIRECT/OFFSET, and `refers_to_empty` ignores ranges.
- `text_cut_off`, `number_hashes` and `number_rounded` leave conditional formatting out (a
  rule's bold or number format), look at the first 60 pages of a sheet,
  and skip sheets of more than 50,000 filled cells (as do
  `outside_print_area` and `split_by_page`).

## Macros

Kantor keeps a workbook's VBA project when it saves an `.xlsm` and shows its
code (View ▸ Macros, Alt+F8); it runs a macro only when the user asks (Run
in the Macros dialog after Enable Content, a button, or `kantor sheets
macros run FILE --macro NAME`, see docs/scripting.md). The AI tools never run
macros. `kantor sheets macros FILE` lists the modules (standard, class, document such as `ThisWorkbook`
and `Sheet1`, UserForm) and their `Sub`s, `Function`s and `Property`
procedures with line numbers; `--module NAME` prints a module's code
(attribute lines left out, as the Visual Basic Editor shows it); `--json`
gives the listing as JSON:

```json
{"has_macros": true, "project": "VBAProject", "codepage": 1252,
 "references": [{"name": "stdole", "kind": "library", "libid": "*\\G{00020430-…}#2.0#0#…#OLE Automation"}],
 "modules": [{"name": "Module1", "kind": "standard", "lines": 12,
              "procedures": [{"name": "TestMacro", "kind": "Sub", "private": false,
                              "arguments": false, "macro": true, "line": 1, "end_line": 12}]}],
 "note": "AI tools don't run macros: … "}
```

`macro` marks what Excel's Macros dialog lists (a public `Sub` without
arguments). The MCP tool `sheets_macros` takes the same `module`, and
`code: true` for every module's code, on the open workbook or a `path`;
Paparan AI's `macros` tool does the same for files in its folder. Use it
to explain what a macro does, or to do its work with formulas and the
editing tools instead.
