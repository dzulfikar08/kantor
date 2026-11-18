# Real-world file sweep

424 public Office files, each opened, saved, reopened and exported to PDF by
Kantor's engines, with the saved files checked by Microsoft's Open XML SDK.
First run: 2026-11-15; second, third and fourth runs on main: 2026-11-16;
fifth run, before v0.11.0, and sixth, before v0.12.0: 2026-11-17;
seventh, before v0.13.0, and eighth, before v0.14.0: 2026-11-18.

## What was swept

| format | files | | format | files |
|---|---:|---|---|---:|
| `.xlsx` | 155 | | `.xls` | 32 |
| `.docx` | 114 | | `.ods` | 21 |
| `.pptx` | 62 | | `.doc` | 16 |
| `.xlsm` | 4 | | `.ppt` | 12 |
| | | | `.xlsb` | 8 |

74 MB in all, up to 11 MB a file. Sources, by site:

| source | files | licence |
|---|---:|---|
| Apache POI test data (github.com/apache/poi): mostly files attached to user bug reports | 128 | Apache-2.0 |
| LibreOffice QA documents (github.com/LibreOffice/core, `sc/`, `sw/`, `sd/` `qa/unit/data`): bug-report files | 106 | MPL-2.0 |
| docx4j samples (github.com/plutext/docx4j) | 45 | Apache-2.0 |
| Open XML SDK test assets (github.com/dotnet/Open-XML-SDK) | 39 | MIT |
| data.gov.uk (UK government open data: statistics, registers, reports) | 36 | mostly OGL |
| calamine test files (github.com/tafia/calamine) | 23 | MIT |
| Indonesian public GitHub projects (budget workbooks, reports, overtime and election data) | 26 | as published |
| ClosedXML, python-pptx, python-docx test files | 21 | MIT |

Indonesian government portals (data.go.id, BPS, regional Satu Data sites)
were tried but serve their files through JavaScript pages or API keys, so the
Indonesian files come from public repositories instead.

The files are **not** in this repository (their licences vary). They live in
`~/.cache/realworld/<format>/` with `manifest.tsv` (URL, site, licence,
sha256). Many of the test-suite files are small and made to show one feature
or bug; the government and Indonesian files are ordinary work files.

## How to run it

```sh
# 1. Fetch (a plan is `format \t url \t site \t licence` per line)
python3 -I tools/realworld/fetch.py plan.tsv
# 2. Build the probe (release) and the validator
CARGO_TARGET_DIR=~/.cache/rw-target cargo build --release -p office_fidelity --bin realworld_probe
dotnet build -c Release tools/ooxml-validate -o ~/.cache/rwtools/validator
# 3. Sweep
python3 -I tools/realworld/sweep.py \
    --probe ~/.cache/rw-target/release/realworld_probe \
    --validator 'dotnet ~/.cache/rwtools/validator/OoxmlValidate.dll' \
    --out ~/.cache/realworld-out/run --jobs 4
```

`realworld_probe` (tools/fidelity/src/realworld.rs) opens one file, saves it
as `.xlsx`/`.docx`/`.pptx`, opens that again, exports a PDF and prints JSON
with each stage's time and error (panics caught) and counts before and after
(sheets, cells, formulas, every cell's input; paragraphs and their text;
slides, shapes and text). `sweep.py` runs it with a 120 s timeout and a 4 GB
address-space cap, validates every saved Office file and its original with
the Open XML SDK (errors the original already has don't count), and writes
`results.csv` and `summary.md`. For Docs and Slides the probe also lays
out the saved-and-reopened copy: a page count that differs from the
original's is listed in the summary ("page count changed by the round
trip"; `pages_changed` in `results.csv`). Each file gets the first matching kind:
crash, timeout, panic, open error, save error, reopen error, PDF error,
invalid save, lost content, ok.

"Lost content" compares Kantor's reading of the file with its reading of its
own saved copy, so it finds what saving drops, not what the first open
misreads. Layout (how pages look next to Office) is not compared.

## Results

| | before | after |
|---|---:|---:|
| crash or panic | 5 | 0 |
| open over 30 s (one timed out at 120 s) | 2 | 0 |
| refused to open | 34 | 19 |
| saved file Office would repair | 2 | 1 |
| content changed by a save | 0 | 0 |
| ok | 381 | 404 |
| total time (4 jobs) | 263 s | 156 s |

Every saved file reopened and every opened file exported a PDF, in both runs.

## Fixed

- **Workbooks from other writers that crashed or were refused (19 files).**
  A styles part without `<fills>`, `<borders>`, `<cellStyleXfs>` or
  `<cellStyles>` panicked; so did a format pointing at a font that isn't
  there. Workbooks without a styles part at all, and rows or cells without
  the optional `r` reference, were refused. (vendor/ironcalc §10)
- **A crash in a file reader is now an open error.** The `.xlsx` and
  `.ods`/`.xls`/`.xlsb` readers run behind a panic guard, so a reader bug
  (calamine panics on one `.xlsb`) says the file can't be read instead of
  closing Kantor.
- **`.ods` files that took minutes to open (2 files, 54–120 s → under 4 s).**
  LibreOffice ends each row with one empty cell repeated to the last column,
  carrying the column's or the row's style; Kantor styled each of those
  cells one by one. Column defaults now become column styles and whole-row
  runs row styles (column widths are capped at Excel's 255).
- **`.ods` sheet names Excel doesn't allow** (LibreOffice names a sheet linked
  from another file after its URL) refused the whole file; such sheets are
  renamed now, with an import note.
- **Docs: an empty `<w:sectPr/>`** got its page size and margins written after
  it, into the body; Word reports the saved file as damaged.
- **Slides: `.ppt` groups with very large coordinate spaces** were written
  with coordinates past Office XML's 32-bit limit (16 schema errors in one
  deck); the group's space is scaled down.
- Clearer refusals: Strict Open XML workbooks and documents say so (they
  were "XML error: missing r:id" and "not a WordprocessingML document"),
  and Word 6/95 files are named as such instead of "not a Word document".

Each fix has a synthetic regression test (crates/sheets_engine/tests/realworld.rs,
docs_engine `self_closing_section_properties_get_page_setup_inside`,
msdoc `refuses_encrypted_old_and_foreign_files`,
slides_engine ppt `huge_group_spaces_fit_32_bit_coordinates`, docs_engine
page `missing_elements_are_inserted_in_schema_order`).

## Strict Open XML (later)

The 8 Strict Open XML files (ISO/IEC 29500 Strict: 5 `.pptx`, 2 `.docx`,
1 `.xlsx`) open now. `crates/kantor_ooxml` rewrites a Strict package as
Transitional before the Sheets, Docs and Slides readers run: Strict
namespace and relationship-type URIs (`http://purl.oclc.org/ooxml/...`) map
to their Transitional ones, ISO 8601 date cells (`t="d"`) become serial
numbers, DrawingML percentages (`"35%"`) and lengths with units (`"1in"`)
become numbers, and `conformance="strict"` goes. The rewritten package is
what the document keeps, so Save writes Transitional under the same name.
Rerun on those 8 files: all ok (opened, saved, reopened, printed, 0 new
Open XML SDK errors). The 2 `.docx` also showed that a part starting with a
byte order mark (the Open XML SDK writes one) was saved damaged; fixed.
A rerun of all `.docx` and `.pptx` files: 112/114 and 62/62 ok, so 412 of
the 424 files are ok now.

Tests: `crates/{sheets,docs,slides}_engine/tests/strict.rs` open Strict
fixtures made from corpus files by `scripts/corpus/gen_strict.py` and compare
them with their Transitional originals; `tools/fidelity` validates the
Transitional files saved from them.

## Second run (2026-11-16)

Rerun of all 424 files on main after Strict Open XML, Docs fields,
floating objects, headers/footers, notes, Track Changes and comment
threads, Slides groups/frames/turned text, Sheets shapes, charts, pivots,
notes and new functions, and `.ods`/`.doc` drawings had merged, compared
file by file with the first run (and its Strict reruns).

| | first run (after fixes) | main, 2026-11-16 | with this run's fixes |
|---|---:|---:|---:|
| crash, panic or timeout | 0 | 0 | 0 |
| refused to open | 19 (11 after Strict) | 11 | 6 |
| saved file Office would repair | 1 | 1 | 0 |
| content changed by a save | 0 | 0 | 0 |
| page count changed by a save (Docs, Slides; new check) | – | 0 of 199 | 0 of 199 |
| ok | 404 (412 after Strict) | 412 | 418 |
| slowest file | 9.4 s | 5.1 s | 4.9 s |
| all files' probe time added up | 156 s | 68 s | 75 s |

No regressions: no new crash, open, save or reopen error, no new Open XML
SDK error, no file more than twice as slow and over a second (three files
flagged once were back to their old times when run alone; the sweep shares
the machine). What changed, and why:

- **Page counts of Docs files went down** for 13 documents (`.docx` and `.doc`) (e.g. 3 → 2
  pages for files with headers and footers, 63 → 57 for a long manual);
  each now matches LibreOffice's count or is closer to it (Docs
  header/footer, floats and page-break fixes).
- **More text read** in 16 `.docx` and 1 `.doc` (field results, tracked insertions and
  deletions, comment text), and 99 more paragraphs in one docx4j manual
  (text boxes): the newer readers see more, and saving keeps all of it.
- **Sheets printing:** `.ods` page setup is read now, so `.ods` page counts
  follow the file's margins and print ranges (4752 → 28 pages for one
  file; LibreOffice prints 19). One LibreOffice test workbook (tdf66668)
  prints 4 pages instead of 2 because shapes print now: it has a white
  rectangle anchored over A1:N57, which Excel prints too.

Fixed in this run (each with a synthetic test in
`crates/sheets_engine/tests/realworld.rs`):

- **Workbooks whose parts aren't where Excel puts them** (3 files):
  the workbook part at the package root or renamed (`xl/workbook2.xml`),
  and part names written with backslashes and in another case than the
  relationships give. `kantor_ooxml::spreadsheet_layout` moves the
  workbook, styles and shared strings to Excel's part names before
  IronCalc reads the package (`xlsx_parts_outside_excels_layout_open`).
- **Indented `.ods`** (2 files): calamine stops on whitespace between
  cells; such a file is read again from a copy without the indentation
  (`ods_indented_content_opens`).
- **Cell formats past the end** (1 file, the saved file Office would
  repair): cells pointing at format 336 of 100 take the default format, as
  Excel repairs them (vendor/ironcalc §13,
  `xlsx_cell_formats_past_the_end_are_the_default`).

## Third run, before v0.9.0 (2026-11-16)

Rerun of all 424 files on main at `ab9aa6b` (after Slides chartex and
SmartArt relayout, Sheets slicers, pivot calculated items, custom table
and PivotTable styles, and IMAGE pictures saved as Excel's rich values
had merged), release build, compared file by file with the second run.

| | second run (with its fixes) | main, `ab9aa6b` | with this run's fix |
|---|---:|---:|---:|
| crash, panic or timeout | 0 | 0 | 0 |
| refused to open | 6 | 6 | 6 |
| saved file Office would repair | 0 | 2 | 0 |
| content changed by a save | 0 | 0 | 0 |
| page count changed by a save (Docs, Slides) | 0 of 199 | 0 of 199 | 0 of 199 |
| ok | 418 | 416 | 418 |

By source:

| source | files | ok | refused |
|---|---:|---:|---:|
| Apache POI | 128 | 124 | 4 (3 Word 6/95, 1 `.xlsb`) |
| LibreOffice QA documents | 106 | 106 | |
| docx4j | 45 | 43 | 2 (not WordprocessingML) |
| Open XML SDK | 39 | 39 | |
| data.gov.uk | 36 | 36 | |
| calamine | 23 | 23 | |
| Indonesian public GitHub projects | 26 | 26 | |
| ClosedXML, python-pptx, python-docx | 21 | 21 | |

No other changes: no new open, save or reopen error, no new Open XML SDK
error apart from the one below, and the same counts of cells, formulas,
paragraphs, slides and shapes before and after saving as in the second
run. The sweep flagged 9 workbooks as more than twice as slow; timed
alone, five runs each, alternating with a probe built from the second
run's commit, they were within noise (0.95–1.24×, under 250 ms apart),
and the slowest file (a 9 MB `.xls`, 4.8 s in the second run, 8.4 s in
this one) took the same time with both probes. The machine was heavily
loaded during the sweep (its added-up probe time went from 75 s to 144 s
for the same reason).

Fixed in this run:

- **Workbooks with custom table styles saved as Office would repair them**
  (2 files). Excel puts an `xr9:uid` on each `<tableStyle>` and declares
  `xr9` as ignorable on the styles part's root only. The custom table
  styles work (v0.9) writes the file's own table styles back into the
  styles part, which has no such root declaration, so the Open XML SDK
  (and Office) saw an undeclared attribute. Extension prefixes the copied
  markup uses are now declared on the root and listed in `mc:Ignorable`
  (`table_style_edit::copied_table_style_uids_stay_ignorable`).

The Office validation scenarios (`tools/fidelity/office_check.py`) were run
in full as well: 157 saved files, 0 new Open XML SDK errors, all converted
to PDF by LibreOffice.

Results: `~/.cache/realworld-out/pre090-2026-11-16/`.

## Fourth run, before v0.10.0 (2026-11-16)

Rerun of all 424 files on main at `2d1388d` (after map charts drawn from
Office's geography cache, SmartArt hierarchy relayout, Insert Slicer,
timelines that filter and slicers that print in Sheets, Docs tracked
cross-column merges, splits and field codes, column insert/delete in
merged tables, Docs SmartArt editing, gradients and header object
selection had merged), release build, compared file by file with the
third run.

| | third run (with its fix) | main, `2d1388d` |
|---|---:|---:|
| crash, panic or timeout | 0 | 0 |
| refused to open | 6 | 6 |
| saved file Office would repair | 0 | 0 |
| content changed by a save | 0 | 0 |
| page count changed by a save (Docs, Slides) | 0 of 199 | 0 of 199 |
| ok | 418 | 418 |
| slowest file | 8.4 s | 6.6 s |
| all files' probe time added up | 144 s | 110 s |

No regressions: the same kinds, errors, counts of cells, formulas,
paragraphs, slides and shapes, and page counts as the third run, and no new
Open XML SDK error. No file was flagged as more than twice as slow and over
a second; the few small `.docx` that took about twice as long in the sweep
(100–160 ms → 220–360 ms) took 39–60 ms each when run alone.

The Office validation scenarios (`tools/fidelity/office_check.py`): 164
saved files, 0 new Open XML SDK errors, all converted to PDF by
LibreOffice. Two scenarios added with the new features failed on their own
assumptions, not on Kantor's output, and were fixed:

- `slicers_inserted_25_slicers.xlsx` selected the new pivot slicer as
  "Product", but the file already has a Product slicer, so the new one is
  named "Product 1" (as in Excel). The scenario now finds the slicers it
  inserted by key, and changes the settings of its new Region slicer.
- `edit_region_map.pptx` edited the data of the first chart frame, but a
  map (chartex) chart has no editable data grid, and then duplicated a slide
  whose map frame `group_edits` had just put in a group. The scenario now
  picks a chart with editable data and looks inside groups for frames
  Kantor can't copy.

Results: `~/.cache/realworld-out/pre0100-2026-11-16/`.

## Fifth run, before v0.11.0 (2026-11-17)

Rerun of all 424 files on main at `0e49fbb` (after Docs text boxes edited
in place, SmartArt text that shrinks to fit and the Labeled Hierarchy
layout, Insert Timeline and Timeline Settings, and editing the data of
waterfall, funnel, treemap, sunburst, histogram and box charts in Slides
and their data range in Sheets had merged), release build, compared file
by file with the fourth run.

| | fourth run | main, `0e49fbb` |
|---|---:|---:|
| crash, panic or timeout | 0 | 0 |
| refused to open | 6 | 6 |
| saved file Office would repair | 0 | 0 |
| content changed by a save | 0 | 0 |
| page count changed by a save (Docs, Slides) | 0 of 199 | 0 of 199 |
| ok | 418 | 418 |
| slowest file | 6.6 s | 4.4 s |
| all files' probe time added up | 110 s | 64 s |

No regressions: every file has the same kind, error, counts of cells,
formulas, paragraphs, slides and shapes, page counts and import notes as
in the fourth run, and no new Open XML SDK error. No file was more than
twice as slow (the machine was less loaded than in the fourth run, hence
the lower times).

The Office validation scenarios (`tools/fidelity/office_check.py`): 165
saved files, 0 new Open XML SDK errors, all converted to PDF by
LibreOffice. One scenario was added: `timeline_inserted_25_slicers.xlsx`
inserts a timeline on the pivot table's date field, changes its caption,
style, scrollbar and selection label, filters it to February–March and
deletes the file's own timeline.

Results: `~/.cache/realworld-out/pre0110-2026-11-17/`.

## Sixth run, before v0.12.0 (2026-11-17)

Rerun of all 424 files on main at `13d5337` (after 3-D references, macros
kept in `.xlsm`, filter and sort by colour and icon, the icon set and data
bar editor, Page Break Preview, table formats, the shared WMF/EMF/PICT
renderer used by every importer and by PDF export, `.doc` groups, revisions
and nested tables, `.xls` shapes, charts, Excel 5/95 formatting and pattern
fills, `.ods` freehand shapes, groups and form controls, `.xlsb`
formatting, drawings and names, computed Docs fields, SmartArt Change
Layout and 3-D charts drawn in 3-D had merged), release build, compared file
by file with the fifth run.

| | fifth run | main, `13d5337` |
|---|---:|---:|
| crash, panic or timeout | 0 | 0 |
| refused to open | 6 | 6 |
| saved file Office would repair | 0 | 0 |
| content changed by a save | 0 | 0 |
| page count changed by a save (Docs, Slides) | 0 of 199 | 0 of 199 |
| ok | 418 | 418 |
| slowest file | 4.4 s | 4.2 s |
| all files' probe time added up | 64 s | 64 s |

No regressions: every file has the same kind, error and counts of cells,
formulas, paragraphs, slides and shapes as in the fifth run, and no new Open
XML SDK error. The `.xlsb` refused by calamine is still refused (calamine
can't read its workbook part). What changed:

- **Import notes went away** for 12 files: 7 Excel 5/95 `.xls` no longer say
  "formatting not imported", 4 `.ppt` no longer show an EMF/WMF picture as a
  box, and the `.ods` with a polygon (tdf137216) imports it now. The 3
  `.xlsm` with a VBA project say "macros are kept but not run".
- **Printed pages of 5 workbooks changed**, each because more of the file is
  read now: 3 Excel 5/95 `.xls` print with their own fonts and row heights
  (t_giat 72 → 76 pages, LibreOffice 76; t_akun 74 → 76, LibreOffice 74,
  which lays the columns out wider than Excel's default width; t_output
  2088 → 2184), and 2 `.xlsb` print their formatting and pictures
  (picture.xlsb 1 → 2 pages, LibreOffice 2; 51519.xlsb 1 → 2, LibreOffice 6).
- **Timing:** no file was more than twice as slow and over a second. Timed
  alone (min of 5–15 runs, alternating with a probe built from the fifth
  run's commit), the big workbooks took the same time (0.91–1.01×) and the
  `.doc` files flagged as slower in the sweep were not. The 4 `.ppt` whose
  EMF pictures are drawn now spend 15–30 ms more printing (each print draws
  the picture), the only real cost of the metafile renderer seen here.
  Pattern fills cost nothing measurable (the 9 MB `.xls`: 3.6 s → 3.3 s).

The Office validation scenarios (`tools/fidelity/office_check.py`): 189
saved files, 0 new Open XML SDK errors, all converted to PDF by LibreOffice.
Two scenarios were added for markup v0.12 writes:
`sheets_v012_features.xlsx` (3-D references, a colour filter, Top 10, sort by
cell colour and a custom list, an Excel 2010 data bar with every setting, a
custom icon set with No Cell Icon, rule number formats and borders, a
"Text that is equal to" rule, a validation IME mode, Page Break Preview and
a manual break, saved twice) and `macros_kept.xlsm` (a workbook with a VBA
project edited and saved twice as `.xlsm`).

Fixed in this run: a "Text that is equal to" rule is saved as the formula
Excel uses for one (`B2="Plums"`, Excel has no such text rule), and opened
again it was an ordinary formula rule; it is read back as "Text that is
equal to" now (`cf_create::text_equal_to_number_formats_and_borders`).

Results: `~/.cache/realworld-out/pre0120-2026-11-17/`.

## Seventh run, before v0.13.0 (2026-11-18)

Rerun of all 424 files on main at `441091b` (after Sheets zoom saved per
sheet, Docs section columns with balancing, column breaks and `.doc`/`.rtf`
import, `.xlsb` conditional formatting, `.xls` cell formulas decoded by
Kantor's own reader (3-D references), `.ods` custom shapes and arcs and
shape placement on Kantor's default cell sizes, DrawingML pattern fills,
charts inside groups, SmartArt Organization Chart assistants and hanging
layouts, six picture layouts, and overlays that block hover had merged),
release build, compared file by file with the sixth run.

| | sixth run | main, `441091b` |
|---|---:|---:|
| crash, panic or timeout | 0 | 0 |
| refused to open | 6 | 6 |
| saved file Office would repair | 0 | 0 |
| content changed by a save | 0 | 0 |
| page count changed by a save (Docs, Slides) | 0 of 199 | 0 of 199 |
| ok | 418 | 418 |
| slowest file | 4.2 s | 4.6 s |
| all files' probe time added up | 64 s | 78 s |

No regressions: every file has the same kind, error and counts of cells,
paragraphs, slides and shapes as in the sixth run, and no new Open XML SDK
error. What changed:

- **`.xls` formulas.** POI's 55906-MultiSheetRefs.xls keeps its 11 3-D
  formulas (`SUM(Sheet1:Sheet3!A1:B2)`, `COUNTA`, `MIN`/`MAX`/`COUNT` with
  mixed references) instead of their values: 13 formulas instead of 2, and
  its "11 formulas kept as values" import note is gone. Every `.xls` cell
  formula now goes through Kantor's decoder, so the values Kantor works out
  were compared with the results cached in each file (read with xlrd): 4,623
  formulas in the 31 files xlrd reads, none different (xlrd can't read the damaged
  cfb_dir_short_chunk_panic.xls).
- **Docs pages.** Docx4j_GettingStarted.docx (a two-column section) prints
  71 pages instead of 72 (LibreOffice 70). The other files with columns (3
  `.docx` and the seven-column `.doc`) have the same page count as before
  and as LibreOffice, and their pages look like LibreOffice's (columns,
  line between, INDEX in columns). No other Docs page count changed.
- **Zoom.** The 17 workbooks with a zoom of their own keep it through a
  save (`zoomScale` read and written back for every sheet); printed pages
  don't change.
- **Timing:** one flag (bug66675.xlsx, PDF 0.2 → 1.0 s in the sweep) was
  157 ms alone against v0.12.0's 181 ms. Timed alone against a probe built
  from the `v0.12.0` tag (min of 5–15 interleaved runs), the 12 slowest files,
  the 12 with the largest sweep ratio and the files with columns took the
  same time (0.87–1.14×, the 1.14× ones back to 0.96–0.99× over 15 runs).
  The added-up time was higher because the machine was busier.

The Office validation scenarios (`tools/fidelity/office_check.py`): 207
saved files, 0 new Open XML SDK errors, all converted to PDF by LibreOffice.
Four scenarios were added for markup v0.13 writes:
`sheets_v013_features.xlsx` (a zoom on each of three sheets, one in Page
Break Preview, a pattern fill set on a shape, the sheet whose group holds a
chart copied with Move or Copy, saved twice), `columns_edited.docx` and
`new_doc_columns.docx` (two columns with a line between from This point
forward, a column break, three columns in the last section, saved twice)
and `org_chart_edits.pptx` (Add Assistant and Right Hanging on an
Organization Chart, a Hierarchy changed to an Organization Chart and given
an assistant, and a pattern-filled shape, saved twice).

Nothing needed fixing in this run.

Results: `~/.cache/realworld-out/pre0130-2026-11-18/` (a second sweep with
a clean rebuild of the probe, identical apart from one timing flag:
`pre0130-2026-11-18-b/`).

## Eighth run, before v0.14.0 (2026-11-18)

Rerun of all 424 files on main at `f7d65e9` (after Sheets' grid in Excel's
pixel sizes, styled columns written with the default width, `.xlsb` Excel
2010 conditional formats and average rules, `.ods` array formulas and more
`.xls` names imported as formulas, Docs exact line spacing with a new
baseline, the column ruler, notes and spacing at column tops, pictures cut
to their shapes, sparklines drawn on screen and in PDFs, plain cells drawn
per row in Sheets, gpui-kit menus shielding hover, and Circular Picture
Callout had merged), release build, compared file by file with the seventh
run.

| | seventh run | main, `f7d65e9` |
|---|---:|---:|
| crash, panic or timeout | 0 | 0 |
| refused to open | 6 | 6 |
| saved file Office would repair | 0 | 0 |
| content changed by a save | 0 | 0 |
| page count changed by a save (Docs, Slides) | 0 of 199 | 0 of 199 |
| ok | 418 | 418 |
| slowest file | 4.6 s | 4.0 s |
| all files' probe time added up | 78 s | 66 s |

What changed:

- **Formulas.** LibreOffice's tdf112780.ods keeps its 33 matrix formulas
  (CSE array formulas now; the other cells of their areas are results, not inputs,
  so the file counts 1,039 cells instead of 1,196) and POI's 49612.xls its 2
  formulas that name a local name through `PtgNameX`; both "kept as values"
  notes are gone. Every cached value in the 21 `.ods` files was compared with
  Kantor's (numbers, booleans, formula results): none differ. The 4,623
  `.xls` formulas xlrd reads still match the files' results, after one fix
  (below).
- **Printed pages and sizes.** Every workbook prints the same number of
  pages as in the seventh run (12,112 in all), and every Docs and Slides
  file too, including the files with exact line spacing (the new baseline
  moves text inside its line only). Column widths, row heights and
  `sheetFormatPr` saved from all 220 workbooks that open are identical to
  what v0.13.0 saved (both probes run on every file).
- **Timing**, against a probe built from the `v0.13.0` tag (min of 5–15
  interleaved runs): the 12 slowest files, the sparkline workbook (its PDF
  now draws the sparklines: 65 → 66 ms), the decks with shaped pictures or
  picture fills, and the documents with the most exact-spaced lines took the
  same time (0.96–1.03×).

**Fixed:** POI's 30978-deleted.xls has `=SUM(On2)` where `On2` names cells on
a deleted sheet; the file's result is #REF!. Since `On2` is a cell in
today's grid, the name now gets Excel's underscore and the formula came in as
`=SUM(_On2)`, but IronCalc doesn't take a name that is `#REF!`, so the
formula showed #NAME?. The import check took any error for the file's error;
a formula's error must now be the same one as the file's, else the cached
value is kept (as in v0.13: #REF!, "1 formula kept as values"). Test:
sheets_engine `import::tests::a_cached_error_must_be_the_same_error`. The
sweep's other files gave the same results with the stricter check.

The Office validation scenarios (`tools/fidelity/office_check.py`): 214
saved files, 0 new Open XML SDK errors, all converted to PDF by LibreOffice.
Five scenarios were added for markup v0.14 writes:
`sheets_columns_resized.xlsx` (column widths set as a header drag and as an
autofit leaves them, in odd pixels, and row heights; reopened at the same
pixels, saved twice), `sheets_styled_column.xlsx` (a column with a style of
its own and no width, in a sheet naming its default width: written with
that width and `customWidth="0"`, the same width after two saves),
`sheets_sparklines_saved.xlsx` (the sparkline fixture edited and saved
twice), `columns_ruler_drag.docx` (the space between two columns dragged on
the ruler, equal and then unequal columns, saved twice) and
`circular_picture_callout.pptx` (a SmartArt changed to Circular Picture
Callout and given an item, saved twice).

Results: `~/.cache/realworld-out/pre0140-2026-11-18/` (the run on main;
`fixed/` the rerun with the fix, `office-check/`, `ab1.txt`, `widths.txt`,
and the helper scripts in `tools/`: `odsvals.py`, `widths.py`, `ab.py`).

## Ninth run, before v0.15.0 (2026-11-18)

Rerun of all 424 files on main at `d1922b3` (after Docs SmartArt picture
fills, inline pictures cut to their shapes, inline SmartArt and
top-and-bottom floats moving to the next page; Sheets picture outlines along
shapes and Move or Copy keeping them; pattern fills on whole device pixels;
names and formulas on deleted sheets giving #REF!; the first wheel notch on
X11 had merged), release build, compared file by file with the eighth run.

| | eighth run | main, `d1922b3` |
|---|---:|---:|
| crash, panic or timeout | 0 | 0 |
| refused to open | 6 | 6 |
| saved file Office would repair | 0 | 0 |
| content changed by a save | 0 | 0 |
| page count changed by a save (Docs, Slides) | 0 of 199 | 0 of 199 |
| ok | 418 | 418 |
| slowest file | 4.0 s | 3.9 s |
| all files' probe time added up | 63 s | 64 s |

What changed:

- **Formulas.** POI's 30978-deleted.xls keeps `=SUM(_On2)` as a formula
  showing #REF! (its name on a deleted sheet is saved as Excel's
  `#REF!$A$1:$A$3`); its "1 formula kept as values" note is gone. Nothing
  else changed: kinds, counts, import notes and validator results are the
  same file by file. The 4,623 `.xls` formulas xlrd reads still match the
  files' results, and the `.ods` cached values compare as with v0.14.0
  (the same text-spacing noise in one file and covered.ods's covered cell,
  identical with both probes).
- **Printed pages and sizes.** Every workbook, document and deck prints the
  same number of pages as in the eighth run (12,739 in all, including the
  199 Docs and Slides files). Column widths, row heights and
  `sheetFormatPr` saved from all 220 workbooks that open are identical to
  what v0.14.0 saved.
- **Timing**, against a probe built from the `v0.14.0` tag (min of 5–15
  interleaved runs): the 12 slowest files, documents with SmartArt, group
  shapes and comments, decks with shaped pictures, and workbooks with
  pictures and sparklines took the same time (0.88–1.04×; a 46 ms comments
  document at 1.11× was 1.00× over 15 runs), except one.

**Fixed:** a Docs PDF export slowdown. A manual with 87 pictures in the text
(MANUAL-BOOK.docx) exported 3 % slower (787 → 810 ms, steady over several
rounds of 11–15 runs). Two of its pictures have Word's framed picture style
with a blurred shadow. Working out how an inline picture is cut and
outlined went through the floating-picture painter, which also blurred the
shadow on the CPU, and then dropped it (inline pictures don't draw their
shadow). On screen this ran on every paint of such a picture. The shadow
is now left out there: 786 ms, as v0.14.0. Test: docs_pdf
`floats::tests::an_inline_pictures_look_leaves_its_shadow_out`. The sweep
rerun with the fix gave the same results file by file.

The Office validation scenarios (`tools/fidelity/office_check.py`): 225
saved files, 0 new Open XML SDK errors, all converted to PDF by LibreOffice.
Three scenarios were added for markup v0.15 writes:
`sheets_shaped_pictures_copied.xlsx` (28_shaped_pictures.xlsx's sheet
copied with Move or Copy, its pictures' shapes and outlines checked after
reopening, the copy copied again and saved twice),
`sheets_deleted_sheet_name.xlsx` (08_defined_names.xlsx with the sheet its
names point at deleted in Kantor: `SUM(Prices)` shows #REF! before and after
saving, saved twice) and `smartart_inline_resized.docx` (the inline
SmartArt fixture resized to 3/4 and saved twice). Kantor saves a name whose
sheet was deleted in Kantor as `#REF!`; Excel writes `#REF!$B$3:$B$6`
(both valid, both give #REF!).

Results: `~/.cache/realworld-out/pre0150-2026-11-18/` (the run on main;
`fixed/` the rerun with the fix, `office-check/`, `ab1.txt`, `ab2.txt`,
`widths.txt`, `xlsvals.txt`, `odsvals.txt`, `ab.py`).

## Left

Refused (6):

- 3 Word 6/95 `.doc` files (refused by design).
- 2 `.docx` from docx4j's tests that aren't valid WordprocessingML.
- 1 `.xlsb` that calamine can't read (an error, not a crash).

Import notes left (content Kantor doesn't bring in): formulas kept as their
values in 2 `.xls`/`.ods` files (17 formulas: 8 in tdf170565_empty_functions.ods
and 9 in ex45046-21984.xls; 30978-deleted.xls's name on a deleted sheet comes
in as a formula showing #REF! since v0.15, tdf112780.ods's matrix formulas and
49612.xls's since v0.14, the 11 3-D formulas of POI's
55906-MultiSheetRefs.xls since v0.13), 2 controls in SimpleWithAutofilter.xls, the
formatting of one damaged `.xls` (cfb_dir_short_chunk_panic), and 13 `.ods`
sheet names Excel doesn't allow (renamed). The `.ods` polygon of tdf137216
imports since v0.12.
