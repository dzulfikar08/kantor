//! What macros need of tables beyond the Table Design commands
//! (`sheets_script`'s `ListObject`): resizing a table (`ListObject.Resize`)
//! and adding or deleting its rows and columns (`ListRows.Add`,
//! `ListRow.Delete`, `ListColumns.Add`, `ListColumn.Delete`).
//!
//! Rows and columns are inserted and deleted across the sheet (the engine's
//! `insert_rows` / `delete_columns`...: tables, names, formats and
//! references follow as for the grid's commands), not only inside the
//! table's own cells as Excel does. A row added at the end of a table whose
//! next row is free just extends the table.

use std::collections::HashSet;

use anyhow::{Result, anyhow, bail};
use ironcalc_base::kantor_tables::{find_table_pub, table_reference, with_auto_filter_ref};
use ironcalc_base::types::{Table, TableColumn};

use crate::table::TableInfo;
use crate::{CellRange, MAX_COLS, MAX_ROWS, Workbook};

impl Workbook {
    /// Table `name` (any case): its sheet, model entry and geometry.
    fn script_table(&self, name: &str) -> Result<(u32, Table, TableInfo)> {
        let t = find_table_pub(&self.model.get_model().workbook.tables, name)
            .cloned()
            .ok_or_else(|| anyhow!("no table {name}"))?;
        let sheet = self
            .sheet_names()
            .iter()
            .position(|n| *n == t.sheet_name)
            .ok_or_else(|| anyhow!("no table {name}"))? as u32;
        let info = self
            .tables(sheet)
            .into_iter()
            .find(|i| i.name.eq_ignore_ascii_case(&t.name))
            .ok_or_else(|| anyhow!("no table {name}"))?;
        Ok((sheet, t, info))
    }

    /// Whether the cells of `range` are empty and outside every table but
    /// `own`.
    fn free_for_table(&self, sheet: u32, range: CellRange, own: &str) -> bool {
        (range.top..=range.bottom)
            .all(|r| (range.left..=range.right).all(|c| !self.is_filled(sheet, r, c)))
            && self
                .tables(sheet)
                .iter()
                .all(|t| t.name.eq_ignore_ascii_case(own) || !t.area.intersects(&range))
    }

    /// Whether table `name` has no data yet: one data row, all empty (the
    /// row a new table gets, Excel's insert row).
    pub fn table_body_is_empty(&self, name: &str) -> bool {
        let Ok((sheet, _, info)) = self.script_table(name) else {
            return false;
        };
        let (top, bottom) = info.data_rows();
        top > bottom
            || (top == bottom
                && (info.area.left..=info.area.right).all(|c| !self.is_filled(sheet, top, c)))
    }

    /// `ListObject.Resize`: table `name` covers `area` (header, data and
    /// total rows). The header row stays where it is and the new area must
    /// overlap the old one, as in Excel. Columns that stay keep their names
    /// and total row settings; new ones are named from their header cells
    /// (blank or repeated names become `ColumnN`). Formulas in the last data
    /// row are filled down into new rows (calculated columns). One undo step.
    pub fn resize_table(&mut self, name: &str, area: CellRange) -> Result<()> {
        let (sheet, t, info) = self.script_table(name)?;
        let old = info.area;
        if area == old {
            return Ok(());
        }
        if area.bottom >= MAX_ROWS || area.right >= MAX_COLS || area.top > area.bottom {
            bail!("The range is outside the sheet");
        }
        if t.header_row_count > 0 && area.top != old.top {
            bail!(
                "The header row must remain in the same row and the resulting table range must overlap the original table range"
            );
        }
        if !area.intersects(&old) {
            bail!("The resulting table range must overlap the original table range");
        }
        if area.rows_count() < 1 + t.header_row_count + t.totals_row_count {
            bail!("A table needs a data row");
        }
        for other in self.tables(sheet) {
            if !other.name.eq_ignore_ascii_case(&t.name) && other.area.intersects(&area) {
                bail!("It overlaps table {}", other.name);
            }
        }
        let tname = t.name.clone();
        if t.totals_row_count > 0 && area.bottom != old.bottom {
            // The total row moves to the new last row: off, resize, on.
            return self.batch(|wb| {
                let mut opts = wb
                    .table_options(&tname)
                    .ok_or_else(|| anyhow!("no table {tname}"))?;
                opts.total_row = false;
                wb.set_table_options(&tname, opts.clone())?;
                let inner = CellRange {
                    bottom: area.bottom - 1,
                    ..area
                };
                wb.resize_table(&tname, inner)?;
                opts.total_row = true;
                wb.set_table_options(&tname, opts)
            });
        }
        let header = (t.header_row_count > 0).then_some(area.top);
        // Columns: those that stay keep theirs.
        let mut columns: Vec<Option<TableColumn>> = (area.left..=area.right)
            .map(|c| {
                if (old.left..=old.right).contains(&c) {
                    t.columns.get((c - old.left) as usize).cloned()
                } else {
                    None
                }
            })
            .collect();
        let mut seen: HashSet<String> = columns
            .iter()
            .flatten()
            .map(|c| c.name.to_lowercase())
            .collect();
        let mut next_id = t.columns.iter().map(|c| c.id).max().unwrap_or(0) + 1;
        let mut writes: Vec<(u32, u32, String)> = Vec::new();
        for (i, slot) in columns.iter_mut().enumerate() {
            if slot.is_some() {
                continue;
            }
            let c = area.left + i as u32;
            let typed = header
                .map(|h| self.display_value(sheet, h, c).trim().to_owned())
                .unwrap_or_default();
            let mut n = if typed.is_empty() {
                let mut k = 1;
                while seen.contains(&format!("column{k}")) {
                    k += 1;
                }
                format!("Column{k}")
            } else {
                typed.clone()
            };
            let base = n.clone();
            let mut k = 2;
            while seen.contains(&n.to_lowercase()) {
                n = format!("{base}{k}");
                k += 1;
            }
            seen.insert(n.to_lowercase());
            if let Some(h) = header
                && (n != typed || self.raw_number(sheet, h, c).is_some())
            {
                writes.push((h, c, n.clone()));
            }
            *slot = Some(TableColumn {
                id: next_id,
                name: n,
                ..TableColumn::default()
            });
            next_id += 1;
        }
        let mut table = t;
        table.columns = columns.into_iter().flatten().collect();
        table.reference = table_reference(
            area.top as i32 + 1,
            area.left as i32 + 1,
            area.bottom as i32 + 1,
            area.right as i32 + 1,
        );
        if let Some(xml) = table.kantor_auto_filter.take() {
            let last = area
                .bottom
                .saturating_sub(table.totals_row_count)
                .max(area.top);
            table.kantor_auto_filter = Some(with_auto_filter_ref(
                &xml,
                &format!(
                    "{}:{}",
                    crate::cell_name(area.top, area.left),
                    crate::cell_name(last, area.right)
                ),
            ));
        }
        // Calculated columns: formulas of the last data row fill new rows.
        let (old_top, old_bottom) = info.data_rows();
        let new_bottom = area.bottom.saturating_sub(table.totals_row_count);
        let fills: Vec<u32> = if old_top <= old_bottom && new_bottom > old_bottom {
            (area.left.max(old.left)..=area.right.min(old.right))
                .filter(|&c| self.input(sheet, old_bottom, c).starts_with('='))
                .filter(|&c| (old_bottom + 1..=new_bottom).all(|r| !self.is_filled(sheet, r, c)))
                .collect()
        } else {
            Vec::new()
        };
        self.batch(|wb| {
            // Header text first (as Format as Table), then the table.
            if !writes.is_empty() {
                wb.set_inputs(sheet, &writes)?;
            }
            wb.model
                .kantor_set_table(&tname, Some(table))
                .map_err(|e| anyhow!("{e}"))?;
            wb.history.edited();
            for c in fills {
                let src = CellRange::from_corners((old_bottom, c), (old_bottom, c)).area(sheet);
                if wb.model.auto_fill_rows(&src, new_bottom as i32 + 1).is_ok() {
                    wb.history.edited();
                }
            }
            wb.touch_table(&tname, None);
            Ok(())
        })
    }

    /// `ListRows.Add`: a new data row at `position` (1-based among the data
    /// rows; `None` adds one at the end, above the total row). A table with
    /// no data yet uses its empty row. Formulas of the neighbouring data
    /// row are filled in (calculated columns). One undo step. Returns the
    /// new row's sheet row.
    pub fn add_table_row(&mut self, name: &str, position: Option<u32>) -> Result<u32> {
        let (sheet, t, info) = self.script_table(name)?;
        let (top, bottom) = info.data_rows();
        let empty = self.table_body_is_empty(&t.name);
        let count = if empty { 0 } else { bottom + 1 - top };
        let pos = position.unwrap_or(count + 1);
        if pos < 1 || pos > count + 1 {
            bail!("The position is outside the table");
        }
        if empty {
            return Ok(top);
        }
        let row = top + pos - 1;
        if row >= MAX_ROWS {
            bail!("No room for a row");
        }
        let tname = t.name.clone();
        let area = info.area;
        self.batch(|wb| {
            if row > area.top && row <= area.bottom {
                // Inside the table: it grows.
                wb.insert_rows(sheet, row, 1)?;
                let now = wb.script_table(&tname)?.2;
                let (dt, db) = now.data_rows();
                let src = if row > dt { row - 1 } else { row + 1 };
                if (dt..=db).contains(&src) {
                    for c in now.area.left..=now.area.right {
                        if wb.input(sheet, src, c).starts_with('=') && !wb.is_filled(sheet, row, c)
                        {
                            let a = CellRange::from_corners((src, c), (src, c)).area(sheet);
                            if wb.model.auto_fill_rows(&a, row as i32 + 1).is_ok() {
                                wb.history.edited();
                            }
                        }
                    }
                }
            } else {
                // Under the last row (no total row), or above the first one
                // of a table without a header row.
                let line = CellRange::from_corners((row, area.left), (row, area.right));
                if row != area.bottom + 1 || !wb.free_for_table(sheet, line, &tname) {
                    wb.insert_rows(sheet, row, 1)?;
                }
                let now = wb.script_table(&tname)?.2.area;
                let want = CellRange {
                    top: now.top.min(row),
                    bottom: now.bottom.max(row),
                    ..now
                };
                if want != now {
                    wb.resize_table(&tname, want)?;
                }
            }
            Ok(row)
        })
    }

    /// `ListRow.Delete`: delete data row `index` (1-based) of table `name`
    /// (the sheet row goes). One undo step.
    pub fn delete_table_row(&mut self, name: &str, index: u32) -> Result<()> {
        let (sheet, _, info) = self.script_table(name)?;
        let (top, bottom) = info.data_rows();
        if index < 1 || top + index - 1 > bottom {
            bail!("no row {index}");
        }
        let row = top + index - 1;
        self.delete_rows(sheet, row, row)
    }

    /// `ListColumns.Add`: a new column at `position` (1-based; `None` adds
    /// one at the right end), named `ColumnN`. One undo step. Returns the new
    /// column's sheet column.
    pub fn add_table_column(&mut self, name: &str, position: Option<u32>) -> Result<u32> {
        let (sheet, t, info) = self.script_table(name)?;
        let count = t.columns.len() as u32;
        let pos = position.unwrap_or(count + 1);
        if pos < 1 || pos > count + 1 {
            bail!("The position is outside the table");
        }
        let area = info.area;
        let col = area.left + pos - 1;
        if col >= MAX_COLS {
            bail!("No room for a column");
        }
        let tname = t.name.clone();
        self.batch(|wb| {
            if col > area.left && col <= area.right {
                // Inside the table: it grows and names the column.
                wb.insert_columns(sheet, col, 1)?;
            } else {
                let line = CellRange::from_corners((area.top, col), (area.bottom, col));
                if col != area.right + 1 || !wb.free_for_table(sheet, line, &tname) {
                    wb.insert_columns(sheet, col, 1)?;
                }
                let now = wb.script_table(&tname)?.2.area;
                let want = CellRange {
                    left: now.left.min(col),
                    right: now.right.max(col),
                    ..now
                };
                if want != now {
                    wb.resize_table(&tname, want)?;
                }
            }
            Ok(col)
        })
    }

    /// `ListColumn.Delete`: delete column `index` (1-based) of table `name`
    /// (the sheet column goes). One undo step.
    pub fn delete_table_column(&mut self, name: &str, index: u32) -> Result<()> {
        let (sheet, _, info) = self.script_table(name)?;
        if index < 1 || info.area.left + index - 1 > info.area.right {
            bail!("no column {index}");
        }
        let col = info.area.left + index - 1;
        self.delete_columns(sheet, col, col)
    }
}
