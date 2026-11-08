//! Hooks for the AI formula assistant, which lives in the shell: what it may
//! read (the workbook, through `read_book`) and how it inserts a formula.

use gpui_kit::*;
use sheets_engine::{Workbook, cell_name};

use crate::SheetsView;

/// The cell a formula is written for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiTarget {
    pub sheet: u32,
    pub row: u32,
    pub col: u32,
}

impl SheetsView {
    /// The active cell on the visible sheet.
    pub fn ai_target(&self, cx: &App) -> AiTarget {
        let (row, col) = self.selected;
        AiTarget {
            sheet: self.table.read(cx).delegate().sheet,
            row,
            col,
        }
    }

    /// Read the workbook and the visible sheet's index.
    pub fn read_book<R>(&self, cx: &App, f: impl FnOnce(&Workbook, u32) -> R) -> R {
        let grid = self.table.read(cx).delegate();
        f(&grid.book, grid.sheet)
    }

    /// Put `formula` into `target` as one undo step (an edit in progress is
    /// committed first).
    pub fn insert_formula_doc(
        &mut self,
        target: AiTarget,
        formula: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.editing.is_some() {
            self.commit_edit(None, window, cx);
        }
        let sheets = self.table.read(cx).delegate().book.sheet_names().len() as u32;
        if target.sheet >= sheets {
            return Err("the sheet was deleted".into());
        }
        self.set_cell(target.sheet, target.row, target.col, formula, cx)
            .map_err(|e| format!("{e:#}"))?;
        self.status = format!("Formula inserted in {}", cell_name(target.row, target.col)).into();
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
        Ok(())
    }
}
