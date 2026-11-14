//! Row heights in the grid, like Excel's:
//!
//! * A row the file gives a custom height (`customHeight`) is drawn at it,
//!   relative to the sheet's default row height: a default-height row is
//!   Kantor's usual [`look::row_height`], a row twice as tall in Excel is
//!   twice as tall here. Hidden rows take no room.
//! * Other rows grow to fit their wrapped text and larger fonts (Excel's
//!   auto-fit), never below the default. Fitting needs text shaping, so it
//!   runs lazily for the rows about to be shown ([`RowLayout::fit`], called
//!   by the table before it builds them) and is remembered until the
//!   workbook changes; a row not measured yet uses the height Excel saved
//!   for it, if any.
//! * The user drags a row number's bottom edge to resize (the selected
//!   whole rows together, if the row is one of them), double-clicks it to
//!   auto-fit, or uses "Row Height…" / "Autofit Row Height". Each is one
//!   undo step, saved to xlsx as a custom height (auto-fit removes it).
//!
//! Positions come from [`RowHeights`] (prefix sums in device pixels), rebuilt
//! in O(rows) only when the workbook, the sheet or the scale changes.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::component::input::{self, Input, InputEvent, InputState};
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, tf};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{RowHeight, Workbook};

use crate::SheetsView;
use crate::look::{self, FontCache};
use crate::row_heights::{RowHeights, device_px};

/// Room above and below fitted text.
const FIT_PAD: f32 = 4.0;
/// Thinnest row a drag makes (logical px).
const MIN_DRAG_PX: f32 = 4.0;
/// Most rows a height command changes at once.
const MAX_ROWS_AT_ONCE: u32 = 100_000;
/// Most cells measured for one row (a row of wrapped cells across thousands
/// of columns stays cheap).
const FIT_MAX_CELLS: usize = 200;

/// What the heights were last built from.
#[derive(Clone, Copy, PartialEq)]
struct Key {
    sheet: u32,
    revision: (u64, u64),
    scale: u32,
    rows: u32,
    preview: Option<(u32, u32)>,
    /// Rows above panes frozen on a scrolled sheet (out of sight).
    above_panes: u32,
}

pub(crate) struct RowLayout {
    pub(crate) heights: Rc<RefCell<RowHeights>>,
    key: Option<Key>,
    /// Rows drawn at a height of their own (custom, hidden, being dragged):
    /// never fitted.
    fixed: Vec<bool>,
    /// Fitted heights of automatic rows in device pixels (0: not measured),
    /// kept across rebuilds so rows don't jump back while being re-measured.
    fitted: Vec<u32>,
    /// The generation each fitted height was measured in.
    fitted_at: Vec<u32>,
    generation: u32,
    /// A row being resized by a drag, and its height so far (logical px).
    pub(crate) preview: Option<(u32, f32)>,
}

impl Default for RowLayout {
    fn default() -> Self {
        Self {
            heights: Rc::new(RefCell::new(RowHeights::uniform(0, 1, 1.0))),
            key: None,
            fixed: Vec::new(),
            fitted: Vec::new(),
            fitted_at: Vec::new(),
            generation: 1,
            preview: None,
        }
    }
}

/// Logical pixels per point of row height on `sheet`: the sheet's default
/// row height is drawn as Kantor's default row.
pub(crate) fn px_per_point(book: &Workbook, sheet: u32) -> f64 {
    f64::from(f32::from(look::row_height())) / book.default_row_points(sheet)
}

impl RowLayout {
    /// Rebuild the heights if the workbook, the sheet, its row count, the
    /// scale or the drag preview changed since the last build.
    pub(crate) fn sync(&mut self, book: &Workbook, sheet: u32, rows: u32, scale: f32) {
        let key = Key {
            sheet,
            revision: book.revision(),
            scale: scale.to_bits(),
            rows,
            preview: self.preview.map(|(r, h)| (r, h.to_bits())),
            above_panes: book.frozen_origin(sheet).0,
        };
        let Some(old) = self.key.replace(key) else {
            self.rebuild(book, key, scale, true);
            return;
        };
        if old == key {
            return;
        }
        // Fitted heights hold for the same sheet at the same scale; any
        // other change to the workbook makes them stale (but still the best
        // guess until re-measured).
        let reset = old.sheet != sheet || old.scale != key.scale;
        if old.revision != key.revision || reset {
            self.generation += 1;
        }
        self.rebuild(book, key, scale, reset);
    }

    fn rebuild(&mut self, book: &Workbook, key: Key, scale: f32, reset: bool) {
        let n = key.rows as usize;
        if reset {
            self.fitted.clear();
            self.fitted_at.clear();
        }
        self.fitted.resize(n, 0);
        self.fitted_at.resize(n, 0);
        let spec = book.row_spec(key.sheet);
        let k = f64::from(f32::from(look::row_height())) / spec.default_points;
        let default_dp = device_px(f32::from(look::row_height()), scale);
        let mut heights = vec![default_dp; n];
        let mut fixed = vec![false; n];
        for e in spec.rows.iter().filter(|e| (e.row as usize) < n) {
            let r = e.row as usize;
            let dp = device_px((e.points * k) as f32, scale);
            if e.hidden {
                heights[r] = 0;
                fixed[r] = true;
            } else if e.custom {
                heights[r] = dp;
                fixed[r] = true;
            } else {
                // Excel's fitted height, until measured here.
                heights[r] = dp.max(default_dp);
            }
        }
        for r in 0..n {
            if !fixed[r] && self.fitted[r] > 0 {
                heights[r] = self.fitted[r];
            }
        }
        // Above panes frozen on a scrolled sheet: out of sight, as in Excel.
        for r in 0..(key.above_panes as usize).min(n) {
            heights[r] = 0;
            fixed[r] = true;
        }
        if let Some((row, h)) = self.preview
            && (row as usize) < n
        {
            heights[row as usize] = device_px(h, scale);
            fixed[row as usize] = true;
        }
        self.fixed = fixed;
        *self.heights.borrow_mut() = RowHeights::from_heights(heights, scale);
    }

    /// Fit the automatic rows among `rows` to their content (those not
    /// measured since the workbook last changed). Returns whether a height
    /// changed.
    pub(crate) fn fit(
        &mut self,
        rows: &[usize],
        book: &Workbook,
        sheet: u32,
        fonts: &mut FontCache,
        window: &mut Window,
        cx: &App,
    ) -> bool {
        let scale = self.heights.borrow().scale();
        let default_dp = device_px(f32::from(look::row_height()), scale);
        let mut changed = false;
        for &r in rows {
            if r >= self.fixed.len() || self.fixed[r] || self.fitted_at[r] == self.generation {
                continue;
            }
            let px = fit_row_height(book, sheet, r as u32, fonts, window, cx);
            let dp = device_px(px, scale).max(default_dp);
            self.fitted[r] = dp;
            self.fitted_at[r] = self.generation;
            changed |= self.heights.borrow_mut().set_dp(r, dp) != 0;
        }
        changed
    }

    /// Rows were inserted (`count > 0`) or deleted at `at`: move the fitted
    /// heights with them, so rows keep their measured heights.
    pub(crate) fn shift(&mut self, at: u32, count: i64) {
        let at = (at as usize).min(self.fitted.len());
        if count > 0 {
            let n = count as usize;
            self.fitted.splice(at..at, std::iter::repeat_n(0, n));
            self.fitted_at.splice(at..at, std::iter::repeat_n(0, n));
        } else {
            let end = (at + (-count) as usize).min(self.fitted.len());
            self.fitted.drain(at..end);
            self.fitted_at.drain(at..end);
        }
    }
}

/// The height (logical px) row `row` needs for its wrapped text and larger
/// fonts, drawn as [`look::Content`] draws them; 0 when nothing in it needs
/// more than a plain row.
pub(crate) fn fit_row_height(
    book: &Workbook,
    sheet: u32,
    row: u32,
    fonts: &mut FontCache,
    window: &mut Window,
    cx: &App,
) -> f32 {
    let cells = book.row_fit_cells(sheet, row);
    if cells.is_empty() {
        return 0.0;
    }
    let default_size = book.default_font_size().max(1) as f32;
    let base = window.text_style();
    let mut tallest: f32 = 0.0;
    for cell in cells.into_iter().take(FIT_MAX_CELLS) {
        let s = &cell.style;
        let size = FONT_BODY * (s.font_size.max(1) as f32 / default_size);
        let line = f32::from(size) * look::LINE_HEIGHT;
        let lines = if s.wrap {
            let width = look::column_width(book, sheet, cell.col, window.scale_factor())
                - SPACE_3 * 2.
                - GRID_LINE;
            let mut run = base.to_run(cell.text.len());
            if let Some(family) = fonts.for_style(s, cx) {
                run.font.family = family;
            }
            if s.bold {
                run.font.weight = FontWeight::BOLD;
            }
            if s.italic {
                run.font.style = FontStyle::Italic;
            }
            window
                .text_system()
                .shape_text(
                    cell.text.into(),
                    size,
                    &[run],
                    Some(width.max(px(1.))),
                    None,
                )
                .map(|lines| {
                    lines
                        .iter()
                        .map(|l| l.wrap_boundaries().len() + 1)
                        .sum::<usize>()
                })
                .unwrap_or(1)
        } else {
            1
        };
        tallest = tallest.max(lines as f32 * line + 2. * FIT_PAD);
    }
    tallest
}

/// A drag on a row number's bottom edge.
#[derive(Clone, Copy)]
pub(crate) struct RowResize {
    row: u32,
    y: Pixels,
    height: f32,
    /// A double-click fitted the row: its second press must not drag.
    fitted: bool,
}

/// The drag payload (GPUI wants a view for the dragged thing).
#[derive(Clone)]
pub(crate) struct ResizeRow;

impl Render for ResizeRow {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

/// Grab room for a row's bottom edge, in the row number cell.
const HANDLE: Pixels = px(4.);

/// The resize grips of row `row`'s number cell: its bottom edge (row `row`)
/// and its top edge (the row above, which may be hidden: dragging it shows
/// it again, as in Excel).
pub(crate) fn resize_handles(row: u32, view: &WeakEntity<SheetsView>) -> Vec<AnyElement> {
    let grip = |target: u32, top: bool| {
        let down = view.clone();
        let moved = view.clone();
        let up = view.clone();
        div()
            .id(("row-resize", target as usize * 2 + top as usize))
            .absolute()
            .left_0()
            .right_0()
            .h(if top { HANDLE - px(1.) } else { HANDLE })
            .map(|d| if top { d.top_0() } else { d.bottom_0() })
            .cursor_row_resize()
            .occlude()
            .on_mouse_down(MouseButton::Left, move |e, window, cx| {
                cx.stop_propagation();
                let _ = down.update(cx, |v, cx| {
                    v.row_resize_down(target, e.position.y, e.click_count, window, cx)
                });
            })
            .on_drag(ResizeRow, |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .on_drag_move(move |e: &DragMoveEvent<ResizeRow>, _, cx| {
                let y = e.event.position.y;
                let _ = moved.update(cx, |v, cx| v.row_resize_move(y, cx));
            })
            .on_mouse_up_out(MouseButton::Left, move |_, window, cx| {
                let _ = up.update(cx, |v, cx| v.row_resize_end(window, cx));
            })
            .into_any_element()
    };
    let mut handles = vec![grip(row, false)];
    if row > 0 {
        handles.push(grip(row - 1, true));
    }
    handles
}

impl SheetsView {
    /// When `row` is hidden, the first row of the run of hidden rows ending
    /// at it.
    fn hidden_run(&self, row: u32, cx: &App) -> Option<u32> {
        let grid = self.table.read(cx).delegate();
        let h = grid.layout.heights.borrow();
        if h.height_dp(row as usize) > 0 || row as usize >= h.len() {
            return None;
        }
        let shown_above = row.checked_sub(1).and_then(|r| h.prev_shown(r as usize));
        Some(shown_above.map_or(0, |r| r as u32 + 1))
    }

    /// Unhide rows `first..=last` (one undo step).
    fn unhide_rows_doc(
        &mut self,
        first: u32,
        last: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let res = grid.book.unhide_rows(sheet, first, last);
            cx.notify();
            res
        });
        self.status = match result {
            Ok(done) if done.shown == 1 => i18n::t("Unhid 1 row").into(),
            Ok(done) if done.shown > 0 => tf("Unhid {} rows", &[&done.shown]).into(),
            Ok(_) => {
                i18n::t("Those rows are hidden by a filter; clear the filter to show them").into()
            }
            Err(err) => format!("{err:#}").into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }

    /// The rows a height command acts on: the selection's rows, the used
    /// part for whole columns or the whole sheet.
    fn height_rows(&self, cx: &App) -> Option<(u32, u32)> {
        let range = self.op_range(cx)?;
        let last = range.bottom.min(range.top + MAX_ROWS_AT_ONCE - 1);
        Some((range.top, last))
    }

    /// A press on a row's bottom edge: start a resize, or auto-fit on a
    /// double-click.
    pub(crate) fn row_resize_down(
        &mut self,
        row: u32,
        y: Pixels,
        clicks: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        // Double-clicking the edge of hidden rows shows them again, as
        // Excel does (dragging it opens the row next to it).
        if clicks >= 2
            && let Some(first) = self.hidden_run(row, cx)
        {
            self.unhide_rows_doc(first, row, window, cx);
            return;
        }
        if clicks >= 2 {
            let rows = self.drag_rows(row, cx);
            self.set_row_heights(rows, None, window, cx);
            self.row_resize = Some(RowResize {
                row,
                y,
                height: 0.,
                fitted: true,
            });
            return;
        }
        let height = {
            let grid = self.table.read(cx).delegate();
            grid.layout.heights.borrow().height(row as usize) as f32
        };
        self.row_resize = Some(RowResize {
            row,
            y,
            height,
            fitted: false,
        });
    }

    pub(crate) fn row_resize_move(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = self.row_resize.filter(|d| !d.fitted) else {
            return;
        };
        let h = (drag.height + f32::from(y - drag.y)).max(MIN_DRAG_PX);
        self.table.update(cx, |table, cx| {
            let layout = &mut table.delegate_mut().layout;
            if layout.preview != Some((drag.row, h)) {
                layout.preview = Some((drag.row, h));
                cx.notify();
            }
        });
    }

    /// The mouse was released: store the dragged height.
    pub(crate) fn row_resize_end(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(drag) = self.row_resize.take() else {
            return;
        };
        let preview = self.table.update(cx, |table, cx| {
            cx.notify();
            table.delegate_mut().layout.preview.take()
        });
        let Some((row, h)) = preview.filter(|_| !drag.fitted) else {
            return;
        };
        let points = {
            let grid = self.table.read(cx).delegate();
            f64::from(h) / px_per_point(&grid.book, grid.sheet)
        };
        // Excel keeps heights to a quarter point.
        let points = (points * 4.0).round().max(1.0) / 4.0;
        let rows = self.drag_rows(row, cx);
        self.set_row_heights(rows, Some(points), window, cx);
    }

    /// The rows a drag (or double-click) on `row`'s edge resizes: the selected
    /// whole rows if `row` is one of them, else `row` alone.
    fn drag_rows(&self, row: u32, cx: &App) -> (u32, u32) {
        let range = self.range(cx);
        if range.is_whole_rows() && (range.top..=range.bottom).contains(&row) {
            (
                range.top,
                range.bottom.min(range.top + MAX_ROWS_AT_ONCE - 1),
            )
        } else {
            (row, row)
        }
    }

    /// Set rows `first..=last` to `points` (`None`: auto-fit them), as one
    /// undo step.
    pub(crate) fn set_row_heights(
        &mut self,
        (first, last): (u32, u32),
        points: Option<f64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let height = match points {
                Some(p) => RowHeight::Points(p),
                None => {
                    // Written as the rows' `ht` when they keep other
                    // formatting: the tallest fit, in points.
                    let k = px_per_point(&grid.book, sheet);
                    let fit = (first..=last.min(first + 1000))
                        .map(|r| fit_row_height(&grid.book, sheet, r, &mut grid.fonts, window, cx))
                        .fold(0.0f32, f32::max);
                    let fit = (f64::from(fit) / k).max(grid.book.default_row_points(sheet));
                    RowHeight::Auto((fit * 4.0).round() / 4.0)
                }
            };
            let res = grid.book.set_row_heights(sheet, first, last, height);
            cx.notify();
            res
        });
        let (a, b) = (first + 1, last + 1);
        self.status = match (result, points) {
            (Ok(true), Some(p)) if a == b => tf("Row height {} pt: row {}", &[&trim(p), &a]).into(),
            (Ok(true), Some(p)) => tf("Row height {} pt: rows {}–{}", &[&trim(p), &a, &b]).into(),
            (Ok(true), None) if a == b => tf("Fitted the height of row {}", &[&a]).into(),
            (Ok(true), None) => tf("Fitted the height of rows {}–{}", &[&a, &b]).into(),
            (Ok(false), _) => i18n::t("Row height unchanged").into(),
            (Err(err), _) => format!("{err:#}").into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }

    /// "Autofit Row Height": the selection's rows follow their content.
    pub fn autofit_rows_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(rows) = self.height_rows(cx) {
            self.set_row_heights(rows, None, window, cx);
        }
    }

    /// "Row Height…": ask for a height in points for the selection's rows.
    pub fn row_height_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let points = {
            let grid = self.table.read(cx).delegate();
            let row = self.selected.0 as usize;
            let px = grid.layout.heights.borrow().height(row);
            px / px_per_point(&grid.book, grid.sheet)
        };
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(i18n::t("Points")));
        input.update(cx, |i, cx| {
            i.set_value(trim((points * 4.0).round() / 4.0), window, cx);
            i.focus(window, cx);
        });
        let sub = cx.subscribe_in(&input, window, Self::on_row_height_input);
        self.row_height_prompt = Some((input, sub));
        cx.notify();
    }

    fn on_row_height_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { .. } => {
                let text = input.read(cx).value().trim().replace(',', ".");
                match text.parse::<f64>() {
                    Ok(p) if p > 0.0 && p <= sheets_engine::MAX_ROW_POINTS => {
                        self.close_row_height_prompt(window, cx);
                        if let Some(rows) = self.height_rows(cx) {
                            self.set_row_heights(rows, Some(p), window, cx);
                        }
                    }
                    _ => {
                        let msg = i18n::t("Row height must be a number of points from 0 to {}");
                        self.status = i18n::fill(msg, &[&sheets_engine::MAX_ROW_POINTS]).into();
                        cx.notify();
                    }
                }
            }
            InputEvent::Blur => self.close_row_height_prompt(window, cx),
            _ => {}
        }
    }

    fn close_row_height_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.row_height_prompt.take().is_some() {
            self.focus_grid(window, cx);
            cx.notify();
        }
    }

    /// The "Row Height…" prompt over the grid, if open.
    pub(crate) fn render_row_height_prompt(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (input, _) = self.row_height_prompt.as_ref()?;
        let t = tokens(cx);
        let rows = self
            .height_rows(cx)
            .map(|(a, b)| {
                if a == b {
                    tf("Row {}", &[&(a + 1)])
                } else {
                    tf("Rows {}–{}", &[&(a + 1), &(b + 1)])
                }
            })
            .unwrap_or_default();
        Some(
            div()
                .absolute()
                .top(CONTROL_LG * 2. + SPACE_5)
                .right(SPACE_6)
                .w(px(260.))
                .p(SPACE_4)
                .rounded(RADIUS_MD)
                .bg(t.surface)
                .border_1()
                .border_color(t.border_strong)
                .shadow_md()
                .on_action(
                    cx.listener(|this, _: &input::Escape, w, cx| {
                        this.close_row_height_prompt(w, cx)
                    }),
                )
                .child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_muted)
                        .mb(SPACE_3)
                        .child(SharedString::from(tf("Row height (pt) · {}", &[&rows]))),
                )
                .child(
                    h_flex()
                        .gap(SPACE_3)
                        .child(div().flex_1().child(Input::new(input).small()))
                        .child(
                            div()
                                .text_size(FONT_SMALL)
                                .text_color(t.fg_dim)
                                .child(i18n::t("Enter to apply")),
                        ),
                )
                .into_any_element(),
        )
    }
}

/// `15`, `15.75`: a number of points without trailing zeros.
fn trim(p: f64) -> String {
    let s = format!("{p:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheets_engine::RowHeight;

    #[std::prelude::v1::test]
    fn heights_follow_the_workbook_and_scale() {
        let mut book = Workbook::new_empty().unwrap();
        book.set_row_heights(0, 2, 2, RowHeight::Points(30.0))
            .unwrap();
        let mut layout = RowLayout::default();
        layout.sync(&book, 0, 1000, 1.25);
        let h = layout.heights.borrow().clone();
        let default = device_px(f32::from(look::row_height()), 1.25);
        assert_eq!(h.height_dp(0), default);
        // 30 pt on a 15 pt default: twice a default row.
        assert_eq!(
            h.height_dp(2),
            device_px(2.0 * f32::from(look::row_height()), 1.25)
        );
        assert_eq!(
            h.top_dp(3),
            u64::from(default) * 2 + u64::from(h.height_dp(2))
        );
        // An edit rebuilds; undo goes back.
        book.undo().unwrap();
        layout.sync(&book, 0, 1000, 1.25);
        assert_eq!(layout.heights.borrow().height_dp(2), default);
        // A drag preview shows at once and goes with the drag.
        layout.preview = Some((5, 100.0));
        layout.sync(&book, 0, 1000, 1.0);
        assert_eq!(layout.heights.borrow().height_dp(5), 100);
        layout.preview = None;
        layout.sync(&book, 0, 1000, 1.0);
        assert_eq!(layout.heights.borrow().height_dp(5), 30);
    }

    #[std::prelude::v1::test]
    fn fitted_heights_move_with_inserted_rows() {
        let mut layout = RowLayout {
            fitted: vec![0, 50, 0, 70],
            fitted_at: vec![0, 1, 0, 1],
            ..Default::default()
        };
        layout.shift(1, 2);
        assert_eq!(layout.fitted, vec![0, 0, 0, 50, 0, 70]);
        layout.shift(0, -3);
        assert_eq!(layout.fitted, vec![50, 0, 70]);
        assert_eq!(layout.fitted_at, vec![1, 0, 1]);
    }

    #[std::prelude::v1::test]
    fn point_counts_print_short() {
        assert_eq!(trim(15.0), "15");
        assert_eq!(trim(15.75), "15.75");
        assert_eq!(trim(12.5), "12.5");
    }
}
