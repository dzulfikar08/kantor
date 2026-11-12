# Changelog

All notable changes to Kantor are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/) (before 1.0, minor versions may break things).

The release workflow (`.github/workflows/release.yml`) uses the section for a tag as
that GitHub Release's notes, so every `v*` tag needs a `## [x.y.z]` heading here.

## [Unreleased]

### Added

#### Windows (new platform, x64 and Arm)
- Kantor builds and runs on **Windows 10 and 11**. The GUI uses GPUI's DirectX backend, Segoe UI
  is the default interface font (Cascadia Mono or Consolas for monospace), and keyboard shortcuts
  use Ctrl as on Linux.
- **Installer** `Kantor-Setup-x64.exe` / `Kantor-Setup-arm64.exe` (Inno Setup): installs per user
  without an administrator prompt and adds a Start menu entry, an uninstall entry, **Open with**
  entries for .xlsx/.xlsm/.xls/.ods/.csv/.docx/.pptx (your default apps stay as they are) and an
  optional PATH entry. A **portable zip** `Kantor-x64.zip` / `Kantor-arm64.zip` is also built.
  Both are unsigned for now (SmartScreen asks once); signing turns on when a certificate secret
  is configured, see docs/windows-signing.md.
- **Updates on Windows:** the in-app updater downloads the new installer, checks it against the
  signed manifest and runs it silently once Kantor quits, then reopens Kantor after *Restart to
  update*. Portable copies replace their own files. The update manifests gain the
  `windows-{x86_64,aarch64}-{setup,portable}` assets.
- **Command line on Windows:** `kantor.com`, a console front shipped next to the windowed
  `kantor.exe`, so `kantor convert`, `kantor pdf`, `kantor mcp` and `kantor --version` print
  to the terminal.
- **MCP on Windows:** the app listens on 127.0.0.1 behind a per-run token kept in the user's
  profile, with the same protocol as the Unix socket. AI keys go to Windows Credential Manager.
  Settings live in `%APPDATA%\Kantor`, the session and update state in `%LOCALAPPDATA%\Kantor`.
- CI runs clippy and every test on windows-latest, and the nightly build makes Windows x64 and
  arm64 installers and smoke-tests them on the runner (silent install, `--version`, convert,
  MCP against the running app, silent uninstall).

#### All apps
- **Recovery copies:** every document with unsaved changes gets a copy in your data folder
  (`~/.local/share/kantor/recovery` on Linux, `~/Library/Application Support/Kantor/recovery` on
  macOS, `%LOCALAPPDATA%\Kantor\recovery` on Windows) two minutes after its first unsaved change
  (at a pause in typing) and whenever the window loses focus. The window only takes a snapshot
  (≈50–150 ms for a 90,000-row workbook); the copy is built and written in the background,
  atomically. Copies are removed when you save, close with *Don't Save*, or quit normally.
- **Recover unsaved work:** after a crash, power cut or killed process, the next Kantor lists the
  documents it kept, with when they were last copied. *Restore* opens a copy as the original
  document with its unsaved changes; *Save* writes to the original file, asking first if that file
  changed on disk since. Nothing is written to the original until you save. *Discard* deletes the
  copy. *Ctrl K → Recover Unsaved Work…* shows the list again after *Not Now*.
- **Autosave** (off by default): `Ctrl K → Turn Autosave On or Off`, the *Autosave* switch in the
  status bar, or `[autosave] enabled = true` in `settings.toml` (`idle_seconds`, default 30). It
  saves `.xlsx`, `.docx` and `.pptx` files that already have a name, after you stop typing;
  workbooks are written in the background. Imported files (`.csv`, `.ods`, `.xls`, `.xlsb`), files
  with content Kantor can't keep, and files changed on disk by someone else are never autosaved;
  they still get recovery copies.

#### Sheets
- **Page Setup…** (print menu in the toolbar, palette) for the active sheet, with Excel's four
  tabs: paper (A4, Letter, Legal, F4/Folio, A3, A5, B5, Executive, Tabloid), orientation, scale or
  fit to N pages wide × M tall, first page number, page order; margins in centimetres with
  Normal/Wide/Narrow presets, centre horizontally/vertically; header and footer presets or your
  own left/centre/right text with Excel's codes (`&P`, `&N`, `&D`, `&T`, `&F`, `&A`); print area
  (from the selection or typed, several ranges allowed), rows and columns to repeat, gridlines,
  row and column headings, black and white. **Set Print Area** / **Clear Print Area** work on the
  selection directly. One undo step each; saved as `pageSetup` (other attributes such as the
  printer settings link are kept), `pageMargins`, `printOptions`, `headerFooter`, `fitToPage` and
  the `_xlnm.Print_Area` / `_xlnm.Print_Titles` names; the print area and titles follow inserted
  and deleted rows and columns. Export as PDF and Print use the new settings. Sheets you didn't
  change keep the file's own markup byte for byte.
- **Format as Table** (Ctrl+T or Ctrl+L, Table menu in the toolbar, palette): the selection or
  the data around the active cell, "My table has headers" (without, a header row with Column1,
  Column2… goes in the empty row above, else a new row), and a gallery of the built-in Light,
  Medium and Dark styles drawn as the grid draws them. Blank or repeated header names are made
  unique as in Excel; overlapping a table, a PivotTable, merged cells or the sheet's filter is
  refused. The filter buttons are on.
- **Table Design** panel beside the grid for the table at the active cell: rename (structured
  references in formulas follow), style, Header Row, Total Row (uses the empty row below or
  inserts one; "Total" label and a SUM, or COUNT for text, in the last column), Banded
  Rows/Columns, First/Last Column, Filter Button, a total row function per column (None, Sum,
  Average, Count, Count Numbers, Max, Min, StdDev, Var, written as `SUBTOTAL(109,Table1[Col])`
  etc. as Excel does), and **Convert to Range** (structured references become A1 references).
  Each change is one undo step. New tables are saved as table parts with their relationship,
  `<tableParts>` entry and content type; tables changed in Kantor are rewritten from the model,
  keeping their id and their columns' calculated column formulas; other tables stay byte for
  byte.
- **Frozen rows stay put** while the sheet scrolls, like frozen columns already did: the frozen rows
  (and the top-left corner when rows and columns are both frozen) are pinned under the column
  letters. Everything drawn on the grid follows: the selection and fill handle, the in-cell editor
  (clipped to its pane), formula reference outlines, merged cells crossing the split (each pane shows
  its part), data bars and icons, filter buttons, drop-down lists, and charts and pictures, which
  are drawn in up to four panes and clipped to each. Arrow keys, Ctrl+arrows, Page Up/Down and
  Find scroll the cell into view below the frozen rows; the wheel scrolls the rest even over the
  frozen rows.
- **Hide Rows / Unhide Rows / Hide Columns / Unhide Columns** in the row and column header menus,
  a new View (snowflake) menu in the toolbar with Excel's freeze choices, and the palette;
  Ctrl+9 / Ctrl+Shift+9 / Ctrl+0 / Ctrl+Shift+0 as in Excel. One undo step each, saved as hidden
  rows and columns; unhidden rows and columns come back at their old height and width. Headers
  show Excel's double line where rows or columns are hidden; double-click it (or drag it) to show
  them again. As in Excel, Unhide Rows leaves rows that a filter hides alone (it says how many);
  clear or change the filter to show them.

### Changed
- Sheets' `.xlsx` writer is shared by Save, autosave and recovery copies (no change to the files
  written).

#### Sheets
- Freeze Panes, Freeze Top Row, Freeze First Column and Unfreeze are no longer undo steps, as in
  Excel; they still mark the workbook as changed.

### Fixed

#### Sheets
- Total row labels and functions of tables (`totalsRowLabel`, `totalsRowFunction`) were not read
  from files, and a column's data format id was read from its header's (vendored IronCalc).
- Print titles made of both rows and columns (`$A:$A,$1:$2`) lost their second part when Kantor
  saved the file (IronCalc keeps one part of a union); the whole name is written again.
- Hidden columns from a file were drawn 24 px wide; they now take no room, and arrow keys step
  over hidden rows and columns.

## [0.3.1] - 2026-11-11

Pivot tables can now be made in Kantor: Insert PivotTable…, a PivotTable Fields panel (also for
pivots that came in a file) and Delete PivotTable. Pivots made in Kantor have not yet been opened
in Excel; they are saved with `refreshOnLoad` so Excel rebuilds them from their source.

### Added

#### Sheets
- **Insert PivotTable…** (toolbar PivotTable menu, command palette): the source is the Excel table
  the active cell is in, else the selection, else the data around the active cell, shown as an
  editable reference (`Sheet1!$A$1:$D$13` or a table name) and checked as you type (a name in every
  header cell, each one different, rows under it). The pivot goes on a new sheet (`Sheet2`…, at
  `A3`, as in Excel) or at a cell you type. It works in any workbook: one with no pivots yet, a
  new one, or a `.csv`/`.ods`/`.xls` import.
- **PivotTable Fields** panel beside the grid, for new pivots and those from files: check a field
  (text goes to Rows, numbers to Values as Sum, as in Excel), drag fields from the list or between
  Filters / Columns / Rows / Values (each chip also has a menu to move it up, down or to another
  area), Value Field Settings (Sum, Count, Average, Max, Min, Product, Count Numbers, StdDev,
  StdDevp, Var, Varp; a custom name; the source's or another number format), an item picker for
  report filters and row/column fields, Compact / Outline / Tabular layout, subtotals off / at the
  top / at the bottom, and grand total row and column. Every change lays the pivot out again from
  its source and is one undo step. Open it from the menu, the palette or the status bar's
  PivotTable chip; it follows the active cell into another pivot.
- **Delete PivotTable** removes the pivot at the active cell (one undo step); its parts, and its
  cache once no pivot uses it, are left out of the next save.
- New pivots are saved with their own parts (definition, cache with `refreshOnLoad`, records,
  relationships, the workbook's `<pivotCaches>`, content types; `PivotStyleLight16` and Excel's
  captions), so Kantor reads them back as it wrote them and they refresh, move with inserted rows
  and columns and answer GETPIVOTDATA like pivots from Excel.

### Fixed

#### Sheets
- Pivot tables: the grand total row and column were swapped when only one of them was on
  (`colGrandTotals` is the row at the bottom), and an item above a nested Σ Values field showed the
  first value field's total on its own row.

## [0.3.0] - 2026-11-11

Sheets catches up with how Excel files really look and work: conditional formatting, per-row
heights, Excel tables with sort and AutoFilter, data validation drop-downs, charts and pictures on
sheets, pivot tables that survive saving and refresh, AGGREGATE/GROUPBY/PIVOTBY/GETPIVOTDATA, and
printing / PDF export (also `kantor convert book.xlsx book.pdf`). Kantor can now update itself:
this is the first release the in-app updater offers.

### Added

#### Packaging
- Every download (dmg, AppImage, tarballs) ships `THIRD_PARTY_LICENSES.txt`: the licence texts of
  all ~610 crates linked into Kantor, generated at packaging time by
  `scripts/third-party-licenses.py`. On macOS it is inside `Kantor.app/Contents/Resources`.

#### Sheets
- Printing and PDF export (and `kantor convert book.xlsx book.pdf`) draw the charts and pictures
  on worksheets where they sit over the printed cells. An object across a page break is split,
  each page showing its part; print areas clip objects, fit to page and scale shrink them, hidden
  rows and columns shrink objects anchored over them, and objects in repeated title rows aren't
  repeated (as in Excel). Without a print area, objects beside the data widen the printed range,
  as Excel's does. Charts use their live cell values and keep vector, searchable text; pictures are
  embedded at their own resolution (JPEGs as they are). Hidden objects and those set not to print
  (`fPrintsWithSheet="0"`) are left out; shapes and text boxes aren't printed yet.
- **Pivot tables** are kept when a workbook is saved (they used to be dropped): the pivot table
  definitions, their caches and records, the workbook's `<pivotCaches>` list, relationships and
  content types are carried, and a pivot's location and its cache's source range follow inserted
  and deleted rows and columns. Pivot tables are drawn with their style (the built-in
  `PivotStyleLight/Medium/Dark 1–28`, coloured by the workbook theme: header, row subheadings,
  subtotals, grand totals, report filters), and the status bar shows **PivotTable · name** when
  the active cell is in one. **Refresh PivotTable** (Alt+F5) and **Refresh All PivotTables**
  (Ctrl+Alt+F5) recompute them from their source (a range, a table or a defined name): row and
  column fields in compact, outline or tabular form, the definition's item order with new items
  after, hidden and collapsed items, report filters, subtotals at the top or bottom, grand totals,
  several data fields with sum, count, average, max, min, product, count numbers, StdDev(p) and
  Var(p), and the data fields' number formats. A refresh is one undo step and rewrites the pivot's
  definition, cache and records for the next save. Inserting or deleting rows or columns through
  a pivot table is refused, as in Excel, and so is a refresh that would overwrite other cells.
  `GETPIVOTDATA` works against the pivot tables' cells (data field by caption or source field
  name, field/item pairs in any order) and follows refreshes and moved pivots.
  Slicers and timelines are kept too (their caches, parts, drawing anchors and the workbook and
  sheet extensions listing them), though Kantor doesn't show them yet.
- Charts and pictures on worksheets are shown over the grid, as in Excel: column, bar, line,
  area, pie, doughnut, scatter and combination charts (the Slides chart renderer, with the
  workbook theme's colours and fonts), and pictures with their cropping. They sit where their
  anchors put them and move with scrolling, frozen columns, row and column resizing and inserted
  or deleted rows and columns. Charts read their cells live: an edit or a recalculation redraws
  them, and axis and label number formats come from the cells. Click one to select it, drag to
  move it, drag a handle to resize it, Delete removes it; each is one undo step and is saved
  into the drawing's anchor (untouched drawings are saved byte for byte). Shapes, text boxes and
  unsupported chart types show as a placeholder box.
- **Export as PDF…** and **Print…** (toolbar printer menu, palette) and `kantor convert book.xlsx
  book.pdf` print a workbook the way Excel does (new crate `sheets_pdf`): each sheet's paper (A4,
  Letter, Legal, F4/Folio and Excel's other `paperSize` codes or a custom size), orientation,
  scale and *fit to pages wide/tall*, margins, horizontal/vertical centring, print area (several
  areas print on their own pages), print titles repeated after their page, manual row and column
  breaks, page order, printed gridlines and headings, headers and footers with `&P` `&N` `&D` `&T`
  `&F` `&Z` `&A`, sections, fonts, sizes and colours, first-page and odd/even variants. Without a
  print area the used range prints (cells with a value, fill or border). Hidden rows, columns and
  sheets are left out; rows without a stored height grow to fit wrapped or turned text. Cells keep
  number formats, fonts, fills, all 13 border styles, indent, rotation and stacked text, shrink to
  fit, centre across selection, justify, fill alignment, merged cells, overflow into empty
  neighbours, `####` for numbers too wide, conditional formats with data bars and icons, and table
  styles. Fonts are embedded subset with ToUnicode (text can be searched and copied). *Print…*
  writes a temporary PDF and opens it in the system's PDF viewer. Pages are read between frames
  and written by a background thread as they come: the 90,000-order sample (6,183 pages) exports
  in about 10–17 s after opening, peak memory 215 MB.
- `kantor convert` turns `.xlsx`, `.xlsm`, `.xls`, `.xlsb`, `.ods` and `.csv` into PDF: all visible
  sheets or `--sheet NAME`, with `--fit-width`, `--fit-page`, `--scale N`, `--landscape`,
  `--portrait`, `--paper a4|letter|legal|f4|…|WxHmm` and `--no-print-area` overriding the file.
- The engine reads the print settings IronCalc doesn't model (`Workbook::print_settings`), prints
  column widths as Excel does (`print_columns`) and keeps all 13 border styles apart (the grid
  draws the broken ones dashed, as before). Icon-set shapes are shared by the grid and the PDF.
- Excel tables are shown with their table style: header and total rows, banded rows and columns,
  first / last column, from the built-in Light, Medium and Dark styles (coloured from the
  workbook's theme, as Excel does) and the workbook's own table styles. Typing under a table
  without a total row extends it (calculated columns follow); rows and columns inserted inside
  one grow it (new columns get a `ColumnN` header); editing a header renames the column and the
  formulas that use it. All undoable, and saved into the table's part.
- Structured references work: `Sales[Units]`, `Sales[@Units]`, `Sales[[#Totals],[Units]]`,
  `Sales[#All]`, `Sales[[#Headers],[#Data]]`, column ranges and `[@Units]` inside a table. They
  follow the table as it grows and are saved as written (`[#This Row]` in the file, `@` on
  screen); before, they turned into fixed ranges on save.
- Sort: **Sort A to Z / Z to A** by the active cell's column (a table's rows, the selection, or
  the data around the active cell, with a header row guessed as Excel does) and **Custom Sort…**
  by several columns. Excel's order: numbers and dates, text (any case), FALSE/TRUE, errors,
  blanks last; stable; formulas move with their rows (relative references follow the row,
  absolute ones stay). Rows hidden by a filter stay put. One undo step. In the column header menu,
  the palette and each filter button.
- AutoFilter: **Filter** (`Ctrl Shift L`) puts filter buttons on a table or a data range; a button
  opens a menu to sort, filter by a text, number or date condition (equals, contains, begins
  with, greater than, between, before…) or by ticking values in a searchable list, or clear the
  column's filter. A filtered column's button shows a funnel. **Clear Filters** and **Reapply
  Filters** are in the palette. Filtered rows are hidden rows; SUBTOTAL and AGGREGATE skip them,
  and the filter is saved as Excel writes it (`<autoFilter>`, hidden rows) and read back from
  Excel's files, including filters Kantor keeps but can't edit (top 10, colours, dynamic dates).
  A 90 000-row table sorts in about 0.4 s and filters in under 0.1 s (release build).
- Rows have their own heights, as in Excel: custom heights from the file (relative to the sheet's
  default row height), hidden rows, and rows that grow to fit wrapped text and larger fonts
  (measured for the rows on screen, so big workbooks open and scroll as fast as before). Drag a
  row number's bottom edge to resize (all selected whole rows together), double-click it to fit,
  or use **Row Height…** / **Autofit Row Height** from the row header menu or the command
  palette. Each is one undo step and is saved to `.xlsx` as a custom height. Merged cells,
  selection, the cell editor, formula references and conditional formatting follow the heights,
  with no seams at 125 %. Arrow keys skip hidden rows, and Page Up / Page Down move by the
  visible height.
- **Ctrl+Home** goes to A1 and **Ctrl+End** to the last cell of the used range.
- Data validation from Excel files: list rules show a drop-down button beside the active cell
  (none when the file hides it); the button or **Alt+Down** opens the list, with arrows, Enter,
  Escape and type-to-filter, and picking an item is an ordinary undoable edit. Lists can be
  inline (`"a,b,c"`), a range, a range on another sheet (Excel 2010 form), a defined name or a
  formula such as `INDIRECT` or `OFFSET`, and are evaluated when opened, so they follow the cells
  they read. Input messages show under the selected cell. A typed value that breaks its rule
  (whole number, decimal, date, time, text length, list or custom formula, with every operator
  and *Ignore blank*) brings up the rule's error alert: Stop (Retry / Cancel), Warning
  (Yes / No / Cancel) or Information (OK / Cancel), with the file's title and text or Excel's
  default message; Retry and Cancel take the value back without leaving a redo. Pasting and
  filling aren't checked, as in Excel. **Circle Invalid Data** / **Clear Validation Circles** in
  the command palette ring cells that break their rule. Rules follow inserted and deleted rows
  and columns on screen as they do on save, and an unedited sheet's `<dataValidations>` is
  saved byte for byte.
- Conditional formatting is shown as Excel shows it: cell-value rules (every operator, operands
  with relative references), formula rules (relative to the top-left of the range), text rules,
  time periods, duplicate / unique values, top / bottom N and N %, above / below average, blanks
  and errors, 2- and 3-colour scales (min, max, number, percent, percentile, formula), data bars
  (gradient or solid, negative values around an axis) and icon sets drawn as vector shapes
  (arrows, traffic lights, flags, symbols, stars, quarters, ratings; reversed order and "icon
  only"). Priority and *Stop If True* are respected; formats of several true rules combine (fill,
  text colour, bold, italic, underline, strikethrough, number format). Only the cells on screen
  are evaluated, and what a rule needs from its whole range (min/max, percentiles, average,
  top N, duplicates) is worked out once per workbook state, so edits and recalculation update it.
  Saving writes the file's rules, Excel 2010 extensions and differential formats unchanged; their
  ranges and formulas follow inserted and deleted rows and columns.
- Formatted workbooks look as they do in Excel: cell fills (theme colours with tints and indexed
  colours resolved), text colour, font size and family (Calibri → Carlito, Arial → Liberation
  Sans when the original isn't installed), vertical alignment, wrapped text, borders (thin, medium,
  thick, double, dotted, medium dashed; shared edges resolved like Excel), merged cells (drawn as
  one cell; clicking or arrowing into one selects it as a whole) and gridlines off per sheet.
- Toolbar and command palette: fill and text colour, larger / smaller font, all / outside / no
  borders, wrap text, merge & center / unmerge, and vertical alignment (palette). Each is one
  undo step.
- MCP `sheets_format` also sets fill, text colour, font size, wrap, vertical alignment, borders
  and merging.
- Number formats show values as Excel does (`crates/kantor_numfmt`, adapted from GridCraft):
  fractions (`# ?/?`), elapsed time (`[h]:mm:ss`), conditions (`[>=1000]#,##0,"K";0`),
  colours (`[Red]` draws the value red), text sections (`"Name: "@`), fill (`* ` spreads
  accounting formats across the cell), spacing (`_)`), engineering notation, built-in format 14
  as `m/d/yyyy`, and halves rounded away from zero (`0` shows 1234.5 as `1235`).
- Locale-tagged formats work: `[$Rp-421]#,##0` shows `Rp1.500.000` (Indonesian separators from
  the tag) and `[$-421]d mmmm yyyy` shows `17 Agustus 2025`. They showed `#VALUE!` before.
- New functions: `AGGREGATE` (all 19 functions and options 0–7: skipping hidden or filtered rows,
  error values, and nested `SUBTOTAL`/`AGGREGATE` cells), `GROUPBY` and `PIVOTBY` (spilled
  summaries with headers, subtotals, grand totals, sorting by keys or values, a filter, and
  `relative_to` for `PERCENTOF`), `BAHTTEXT`, `USDOLLAR`, `DBCS`/`JIS` and `PHONETIC`. They
  showed `#NAME?` before, and they keep Excel's names in saved files (`_xlfn.GROUPBY`).
- A function can be passed by name where a LAMBDA is expected, as in Excel:
  `GROUPBY(A2:A99,B2:B99,SUM)`, `BYROW(A1:C9,MAX)`, `MAP(A1:A9,ABS)` (saved as `_xleta.SUM`).
- Formula autocomplete lists every function the engine has (it missed some before).

#### Updates
- In-app updates for the DMG, AppImage and install-script installs: a check at most once a day
  (and *Check for Updates…* in the palette) fetches a signed manifest, sending only Kantor's
  version and OS. The new version downloads in the background, is verified (Ed25519 manifest
  signature, SHA-256, and on macOS codesign / Gatekeeper) and installs on *Restart to update* or
  at quit, all or nothing. Package and source installs only get a notice. Turn it off with
  `[update] enabled = false` in `settings.toml` (README "Updates").
- `kantor --version` names the nightly build (`kantor 0.2.0 (nightly 2026-11-10 14:30)`).

### Changed
- Chart number formats (Slides and Sheets) go through the same formatter as Sheets cells
  (`kantor_numfmt`): locale tags such as `[$Rp-421]` use that locale's separators.
- Sheets: big workbooks use about a third less memory (a 180k-row, 32 MB `.xlsx`: 410 → 323 MB
  with the app open, 467 → 377 MB after F9), and closing one gives its memory back to the system
  (it stayed allocated before).
- Linux: the automatic UI font skips variable fonts (such as a single-file Inter or Adwaita Sans)
  when a family with static bold faces is installed, since GPUI draws only a variable font's
  default weight there and bold text looked regular. macOS is unchanged.

### Fixed
- Sheets: `COUNT` counts the numbers in an array (`COUNT(FILTER(...))` was always 0);
  `SUBTOTAL(110/111, ...)` skip hidden rows like the other 100-codes; `SUBTOTAL` skips any cell
  whose formula contains `SUBTOTAL` or `AGGREGATE`, not only those that start with it.
- Sheets: styled empty cells stored as `<c s=".." t="n"/>` (openpyxl and others) open blank
  with their style, as in Excel, instead of showing 0.
- Sheets: no more 1-pixel seams inside merged and filled cells at fractional scaling (125 %);
  merged text no longer looks doubled where it crosses a row or column edge, and a merge across
  frozen columns shows its part in each pane, lined up.
- Sheets: formula references get six distinct, readable colours in every theme (in Catppuccin
  and all Ghostty themes the first two were the same blue), and the coloured formula text is
  drawn once instead of over the plain text (it looked ghosted at 125 % on X11).

### Performance
- Sheets resolves each cell style once per workbook state instead of cloning it for every
  visible cell on every frame.

## [0.2.0] - 2026-11-10

Docs and Slides join Sheets: all three apps open, edit and save their Office formats, and Docs and
Slides export PDF. New: macOS builds (a notarised `Kantor.dmg`), Linux AppImages, `kantor convert`
and `kantor pdf` on the command line, opt-in AI features, and an MCP server for AI agents.

### Added

#### Docs
- **Docs** (first slice): open, edit and save `.docx` on A4 pages laid out with parley — typing,
  selection, Bold/Italic/Underline, headings (Ctrl+Alt+0–3), alignment, undo/redo, zoom, plain-text
  clipboard, IME input. New Document is Ctrl+Shift+N. Saving an opened `.docx` rewrites only the
  body and copies every other part unchanged.
- **Docs: lists, tables and pictures.** Bulleted and numbered lists (Ctrl+Shift+8 / Ctrl+Shift+7,
  toolbar; Enter continues, Enter on an empty item ends the list, Tab / Shift+Tab change level,
  Backspace at the start removes the number) with markers drawn in the hanging indent. Tables with
  merged cells and borders, laid out row by row across pages; Tab moves to the next cell (and adds a
  row at the end), toolbar grid to insert a table, insert/delete rows and columns, delete table.
  Inline PNG/JPEG pictures, shown at their size, selectable and deletable like a character; insert a
  picture from a file. All of it is undoable and saves in place.
- **Docs: keeps what it can't show yet.** Floating pictures, shapes and text boxes, content
  controls, page-number and date fields, equations, footnote references, bookmarks, nested tables,
  headers/footers and unmodelled formatting are carried byte for byte and shown as placeholders, so
  ordinary Word files now open as fully supported and save in place. Only tracked changes, comments
  and fields that depend on the document (tables of contents, cross-references) still make Save go
  to a new file. Page breaks and hyperlinks (editable text) are supported too.
- **Docs: Export as PDF…** (command palette) writes the document with the same layout as the
  screen: fonts subset and embedded (text stays selectable and searchable), pictures, colours,
  underline, tables and list markers. Headers and footers are not in the PDF yet.
- **Docs: page setup and ruler.** Paper size (A4, Letter, Legal, A5, F4), orientation and margins
  from the toolbar's Page control or the palette; a centimetre ruler with the margins shaded and
  indent markers you can drag. Saving changes only those settings in the file.
- **Docs: word and character count** in the status bar, for the selection when there is one
  ("12 of 1,234 words · 60 characters").
- **Docs: Rewrite / Translate selection.** Fix grammar, More formal, Shorter, or translate between
  Indonesian and English. Only the selected paragraphs' text is sent; the result is shown first and
  *Replace* applies it as one undo step, keeping paragraph styles (formatting inside a paragraph
  becomes that of its first word).

#### Slides
- **Slides** (first slice): new presentation (Ctrl+Alt+N, Home, sidebar, palette) and open/save
  `.pptx`. Slides look like the file: placeholder positions and text formatting come from the
  layout, master and theme, and master/layout decorations, backgrounds and pictures are drawn.
  Thumbnail strip (drag to reorder, Ctrl+Up/Down), new/duplicate/delete slide (Ctrl+M / Ctrl+D),
  select, move (drag or arrows) and resize shapes, edit text in text boxes and placeholders
  (Bold/Italic/Underline, IME), insert text box, rectangle, ellipse or PNG/JPEG picture, undo/redo,
  and a simple presenter mode (F5 / Shift+F5; arrows, space, Esc). Saving copies every part it
  doesn't rewrite byte for byte, keeps unchanged shapes verbatim and, for edited ones, replaces
  only the position or the paragraphs; tables, charts, groups and SmartArt are written back
  untouched. When slides are added, removed or reordered in a file with slide sections or
  custom shows, those can't be kept, so Save goes to a new file and the original is never
  overwritten.
- **Slides: lists, rotation, notes, fill and outline, copy/paste.** Bullets and numbering (from the
  master and layout, or the toolbar's list buttons), indent levels (Tab / Shift+Tab) and paragraph
  spacing are shown and edited; unmodelled paragraph settings are kept. Rotated and flipped shapes,
  pictures and text are drawn as in the file, with a rotate handle (Shift snaps to 15°). A
  resizable speaker-notes pane under the slide (notes are created with their part, relationships
  and notes master when needed) and notes under the slide while presenting (S hides them). Fill and
  outline popovers with the theme's colours, no fill / no outline and outline widths. Copy, cut and
  paste shapes within and between presentations (pictures take their image along) and Ctrl+D to
  duplicate the selected shape. All undoable.
- **Slides: canvas.** The slide fits the pane and stays centred; zoom with Ctrl+= / Ctrl+- /
  Ctrl+wheel and back to fit with Ctrl+0. Empty placeholders show a dashed outline and a grey
  prompt in the editor only. Resize, move and text cursors over shapes.
- **Slides: shapes keep their real outlines.** All 187 PowerPoint preset shapes (arrows, chevrons,
  stars, callouts, flowchart symbols, brackets, clouds, hearts, connectors…), their adjustments
  (handle positions such as an arrow's head size) and custom freeform shapes are drawn with their
  own outline instead of a box; 3D-style faces (cans, cubes, bevels) get their lighter and darker
  shades. Connectors are now shapes: they can be selected, moved and recoloured. Geometry XML is
  still written back unchanged. The evaluator (`slides_engine::geom`, no GPUI) follows ECMA-376
  shape guides and paths.
- **Slides: slides look like PowerPoint's.** Gradient and picture fills, transparency, dashed
  lines, line caps and arrowheads, and slide, layout and master backgrounds are drawn. Groups,
  tables (merged cells, borders, PowerPoint's table styles and banding) and SmartArt (from the
  drawing PowerPoint saves with it) are drawn instead of grey boxes. Charts are drawn from the
  values saved in the file: column and bar (clustered, stacked, 100 %), line, area, pie,
  doughnut, scatter and combinations, with titles, legends, axes, gridlines and data labels;
  other chart types still show a placeholder. All of this is display only: the XML of these
  objects is written back unchanged.
- **Slides: Export as PDF…** (command palette), one page per slide at the slide's size, hidden
  slides left out: real gradients, dashes, rotated text and pictures, embedded fonts. Exporting
  runs in the background.

#### Sheets
- **Sheets: big workbooks stay responsive.** Spreadsheets open in the background: the pane appears
  at once with the file name, size, elapsed time and a Cancel button. `.xlsx` files show the values
  Excel saved instead of recalculating everything on open. Workbooks with more than 50,000 formulas
  switch to **manual calculation** (status bar: "Manual calculation"): edits don't recalculate the
  workbook, F9 (or Shift+F9, or the palette's "Calculate Now") does, in the background with a
  "Calculating…" pane. Palette: "Automatic Calculation" / "Manual Calculation". A workbook saved
  with out-of-date values asks Excel to recalculate when opened. Status-bar sums and the used range
  are no longer recomputed over every cell on each frame.
- **Sheets: faster recalculation of lookup-heavy workbooks.** Kantor now carries a patched copy of
  IronCalc that caches lookups during a recalculation: a real 767,000-formula workbook went from
  more than 10 minutes to about 3 seconds.
- **Sheets: gridlines and headers.** Gridlines, a 2 px outline around the active cell and
  highlighted headers for the selected rows and columns.
- **Sheets: formula point mode.** While typing a formula with the caret after `=`, `(`, `,` or an
  operator, clicking a cell inserts its reference (drag or Shift+click for a range; clicking again
  replaces it), and arrows / Shift+arrows point too; a click elsewhere in the text still commits as
  before. Clicking another sheet's tab keeps the edit in the formula bar and inserts `'Sheet2'!A1`.
  References are coloured in the editor and outlined in the same colour on the grid; F4 cycles
  `A1 → $A$1 → A$1 → $A1`; typing a function name lists matching IronCalc functions (Tab/Enter
  inserts `SUM(`).
- **Sheets: grid manipulation.** Click a column letter or row number to select it (Shift+click or
  drag across headers for several), the corner or Ctrl+A for everything; Ctrl+arrows jump to the
  edge of the data and Ctrl+Shift+arrows extend to it. Drag the fill handle to fill down, up, left
  or right (series such as 1, 2 → 3, 4 and dates continue, other values repeat, relative references
  in formulas shift), or use Ctrl+D / Ctrl+R; each fill is one undo step. Double-click a column
  border to fit it to its content. Right-click a header to insert or delete rows and columns,
  autofit or reset widths, or freeze panes; the palette has the same commands plus Freeze Top Row /
  First Column / Panes and Unfreeze (saved in the `.xlsx`; frozen columns stay in view, frozen rows
  are marked with a line but still scroll). Backspace clears a selected range. The status bar shows
  Average, Count and Sum for a selected range.
- **Sheets: Ask AI for a formula.** Describe what you want in English or Indonesian ("jumlah penjualan
  untuk region Jakarta"); Kantor sends only the column headers, up to 20 sample rows near the
  selection and the active cell's address, checks the suggested formula with IronCalc (it must parse
  and calculate) and shows it with a one-line explanation; *Insert* puts it in the active cell as one
  undo step. *Explain This Formula* explains the active cell's formula. Sparkle button in the
  toolbar while AI is on.

#### All apps
- **Find & Replace in Docs, Sheets and Slides.** Ctrl+F opens a small find bar at the top right
  (match count "3 of 12", Enter / Shift+Enter or F3 / Shift+F3 for next / previous, Match case and
  Whole word toggles, Esc closes); Ctrl+H adds the replace row, and Replace All is one undo step.
  Docs highlights every match on the page and selects the current one; Sheets searches the active
  sheet's displayed values (or formulas, with "Look in formulas") and selects the cell; Slides
  searches every slide's text and jumps to it. Also in the palette (Find, Replace).
- **Untitled documents and tooltips.** New documents are named Untitled.xlsx, Untitled 2.xlsx …
  per app, and every toolbar button has a tooltip with its shortcut ("Bold · Ctrl+B").
- **AI features, opt-in.** *AI Settings* (command palette) has a master switch (off by default),
  a provider picker (Anthropic, OpenAI, OpenRouter, Groq, DeepSeek, Google Gemini, Ollama running
  locally, or any OpenAI-compatible server), the API key (stored in the system keychain, never in the
  settings file), base URL, model and *Test connection*. Requests go only to the provider you chose;
  Kantor runs no AI service of its own.
- **AI agents can edit open documents (`kantor mcp`).** An MCP server on stdio for Claude Code,
  Claude Desktop, Cursor and other agents (`claude mcp add kantor -- kantor mcp`; JSON setup via
  `kantor mcp --print-config` or the palette's *Copy MCP Setup for Claude Code*). It talks to the
  running Kantor over a private Unix socket (started on demand) and offers 26 tools: list, open,
  create, save and show documents; read and write cell ranges, select, format, insert and delete
  rows/columns in Sheets; read, insert, replace and delete paragraphs and find & replace in Docs;
  list slides, add a slide with title and bullets, set a shape's text or the speaker notes, delete
  and show slides in Slides. Each change is one undo step, selects what changed and shows
  "Agent: …" in the status bar. Speaks MCP 2026-07-28 (stateless) and the 2025-11-25 … 2024-11-05
  handshake revisions.

#### Command line
- **`kantor convert`** converts documents without opening a window (no display needed):
  `.docx` → `.pdf`, `.pptx` → `.pdf`, and spreadsheets (`.xlsx`, `.xlsm`, `.xls`, `.xlsb`, `.ods`,
  `.csv`, `.tsv`) → `.xlsx`, `.csv` or `.tsv` (`--sheet` picks the sheet). `kantor convert IN OUT`,
  or `-o DIR --to FORMAT FILES…` for many. It never overwrites without `-f` and never writes over
  its input. Spreadsheet → PDF is not supported yet.
- **`kantor pdf`** tools for PDF files: `merge` (with page ranges per file), `split`, `extract`,
  `delete`, `reorder`, `rotate`, `info`, `images` (JPEG/PNG to PDF), `watermark`, `page-numbers`
  and lossless `compress`. Password-protected PDFs open with `--password`. Outputs are never
  overwritten without `-f`.

#### Downloads and install
- **macOS.** Kantor runs on macOS: `Kantor.dmg` is a universal app (Apple Silicon and Intel,
  macOS 11 or later), signed with a Developer ID and notarised by Apple, so it opens with a
  double-click. Shortcuts use ⌘ instead of Ctrl (Ctrl+Tab stays), and shortcut labels show ⌘⇧⌥.
- **Linux AppImages** for x86_64 and aarch64 (`Kantor-x86_64.AppImage`,
  `Kantor-aarch64.AppImage`): download, make executable, run. They need glibc 2.39 or newer
  (Ubuntu 24.04, Debian 13, Fedora 40 or later), like the tarballs.
- **Stable download links.** Every release has `Kantor.dmg`, `Kantor-x86_64.AppImage` and
  `Kantor-aarch64.AppImage` under the same names (plus versioned copies), so
  `https://github.com/dzulfikar08/kantor/releases/latest/download/<name>` always points at the
  newest release.
- **Install script:** `--uninstall` removes what it installed (binary or link, Kantor.app,
  launchers, icons; settings are kept); when `~/.local/bin` is not on `PATH` it prints the line to
  add for zsh, bash or fish; macOS installs from tagged releases (v0.2.0 on), with a clear message
  for releases that have no macOS build.

### Changed
- Release builds use fat LTO and one codegen unit: the Linux binary shrank from 60.3 MB to
  51.0 MB.
- The command palette reacts faster: typing redraws only the palette, and the list of system fonts
  is read once (on macOS each letter took about 190 ms before).

### Fixed
- Sheets: approximate lookups (`VLOOKUP`/`HLOOKUP`/`LOOKUP` with sorted data, `MATCH` with match
  type 1 or -1) stop at the last non-empty cell, as Excel does. Before, a lookup over whole
  columns could land on a blank row and return 0.
- Docs: the ruler is drawn in the theme's chrome colours instead of paper white.
- Release notes and README no longer claim a `kantor-bin` AUR package: it was prepared
  (`packaging/aur/`) but never published.

### Known limitations
- Not yet checked against a large set of real-world files; expect layout differences from Word
  and PowerPoint.
- Docs: headers and footers, footnotes, fields and floating objects are kept in the file but shown
  only as placeholders, and are not in the PDF; tracked changes, comments and tables of contents make Save go to a new file.
- Slides: charts other than the types above show a placeholder; the content of charts, tables,
  groups and SmartArt can't be edited yet. On screen, rotated text stays upright (the PDF turns it).
- Sheets: no sort, filter, charts, cell colours/borders editing, printing or autosave yet; `.ods`,
  `.xls` and `.xlsb` are import-only.
- `kantor convert` can't turn spreadsheets into PDF yet.
- The Linux builds need glibc 2.39 or newer.

## [0.1.0] - 2026-11-07

First tagged release: the **Sheets MVP**. Docs and Slides are not started yet.
Prebuilt for x86_64 and aarch64 Linux (built on Ubuntu 24.04, glibc 2.39; runs on Arch
and on Asahi's 16K-page kernel).

### Added

#### Suite shell
- One binary for the whole suite: `kantor` opens the Home screen (Docs · Sheets · Slides),
  `kantor file.xlsx` or `kantor --sheets` goes straight to Sheets.
- Document tabs in the titlebar, split panes (`Ctrl \` / `Ctrl Alt \`) with draggable
  dividers, a resizable sidebar (open, recent, create) and inspector (cell and workbook
  details), and a status bar.
- Command palette (`Ctrl K` / `Ctrl Shift P`) with fuzzy search.
- Themes: follows the Omarchy theme by default and live-reloads on `omarchy theme set`;
  the theme picker also offers the Omarchy themes and 907 Ghostty themes with live preview.
- UI font picker (SF Pro when installed, never bundled; else Inter / Adwaita Sans).
- Open tabs, splits, panel sizes and recent files are restored on the next launch
  (`~/.local/state/kantor/workspace.json`; settings in `~/.config/kantor/settings.toml`).
- Asks to Save / Don't Save / Cancel before closing a pane or tab, quitting (`Ctrl Q`) or
  closing the window with unsaved changes.
- Renderer memory patch (vendored `gpui-pre-wgpu`): path textures are allocated only when
  used and path MSAA is off by default, which fixes wgpu *Out of Memory* crashes on 8 GB
  machines with a browser open.

#### Sheets
- Open, edit and save `.xlsx` through IronCalc 0.9.1: formulas recalculate, multiple
  sheets, atomic save.
- CSV/TSV open and save: delimiter detection, UTF-8 BOM / UTF-16 / Windows-1252 input; one
  sheet of displayed values; a leading `=` stays text.
- Read-only import of `.ods`, `.xls` and `.xlsb` (values, dates, formulas). Formulas that
  don't reproduce the file's cached result are kept as values. Saving an imported file
  goes to Save As `name.xlsx`.
- Undo/redo (`Ctrl Z`, `Ctrl Shift Z` / `Ctrl Y`); the unsaved marker follows the undo
  history, so undoing back to the saved state makes the document clean again.
- Range selection (Shift+arrows, Shift+click, drag) and copy/cut/paste of ranges as TSV
  (`Ctrl C`/`X`/`V`), to and from other apps; each paste or clear is one undo step.
- Excel-style editing: type to replace, `Enter`/`F2`/double-click to edit, `Esc` to
  cancel, `Delete` to clear; name box and formula bar.
- In-cell editor over the active cell, synced live with the formula bar (IME input goes
  through GPUI); Enter/Tab and their Shift variants commit and move.
- Formatting: bold, italic, underline, strikethrough (`Ctrl B`/`I`/`U`), horizontal
  alignment, number formats (General, Number, Percent, Rupiah, Dollar, Date) and column
  widths, all saved to `.xlsx`. A toolbar above the formula bar collapses into a "…" menu
  when the window is narrow.

#### Project
- Install script: `curl -fsSL https://raw.githubusercontent.com/dzulfikar08/kantor/main/scripts/install.sh | sh`
  (nightly), or `KANTOR_VERSION=v0.1.0` for this release.
- A PKGBUILD for an AUR package `kantor-bin` (`packaging/aur/`; not published).
- xlsx fidelity corpus (12 generated files) with a round-trip test and a Fidelity CI job.
- Round-trip safety for `.xlsx`: comments, custom properties, drawings/charts/images, data
  validation, print setup and other parts IronCalc doesn't model are carried over from the
  original file on save.

### Known limitations
- **Docs and Slides** are placeholders ("coming soon"). A text-layout prototype
  (`docs_layout`, parley) exists but has no UI yet.
- **Row heights** are read and saved but not drawn: every row has the same height.
- Cell colours, borders and merged cells are kept in the file but can't be edited yet.
  No sort, filter, find and replace, fill handle, frozen panes, printing or autosave.
- Pasted formulas are not reference-shifted (`=A1` stays `=A1`).
- The in-cell editor has no Alt+Enter line breaks and hides if the cell is scrolled away.
- Splitting a pane opens a new blank sheet; the same workbook can't be shown in two panes.
- `.ods`, `.xls` and `.xlsb` are import-only; Kantor never writes them.
- IronCalc 0.9.1 file-fidelity gaps (full list in `tests/corpus/xlsx/known-gaps.tsv`):
  - 3-D references (`SUM(Jan:Mar!B2:B4)`) evaluate to `#ERROR!` in Kantor; the file is saved
    with the formula and no cached value, so Excel/LibreOffice recalculate it on open.
  - Parts Kantor doesn't carry over on save (each reported, never silently): tables, pivot
    tables, external links, VBA macros, chart sheets, embedded objects/form controls, slicers.
  - Defined names holding a constant (`Ten = 10`) give `#NAME?`.
  - Dashed and hair borders become thin borders.
  - Number formats: locale-tagged currency (`[$Rp-421]#,##0`) shows `#VALUE!`; fraction
    formats are unsupported; built-in date format 14 shows as `mm-dd-yy`; times can lose a
    minute to rounding; `0` rounds halves to even (1234.5 → 1234; Excel gives 1235).
  - `LEN` counts Unicode scalar values, not UTF-16 code units as Excel does (emoji differ).
  - `INDEX(...)` formulas display as `@INDEX(...)`; quotes around non-ASCII sheet names are
    dropped from formula text; shared strings lose leading/trailing spaces in strict readers.
- Not yet verified on the M1 Air: fractional scale 1.6, fcitx5 composition, memory use
  with a 10k-cell sheet, Hyprland's close-window path for the unsaved-changes prompt.

[Unreleased]: https://github.com/dzulfikar08/kantor/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/dzulfikar08/kantor/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/dzulfikar08/kantor/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/dzulfikar08/kantor/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/dzulfikar08/kantor/releases/tag/v0.1.0
