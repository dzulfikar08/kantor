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

## Updating or dropping

Upstream: https://github.com/ironcalc/IronCalc. Drop this copy (remove the
`[patch.crates-io]` line and `vendor/ironcalc_base`) once upstream caches lookup
vectors or indexes exact matches itself; time
`cargo run --release -p sheets_engine --example eval_timing -- <big.xlsx>` before
and after. To move to a newer upstream with the patch still needed: copy the new
crate over this directory, reapply the `KANTOR PATCH` hunks (the new file
`functions/lookup_cache.rs` and `test/test_kantor_lookup_cache.rs`, plus small
call-site edits in `evaluation.rs`, `model.rs`, `functions/mod.rs`,
`functions/lookup_and_reference/{mod,xmatch}.rs`, `functions/xlookup.rs`,
`functions/statistical/if_ifs.rs`, `test/mod.rs`, `build.rs`) and run the crate's
tests in it (`cargo test`, also with `IRONCALC_MAX_DEPTH=1`). The crate's
`Cargo.lock` is upstream's and only used when testing in this directory.
