//! Kantor Sheets UI.
//!
//! The grid is a fork of GPUI Kit's virtualised `DataTable` ([`table`]) whose
//! rows have their own heights ([`row_layout`]); column 0 is a fixed
//! row-number gutter, so table column `n` maps to sheet column `n - 1`.
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
//! Formatting is drawn as in the file: fills, text colour, size and font,
//! vertical alignment, wrapping, borders (shared edges resolved like Excel),
//! merged cells and per-sheet gridlines; see [`look`]. Rows are drawn at the
//! file's heights and grow to fit wrapped text and larger fonts; dragging a
//! row number's bottom edge resizes it (see [`row_layout`]).
//! Formatting commands (font flags, alignment, number formats, fill and text
//! colour, font size, borders, wrap, merge) apply to the whole range as one
//! undo step; see [`toolbar`]. Column widths come from the workbook and a
//! resize (or a double-click autofit on the header border) is saved back.
//!
//! Data validation (drop-down lists, input messages, error alerts for typed
//! values, circling invalid data) is in [`data_validation`].
//!
//! Header selection, Ctrl+arrows, the fill handle, inserting/deleting rows and
//! columns, frozen panes and the status-bar sums are in [`grid_ops`].

extern crate gpui_kit as gpui;

mod agent;
mod ai;
mod bench;
mod calc;
mod cell_editor;
mod cf_ui;
mod chart_ui;
mod cond_paint;
mod data_validation;
mod dialog;
mod drawings;
mod dv_ui;
mod filter_menu;
mod find;
mod grid_ops;
mod look;
pub mod page_setup;
pub mod pivot;
mod point;
mod popup_list;
mod print;
mod print_preview;
mod recovery;
mod row_heights;
mod row_layout;
mod saving;
mod spell_ui;
// Upstream code, kept close to GPUI Kit's so it can be compared.
#[allow(dead_code, clippy::all)]
mod table;
mod table_ui;

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::component::{
    Sizable as _, h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, tf, tk};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{CellRange, CellStyle, StyleChange, Tab, Workbook, cell_name, column_name};
use table::{Column, DataTable, TableDelegate, TableEvent, TableState};

mod notes_ui;
mod toolbar;

pub use ai::AiTarget;
pub use calc::SheetsEvent;
use cell_editor::{CellEditor, EditMode};
pub use grid_ops::GridCommand;
use grid_ops::{GridDrag, Header, Span};
pub use sheets_engine::CalcMode;
pub use sheets_engine::{BorderPreset, HAlign, NumberFormat, Rgb, VAlign};

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
    VAlign(VAlign),
    Number(NumberFormat),
    /// Fill colour (`None`: no fill).
    Fill(Option<Rgb>),
    /// Text colour (`None`: automatic).
    FontColor(Option<Rgb>),
    /// Font size, in points, up (positive) or down.
    FontSize(i32),
    /// Toggles wrapping, like the font flags.
    Wrap,
    Borders(BorderPreset),
    /// Merge & center the range, or unmerge it if it holds merged cells.
    Merge,
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
    let mut s = if book.source_format().is_imported() {
        let msg = kantor_ui::i18n::t("Opened {} (imported; Save creates an .xlsx copy)");
        kantor_ui::i18n::fill(msg, &[&book.title()])
    } else {
        tf("Opened {}", &[&book.title()])
    };
    for note in book.import_note_list() {
        s.push_str("; ");
        use sheets_engine::ImportNote as N;
        let plural = |n: usize, one: &'static str, many: &'static str| {
            if n == 1 {
                kantor_ui::i18n::t(one).to_owned()
            } else {
                tf(many, &[&n])
            }
        };
        s.push_str(&match *note {
            N::FormulasKeptAsValues(n) => plural(
                n,
                "1 formula kept as values",
                "{} formulas kept as values",
            ),
            N::ValidationsSkipped(n) => plural(
                n,
                "1 data validation rule not imported (formula not supported)",
                "{} data validation rules not imported (formula not supported)",
            ),
            N::ConditionalFormatsSkipped(n) => plural(
                n,
                "1 conditional formatting rule not imported (data bars, colour scales, icon sets or formulas not supported)",
                "{} conditional formatting rules not imported (data bars, colour scales, icon sets or formulas not supported)",
            ),
            N::FormattingNotImported => kantor_ui::i18n::t("formatting not imported").to_owned(),
            N::FormattingNotImportedNotXls => {
                kantor_ui::i18n::t("formatting not imported (not an Excel 97-2003 file)").to_owned()
            }
        });
    }
    s
}

/// Register Sheets key bindings. Both Ctrl and Super work (Mac-style Cmd = Super).
pub fn init(cx: &mut App) {
    // Pivot captions and the like in new content follow the interface language.
    sheets_engine::set_content_translator(kantor_ui::i18n::t);
    // Chart fallbacks ("Chart Title") come from the renderer shared with Slides.
    slides_engine::display_text::set_display_translator(kantor_ui::i18n::t);
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
    calc::bind_keys(cx);
    pivot::bind_keys(cx);
    table_ui::bind_keys(cx);
    data_validation::bind_keys(cx);
    spell_ui::bind_keys(cx);
    kantor_ui::spelling::init(cx);
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
    /// Cell font families, resolved once per name.
    fonts: look::FontCache,
    /// [`Self::selection`] for its inputs, as it is asked for every cell.
    selection_memo: Cell<Option<(SelectionKey, Selection)>>,
    /// The window's scale factor: column widths and merged-cell offsets are
    /// snapped to its device pixels ([`look::snap`]).
    scale: f32,
    /// Where each row sits ([`row_layout`]).
    layout: row_layout::RowLayout,
    /// Drop-down list and circles ([`data_validation`]).
    validation: data_validation::GridValidation,
    /// The sheet's filters, for the state they were read in (the header
    /// cells ask for every frame).
    filters_memo: RefCell<Option<FiltersMemo>>,
    /// The cell with a note under the pointer, `(sheet, row, col)`: its
    /// note is shown ([`notes_ui`]).
    note_hover: Option<(u32, u32, u32)>,
}

/// A sheet's filters for `((revision), sheet)`.
type FiltersMemo = (((u64, u64), u32), Rc<Vec<sheets_engine::FilterInfo>>);

/// What [`SheetGrid::selection`] depends on.
type SelectionKey = ((u32, u32), (u32, u32), Span, u32, (u64, u64));

/// The selected range grown to whole merged cells, and the active cell's
/// area (the merged cell it anchors, or itself).
#[derive(Clone, Copy, PartialEq, Eq)]
struct Selection {
    range: CellRange,
    active: CellRange,
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
            fonts: look::FontCache::default(),
            selection_memo: Cell::new(None),
            scale: 1.0,
            layout: row_layout::RowLayout::default(),
            validation: Default::default(),
            filters_memo: RefCell::new(None),
            note_hover: None,
        };
        grid.resize();
        grid
    }

    /// The filter whose button sits in `(row, col)`, and whether that column
    /// is filtered.
    fn filter_button(&self, row: u32, col: u32) -> Option<(sheets_engine::FilterScope, bool)> {
        let key = (self.book.revision(), self.sheet);
        let mut memo = self.filters_memo.borrow_mut();
        if memo.as_ref().is_none_or(|(k, _)| *k != key) {
            *memo = Some((key, Rc::new(self.book.filters(self.sheet))));
        }
        let filters = &memo.as_ref().expect("set").1;
        filters
            .iter()
            .find(|f| f.area.top == row && (f.area.left..=f.area.right).contains(&col))
            .map(|f| (f.scope.clone(), f.is_filtered(col)))
    }

    /// The selected range; a range of cells includes every merged cell it
    /// touches whole, as in Excel.
    fn range(&self) -> CellRange {
        self.selection().range
    }

    fn selection(&self) -> Selection {
        let key = (
            self.anchor,
            self.end,
            self.span,
            self.sheet,
            self.book.revision(),
        );
        if let Some((k, sel)) = self.selection_memo.get()
            && k == key
        {
            return sel;
        }
        let raw = self.span.range(self.anchor, self.end);
        let range = match self.span {
            Span::Cells => self.book.expand_to_merges(self.sheet, raw),
            _ => raw,
        };
        let (r, c) = self.anchor;
        let active = self
            .book
            .merge_at(self.sheet, r, c)
            .unwrap_or_else(|| CellRange::from_corners(self.anchor, self.anchor));
        let sel = Selection { range, active };
        self.selection_memo.set(Some((key, sel)));
        sel
    }

    /// Recompute grid extents: the used range (and the cells charts and
    /// pictures cover) plus headroom, never below the minimum.
    fn resize(&mut self) {
        let (mut r, mut c) = self.book.used_size(self.sheet);
        if self.book.has_drawings() {
            for d in self.book.drawings(self.sheet) {
                use sheets_engine::drawing::Anchor;
                let (row, col) = match d.anchor {
                    Anchor::TwoCell { to, .. } => (to.row + 1, to.col + 1),
                    Anchor::OneCell { from, .. } => (from.row + 20, from.col + 8),
                    Anchor::Absolute { .. } => (0, 0),
                };
                r = r.max(row);
                c = c.max(col);
            }
        }
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

    fn row_heights(&self) -> Rc<RefCell<row_heights::RowHeights>> {
        self.layout.heights.clone()
    }

    fn frozen_rows(&self, _: &App) -> usize {
        self.book.frozen(self.sheet).0 as usize
    }

    fn sync_rows(&mut self, _: &mut Window, _: &mut Context<TableState<Self>>) {
        self.layout
            .sync(&self.book, self.sheet, self.rows, self.scale);
    }

    fn fit_rows(
        &mut self,
        rows: &[usize],
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> bool {
        self.layout
            .fit(rows, &self.book, self.sheet, &mut self.fonts, window, cx)
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
            let width = look::column_width(&self.book, self.sheet, col, self.scale);
            let frozen = col < self.book.frozen(self.sheet).1;
            // A hidden column has no width (and may have none).
            let min_width = if width > px(0.) { px(24.) } else { px(0.) };
            Column::new(SharedString::from(name.clone()), name)
                .width(width)
                .min_width(min_width)
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
        // Double-clicking a hidden column's edge shows it again, as wide as
        // it was (the width the table then reports is the workbook's, so
        // that adds no second undo step).
        if self.book.is_column_hidden(self.sheet, col) {
            self.book
                .set_columns_hidden(self.sheet, col, col, false)
                .ok()?;
            return Some(look::column_width(&self.book, self.sheet, col, self.scale));
        }
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
        // Excel's double line where columns are hidden: on the edge of the
        // shown column next to them.
        let hidden_left = col > 0 && self.book.is_column_hidden(self.sheet, col - 1);
        let hidden_right = self.book.is_column_hidden(self.sheet, col + 1);
        let cell = cell
            .id(("col-head", col_ix))
            .flex()
            .items_center()
            .justify_center()
            .child(SharedString::from(column_name(col)))
            .child(grid_line_right(t.grid_line))
            .when(hidden_left, |d| d.child(hidden_marker(false, false, &t)))
            .when(hidden_right, |d| d.child(hidden_marker(false, true, &t)))
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

    /// Rows have no border of their own: cells draw their gridlines, so a
    /// fill, a border or a merged cell can cover them.
    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> Stateful<Div> {
        div().id(("row", row_ix)).border_b(px(0.))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        window: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let _timer = kantor_ui::perf::frame_timer("sheets cells", window);
        let t = tokens(cx);
        let Selection { range, active } = self.selection();
        if col_ix == 0 {
            // Row-number gutter, highlighted for the rows of the selection.
            let row = row_ix as u32;
            let active = (range.top..=range.bottom).contains(&row);
            let whole = active && range.is_whole_rows();
            // Excel's double line where rows are hidden: under the shown row
            // above them (or over the first shown row, for hidden rows at
            // the top).
            // Rows above panes frozen on a scrolled sheet get no mark.
            let above_panes = self.book.frozen_origin(self.sheet).0 as usize;
            let (hidden_below, hidden_above) = {
                let h = self.layout.heights.borrow();
                (
                    row_ix + 1 < h.len() && h.height_dp(row_ix + 1) == 0,
                    row_ix > above_panes && h.prev_shown(row_ix - 1).is_none(),
                )
            };
            let frozen_edge = {
                let fr = self.book.frozen(self.sheet).0;
                fr > 0 && row + 1 == fr
            };
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
                .child(SharedString::from((row_ix + 1).to_string()))
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(GRID_LINE)
                        .bg(if frozen_edge {
                            t.border_strong
                        } else {
                            t.grid_line
                        }),
                )
                .when(hidden_below, |d| d.child(hidden_marker(true, true, &t)))
                .when(hidden_above, |d| d.child(hidden_marker(true, false, &t)))
                // Drag the bottom edge to resize, double-click to fit.
                .children(row_layout::resize_handles(row, &self.view));
            return grid_ops::header_element(gutter, Header::Row(row), &self.view);
        }
        let (row, col) = (row_ix as u32, col_ix as u32 - 1);
        let sheet = self.sheet;
        let mut look = self.book.cell_look(sheet, row, col);
        // A merged cell shows its anchor's value.
        let (vr, vc) = look.merge.map_or((row, col), |m| (m.top, m.left));
        let shown = self.book.display(sheet, vr, vc);
        // Conditional formatting (a merged cell's comes from its anchor).
        let cond = self.book.conditional(sheet, vr, vc);
        let shown = cond
            .as_ref()
            .and_then(|c| c.display.clone())
            .unwrap_or(shown);
        let mut text = if cond.as_ref().is_some_and(|c| !c.show_value) {
            String::new()
        } else {
            shown.text
        };
        // A formula typed in manual calculation has no value until F9.
        let pending = text.is_empty() && self.book.is_pending(sheet, vr, vc);
        if pending {
            text.push('…');
        }
        let style = cond
            .as_ref()
            .and_then(|c| c.style.as_deref())
            .unwrap_or(&look.style);
        // The table style, under the cell's own formatting.
        let table_look = self.book.table_cell_look(sheet, vr, vc);
        let table_styled;
        let style = match &table_look {
            Some(tl) => {
                table_styled = table_style(style, tl);
                &table_styled
            }
            None => style,
        };
        if let Some(tl) = &table_look {
            look.right = look.right.or(tl.right);
            look.bottom = look.bottom.or(tl.bottom);
            look.top = look.top.or(tl.top);
            look.left = look.left.or(tl.left);
            if style.fill.is_some() {
                look.grid_right = false;
                look.grid_bottom = false;
            }
            look.grid_right &= look.right.is_none();
            look.grid_bottom &= look.bottom.is_none();
        }
        let filter_button = self.filter_button(row, col);
        let align = self.book.display_align_of(style, sheet, vr, vc);
        let heights = self.layout.heights.clone();
        let heights = heights.borrow();
        let geometry = look
            .merge
            .map(|m| look::merge_geometry(&self.book, sheet, m, row, col, self.scale, &heights));
        let row_height = px(heights.height(row as usize) as f32);
        drop(heights);
        let content = look::Content {
            text: SharedString::from(text),
            style,
            align,
            color: if pending {
                t.fg_dim
            } else {
                look::value_color(
                    &cond_paint::text_backdrop(style, cond.as_deref()),
                    shown.color,
                    &t,
                )
            },
            base_size: FONT_BODY,
            default_font_size: self.book.default_font_size(),
            family: self.fonts.for_style(style, cx),
            height: geometry.map_or(row_height, |g| g.3),
            fill: shown.fill,
        };
        let cell = div()
            .size_full()
            .relative()
            .when_some(style.fill, |d, fill| d.bg(look::hsla(fill)));
        let cell = match filter_button {
            Some((scope, filtered)) => cell.child(filter_button_element(
                scope, col, filtered, row_height, &self.view, &t,
            )),
            None => cell,
        };
        let cell = look::gridlines(cell, &look, &t);
        // Data bars and icons (not drawn across merged cells).
        let cell = match cond.as_ref().filter(|_| look.merge.is_none()) {
            Some(c) => cell.children(cond_paint::layers(c)),
            None => cell,
        };
        let cell = match geometry {
            // The whole merged block, shifted so this cell shows its slice.
            Some((x, y, w, h)) => {
                cell.child(content.apply(div().absolute().left(-x).top(-y).w(w).h(h)))
            }
            None => content.apply(cell),
        };
        let validation = self.validation_layers(row, col, row_height, cx);
        // A note: its corner mark, and the note while the pointer is over
        // the cell (not while the cell is being edited).
        let note_layers = self
            .book
            .note_at(sheet, row, col)
            .map(|note| {
                let editing = self
                    .editor
                    .as_ref()
                    .is_some_and(|e| e.cell == (row, col) && e.sheet == sheet);
                let hovered = !editing && self.note_hover == Some((sheet, row, col));
                notes_ui::layers(&note, (sheet, row, col), hovered, &self.view, &t)
            })
            .unwrap_or_default();
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
        // A selected merged cell counts as one cell.
        let single = range == active;
        let fill_view = self.view.clone();
        let frozen_rows = self.book.frozen(self.sheet).0;
        cell.children(look::borders(&look, &t))
            .when(!single && range.contains(row, col), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .when(!active.contains(row, col), |d| d.bg(t.accent_soft))
                        .border_color(t.accent)
                        .when(row == range.top, |d| d.border_t_1())
                        .when(row == range.bottom, |d| d.border_b_1())
                        .when(col == range.left, |d| d.border_l_1())
                        .when(col == range.right, |d| d.border_r_1()),
                )
            })
            .children(marks)
            // The active cell (or merged cell): a strong outline and no fill,
            // even inside a range.
            .when(active.contains(row, col), |d| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_color(t.accent)
                        .when(row == active.top, |d| d.border_t(CELL_OUTLINE))
                        .when(row == active.bottom, |d| d.border_b(CELL_OUTLINE))
                        .when(col == active.left, |d| d.border_l(CELL_OUTLINE))
                        .when(col == active.right, |d| d.border_r(CELL_OUTLINE)),
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
            // Frozen rows (pinned by the table): a stronger line under the
            // last one.
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
            .children(validation)
            .children(note_layers)
            .when_some(
                self.editor
                    .as_ref()
                    .filter(|e| e.cell == (row, col) && e.sheet == self.sheet),
                |d, editor| {
                    let (fr, fc) = self.book.frozen(self.sheet);
                    d.child(editor.render(self.ref_span_colors(&t), (row < fr, col < fc), cx))
                },
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

/// A cell's style with a table style's look under it: the cell's own fill,
/// text colour and borders win, bold adds up.
fn table_style(style: &CellStyle, tl: &sheets_engine::TableCellLook) -> CellStyle {
    let mut s = style.clone();
    if s.fill.is_none() {
        s.fill = tl.fill;
    }
    if s.font_color.is_none() {
        s.font_color = tl.font_color;
    }
    s.bold |= tl.bold;
    s
}

/// The filter button in a filter's header cell: a chevron, or a funnel when
/// the column is filtered.
fn filter_button_element(
    scope: sheets_engine::FilterScope,
    col: u32,
    filtered: bool,
    row_height: Pixels,
    view: &WeakEntity<SheetsView>,
    t: &kantor_ui::Tokens,
) -> AnyElement {
    let view = view.clone();
    let size = (row_height - px(4.)).clamp(px(12.), px(18.));
    div()
        .id(("filter-button", col as usize))
        .absolute()
        .right(px(2.))
        .top((row_height - size) / 2.)
        .size(size)
        .flex()
        .items_center()
        .justify_center()
        .rounded(RADIUS_SM)
        .border_1()
        .border_color(if filtered { t.accent } else { t.border_strong })
        .bg(if filtered { t.accent_soft } else { t.surface })
        .text_color(if filtered { t.accent } else { t.fg_muted })
        .cursor_pointer()
        .hover(|s| s.border_color(t.accent))
        .child(
            gpui_kit::component::Icon::new(if filtered {
                gpui_kit::assets::IconName::Funnel
            } else {
                gpui_kit::assets::IconName::ChevronDown
            })
            .size(size - px(5.)),
        )
        .on_mouse_down(MouseButton::Left, move |e, window, cx| {
            // Not a click on the cell.
            cx.stop_propagation();
            let at = e.position;
            let scope = scope.clone();
            let _ = view.update(cx, |v, cx| v.open_filter_menu(scope, col, at, window, cx));
        })
        .into_any_element()
}

/// Excel's mark for hidden rows or columns in a header: two lines across
/// the header's edge (`rows`: the bottom (`end`) or top edge of a row
/// number; else the right (`end`) or left edge of a column letter). Mouse
/// input passes through to the resize handles under it.
fn hidden_marker(rows: bool, end: bool, t: &kantor_ui::Tokens) -> Div {
    let d = div()
        .absolute()
        .border_color(t.border_strong)
        .bg(t.grid_header);
    if rows {
        d.left_0()
            .right_0()
            .h(px(3.))
            .border_t_1()
            .border_b_1()
            .map(|d| if end { d.bottom_0() } else { d.top_0() })
    } else {
        d.top_0()
            .bottom_0()
            .w(px(3.))
            .border_l_1()
            .border_r_1()
            .map(|d| if end { d.right_0() } else { d.left_0() })
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
    /// The cell a plain click went to, so the table's `SelectCell` for it
    /// selects the merged cell there rather than stepping over it.
    clicked: Option<(u32, u32)>,
    status: SharedString,
    /// Last measured pane width (for compact layout).
    width: Rc<Cell<Pixels>>,
    /// Name shown while the document has no file (`Untitled 2.xlsx`), given
    /// by the shell so new documents are numbered across the session.
    untitled_name: Option<String>,
    /// Find & Replace (Ctrl+F / Ctrl+H).
    find: find::SheetsFind,
    /// Opening or recalculating in the background (see [`calc`]).
    busy: Option<calc::Busy>,
    /// The file being opened (or that failed to open), until it is shown.
    opening: Option<PathBuf>,
    /// Holds keyboard focus while the grid is replaced by the busy pane.
    busy_focus: FocusHandle,
    /// `KANTOR_PERF_SCROLL` benchmark in progress (see [`bench`]).
    bench: Option<bench::ScrollBench>,
    /// A drag on a row number's edge (see [`row_layout`]).
    row_resize: Option<row_layout::RowResize>,
    /// The "Row Height…" prompt.
    row_height_prompt: Option<(Entity<InputState>, Subscription)>,
    /// A data validation error alert waiting for an answer.
    alert: Option<data_validation::Alert>,
    /// Closes the validation drop-down when it loses focus.
    validation_list_sub: Option<Subscription>,
    /// A column's filter menu (see [`filter_menu`]).
    filter_menu: Option<filter_menu::FilterMenu>,
    /// The Custom Sort panel.
    sort_panel: Option<filter_menu::SortPanel>,
    /// Charts and pictures over the grid (see [`drawings`]).
    drawings: Rc<RefCell<drawings::DrawLayer>>,
    /// The Insert PivotTable card (see [`pivot`]).
    pivot_dialog: Option<pivot::InsertDialog>,
    /// The PivotTable Fields panel beside the grid.
    pivot_panel: Option<pivot::FieldPanel>,
    /// The Format as Table card and the Table Design panel (see
    /// [`table_ui`]).
    table_dialog: Option<table_ui::TableDialog>,
    table_panel: Option<table_ui::TablePanel>,
    /// The conditional formatting rule card and Manage Rules (see
    /// [`cf_ui`]).
    cf_dialog: Option<cf_ui::CfDialog>,
    cf_manager: Option<cf_ui::CfManager>,
    /// The Chart panel (see [`chart_ui`]) and a "series in rows" choice
    /// waiting for the window.
    chart_panel: Option<chart_ui::ChartPanel>,
    pending_chart_rows: Option<bool>,
    /// The Data Validation card (see [`dv_ui`]).
    dv_dialog: Option<dv_ui::DvDialog>,
    /// The Page Setup card (see [`page_setup`]).
    page_setup: Option<page_setup::PageSetupDialog>,
    /// The Spelling dialog (see [`spell_ui`]).
    spell_dialog: Option<spell_ui::SpellDialog>,
    /// A restored recovery copy's original: Save asks before overwriting it
    /// if it changed on disk (see [`recovery`]).
    disk_guard: Option<kantor_ui::disk_guard::DiskGuard>,
    /// The save being written in the background (see [`saving`]).
    saving: Option<saving::Saving>,
    /// A save written while the workbook was in the background, to record
    /// when it's back.
    pending_save: Option<sheets_engine::Saved>,
    /// The chart sheet shown instead of the grid (an index of
    /// `Workbook::chart_sheet_name`), and what has focus while it shows.
    chart_tab: Option<usize>,
    chart_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SheetsView {
    /// A new workbook, or the file at `path`, which opens in the background
    /// (the pane shows its progress meanwhile).
    pub fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let book = Workbook::new_empty().expect("empty workbook");
        let status = i18n::t("Ready").to_owned();

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
        let formula =
            cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t("Value or =formula")));
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
            clicked: None,
            status: status.into(),
            width: Rc::new(Cell::new(px(0.))),
            untitled_name: None,
            find: find::SheetsFind::new(window, cx),
            busy: None,
            opening: None,
            busy_focus: cx.focus_handle(),
            chart_tab: None,
            chart_focus: cx.focus_handle(),
            bench: bench::ScrollBench::from_env(),
            row_resize: None,
            row_height_prompt: None,
            alert: None,
            validation_list_sub: None,
            filter_menu: None,
            sort_panel: None,
            drawings: Rc::default(),
            pivot_dialog: None,
            pivot_panel: None,
            page_setup: None,
            spell_dialog: None,
            table_dialog: None,
            cf_dialog: None,
            dv_dialog: None,
            chart_panel: None,
            pending_chart_rows: None,
            cf_manager: None,
            table_panel: None,
            disk_guard: None,
            saving: None,
            pending_save: None,
            _subscriptions: subscriptions,
        };
        this.select_cell(0, 0, cx);
        match path {
            Some(path) => {
                this.start_open(path, window, cx);
                window.focus(&this.busy_focus, cx);
            }
            None => this.focus_grid(window, cx),
        }
        this
    }

    /// While opening or recalculating, refuse a command (and say why).
    fn blocked(&mut self, cx: &mut Context<Self>) -> bool {
        if self.busy.is_some() {
            self.status = i18n::t("Please wait until the workbook is ready").into();
            cx.notify();
        }
        self.busy.is_some()
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
                let cell = (*row as u32, *col as u32 - 1);
                // Pressing a cell selects (the anchor) at once; the table's
                // click selects the cell itself on release.
                let clicked = self.clicked == Some(cell);
                if clicked {
                    self.clicked = None;
                }
                if let Some(target) = self.merge_redirect(self.selected, cell, clicked, cx) {
                    // Inside a merged cell: select its anchor, or step past it.
                    self.selected = target;
                    self.select_cell(target.0, target.1, cx);
                    return;
                }
                self.selected = cell;
                self.close_validation_list(window, cx);
                // A new active cell starts a new range (unless a header
                // click asked for one).
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

    /// Where the active cell goes when the table selects `to` (coming from
    /// `from`) and `to` lies inside a merged cell other than its anchor:
    /// the anchor, unless the move started at that anchor (an arrow key or
    /// Enter from the merged cell), which steps past the merge in the same
    /// direction. `None` when `to` needs no redirect.
    fn merge_redirect(
        &self,
        from: (u32, u32),
        to: (u32, u32),
        clicked: bool,
        cx: &App,
    ) -> Option<(u32, u32)> {
        let grid = self.table.read(cx).delegate();
        let m = grid.book.merge_at(grid.sheet, to.0, to.1)?;
        look::merge_step(m, from, to, clicked, (grid.rows, grid.cols))
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
        let plain = !(ks.modifiers.control || ks.modifiers.platform || ks.modifiers.alt);
        if plain && self.drawing_key(ks.key.as_str(), window, cx) {
            cx.stop_propagation();
            return;
        }
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
        if self.blocked(cx) {
            return;
        }
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
        self.status = i18n::t("Edit cancelled").into();
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
            let manual = self.table.read(cx).delegate().book.calc_mode() == CalcMode::Manual;
            let stored = result.is_ok();
            self.status = match result {
                Ok(()) if manual => {
                    let msg = i18n::t("{} updated — manual calculation, press F9 to recalculate");
                    i18n::fill(msg, &[&cell_name(row, col)]).into()
                }
                Ok(()) => tf("{} updated", &[&cell_name(row, col)]).into(),
                Err(err) => format!("{err:#}").into(),
            };
            // A value its data validation rejects waits for the alert.
            if stored && self.check_typed_value(sheet, (row, col), &value, step, window, cx) {
                self.update_title(window, cx);
                return;
            }
        }
        self.finish_commit(sheet, (row, col), step, window, cx);
    }

    /// The end of a commit of (`row`, `col`): move by `step` and return to
    /// the grid, or (no step: focus went elsewhere) show the newly selected
    /// cell's content.
    fn finish_commit(
        &mut self,
        sheet: u32,
        (row, col): (u32, u32),
        step: Option<(i64, i64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(step) = step {
            // Pointing may have left another sheet on screen.
            if self.grid_sheet(cx) != sheet {
                self.show_sheet(sheet, cx);
            }
            let (row, col) = {
                let grid = self.table.read(cx).delegate();
                let (book, sheet) = (&grid.book, grid.sheet);
                cell_editor::step_cell(
                    (row, col),
                    step,
                    grid.rows,
                    grid.cols,
                    |r| book.row_skipped(sheet, r),
                    |c| book.col_skipped(sheet, c),
                )
            };
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
        if self.blocked(cx) {
            return;
        }
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
            let mut row = step(grid.end.0, d_row, grid.rows);
            let mut col = step(grid.end.1, d_col, grid.cols);
            // Hidden rows and columns are stepped over, as in Excel.
            if d_row != 0 {
                let h = grid.layout.heights.borrow();
                let shown = if d_row > 0 {
                    h.next_shown(row as usize)
                } else {
                    h.prev_shown(row as usize)
                };
                row = shown.map_or(grid.end.0, |r| r as u32);
            }
            while d_col != 0
                && grid.book.is_column_hidden(grid.sheet, col)
                && ((d_col > 0 && col + 1 < grid.cols) || (d_col < 0 && col > 0))
            {
                col = if d_col > 0 { col + 1 } else { col - 1 };
            }
            if grid.book.is_column_hidden(grid.sheet, col) {
                col = grid.end.1;
            }
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
            self.clicked = Some((row, col));
            let (row, col) = {
                let grid = self.table.read(cx).delegate();
                grid.book
                    .merge_at(grid.sheet, row, col)
                    .map_or((row, col), |m| (m.top, m.left))
            };
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
        self.status = tf("Copied {}", &[&range.name()]).into();
        cx.notify();
    }

    /// Copy, then clear the range straight away (not when pasted, as Excel does).
    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let range = self.copy_range(cx);
        self.clear_selected(window, cx);
        self.status = tf("Cut {}", &[&range.name()]).into();
        cx.notify();
    }

    /// Paste clipboard text (tab-separated) at the active cell and select what
    /// was pasted.
    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            self.status = i18n::t("Nothing to paste").into();
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
            Ok(Some(range)) => tf("Pasted {}", &[&range.name()]).into(),
            Ok(None) => i18n::t("Nothing to paste").into(),
            Err(err) => format!("{err:#}").into(),
        };
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    /// The active cell's formatting, for the toolbar.
    fn active_style(&self, cx: &App) -> std::sync::Arc<CellStyle> {
        let (row, col) = self.selected;
        let grid = self.table.read(cx).delegate();
        grid.book.cell_style(grid.sheet, row, col)
    }

    /// Apply a formatting command to the selected range (one undo step).
    pub fn format_doc(&mut self, format: Format, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let style = self.active_style(cx);
        let Some(range) = self.op_range(cx) else {
            return;
        };
        enum Edit {
            Style(StyleChange),
            Borders(BorderPreset),
            Merge,
            Unmerge,
        }
        let (edit, what) = match format {
            Format::Bold => (Edit::Style(StyleChange::Bold(!style.bold)), tk("Bold")),
            Format::Italic => (
                Edit::Style(StyleChange::Italic(!style.italic)),
                tk("Italic"),
            ),
            Format::Underline => (
                Edit::Style(StyleChange::Underline(!style.underline)),
                tk("Underline"),
            ),
            Format::Strikethrough => (
                Edit::Style(StyleChange::Strikethrough(!style.strikethrough)),
                tk("Strikethrough"),
            ),
            Format::Align(a) => {
                let a = if style.align == a { HAlign::General } else { a };
                (Edit::Style(StyleChange::Align(a)), tk("Alignment"))
            }
            Format::VAlign(a) => (
                Edit::Style(StyleChange::VAlign(a)),
                tk("Vertical alignment"),
            ),
            Format::Number(f) => (Edit::Style(StyleChange::NumberFormat(f)), f.label()),
            Format::Fill(c) => (Edit::Style(StyleChange::Fill(c)), tk("Fill colour")),
            Format::FontColor(c) => (Edit::Style(StyleChange::FontColor(c)), tk("Text colour")),
            Format::FontSize(d) => (Edit::Style(StyleChange::FontSizeDelta(d)), tk("Font size")),
            Format::Wrap => (Edit::Style(StyleChange::Wrap(!style.wrap)), tk("Wrap text")),
            Format::Borders(p) => (Edit::Borders(p), tk("Borders")),
            Format::Merge => {
                let grid = self.table.read(cx).delegate();
                if grid.book.merges_in(grid.sheet, range).is_empty() {
                    (Edit::Merge, tk("Merged"))
                } else {
                    (Edit::Unmerge, tk("Unmerged"))
                }
            }
        };
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = match edit {
                Edit::Style(change) => grid.book.set_range_style(sheet, range, change),
                Edit::Borders(preset) => grid.book.set_range_borders(sheet, range, preset),
                Edit::Merge => grid.book.merge_range(sheet, range),
                Edit::Unmerge => grid.book.unmerge_range(sheet, range).map(|_| ()),
            };
            cx.notify();
            res
        });
        self.status = match result {
            Ok(()) => format!("{}: {}", i18n::t_dyn(what), range.name()).into(),
            Err(err) => format!("{err:#}").into(),
        };
        // A merge moves the active cell to its anchor.
        if matches!(format, Format::Merge) {
            let (r, c) = (range.top, range.left);
            self.selected = (r, c);
            self.select_cell(r, c, cx);
        }
        self.load_selected_into_formula(window, cx);
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
            // Table column 0 is the row-number gutter. Columns the table
            // still has at their snapped width keep the workbook's exact one.
            let widths: Vec<(u32, f64)> = widths
                .iter()
                .enumerate()
                .skip(1)
                .map(|(ix, w)| {
                    let col = ix as u32 - 1;
                    let snapped = look::column_width(&grid.book, sheet, col, grid.scale);
                    if (*w - snapped).abs() < px(0.01) {
                        (col, grid.book.column_width(sheet, col))
                    } else {
                        (col, f64::from(f32::from(*w)))
                    }
                })
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
        if self.editing.is_some() || self.blocked(cx) {
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
            (Ok(true), false) => i18n::t("Undone").into(),
            (Ok(true), true) => i18n::t("Redone").into(),
            (Ok(false), false) => i18n::t("Nothing to undo").into(),
            (Ok(false), true) => i18n::t("Nothing to redo").into(),
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
        if self.chart_tab.take().is_some() && self.editing.is_none() {
            self.status = "Ready".into();
            self.show_sheet(sheet, cx);
            self.load_selected_into_formula(window, cx);
            self.focus_grid(window, cx);
            return;
        }
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

    /// Show chart sheet `k` in place of the grid (an edit in progress is
    /// committed first, as when Excel leaves a sheet).
    fn select_chart_sheet(&mut self, k: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.commit_edit(None, window, cx);
        }
        self.drawings.borrow_mut().selected = None;
        self.chart_tab = Some(k);
        window.focus(&self.chart_focus, cx);
        cx.notify();
    }

    /// The chart sheet shown, if one is (and still exists).
    fn shown_chart_sheet(&self, cx: &App) -> Option<(usize, String)> {
        let k = self.chart_tab?;
        let name = self
            .table
            .read(cx)
            .delegate()
            .book
            .chart_sheet_name(k)?
            .to_owned();
        Some((k, name))
    }

    /// Whether a chart sheet is shown instead of the grid.
    pub fn on_chart_sheet(&self, cx: &App) -> bool {
        self.shown_chart_sheet(cx).is_some()
    }

    /// For commands on cells: `true` (and a status saying why) when a chart
    /// sheet is shown, so they don't change a worksheet out of sight.
    pub fn chart_sheet_blocks(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((_, name)) = self.shown_chart_sheet(cx) else {
            return false;
        };
        self.status = format!(
            "\u{201c}{name}\u{201d} is a chart sheet: select a worksheet tab to use this command"
        )
        .into();
        cx.notify();
        true
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
        let sel = self.table.read(cx).delegate().selection();
        // A selected merged cell is named by its anchor, as in Excel.
        if sel.range == sel.active {
            cell_name(sel.active.top, sel.active.left)
        } else {
            sel.range.name()
        }
    }

    /// File name for tabs, e.g. `budget.xlsx` or `Untitled 2.xlsx`.
    pub fn doc_name(&self, cx: &App) -> String {
        if let Some(busy) = &self.busy {
            return busy.name.clone();
        }
        self.name(&self.table.read(cx).delegate().book)
    }

    /// Set the name shown while the workbook has no file.
    pub fn set_untitled_name(&mut self, name: String, cx: &mut Context<Self>) {
        self.untitled_name = Some(name);
        cx.notify();
    }

    /// `book`'s file name, or the shell-given name while it has no file.
    fn name(&self, book: &Workbook) -> String {
        if book.source_path().is_none()
            && let Some(name) = self.opening.as_deref().and_then(|p| p.file_name())
        {
            return name.to_string_lossy().into_owned();
        }
        match &self.untitled_name {
            Some(name) if book.source_path().is_none() => name.clone(),
            _ => book.title(),
        }
    }

    /// The file this document shows (an imported `.ods`/`.xls` too, though
    /// Save never writes there), if any.
    pub fn path(&self, cx: &App) -> Option<PathBuf> {
        if let Some(busy) = &self.busy {
            return busy.path.clone();
        }
        self.table
            .read(cx)
            .delegate()
            .book
            .source_path()
            .map(|p| p.to_path_buf())
    }

    pub fn is_dirty(&self, cx: &App) -> bool {
        if let Some(busy) = &self.busy {
            return busy.dirty;
        }
        self.table.read(cx).delegate().book.is_dirty()
    }

    /// Give keyboard focus to the grid.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_grid(window, cx);
    }

    /// The grid's focus (typing there starts a cell edit).
    pub fn editor_focus_handle(&self, cx: &App) -> FocusHandle {
        self.table.read(cx).focus_handle(cx)
    }

    pub fn save_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save(&Save, window, cx);
    }

    pub fn save_doc_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as(&SaveAs, window, cx);
    }

    pub fn title(&self, cx: &App) -> String {
        let dirty = if self.is_dirty(cx) { "• " } else { "" };
        format!("{dirty}{} — Kantor Sheets", self.doc_name(cx))
    }

    fn update_title(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        window.set_window_title(&title);
    }

    /// Replace the open workbook with the file at `path` (opened in the
    /// background).
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy.is_some() || self.saving.is_some() {
            return;
        }
        self.start_open(path, window, cx);
        window.focus(&self.busy_focus, cx);
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        if self.blocked(cx) {
            return;
        }
        let Some(target) = self
            .table
            .read(cx)
            .delegate()
            .book
            .path()
            .map(|p| p.to_path_buf())
        else {
            self.save_as(&SaveAs, window, cx);
            return;
        };
        if self.ask_before_overwrite(&target, window, cx) {
            return;
        }
        self.start_save(target, false, window, cx);
    }

    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as_task(window, cx).detach();
    }

    /// Save before the document closes (waiting for a save already
    /// running). Resolves to `true` once the workbook is saved; `false` if
    /// the Save As prompt was cancelled or saving failed.
    pub fn save_for_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        let running = self.saved_signal();
        cx.spawn_in(window, async move |this, cx| {
            if let Some(running) = running {
                let _ = running.await;
            }
            let Ok(next) =
                this.update_in(cx, |this, window, cx| this.save_now_for_close(window, cx))
            else {
                return false;
            };
            next.await
        })
    }

    fn save_now_for_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if !self.is_dirty(cx) && self.busy.is_none() {
            return Task::ready(true);
        }
        if self.blocked(cx) {
            return Task::ready(false);
        }
        let Some(target) = self
            .table
            .read(cx)
            .delegate()
            .book
            .path()
            .map(|p| p.to_path_buf())
        else {
            return self.save_as_task(window, cx);
        };
        match self.start_save(target, false, window, cx) {
            Some(done) => cx.background_spawn(async move { matches!(done.await, Ok(Ok(true))) }),
            None => Task::ready(false),
        }
    }

    /// Ask for a path and save there; resolves to whether the workbook is saved.
    fn save_as_task(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if self.blocked(cx) {
            return Task::ready(false);
        }
        if self.saving.is_some() {
            self.status = i18n::t("Please wait until the save finishes").into();
            cx.notify();
            return Task::ready(false);
        }
        let (dir, name) = {
            let book = &self.table.read(cx).delegate().book;
            let dir = book
                .source_path()
                .and_then(|p| p.parent())
                .map(|p| p.to_path_buf())
                .or_else(kantor_core::dirs::home_dir)
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
            let Ok(Some(done)) = this.update_in(cx, |this, window, cx| {
                this.start_save(path, false, window, cx)
            }) else {
                return false;
            };
            matches!(done.await, Ok(Ok(true)))
        })
    }

    fn render_sheet_tabs(&self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let chart = self.shown_chart_sheet(cx).map(|(k, _)| k);
        let grid = self.table.read(cx).delegate();
        let names = grid.book.sheet_names();
        let tabs: Vec<(Tab, String)> = grid
            .book
            .tabs()
            .into_iter()
            .map(|tab| {
                let name = match tab {
                    Tab::Sheet(i) => names[i as usize].clone(),
                    Tab::Chart(k) => grid.book.chart_sheet_name(k).unwrap_or_default().to_owned(),
                };
                (tab, name)
            })
            .collect();
        let active = match chart {
            Some(k) => Tab::Chart(k),
            None => Tab::Sheet(grid.sheet),
        };
        h_flex()
            .gap(SPACE_1)
            .children(tabs.into_iter().map(|(tab, name)| {
                let view = cx.entity().downgrade();
                let selected = tab == active;
                let id = match tab {
                    Tab::Sheet(ix) => ElementId::from(("sheet-tab", ix as usize)),
                    Tab::Chart(k) => ElementId::from(("chart-sheet-tab", k)),
                };
                div()
                    .id(id)
                    .h(CONTROL_SM)
                    .px(SPACE_5)
                    .flex()
                    .items_center()
                    .gap(SPACE_2)
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
                    .when(matches!(tab, Tab::Chart(_)), |d| {
                        d.child(
                            gpui_kit::component::Icon::new(gpui_kit::assets::IconName::ChartColumn)
                                .xsmall(),
                        )
                    })
                    .child(name)
                    .on_click(move |_, window, cx| {
                        let _ = view.update(cx, |this, cx| match tab {
                            Tab::Sheet(ix) => this.select_sheet(ix, window, cx),
                            Tab::Chart(k) => this.select_chart_sheet(k, window, cx),
                        });
                    })
            }))
    }
}

impl Render for SheetsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_print_preview(window, cx);
        let _span = kantor_ui::perf::span("sheets view");
        let t = tokens(cx);
        // Column widths are snapped to device pixels: new ones on a new scale.
        let scale = window.scale_factor();
        if self.table.read(cx).delegate().scale != scale {
            self.table.update(cx, |table, cx| {
                table.delegate_mut().scale = scale;
                table.refresh(cx);
            });
        }
        let completion = self.render_completion(window, cx);
        // The bar colours references only while it holds the edit.
        let bar_refs = if self.editing.is_some() {
            self.table.read(cx).delegate().ref_span_colors(&t)
        } else {
            Vec::new()
        };
        let tabs = self.render_sheet_tabs(cx);
        let chart_sheet = self.shown_chart_sheet(cx);
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

        if let Some(busy) = self.render_busy(cx) {
            return v_flex()
                .key_context(KEY_CONTEXT)
                .size_full()
                .bg(t.bg)
                .text_color(t.fg)
                .text_size(FONT_BODY)
                .child(busy)
                .into_any_element();
        }
        if self.opening.is_none() {
            self.bench_step(window, cx);
        }
        self.apply_pending_chart(window, cx);
        self.sync_chart_panel(window, cx);
        let root = self.editor_actions(v_flex(), cx);
        let root = self.point_actions(root, cx);
        let root = self.calc_actions(root, cx);
        let root = self.pivot_actions(root, cx);
        let root = self.page_setup_actions(root, cx);
        let root = self.table_actions(root, cx);
        let root = self.cf_actions(root, cx);
        let root = self.dv_actions(root, cx);
        let root = self.chart_actions(root, cx);
        let root = self.spell_actions(root, cx);
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
            .when_some(chart_sheet.clone(), |root, (k, name)| {
                root.child(self.render_chart_sheet_pane(k, name, cx))
            })
            .when(chart_sheet.is_none(), |root| {
                root.child({
                    let style = self.active_style(cx);
                    let merged = {
                        let grid = self.table.read(cx).delegate();
                        let (r, c) = self.selected;
                        grid.book.merge_at(grid.sheet, r, c).is_some()
                    };
                    toolbar::render(
                        &toolbar::Active {
                            style: &style,
                            merged,
                        },
                        compact,
                        cx,
                    )
                })
            })
            // Formula bar: name box + input
            .when(chart_sheet.is_none(), |root| {
                root.child(
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
                                .child(
                                    Input::new(&self.formula)
                                        .small()
                                        .text_color(point::refs_text_color(&bar_refs, t.fg)),
                                )
                                .child(point::colored_refs(self.formula.clone(), bar_refs, t.fg)),
                        ),
                )
                // Grid, and the PivotTable Fields panel beside it.
                .child({
                    let panel = self.render_pivot_panel(cx);
                    let table_panel = self.render_table_panel(cx);
                    let chart_panel = self.render_chart_panel(cx);
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .items_start()
                        .child(
                            self.validation_actions(self.grid_actions(div(), cx), cx)
                                .flex_1()
                                .min_w_0()
                                .h_full()
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
                                    this.row_resize_end(window, cx);
                                }))
                                .on_key_down(cx.listener(Self::on_grid_key))
                                .relative()
                                .child(DataTable::new(&self.table).bordered(false).small())
                                .child(self.render_drawings(cx)),
                        )
                        .children(panel)
                        .children(table_panel)
                        .children(chart_panel)
                })
            })
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
            .children(self.render_row_height_prompt(cx))
            .children(self.render_alert(cx))
            .children(self.render_sort_panel(cx))
            .children(self.render_filter_menu(cx))
            .children(self.render_pivot_dialog(cx))
            .children(self.render_page_setup(cx))
            .children(self.render_table_dialog(cx))
            .children(self.render_cf_manager(cx))
            .children(self.render_cf_dialog(cx))
            .children(self.render_dv_dialog(cx))
            .children(self.render_spell_dialog(cx))
            .into_any_element()
    }
}
