# Real-world file sweep

424 public Office files, each opened, saved, reopened and exported to PDF by
Kantor's engines, with the saved files checked by Microsoft's Open XML SDK.
First run: 2026-11-15.

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
`results.csv` and `summary.md`. Each file gets the first matching kind:
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

## Left

Refused (11; 19 before Strict Open XML was read):

- 3 Word 6/95 `.doc` files (refused by design).
- 3 `.xlsx` packages laid out unusually: the workbook part not at
  `xl/workbook.xml` (2), part names with backslashes (1).
- 2 `.ods` files with pretty-printed XML (whitespace between cells): the
  `.ods` reader (calamine 0.38) stops on it. LibreOffice itself never writes
  that.
- 2 `.docx` from docx4j's tests that aren't valid WordprocessingML.
- 1 `.xlsb` that calamine can't read (now an error, not a crash).

Saved file Office would repair (1): a LibreOffice test workbook whose cells
refer to formats past the end of its format list (Excel repairs the
original too); Kantor keeps those references as they were.

Not checked: layout against Office (fonts, line breaks, page breaks), charts
and pictures beyond "the PDF was written", and what the first open misreads.
These need a visual comparison with Office or LibreOffice renderings, which
this sweep doesn't do yet.
