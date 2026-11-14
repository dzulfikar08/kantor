//! Find & Replace in Sheets: searches the active sheet's displayed values
//! (or, with "Look in formulas", what was typed: formulas and raw input),
//! row by row, and selects and scrolls to the matching cell. Replace edits
//! the cell's input; Replace All is one undo step
//! ([`sheets_engine::Workbook::set_inputs`]).
//!
//! The bar sits over the grid but is not inside the grid element: the
//! grid's key-down listener starts a cell edit on any typed character.

use gpui_kit::*;
use kantor_core::find::{FindOptions, contains, pick, replace_all};
use kantor_ui::find_bar::{self, FindBar, FindBarEvent};
use kantor_ui::i18n::{t, tf};
use kantor_ui::metrics::*;

use crate::SheetsView;

/// The bar and the matching cells of the last search.
pub(crate) struct SheetsFind {
    bar: Entity<FindBar>,
    /// Matching cells `(row, col)` in reading order.
    found: Vec<(u32, u32)>,
    current: Option<usize>,
    _sub: Subscription,
}

impl SheetsFind {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SheetsView>) -> Self {
        let bar = cx.new(|cx| FindBar::new(true, window, cx));
        let sub = cx.subscribe_in(&bar, window, SheetsView::on_find_event);
        Self {
            bar,
            found: Vec::new(),
            current: None,
            _sub: sub,
        }
    }
}

/// The new input of a cell for a replacement, if its input has a match.
fn replaced(input: &str, query: &str, with: &str, opts: FindOptions) -> Option<String> {
    let (out, n) = replace_all(input, query, with, opts);
    (n > 0).then_some(out)
}

impl SheetsView {
    fn on_find_event(
        &mut self,
        _: &Entity<FindBar>,
        ev: &FindBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            FindBarEvent::Search => {
                self.find_search(cx);
                self.find_go(true, false, cx);
            }
            FindBarEvent::Next => self.find_step(true, cx),
            FindBarEvent::Prev => self.find_step(false, cx),
            FindBarEvent::Replace => self.find_replace(false, window, cx),
            FindBarEvent::ReplaceAll => self.find_replace(true, window, cx),
            FindBarEvent::Closed => {
                self.find.found.clear();
                self.find.current = None;
                let handle = self.table.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
                cx.notify();
            }
        }
    }

    /// Ctrl+F / Ctrl+H.
    pub fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.find
            .bar
            .update(cx, |bar, cx| bar.show(replace, None, window, cx));
    }

    /// F3 / Shift+F3 outside the bar.
    pub fn find_next_doc(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find.bar.read(cx).is_open() {
            self.open_find(false, window, cx);
            return;
        }
        self.find_step(forward, cx);
    }

    /// The matching cells of the active sheet.
    fn find_search(&mut self, cx: &mut Context<Self>) {
        let bar = self.find.bar.read(cx);
        let found = match bar.active_query(cx) {
            Some(q) => {
                let (opts, formulas) = (bar.options(), bar.look_in_formulas());
                let grid = self.table.read(cx).delegate();
                let (book, sheet) = (&grid.book, grid.sheet);
                book.filled_cells(sheet)
                    .into_iter()
                    .filter(|&(r, c)| {
                        let text = if formulas {
                            book.input(sheet, r, c)
                        } else {
                            book.display_value(sheet, r, c)
                        };
                        contains(&text, &q, opts)
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        self.find.found = found;
        self.find.current = self.find.found.iter().position(|&c| c == self.selected);
        self.report_find(cx);
    }

    fn report_find(&mut self, cx: &mut Context<Self>) {
        let (current, total) = (self.find.current, self.find.found.len());
        self.find
            .bar
            .update(cx, |bar, cx| bar.set_result(current, total, cx));
        cx.notify();
    }

    /// Search again (cells may have changed) and go to the next match.
    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.find_search(cx);
        self.find_go(forward, true, cx);
    }

    /// Select the match from the active cell: forward, the first at or
    /// after it (strictly after with `skip_at`); backward, the last before.
    fn find_go(&mut self, forward: bool, skip_at: bool, cx: &mut Context<Self>) {
        let Some(ix) = pick(&self.find.found, &self.selected, forward, skip_at) else {
            self.find.current = None;
            self.report_find(cx);
            return;
        };
        let (row, col) = self.find.found[ix];
        self.find.current = Some(ix);
        if self.selected != (row, col) {
            self.select_cell(row, col, cx);
            self.selected = (row, col);
        }
        self.report_find(cx);
    }

    /// Replace in the current match's cell (then go to the next), or in
    /// every matching cell as one undo step.
    fn find_replace(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.find_search(cx);
        let bar = self.find.bar.read(cx);
        let Some(q) = bar.active_query(cx) else {
            return;
        };
        let (with, opts) = (bar.replacement(cx), bar.options());
        let targets: Vec<(u32, u32)> = if all {
            self.find.found.clone()
        } else {
            match self.find.current {
                Some(i) => vec![self.find.found[i]],
                None => {
                    self.find_go(true, false, cx);
                    return;
                }
            }
        };
        let cells: Vec<(u32, u32, String)> = {
            let grid = self.table.read(cx).delegate();
            targets
                .iter()
                .filter_map(|&(r, c)| {
                    replaced(&grid.book.input(grid.sheet, r, c), &q, &with, opts).map(|v| (r, c, v))
                })
                .collect()
        };
        let n = cells.len();
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = grid.book.set_inputs(sheet, &cells);
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        });
        self.status = match result {
            Err(err) => format!("{err:#}").into(),
            Ok(()) if n == 0 => t("Nothing to replace: the matches are computed values").into(),
            Ok(()) if all && n == 1 => t("Replaced in 1 cell").into(),
            Ok(()) if all => tf("Replaced in {} cells", &[&n]).into(),
            Ok(()) => {
                let cell = sheets_engine::cell_name(cells[0].0, cells[0].1);
                tf("Replaced in {}", &[&cell]).into()
            }
        };
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        self.find_search(cx);
        if !all {
            self.find_go(true, true, cx);
        }
        cx.notify();
    }

    /// The bar, placed over the grid (below the toolbar and formula bar).
    pub(crate) fn render_find_bar(&self, cx: &App) -> Option<Div> {
        let bar = &self.find.bar;
        bar.read(cx).is_open().then(|| {
            div()
                .absolute()
                .left_0()
                .right_0()
                // Toolbar + formula bar, each with a 1 px border.
                .top(CONTROL_LG * 2. + SPACE_3 + px(2.))
                .child(bar.clone())
        })
    }
}

/// Register F3 / Ctrl+F / Ctrl+H for Sheets.
pub(crate) fn bind_keys(context: &'static str, cx: &mut App) {
    find_bar::bind_app_keys(context, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn replacement_only_touches_matching_inputs() {
        let o = FindOptions::default();
        assert_eq!(
            replaced("Cat food", "cat", "Dog", o).as_deref(),
            Some("Dog food")
        );
        assert_eq!(replaced("=SUM(A1)", "cat", "x", o), None);
    }
}
