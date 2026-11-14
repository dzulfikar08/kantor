//! Shared pieces of the cloud panels (the AI panels' look, own key context).

use gpui_kit::assets::IconName;
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::metrics::*;
use kantor_ui::tokens;
use kantor_ui::tokens::Elevation;

pub use crate::ai::widgets::{Status, field, footer, note, status_line};

/// Key context of every cloud panel.
pub const CLOUD_PANEL: &str = "CloudPanel";

actions!(kantor_cloud_panel, [ClosePanel]);

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", ClosePanel, Some(CLOUD_PANEL))]);
}

/// Width of a cloud panel card.
pub const PANEL_WIDTH: Pixels = px(560.);

/// The card: icon and title row, then the body.
pub fn card(icon: IconName, title: &str, cx: &App) -> Div {
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
                    gpui_kit::component::Icon::new(icon)
                        .with_size(ICON_LG)
                        .text_color(t.accent),
                )
                .child(
                    div()
                        .text_size(FONT_TITLE)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.fg)
                        .child(title.to_owned()),
                ),
        )
}
