# Kantor patches to ironcalc_base 0.9.1 (MIT OR Apache-2.0)

Vendored from crates.io `ironcalc_base` 0.9.1 and wired in with `[patch.crates-io]`
in the workspace `Cargo.toml`. The `ironcalc` crate (xlsx import/export) still
comes from crates.io and picks this copy up through the patch. Changes are marked
`KANTOR PATCH` in the source.

## 1. Fast lookups (per-pass lookup cache)

Why: a 28 MB customer workbook with ~185k `VLOOKUP(x, Other!$A:$B, 2, 0)` (one per
row) loaded in 7 s but `Model::evaluate()` ran for more than 10 minutes. Upstream
VLOOKUP/HLOOKUP/MATCH/LOOKUP materialise their whole search vector on every call
(`LookupTable::first_column`, `as_vector`), cell by cell through `evaluate_cell`,
and scan it linearly: quadratic in the number of rows. Worse, they do not clip a
whole-column range to the used area, so `A:B` meant 1,048,576 reads per call, each
one also put on record in `Evaluation::seen`. With the patch the same workbook
evaluates in a few seconds with results identical to upstream (before section 2).

What (`src/functions/lookup_cache.rs`, documented at the top of the file):

* `Model::search_vector` reads a single row/column once per evaluation pass and
  keeps it, keyed by `(sheet, row1, column1, row2, column2)`. Only the part inside
  the sheet's used area is read; the rest is a virtual tail of empty cells that is
  put on record in one go (`record_seen_empty_range`), so binary searches and
  exact matches see exactly the vector upstream built.
* Exact-match searches use a hash index built on first use, with the equality of
  the scan they replace: VLOOKUP/HLOOKUP/MATCH match text as a wildcard pattern,
  case-insensitively (lowercase); other values with `compare_values` (VLOOKUP,
  HLOOKUP) or `values_are_equal` (MATCH); XLOOKUP/XMATCH exact mode use
  `compare_values` throughout (uppercase text, numbers rounded to 15 digits, empty
  equal to 0/""/FALSE). Text with `*`, `?` or `~`, numbers below 1 (2 for
  `values_are_equal`), booleans, blanks and errors are scanned linearly, so
  wildcards and tolerance-based number equality behave as before. Duplicates
  return the first (XLOOKUP/XMATCH search mode -1: the last) position.
* Used by VLOOKUP, HLOOKUP, MATCH, LOOKUP, XLOOKUP and XMATCH (range arguments;
  in-formula arrays take the old path). Approximate/binary searches go through the
  same vectors (but see section 2).
* SUMIF/SUMIFS/AVERAGEIF(S)/MAXIFS/MINIFS (`run_ifs`) and COUNTIF/COUNTIFS (their
  own loop) over a single column read their first criteria range as a cached vector. Later
  criteria ranges, which upstream reads only for rows that pass the earlier ones,
  use a vector only if reading it evaluates nothing (`evaluated_search_vector`:
  every cell constant, blank or already evaluated, and the workbook has no dynamic
  arrays).
* `Worksheet::dimension()` walks every cell of a sheet; `clip_to_used_area` and
  `apply_ifs` called it per formula. It is kept per pass (`Model::used_dimension`).

Validity (why results are identical to upstream):

* The cache lives only while a pass runs: it is cleared at the start and at the
  end of `run_pass`, so every restart and every evaluation after an edit starts
  empty. Outside a pass nothing is cached.
* A vector is not kept if building it hit a cycle (`Evaluation::cycle_reads`), a
  pending restart or an unwind; it is not handed out if a cell on the evaluation
  stack lies in its range. Formula cells it read were evaluated and are final.
* A dynamic array that writes or removes spill cells drops all vectors of its
  sheet and its used area; a CSE array drops the vectors its area overlaps and
  grows the used area.
* Reads put on record by the reader that built a vector cover later readers: a
  spill into the range after the build restarts the pass as before.
* `Evaluation::lookup_cache_enabled = false` turns it all off; the tests in
  `src/test/test_kantor_lookup_cache.rs` evaluate each workbook both ways and
  compare every cell (duplicates, case, number vs text "1", errors, blanks,
  wildcards and `~`, approximate match, HLOOKUP, cross-sheet whole columns,
  self-referencing ranges, dynamic spills into a lookup range, *IFS, edits between
  evaluations). `lookup_cache::tests` checks the hash indexes against the linear
  scans over random vectors.

## 2. Approximate lookups ignore trailing blanks (Excel compatibility)

Why: upstream binary-searched the whole range of an approximate lookup and read
the empty cells below the data as 0 or "" (`compare_values`). `VLOOKUP(x, A:C, 2,
TRUE)` over whole columns, or over a range longer than its data, then landed on a
blank row and returned 0 where Excel returns the matching row. In the customer
workbook every one of the 12,574 formula cells (of 767,272) that differed from the
values Excel stored traced back to two such columns; with this change none differ.

What: `binary_search_on_vector` (VLOOKUP, HLOOKUP and LOOKUP in sorted mode, MATCH
with match type 1) and MATCH with match type -1 search up to the last non-empty
cell of the vector (`SearchVector::sorted_len`). Blank cells between values are
still read as upstream reads them. In-formula arrays and XLOOKUP/XMATCH binary
modes are unchanged. Unlike section 1 this changes results, deliberately; drop it
separately if upstream fixes the behaviour (or if it ever disagrees with Excel).

## 3. build.rs

Upstream runs `git describe` for `INFO("release")`. Inside the Kantor repository
that would report Kantor's commit, so the value a crates.io build gets
(`unknown`) is fixed instead.

## 4. Public style lookup

`Styles::get_style(index)` is `pub` (upstream: `pub(crate)`). The Sheets grid
resolves each style index once and caches it (`sheets_engine::look`), instead of
cloning a whole `Style` (font name, number format, border items) through
`Model::get_style_for_cell` for every visible cell on every frame.

## 5. Packed rows (memory)

Why: a generated 32 MB workbook (180k rows, 1.62M formulas) held 211 MB of live
heap after load, 184 MB of it the rows' `Vec<(i32, Cell)>`: 56 bytes a cell, because
`Cell` is as large as its largest kind (`ArrayFormula`, 48 bytes) and most cells
need far less. That made Kantor ~410 MB with the file open on an 8 GB Mac.

What (`src/sheet_data/packed.rs`, and `KANTOR PATCH` hunks in `sheet_data/`):

* `Row::Packed(Vec<Packed>)`, next to upstream's `Row::Cells` and `Row::Numbers`:
  a private 32-byte enum carrying the column and one of empty, boolean, number,
  error, shared string, or a plain formula (`CellFormula`) with its value split by
  kind; text values are `Box<str>`. `set_row` (the xlsx importer, bitcode loading)
  packs every row whose cells all pack; a row with an `ArrayFormula` or
  `SpillCell` stays a row of cells, and inserting one turns a packed row into
  cells. A row of numbers given another cell that packs becomes packed.
* Like a row of numbers, a packed cell is a `Cell` only while looked at:
  `cell()`/`cells()` hand out `Cow::Owned` (a text value is copied); `cell_mut`
  turns the row into cells, so the writers that run per cell avoid it:
  evaluation stores a plain formula's value with `update_cell` (`model.rs`),
  `Worksheet::set_cell_style` uses `SheetData::set_cell_style` (in place).
* The full-sheet scans of every evaluation don't build cells for packed rows:
  `SheetData::for_each_formula` (formula positions) and `SheetData::array_cells`
  (dynamic anchors and spills, which only a row of cells can hold).
* Saved bytes (`saved.rs`) are the cells, as for rows of numbers: the format is
  unchanged.

Effect on that workbook: 257 → 182 MB resident after load (engine only),
evaluation 1.90 → 1.95 s, every formula value identical (`eval_timing --dump`).
Tests: `sheet_data/test.rs` (`KANTOR PATCH` section).

## 6. Read records only for dynamic arrays, freed after evaluation

Why: `Evaluation::seen`/`seen_ranges` record every empty position a formula reads,
for every pass, but only a dynamic anchor's spill (`spill_contradicts_a_read`)
ever asks; `run_pass` only `clear()`ed them, so their capacity stayed allocated.

What: `Evaluation::record_reads` is set per pass to "the workbook has dynamic
anchors" (they all exist before the first pass: `sync_anchor_order`); without it
nothing is recorded. When `evaluate()` ends, both are replaced by empty ones.
`last_records` (tests only) keeps what the last pass recorded:
`test_range_reads.rs` now puts a dynamic anchor in its whole-column test and checks
that nothing is recorded without one. `Evaluation::cells` (each formula's state)
is kept as upstream keeps it: reads outside a pass use it to return stored values.

## 7. Model fields for xlsx fidelity

Why: the xlsx reader (`vendor/ironcalc`, its KANTOR_PATCHES.md §1) had nowhere
to put some formatting, so it was lost on save.

What (`src/types.rs`, `KANTOR PATCH (fidelity)`):

* `BorderStyle` gains `Hair`, `Dashed`, `DashDot`, `DashDotDot` (added last:
  bitcode numbers variants), with `ooxml_name()` / `from_ooxml()` for the
  case-sensitive OOXML names. `Display` and serde keep upstream's lower case.
* `Alignment` gains `indent`, `text_rotation` (OOXML's encoding) and
  `shrink_to_fit` (serde defaults, skipped when unset).
* `CellProtection { locked, hidden }`, as `Style::protection` and
  `CellXfs::protection` (`None` = Excel's default, locked and shown); carried
  through `Styles::create_new_style`, `get_style`, `get_style_index` and
  applying a named style (`src/styles.rs`), so restyling a cell keeps it.
  `builtin_styles.rs` gets `protection: None` in every literal.
* `Row` and `Col` gain `outline_level` and `collapsed`; splitting a column
  range keeps them (`src/worksheet.rs`); other literals (including tests) set
  0 / false.

Struct literals in the tests got `..Default::default()` or the new fields.

## 8. Defined names follow inserted and deleted rows and columns

Why: inserting or deleting rows or columns moved cells, formulas, merges,
links and conditional formats, but left defined names pointing at the old
cells: a print area, a filter range (`_xlnm._FilterDatabase`) or a named range
used by formulas then covered the wrong cells, in the model and in the saved
file.

What: `Model::displace_defined_names` (`src/actions.rs`) runs next to
`displace_cf_ranges` for every displacement (insert/delete/move of rows and
columns) and re-writes each name's formula with `to_string_displaced`, like a
conditional format's (references into deleted cells become `#REF!`). Deleting
can't be undone by re-inserting, so `UserModel::delete_rows`/`delete_columns`
put `Diff::UpdateDefinedName` snapshots before the delete
(`defined_names_snapshot_diffs` in `src/user_model/common.rs`, like the merged
cells snapshot): undo restores the old formulas once the rows are back.
Tests: `src/test/user_model/test_kantor_defined_names_move.rs`.

## 9. Conditional formatting evaluated by the caller

Why: upstream evaluates every conditional formatting rule over every cell of
its range on load, after each `evaluate()` and on theme changes
(`Model::evaluate_conditional_formatting`), keeping a result per cell. On a
90k-row sheet with a few rules that is a walk over every rule's range (an
expression rule runs its formula per cell) after every edit, and a hash map
entry per formatted cell. Kantor evaluates rules lazily for the cells on
screen (`crates/sheets_engine/src/cond_format.rs`). Upstream's evaluator also
evaluates `cellIs` operands once at A1 (no relative references, numbers only)
and lets a lower-priority `stopIfTrue` rule replace a higher one's result.

What (`src/kantor_cf.rs`, `KANTOR PATCH (conditional formatting)`):

* `set_eager_conditional_formatting(false)` turns the eager pass into a no-op
  (process-wide; on by default, so upstream's tests are unchanged).
* `Model::cf_parse` / `UserModel::cf_parse` parse a rule formula (English) as
  written in a given cell; `cf_evaluate` evaluates it for another cell
  (relative references shift) and returns a public `CfValue`.
* `Evaluation::stored_only` (`src/evaluation.rs`): while set, a formula or
  spill cell read gives its stored value instead of being evaluated, so
  painting never recalculates (or changes) the workbook.

## Updating or dropping

Upstream: https://github.com/ironcalc/IronCalc. Drop this copy (remove the
`[patch.crates-io]` line and `vendor/ironcalc_base`) once upstream caches lookup
vectors or indexes exact matches itself and stores cells about as compactly as
section 4 (compare `cargo run --release -p sheets_engine --example open_memory --
<big.xlsx> --recalc`); time
`cargo run --release -p sheets_engine --example eval_timing -- <big.xlsx>` before
and after. To move to a newer upstream with the patch still needed: copy the new
crate over this directory, reapply the `KANTOR PATCH` hunks (the new file
`functions/lookup_cache.rs` and `test/test_kantor_lookup_cache.rs`, plus small
call-site edits in `evaluation.rs`, `model.rs`, `functions/mod.rs`,
`functions/lookup_and_reference/{mod,xmatch}.rs`, `functions/xlookup.rs`,
`functions/statistical/if_ifs.rs`, `test/mod.rs`, `build.rs`; for sections 5
and 6 the new file `sheet_data/packed.rs` and edits in `sheet_data/{mod,row,chunks,
saved,test}.rs`, `worksheet.rs`, `test/dynamic_evaluation/test_range_reads.rs`; for
section 7 `types.rs`, `styles.rs`, `builtin_styles.rs`, `worksheet.rs`,
`user_model/common.rs` and test literals; for section 8 `actions.rs`,
`user_model/common.rs`, `test/user_model/{mod,test_kantor_defined_names_move}.rs`)
and run the crate's
tests in it (`cargo test`, also with `IRONCALC_MAX_DEPTH=1`). The crate's
`Cargo.lock` is upstream's and only used when testing in this directory.
With it, `test::user_model::test_to_from_bytes::errors` fails also without any
patch (the bitcode error message changed); everything else passes.
