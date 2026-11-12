//! Hooks for programs driving Kantor (the MCP server, `kantor mcp`): read
//! the workbook, edit it the way the user's own commands do (one undo step,
//! dirty state, redraw) and show what changed by selecting it.

use anyhow::bail;
use gpui_kit::*;
use sheets_engine::{CellRange, Workbook, cell_name};

use crate::SheetsView;
use crate::grid_ops::Span;

impl SheetsView {
    /// Read the workbook; `f` also gets the index of the sheet on screen.
    pub fn agent_read<R>(&self, cx: &App, f: impl FnOnce(&Workbook, u32) -> R) -> R {
        let grid = self.table.read(cx).delegate();
        f(&grid.book, grid.sheet)
    }

    /// The sheet on screen, the selected range and the active cell.
    pub fn agent_selection(&self, cx: &App) -> (u32, CellRange, (u32, u32)) {
        (self.grid_sheet(cx), self.range(cx), self.selected)
    }

    /// Edit the workbook as one undo step, then show `show` (a sheet and
    /// range) selected and "Agent: {what}" in the status bar. Refused
    /// while the user is typing in a cell.
    pub fn agent_edit<R>(
        &mut self,
        what: &str,
        show: Option<(u32, CellRange)>,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Workbook) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        self.refuse_while_editing()?;
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let res = grid.book.batch(f);
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        });
        if result.is_ok() {
            if let Some((sheet, range)) = show {
                self.agent_show(sheet, range, cx);
            }
            self.status = format!("Agent: {what}").into();
        }
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
        result
    }

    /// Select `range` on `sheet` (switching to it), as if the user had.
    pub fn agent_select(
        &mut self,
        sheet: u32,
        range: CellRange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.refuse_while_editing()?;
        self.agent_show(sheet, range, cx);
        self.load_selected_into_formula(window, cx);
        cx.notify();
        Ok(())
    }

    /// Save to the workbook's own file (`None`) or to `path` (Save As
    /// rules: the caller has checked overwriting). Returns the status text.
    pub fn agent_save(
        &mut self,
        path: Option<&std::path::Path>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<String> {
        self.refuse_while_editing()?;
        if self.saving.is_some() {
            bail!("a save of this workbook is still being written; try again in a moment");
        }
        let result = self.table.update(cx, |table, _| {
            let book = &mut table.delegate_mut().book;
            match path {
                Some(path) => book.save_as(path),
                None if book.path().is_none() => {
                    bail!("this workbook has no .xlsx file yet (new or imported); give a path")
                }
                None => book.save(),
            }
        });
        self.status = match result {
            Ok(()) => {
                self.disk_guard = None;
                self.saved_status(false, cx).into()
            }
            Err(err) => format!("Save failed: {err:#}").into(),
        };
        self.update_title(window, cx);
        cx.notify();
        if self.status.starts_with("Save failed") {
            bail!("{}", self.status)
        }
        Ok(self.status.to_string())
    }

    /// Err while the workbook can't be read or edited: it's still opening
    /// or recalculating (the grid holds a stand-in meanwhile), or the user
    /// is typing in a cell.
    pub fn agent_ready(&self) -> anyhow::Result<()> {
        self.refuse_while_editing()
    }

    fn refuse_while_editing(&self) -> anyhow::Result<()> {
        if self.busy.is_some() {
            bail!("this workbook is still opening or recalculating; try again in a moment");
        }
        match self.editing {
            Some((row, col)) => bail!(
                "the user is typing in cell {} of this workbook; try again in a moment",
                cell_name(row, col)
            ),
            None => Ok(()),
        }
    }

    fn agent_show(&mut self, sheet: u32, range: CellRange, cx: &mut Context<Self>) {
        if self.grid_sheet(cx) != sheet {
            self.show_sheet(sheet, cx);
        }
        let end = (range.bottom, range.right);
        self.select_range((range.top, range.left), end, Span::Cells, true, cx);
    }
}
