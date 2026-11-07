//! Kantor Sheets UI.
//!
//! The grid is GPUI Kit's virtualised `DataTable`; column 0 is a fixed row-number
//! gutter, so table column `n` maps to sheet column `n - 1`.
//!
//! Editing is spreadsheet-style: with a cell selected, typing starts an edit,
//! Enter/F2 edits the existing content, Delete clears, Escape cancels and Enter
//! commits and moves down. The edit happens in the formula bar for now; an
//! in-cell editor overlay comes later.

extern crate gpui_kit as gpui;

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, h_flex,
    input::{self, Input, InputEvent, InputState},
    table::{Column, DataTable, TableDelegate, TableEvent, TableState},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{Workbook, cell_name, column_name};

actions!(sheets, [Save, SaveAs]);

const KEY_CONTEXT: &str = "KantorSheets";
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

/// Register Sheets key bindings. Both Ctrl and Super work (Mac-style Cmd = Super).
pub fn init(cx: &mut App) {
    let ctx = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("ctrl-s", Save, ctx),
        KeyBinding::new("super-s", Save, ctx),
        KeyBinding::new("ctrl-shift-s", SaveAs, ctx),
        KeyBinding::new("super-shift-s", SaveAs, ctx),
    ]);
}

/// Feeds one sheet of a [`Workbook`] to the table.
pub struct SheetGrid {
    book: Workbook,
    sheet: u32,
    rows: u32,
    cols: u32,
}

impl SheetGrid {
    fn new(book: Workbook) -> Self {
        let mut grid = Self {
            book,
            sheet: 0,
            rows: 0,
            cols: 0,
        };
        grid.resize();
        grid
    }

    /// Recompute grid extents: the used range plus headroom, never below the minimum.
    fn resize(&mut self) {
        let (r, c) = self.book.used_size(self.sheet);
        self.rows = (r + 200).max(MIN_ROWS);
        self.cols = (c + 10).max(MIN_COLS);
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
                .width(px(52.))
                .fixed_left()
                .resizable(false)
                .selectable(false)
                .text_right()
        } else {
            let name = column_name(col_ix as u32 - 1);
            Column::new(SharedString::from(name.clone()), name)
                .width(px(100.))
                .min_width(px(24.))
                .resizable(true)
        }
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        if col_ix == 0 {
            return div()
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from((row_ix + 1).to_string()));
        }
        let text = self
            .book
            .display_value(self.sheet, row_ix as u32, col_ix as u32 - 1);
        div().truncate().child(SharedString::from(text))
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        (col_ix > 0).then(|| {
            self.book
                .display_value(self.sheet, row_ix as u32, col_ix as u32 - 1)
                .into()
        })
    }
}

/// A Sheets document view.
pub struct SheetsView {
    table: Entity<TableState<SheetGrid>>,
    formula: Entity<InputState>,
    /// Selected cell in sheet coordinates `(row, col)`.
    selected: (u32, u32),
    /// The cell being edited in the formula bar, if an edit is in progress.
    editing: Option<(u32, u32)>,
    status: SharedString,
    /// Last measured pane width (for compact layout).
    width: Rc<Cell<Pixels>>,
    _subscriptions: Vec<Subscription>,
}

impl SheetsView {
    pub fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (book, status) = match path.as_deref().map(Workbook::open) {
            Some(Ok(book)) => {
                let s = format!("Opened {}", book.title());
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

        let table = cx.new(|cx| {
            TableState::new(SheetGrid::new(book), window, cx)
                .cell_selectable(true)
                .row_header(false)
                .row_selectable(false)
                .col_selectable(false)
                .col_movable(false)
                .sortable(false)
        });
        let formula = cx.new(|cx| InputState::new(window, cx).placeholder("Value or =formula"));

        let subscriptions = vec![
            cx.subscribe_in(&table, window, Self::on_table_event),
            cx.subscribe_in(&formula, window, Self::on_formula_event),
        ];

        let mut this = Self {
            table,
            formula,
            selected: (0, 0),
            editing: None,
            status: status.into(),
            width: Rc::new(Cell::new(px(0.))),
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
                if self.editing.is_none() {
                    self.load_selected_into_formula(window, cx);
                }
                cx.notify();
            }
            TableEvent::DoubleClickedCell(row, col) if *col > 0 => {
                self.selected = (*row as u32, *col as u32 - 1);
                self.start_edit(None, window, cx);
            }
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
                    self.editing = Some(self.selected);
                }
            }
            InputEvent::PressEnter { .. } => self.commit_edit(true, window, cx),
            // Clicking elsewhere (e.g. another cell) commits, like a spreadsheet.
            InputEvent::Blur => self.commit_edit(false, window, cx),
            _ => {}
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

    /// Begin editing the selected cell. `initial` replaces its content (typing
    /// into a cell); `None` keeps the content (Enter/F2/double-click).
    fn start_edit(&mut self, initial: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = Some(self.selected);
        if let Some(text) = initial {
            self.formula
                .update(cx, |input, cx| input.set_value(text, window, cx));
        } else {
            self.load_selected_into_formula(window, cx);
        }
        self.formula.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    /// Escape while editing: restore the cell's content and return to the grid.
    fn cancel_edit(&mut self, _: &input::Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.take().is_none() {
            cx.propagate();
            return;
        }
        self.load_selected_into_formula(window, cx);
        self.focus_grid(window, cx);
        self.status = "Edit cancelled".into();
        cx.notify();
    }

    fn commit_edit(&mut self, move_down: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((row, col)) = self.editing.take() else {
            return;
        };
        let value = self.formula.read(cx).value().to_string();
        let changed = {
            let grid = self.table.read(cx).delegate();
            grid.book.input(grid.sheet, row, col) != value
        };
        if changed {
            let result = self.set_cell(row, col, &value, cx);
            self.status = match result {
                Ok(()) => format!("{} updated", cell_name(row, col)).into(),
                Err(err) => format!("{err:#}").into(),
            };
        }
        if move_down {
            self.select_cell(row + 1, col, cx);
            self.focus_grid(window, cx);
        } else {
            // Focus went elsewhere (another cell was clicked): show its content.
            self.load_selected_into_formula(window, cx);
        }
        self.update_title(window, cx);
        cx.notify();
    }

    fn clear_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (row, col) = self.selected;
        if let Err(err) = self.set_cell(row, col, "", cx) {
            self.status = format!("{err:#}").into();
        }
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    fn set_cell(
        &mut self,
        row: u32,
        col: u32,
        value: &str,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
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

    fn select_sheet(&mut self, sheet: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.editing = None;
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            grid.sheet = sheet;
            grid.resize();
            table.refresh(cx);
            cx.notify();
        });
        self.select_cell(0, 0, cx);
        self.load_selected_into_formula(window, cx);
        self.focus_grid(window, cx);
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
            name: grid.book.title(),
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

    /// File name for tabs, e.g. `budget.xlsx`.
    pub fn doc_name(&self, cx: &App) -> String {
        self.table.read(cx).delegate().book.title()
    }

    /// Path of the open file, if it has been saved.
    pub fn path(&self, cx: &App) -> Option<PathBuf> {
        self.table
            .read(cx)
            .delegate()
            .book
            .path()
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
        format!("{dirty}{} — Kantor Sheets", book.title())
    }

    fn update_title(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        window.set_window_title(&title);
    }

    /// Replace the open workbook with the file at `path`.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match Workbook::open(&path) {
            Ok(book) => {
                self.status = format!("Opened {}", book.title()).into();
                self.editing = None;
                self.table.update(cx, |table, cx| {
                    *table.delegate_mut() = SheetGrid::new(book);
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
                .path()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
                .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("."));
            let name = match book.path() {
                Some(_) => book.title(),
                None => "Untitled.xlsx".to_owned(),
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
            Ok(()) => format!("Saved {}", self.table.read(cx).delegate().book.title()).into(),
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
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

        v_flex()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .relative()
            .size_full()
            .bg(t.bg)
            .text_color(t.fg)
            .text_size(FONT_BODY)
            .child(measure)
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
                            .child(SharedString::from(self.cell_ref())),
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
                            .on_action(cx.listener(Self::cancel_edit))
                            .child(Input::new(&self.formula).small()),
                    ),
            )
            // Grid
            .child(
                div()
                    .flex_1()
                    .min_h_0()
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
    }
}
