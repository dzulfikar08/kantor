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

## 10. Functions upstream lacks, and two fixes

Why: workbooks using these showed `#NAME?`. Against Excel's function list (and
GridCraft's, which has the same ones), upstream 0.9.1 lacks only AGGREGATE,
GROUPBY, PIVOTBY, BAHTTEXT, USDOLLAR, DBCS, JIS, PHONETIC, EUROCONVERT,
FILTERXML, WEBSERVICE, the CUBE* functions, GETPIVOTDATA (added later, section
14) and the server/online ones (RTD, STOCKHISTORY, IMAGE, TRANSLATE, ...). The first eight are
added; the rest need data Kantor doesn't have (OLAP cubes, pivot caches, the
network). Marked `KANTOR PATCH (functions)`.

What:

* Names: upstream keeps every name in `language/language.bin` (bitcode, built
  outside the crate). Kantor's are in `functions/kantor_names.rs` instead, English
  in every language: `Functions::lookup` falls back to it, `to_localized_name`
  uses it, `to_xlsx_string` writes `_xlfn.AGGREGATE`, `_xlfn.GROUPBY`,
  `_xlfn.PIVOTBY`, `_xlfn.DBCS`. New `Function` variants are at the end of the
  enum and of `into_iter()`; `static_analysis.rs` has their signatures.
  `all_function_names()` (public) lists every name, for Kantor's autocomplete.
* `functions/kantor_aggregate.rs`: AGGREGATE, 19 functions, options 0–7. Hidden
  rows (by hand or a filter) are skipped for options 1/3/5/7; nested cells are
  those whose formula contains SUBTOTAL or AGGREGATE anywhere (`has_subtotal`);
  ranges are read down to the sheet's used area (the rest put on record for
  dynamic arrays). Functions 1–13 give #VALUE! for a computed array, as Excel.
* `functions/kantor_groupby.rs`: GROUPBY and PIVOTBY (spilled arrays): automatic
  header detection (shown with several group levels), totals and subtotals at
  either end, sort by key columns (vector allowed) or a value column, within the
  hierarchy; filter array; `field_relationship` 1; `relative_to` 0–4 for a
  two-argument function (PERCENTOF or `LAMBDA(part,all,...)`). Text keys group
  regardless of case; blanks in the result are written as empty text, since a
  spilled blank shows as 0. A vector of functions is not supported (#VALUE!). A
  whole-column reference is read to the last used row plus one blank row.
* Eta LAMBDAs (a function passed by name, `GROUPBY(...,SUM)`): the parser drops
  `_xleta.` (the result is an unbound `NamedVariableKind`, as a bare `SUM` was
  already); `Model::lambda_arg` turns such a name into a LAMBDA of the arity the
  caller passes. Used by GROUPBY/PIVOTBY and BYROW, BYCOL, MAKEARRAY, MAP,
  REDUCE, SCAN (`functions/logical/*`). On export `prefix_bound_variables`
  (`stringify.rs`) writes an unbound function name as `_xleta.NAME`. Elsewhere
  the bare name is still #NAME?.
* `functions/kantor_text.rs`: BAHTTEXT (LibreOffice's Excel-compatible algorithm),
  USDOLLAR (= DOLLAR), DBCS/JIS (ASCII and space to full width, the reverse of
  ASC), PHONETIC (the cell's text: no phonetic guides are kept).
* Fixes: `SUBTOTAL(110|111, ...)` skipped hidden rows only for the other codes;
  SUBTOTAL skipped only cells whose formula *starts* with SUBTOTAL (now any
  SUBTOTAL/AGGREGATE inside, `subtotal.rs`); COUNT counted no numbers in an array
  (`count_and_average.rs`).

Tests: `src/test/test_kantor_functions.rs` (Microsoft's AGGREGATE example, every
AGGREGATE function and option kind, BAHTTEXT, GROUPBY/PIVOTBY layouts and
errors, eta LAMBDAs), `src/expressions/parser/tests/test_kantor_functions_export.rs`
(xlsx names and `_xleta.`), `crates/sheets_engine/tests/functions.rs` (save and
reopen an xlsx).

## 11. Row heights in points, undone exactly

Why: Kantor's grid draws per-row heights and lets the user set them.
Upstream's `UserModel::set_rows_height` takes IronCalc pixels, always sets
`custom_height`, leaves a hidden row hidden, can't make a row automatic again,
and its undo restores only the height (the row stays custom, and a hidden row
comes back 0 high because `get_row_height` reports hidden rows as 0). It also
looks every row up by scanning the sheet's row entries.

What (`KANTOR PATCH (row heights)`):

* `Worksheet::default_row_height: Option<f64>` (`src/types.rs`, `None` in
  `new_empty.rs`): the file's default row height in points, filled by the
  vendored reader (`vendor/ironcalc` §5).
* `src/kantor_rows.rs`: `UserModel::kantor_set_rows_height(sheet, first,
  last, Some(points) | None, auto_points)`, one undo step. A custom height
  shows a hidden row (as in Excel); `None` makes rows automatic again (an
  entry kept only for its height is removed). Rows are indexed once per call.
* `Diff::SetRowEntry { old_value, new_value }` (`src/user_model/history.rs`)
  holds the row's whole entry (or `None`); undo/redo put it back as it was
  (`src/user_model/undo_redo.rs`).

Tests: `crates/sheets_engine/src/rows.rs` and `crates/sheets_engine/tests/rows.rs`.

## 12. Data validation helpers

Why: Kantor applies Excel data validation itself (`crates/sheets_engine/src/validation.rs`;
IronCalc doesn't model it). A list's source is a range (`$A$1:$A$5`, `Lists!A:A`, a defined
name, `INDIRECT`/`OFFSET`), which `cf_evaluate` reduces to `CfValue::Other`; a value the
validation rejects must be taken back without leaving a redo.

What (`src/kantor_validation.rs`, `KANTOR PATCH (data validation)`):

* `Model::cf_evaluate_ref` / `UserModel::cf_evaluate_ref`: like `cf_evaluate` (stored values
  only), but a range result comes back as `CfRef::Range { sheet, top, left, bottom, right }`
  and an array as `CfRef::Array`. `CfFormula::node` (crate-private) gives it the parsed node.
* `Model::kantor_input_value`: the value `set_user_input` would store for a typed string
  (number formats, booleans, error names, quote prefix), without storing it; `None` for a
  formula.
* `UserModel::kantor_forget_redo` (`src/user_model/common.rs`): clears the redo stack.

Tests: `crates/sheets_engine/tests/validation.rs`.

## 13. Excel tables: structured references, geometry, sort and filter state

Why: upstream turned a structured reference into a fixed range when the
formula was parsed: `Sales[Units]` was saved as `$C$2:$C$9`, `[#This Row]`
became an absolute row (one shared formula per row), `@` and several
specifiers (`[[#Headers],[#Data]]`) didn't parse, references inside a table
without its name (`[@Units]`, as XlsxWriter writes them) didn't either, and
nothing followed a table that grew. Kantor also needs to sort rows, hide rows
for a filter (thousands at a time) and keep the filter state, all undoable.

What (`src/kantor_tables.rs`, `KANTOR PATCH (tables)`):

* Lexer (`expressions/lexer/structured_references.rs`, `lexer/mod.rs`): the
  whole bracket after a table name is read (also in R1C1 mode, in which
  formulas are stored, and with no table name inside a table): `T[Col]`,
  `T[[Col Name]]`, `T[[A]:[B]]`, `T[#All]`..., `T[[#Totals],[Col]]`,
  `T[@Col]`, `T[@[Col Name]]`, `T[@]`; specifiers in any case; `'` escapes.
  Without a table name, `[@…]`, `[#…]` and `[[…]]` are always structured;
  a plain `[Name]` only when the formula's cell is inside a table
  (`Lexer::kantor_in_table`, set by `Parser::parse`): elsewhere `[b]` is a
  LAMBDA's optional parameter.
  `TableSpecifier` gains `HeadersData` and `DataTotals`.
* Parser: `Node::TableRefKind(Box<TableRef>)` (table and column names in their
  own spelling, specifier); the table and columns must exist (any case).
  Stringified as `Sales[[#This Row],[Units]]` for files and `Sales[@Units]`
  otherwise (`table_ref_text`), so the R1C1 text is the same in every row.
  `static_analysis`: one cell for `@` with one column, else like a range.
* Evaluation (`model.rs`): `resolve_table_ref` turns it into a reference or
  range against the table's current `reference` (#VALUE! for `@` outside the
  data rows, #REF! for a missing header/total row); function arguments are
  resolved first (`resolve_table_args`, for ROW, SUBTOTAL... which look at
  nodes). A bare table name evaluates to its data.
* Geometry: `displace_tables` (called next to `displace_defined_names`):
  rows/columns inserted inside a table grow it (new columns `ColumnN`), a
  delete shrinks it, deleting the header row removes it; the sheet's
  AutoFilter range moves the same way. `UserModel::delete_rows/columns`
  snapshot tables and the sheet filter before (`tables_snapshot_diffs`), as
  for defined names.
* `Table::kantor_auto_filter` and `Worksheet::kantor_auto_filter`: the
  `<autoFilter>` element (filled by the vendored reader, written by its
  writer for the sheet; Kantor writes tables' into their parts).
  `UserModel::kantor_set_table` (create/change/remove),
  `kantor_set_sheet_filter`, `kantor_rename_table_column` (renames the column
  in every structured reference: shared formulas are re-parsed),
  `kantor_set_rows_hidden` (many rows, one step, rows looked up once) and
  `kantor_permute_rows` (sort: cells of a block of rows move as they are, so
  relative references follow their row as in Excel; hyperlinks move too;
  refused over merged cells and array formulas). New `Diff`s:
  `KantorRowEntries`, `KantorSetTable`, `KantorRenameTableColumn`,
  `KantorSetSheetFilter`, `KantorPermuteRows`.
* SUBTOTAL (`functions/subtotal.rs`): a hidden row is "filtered" if it lies
  under the sheet's AutoFilter with a filter on (or, as upstream, in a table
  with an active filter); hidden rows are looked up once per range (upstream
  scanned the sheet's rows for every cell).
* `evaluate_if_not_paused` is `pub(crate)`.

Tests: `src/test/test_kantor_tables.rs` (every specifier, `@`, escapes,
unqualified references, shared formulas, export text, growth and undo,
permutation and undo, filtered SUBTOTAL/AGGREGATE, the sheet filter moving),
`expressions/parser/tests/test_tables.rs` (now expects structured text), and
upstream's `tests/calc_test_no_export/tables.xlsx` in `vendor/ironcalc`
(matches Excel's values). Kantor's: `crates/sheets_engine/tests/tables.rs`.

## 14. GETPIVOTDATA

Why: reports built on pivot tables read them with GETPIVOTDATA, which showed
`#NAME?`. The model has no pivot tables (their cells are ordinary cells), so the
caller describes them.

What (`src/functions/kantor_pivot.rs`, `KANTOR PATCH (pivot)`):

* `Model::kantor_pivots: Vec<PivotGrid>` (public, not saved or serialized, empty
  in every constructor) and `UserModel::kantor_set_pivots`: each pivot table's
  area (body and report filters) and its value cells, each with the names of its
  data field (caption and source field, lower case) and its `(field, item)`
  pairs. Kantor (`sheets_engine::pivot`) sets it when a workbook opens and when
  pivot tables are refreshed or move (inserted/deleted rows and columns, undo,
  redo), and recalculates then.
* `GETPIVOTDATA(data_field, pivot_table, [field, item]...)`: the pivot table the
  reference's top-left cell is in, then the value cell whose data field and pairs
  are exactly those given (any order, case-insensitive); its value, `#REF!` if
  none. Named in `kantor_names.rs`, written without a prefix.
* `Function::into_iter` returns 506 functions.

Tests: `src/test/test_kantor_pivot.rs`; `crates/sheets_engine/tests/pivot.rs`
against the corpus file 21.

## 15. Hiding columns as one undo step

`UserModel::kantor_set_columns_hidden(sheet, &[(column, hidden)])`
(`src/kantor_tables.rs`) hides or shows columns as one diff list of upstream's
`Diff::SetColumnHidden` (which upstream undoes and redoes but never records).
Widths are kept, so an unhidden column comes back as wide as it was. Rows use
`kantor_set_rows_hidden` (§13). Tests: `crates/sheets_engine/src/hide.rs`.

`UserModel::kantor_set_frozen(sheet, rows, cols)` sets the frozen panes without
an undo step: Excel's Freeze Panes isn't undone by Ctrl+Z (Kantor marks the
workbook unsaved instead, `sheets_engine::grid::set_frozen`).

## 16. What changed, for caches kept across edits; a cheaper `dimension()`

Why: on a 90k-row sheet with conditional formatting every edit cost ~105 ms
recomputing each rule's range-wide numbers (min/max, sorted values, counts),
even for an edit nowhere near a rule. And `UserModel` put every diff list on
its `send_queue` (upstream: to sync a remote copy), which Kantor never sent:
a second copy of every edit's diffs, old values included, for the session.

* `UserModel::kantor_take_changes()` (`src/kantor_changes.rs`) returns a
  `KantorChanges` summed up from every diff list pushed, undone or redone
  since the last call (it used to empty the `send_queue` instead, which
  only happened on sheets with conditional formatting; elsewhere the copy
  stayed for the session): rectangles whose values may have changed
  (cell values, array formulas, cleared ranges, sorted rows), sheets where
  anything may have moved (rows/columns inserted, deleted or moved, merges,
  conditional formatting rules, sheet filters), `all` for workbook-wide
  changes (sheets, names, tables, locale, theme, named styles), `restyled`
  for changes that keep values (styles, sizes, hidden rows, frozen panes).
  Undo and redo report the cells of the diff list they apply.
* `UserModel::kantor_stop_send_queue()` stops filling `send_queue`
  (`flush_send_queue` then returns nothing); `sheets_engine` calls it on
  every workbook. The queue cloned each diff list (~75 ms for a 10 000-row
  fill) to keep a copy nobody read.
* `Model::kantor_evaluations()` counts `evaluate()` passes (formula results
  change without a diff); `Model::kantor_has_dynamic_arrays()` says whether
  the last pass had dynamic arrays (whose spills can land anywhere).
* `Worksheet::dimension()` takes each row's first and last column
  (`SheetData::kantor_extent`, `Row::column_span`: cells are sorted) instead
  of visiting every cell: ~35 ms less per call on 1.8M cells, same result.

Used by `crates/sheets_engine/src/cond_format.rs` (`cond_sync`). Tests:
`kantor_changes::tests`, `sheets_engine` `cond_format::invalidation`.

## 17. Keying formulas by what they parse to (fast open)

Why: the xlsx reader parses every formula cell's A1 text to turn it into the
R1C1 form the model keeps. A filled-down column is one R1C1 formula written
90 000 times (`E2*F2`, `E3*F3`, ...), and parsing them was ~80% of opening
the generated 90k-row workbook (810k formula cells).

* `Parser::kantor_rc_key(formula, context, key)` lexes the formula and
  writes a key in which every reference is what `parse` makes of it
  (absolute parts as they are, relative parts as offsets from `context`,
  range ends put in order first, as `parse_primary` does) and every other
  token is its text. Two formulas on one sheet with equal keys parse to the
  same tree. It refuses (returns false) formulas with `[` (structured
  references depend on the table the cell is in) and ones the lexer
  rejects. The caller (vendor/ironcalc §8) also never shares a result that
  is a parse error, whose text is the formula as written.
* `Lexer::set_formula` reuses its buffer; `Lexer::kantor_push_text` copies a
  token's text into the key.

Test: `rc_cache_matches_parsing_each_formula` in vendor/ironcalc (20 000
random formulas, filled-down and not, compared with parsing each alone).

## 18. Sorting moves rows whole (fast sort)

Why: sorting the 90k-row table took ~0.4–0.5 s (and its undo as long):
`kantor_permute_rows` took every row apart into cells and built it again,
and checked every cell of the range for array formulas through `Cow<Cell>`.

* `SheetData::kantor_permute_whole_rows` moves the rows as they are held
  (packed, numbers or cells) when every row of the range has its cells
  within the sorted columns; otherwise the old cell-by-cell path runs.
* `SheetData::kantor_has_array_cells` looks only into rows of cells (rows
  of numbers and packed rows can't hold array formulas or spills).

Tests: the sort tests of `crates/sheets_engine` (tables, ranges, filters,
undo), and `kantor_tables` tests here.

## 19. Partial recalculation

Why: every edit IronCalc evaluated the whole workbook (and auto-fill did so
even in manual calculation): filling 1000 rows of formulas in the 90k-row
workbook took 1–2 s, nearly all of it recalculating 810k formulas whose
inputs hadn't changed.

What (`src/kantor_recalc.rs`):

* `UserModel` keeps `kantor_unevaluated`, the `KantorChanges` (§16) of every
  diff list pushed, undone or redone since the last evaluation.
  `evaluate_if_not_paused`, auto-fill and the new public
  `UserModel::kantor_evaluate_changes` evaluate only what those changes can
  reach (`Model::kantor_evaluate_changed`); `UserModel::evaluate` stays a full
  evaluation (F9) and empties the record.
* Dependents are worked out from the parsed formulas each time, no graph is
  kept: formula cells are grouped into runs (consecutive rows of a column with
  one shared formula), each run's references are translated over the run at
  once, and the changed rectangles are spread to the rows that read them
  until nothing new is marked. Formulas still without a value (typed in
  manual mode) are evaluated too.
* Only the marked cells run; any other formula read gives its stored value
  (`Evaluation::kantor_only`, also honoured by the lookup cache's "is this
  vector ready" check).
* Always evaluated, with their dependents: volatile functions (RAND,
  RANDBETWEEN, RANDARRAY, NOW, TODAY), and formulas whose reads can't be
  told from the formula: OFFSET, INDIRECT, CELL, INFO, SHEETS, SUBTOTAL and
  AGGREGATE (hidden rows), GETPIVOTDATA, HYPERLINK, defined names, tables
  and structured references, `:` between function results, spills (`#`),
  user functions.
* Falls back to a full evaluation: changes to sheets, rows/columns, merges,
  rules, names, tables or more than 64 scattered rectangles on a sheet (§16
  says "sheet" or "all"); dynamic arrays or array formulas anywhere (spills
  can land on any cell; formulas IronCalc thinks may return an array, such as
  some INDEX or name formulas typed in, become dynamic arrays); dynamic
  links; more than 30M run checks; a pass that met a circular reference
  (which cells of a loop get `#CIRC!` depends on where the evaluation
  entered it) or a restart.
* `Model::kantor_last_partial()` says how many formulas the last evaluation
  ran when it was partial (for tests and timing).
* Fixes found on the way: `UserModel::set_user_input` evaluated before its
  diff list was recorded (now after, so the edit is known); criteria
  functions (`COUNTIF`, `SUMIFS`...) compared text through a lower-case copy
  of every cell, now without a copy for ASCII text (`functions/util.rs`),
  and lower-cased the criterion itself for every cell too (once now: a
  whole-column `SUMIF` on 90k rows went from ~6.9 to ~4.3 ms).

Tests: `kantor_recalc::tests`: random formula graphs on two sheets (own and
cross-sheet references, mixed absolute ranges, whole columns, lookups,
criteria functions, OFFSET/INDIRECT, SUBTOTAL) with random edits, fills,
clears, undo and redo; after each partial evaluation every value is compared
with a full evaluation, in automatic mode (with and without loops) and in
manual mode (several edits, then one evaluation; most must be partial). A
fill down a 2000-row running total runs only the new cells; an edit at the
top runs the chain and nothing else.

## 20. Fast fill

Why: filling 9 formulas down 10 000 rows of the 90k-row test workbook took
~2 s, ~1.9 s of it writing the 90 000 cells: each got its source formula
printed for its position (`extend_to`), then typed in (`set_user_input`:
parsed again, units worked out, the shared formula looked up by its R1C1
text), and the source's style rebuilt and searched for in the style table,
and its link cloned, once per cell.

What (`src/user_model/autofill.rs`, both `auto_fill_rows` and
`auto_fill_columns`; `kantor_fill_cell`):

* A target whose source is a plain formula (not an array or dynamic one)
  gets the source's shared formula index directly when
  `kantor_copies_unchanged` says the translated text would parse back to the
  same formula: relative references are stored as offsets, so that holds
  when every reference stays on the sheet and the formula holds nothing
  looked up in the workbook (defined names, tables, lambdas, `@`, spill `#`,
  wrong references, parse errors say no). The cell goes through
  `prepare_cell_for_user_input` and drops a quote prefix as
  `set_user_input` would. Everything else is typed in as before.
* Each source cell's style, its index in the style table and its link are
  looked up once per fill (`KantorFillSource`).
* The diffs are unchanged (the translated text, old values and styles), so
  undo and redo are exactly as before.
* `UserModel::kantor_fill_copies` counts the cells copied by the last fill
  (for tests).

Idle machine, back to back (`crates/sheets_engine/examples/big_timing.rs`):
10 000 rows 1.05 s → 0.45 s, 1000 rows 227 → 146 ms. In the app (fill job
on its background thread) the old way was about twice as slow as in the
example, the new one is not: 10 000 rows 2.28 s → 0.47 s, 1000 rows
~370 → ~175 ms.

Tests: `test/user_model/test_kantor_fast_fill.rs`: fills down, up, right
and left of formulas of every kind (relative, absolute, mixed, whole
columns, other sheet, a name, an array, OFFSET, references falling off the
sheet) compared with typing each translated formula in; undo and redo
restore exactly; the change summary without the send queue.

## 21. Cheap snapshots (copy-on-write rows)

Why: a recovery copy or a Save snapshots the workbook on the UI thread
(`sheets_engine` `Workbook::xlsx_snapshot`, a clone of `types::Workbook`)
and writes it in the background. On the 90k-row test workbook the clone
took 55-73 ms, nearly all of it copying every sheet's cells, which stalled
typing every time a recovery copy was due.

What (`src/sheet_data/chunks.rs`): `SheetData` holds its chunks of 1024
rows as `Arc<Chunk>` instead of `Box<Chunk>`. A clone copies one pointer per
chunk; the writers (`row_mut`, `row_or_insert`, `remove_row`, `take_row`)
go through `Arc::make_mut`, so a chunk is copied the first time it is
written while a clone shares it (well under a millisecond), and never
otherwise. Reads are unchanged. bitcode encodes an `Arc` as it did the
`Box`: saved bytes are identical.

Effect: the snapshot 55-73 ms → 3-6 ms (machine under load), measured with
`KANTOR_BIG_XLSX=big.xlsx cargo test --release -p sheets_engine
snapshot_timing -- --ignored --nocapture`.

Tests: `sheet_data/test.rs` (`a_clone_shares_chunks_until_either_is_written`:
every kind of write on either side leaves the other unchanged, untouched
chunks stay shared) and `sheet_data/chunks.rs`
(`shared_chunks_save_as_boxed_ones_did`).

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
`user_model/common.rs`, `test/user_model/{mod,test_kantor_defined_names_move}.rs`;
for section 12 the new files `kantor_tables.rs`, `test/test_kantor_tables.rs` and hunks in `expressions/{token,lexer/mod,lexer/structured_references,parser/mod,parser/stringify,parser/move_formula,parser/static_analysis}.rs`, `model.rs`, `units.rs`, `actions.rs`, `types.rs`, `new_empty.rs`, `functions/subtotal.rs`, `user_model/{history,undo_redo,common}.rs`; for section 10 the new files `functions/kantor_*.rs` and the two test files, plus
`functions/{mod,subtotal}.rs`, `functions/logical/{bycol_byrow,map_reduce,scan}.rs`,
`functions/statistical/count_and_average.rs`, `expressions/parser/{mod,stringify,static_analysis}.rs`,
`lib.rs`, `test/mod.rs`, `expressions/parser/tests/mod.rs`)
and run the crate's
tests in it (`cargo test`, also with `IRONCALC_MAX_DEPTH=1`). The crate's
`Cargo.lock` is upstream's and only used when testing in this directory.
With it, `test::user_model::test_to_from_bytes::errors` fails also without any
patch (the bitcode error message changed); everything else passes.
