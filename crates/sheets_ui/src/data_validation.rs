//! Data validation in the grid, as Excel shows it in Kantor's look.
//!
//! * A list validation puts a drop-down button beside the active cell (only
//!   that one, as Excel does; none for `showDropDown="1"`). The button or
//!   Alt+Down opens a [`PopupList`] of the source's items, evaluated when it
//!   opens; picking one is an ordinary, undoable edit.
//! * A rule's input message shows as a small card under the selected cell.
//! * A value typed into the cell is stored, then checked
//!   ([`sheets_engine::Workbook::validate_cell`]). If it fails and the rule
//!   has an error alert, the alert asks what to do: Stop (Retry / Cancel),
//!   Warning ("Continue?" Yes / No / Cancel) or Information (OK / Cancel).
//!   Retry and No take the value back and edit it again, Cancel takes it
//!   back ([`sheets_engine::Workbook::revert_edit`]: no redo is left).
//!   Pasting and filling are not checked, as in Excel.
//! * "Circle Invalid Data" rings the cells whose values break their rule
//!   (until "Clear Validation Circles"); a corrected cell loses its ring.
//!
//! The button, the list and the message are drawn from the active cell
//! (`render_td`) as deferred overlays, so they follow it while scrolling.
//! The list is not an `Input` (see the gpui-input-overlays note): it takes
//! focus itself and filters on typed characters. The alert is drawn over the
//! whole pane, from the view.

use gpui_kit::component::{Icon, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::metrics::*;
use kantor_ui::tokens;
use kantor_ui::tokens::Elevation;
use sheets_engine::{ErrorStyle, ListItem, cell_name};

use crate::popup_list::{PopupEvent, PopupList};
use crate::table::TableState;
use crate::{GRID_CONTEXT, SheetGrid, SheetsView};

use gpui_kit::assets::IconName as Lucide;

actions!(
    sheets,
    [
        OpenValidationList,
        AlertAccept,
        AlertCancel,
        AlertNext,
        AlertPrev
    ]
);

const ALERT_CONTEXT: &str = "KantorSheetsAlert";
/// Excel's message when a rule's alert has no text of its own.
const DEFAULT_ERROR: &str =
    "This value doesn't match the data validation restrictions defined for this cell.";

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-down", OpenValidationList, Some(GRID_CONTEXT)),
        KeyBinding::new("enter", AlertAccept, Some(ALERT_CONTEXT)),
        KeyBinding::new("space", AlertAccept, Some(ALERT_CONTEXT)),
        KeyBinding::new("escape", AlertCancel, Some(ALERT_CONTEXT)),
        KeyBinding::new("tab", AlertNext, Some(ALERT_CONTEXT)),
        KeyBinding::new("right", AlertNext, Some(ALERT_CONTEXT)),
        KeyBinding::new("shift-tab", AlertPrev, Some(ALERT_CONTEXT)),
        KeyBinding::new("left", AlertPrev, Some(ALERT_CONTEXT)),
    ]);
    crate::popup_list::bind_keys(cx);
}

/// What the grid draws for data validation.
#[derive(Default)]
pub(crate) struct GridValidation {
    /// The open drop-down: its cell `(sheet, row, col)`, items and list.
    list: Option<OpenList>,
    /// "Circle Invalid Data" is on.
    pub(crate) circles: bool,
    /// An error alert is up: no input message over it.
    alert: bool,
}

struct OpenList {
    cell: (u32, u32, u32),
    items: Vec<ListItem>,
    list: PopupList,
}

/// What an alert button does with the typed value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Keep it (Warning's Yes, Information's OK).
    Keep,
    /// Take it back and edit it again (Retry, No).
    Retry,
    /// Take it back (Cancel).
    Discard,
}

/// An error alert waiting for an answer.
pub(crate) struct Alert {
    sheet: u32,
    cell: (u32, u32),
    /// What was typed (Retry edits it again).
    value: String,
    /// Where the commit would have moved the active cell.
    step: Option<(i64, i64)>,
    style: ErrorStyle,
    title: SharedString,
    text: SharedString,
    /// The highlighted button.
    choice: usize,
    focus: FocusHandle,
}

impl Alert {
    fn buttons(&self) -> &'static [(&'static str, Outcome)] {
        match self.style {
            ErrorStyle::Stop => &[("Retry", Outcome::Retry), ("Cancel", Outcome::Discard)],
            ErrorStyle::Warning => &[
                ("Yes", Outcome::Keep),
                ("No", Outcome::Retry),
                ("Cancel", Outcome::Discard),
            ],
            ErrorStyle::Information => &[("OK", Outcome::Keep), ("Cancel", Outcome::Discard)],
        }
    }

    /// The button Enter presses first: Retry, No (as Excel), OK.
    fn default_choice(style: ErrorStyle) -> usize {
        match style {
            ErrorStyle::Warning => 1,
            _ => 0,
        }
    }
}

// ------------------------------------------------------------------ grid

impl SheetGrid {
    /// Validation overlays of cell (`row`, `col`): its ring when circling
    /// invalid data and, for the active cell, the drop-down button, the
    /// open list and the input message.
    pub(crate) fn validation_layers(
        &mut self,
        row: u32,
        col: u32,
        row_height: Pixels,
        cx: &mut Context<TableState<Self>>,
    ) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let sheet = self.sheet;
        let t = tokens(cx);
        if self.validation.circles && self.book.invalid_cell(sheet, row, col) {
            out.push(
                div()
                    .absolute()
                    .inset_0()
                    .m(px(1.))
                    .rounded_full()
                    .border_2()
                    .border_color(t.danger)
                    .into_any_element(),
            );
        }
        if (row, col) != self.anchor || self.editor.is_some() || self.validation.alert {
            return out;
        }
        let Some(v) = self.book.validation(sheet, row, col) else {
            return out;
        };
        let open = self
            .validation
            .list
            .as_ref()
            .filter(|l| l.cell == (sheet, row, col));
        if v.kind == sheets_engine::ValidationType::List && v.dropdown {
            let view = self.view.clone();
            let size = row_height.max(px(16.)).min(CONTROL_MD);
            let button = div()
                .id("validation-dropdown")
                .occlude()
                .size(size)
                .flex()
                .items_center()
                .justify_center()
                .bg(if open.is_some() {
                    t.selected
                } else {
                    t.surface
                })
                .border_1()
                .border_color(t.border_strong)
                .rounded_r(RADIUS_SM)
                .text_color(t.fg_muted)
                .hover(|s| s.bg(t.hover).text_color(t.fg))
                .cursor_pointer()
                .child(Icon::new(Lucide::ChevronDown).with_size(ICON_SM))
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    let _ = view.update(cx, |view, cx| view.toggle_validation_list(window, cx));
                });
            // At the cell's top-right corner, outside it.
            out.push(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .size_0()
                    .child(deferred(anchored().child(button)).with_priority(1))
                    .into_any_element(),
            );
        }
        if let Some(open) = open {
            let view = self.view.clone();
            let list = open.list.render(
                px(140.),
                move |ev, window, cx| {
                    let _ = view.update(cx, |view, cx| view.on_list_event(ev, window, cx));
                },
                cx,
            );
            out.push(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .size_0()
                    .child(
                        deferred(anchored().offset(point(px(0.), SPACE_1)).child(list))
                            .with_priority(2),
                    )
                    .into_any_element(),
            );
        } else if let Some(prompt) = &v.prompt {
            let card = v_flex()
                .max_w(px(260.))
                .px(SPACE_5)
                .py(SPACE_4)
                .gap(SPACE_1)
                .bg(t.elevated)
                .border_1()
                .border_color(t.border)
                .rounded(RADIUS_MD)
                .shadow(t.shadow(Elevation::Popover))
                .text_size(FONT_SMALL)
                .when(!prompt.title.is_empty(), |d| {
                    d.child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.fg)
                            .child(SharedString::from(prompt.title.clone())),
                    )
                })
                .when(!prompt.text.is_empty(), |d| {
                    d.child(
                        div()
                            .text_color(t.fg_muted)
                            .child(SharedString::from(prompt.text.clone())),
                    )
                });
            out.push(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .size_0()
                    .child(deferred(
                        anchored().offset(point(SPACE_7, SPACE_3)).child(card),
                    ))
                    .into_any_element(),
            );
        }
        out
    }
}

// ------------------------------------------------------------------ view

impl SheetsView {
    pub(crate) fn validation_actions(&self, d: Div, cx: &mut Context<Self>) -> Div {
        d.on_action(cx.listener(|this, _: &OpenValidationList, window, cx| {
            this.open_validation_list(window, cx)
        }))
    }

    fn toggle_validation_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.table.read(cx).delegate().validation.list.is_some() {
            self.close_validation_list(window, cx);
            self.focus_grid(window, cx);
        } else {
            self.open_validation_list(window, cx);
        }
    }

    /// Alt+Down or the button: open the active cell's list.
    pub(crate) fn open_validation_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (row, col) = self.selected;
        let found = self.table.update(cx, |table, _| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let v = grid.book.validation(sheet, row, col)?;
            if !v.dropdown {
                return None;
            }
            let items = grid.book.validation_list(sheet, row, col)?;
            let current = grid.book.display_value(sheet, row, col);
            Some((sheet, items, current))
        });
        let Some((sheet, items, current)) = found else {
            return;
        };
        if items.is_empty() {
            self.status = "The list of this cell is empty".into();
            cx.notify();
            return;
        }
        let at = items
            .iter()
            .position(|i| i.text.eq_ignore_ascii_case(&current));
        let texts = items
            .iter()
            .map(|i| SharedString::from(i.text.clone()))
            .collect();
        let list = PopupList::new(texts, at, cx);
        let handle = list.focus_handle().clone();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().validation.list = Some(OpenList {
                cell: (sheet, row, col),
                items,
                list,
            });
            cx.notify();
        });
        window.focus(&handle, cx);
        // Clicking elsewhere (or the cell scrolling away) closes it.
        self.validation_list_sub = Some(cx.on_focus_out(&handle, window, |this, _, window, cx| {
            this.close_validation_list(window, cx);
        }));
        cx.notify();
    }

    pub(crate) fn close_validation_list(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.validation_list_sub = None;
        self.table.update(cx, |table, cx| {
            if table.delegate_mut().validation.list.take().is_some() {
                cx.notify();
            }
        });
    }

    fn on_list_event(&mut self, ev: PopupEvent, window: &mut Window, cx: &mut Context<Self>) {
        let pick = match ev {
            PopupEvent::Pick(ix) => Some(ix),
            PopupEvent::Confirm => self
                .table
                .read(cx)
                .delegate()
                .validation
                .list
                .as_ref()
                .and_then(|l| l.list.highlighted()),
            PopupEvent::Dismiss => {
                self.close_validation_list(window, cx);
                self.focus_grid(window, cx);
                return;
            }
            ev => {
                self.table.update(cx, |table, cx| {
                    if let Some(open) = table.delegate_mut().validation.list.as_mut() {
                        open.list.apply(&ev);
                        cx.notify();
                    }
                });
                return;
            }
        };
        let chosen = self
            .table
            .read(cx)
            .delegate()
            .validation
            .list
            .as_ref()
            .and_then(|l| {
                let item = l.items.get(pick?)?;
                Some((l.cell, item.input.clone()))
            });
        self.close_validation_list(window, cx);
        self.focus_grid(window, cx);
        let Some(((sheet, row, col), input)) = chosen else {
            return;
        };
        if self.grid_sheet(cx) != sheet {
            return;
        }
        let unchanged = self.table.read(cx).delegate().book.input(sheet, row, col) == input;
        if !unchanged {
            self.status = match self.set_cell(sheet, row, col, &input, cx) {
                Ok(()) => format!("{} updated", cell_name(row, col)).into(),
                Err(e) => format!("{e:#}").into(),
            };
        }
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        cx.notify();
    }

    /// After a typed value was stored in (`sheet`, `row`, `col`): if it
    /// breaks the cell's validation and the rule has an error alert, show
    /// it and return `true` (the commit waits for the answer).
    pub(crate) fn check_typed_value(
        &mut self,
        sheet: u32,
        (row, col): (u32, u32),
        value: &str,
        step: Option<(i64, i64)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let failed = self.table.update(cx, |table, _| {
            table.delegate_mut().book.validate_cell(sheet, row, col)
        });
        let Some((style, msg)) = failed.and_then(|v| v.error.clone()) else {
            return false;
        };
        // The cell stays the active one while the alert is up.
        if self.grid_sheet(cx) != sheet {
            self.show_sheet(sheet, cx);
        }
        if self.selected != (row, col) {
            self.selected = (row, col);
            self.select_cell(row, col, cx);
        }
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.set_alert_shown(true, cx);
        self.status = format!("{} breaks its data validation", cell_name(row, col)).into();
        self.alert = Some(Alert {
            sheet,
            cell: (row, col),
            value: value.to_owned(),
            step,
            style,
            title: if msg.title.is_empty() {
                "Invalid value".into()
            } else {
                msg.title.into()
            },
            text: if msg.text.is_empty() {
                DEFAULT_ERROR.into()
            } else {
                msg.text.into()
            },
            choice: Alert::default_choice(style),
            focus,
        });
        cx.notify();
        true
    }

    fn answer_alert(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        let Some(alert) = self.alert.take() else {
            return;
        };
        self.set_alert_shown(false, cx);
        let (row, col) = alert.cell;
        match outcome {
            Outcome::Keep => {
                self.status = format!("{} updated", cell_name(row, col)).into();
                self.finish_commit(alert.sheet, alert.cell, alert.step, window, cx);
                return;
            }
            Outcome::Retry | Outcome::Discard => {
                let reverted = self.table.update(cx, |table, cx| {
                    let grid = table.delegate_mut();
                    let res = grid.book.revert_edit();
                    grid.resize();
                    table.refresh(cx);
                    res
                });
                if let Err(e) = reverted {
                    self.status = format!("{e:#}").into();
                }
            }
        }
        self.selected = (row, col);
        self.select_cell(row, col, cx);
        if outcome == Outcome::Retry {
            self.start_edit(Some(alert.value), window, cx);
            self.status = "Edit the value, or press Escape to cancel".into();
        } else {
            self.load_selected_into_formula(window, cx);
            self.focus_grid(window, cx);
            self.status = format!("{} not changed", cell_name(row, col)).into();
        }
        self.update_title(window, cx);
        cx.notify();
    }

    fn set_alert_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().validation.alert = shown;
            cx.notify();
        });
    }

    fn alert_press(&mut self, choice: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(alert) = &self.alert else { return };
        let ix = choice.unwrap_or(alert.choice);
        if let Some(&(_, outcome)) = alert.buttons().get(ix) {
            self.answer_alert(outcome, window, cx);
        }
    }

    fn alert_move(&mut self, d: isize, cx: &mut Context<Self>) {
        if let Some(alert) = &mut self.alert {
            let n = alert.buttons().len() as isize;
            alert.choice = (alert.choice as isize + d).rem_euclid(n) as usize;
            cx.notify();
        }
    }

    /// The palette's "Pick From Validation List" (Alt+Down in the grid).
    pub fn open_validation_list_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_validation_list(window, cx);
        if self.table.read(cx).delegate().validation.list.is_none() && self.alert.is_none() {
            self.status = "The active cell has no drop-down list".into();
            cx.notify();
        }
    }

    /// "Circle Invalid Data" (`on`) / "Clear Validation Circles".
    pub fn circle_invalid_doc(&mut self, on: bool, _: &mut Window, cx: &mut Context<Self>) {
        let has = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            grid.validation.circles = on;
            let sheet = grid.sheet;
            let has = grid.book.has_validations(sheet);
            table.refresh(cx);
            cx.notify();
            has
        });
        self.status = match (on, has) {
            (true, true) => "Invalid data circled".into(),
            (true, false) => "This sheet has no data validation".into(),
            (false, _) => "Validation circles cleared".into(),
        };
        cx.notify();
    }

    /// The error alert, over the whole pane.
    pub(crate) fn render_alert(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let alert = self.alert.as_ref()?;
        let t = tokens(cx);
        let (icon, color) = match alert.style {
            ErrorStyle::Stop => (Lucide::CircleX, t.danger),
            ErrorStyle::Warning => (Lucide::TriangleAlert, t.warning),
            ErrorStyle::Information => (Lucide::Info, t.info),
        };
        let buttons = alert.buttons().iter().enumerate().map(|(ix, (label, _))| {
            let on = ix == alert.choice;
            div()
                .id(("validation-alert-button", ix))
                .h(CONTROL_MD)
                .min_w(px(72.))
                .px(SPACE_6)
                .flex()
                .items_center()
                .justify_center()
                .rounded(RADIUS_MD)
                .text_size(FONT_SMALL)
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .map(|d| {
                    if on {
                        d.bg(t.accent).text_color(t.accent_fg)
                    } else {
                        d.bg(t.surface)
                            .border_1()
                            .border_color(t.border_strong)
                            .text_color(t.fg)
                            .hover(|s| s.bg(t.hover))
                    }
                })
                .child(*label)
                .on_click(
                    cx.listener(move |this, _, window, cx| this.alert_press(Some(ix), window, cx)),
                )
        });
        let text = v_flex()
            .flex_1()
            .min_w_0()
            .gap(SPACE_3)
            .child(
                div()
                    .text_size(FONT_EMPHASIS)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.fg)
                    .child(alert.title.clone()),
            )
            .child(div().text_color(t.fg_muted).child(alert.text.clone()))
            .when(alert.style == ErrorStyle::Warning, |d| {
                d.child(div().text_color(t.fg).child("Continue?"))
            });
        let card = v_flex()
            .id("validation-alert")
            .track_focus(&alert.focus)
            .key_context(ALERT_CONTEXT)
            .on_action(cx.listener(|this, _: &AlertAccept, w, cx| this.alert_press(None, w, cx)))
            .on_action(cx.listener(|this, _: &AlertCancel, w, cx| {
                this.answer_alert(Outcome::Discard, w, cx)
            }))
            .on_action(cx.listener(|this, _: &AlertNext, _, cx| this.alert_move(1, cx)))
            .on_action(cx.listener(|this, _: &AlertPrev, _, cx| this.alert_move(-1, cx)))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                // The buttons' initials, as in a system dialog.
                let Some(alert) = &this.alert else { return };
                let key = e.keystroke.key.to_ascii_lowercase();
                let hit = alert.buttons().iter().position(|(label, _)| {
                    label
                        .chars()
                        .next()
                        .map(|c| c.to_ascii_lowercase().to_string())
                        == Some(key.clone())
                });
                if let Some(ix) = hit {
                    cx.stop_propagation();
                    this.alert_press(Some(ix), window, cx);
                }
            }))
            .w(px(400.))
            .p(SPACE_7)
            .gap(SPACE_7)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .rounded(RADIUS_LG)
            .shadow(t.shadow(Elevation::Modal))
            .text_size(FONT_SMALL)
            .child(
                h_flex()
                    .items_start()
                    .gap(SPACE_6)
                    .child(
                        div()
                            .flex_none()
                            .text_color(color)
                            .child(Icon::new(icon).with_size(px(22.))),
                    )
                    .child(text),
            )
            .child(h_flex().justify_end().gap(SPACE_4).children(buttons));
        Some(
            div()
                .id("validation-alert-scrim")
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(t.scrim)
                .child(card)
                .into_any_element(),
        )
    }
}
