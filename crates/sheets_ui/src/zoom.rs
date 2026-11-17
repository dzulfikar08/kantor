//! Zoom, as in Excel: each sheet has its own (10–400 %), saved in the file
//! (`sheetView@zoomScale`, see [`Workbook::set_zoom`]); Page Break Preview
//! keeps a zoom of its own.
//!
//! The grid draws every length at the zoom: column widths
//! ([`look::column_width`]), row heights ([`crate::row_layout`]), cell text
//! and padding ([`look::Content`]), the column letters and row numbers,
//! and the objects over the cells (built at 100 % and painted scaled, see
//! [`crate::drawings`]). Lines (gridlines, borders, the selection) keep
//! their width, as in Excel. Widths and heights the user sets by dragging
//! are stored at 100 %. Printing doesn't zoom.
//!
//! Controls: the slider, its − and + buttons and the percentage (which
//! opens the Zoom dialog) at the right of the sheet tabs; Ctrl+mouse
//! wheel, Ctrl+= and Ctrl+-; and the palette's Zoom In, Zoom Out, Zoom to
//! 100 % and Zoom to Selection.
//!
//! Changing the zoom rebuilds nothing but the row prefix sums (O(rows), as
//! for a sheet switch) and the visible cells; the scroll position is scaled
//! so the cell at the top left stays there.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    slider::{Slider, SliderEvent, SliderState},
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, tf, tk};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{SHEET_ZOOM_MAX, SHEET_ZOOM_MIN, Workbook};

use crate::{SheetsView, dialog, look};

actions!(sheets_zoom, [ZoomIn, ZoomOut]);

/// Ctrl+= (Ctrl++) and Ctrl+-, on the grid.
pub(crate) fn bind_keys(cx: &mut App) {
    let ctx = Some(crate::GRID_CONTEXT);
    cx.bind_keys(
        [
            kantor_ui::keys::primary_pair("ctrl-=", ZoomIn, ctx),
            kantor_ui::keys::primary_pair("ctrl-+", ZoomIn, ctx),
            kantor_ui::keys::primary_pair("ctrl-shift-=", ZoomIn, ctx),
            kantor_ui::keys::primary_pair("ctrl--", ZoomOut, ctx),
        ]
        .into_iter()
        .flatten(),
    );
}

/// The grid's zoom actions.
pub(crate) fn zoom_actions(d: Div, cx: &mut Context<SheetsView>) -> Div {
    d.on_action(cx.listener(|v, _: &ZoomIn, w, cx| v.zoom_step_doc(true, w, cx)))
        .on_action(cx.listener(|v, _: &ZoomOut, w, cx| v.zoom_step_doc(false, w, cx)))
}

/// The − and + buttons and Ctrl+= / Ctrl+- step by 10 %.
pub(crate) const STEP: u16 = 10;
/// Wheel travel (px) per zoom step, for touchpads' pixel deltas.
const WHEEL_PX_PER_STEP: f32 = 40.0;
/// The slider's positions: 0–100, 100 % in the middle.
const SLIDER_MAX: f32 = 100.0;

/// Where zoom `z` (percent) sits on the slider: 10–100 % on the left half,
/// 100–400 % on the right, as Excel's slider.
pub(crate) fn slider_pos(z: u16) -> f32 {
    let z = f32::from(z.clamp(SHEET_ZOOM_MIN, SHEET_ZOOM_MAX));
    let (min, max) = (f32::from(SHEET_ZOOM_MIN), f32::from(SHEET_ZOOM_MAX));
    let half = SLIDER_MAX / 2.0;
    if z <= 100.0 {
        (z - min) / (100.0 - min) * half
    } else {
        half + (z - 100.0) / (max - 100.0) * half
    }
}

/// The zoom (percent) at slider position `pos` (see [`slider_pos`]).
pub(crate) fn slider_zoom(pos: f32) -> u16 {
    let (min, max) = (f32::from(SHEET_ZOOM_MIN), f32::from(SHEET_ZOOM_MAX));
    let half = SLIDER_MAX / 2.0;
    let pos = pos.clamp(0.0, SLIDER_MAX);
    let z = if pos <= half {
        min + pos / half * (100.0 - min)
    } else {
        100.0 + (pos - half) / half * (max - 100.0)
    };
    (z.round() as u16).clamp(SHEET_ZOOM_MIN, SHEET_ZOOM_MAX)
}

/// The next zoom up or down: the next multiple of [`STEP`].
pub(crate) fn step_zoom(z: u16, up: bool) -> u16 {
    let next = if up {
        (z / STEP + 1) * STEP
    } else {
        z.saturating_sub(1) / STEP * STEP
    };
    next.clamp(SHEET_ZOOM_MIN, SHEET_ZOOM_MAX)
}

/// Zoom to Selection: the largest zoom (percent) at which a block
/// `size` px at 100 % fits in `room`, where the headers take `headers` px at
/// 100 % (they zoom too) out of it.
pub(crate) fn fit_zoom(size: (f32, f32), headers: (f32, f32), room: (f32, f32)) -> u16 {
    let fit = |s: f32, h: f32, r: f32| if s + h > 0.0 { r / (s + h) } else { f32::MAX };
    let k = fit(size.0, headers.0, room.0).min(fit(size.1, headers.1, room.1));
    ((k * 100.0).floor().clamp(0.0, f32::from(SHEET_ZOOM_MAX)) as u16)
        .clamp(SHEET_ZOOM_MIN, SHEET_ZOOM_MAX)
}

/// The Zoom dialog's choices, as Excel's.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ZoomChoice {
    Percent(u16),
    FitSelection,
    Custom,
}

const PRESETS: [u16; 5] = [200, 100, 75, 50, 25];

pub(crate) struct ZoomDialog {
    choice: ZoomChoice,
    custom: Entity<InputState>,
    error: bool,
    _sub: Subscription,
}

/// The view's zoom controls.
#[derive(Default)]
pub(crate) struct ZoomState {
    slider: Option<(Entity<SliderState>, Subscription)>,
    /// Pixel wheel travel not yet turned into a step.
    wheel: Rc<Cell<f32>>,
    pub(crate) dialog: Option<ZoomDialog>,
}

/// A sheet's zoom in percent.
pub(crate) fn percent(book: &Workbook, sheet: u32) -> u16 {
    book.zoom(sheet)
}

impl SheetsView {
    /// The shown sheet's zoom, as a factor.
    pub(crate) fn sheet_zoom(&self, cx: &App) -> f32 {
        self.table.read(cx).delegate().zoom()
    }

    /// The shown sheet's zoom, in percent.
    pub fn zoom_percent(&self, cx: &App) -> u16 {
        let grid = self.table.read(cx).delegate();
        percent(&grid.book, grid.sheet)
    }

    /// Zoom the shown sheet to `percent` (10–400), keeping the cell at the
    /// top left in place.
    pub fn set_zoom_doc(&mut self, percent: u16, window: &mut Window, cx: &mut Context<Self>) {
        if self.chart_tab.is_some() {
            return;
        }
        let changed = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let old = grid.zoom();
            if !grid.book.set_zoom(grid.sheet, percent) {
                return false;
            }
            let new = grid.zoom();
            grid.resize();
            table.refresh(cx);
            table.scale_scroll(new / old, cx);
            true
        });
        let now = self.zoom_percent(cx);
        if changed {
            self.status = tf("Zoom {}%", &[&now]).into();
            self.update_title(window, cx);
        }
        cx.notify();
    }

    /// After a view change that set a new zoom (Page Break Preview): lay
    /// the grid out again, the top left cell staying put.
    pub(crate) fn zoom_changed(&mut self, old: f32, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            let new = table.delegate().zoom();
            table.delegate_mut().resize();
            table.refresh(cx);

            if new != old && old > 0.0 {
                table.scale_scroll(new / old, cx);
            }
        });
    }

    /// Zoom In / Zoom Out: the next 10 %.
    pub fn zoom_step_doc(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        let z = step_zoom(self.zoom_percent(cx), up);
        self.set_zoom_doc(z, window, cx);
    }

    /// Zoom to Selection: the selected cells fill the window, as in Excel.
    pub fn zoom_to_selection_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chart_tab.is_some() {
            return;
        }
        let range = self.range(cx);
        let fit = {
            let table = self.table.read(cx);
            let grid = table.delegate();
            let z = grid.zoom();
            let book = &grid.book;
            let sheet = grid.sheet;
            let right = range.right.min(range.left + 2000);
            let w: f32 = (range.left..=right)
                .map(|c| f32::from(look::column_width(book, sheet, c, grid.scale)))
                .sum::<f32>()
                / z;
            let heights = grid.layout.heights.borrow();
            let n = heights.len();
            let (top, bottom) = (
                (range.top as usize).min(n),
                (range.bottom as usize + 1).min(n),
            );
            let h = (heights.top(bottom) - heights.top(top)) as f32 / z;
            drop(heights);
            let rows_vp = table.vertical_scroll_handle.0.borrow().base_handle.bounds();
            let header = f32::from(look::row_height());
            // The rows' viewport, the pinned rows and the header row.
            let room = (
                f32::from(rows_vp.size.width),
                f32::from(rows_vp.size.height) + f32::from(table.pinned_rows_height) + header * z,
            );
            (room.0 > 0.0 && room.1 > 0.0)
                .then(|| fit_zoom((w, h), (f32::from(ROW_GUTTER_WIDTH), header), room))
        };
        let Some(fit) = fit else { return };
        self.set_zoom_doc(fit, window, cx);
        // The selection at the top left.
        self.table.update(cx, |table, cx| {
            let frozen = table.delegate().book.frozen(table.delegate().sheet);
            if range.top >= frozen.0 {
                table.scroll_to_row(range.top as usize, cx);
            }
            if range.left >= frozen.1 {
                let grid = table.delegate();
                let x: f32 = (frozen.1..range.left)
                    .map(|c| f32::from(look::column_width(&grid.book, grid.sheet, c, grid.scale)))
                    .sum();
                let y = table.horizontal_scroll_handle.offset().y;
                table.horizontal_scroll_handle.set_offset(point(px(-x), y));
            }
            cx.notify();
        });
    }

    /// View ▸ Zoom…: the Zoom dialog.
    pub fn zoom_dialog_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.chart_tab.is_some() || self.blocked(cx) {
            return;
        }
        let now = self.zoom_percent(cx);
        let custom = cx.new(|cx| InputState::new(window, cx));
        custom.update(cx, |i, cx| i.set_value(now.to_string(), window, cx));
        let sub = cx.subscribe_in(
            &custom,
            window,
            |this, _, e: &InputEvent, window, cx| match e {
                InputEvent::Change => {
                    if let Some(d) = this.zoom.dialog.as_mut() {
                        d.choice = ZoomChoice::Custom;
                        d.error = false;
                    }
                    cx.notify();
                }
                InputEvent::PressEnter { .. } => this.apply_zoom_dialog(window, cx),
                _ => {}
            },
        );
        let choice = if PRESETS.contains(&now) {
            ZoomChoice::Percent(now)
        } else {
            ZoomChoice::Custom
        };
        self.zoom.dialog = Some(ZoomDialog {
            choice,
            custom,
            error: false,
            _sub: sub,
        });
        cx.notify();
    }

    fn close_zoom_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.zoom.dialog = None;
        self.focus_grid(window, cx);
        cx.notify();
    }

    fn apply_zoom_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.zoom.dialog.as_mut() else {
            return;
        };
        match d.choice {
            ZoomChoice::Percent(z) => {
                self.close_zoom_dialog(window, cx);
                self.set_zoom_doc(z, window, cx);
            }
            ZoomChoice::FitSelection => {
                self.close_zoom_dialog(window, cx);
                self.zoom_to_selection_doc(window, cx);
            }
            ZoomChoice::Custom => {
                let text = d
                    .custom
                    .read(cx)
                    .value()
                    .trim()
                    .trim_end_matches('%')
                    .to_owned();
                match dialog::parse_number(&text) {
                    Some(z)
                        if (f64::from(SHEET_ZOOM_MIN)..=f64::from(SHEET_ZOOM_MAX)).contains(&z) =>
                    {
                        self.close_zoom_dialog(window, cx);
                        self.set_zoom_doc(z.round() as u16, window, cx);
                    }
                    _ => {
                        d.error = true;
                        cx.notify();
                    }
                }
            }
        }
    }

    /// The Zoom dialog, if open.
    pub(crate) fn render_zoom_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.zoom.dialog.as_ref()?;
        let view = cx.entity().downgrade();
        let mut options: Vec<(ZoomChoice, String)> = PRESETS
            .iter()
            .map(|z| (ZoomChoice::Percent(*z), format!("{z}%")))
            .collect();
        options.push((
            ZoomChoice::FitSelection,
            i18n::t("Fit selection").to_owned(),
        ));
        options.push((ZoomChoice::Custom, i18n::t("Custom").to_owned()));
        let mut list = gpui_kit::component::v_flex().gap(SPACE_2);
        for (i, (choice, label)) in options.into_iter().enumerate() {
            let v = view.clone();
            list = list.child(
                Button::new(("zoom-choice", i))
                    .small()
                    .label(label)
                    .when_else(d.choice == choice, |b| b.primary(), |b| b.ghost())
                    .on_click(move |_, window, cx| {
                        let _ = v.update(cx, |this, cx| {
                            if let Some(d) = this.zoom.dialog.as_mut() {
                                d.choice = choice;
                                d.error = false;
                            }
                            if choice == ZoomChoice::Custom
                                && let Some(d) = this.zoom.dialog.as_ref()
                            {
                                d.custom.update(cx, |i, cx| i.focus(window, cx));
                            }
                            cx.notify();
                        });
                    }),
            );
        }
        let custom = h_flex()
            .gap(SPACE_3)
            .child(div().w(px(80.)).child(Input::new(&d.custom).small()))
            .child("%");
        let card = dialog::card("zoom-dialog", i18n::t("Zoom"), px(260.), cx)
            .on_action(
                cx.listener(|this, _: &gpui_kit::component::input::Escape, w, cx| {
                    this.close_zoom_dialog(w, cx)
                }),
            )
            .child(list)
            .child(dialog::row(i18n::t("Custom"), custom, cx))
            .when(d.error, |c| {
                c.child(dialog::note(
                    tf(
                        "Type a zoom from {}% to {}%",
                        &[&SHEET_ZOOM_MIN, &SHEET_ZOOM_MAX],
                    ),
                    true,
                    cx,
                ))
            })
            .child(dialog::buttons(
                "zoom-buttons",
                tk("OK"),
                true,
                &view,
                Self::apply_zoom_dialog,
                Self::close_zoom_dialog,
            ));
        Some(dialog::place(card).into_any_element())
    }

    /// The zoom slider, its − and + buttons and the percentage, at the
    /// right of the sheet tabs (as in Excel's status bar).
    pub(crate) fn render_zoom_control(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use gpui_kit::assets::IconName as Lucide;
        let t = tokens(cx);
        let now = self.zoom_percent(cx);
        let slider = match &self.zoom.slider {
            Some((s, _)) => s.clone(),
            None => {
                let s = cx.new(|_| {
                    SliderState::new()
                        .min(0.0)
                        .max(SLIDER_MAX)
                        .step(0.5)
                        .default_value(slider_pos(now))
                });
                let sub = cx.subscribe_in(&s, window, |this, _, e: &SliderEvent, window, cx| {
                    let (SliderEvent::Change(v) | SliderEvent::Release(v)) = e;
                    let z = slider_zoom(v.end());
                    // Close to 100 %: 100 %, so the middle is easy to hit.
                    let z = if (95..=105).contains(&z) { 100 } else { z };
                    if z != this.zoom_percent(cx) {
                        this.set_zoom_doc(z, window, cx);
                    }
                });
                self.zoom.slider = Some((s.clone(), sub));
                s
            }
        };
        // The zoom changed elsewhere (keys, sheet switch, opening): move
        // the thumb.
        if slider_zoom(slider.read(cx).value().end()) != now {
            let pos = slider_pos(now);
            slider.update(cx, |s, cx| s.set_value(pos, window, cx));
        }
        let button = |id: &'static str, icon: Lucide, label: &'static str, up: bool| {
            kantor_ui::components::icon_button(id, icon, false, cx)
                .xsmall()
                .tooltip(i18n::t(label))
                .on_click(
                    cx.listener(move |this, _, window, cx| this.zoom_step_doc(up, window, cx)),
                )
        };
        h_flex()
            .flex_none()
            .gap_1()
            .ml(SPACE_4)
            .child(button("zoom-out", Lucide::Minus, tk("Zoom Out"), false))
            .child(
                div()
                    .w(px(96.))
                    .child(Slider::new(&slider).accessibility_label(i18n::t("Zoom"))),
            )
            .child(button("zoom-in", Lucide::Plus, tk("Zoom In"), true))
            .child(
                div()
                    .id("zoom-percent")
                    .flex_none()
                    .w(px(44.))
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .text_right()
                    .cursor_pointer()
                    .hover(|s| s.text_color(t.fg))
                    .tooltip(|window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(i18n::t("Zoom…"))
                            .build(window, cx)
                    })
                    .on_click(cx.listener(|this, _, window, cx| this.zoom_dialog_doc(window, cx)))
                    .child(SharedString::from(format!("{now}%"))),
            )
            .into_any_element()
    }

    /// Ctrl+mouse wheel over the grid zooms (the grid doesn't scroll).
    pub(crate) fn render_wheel_zoom(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        let wheel = self.zoom.wheel.clone();
        canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let view = view.clone();
                let wheel = wheel.clone();
                window.on_mouse_event(move |e: &ScrollEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture
                        || !(e.modifiers.control || e.modifiers.platform)
                        || !bounds.contains(&e.position)
                    {
                        return;
                    }
                    cx.stop_propagation();
                    let dy = match e.delta {
                        ScrollDelta::Lines(l) => l.y * WHEEL_PX_PER_STEP,
                        ScrollDelta::Pixels(p) => f32::from(p.y),
                    };
                    let total = wheel.get() + dy;
                    let steps = (total / WHEEL_PX_PER_STEP).trunc();
                    wheel.set(total - steps * WHEEL_PX_PER_STEP);
                    if steps == 0.0 {
                        return;
                    }
                    let _ = view.update(cx, |this, cx| {
                        let mut z = this.zoom_percent(cx);
                        for _ in 0..(steps.abs() as usize).min(10) {
                            z = step_zoom(z, steps > 0.0);
                        }
                        this.set_zoom_doc(z, window, cx);
                    });
                });
            },
        )
        .absolute()
        .inset_0()
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn slider_puts_100_in_the_middle() {
        assert_eq!(slider_pos(100), 50.0);
        assert_eq!(slider_pos(10), 0.0);
        assert_eq!(slider_pos(400), 100.0);
        for z in [10, 25, 50, 75, 100, 150, 200, 333, 400] {
            assert_eq!(slider_zoom(slider_pos(z)), z, "{z}");
        }
        assert_eq!(slider_zoom(-5.0), 10);
        assert_eq!(slider_zoom(500.0), 400);
    }

    #[std::prelude::v1::test]
    fn steps_go_by_tens_within_limits() {
        assert_eq!(step_zoom(100, true), 110);
        assert_eq!(step_zoom(100, false), 90);
        assert_eq!(step_zoom(85, true), 90);
        assert_eq!(step_zoom(85, false), 80);
        assert_eq!(step_zoom(10, false), 10);
        assert_eq!(step_zoom(400, true), 400);
        assert_eq!(step_zoom(395, true), 400);
    }

    /// The grid's geometry at `percent` (scale 1): column B is 100 px wide
    /// at 100 %, row 3 is 30 pt tall.
    fn grid_at(percent: u16) -> crate::SheetGrid {
        let mut book = Workbook::new_empty().unwrap();
        book.set_column_width(0, 1, 1, 100.0).unwrap();
        book.set_row_heights(0, 2, 2, sheets_engine::RowHeight::Points(30.0))
            .unwrap();
        book.set_zoom(0, percent);
        let mut grid = crate::SheetGrid::new(book, WeakEntity::new_invalid());
        grid.scale = 1.0;
        let rows = grid.rows;
        grid.layout.sync(&grid.book, 0, rows, 1.0);
        grid
    }

    #[std::prelude::v1::test]
    fn hit_testing_follows_the_zoom() {
        use crate::drawings::GridGeometry;
        use sheets_engine::drawing::{Anchor, EditAs, Marker, SheetGeometry as _};
        let anchor = Anchor::TwoCell {
            from: Marker {
                col: 1,
                col_off: 9525 * 10,
                row: 1,
                row_off: 9525 * 8,
            },
            to: Marker {
                col: 3,
                col_off: 0,
                row: 4,
                row_off: 0,
            },
            edit_as: EditAs::TwoCell,
        };
        let base = grid_at(100);
        let base_rows = base.layout.heights.borrow().clone();
        let at_100 = GridGeometry::new(&base, None, &base_rows, 8);
        let rect_100 = anchor.rect(&at_100);
        for (percent, k) in [(50u16, 0.5f64), (200, 2.0)] {
            let grid = grid_at(percent);
            assert_eq!(grid.zoom() as f64, k);
            let rows = grid.layout.heights.borrow().clone();
            let geom = GridGeometry::new(&grid, None, &rows, 8);
            // Column B: from A's zoomed width, 100 px × k wide.
            let a = geom.col_left(1);
            let b = geom.col_width(1);
            assert!((a - at_100.col_left(1) * k).abs() <= 1.0, "{percent}: {a}");
            assert!((b - 100.0 * k).abs() <= 1.0, "{percent}: {b}");
            assert_eq!(geom.col_at(a + 1.0), 1);
            assert_eq!(geom.col_at(a + b - 1.0), 1);
            assert_eq!(geom.col_at(a + b + 1.0), 2);
            // Row 3 (30 pt): twice a default row's 15 pt, at the zoom.
            let default = f64::from(f32::from(look::row_height())) * k;
            assert!((geom.row_height(0) - default).abs() <= 1.0, "{percent}");
            assert!(
                (geom.row_height(2) - 2.0 * default).abs() <= 1.0,
                "{percent}"
            );
            let y = geom.row_top(2);
            assert_eq!(geom.row_at(y + 1.0), 2);
            assert_eq!(geom.row_at(y + geom.row_height(2) - 1.0), 2);
            assert_eq!(geom.row_at(y + geom.row_height(2) + 1.0), 3);
            // An object is placed (and dragged back to its anchor) at the zoom.
            let rect = anchor.rect(&geom);
            for i in 0..4 {
                assert!(
                    (rect[i] - rect_100[i] * k).abs() <= 2.0,
                    "{percent}: {rect:?} vs {rect_100:?}"
                );
            }
            let back = anchor.with_rect(rect, &geom);
            assert_eq!(back.rect(&geom), rect);
            // A dragged row height is stored in points at 100 %.
            let ppp = crate::row_layout::px_per_point(&grid.book, 0);
            let ppp_100 = crate::row_layout::px_per_point(&base.book, 0);
            assert!((ppp - ppp_100 * k).abs() < 1e-9);
        }
    }

    #[std::prelude::v1::test]
    fn fit_selection_counts_the_zoomed_headers() {
        // 400 × 200 px of cells with 52 × 22 px of headers in 904 × 444.
        assert_eq!(fit_zoom((400.0, 200.0), (52.0, 22.0), (904.0, 444.0)), 200);
        // A single small cell: Excel's largest zoom.
        assert_eq!(fit_zoom((10.0, 10.0), (52.0, 22.0), (900.0, 600.0)), 400);
        // A huge range: the smallest.
        assert_eq!(fit_zoom((1e6, 1e6), (52.0, 22.0), (900.0, 600.0)), 10);
    }
}
