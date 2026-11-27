//! `Range.AutoFit` (columns and rows) and `Range.RemoveDuplicates`.
//!
//! AutoFit measures the text with the cells' own fonts, headless, through
//! `sheets_pdf`'s shaper (parley over the fonts installed, with the metric
//! twins Carlito / Liberation for Calibri / Arial…): the widths and heights
//! Excel gives at 96 dpi, the way PDF export fits rows. The app's own
//! Autofit measures with the window's text system instead, so the two can
//! differ by a few pixels; where neither the font nor a twin is installed,
//! a fallback font is measured.
//!
//! RemoveDuplicates moves the kept rows up inside the range with their
//! formats (`Workbook::copy_cells`, as Copy and Paste), then clears the rows
//! left at the bottom (contents and formats).

use std::cell::RefCell;
use std::sync::Arc;

use kantor_script::{ScriptError, Value};
use sheets_engine::{CellRange, CellStyle, RowHeight};
use sheets_pdf::{FontKey, Shaper};

use crate::range::{Mode, RangeObj};
use crate::session::Session;
use crate::values::*;

/// Cells measured at most per column, as the app's Autofit.
const FIT_MAX_CELLS: usize = 10_000;
/// Cells measured at most per row.
const FIT_MAX_ROW_CELLS: usize = 200;
/// Rows fitted one by one at most; rows past these (or past the used
/// range) go back to automatic height at the default height.
const FIT_MAX_ROWS: u32 = 100_000;
/// Cell padding left and right, and the extra height of a row, in points
/// (as `sheets_pdf` prints and fits cells).
const PAD_X: f64 = 2.0;
const PAD_Y: f32 = 1.6;
/// Pixels per point at 100 % (column widths are in Excel's pixels).
const PX_PER_PT: f64 = 96.0 / 72.0;
/// Excel's tallest row, points.
const MAX_ROW_POINTS: f64 = 409.0;

thread_local! {
    /// Creating a shaper scans the installed fonts: one per thread.
    static SHAPER: RefCell<Option<Shaper>> = const { RefCell::new(None) };
}

fn with_shaper<T>(f: impl FnOnce(&mut Shaper) -> T) -> T {
    SHAPER.with(|cell| {
        let mut slot = cell.borrow_mut();
        f(slot.get_or_insert_with(Shaper::new))
    })
}

/// The font family list for a cell font: the font, its metric-compatible
/// twin, then common fallbacks (as `sheets_pdf` prints cells).
fn family(name: &str) -> Arc<str> {
    let twin = match name.to_ascii_lowercase().as_str() {
        "calibri" | "calibri light" => Some("Carlito"),
        "cambria" | "cambria math" => Some("Caladea"),
        "arial" | "helvetica" => Some("Liberation Sans"),
        "times new roman" | "times" => Some("Liberation Serif"),
        "courier new" | "courier" => Some("Liberation Mono"),
        _ => None,
    };
    let mut list = String::new();
    for f in [Some(name), twin].into_iter().flatten() {
        if !f.is_empty() {
            list.push_str(f);
            list.push_str(", ");
        }
    }
    list.push_str("Carlito, Liberation Sans, Noto Sans, sans-serif");
    Arc::from(list)
}

fn font_of(style: &CellStyle, default_name: &str) -> FontKey {
    FontKey::new(
        family(style.font_name.as_deref().unwrap_or(default_name)),
        style.font_size.max(1) as f32,
        style.bold,
        style.italic,
    )
}

/// `Range.AutoFit` on rows, columns or cells, as Excel: whole rows (or
/// `.Rows`) fit their heights; otherwise the columns fit their widths, to
/// the range's own cells unless whole columns are given
/// (`Range("A1:E1").Columns.AutoFit` looks only at row 1).
pub fn autofit(r: &RangeObj) -> Result<Value, ScriptError> {
    for &a in &r.areas {
        let rows = a.is_whole_rows() || r.mode == Mode::Rows;
        let cols = a.is_whole_columns() || r.mode == Mode::Columns;
        if rows && !cols {
            autofit_rows(&r.s, r.sheet, (a.top, a.bottom))?;
        } else {
            let within = (!a.is_whole_columns()).then_some((a.top, a.bottom));
            autofit_columns(&r.s, r.sheet, (a.left, a.right), within)?;
        }
    }
    Ok(Value::Boolean(true))
}

/// Fit the widths of columns `cols` (inclusive, 0-based) of sheet `sheet`
/// (`sheetId`) to their shown text, looking only at rows `within` when
/// given. Empty columns keep their width. Widest is 255 characters.
pub fn autofit_columns(
    s: &Session,
    sheet: u32,
    cols: (u32, u32),
    within: Option<(u32, u32)>,
) -> Result<(), ScriptError> {
    let idx = s.sheet_index(sheet)?;
    // Values brought up to date first: the shown text is measured.
    let mut book = s.read()?;
    let used = book.used_size(idx).1;
    if used == 0 || cols.0 >= used {
        return Ok(());
    }
    let last = cols.1.min(used - 1);
    let scale = book.column_scale(idx);
    let default_name = book.default_font_name();
    let indent_px = scale.digit_px * 10.0 / 7.0;
    let max_px = scale.px(255.0);
    let mut widths = Vec::new();
    for col in cols.0..=last {
        let cells = book.column_fit_cells(idx, col);
        let widest = with_shaper(|sh| {
            let mut widest: Option<f64> = None;
            for cell in cells
                .iter()
                .filter(|c| within.is_none_or(|(top, bottom)| (top..=bottom).contains(&c.row)))
                .take(FIT_MAX_CELLS)
            {
                let font = font_of(&cell.style, &default_name);
                // Wrapped text: its longest line; otherwise one line.
                let text_pt = if cell.style.wrap {
                    cell.text
                        .split('\n')
                        .map(|l| sh.line(l, &font).width)
                        .fold(0.0f32, f32::max)
                } else {
                    sh.line(&cell.text, &font).width
                };
                let px = (f64::from(text_pt) + 2.0 * PAD_X) * PX_PER_PT
                    + f64::from(cell.style.indent.min(250)) * indent_px
                    + 1.0;
                widest = Some(widest.map_or(px, |w: f64| w.max(px)));
            }
            widest
        });
        if let Some(px) = widest {
            widths.push((col, px.ceil().min(max_px)));
        }
    }
    book.set_column_widths(idx, &widths).map_err(engine)?;
    Ok(())
}

/// Fit the heights of rows `rows` (inclusive, 0-based) of sheet `sheet`
/// (`sheetId`) to their content (wrapped text, larger fonts), as Excel
/// fits automatic rows; the rows follow their content again (no custom
/// height).
pub fn autofit_rows(s: &Session, sheet: u32, rows: (u32, u32)) -> Result<(), ScriptError> {
    let idx = s.sheet_index(sheet)?;
    let mut book = s.read()?;
    let default = book.default_row_points(idx);
    let used = book.used_size(idx).0;
    let fit_last = rows
        .1
        .min(used.saturating_sub(1))
        .min(rows.0.saturating_add(FIT_MAX_ROWS - 1));
    let mut heights: Vec<(u32, f64)> = Vec::new();
    if used > 0 && rows.0 < used {
        let columns = book.print_columns(idx);
        let default_name = book.default_font_name();
        for r in rows.0..=fit_last {
            let cells = book.row_fit_cells(idx, r);
            let h = with_shaper(|sh| {
                let mut h = default;
                for cell in cells.iter().take(FIT_MAX_ROW_CELLS) {
                    let st = &cell.style;
                    let font = font_of(st, &default_name);
                    let need = if st.wrap && st.rotation == 0 {
                        let w = (columns.width_pt(cell.col) - 2.0 * PAD_X) as f32;
                        sh.wrapped(&cell.text, &font, w, false).height
                    } else {
                        sh.line(&cell.text, &font).height
                    } + PAD_Y;
                    h = h.max(f64::from(need).min(MAX_ROW_POINTS));
                }
                h
            });
            // Excel keeps heights to a quarter point.
            heights.push((r, (h * 4.0).round() / 4.0));
        }
    }
    // Runs of rows with the same height, one edit each.
    let mut start = 0;
    while start < heights.len() {
        let (first, h) = heights[start];
        let mut end = start;
        while end + 1 < heights.len() && heights[end + 1].1 == h {
            end += 1;
        }
        book.set_row_heights(idx, first, heights[end].0, RowHeight::Auto(h))
            .map_err(engine)?;
        start = end + 1;
    }
    // The rest (empty rows) back to automatic.
    let rest = heights.last().map_or(rows.0, |&(r, _)| r + 1);
    if rest <= rows.1 {
        book.set_row_heights(idx, rest, rows.1, RowHeight::Auto(default))
            .map_err(engine)?;
    }
    Ok(())
}

/// `Range.RemoveDuplicates`: rows of the range (after the header row with
/// `header`) whose `columns` (0-based, within the range) repeat an earlier
/// row's are removed; the kept rows move up inside the range with their
/// formats, and the rows freed at the bottom are cleared. Cells outside the
/// range don't move. Values are compared as shown, ignoring case, as Excel
/// does.
pub fn remove_duplicates(r: &RangeObj, columns: &[u32], header: bool) -> Result<(), ScriptError> {
    let idx = r.idx()?;
    let fail = || err1004("RemoveDuplicates method of Range class failed");
    let (a, first, keep) = {
        let book = r.s.read()?;
        let (rows, cols) = book.used_size(idx);
        let Some(a) = r.first().clip(rows.max(1), cols.max(1)) else {
            return Ok(());
        };
        if columns.iter().any(|&c| c >= r.first().cols_count()) {
            return Err(fail());
        }
        let first = if header { a.top + 1 } else { a.top };
        if first > a.bottom {
            return Ok(());
        }
        let mut seen = std::collections::HashSet::new();
        let mut keep: Vec<u32> = Vec::new();
        for row in first..=a.bottom {
            let key: Vec<String> = columns
                .iter()
                .map(|&c| book.display_value(idx, row, a.left + c).to_lowercase())
                .collect();
            if seen.insert(key) {
                keep.push(row);
            }
        }
        (a, first, keep)
    };
    if keep.len() as u32 == a.bottom - first + 1 {
        return Ok(());
    }
    let mut book = r.s.write()?;
    // Runs of kept rows move up together, in pieces that never overlap
    // their own destination.
    let mut target = first;
    let mut i = 0;
    while i < keep.len() {
        let mut j = i;
        while j + 1 < keep.len() && keep[j + 1] == keep[j] + 1 {
            j += 1;
        }
        let mut src = keep[i];
        let mut left = (j - i + 1) as u32;
        while left > 0 {
            let n = if src == target {
                left
            } else {
                left.min(src - target)
            };
            if src != target {
                book.copy_cells(
                    idx,
                    CellRange::from_corners((src, a.left), (src + n - 1, a.right)),
                    idx,
                    target,
                    a.left,
                    false,
                )
                .map_err(engine)?;
            }
            src += n;
            target += n;
            left -= n;
        }
        i = j + 1;
    }
    if target <= a.bottom {
        book.clear_all(
            idx,
            CellRange::from_corners((target, a.left), (a.bottom, a.right)),
        )
        .map_err(engine)?;
    }
    Ok(())
}
