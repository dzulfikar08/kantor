//! Pivot tables in the Sheets view: the status bar says when the active cell
//! is in one, and Refresh PivotTable (Alt+F5) / Refresh All (Ctrl+Alt+F5)
//! recompute them from their source (`sheets_engine::pivot`), as one undo
//! step each.

use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::{KEY_CONTEXT, SheetsView};

actions!(sheets, [RefreshPivotTable, RefreshAllPivotTables]);

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-f5", RefreshPivotTable, Some(KEY_CONTEXT)),
        KeyBinding::new("ctrl-alt-f5", RefreshAllPivotTables, Some(KEY_CONTEXT)),
    ]);
}

impl SheetsView {
    /// `PivotTable · PivotTable1` when the active cell is in a pivot table
    /// (with why it can't be refreshed, if it can't).
    pub fn pivot_status(&self, cx: &App) -> Option<String> {
        if self.busy.is_some() {
            return None;
        }
        let grid = self.table.read(cx).delegate();
        let (row, col) = self.selected;
        let p = grid.book.pivot_at(grid.sheet, row, col)?;
        Some(match p.refresh_problem {
            None => format!("PivotTable · {}", p.name),
            Some(_) => format!("PivotTable · {} (can't refresh here)", p.name),
        })
    }

    /// Refresh the pivot table at the active cell (and those sharing its
    /// data), or every pivot table of the workbook.
    pub fn refresh_pivots_doc(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (row, col) = self.selected;
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = if all {
                grid.book.refresh_all_pivots()
            } else {
                grid.book.refresh_pivot(sheet, row, col)
            };
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        });
        self.status = match result {
            Ok(0) => "This workbook has no PivotTables to refresh".into(),
            Ok(1) => "PivotTable refreshed".into(),
            Ok(n) => format!("{n} PivotTables refreshed").into(),
            Err(e) => format!("{e:#}").into(),
        };
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    pub(crate) fn pivot_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(
            cx.listener(|this, _: &RefreshPivotTable, w, cx| this.refresh_pivots_doc(false, w, cx)),
        )
        .on_action(cx.listener(|this, _: &RefreshAllPivotTables, w, cx| {
            this.refresh_pivots_doc(true, w, cx)
        }))
    }
}
