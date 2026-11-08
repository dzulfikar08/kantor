//! Kantor Sheets UI.
//!
//! The grid is GPUI Kit's virtualised `DataTable`; column 0 is a fixed row-number
//! gutter, so table column `n` maps to sheet column `n - 1`.
//!
//! Editing is spreadsheet-style: with a cell selected, typing starts an edit,
//! Enter/F2 edits the existing content, Delete clears, Escape cancels and Enter
//! commits and moves down. The edit happens in an editor drawn over the cell,
//! mirrored live in the formula bar (see [`cell_editor`]).
//!
//! The table only knows a single selected cell, which is the *active* cell
//! (the anchor of a range). Range selection is ours: Shift+arrows, Shift+click
//! and mouse drag move the range's other corner, kept in [`SheetGrid`] and
//! drawn by `render_td`.
//!
//! Ctrl+C / Ctrl+X / Ctrl+V copy and paste the range as tab-separated text
//! (inputs, so formulas copy as typed). Cut copies and clears the range at
//! once, rather than waiting for the paste.
//!
//! Formulas get help while they are typed: clicking or arrowing to cells
//! inserts their references, references are coloured, F4 toggles `$`, and
//! function names autocomplete; see [`point`].
//!
//! Formatting (bold/italic/underline/strikethrough, alignment, number formats)
//! applies to the whole range as one undo step; see [`toolbar`]. Column widths
//! come from the workbook and a resize (or a double-click autofit on the
//! header border) is saved back to it. Row heights are kept in the file but
//! not drawn: the table has one height for every row.
//!
//! Header selection, Ctrl+arrows, the fill handle, inserting/deleting rows and
//! columns, frozen panes and the status-bar sums are in [`grid_ops`].

extern crate gpui_kit as gpui;

mod ai;
mod cell_editor;
mod find;
mod grid_ops;
mod point;

use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    Sizable as _, h_flex,
    input::{Input, InputEvent, InputState},
    table::{Column, DataTable, TableDelegate, TableEvent, TableState},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{
    CellRange, CellStyle, SourceFormat, StyleChange, Workbook, cell_name, column_name,
};

mod toolbar;

pub use ai::AiTarget;
use cell_editor::{CellEditor, EditMode};
pub use grid_ops::GridCommand;
use grid_ops::{GridDrag, Header, Span};
pub use sheets_engine::{HAlign, NumberFormat};

actions!(
    sheets,
    [
        Save,
        SaveAs,
        Undo,
        Redo,
        Copy,
        Cut,
        Paste,
        ExtendUp,
        ExtendDown,
        ExtendLeft,
        ExtendRight,
        ToggleBold,
        ToggleItalic,
        ToggleUnderline
    ]
);

/// A formatting command from the toolbar, a shortcut or the palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Toggles, like the rest of the font flags: on for the whole range
    /// unless the active cell already has it.
    Bold,
    Italic,
    Underline,
    Strikethrough,
    /// Choosing the active alignment again goes back to `General`.
    Align(HAlign),
    Number(NumberFormat),
}

const KEY_CONTEXT: &str = "KantorSheets";
/// The grid, not the formula bar: there Ctrl+Z undoes typing instead.
const GRID_CONTEXT: &str = "KantorSheetsGrid";
const MIN_ROWS: u32 = 1000;
const MIN_COLS: u32 = 26;
/// Below this pane width the formula bar drops its labels.
const COMPACT_WIDTH: Pixels = px(560.);

/// The selected cell, for the inspector.
pub struct CellInfo {
    pub address: String,
    /// Formula or raw input.
    pub input: String,
    /// Displayed (formatted) value.
    pub value: String,
}

/// Workbook facts, for the inspector.
pub struct BookInfo {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub sheets: Vec<String>,
    pub active_sheet: usize,
    /// Used range of the active sheet, `(rows, cols)`.
    pub used: (u32, u32),
}

/// Status line after opening: imports say that Save writes a copy as `.xlsx`.
fn opened_status(book: &Workbook) -> String {
    let mut s = format!("Opened {}", book.title());
    if book.source_format().is_imported() {
        s.push_str(" (imported; Save creates an .xlsx copy)");
    }
    for note in book.import_notes() {
        s.push_str("; ");
        s.push_str(note);
    }
    s
}

/// Register Sheets key bindings. Both Ctrl and Super work (Mac-style Cmd = Super).
pub fn init(cx: &mut App) {
    let ctx = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new(kantor_ui::keys::primary("ctrl-s"), Save, ctx),
        KeyBinding::new("super-s", Save, ctx),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-shift-s"), SaveAs, ctx),
        KeyBinding::new("super-shift-s", SaveAs, ctx),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-z"), Undo, Some(GRID_CONTEXT)),
        KeyBinding::new("super-z", Undo, Some(GRID_CONTEXT)),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-z"),
            Redo,
            Some(GRID_CONTEXT),
        ),
        KeyBinding::new("super-shift-z", Redo, Some(GRID_CONTEXT)),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-y"), Redo, Some(GRID_CONTEXT)),
        // The formula bar keeps its own text clipboard.
        KeyBinding::new(kantor_ui::keys::primary("ctrl-c"), Copy, Some(GRID_CONTEXT)),
        KeyBinding::new("super-c", Copy, Some(GRID_CONTEXT)),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-x"), Cut, Some(GRID_CONTEXT)),
        KeyBinding::new("super-x", Cut, Some(GRID_CONTEXT)),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-v"),
            Paste,
            Some(GRID_CONTEXT),
        ),
        KeyBinding::new("super-v", Paste, Some(GRID_CONTEXT)),
        KeyBinding::new("shift-up", ExtendUp, Some(GRID_CONTEXT)),
        KeyBinding::new("shift-down", ExtendDown, Some(GRID_CONTEXT)),
        KeyBinding::new("shift-left", ExtendLeft, Some(GRID_CONTEXT)),
        KeyBinding::new("shift-right", ExtendRight, Some(GRID_CONTEXT)),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-b"),
            ToggleBold,
            Some(GRID_CONTEXT),
        ),
        KeyBinding::new("super-b", ToggleBold, Some(GRID_CONTEXT)),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-i"),
            ToggleItalic,
            Some(GRID_CONTEXT),
        ),
        KeyBinding::new("super-i", ToggleItalic, Some(GRID_CONTEXT)),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-u"),
            ToggleUnderline,
            Some(GRID_CONTEXT),
        ),
        KeyBinding::new("super-u", ToggleUnderline, Some(GRID_CONTEXT)),
    ]);
    cell_editor::bind_keys(cx);
    point::bind_keys(cx);
    find::bind_keys(KEY_CONTEXT, cx);
    grid_ops::bind_keys(cx);
}

/// Feeds one sheet of a [`Workbook`] to the table.
pub struct SheetGrid {
    book: Workbook,
    sheet: u32,
    rows: u32,
    cols: u32,
    /// The view, for mouse selection in `render_td`.
    view: WeakEntity<SheetsView>,
    /// Selected range: the active cell (same as the table's selected cell) and
    /// the opposite corner, in sheet coordinates `(row, col)`.
    anchor: (u32, u32),
    end: (u32, u32),
    /// Whether the range is whole columns or rows (see [`grid_ops`]).
    span: Span,
    /// The range a fill-handle drag would cover, drawn as a preview.
    fill_preview: Option<CellRange>,
    /// The in-cell editor, drawn over its cell while an edit is in progress.
    editor: Option<CellEditor>,
    /// References in the formula being edited: cells on this sheet with their
    /// colour slots, and every reference's place in the text.
    ref_marks: Vec<(CellRange, usize)>,
    ref_spans: Vec<(Range<usize>, usize)>,
}

impl SheetGrid {
    fn new(book: Workbook, view: WeakEntity<SheetsView>) -> Self {
        let mut grid = Self {
            book,
            sheet: 0,
            rows: 0,
            cols: 0,
            view,
            anchor: (0, 0),
            end: (0, 0),
            span: Span::Cells,
            fill_preview: None,
            editor: None,
            ref_marks: Vec::new(),
            ref_spans: Vec::new(),
        };
        grid.resize();
        grid
    }

    fn range(&self) -> CellRange {
        self.span.range(self.anchor, self.end)
    }

    /// Recompute grid extents: the used range plus headroom, never below the minimum.
    fn resize(&mut self) {
        let (r, c) = self.book.used_size(self.sheet);
        self.rows = (r + 200).max(MIN_ROWS);
        self.cols = (c + 10).max(MIN_COLS);
    }

    /// The formula's references with their colours, for the editors.
    fn ref_span_colors(&self, t: &kantor_ui::Tokens) -> Vec<(Range<usize>, Hsla)> {
        self.ref_spans
            .iter()
            .map(|(span, slot)| (span.clone(), point::ref_color(t, *slot)))
            .collect()
    }
}

impl TableDelegate for SheetGrid {
    fn columns_count(&self, _: &App) -> usize {
        self.cols as usize + 1
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows as usize
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        if col_ix == 0 {
            Column::new("#", "")
                .width(ROW_GUTTER_WIDTH)
                .fixed_left()
                .resizable(false)
                .selectable(false)
                // `render_td` / `render_th` fill the cell (header background).
                .p_0()
        } else {
            let col = col_ix as u32 - 1;
            let name = column_name(col);
            let width = self.book.column_width(self.sheet, col);
            let frozen = col < self.book.frozen(self.sheet).1;
            Column::new(SharedString::from(name.clone()), name)
                .width(px(width as f32))
                .min_width(px(24.))
                .resizable(true)
                // Frozen columns stay put while the rest scrolls.
                .when(frozen, |c| c.fixed_left())
                // `render_td` fills the whole cell (to draw the range) and pads itself.
                .p_0()
        }
    }

    /// Double-click on a column's right border: the width of its widest cell.
    fn fit_column(
        &mut self,
        col_ix: usize,
        window: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Option<Pixels> {
        let col = (col_ix as u32).checked_sub(1)?;
        grid_ops::fit_width(&self.book, self.sheet, col, window)
    }

    /// Column letters, centred; the columns of the selection are highlighted
    /// with an accent edge towards the grid, like Excel.
    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let t = tokens(cx);
        // Absolute, so it fills the header cell including the padding the
        // table reserves for a sort icon.
        let cell = div().absolute().inset_0();
        if col_ix == 0 {
            // The corner selects everything.
            let all = self.range() == CellRange::all();
            let cell = cell
                .id("select-all")
                .cursor_pointer()
                .when(all, |d| d.bg(t.grid_header_active))
                .hover(|s| s.bg(t.grid_header_active));
            return grid_ops::corner_element(cell, &self.view);
        }
        let col = col_ix as u32 - 1;
        let range = self.range();
        let active = (range.left..=range.right).contains(&col);
        let whole = active && range.is_whole_columns();
        let cell = cell
            .id(("col-head", col_ix))
            .flex()
            .items_center()
            .justify_center()
            .child(SharedString::from(column_name(col)))
            .child(grid_line_right(t.grid_line))
            .when(active, |d| {
                d.bg(t.grid_header_active)
                    .text_color(t.fg)
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(CELL_OUTLINE)
                            .bg(t.accent),
                    )
            })
            .when(whole, |d| d.bg(t.accent_soft).text_color(t.accent));
        grid_ops::header_element(cell, Header::Column(col), &self.view)
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let t = tokens(cx);
        let range = self.range();
        if col_ix == 0 {
            // Row-number gutter, highlighted for the rows of the selection.
            let row = row_ix as u32;
            let active = (range.top..=range.bottom).contains(&row);
            let whole = active && range.is_whole_rows();
            let gutter = div()
                .id(("row-head", row_ix))
                .size_full()
                .relative()
                .flex()
                .items_center()
                .justify_end()
                .px(SPACE_3)
                .bg(if active {
                    t.grid_header_active
                } else {
                    t.grid_header
                })
                .text_color(if active { t.fg } else { t.fg_muted })
                .when(active, |d| {
                    d.font_weight(FontWeight::MEDIUM).child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right_0()
                            .w(CELL_OUTLINE)
                            .bg(t.accent),
                    )
                })
                .when(whole, |d| d.bg(t.accent_soft).text_color(t.accent))
                .child(SharedString::from((row_ix + 1).to_string()));
            return grid_ops::header_element(gutter, Header::Row(row), &self.view);
        }
        let (row, col) = (row_ix as u32, col_ix as u32 - 1);
        let text = self.book.display_value(self.sheet, row, col);
        let style = self.book.cell_style(self.sheet, row, col);
        let align = self.book.display_align(self.sheet, row, col);
        let down_view = self.view.clone();
        let move_view = self.view.clone();
        // References of the formula being edited: coloured border and fill.
        let marks = self
            .ref_marks
            .iter()
            .filter(|(r, _)| r.contains(row, col))
            .map(|(r, slot)| {
                let color = point::ref_color(&t, *slot);
                div()
                    .absolute()
                    .inset_0()
                    .bg(color.opacity(0.12))
                    .border_color(color)
                    .when(row == r.top, |d| d.border_t(CELL_OUTLINE))
                    .when(row == r.bottom, |d| d.border_b(CELL_OUTLINE))
                    .when(col == r.left, |d| d.border_l(CELL_OUTLINE))
                    .when(col == r.right, |d| d.border_r(CELL_OUTLINE))
            });
        let fill_view = self.view.clone();
        let frozen_rows = self.book.frozen(self.sheet).0;
        div()
            .size_full()
            .relative()
            .flex()
            .items_center()
            .px(SPACE_3)
            // Vertical gridline; the row's bottom border is the horizontal one.
            .child(grid_line_right(t.grid_line))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(style.bold, |d| d.font_weight(FontWeight::BOLD))
                    .when(style.italic, |d| d.italic())
                    .when(style.underline, |d| d.underline())
                    .when(style.strikethrough, |d| d.line_through())
                    .when(align == HAlign::Center, |d| d.text_center())
                    .when(align == HAlign::Right, |d| d.text_right())
                    .child(SharedString::from(text)),
            )
            .when(!range.is_single() && range.contains(row, col), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .when((row, col) != self.anchor, |d| d.bg(t.accent_soft))
                        .border_color(t.accent)
                        .when(row == range.top, |d| d.border_t_1())
                        .when(row == range.bottom, |d| d.border_b_1())
                        .when(col == range.left, |d| d.border_l_1())
                        .when(col == range.right, |d| d.border_r_1()),
                )
            })
            .children(marks)
            // The active cell: a strong outline and no fill, even inside a range.
            .when((row, col) == self.anchor, |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border(CELL_OUTLINE)
                        .border_color(t.accent),
                )
            })
            // Where a fill-handle drag would fill: an outline, no fill.
            .when_some(
                self.fill_preview.filter(|p| p.contains(row, col)),
                |d, p| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .border_dashed()
                            .border_color(t.accent)
                            .when(row == p.top, |d| d.border_t_1())
                            .when(row == p.bottom, |d| d.border_b_1())
                            .when(col == p.left, |d| d.border_l_1())
                            .when(col == p.right, |d| d.border_r_1()),
                    )
                },
            )
            // Frozen rows: a stronger line under the last one. (The table
            // can't pin rows yet; frozen columns are pinned.)
            .when(frozen_rows > 0 && row + 1 == frozen_rows, |d| {
                d.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(GRID_LINE)
                        .bg(t.border_strong),
                )
            })
            // Fill handle at the selection's bottom-right corner; drag it
            // to fill. Cells clip, so it sits just inside the corner.
            .when((row, col) == (range.bottom, range.right), |d| {
                d.child(
                    div()
                        .absolute()
                        .right_0()
                        .bottom_0()
                        .size(FILL_HANDLE)
                        .bg(t.accent)
                        .border_1()
                        .border_color(t.bg)
                        .cursor_crosshair()
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            // Not a click on the cell under it.
                            cx.stop_propagation();
                            let _ = fill_view.update(cx, |view, cx| view.fill_handle_down(cx));
                        }),
                )
            })
            .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                let _ = down_view.update(cx, |view, cx| {
                    view.cell_mouse_down(row, col, e.modifiers.shift, window, cx)
                });
            })
            .on_mouse_move(move |e, window, cx| {
                if e.pressed_button == Some(MouseButton::Left) {
                    let _ = move_view.update(cx, |view, cx| view.cell_drag(row, col, window, cx));
                }
            })
            .when_some(
                self.editor
                    .as_ref()
                    .filter(|e| e.cell == (row, col) && e.sheet == self.sheet),
                |d, editor| d.child(editor.render(self.ref_span_colors(&t), cx)),
            )
            .into_any_element()
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        (col_ix > 0).then(|| {
            self.book
                .display_value(self.sheet, row_ix as u32, col_ix as u32 - 1)
                .into()
        })
    }
}

/// A gridline down the right edge of a cell (absolute, so it costs no width
/// and the active cell's outline paints over it).
fn grid_line_right(color: Hsla) -> Div {
    div()
        .absolute()
        .top_0()
        .bottom_0()
        .right_0()
        .w(GRID_LINE)
        .bg(color)
}

/// A Sheets document view.
pub struct SheetsView {
    table: Entity<TableState<SheetGrid>>,
    formula: Entity<InputState>,
    /// The in-cell editor's text (kept in sync with `formula`).
    cell_input: Entity<InputState>,
    /// Selected cell in sheet coordinates `(row, col)`.
    selected: (u32, u32),
    /// The cell being edited, if an edit is in progress. Change it with
    /// `set_editing`, which also shows or hides the in-cell editor.
    editing: Option<(u32, u32)>,
    /// Whether arrows move the caret or commit (Excel's Edit / Enter mode).
    edit_mode: EditMode,
    /// The sheet of the cell being edited (the grid may show another one
    /// while pointing).
    edit_sheet: u32,
    /// The reference last inserted by pointing, and the function list.
    point: Option<point::PointRef>,
    completion: Option<point::Completion>,
    /// A mouse drag that started on a cell is extending the range.
    dragging: bool,
    /// A mouse drag that started by pointing is growing the reference.
    point_dragging: bool,
    /// A drag that started on a header or the fill handle.
    grid_drag: Option<GridDrag>,
    /// The range to apply when the next `SelectCell` arrives (see
    /// `grid_ops::select_range`).
    pending_selection: Option<((u32, u32), Span)>,
    status: SharedString,
    /// Last measured pane width (for compact layout).
    width: Rc<Cell<Pixels>>,
    /// Name shown while the document has no file (`Untitled 2.xlsx`), given
    /// by the shell so new documents are numbered across the session.
    untitled_name: Option<String>,
    /// Find & Replace (Ctrl+F / Ctrl+H).
    find: find::SheetsFind,
    _subscriptions: Vec<Subscription>,
}

impl SheetsView {
    pub fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (book, status) = match path.as_deref().map(Workbook::open) {
            Some(Ok(book)) => {
                let s = opened_status(&book);
                (book, s)
            }
            Some(Err(err)) => {
                log::error!("{err:#}");
                (
                    Workbook::new_empty().expect("empty workbook"),
                    format!("{err:#}"),
                )
            }
            None => (
                Workbook::new_empty().expect("empty workbook"),
                "Ready".to_owned(),
            ),
        };

        let view = cx.entity().downgrade();
        let table = cx.new(|cx| {
            TableState::new(SheetGrid::new(book, view), window, cx)
                .cell_selectable(true)
                .row_header(false)
                .row_selectable(false)
                .col_selectable(false)
                .col_movable(false)
                .sortable(false)
        });
        let formula = cx.new(|cx| InputState::new(window, cx).placeholder("Value or =formula"));
        let cell_input = cx.new(|cx| InputState::new(window, cx));

        let subscriptions = vec![
            cx.subscribe_in(&table, window, Self::on_table_event),
            cx.subscribe_in(&formula, window, Self::on_formula_event),
            cx.subscribe_in(&cell_input, window, Self::on_cell_input_event),
        ];

        let mut this = Self {
            table,
            formula,
            cell_input,
            selected: (0, 0),
            editing: None,
            edit_mode: EditMode::Edit,
            edit_sheet: 0,
            point: None,
            completion: None,
            dragging: false,
            point_dragging: false,
            grid_drag: None,
            pending_selection: None,
            status: status.into(),
            width: Rc::new(Cell::new(px(0.))),
            untitled_name: None,
            find: find::SheetsFind::new(window, cx),
            _subscriptions: subscriptions,
        };
        this.select_cell(0, 0, cx);
        this.focus_grid(window, cx);
        this
    }

    /// Select a sheet cell in the table (scrolls to it and emits `SelectCell`).
    fn select_cell(&mut self, row: u32, col: u32, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            table.set_selected_cell(row as usize, col as usize + 1, cx)
        });
    }

    fn focus_grid(&self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = self.table.read(cx).focus_handle(cx);
        window.focus(&handle, cx);
    }

    fn on_table_event(
        &mut self,
        _: &Entity<TableState<SheetGrid>>,
        event: &TableEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TableEvent::SelectCell(row, col) if *col > 0 => {
                self.selected = (*row as u32, *col as u32 - 1);
                // A new active cell starts a new range (unless a header
                // click asked for one).
                let cell = self.selected;
                let (end, span) = self.pending_selection.take().unwrap_or((cell, Span::Cells));
                self.table.update(cx, |table, cx| {
                    let grid = table.delegate_mut();
                    if grid.anchor != cell || grid.end != end || grid.span != span {
                        grid.anchor = cell;
                        grid.end = end;
                        grid.span = span;
                        cx.notify();
                    }
                });
                if self.editing.is_none() {
                    self.load_selected_into_formula(window, cx);
                }
                cx.notify();
            }
            TableEvent::DoubleClickedCell(row, col) if *col > 0 => {
                self.selected = (*row as u32, *col as u32 - 1);
                self.start_edit(None, window, cx);
            }
            TableEvent::ColumnWidthsChanged(widths) => self.save_column_widths(widths, window, cx),
            _ => {}
        }
    }

    fn on_formula_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Focus => {
                if self.editing.is_none() {
                    self.edit_mode = EditMode::Edit;
                    self.set_editing(Some(self.selected), cx);
                    let text = self.formula.read(cx).value();
                    self.cell_input
                        .update(cx, |input, cx| input.set_value(text, window, cx));
                    self.refresh_refs(cx);
                }
            }
            InputEvent::Change => self.mirror_formula(window, cx),
            // Ctrl+Enter (Enter itself is `CommitDown`).
            InputEvent::PressEnter { .. } => self.commit_edit(Some((1, 0)), window, cx),
            // Clicking elsewhere (e.g. another cell) commits, like a spreadsheet.
            InputEvent::Blur => self.on_editor_blur(window, cx),
        }
    }

    /// Keys pressed while the grid has focus. Arrow keys, Tab, Home/End and paging
    /// are handled by the table's own bindings before this runs.
    fn on_grid_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let ks = &event.keystroke;
        let m = &ks.modifiers;
        if m.control || m.platform || m.alt {
            return; // leave shortcuts alone
        }
        match ks.key.as_str() {
            "enter" | "f2" => self.start_edit(None, window, cx),
            // Backspace clears a range at once; on one cell it clears and edits.
            "backspace" if !self.range(cx).is_single() => self.clear_selected(window, cx),
            "backspace" => self.start_edit(Some(String::new()), window, cx),
            "delete" => self.clear_selected(window, cx),
            _ => match ks.key_char.as_deref() {
                Some(ch) if !ch.is_empty() && !ch.chars().any(char::is_control) => {
                    self.start_edit(Some(ch.to_owned()), window, cx)
                }
                _ => return,
            },
        }
        cx.stop_propagation();
    }

    /// Begin editing the selected cell in the in-cell editor. `initial`
    /// replaces its content (typing into a cell, Enter mode); `None` keeps the
    /// content (Enter/F2/double-click, Edit mode).
    fn start_edit(&mut self, initial: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (row, col) = self.selected;
        self.edit_mode = if initial.is_some() {
            EditMode::Enter
        } else {
            EditMode::Edit
        };
        if let Some(text) = initial {
            self.formula
                .update(cx, |input, cx| input.set_value(text, window, cx));
        } else {
            self.load_selected_into_formula(window, cx);
        }
        let text = self.formula.read(cx).value();
        self.cell_input
            .update(cx, |input, cx| input.set_value(text, window, cx));
        // The editor only exists while its cell is rendered.
        self.table.update(cx, |table, cx| {
            table.scroll_to_row(row as usize, cx);
            table.scroll_to_col(col as usize + 1, cx);
        });
        self.set_editing(Some((row, col)), cx);
        self.cell_input
            .update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Escape while editing: restore the cell's content and return to the grid.
    fn cancel_edit(
        &mut self,
        _: &cell_editor::CancelEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_none() {
            cx.propagate();
            return;
        }
        if self.dismiss_completion(cx) {
            return;
        }
        let edit_sheet = self.edit_sheet;
        self.set_editing(None, cx);
        if self.grid_sheet(cx) != edit_sheet {
            self.show_sheet(edit_sheet, cx);
            self.select_cell(self.selected.0, self.selected.1, cx);
        }
        self.load_selected_into_formula(window, cx);
        self.focus_grid(window, cx);
        self.status = "Edit cancelled".into();
        cx.notify();
    }

    /// Store the edit. `step` moves the active cell afterwards (Enter, Tab,
    /// arrows) and returns focus to the grid; `None` means focus already went
    /// elsewhere (another cell was clicked).
    fn commit_edit(
        &mut self,
        step: Option<(i64, i64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((row, col)) = self.editing else {
            return;
        };
        let value = self.edit_value(window, cx);
        let sheet = self.edit_sheet;
        self.set_editing(None, cx);
        let changed = self.table.read(cx).delegate().book.input(sheet, row, col) != value;
        if changed {
            let result = self.set_cell(sheet, row, col, &value, cx);
            self.status = match result {
                Ok(()) => format!("{} updated", cell_name(row, col)).into(),
                Err(err) => format!("{err:#}").into(),
            };
        }
        if let Some(step) = step {
            // Pointing may have left another sheet on screen.
            if self.grid_sheet(cx) != sheet {
                self.show_sheet(sheet, cx);
            }
            let (rows, cols) = {
                let grid = self.table.read(cx).delegate();
                (grid.rows, grid.cols)
            };
            let (row, col) = cell_editor::step_cell((row, col), step, rows, cols);
            self.select_cell(row, col, cx);
            self.load_selected_into_formula(window, cx);
            self.focus_grid(window, cx);
        } else {
            // Focus went elsewhere (another cell was clicked): show its content.
            self.load_selected_into_formula(window, cx);
        }
        self.update_title(window, cx);
        cx.notify();
    }

    /// Delete: clear every cell in the selected range (one undo step).
    fn clear_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(range) = self.op_range(cx) else {
            return;
        };
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = grid.book.clear_range(sheet, range);
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        });
        if let Err(err) = result {
            self.status = format!("{err:#}").into();
        }
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    /// The selected range (a single cell when nothing is extended).
    fn range(&self, cx: &App) -> CellRange {
        self.table.read(cx).delegate().range()
    }

    /// Move the range's far corner (not the active cell) by one step.
    fn extend(&mut self, d_row: i64, d_col: i64, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            // Only the axis that moves is kept inside the grid (a whole
            // column's end row is the sheet's last).
            let step = |at: u32, d: i64, len: u32| {
                if d == 0 {
                    at
                } else {
                    (at as i64 + d).clamp(0, len as i64 - 1) as u32
                }
            };
            let row = step(grid.end.0, d_row, grid.rows);
            let col = step(grid.end.1, d_col, grid.cols);
            grid.end = (row, col);
            if grid.span == Span::All {
                grid.span = Span::Cells;
            }
            if d_row != 0 {
                table
                    .vertical_scroll_handle
                    .scroll_to_item(row as usize, ScrollStrategy::Nearest);
            }
            if d_col != 0 {
                table.scroll_to_col(col as usize + 1, cx);
            }
            cx.notify();
        });
        cx.notify();
    }

    fn extend_up(&mut self, _: &ExtendUp, _: &mut Window, cx: &mut Context<Self>) {
        self.extend(-1, 0, cx);
    }

    fn extend_down(&mut self, _: &ExtendDown, _: &mut Window, cx: &mut Context<Self>) {
        self.extend(1, 0, cx);
    }

    fn extend_left(&mut self, _: &ExtendLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.extend(0, -1, cx);
    }

    fn extend_right(&mut self, _: &ExtendRight, _: &mut Window, cx: &mut Context<Self>) {
        self.extend(0, 1, cx);
    }

    /// Left button down on a cell. A plain press selects it right away (not on
    /// release, so a drag can grow the range from it); Shift extends the range.
    fn cell_mouse_down(
        &mut self,
        row: u32,
        col: u32,
        shift: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // While a formula is being typed, a click can insert the cell's
        // reference instead (Shift+click grows the pointed one into a range).
        if self.editing.is_some() {
            let sheet = self.grid_sheet(cx);
            let anchor = self
                .live_point(window, cx)
                .filter(|p| shift && p.sheet == sheet)
                .map_or((row, col), |p| p.anchor);
            if self.point_to(anchor, (row, col), window, cx) {
                // Keep focus in the editor and the table from selecting.
                window.prevent_default();
                cx.stop_propagation();
                self.point_dragging = true;
                return;
            }
        }
        self.dragging = true;
        if !shift {
            self.select_cell(row, col, cx);
            return;
        }
        // Keep the table from turning this into a click, which would move the
        // active cell.
        cx.stop_propagation();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().end = (row, col);
            cx.notify();
        });
        if self.editing.is_none() {
            self.focus_grid(window, cx);
        }
        cx.notify();
    }

    /// The mouse moved over a cell with the left button held.
    fn cell_drag(&mut self, row: u32, col: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.point_dragging {
            if let Some(point) = self.live_point(window, cx)
                && point.end != (row, col)
            {
                self.point_to(point.anchor, (row, col), window, cx);
            }
            return;
        }
        if self.drag_over(row, col, cx) || !self.dragging {
            return;
        }
        let changed = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let changed = grid.end != (row, col);
            if changed {
                grid.end = (row, col);
                cx.notify();
            }
            changed
        });
        if changed {
            cx.notify();
        }
    }

    pub fn copy_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&Copy, window, cx);
    }

    pub fn cut_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cut(&Cut, window, cx);
    }

    pub fn paste_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.paste(&Paste, window, cx);
    }

    /// Put the selected range on the clipboard as tab-separated inputs.
    fn copy_range(&mut self, cx: &mut Context<Self>) -> CellRange {
        let range = self.op_range(cx).unwrap_or_else(|| self.range(cx));
        let text = {
            let grid = self.table.read(cx).delegate();
            grid.book.range_tsv(grid.sheet, range)
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        range
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let range = self.copy_range(cx);
        self.status = format!("Copied {}", range.name()).into();
        cx.notify();
    }

    /// Copy, then clear the range straight away (not when pasted, as Excel does).
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let range = self.copy_range(cx);
        self.clear_selected(window, cx);
        self.status = format!("Cut {}", range.name()).into();
        cx.notify();
    }

    /// Paste clipboard text (tab-separated) at the active cell and select what
    /// was pasted.
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.status = "Nothing to paste".into();
            cx.notify();
            return;
        };
        let (row, col) = self.selected;
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = grid.book.paste_tsv(sheet, row, col, &text);
            if let Ok(Some(range)) = &res {
                grid.end = (range.bottom, range.right);
                grid.span = Span::Cells;
            }
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        });
        self.status = match result {
            Ok(Some(range)) => format!("Pasted {}", range.name()).into(),
            Ok(None) => "Nothing to paste".into(),
            Err(err) => format!("{err:#}").into(),
        };
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    /// The active cell's formatting, for the toolbar.
    fn active_style(&self, cx: &App) -> CellStyle {
        let (row, col) = self.selected;
        let grid = self.table.read(cx).delegate();
        grid.book.cell_style(grid.sheet, row, col)
    }

    /// Apply a formatting command to the selected range (one undo step).
    pub fn format_doc(&mut self, format: Format, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let style = self.active_style(cx);
        let (change, what) = match format {
            Format::Bold => (StyleChange::Bold(!style.bold), "Bold"),
            Format::Italic => (StyleChange::Italic(!style.italic), "Italic"),
            Format::Underline => (StyleChange::Underline(!style.underline), "Underline"),
            Format::Strikethrough => (
                StyleChange::Strikethrough(!style.strikethrough),
                "Strikethrough",
            ),
            Format::Align(a) => {
                let a = if style.align == a { HAlign::General } else { a };
                (StyleChange::Align(a), "Alignment")
            }
            Format::Number(f) => (StyleChange::NumberFormat(f), f.label()),
        };
        let Some(range) = self.op_range(cx) else {
            return;
        };
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = grid.book.set_range_style(sheet, range, change);
            cx.notify();
            res
        });
        self.status = match result {
            Ok(()) => format!("{what}: {}", range.name()).into(),
            Err(err) => format!("{err:#}").into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }

    fn toggle_bold(&mut self, _: &ToggleBold, window: &mut Window, cx: &mut Context<Self>) {
        self.format_doc(Format::Bold, window, cx);
    }

    fn toggle_italic(&mut self, _: &ToggleItalic, window: &mut Window, cx: &mut Context<Self>) {
        self.format_doc(Format::Italic, window, cx);
    }

    fn toggle_underline(
        &mut self,
        _: &ToggleUnderline,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.format_doc(Format::Underline, window, cx);
    }

    /// The user resized a column (or double-clicked to fit it): store the new
    /// widths in the workbook, as one undo step.
    fn save_column_widths(
        &mut self,
        widths: &[Pixels],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.table.update(cx, |table, _| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            // Table column 0 is the row-number gutter.
            let widths: Vec<(u32, f64)> = widths
                .iter()
                .enumerate()
                .skip(1)
                .map(|(ix, w)| (ix as u32 - 1, f64::from(f32::from(*w))))
                .collect();
            grid.book.set_column_widths(sheet, &widths)
        });
        if let Err(err) = result {
            self.status = format!("{err:#}").into();
        }
        self.update_title(window, cx);
        cx.notify();
    }

    pub fn undo_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.undo(&Undo, window, cx);
    }

    pub fn redo_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.redo(&Redo, window, cx);
    }

    fn undo(&mut self, _: &Undo, window: &mut Window, cx: &mut Context<Self>) {
        self.step_history(false, window, cx);
    }

    fn redo(&mut self, _: &Redo, window: &mut Window, cx: &mut Context<Self>) {
        self.step_history(true, window, cx);
    }

    /// Undo (or redo) one workbook edit and refresh everything showing it.
    fn step_history(&mut self, redo: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return;
        }
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let res = if redo {
                grid.book.redo()
            } else {
                grid.book.undo()
            };
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        });
        self.status = match (result, redo) {
            (Ok(true), false) => "Undone".into(),
            (Ok(true), true) => "Redone".into(),
            (Ok(false), false) => "Nothing to undo".into(),
            (Ok(false), true) => "Nothing to redo".into(),
            (Err(err), _) => format!("{err:#}").into(),
        };
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    fn set_cell(
        &mut self,
        sheet: u32,
        row: u32,
        col: u32,
        value: &str,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let res = grid.book.set_input(sheet, row, col, value);
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        })
    }

    fn load_selected_into_formula(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (row, col) = self.selected;
        let text = {
            let grid = self.table.read(cx).delegate();
            grid.book.input(grid.sheet, row, col)
        };
        self.formula
            .update(cx, |input, cx| input.set_value(text, window, cx));
        cx.notify();
    }

    /// A sheet tab was clicked. While a formula is pointing, the edit goes on
    /// (see [`point`]); any other edit is committed first.
    fn select_sheet(&mut self, sheet: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            if self.can_point(window, cx) {
                self.point_to_sheet(sheet, window, cx);
                return;
            }
            self.commit_edit(None, window, cx);
        }
        self.show_sheet(sheet, cx);
        self.select_cell(0, 0, cx);
        self.load_selected_into_formula(window, cx);
        self.focus_grid(window, cx);
    }

    /// Show `sheet` in the grid, leaving the selection and any edit alone.
    fn show_sheet(&mut self, sheet: u32, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            grid.sheet = sheet;
            let _ = grid.book.set_active_sheet(sheet);
            grid.resize();
            table.refresh(cx);
            cx.notify();
        });
        self.refresh_refs(cx);
    }

    pub fn cell_info(&self, cx: &App) -> CellInfo {
        let (row, col) = self.selected;
        let grid = self.table.read(cx).delegate();
        CellInfo {
            address: cell_name(row, col),
            input: grid.book.input(grid.sheet, row, col),
            value: grid.book.display_value(grid.sheet, row, col),
        }
    }

    pub fn book_info(&self, cx: &App) -> BookInfo {
        let grid = self.table.read(cx).delegate();
        BookInfo {
            name: self.name(&grid.book),
            path: grid.book.path().map(|p| p.to_path_buf()),
            dirty: grid.book.is_dirty(),
            sheets: grid.book.sheet_names(),
            active_sheet: grid.sheet as usize,
            used: grid.book.used_size(grid.sheet),
        }
    }

    /// Status line for the app's status bar.
    pub fn status(&self) -> SharedString {
        self.status.clone()
    }

    /// The selected cell's name, e.g. `B7`.
    pub fn cell_ref(&self) -> String {
        cell_name(self.selected.0, self.selected.1)
    }

    /// The name box text: the cell (`B7`) or the selected range (`A1:C4`).
    fn selection_ref(&self, cx: &App) -> String {
        self.range(cx).name()
    }

    /// File name for tabs, e.g. `budget.xlsx` or `Untitled 2.xlsx`.
    pub fn doc_name(&self, cx: &App) -> String {
        self.name(&self.table.read(cx).delegate().book)
    }

    /// Set the name shown while the workbook has no file.
    pub fn set_untitled_name(&mut self, name: String, cx: &mut Context<Self>) {
        self.untitled_name = Some(name);
        cx.notify();
    }

    /// `book`'s file name, or the shell-given name while it has no file.
    fn name(&self, book: &Workbook) -> String {
        match &self.untitled_name {
            Some(name) if book.source_path().is_none() => name.clone(),
            _ => book.title(),
        }
    }

    /// The file this document shows (an imported `.ods`/`.xls` too, though
    /// Save never writes there), if any.
    pub fn path(&self, cx: &App) -> Option<PathBuf> {
        self.table
            .read(cx)
            .delegate()
            .book
            .source_path()
            .map(|p| p.to_path_buf())
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        self.table.read(cx).delegate().book.is_dirty()
    }

    /// Give keyboard focus to the grid.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_grid(window, cx);
    }

    pub fn save_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save(&Save, window, cx);
    }

    pub fn save_doc_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as(&SaveAs, window, cx);
    }

    pub fn title(&self, cx: &App) -> String {
        let book = &self.table.read(cx).delegate().book;
        let dirty = if book.is_dirty() { "• " } else { "" };
        format!("{dirty}{} — Kantor Sheets", self.name(book))
    }

    fn update_title(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        window.set_window_title(&title);
    }

    /// Replace the open workbook with the file at `path`.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match Workbook::open(&path) {
            Ok(book) => {
                self.status = opened_status(&book).into();
                self.set_editing(None, cx);
                let view = cx.entity().downgrade();
                self.table.update(cx, |table, cx| {
                    *table.delegate_mut() = SheetGrid::new(book, view);
                    table.refresh(cx);
                    cx.notify();
                });
                self.select_cell(0, 0, cx);
                self.load_selected_into_formula(window, cx);
                self.update_title(window, cx);
                self.focus_grid(window, cx);
            }
            Err(err) => self.status = format!("{err:#}").into(),
        }
        cx.notify();
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        if self.table.read(cx).delegate().book.path().is_none() {
            self.save_as(&SaveAs, window, cx);
            return;
        }
        let result = self
            .table
            .update(cx, |table, _| table.delegate_mut().book.save());
        self.after_save(result, window, cx);
    }

    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as_task(window, cx).detach();
    }

    /// Save before the document closes. Resolves to `true` once the workbook
    /// is saved; `false` if the Save As prompt was cancelled or saving failed.
    pub fn save_for_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if self.table.read(cx).delegate().book.path().is_none() {
            return self.save_as_task(window, cx);
        }
        self.save(&Save, window, cx);
        Task::ready(!self.is_dirty(cx))
    }

    /// Ask for a path and save there; resolves to whether the workbook is saved.
    fn save_as_task(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        let (dir, name) = {
            let book = &self.table.read(cx).delegate().book;
            let dir = book
                .source_path()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
                .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("."));
            let name = if book.source_path().is_none() {
                self.name(book)
            } else {
                book.suggested_file_name()
            };
            (dir, name)
        };
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = path.await else {
                return false;
            };
            if path.extension().is_none() {
                path.set_extension("xlsx");
            }
            this.update_in(cx, |this, window, cx| {
                let result = this
                    .table
                    .update(cx, |table, _| table.delegate_mut().book.save_as(&path));
                this.after_save(result, window, cx);
                !this.is_dirty(cx)
            })
            .unwrap_or(false)
        })
    }

    fn after_save(
        &mut self,
        result: anyhow::Result<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.status = match result {
            Ok(()) => {
                let book = &self.table.read(cx).delegate().book;
                let lost = book.sheets_lost_in_csv();
                if matches!(book.source_format(), SourceFormat::Csv { .. }) && lost > 0 {
                    let sheet = book.active_sheet() as usize;
                    let name = book.sheet_names().get(sheet).cloned().unwrap_or_default();
                    format!(
                        "Saved {} with values of sheet \"{name}\" only; CSV can't hold the other {lost}",
                        book.title()
                    )
                    .into()
                } else {
                    format!("Saved {}", book.title()).into()
                }
            }
            Err(err) => format!("Save failed: {err:#}").into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }

    fn render_sheet_tabs(&self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let grid = self.table.read(cx).delegate();
        let active = grid.sheet;
        let names = grid.book.sheet_names();
        h_flex()
            .gap(SPACE_1)
            .children(names.into_iter().enumerate().map(|(ix, name)| {
                let ix = ix as u32;
                let view = cx.entity().downgrade();
                let selected = ix == active;
                div()
                    .id(("sheet-tab", ix as usize))
                    .h(CONTROL_SM)
                    .px(SPACE_5)
                    .flex()
                    .items_center()
                    .rounded(RADIUS_SM)
                    .text_size(FONT_SMALL)
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when_else(
                        selected,
                        |d| {
                            d.bg(t.bg)
                                .border_1()
                                .border_color(t.border_strong)
                                .text_color(t.fg)
                        },
                        |d| d.text_color(t.fg_muted).hover(|s| s.bg(t.hover)),
                    )
                    .child(name)
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| this.select_sheet(ix, window, cx));
                    })
            }))
    }
}

impl Render for SheetsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        let completion = self.render_completion(window, cx);
        // The bar colours references only while it holds the edit.
        let bar_refs = if self.editing.is_some() {
            self.table.read(cx).delegate().ref_span_colors(&t)
        } else {
            Vec::new()
        };
        let tabs = self.render_sheet_tabs(cx);
        let editing = self.editing.is_some();
        let width = self.width.get();
        let compact = width > px(0.) && width < COMPACT_WIDTH;
        // Measure our width after layout; re-render when crossing the threshold.
        let measure = {
            let cell = self.width.clone();
            let view = cx.entity().downgrade();
            canvas(
                move |bounds, _, cx| {
                    let old = cell.replace(bounds.size.width);
                    if (old < COMPACT_WIDTH) != (bounds.size.width < COMPACT_WIDTH) {
                        let _ = view.update(cx, |_, cx| cx.notify());
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        let root = self.editor_actions(v_flex(), cx);
        let root = self.point_actions(root, cx);
        use kantor_ui::find_bar as fb;
        root.key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(|this, _: &fb::OpenFind, w, cx| this.open_find(false, w, cx)))
            .on_action(cx.listener(|this, _: &fb::OpenReplace, w, cx| this.open_find(true, w, cx)))
            .on_action(cx.listener(|this, _: &fb::FindNext, w, cx| this.find_next_doc(true, w, cx)))
            .on_action(
                cx.listener(|this, _: &fb::FindPrev, w, cx| this.find_next_doc(false, w, cx)),
            )
            .relative()
            .size_full()
            .bg(t.bg)
            .text_color(t.fg)
            .text_size(FONT_BODY)
            .child(measure)
            .child(toolbar::render(&self.active_style(cx), compact, cx))
            // Formula bar: name box + input
            .child(
                h_flex()
                    .flex_none()
                    .h(CONTROL_LG + SPACE_3)
                    .gap(SPACE_4)
                    .px(SPACE_4)
                    .bg(t.chrome)
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex_none()
                            .w(if compact { px(52.) } else { px(72.) })
                            .h(CONTROL_MD)
                            .px(SPACE_4)
                            .flex()
                            .items_center()
                            .rounded(RADIUS_SM)
                            .bg(t.surface)
                            .border_1()
                            .border_color(if editing { t.accent } else { t.border })
                            .text_size(FONT_SMALL)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if editing { t.accent } else { t.fg_muted })
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .child(SharedString::from(self.selection_ref(cx))),
                            ),
                    )
                    .when(!compact, |d| {
                        d.child(
                            div()
                                .text_size(FONT_EMPHASIS)
                                .italic()
                                .text_color(t.fg_dim)
                                .child("fx"),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .relative()
                            .key_context(cell_editor::FORMULA_BAR_CONTEXT)
                            .child(Input::new(&self.formula).small())
                            .child(point::colored_refs(self.formula.clone(), bar_refs)),
                    ),
            )
            // Grid
            .child(
                self.grid_actions(div(), cx)
                    .flex_1()
                    .min_h_0()
                    .key_context(GRID_CONTEXT)
                    .on_action(cx.listener(Self::undo))
                    .on_action(cx.listener(Self::redo))
                    .on_action(cx.listener(Self::copy))
                    .on_action(cx.listener(Self::cut))
                    .on_action(cx.listener(Self::paste))
                    .on_action(cx.listener(Self::extend_up))
                    .on_action(cx.listener(Self::extend_down))
                    .on_action(cx.listener(Self::extend_left))
                    .on_action(cx.listener(Self::extend_right))
                    .on_action(cx.listener(Self::toggle_bold))
                    .on_action(cx.listener(Self::toggle_italic))
                    .on_action(cx.listener(Self::toggle_underline))
                    // A drag only extends the range if it started on a cell
                    // (cells set `dragging` after this capture listener runs).
                    .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                        this.dragging = false;
                        this.point_dragging = false;
                        this.reset_grid_drag(cx);
                    }))
                    .capture_any_mouse_up(cx.listener(|this, _, window, cx| {
                        this.dragging = false;
                        this.point_dragging = false;
                        this.end_grid_drag(window, cx);
                    }))
                    .on_key_down(cx.listener(Self::on_grid_key))
                    .child(DataTable::new(&self.table).bordered(false).small()),
            )
            // Sheet tabs
            .child(
                h_flex()
                    .flex_none()
                    .h(CONTROL_LG)
                    .px(SPACE_4)
                    .bg(t.chrome)
                    .border_t_1()
                    .border_color(t.border)
                    .child(tabs),
            )
            .children(completion)
            .children(self.render_find_bar(cx))
    }
}
