//! Cell protection for macros (`Range.Locked`, `Range.FormulaHidden` in
//! `crates/sheets_script`): the `<xf><protection>` a cell's style keeps
//! (see `KANTOR_PATCHES.md`, `Style::protection`). Kantor doesn't protect
//! sheets itself; the flags are kept and saved so a sheet protected in
//! Excel behaves as the macro meant.

use anyhow::{Result, anyhow, bail};
use ironcalc_base::types::CellProtection;

use crate::{CellRange, MAX_COLS, MAX_ROWS, Workbook};

/// Cells set at most by one call (styles are set cell by cell).
const MAX_CELLS: u64 = 2_000_000;

impl Workbook {
    /// Whether the cell `(row, col)` is locked and whether its formula is
    /// hidden when the sheet is protected (Excel's default: locked, shown).
    pub fn cell_protection(&self, sheet: u32, row: u32, col: u32) -> (bool, bool) {
        self.model
            .get_model()
            .get_style_for_cell(sheet, row as i32 + 1, col as i32 + 1)
            .ok()
            .and_then(|s| s.protection)
            .map_or((true, false), |p| (p.locked, p.hidden))
    }

    /// Set the cells of `range` locked or not (`locked`) and their formulas
    /// hidden or not (`hidden`); `None` leaves that flag as each cell has
    /// it. Their other formatting stays. One undo step; returns whether
    /// anything changed.
    pub fn set_range_protection(
        &mut self,
        sheet: u32,
        range: CellRange,
        locked: Option<bool>,
        hidden: Option<bool>,
    ) -> Result<bool> {
        if range.bottom >= MAX_ROWS
            || range.right >= MAX_COLS
            || range.top > range.bottom
            || range.left > range.right
        {
            bail!("range out of bounds");
        }
        if u64::from(range.rows_count()) * u64::from(range.cols_count()) > MAX_CELLS {
            bail!("too many cells to change at once");
        }
        let e = |e: String| anyhow!("{e}");
        let mut styles = Vec::with_capacity(range.rows_count() as usize);
        let mut changed = false;
        {
            let model = self.model.get_model();
            for r in range.top..=range.bottom {
                let mut row = Vec::with_capacity(range.cols_count() as usize);
                for c in range.left..=range.right {
                    let mut style = model
                        .get_style_for_cell(sheet, r as i32 + 1, c as i32 + 1)
                        .map_err(e)?;
                    let old = style.protection.clone().unwrap_or_default();
                    let new = CellProtection {
                        locked: locked.unwrap_or(old.locked),
                        hidden: hidden.unwrap_or(old.hidden),
                    };
                    changed |= new != old;
                    style.protection = (new != CellProtection::default()).then_some(new);
                    row.push(style);
                }
                styles.push(row);
            }
        }
        if !changed {
            return Ok(false);
        }
        self.model.set_selected_sheet(sheet).map_err(e)?;
        self.model
            .set_selected_cell(range.top as i32 + 1, range.left as i32 + 1)
            .map_err(e)?;
        self.model
            .set_selected_range(
                range.top as i32 + 1,
                range.left as i32 + 1,
                range.bottom as i32 + 1,
                range.right as i32 + 1,
            )
            .map_err(e)?;
        self.model.on_paste_styles(&styles).map_err(e)?;
        self.history.edited();
        Ok(true)
    }
}
