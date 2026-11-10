# kantor_numfmt: provenance and Kantor patches

Adapted from **GridCraft**'s `gridcraft-numfmt` crate:

- Upstream: https://github.com/storytold/gridcraft, `crates/numfmt`
- Pinned commit: `e9addf52fe68847e6c1310fcfa5d42b7aa8b9650`
  ("Arabic bidi layout and cell reading direction in the grid and editors (#947)")
- Licence: MIT OR Apache-2.0, copyright (c) 2026 ArtCraft Team and the GridCraft
  contributors. The texts are in [`LICENSE-MIT`](LICENSE-MIT) and
  [`LICENSE-APACHE`](LICENSE-APACHE). Kantor's changes to this crate are under the same
  terms, so the crate stays MIT OR Apache-2.0 inside the AGPL-3.0-or-later workspace.

Kantor uses it to display cell values (`sheets_engine::Workbook::display`), in place of
IronCalc's formatter, which gets fractions, conditions, scientific steps, some rounding and
every `[$…-LCID]` code wrong (see the table in `src/tests.rs`, `excel_table`).

## What was copied

| File | Upstream | Changes |
|---|---|---|
| `src/lib.rs` | `crates/numfmt/src/lib.rs` | header; `mod core`/`pub mod locale`; no array values; Kantor entry points (`display_number`, `display_text`, `formats_text`, `is_valid`) |
| `src/parse.rs` | same name | keeps the LCID of `[$…-421]` tags and applies it to sections without one |
| `src/render.rs` | same name | decimal and thousands separators (number, fraction, General, sub-second sections) from the locale |
| `src/calendar.rs` | same name | Gregorian month and weekday names from the locale |
| `src/builtin.rs`, `src/decimal.rs` | same names | none |
| `src/tests.rs` | same name | `[$€-407]` now expects German separators; Kantor tests appended (Excel table, colours, fill, text sections) |
| `tests/poi_number_format_approx.rs` | same name | crate names only |
| `src/core.rs` | `crates/core/src/{date,color,value,parse}.rs` | only what the formatter needs: date serials and the indexed palette as they are; `Value` / `CellError` cut down; a small `parse_number_text` (plain numbers, commas, `%`) instead of GridCraft's full entry parser |
| `src/locale.rs` | none | new: LCID to separators and (Indonesian) names |
| `rustfmt.toml`, `clippy.toml` | GridCraft's root settings | kept so the code diffs cleanly against upstream |

Changes inside upstream files are marked `KANTOR PATCH`.

## Locale behaviour (Kantor addition)

GridCraft is en-US only and ignores locale tags. Kantor reads the LCID in `[$-421]`,
`[$Rp-421]`, `[$€-407]` (low 16 bits; below 0x400, such as Excel's `[$€-1]` / `[$€-2]`
currency markers, is no language):

- Month and weekday names follow the tag, as in Excel: `[$-421]d mmmm yyyy` shows
  `17 Agustus 2025`. Only Indonesian names are included; other languages keep English names.
- Separators also follow the tag: `[$Rp-421]#,##0.00` shows `Rp1.234,50`. **This differs from
  Excel**, which takes separators from the system settings (an en-US Excel shows
  `Rp1,234.50`). Kantor shows the file the way its author's Excel showed it. Codes without a
  tag use en-US separators.
- Not localised: the `:` time separator, `AM/PM`, General without a tag.

## Updating

1. Diff `crates/numfmt` and the `core` pieces above between the pinned commit and the new one
   (`git diff e9addf52..NEW -- crates/numfmt crates/core/src/date.rs crates/core/src/color.rs`).
2. Apply the upstream changes here, keeping the `KANTOR PATCH` blocks; update the pinned
   commit above.
3. `cargo test -p kantor_numfmt -p sheets_engine` (the corpus test checks real files).
