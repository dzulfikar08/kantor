# Kantor patches to ironcalc 0.9.1 (MIT OR Apache-2.0)

Vendored from crates.io `ironcalc` 0.9.1 (the xlsx reader and writer) and wired
in with `[patch.crates-io]` in the workspace `Cargo.toml`, next to the patched
`vendor/ironcalc_base`. The first commit is the crate unmodified, including its
test workbooks under `tests/` (upstream's, same licence); changes are marked
`KANTOR PATCH` in the source.

## 0. Cargo.toml

`ironcalc_base` is taken from `../ironcalc_base` (with the version kept), so
the crate builds against the patched model also when tested in this directory.
`tests/test_kantor_fidelity.rs` is registered as a test target.

## 1. Fidelity of styles and outlines

Why: Kantor promises that opening and saving a workbook drops nothing
(plan.md §5). The reader turned every border style other than thin, medium,
thick and double into thin (it matched lower-case names such as
`mediumdashed`, while files say `mediumDashed`; `dotted`, `dashed`, `hair`,
`dashDot`, `dashDotDot` weren't matched at all), read the diagonal flags from
attributes that don't exist (`diagonal_up`), and dropped cell indent, text
rotation, shrink-to-fit, cell protection (unlocked cells, hidden formulas) and
row/column outline levels. The writer wrote border styles in lower case
(invalid for Excel).

What (model fields are in `vendor/ironcalc_base`, its KANTOR_PATCHES.md §7):

* `import/styles.rs`: border styles through `BorderStyle::from_ooxml` (every
  ST_BorderStyle; `none` or unknown is no border), `diagonalUp`/`diagonalDown`,
  `<alignment indent textRotation shrinkToFit>`, `<protection locked hidden>`
  (stored only when it differs from Excel's default, locked and shown).
* `export/styles_util.rs`, `export/styles.rs`: border styles by
  `BorderStyle::ooxml_name`, the diagonal flags, the alignment attributes, and
  `<protection>` with `applyProtection="1"`.
* `import/worksheets.rs`, `import/sheet_data.rs`, `export/worksheets.rs`:
  `outlineLevel` and `collapsed` on `<col>` and `<row>` (a row with only an
  outline level is now kept as a row record).

Tests: `tests/test_kantor_fidelity.rs` (all 13 border styles with diagonals,
indent/rotation/shrink, protection, row and column outlines through a
write/read cycle), and the Kantor corpus (`tests/corpus/xlsx/17_cell_styles.xlsx`,
`16_print_protect_outline.xlsx`, checked by `crates/sheets_engine/tests/corpus.rs`).

Not done: the hyperlink `display` attribute (IronCalc's `Link` has no display
text; Excel shows the cell's value anyway), fill patterns other than solid,
underline styles other than single, superscript/subscript: the model has no
place for them.

## 2. One copy of a sheet's cells while writing (memory)

Why: `get_worksheet_xml` collected every row as a `String`, joined them
(second copy) and `format!`ed the sheet around the result (third copy). For
the generated 90k-row workbook (a 108 MB `sheet1.xml`) saving peaked at 391 MB
in the writer alone, with the model taking 96 MB.

What (`export/worksheets.rs`): rows are written straight into one string, and
the start and end of the sheet are put around it in place
(`insert_str`/`push_str` after one `reserve_exact`). The XML is byte for byte
the same. Writer peak for that workbook: 391 → 216 MB.

## 3. Typed cells without a value are empty

Why: openpyxl (and other writers) store a styled cell that has no value as
`<c r="B3" s="5" t="n"/>`. The reader parsed the missing `<v>` as the number
0, so the cell showed "0" where Excel shows nothing.

What (`import/sheet_data.rs`): a `<c>` with a `t` attribute but no `<v>`, no
`<f>` and no inline string is read as an empty cell that keeps its style (it
is written back as `<c r="B3" s="5"/>`).

Test: `styled_empty_cells_with_a_type_stay_empty` in
`tests/test_kantor_fidelity.rs`.

## 4. Conditional formatting details

Why: icon sets with "Reverse Icon Order" were read in the normal order, and a
differential format's `<b val="0"/>` (also `i`, `u`, `strike`) turned the
property on.

What: `import/conditional_formatting.rs` reverses the icons of an `iconSet`
with `reverse="1"` (main and x14 rules); `import/styles.rs` (`parse_dxf`)
reads the `val` of `b`, `i`, `u` and `strike`. Kantor doesn't use the writer's
conditional formatting for workbooks opened from `.xlsx`: the original markup
and `<dxfs>` are carried instead (`crates/sheets_engine/src/preserve.rs`),
except on sheets whose rules were changed in Kantor (2026-11-12,
`crates/sheets_engine/src/cf_edit.rs`), which are written by this writer;
`export/conditional_formatting.rs` now also writes the `operator` of text rules
(`containsText`, `notContains`, `beginsWith`, `endsWith`) as Excel does.

## 5. Default row height

Why: Kantor draws rows at their heights from the file, relative to the
sheet's default row height, which upstream doesn't read.

What: `import/worksheets.rs` reads `<sheetFormatPr defaultRowHeight>` into
`Worksheet::default_row_height` (points; `vendor/ironcalc_base` §11). The
writer doesn't write `sheetFormatPr`; Kantor carries the original element on
save (`crates/sheets_engine/src/preserve.rs`).

## 6. Tables and AutoFilters

Why: Kantor filters tables and sheets and keeps the state in the model
(`vendor/ironcalc_base` §13). Upstream also looked for `<tableInfo>` instead of
`<tableStyleInfo>`, so every table lost its style name and banding flags.

What: `import/tables.rs` reads `tableStyleInfo` and keeps the table's
`<autoFilter>` (`Table::kantor_auto_filter`); `import/worksheets.rs` keeps the
sheet's (`Worksheet::kantor_auto_filter`), serialised by `XmlNode::to_xml`
(`import/xml.rs`, unprefixed attributes only); `export/worksheets.rs` writes
the sheet's after `</sheetData>`. Tables are not written by the writer: Kantor
carries their parts and updates them from the model
(`crates/sheets_engine/src/preserve.rs`, `fit_table`).

Also (2026-11-12, for Kantor's total row functions): `tableColumn`'s
`totalsRowLabel` and `totalsRowFunction` are read (upstream left them `None`),
and `dataDxfId` is read from `dataDxfId` (upstream read `headerRowDxfId`).
Tables made or changed in Kantor are written from the model by
`crates/sheets_engine/src/table_edit.rs` (`table_xml`).

Known: `tests/test.rs` `test_dynamic_arrays` fails since §5 (the file's
default row height is now read); `test_no_export_tables` passes.

## 7. Each filled-down formula is parsed once (fast open)

Why: `from_a1_to_rc` parsed every formula cell and printed it in R1C1; on
the generated 90k-row workbook (`scripts/corpus/gen_big_xlsx.py`, 810k
formula cells, 36 distinct R1C1 formulas) that was ~80% of the open.

What (`import/worksheets.rs` `RcCache`, used by `import/sheet_data.rs`):
each sheet keeps a map from `Parser::kantor_rc_key` (vendor/ironcalc_base
§17) to the R1C1 text; a formula whose key is known is only lexed. Results
that contain the formula's own text (parse errors keep it as written) are
not shared; formulas the key refuses take the old path.

Test: `import::worksheets::tests::rc_cache_matches_parsing_each_formula`.

## Updating or dropping

Upstream: https://github.com/ironcalc/IronCalc. Drop this copy (remove the
`ironcalc` line under `[patch.crates-io]` and `vendor/ironcalc`) once upstream
reads and writes all border styles, alignment, protection and outlines, and
writes sheets without copying them (§2). To
move to a newer upstream: copy the new crate over this directory, reapply the
`KANTOR PATCH` hunks and `tests/test_kantor_fidelity.rs`, and run the crate's
tests here (`cargo test --no-fail-fast`). Known upstream failures with 0.9.1,
with and without any patch (they depend on today's date):
`test_calc_DATE_AND_TIME_NOW_TODAY` and `test_templates_{event_calendar,
gantt_project_tracker, invoice, weekly_timesheet, yearly_calendar}` in
`generated_tests`. Expected with §5: `test_dynamic_arrays` (`tests/test.rs`) saves
with this crate's writer alone and reloads; the writer doesn't write
`<sheetFormatPr>`, so `default_row_height` comes back `None`. Kantor's save
carries the original `sheetFormatPr` (`crates/sheets_engine/src/preserve.rs`,
which keeps the default column width too), and
`crates/sheets_engine/tests/rows.rs` checks the height survives a save.
