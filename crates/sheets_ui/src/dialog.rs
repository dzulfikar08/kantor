//! Pieces shared by the Sheets dialogs (Page Setup, Format as Table, Data
//! Validation, Conditional Formatting, Insert Chart): a card over the grid,
//! labelled rows, segmented choices and small reference parsers.
//!
//! Like the Insert PivotTable card, dialogs are siblings of the grid, never
//! inside it (the grid's key handler would take the inputs' typing).

use gpui_kit::component::{
    Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, input, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::{metrics::*, tokens};

use std::cell::Cell;

use crate::SheetsView;

/// A dialog card, centred near the top of the view. `body` goes between the
/// title and the buttons row (`footer`).
pub(crate) fn card(
    id: &'static str,
    title: impl Into<SharedString>,
    width: Pixels,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    v_flex()
        .id(id)
        .occlude()
        .w(width)
        .max_h(px(640.))
        .gap(SPACE_5)
        .p(SPACE_6)
        .rounded(RADIUS_MD)
        .bg(t.surface)
        .border_1()
        .border_color(t.border_strong)
        .shadow_md()
        .text_size(FONT_BODY)
        .text_color(t.fg)
        .child(
            div()
                .flex_none()
                .text_size(FONT_EMPHASIS)
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.into()),
        )
}

/// The focus of the dialog card shown, and whether it has taken the focus
/// since it opened ([`place`]).
pub(crate) struct CardFocus {
    handle: FocusHandle,
    /// A card was placed in the frame being built.
    placed: Cell<bool>,
    /// 0: just opened; 1: shown one frame (fields it focused are in the
    /// tree now); 2: focus settled.
    stage: Cell<u8>,
}

impl CardFocus {
    pub(crate) fn new(handle: FocusHandle) -> Self {
        Self {
            handle,
            placed: Cell::new(false),
            stage: Cell::new(0),
        }
    }

    /// Called as the view renders: a card that isn't shown any more starts
    /// over when it opens again.
    pub(crate) fn begin_frame(&self) {
        if !self.placed.replace(false) {
            self.stage.set(0);
        }
    }
}

/// Where a dialog card sits: centred horizontally, below the toolbar and
/// formula bar, over a backdrop that keeps the sheet around it from taking
/// hover and clicks while it is open (modal, as Excel's dialogs).
///
/// Like Excel's dialogs, the card takes the focus when it opens, unless the
/// dialog put it in one of its own fields, so Escape reaches the card's
/// `input::Escape` handler even when no field in it is focused. It does so
/// once: what is focused later (a field, a menu, the palette) keeps it.
pub(crate) fn place(card: Stateful<Div>, view: &SheetsView, _cx: &App) -> Div {
    let state = view.dialog_focus.clone();
    state.placed.set(true);
    let own = state.handle.clone();
    let card = card
        .track_focus(&state.handle)
        .on_key_down(move |e, window, cx| {
            if e.keystroke.key == "escape" && own.is_focused(window) {
                cx.stop_propagation();
                window.dispatch_action(Box::new(input::Escape), cx);
            }
        })
        .child(
            canvas(
                move |_, window, cx| match state.stage.get() {
                    0 => {
                        // Let fields the dialog focused show up first.
                        state.stage.set(1);
                        window.refresh();
                    }
                    1 => {
                        state.stage.set(2);
                        if !state.handle.contains_focused(window, cx) {
                            window.focus(&state.handle, cx);
                        }
                    }
                    _ => {}
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_0(),
        );
    kantor_ui::overlay::backdrop().child(
        h_flex()
            .absolute()
            .top(CONTROL_LG * 2. + SPACE_5)
            .left_0()
            .right_0()
            .justify_center()
            .child(card),
    )
}

/// `label` on the left (fixed width), the control filling the rest.
pub(crate) fn row(label: impl Into<SharedString>, control: impl IntoElement, cx: &App) -> Div {
    let t = tokens(cx);
    h_flex()
        .gap(SPACE_4)
        .min_h(CONTROL_MD)
        .child(
            div()
                .flex_none()
                .w(px(118.))
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(label.into()),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// A muted (or, for errors, red) line of small text.
pub(crate) fn note(text: impl Into<SharedString>, error: bool, cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .text_size(FONT_SMALL)
        .text_color(if error { t.danger } else { t.fg_muted })
        .child(text.into())
}

pub(crate) type SetFn<T> = fn(&mut SheetsView, T);

/// A row of buttons, one per option, the current one filled.
pub(crate) fn choices<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: &[(T, &'static str)],
    current: T,
    view: &WeakEntity<SheetsView>,
    set: SetFn<T>,
) -> Div {
    let mut row = h_flex().gap(SPACE_2).flex_wrap();
    for (i, (value, label)) in options.iter().enumerate() {
        let v = view.clone();
        let value = *value;
        row = row.child(
            Button::new((id, i))
                .small()
                .label(*label)
                .when_else(value == current, |b| b.primary(), |b| b.outline())
                .on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| {
                        set(this, value);
                        cx.notify();
                    });
                }),
        );
    }
    row
}

/// The OK / Cancel row.
pub(crate) fn buttons(
    id: &'static str,
    ok_label: &'static str,
    ok_enabled: bool,
    view: &WeakEntity<SheetsView>,
    ok: fn(&mut SheetsView, &mut Window, &mut Context<SheetsView>),
    cancel: fn(&mut SheetsView, &mut Window, &mut Context<SheetsView>),
) -> Div {
    use gpui_kit::component::Disableable as _;
    let ok_view = view.clone();
    let cancel_view = view.clone();
    h_flex()
        .flex_none()
        .justify_end()
        .gap(SPACE_3)
        .child(
            Button::new((id, 0usize))
                .small()
                .ghost()
                .label(kantor_ui::i18n::t("Cancel"))
                .on_click(move |_, w, cx| {
                    let _ = cancel_view.update(cx, |this, cx| cancel(this, w, cx));
                }),
        )
        .child(
            Button::new((id, 1usize))
                .small()
                .primary()
                .label(ok_label)
                .disabled(!ok_enabled)
                .on_click(move |_, w, cx| {
                    let _ = ok_view.update(cx, |this, cx| ok(this, w, cx));
                }),
        )
}

/// A number typed by a person: `2.5`, `2,5` (Indonesian decimal comma).
pub(crate) fn parse_number(text: &str) -> Option<f64> {
    let t = text.trim().replace(',', ".");
    t.parse::<f64>().ok().filter(|v| v.is_finite())
}

/// `A` → 0, `AB` → 27.
pub(crate) fn column_index(letters: &str) -> Option<u32> {
    if letters.is_empty() || letters.len() > 3 {
        return None;
    }
    let mut n: u32 = 0;
    for c in letters.chars() {
        if !c.is_ascii_alphabetic() {
            return None;
        }
        n = n * 26 + (c.to_ascii_uppercase() as u32 - 'A' as u32 + 1);
    }
    (1..=sheets_engine::MAX_COLS).contains(&n).then(|| n - 1)
}

/// `Sheet1!$A$1:$B$2` → `$A$1:$B$2` (the sheet name dropped).
pub(crate) fn strip_sheet(text: &str) -> &str {
    let t = text.trim().trim_start_matches('=');
    t.rsplit_once('!').map_or(t, |(_, r)| r).trim()
}

/// Whole rows `$1:$3`, `2` (0-based, inclusive).
pub(crate) fn parse_rows(text: &str) -> Option<(u32, u32)> {
    let t = strip_sheet(text).replace('$', "");
    let (a, b) = t.split_once(':').unwrap_or((&t, &t));
    let a: u32 = a.trim().parse().ok()?;
    let b: u32 = b.trim().parse().ok()?;
    let (a, b) = (a.min(b), a.max(b));
    (a >= 1 && b <= sheets_engine::MAX_ROWS).then(|| (a - 1, b - 1))
}

/// Whole columns `$A:$B`, `C` (0-based, inclusive).
pub(crate) fn parse_cols(text: &str) -> Option<(u32, u32)> {
    let t = strip_sheet(text).replace('$', "");
    let (a, b) = t.split_once(':').unwrap_or((&t, &t));
    let a = column_index(a.trim())?;
    let b = column_index(b.trim())?;
    Some((a.min(b), a.max(b)))
}

/// A cell range `$A$1:$D$20` or `B7`, with or without its sheet.
pub(crate) fn parse_cells(text: &str) -> Option<sheets_engine::CellRange> {
    sheets_engine::formula::parse_range(&strip_sheet(text).replace('$', ""))
}

/// Several ranges separated by commas.
pub(crate) fn parse_ranges(text: &str) -> Option<Vec<sheets_engine::CellRange>> {
    if text.trim().is_empty() {
        return Some(Vec::new());
    }
    text.split(',').map(parse_cells).collect()
}

/// `$1:$2` for rows, `$A:$B` for columns.
pub(crate) fn rows_text((a, b): (u32, u32)) -> String {
    format!("${}:${}", a + 1, b + 1)
}

pub(crate) fn cols_text((a, b): (u32, u32)) -> String {
    format!(
        "${}:${}",
        sheets_engine::column_name(a),
        sheets_engine::column_name(b)
    )
}

/// `$A$1:$D$20`.
pub(crate) fn range_text(r: &sheets_engine::CellRange) -> String {
    let a = format!("${}${}", sheets_engine::column_name(r.left), r.top + 1);
    if r.is_single() {
        a
    } else {
        format!(
            "{a}:${}${}",
            sheets_engine::column_name(r.right),
            r.bottom + 1
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn references_are_read() {
        assert_eq!(parse_rows("$1:$2"), Some((0, 1)));
        assert_eq!(parse_rows("Sheet1!$3"), Some((2, 2)));
        assert_eq!(parse_cols("$A:$B"), Some((0, 1)));
        assert_eq!(parse_cols("aa"), Some((26, 26)));
        assert_eq!(parse_cols("1"), None);
        assert_eq!(parse_number("2,5"), Some(2.5));
        let r = parse_ranges("$A$1:$B$2, D4").unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(range_text(&r[0]), "$A$1:$B$2");
        assert_eq!(range_text(&r[1]), "$D$4");
        assert_eq!(parse_ranges("nope"), None);
    }
}
