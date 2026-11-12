//! Grid manipulation on top of the cell selection: whole-column / whole-row
//! selection from the headers (click, Shift+click, drag, right-click menu),
//! select all (corner, Ctrl+A), Ctrl+arrow jumps, the fill handle and
//! Ctrl+D / Ctrl+R, inserting and deleting rows and columns, autofit column
//! widths, frozen panes and the status bar's Sum / Average / Count.
//!
//! The decisions live in `sheets_engine::grid` (pure, unit-tested); this
//! module turns mouse and keyboard input into them and refreshes the table.
//!
//! **Selection shape.** [`SheetGrid`](crate::SheetGrid) keeps the active cell
//! (`anchor`) and the far corner (`end`) plus a [`Span`]: whole columns or
//! rows take their extent from the corners' columns or rows only, so the
//! active cell stays where the user sees it while the range covers every row.
//! Commands on whole columns or rows work on the used part of the sheet
//! ([`SheetsView::op_range`]), so clearing or copying column A does not visit
//! a million empty cells.

use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenu, PopupMenuItem};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::metrics::*;
use sheets_engine::{CellRange, Fill, Step, Workbook, cell_name};

use crate::calc::Job;
use crate::{GRID_CONTEXT, SheetsView};

actions!(
    sheets,
    [
        SelectAll,
        FillDown,
        FillRight,
        JumpUp,
        JumpDown,
        JumpLeft,
        JumpRight,
        JumpExtendUp,
        JumpExtendDown,
        JumpExtendLeft,
        JumpExtendRight,
        GoToStart,
        GoToEnd,
        ToggleFilter,
        HideRows,
        UnhideRows,
        HideColumns,
        UnhideColumns
    ]
);

pub(crate) fn bind_keys(cx: &mut App) {
    let ctx = Some(GRID_CONTEXT);
    // Ctrl and Super (Mac-style Cmd) alike.
    macro_rules! both {
        ($key:literal, $action:expr) => {
            [
                KeyBinding::new(
                    kantor_ui::keys::primary(concat!("ctrl-", $key)),
                    $action,
                    ctx,
                ),
                KeyBinding::new(concat!("super-", $key), $action, ctx),
            ]
        };
    }
    cx.bind_keys(
        [
            both!("a", SelectAll),
            both!("d", FillDown),
            both!("r", FillRight),
            both!("up", JumpUp),
            both!("down", JumpDown),
            both!("left", JumpLeft),
            both!("right", JumpRight),
            both!("shift-up", JumpExtendUp),
            both!("shift-down", JumpExtendDown),
            both!("shift-left", JumpExtendLeft),
            both!("shift-right", JumpExtendRight),
            both!("home", GoToStart),
            both!("end", GoToEnd),
            both!("shift-l", ToggleFilter),
            // Excel: Ctrl+9 / Ctrl+Shift+9 hide / unhide rows, Ctrl+0 /
            // Ctrl+Shift+0 columns (both spellings of the shifted key).
            both!("9", HideRows),
            both!("shift-9", UnhideRows),
            both!("(", UnhideRows),
            both!("0", HideColumns),
            both!("shift-0", UnhideColumns),
            both!(")", UnhideColumns),
        ]
        .into_iter()
        .flatten(),
    );
}

/// What a selection covers besides its corner cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Span {
    #[default]
    Cells,
    /// Every row of the corners' columns.
    Columns,
    /// Every column of the corners' rows.
    Rows,
    /// The whole sheet.
    All,
}

impl Span {
    pub(crate) fn range(self, anchor: (u32, u32), end: (u32, u32)) -> CellRange {
        match self {
            Span::Cells => CellRange::from_corners(anchor, end),
            Span::Columns => CellRange::columns(anchor.1, end.1),
            Span::Rows => CellRange::rows(anchor.0, end.0),
            Span::All => CellRange::all(),
        }
    }
}

/// A row or column header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Header {
    Column(u32),
    Row(u32),
}

/// A mouse drag that started on a header or the fill handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GridDrag {
    /// Extending whole columns (or rows) over the headers or cells.
    Columns,
    Rows,
    /// Dragging the fill handle; the fill it would make, if any.
    Fill(Option<Fill>),
}

/// A grid command for the palette, the header menus and shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridCommand {
    InsertRowsAbove,
    InsertRowsBelow,
    DeleteRows,
    InsertColumnsLeft,
    InsertColumnsRight,
    DeleteColumns,
    AutofitColumns,
    ResetColumnWidth,
    /// Fit the selection's rows to their content (no custom height).
    AutofitRows,
    /// Ask for a height for the selection's rows.
    RowHeight,
    FreezeTopRow,
    FreezeFirstColumn,
    /// Freeze the rows above and the columns left of the active cell.
    FreezePanes,
    Unfreeze,
    SelectAll,
    FillDown,
    FillRight,
    /// Sort by the active cell's column (a table, the selection, or the
    /// data region around the active cell).
    SortAscending,
    SortDescending,
    /// Sort by several columns.
    CustomSort,
    /// Filter buttons on or off (Ctrl+Shift+L).
    ToggleFilter,
    /// Clear every filter on the sheet (the buttons stay).
    ClearFilters,
    /// Re-apply the sheet's filters after edits.
    ReapplyFilters,
    /// Hide the selection's rows (Ctrl+9).
    HideRows,
    /// Show the hidden rows in the selection, except filtered-out ones
    /// (Ctrl+Shift+9).
    UnhideRows,
    /// Hide the selection's columns (Ctrl+0).
    HideColumns,
    /// Show the hidden columns in the selection (Ctrl+Shift+0).
    UnhideColumns,
}

/// Most cells measured for an autofit (keeps a huge column responsive).
const FIT_MAX_CELLS: usize = 10_000;

/// The width that fits every filled cell of column `col` (cell padding and
/// gridline included), or `None` for an empty column.
pub(crate) fn fit_width(
    book: &Workbook,
    sheet: u32,
    col: u32,
    window: &mut Window,
) -> Option<Pixels> {
    let texts = book.column_texts(sheet, col);
    if texts.is_empty() {
        return None;
    }
    let style = window.text_style();
    let mut widest = px(0.);
    for (text, bold) in texts.into_iter().take(FIT_MAX_CELLS) {
        let text = text.replace('\n', " ");
        let mut run = style.to_run(text.len());
        if bold {
            run.font.weight = FontWeight::BOLD;
        }
        let width = window
            .text_system()
            .shape_line(text.into(), FONT_BODY, &[run], None)
            .width;
        widest = widest.max(width);
    }
    Some((widest + SPACE_3 * 2. + GRID_LINE).ceil())
}

impl SheetsView {
    /// Register the grid actions on the grid element.
    pub(crate) fn grid_actions(&self, d: Div, cx: &mut Context<Self>) -> Div {
        d.on_action(cx.listener(|v, _: &SelectAll, w, cx| v.select_all(w, cx)))
            .on_action(
                cx.listener(|v, _: &FillDown, w, cx| v.grid_command(GridCommand::FillDown, w, cx)),
            )
            .on_action(
                cx.listener(|v, _: &FillRight, w, cx| {
                    v.grid_command(GridCommand::FillRight, w, cx)
                }),
            )
            .on_action(cx.listener(|v, _: &JumpUp, _, cx| v.jump(Step::Up, false, cx)))
            .on_action(cx.listener(|v, _: &JumpDown, _, cx| v.jump(Step::Down, false, cx)))
            .on_action(cx.listener(|v, _: &JumpLeft, _, cx| v.jump(Step::Left, false, cx)))
            .on_action(cx.listener(|v, _: &JumpRight, _, cx| v.jump(Step::Right, false, cx)))
            .on_action(cx.listener(|v, _: &JumpExtendUp, _, cx| v.jump(Step::Up, true, cx)))
            .on_action(cx.listener(|v, _: &JumpExtendDown, _, cx| v.jump(Step::Down, true, cx)))
            .on_action(cx.listener(|v, _: &JumpExtendLeft, _, cx| v.jump(Step::Left, true, cx)))
            .on_action(cx.listener(|v, _: &JumpExtendRight, _, cx| v.jump(Step::Right, true, cx)))
            .on_action(cx.listener(|v, _: &GoToStart, _, cx| v.go_to_corner(false, cx)))
            .on_action(cx.listener(|v, _: &GoToEnd, _, cx| v.go_to_corner(true, cx)))
            .on_action(cx.listener(|v, _: &ToggleFilter, w, cx| v.toggle_filter_doc(w, cx)))
            .on_action(
                cx.listener(|v, _: &HideRows, w, cx| v.grid_command(GridCommand::HideRows, w, cx)),
            )
            .on_action(cx.listener(|v, _: &UnhideRows, w, cx| {
                v.grid_command(GridCommand::UnhideRows, w, cx)
            }))
            .on_action(cx.listener(|v, _: &HideColumns, w, cx| {
                v.grid_command(GridCommand::HideColumns, w, cx)
            }))
            .on_action(cx.listener(|v, _: &UnhideColumns, w, cx| {
                v.grid_command(GridCommand::UnhideColumns, w, cx)
            }))
    }

    /// Make `anchor` the active cell and extend the selection to `end`
    /// (shaped by `span`). Without `scroll` the view stays where it is (a
    /// header click must not jump to row 1).
    pub(crate) fn select_range(
        &mut self,
        anchor: (u32, u32),
        end: (u32, u32),
        span: Span,
        scroll: bool,
        cx: &mut Context<Self>,
    ) {
        // Applied by the `SelectCell` handler, which would otherwise reset
        // the range to the new active cell.
        self.pending_selection = Some((end, span));
        self.table.update(cx, |table, cx| {
            let offset = table.horizontal_scroll_handle.offset();
            table.set_selected_cell(anchor.0 as usize, anchor.1 as usize + 1, cx);
            if !scroll {
                table.horizontal_scroll_handle.set_offset(offset);
                table
                    .vertical_scroll_handle
                    .0
                    .borrow_mut()
                    .deferred_scroll_to_item = None;
            }
        });
        cx.notify();
    }

    /// Move the selection's far corner, keeping the active cell.
    fn set_end(&mut self, end: (u32, u32), span: Span, cx: &mut Context<Self>) {
        let changed = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let changed = grid.end != end || grid.span != span;
            if changed {
                grid.end = end;
                grid.span = span;
                cx.notify();
            }
            changed
        });
        if changed {
            cx.notify();
        }
    }

    /// The first body row on screen.
    fn top_visible_row(&self, cx: &App) -> u32 {
        self.table.read(cx).visible_range().rows().start as u32
    }

    /// Select every cell, keeping the active cell.
    pub(crate) fn select_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let active = self.selected;
        self.set_end(active, Span::All, cx);
        self.focus_grid(window, cx);
        self.status = "Selected all cells".into();
        cx.notify();
    }

    /// Left button on a row or column header: select it (Shift extends from
    /// the active cell) and start a drag across headers.
    pub(crate) fn header_mouse_down(
        &mut self,
        header: Header,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let active = self.selected;
        match (header, shift) {
            (Header::Column(col), false) => {
                let row = self.top_visible_row(cx);
                self.select_range((row, col), (row, col), Span::Columns, false, cx);
            }
            (Header::Column(col), true) => self.set_end((active.0, col), Span::Columns, cx),
            (Header::Row(row), false) => {
                self.select_range((row, 0), (row, 0), Span::Rows, false, cx)
            }
            (Header::Row(row), true) => self.set_end((row, active.1), Span::Rows, cx),
        }
        self.grid_drag = Some(match header {
            Header::Column(_) => GridDrag::Columns,
            Header::Row(_) => GridDrag::Rows,
        });
        if self.editing.is_none() {
            self.focus_grid(window, cx);
        }
    }

    /// Right button on a header: select it unless it is already selected,
    /// so the menu acts on what was clicked.
    pub(crate) fn header_right_down(&mut self, header: Header, cx: &mut Context<Self>) {
        let range = self.range(cx);
        let inside = match header {
            Header::Column(col) => {
                range.is_whole_columns() && (range.left..=range.right).contains(&col)
            }
            Header::Row(row) => range.is_whole_rows() && (range.top..=range.bottom).contains(&row),
        };
        if inside {
            return;
        }
        match header {
            Header::Column(col) => {
                let row = self.top_visible_row(cx);
                self.select_range((row, col), (row, col), Span::Columns, false, cx);
            }
            Header::Row(row) => self.select_range((row, 0), (row, 0), Span::Rows, false, cx),
        }
    }

    /// The mouse moved over a header with the left button held.
    pub(crate) fn header_drag(&mut self, header: Header, cx: &mut Context<Self>) {
        let (row, col) = match header {
            Header::Column(col) => (self.selected.0, col),
            Header::Row(row) => (row, self.selected.1),
        };
        self.drag_over(row, col, cx);
    }

    /// Continue a header or fill-handle drag over the cell `(row, col)`.
    /// Returns whether such a drag is in progress (then the plain cell drag
    /// does nothing).
    pub(crate) fn drag_over(&mut self, row: u32, col: u32, cx: &mut Context<Self>) -> bool {
        match self.grid_drag {
            None => false,
            Some(GridDrag::Columns) => {
                self.set_end((self.selected.0, col), Span::Columns, cx);
                true
            }
            Some(GridDrag::Rows) => {
                self.set_end((row, self.selected.1), Span::Rows, cx);
                true
            }
            Some(GridDrag::Fill(old)) => {
                let fill = Fill::from_drag(self.range(cx), (row, col));
                if fill != old {
                    self.grid_drag = Some(GridDrag::Fill(fill));
                    self.table.update(cx, |table, cx| {
                        table.delegate_mut().fill_preview = fill.map(|f| f.target());
                        cx.notify();
                    });
                }
                true
            }
        }
    }

    /// Left button on the fill handle.
    pub(crate) fn fill_handle_down(&mut self, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.op_range(cx).is_none() {
            return;
        }
        self.grid_drag = Some(GridDrag::Fill(None));
        cx.notify();
    }

    /// A mouse button went down on the grid: forget a drag that ended
    /// outside it.
    pub(crate) fn reset_grid_drag(&mut self, cx: &mut Context<Self>) {
        self.grid_drag = None;
        self.table.update(cx, |table, cx| {
            if table.delegate_mut().fill_preview.take().is_some() {
                cx.notify();
            }
        });
    }

    /// A mouse button was released over the grid: finish a fill-handle drag.
    pub(crate) fn end_grid_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let drag = self.grid_drag;
        self.reset_grid_drag(cx);
        if let Some(GridDrag::Fill(Some(fill))) = drag {
            self.apply_fill(fill, window, cx);
        }
    }

    /// Ctrl+arrow moves the active cell to the data edge; with Shift the
    /// selection's far corner goes there instead. Stops at the grid's extent
    /// (the used range plus headroom).
    fn jump(&mut self, step: Step, extend: bool, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let selected = self.selected;
        let (to, span) = {
            let grid = self.table.read(cx).delegate();
            let from = if extend { grid.end } else { selected };
            let from = (from.0.min(grid.rows - 1), from.1.min(grid.cols - 1));
            let (r, c) = grid.book.jump(grid.sheet, from, step);
            ((r.min(grid.rows - 1), c.min(grid.cols - 1)), grid.span)
        };
        if !extend {
            self.select_range(to, to, Span::Cells, true, cx);
            return;
        }
        let span = if span == Span::All { Span::Cells } else { span };
        self.set_end(to, span, cx);
        self.table.update(cx, |table, cx| {
            if step.is_vertical() {
                table
                    .vertical_scroll_handle
                    .scroll_to_item(to.0 as usize, ScrollStrategy::Nearest);
            } else {
                table.scroll_to_col(to.1 as usize + 1, cx);
            }
        });
    }

    /// Ctrl+Home: A1. Ctrl+End: the last cell of the used range, as in Excel.
    fn go_to_corner(&mut self, end: bool, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let to = if end {
            let grid = self.table.read(cx).delegate();
            let (rows, cols) = grid.book.used_size(grid.sheet);
            (rows.saturating_sub(1), cols.saturating_sub(1))
        } else {
            (0, 0)
        };
        self.select_range(to, to, Span::Cells, true, cx);
    }

    /// The selection as commands should see it: whole columns and rows cut
    /// down to the used area (`None` if that leaves nothing).
    pub(crate) fn op_range(&self, cx: &App) -> Option<CellRange> {
        let range = self.range(cx);
        if !range.is_whole_columns() && !range.is_whole_rows() {
            return Some(range);
        }
        let grid = self.table.read(cx).delegate();
        let (rows, cols) = grid.book.used_size(grid.sheet);
        range.clip(rows, cols)
    }

    /// Sum / Average / Count of the selection, for the status bar.
    pub fn selection_summary(&self, cx: &App) -> Option<String> {
        let range = self.range(cx);
        if range.is_single() {
            return None;
        }
        let grid = self.table.read(cx).delegate();
        grid.book.range_stats(grid.sheet, range).summary()
    }

    /// Fill, in the background for big workbooks: IronCalc recalculates
    /// the whole workbook after a fill, even in manual calculation.
    fn apply_fill(&mut self, fill: Fill, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let sheet = self.table.read(cx).delegate().sheet;
        self.run_book_job(
            Job::Editing("Filling"),
            move |book| book.fill(sheet, &fill),
            |this, result, _, _, cx| match result {
                Ok(target) => {
                    // Select what was filled, keeping the active cell.
                    let (row, col) = this.selected;
                    let end_row = if target.top == row {
                        target.bottom
                    } else {
                        target.top
                    };
                    let end_col = if target.left == col {
                        target.right
                    } else {
                        target.left
                    };
                    this.set_end((end_row, end_col), Span::Cells, cx);
                    this.status = format!("Filled {}", target.name()).into();
                }
                Err(err) => this.status = format!("{err:#}").into(),
            },
            window,
            cx,
        );
    }

    /// Run a grid command (palette, header menu, shortcut).
    pub fn grid_command(
        &mut self,
        command: GridCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let range = self.range(cx);
        let active = self.selected;
        // The rows and columns a structural command acts on: the selection's,
        // unless it spans the whole sheet that way (then the active cell's).
        let rows = if range.is_whole_columns() {
            (active.0, active.0)
        } else {
            (range.top, range.bottom)
        };
        let cols = if range.is_whole_rows() {
            (active.1, active.1)
        } else {
            (range.left, range.right)
        };
        let n_rows = rows.1 - rows.0 + 1;
        let n_cols = cols.1 - cols.0 + 1;

        let op: Op = match command {
            GridCommand::SelectAll => {
                self.select_all(window, cx);
                return;
            }
            GridCommand::FillDown | GridCommand::FillRight => {
                let fill = match self.op_range(cx) {
                    Some(r) if command == GridCommand::FillDown => Fill::down(r),
                    Some(r) => Fill::right(r),
                    None => None,
                };
                match fill {
                    Some(fill) => self.apply_fill(fill, window, cx),
                    None => {
                        self.status = "Nothing to fill from".into();
                        cx.notify();
                    }
                }
                return;
            }
            GridCommand::AutofitColumns | GridCommand::ResetColumnWidth => {
                self.size_columns(cols, command == GridCommand::AutofitColumns, window, cx);
                return;
            }
            GridCommand::AutofitRows => {
                self.autofit_rows_doc(window, cx);
                return;
            }
            GridCommand::RowHeight => {
                self.row_height_doc(window, cx);
                return;
            }
            GridCommand::SortAscending | GridCommand::SortDescending => {
                self.sort_doc(command == GridCommand::SortDescending, window, cx);
                return;
            }
            GridCommand::CustomSort => {
                self.open_sort_panel(cx);
                return;
            }
            GridCommand::ToggleFilter => {
                self.toggle_filter_doc(window, cx);
                return;
            }
            GridCommand::ClearFilters | GridCommand::ReapplyFilters => {
                self.clear_filters_doc(command == GridCommand::ReapplyFilters, window, cx);
                return;
            }
            GridCommand::HideRows
            | GridCommand::UnhideRows
            | GridCommand::HideColumns
            | GridCommand::UnhideColumns => {
                self.hide_command(command, range, window, cx);
                return;
            }
            GridCommand::InsertRowsAbove => Box::new(move |b, s| {
                b.insert_rows(s, rows.0, n_rows)?;
                Ok(format!("Inserted {}", count(n_rows, "row")))
            }),
            GridCommand::InsertRowsBelow => Box::new(move |b, s| {
                b.insert_rows(s, rows.1 + 1, n_rows)?;
                Ok(format!("Inserted {}", count(n_rows, "row")))
            }),
            GridCommand::DeleteRows => Box::new(move |b, s| {
                b.delete_rows(s, rows.0, rows.1)?;
                Ok(format!("Deleted {}", count(n_rows, "row")))
            }),
            GridCommand::InsertColumnsLeft => Box::new(move |b, s| {
                b.insert_columns(s, cols.0, n_cols)?;
                Ok(format!("Inserted {}", count(n_cols, "column")))
            }),
            GridCommand::InsertColumnsRight => Box::new(move |b, s| {
                b.insert_columns(s, cols.1 + 1, n_cols)?;
                Ok(format!("Inserted {}", count(n_cols, "column")))
            }),
            GridCommand::DeleteColumns => Box::new(move |b, s| {
                b.delete_columns(s, cols.0, cols.1)?;
                Ok(format!("Deleted {}", count(n_cols, "column")))
            }),
            GridCommand::FreezeTopRow => freeze(1, 0),
            GridCommand::FreezeFirstColumn => freeze(0, 1),
            GridCommand::FreezePanes => freeze(active.0, active.1),
            GridCommand::Unfreeze => freeze(0, 0),
        };
        // Measured row heights move with inserted and deleted rows.
        let shift = match command {
            GridCommand::InsertRowsAbove => Some((rows.0, i64::from(n_rows))),
            GridCommand::InsertRowsBelow => Some((rows.1 + 1, i64::from(n_rows))),
            GridCommand::DeleteRows => Some((rows.0, -i64::from(n_rows))),
            _ => None,
        };
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = op(&mut grid.book, sheet);
            if let (Ok(_), Some((at, count))) = (&res, shift) {
                grid.layout.shift(at, count);
            }
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        });
        self.status = match result {
            Ok(msg) => msg.into(),
            Err(err) => format!("{err:#}").into(),
        };
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    /// Hide or unhide the rows or columns of `range` (the selection), as one
    /// undo step. Unhide leaves rows a filter hides alone, as Excel does
    /// (`sheets_engine::hide`).
    fn hide_command(
        &mut self,
        command: GridCommand,
        range: CellRange,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let book = &mut grid.book;
            let res = match command {
                GridCommand::HideRows => book
                    .hide_rows(sheet, range.top, range.bottom)
                    .map(|n| hide_status(n, "row", "Hid", "already hidden")),
                GridCommand::UnhideRows => {
                    book.unhide_rows(sheet, range.top, range.bottom)
                        .map(|done| match (done.shown, done.filtered) {
                            (0, 0) => "No hidden rows in the selection".to_owned(),
                            (0, f) => format!(
                                "{} hidden by a filter; clear the filter to show {}",
                                count(f, "row"),
                                if f == 1 { "it" } else { "them" }
                            ),
                            (n, 0) => format!("Unhid {}", count(n, "row")),
                            (n, f) => format!(
                                "Unhid {}; {} hidden by a filter stay hidden",
                                count(n, "row"),
                                count(f, "row")
                            ),
                        })
                }
                GridCommand::HideColumns => book
                    .set_columns_hidden(sheet, range.left, range.right, true)
                    .map(|n| hide_status(n, "column", "Hid", "already hidden")),
                _ => book
                    .set_columns_hidden(sheet, range.left, range.right, false)
                    .map(|n| hide_status(n, "column", "Unhid", "hidden in the selection")),
            };
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        });
        self.status = match result {
            Ok(msg) => msg.into(),
            Err(err) => format!("{err:#}").into(),
        };
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    /// Autofit (or reset to the standard width) columns `cols`, as one undo
    /// step. Autofit skips empty columns.
    fn size_columns(
        &mut self,
        cols: (u32, u32),
        fit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let default = grid.book.default_column_width(sheet);
            let used = grid.book.used_size(sheet).1;
            // Autofitting past the used columns would only skip empty ones.
            let last = if fit {
                cols.1.min(used.saturating_sub(1)).max(cols.0)
            } else {
                cols.1.min(cols.0 + 1000)
            };
            let widths: Vec<(u32, f64)> = (cols.0..=last)
                .filter_map(|col| {
                    let width = if fit {
                        f64::from(f32::from(fit_width(&grid.book, sheet, col, window)?))
                    } else {
                        default
                    };
                    Some((col, width))
                })
                .collect();
            let res = grid.book.set_column_widths(sheet, &widths);
            table.refresh(cx);
            cx.notify();
            res
        });
        self.status = match result {
            Ok(true) if fit => "Fitted column widths to their content".into(),
            Ok(true) => "Reset column widths".into(),
            Ok(false) => "Column widths unchanged".into(),
            Err(err) => format!("{err:#}").into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }
}

/// A structural edit of the workbook on one sheet; returns the status line.
type Op = Box<dyn FnOnce(&mut Workbook, u32) -> anyhow::Result<String>>;

/// `1 row`, `3 columns`.
fn count(n: u32, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// `Hid 3 rows`, or `No rows already hidden`-style news when nothing changed.
fn hide_status(n: u32, what: &str, verb: &str, none: &str) -> String {
    if n == 0 {
        match none {
            "already hidden" => format!("The selected {what}s are already hidden"),
            _ => format!("No hidden {what}s in the selection"),
        }
    } else {
        format!("{verb} {}", count(n, what))
    }
}

/// An op setting the frozen rows and columns.
fn freeze(rows: u32, cols: u32) -> Op {
    Box::new(move |book, sheet| {
        book.set_frozen(sheet, rows, cols)?;
        Ok(match (rows, cols) {
            (0, 0) => "Panes unfrozen".to_owned(),
            (r, 0) => format!("Froze the top {}", count(r, "row")),
            (0, c) => format!("Froze the first {}", count(c, "column")),
            (r, c) => format!("Froze panes above and left of {}", cell_name(r, c)),
        })
    })
}

/// The right-click menu of a row or column header.
fn header_menu(menu: PopupMenu, header: Header, view: &WeakEntity<SheetsView>) -> PopupMenu {
    let item = |label: &'static str, command: GridCommand| {
        let view = view.clone();
        PopupMenuItem::new(label).on_click(move |_, window, cx| {
            let _ = view.update(cx, |v, cx| v.grid_command(command, window, cx));
        })
    };
    match header {
        Header::Column(_) => menu
            .item(item("Insert Column Left", GridCommand::InsertColumnsLeft))
            .item(item("Insert Column Right", GridCommand::InsertColumnsRight))
            .item(item("Delete Columns", GridCommand::DeleteColumns))
            .separator()
            .item(item("Autofit Column Width", GridCommand::AutofitColumns))
            .item(item("Standard Column Width", GridCommand::ResetColumnWidth))
            .item(item("Hide Columns", GridCommand::HideColumns))
            .item(item("Unhide Columns", GridCommand::UnhideColumns))
            .separator()
            .item(item("Sort A to Z", GridCommand::SortAscending))
            .item(item("Sort Z to A", GridCommand::SortDescending))
            .item(item("Custom Sort…", GridCommand::CustomSort))
            .item(item("Filter", GridCommand::ToggleFilter))
            .separator()
            .item(item("Freeze Panes", GridCommand::FreezePanes))
            .item(item("Freeze First Column", GridCommand::FreezeFirstColumn))
            .item(item("Unfreeze Panes", GridCommand::Unfreeze)),
        Header::Row(_) => menu
            .item(item("Insert Row Above", GridCommand::InsertRowsAbove))
            .item(item("Insert Row Below", GridCommand::InsertRowsBelow))
            .item(item("Delete Rows", GridCommand::DeleteRows))
            .separator()
            .item(item("Row Height…", GridCommand::RowHeight))
            .item(item("Autofit Row Height", GridCommand::AutofitRows))
            .item(item("Hide Rows", GridCommand::HideRows))
            .item(item("Unhide Rows", GridCommand::UnhideRows))
            .separator()
            .item(item("Freeze Panes", GridCommand::FreezePanes))
            .item(item("Freeze Top Row", GridCommand::FreezeTopRow))
            .item(item("Unfreeze Panes", GridCommand::Unfreeze)),
    }
}

/// Wire a header element for selection, dragging and its menu.
pub(crate) fn header_element(
    element: Stateful<Div>,
    header: Header,
    view: &WeakEntity<SheetsView>,
) -> AnyElement {
    let down = view.clone();
    let right = view.clone();
    let drag = view.clone();
    let menu_view = view.clone();
    element
        .on_mouse_down(MouseButton::Left, move |e, window, cx| {
            // Keep the table from turning this into a cell click.
            cx.stop_propagation();
            let _ = down.update(cx, |v, cx| {
                v.header_mouse_down(header, e.modifiers.shift, window, cx)
            });
        })
        .on_mouse_down(MouseButton::Right, move |_, _, cx| {
            let _ = right.update(cx, |v, cx| v.header_right_down(header, cx));
        })
        .on_mouse_move(move |e, _, cx| {
            if e.pressed_button == Some(MouseButton::Left) {
                let _ = drag.update(cx, |v, cx| v.header_drag(header, cx));
            }
        })
        .context_menu(move |menu, _, _| header_menu(menu, header, &menu_view))
        .into_any_element()
}

/// The select-all corner above the row numbers.
pub(crate) fn corner_element(element: Stateful<Div>, view: &WeakEntity<SheetsView>) -> AnyElement {
    let view = view.clone();
    element
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            let _ = view.update(cx, |v, cx| v.select_all(window, cx));
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    // `gpui_kit::*` exports a `test` attribute macro that shadows the
    // built-in one.
    #[std::prelude::v1::test]
    fn span_shapes_the_range() {
        let (anchor, end) = ((5, 1), (2, 3));
        assert_eq!(Span::Cells.range(anchor, end).name(), "B3:D6");
        assert_eq!(Span::Columns.range(anchor, end).name(), "B:D");
        assert_eq!(Span::Rows.range(anchor, end).name(), "3:6");
        assert_eq!(Span::All.range(anchor, end), CellRange::all());
    }

    #[std::prelude::v1::test]
    fn counts() {
        assert_eq!(count(1, "row"), "1 row");
        assert_eq!(count(3, "column"), "3 columns");
    }
}
