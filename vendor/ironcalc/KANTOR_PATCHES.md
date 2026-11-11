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
and `<dxfs>` are carried instead (`crates/sheets_engine/src/preserve.rs`).

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
`generated_tests`.
