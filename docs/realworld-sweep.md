# Real-world file sweep

424 public Office files, each opened, saved, reopened and exported to PDF by
Kantor's engines, with the saved files checked by Microsoft's Open XML SDK.
First run: 2026-11-15; second run on main: 2026-11-16.

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
| Indonesian public GitHub projects (budget workbooks, reports, overtime and election data) | 25 | as published |
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

## Left

Refused (6):

- 3 Word 6/95 `.doc` files (refused by design).
- 2 `.docx` from docx4j's tests that aren't valid WordprocessingML.
- 1 `.xlsb` that calamine can't read (an error, not a crash).

One `.ods` (tdf137216) has a polygon shape that isn't imported (an import
note says so).
