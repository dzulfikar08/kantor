//! Small stateless pieces styled from the tokens.

use gpui_kit::*;

use crate::metrics::*;
use crate::tokens;

/// A keyboard hint like `Ctrl K`.
pub fn kbd(keys: impl Into<SharedString>, cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .px(SPACE_2)
        .py(px(1.))
        .rounded(RADIUS_SM)
        .border_1()
        .border_color(t.border)
        .bg(t.surface)
        .text_size(FONT_CAPTION)
        .text_color(t.fg_muted)
        .child(keys.into())
}

/// An uppercase caption used to label sections.
pub fn section_label(text: impl Into<SharedString>, cx: &App) -> Div {
    let t = tokens(cx);
    div()
        .text_size(FONT_CAPTION)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.fg_dim)
        .child(text.into().to_uppercase())
}

/// A small round status dot.
pub fn dot(color: Hsla) -> Div {
    div().size(px(7.)).flex_none().rounded_full().bg(color)
}
