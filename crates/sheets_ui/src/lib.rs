//! Kantor Sheets UI.
//!
//! The grid is GPUI Kit's virtualised `DataTable`; column 0 is a fixed row-number
//! gutter, so table column `n` maps to sheet column `n - 1`.

extern crate gpui_kit as gpui;

use std::path::PathBuf;

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    table::{Column, DataTable, TableDelegate, TableEvent, TableState},
    v_flex,
};
use gpui_kit::*;
use sheets_engine::{Workbook, cell_name, column_name};

actions!(sheets, [Open, Save, SaveAs]);

const KEY_CONTEXT: &str = "KantorSheets";
const MIN_ROWS: u32 = 1000;
const MIN_COLS: u32 = 26;

/// Register Sheets key bindings. Both Ctrl and Super work (Mac-style Cmd = Super).
pub fn init(cx: &mut App) {
    let ctx = Some(KEY_CONTEXT);
    cx.bind_keys([
        KeyBinding::new("ctrl-o", Open, ctx),
        KeyBinding::new("super-o", Open, ctx),
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

/// A Sheets document window's root view.
pub struct SheetsView {
    table: Entity<TableState<SheetGrid>>,
    formula: Entity<InputState>,
    /// Selected cell in sheet coordinates `(row, col)`.
    selected: (u32, u32),
    status: SharedString,
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
            status: status.into(),
            _subscriptions: subscriptions,
        };
        this.load_selected_into_formula(window, cx);
        this
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
                self.load_selected_into_formula(window, cx);
            }
            TableEvent::DoubleClickedCell(row, col) if *col > 0 => {
                self.selected = (*row as u32, *col as u32 - 1);
                self.load_selected_into_formula(window, cx);
                self.formula.update(cx, |input, cx| input.focus(window, cx));
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
        if let InputEvent::PressEnter { .. } = event {
            self.commit_formula(window, cx);
        }
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

    fn commit_formula(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.formula.read(cx).value().to_string();
        let (row, col) = self.selected;
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = grid.book.set_input(sheet, row, col, &value);
            grid.resize();
            table.refresh(cx);
            cx.notify();
            res
        });
        match result {
            Ok(()) => {
                // Move down like a spreadsheet does after Enter.
                self.selected = (row + 1, col);
                self.load_selected_into_formula(window, cx);
                self.status = format!("{} updated", cell_name(row, col)).into();
            }
            Err(err) => self.status = format!("{err:#}").into(),
        }
        self.update_title(window, cx);
        cx.notify();
    }

    fn select_sheet(&mut self, sheet: u32, window: &mut Window, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            grid.sheet = sheet;
            grid.resize();
            table.refresh(cx);
            cx.notify();
        });
        self.selected = (0, 0);
        self.load_selected_into_formula(window, cx);
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

    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open spreadsheet".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                match Workbook::open(&path) {
                    Ok(book) => {
                        this.status = format!("Opened {}", book.title()).into();
                        this.table.update(cx, |table, cx| {
                            *table.delegate_mut() = SheetGrid::new(book);
                            table.refresh(cx);
                            cx.notify();
                        });
                        this.selected = (0, 0);
                        this.load_selected_into_formula(window, cx);
                        this.update_title(window, cx);
                    }
                    Err(err) => this.status = format!("{err:#}").into(),
                }
                cx.notify();
            });
        })
        .detach();
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
                return;
            };
            if path.extension().is_none() {
                path.set_extension("xlsx");
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let result = this
                    .table
                    .update(cx, |table, _| table.delegate_mut().book.save_as(&path));
                this.after_save(result, window, cx);
            });
        })
        .detach();
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
        let grid = self.table.read(cx).delegate();
        let active = grid.sheet;
        let names = grid.book.sheet_names();
        h_flex()
            .gap_1()
            .children(names.into_iter().enumerate().map(|(ix, name)| {
                let ix = ix as u32;
                let view = cx.entity().downgrade();
                let button = Button::new(("sheet-tab", ix as usize)).label(name).xsmall();
                let button = if ix == active {
                    button.primary()
                } else {
                    button.ghost()
                };
                button.on_click(move |_, window, cx| {
                    let _ = view.update(cx, |this, cx| this.select_sheet(ix, window, cx));
                })
            }))
    }
}

impl Render for SheetsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (row, col) = self.selected;
        let theme = cx.theme();
        let (bg, fg, border, muted_fg, bar_bg) = (
            theme.background,
            theme.foreground,
            theme.border,
            theme.muted_foreground,
            theme.title_bar,
        );
        let tabs = self.render_sheet_tabs(cx);

        v_flex()
            .key_context(KEY_CONTEXT)
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .size_full()
            .bg(bg)
            .text_color(fg)
            .text_sm()
            // Formula bar
            .child(
                h_flex()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .w(px(72.))
                            .text_color(muted_fg)
                            .child(SharedString::from(cell_name(row, col))),
                    )
                    .child(div().flex_1().child(Input::new(&self.formula).small())),
            )
            // Grid
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(DataTable::new(&self.table).bordered(false)),
            )
            // Sheet tabs + status
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(border)
                    .bg(bar_bg)
                    .child(tabs)
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(self.status.clone()),
                    ),
            )
    }
}
