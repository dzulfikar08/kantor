//! Shared pieces of the AI panels: the card, field labels, notes.

use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::metrics::*;
use kantor_ui::tokens;
use kantor_ui::tokens::Elevation;

/// Width of an AI panel card.
pub const PANEL_WIDTH: Pixels = px(520.);
/// Height limit of result previews before they scroll.
pub const PREVIEW_MAX_HEIGHT: Pixels = px(240.);

/// The card: title row, then `body`.
pub fn card(title: &str, subtitle: Option<String>, cx: &App) -> Div {
    let t = tokens(cx);
    v_flex()
        .w(PANEL_WIDTH)
        .mt(PALETTE_TOP)
        .rounded(RADIUS_XL)
        .bg(t.elevated)
        .border_1()
        .border_color(t.border_strong)
        .shadow(t.shadow(Elevation::Modal))
        .overflow_hidden()
        .child(
            h_flex()
                .px(SPACE_7)
                .py(SPACE_5)
                .gap(SPACE_4)
                .border_b_1()
                .border_color(t.border)
                .child(
                    gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Sparkles)
                        .with_size(ICON_LG)
                        .text_color(t.accent),
                )
                .child(
                    div()
                        .text_size(FONT_TITLE)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.fg)
                        .child(title.to_owned()),
                )
                .child(div().flex_1())
                .children(
                    subtitle.map(|s| div().text_size(FONT_SMALL).text_color(t.fg_dim).child(s)),
                ),
        )
}

/// A labelled form row.
pub fn field(label: &str, control: impl IntoElement, cx: &App) -> Div {
    let t = tokens(cx);
    v_flex()
        .gap(SPACE_2)
        .child(
            div()
                .text_size(FONT_SMALL)
                .font_weight(FontWeight::MEDIUM)
                .text_color(t.fg_muted)
                .child(label.to_owned()),
        )
        .child(control)
}

/// Small explanatory text.
pub fn note(text: impl Into<SharedString>, cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .text_size(FONT_SMALL)
        .text_color(t.fg_dim)
        .child(text.into())
}

/// A status line: neutral, success or error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    None,
    Busy(String),
    Ok(String),
    Warn(String),
    Error(String),
}

pub fn status_line(status: &Status, cx: &App) -> Option<Div> {
    let t = tokens(cx);
    let (text, color) = match status {
        Status::None => return None,
        Status::Busy(s) => (s.clone(), t.fg_muted),
        Status::Ok(s) => (format!("✓ {s}"), t.success),
        Status::Warn(s) => (format!("⚠ {s}"), t.warning),
        Status::Error(s) => (s.clone(), t.danger),
    };
    Some(div().text_size(FONT_SMALL).text_color(color).child(text))
}

/// The footer button row.
pub fn footer(cx: &App) -> Div {
    let t = tokens(cx);
    h_flex()
        .px(SPACE_7)
        .py(SPACE_5)
        .gap(SPACE_4)
        .bg(t.chrome)
        .border_t_1()
        .border_color(t.border)
}

/// Monospace-ish box for a formula or rewritten text.
pub fn result_box(cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .p(SPACE_5)
        .rounded(RADIUS_MD)
        .bg(t.surface)
        .border_1()
        .border_color(t.border)
        .text_size(FONT_BODY)
        .text_color(t.fg)
}
