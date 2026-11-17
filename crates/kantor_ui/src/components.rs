//! Small stateless pieces styled from the tokens.

use gpui_kit::component::Sizable as _;
use gpui_kit::component::button::{
    Button, ButtonCustomVariant, ButtonRounded, ButtonVariants as _,
};
use gpui_kit::*;

use crate::i18n::tk;
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
        .child(crate::keys::label(&keys.into()))
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

/// A square icon button (22 px), `active` shows the selected state.
///
/// Built on GPUI Kit's `Button` so its tooltip goes through the window's
/// tooltip overlay; give it one with [`IconButtonExt::tip`].
pub fn icon_button(
    id: impl Into<ElementId>,
    icon: impl Into<gpui_kit::component::Icon>,
    active: bool,
    cx: &App,
) -> Button {
    let t = tokens(cx);
    let colors = ButtonCustomVariant::new(cx)
        .color(if active {
            t.selected
        } else {
            Hsla::transparent_black()
        })
        .foreground(if active { t.fg } else { t.fg_muted })
        .hover(if active { t.selected } else { t.hover })
        .active(t.selected);
    Button::new(id)
        .custom(colors)
        .tab_stop(false)
        .rounded(ButtonRounded::Size(RADIUS_MD))
        .flex_none()
        .size(CONTROL_SM)
        .p_0()
        .cursor_pointer()
        .child(gpui_kit::component::Icon::new(icon).with_size(ICON_MD))
}

/// Tooltip text for a control: the action, then its shortcut if it has one
/// (`Bold · Ctrl+B`).
pub fn tip_text(label: &str, shortcut: Option<&str>) -> String {
    match shortcut {
        Some(keys) if !keys.is_empty() => format!("{label} · {}", crate::keys::label(keys)),
        _ => label.to_owned(),
    }
}

/// Tooltips for [`icon_button`]s and other GPUI Kit buttons.
pub trait IconButtonExt {
    /// Show `label` and, if given, its keyboard `shortcut` on hover.
    fn tip(self, label: &str, shortcut: Option<&str>) -> Self;
}

impl IconButtonExt for Button {
    fn tip(self, label: &str, shortcut: Option<&str>) -> Self {
        self.tooltip(tip_text(label, shortcut))
    }
}

#[cfg(test)]
mod tests {
    use super::tip_text;

    #[std::prelude::v1::test]
    fn tip_text_adds_the_shortcut_when_there_is_one() {
        assert_eq!(tip_text("Bold", Some("Ctrl+B")), "Bold · Ctrl+B");
        assert_eq!(tip_text("Insert table", None), "Insert table");
        assert_eq!(tip_text("Strikethrough", Some("")), "Strikethrough");
    }
}

/// A pattern fill's 8 × 8 tile (DrawingML `a:pattFill`; bit `i` of a row's
/// byte is column `i` from the left) filling its box: the background
/// colour, then the foreground bits `unit` px square. For the Pattern fill
/// galleries; size it with the returned element's style methods.
pub fn pattern_swatch(bits: [u8; 8], fg: Hsla, bg: Hsla, unit: f32) -> Canvas<()> {
    let unit = unit.max(0.5);
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            window.paint_quad(fill(b, bg));
            let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
            let cols = (w / unit).ceil() as usize;
            let rows = (h / unit).ceil() as usize;
            for y in 0..rows.min(512) {
                let row = bits[y % 8];
                let top = y as f32 * unit;
                let mut x = 0;
                while x < cols.min(512) {
                    if row >> (x % 8) & 1 == 0 {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < cols && row >> (x % 8) & 1 == 1 {
                        x += 1;
                    }
                    let left = start as f32 * unit;
                    let right = (x as f32 * unit).min(w);
                    window.paint_quad(fill(
                        Bounds::new(
                            point(b.origin.x + px(left), b.origin.y + px(top)),
                            size(px(right - left), px(unit.min(h - top))),
                        ),
                        fg,
                    ));
                }
            }
        },
    )
}

/// The names of the DrawingML pattern presets as Office shows them, in
/// the order of `kantor_ooxml::pattern::PRESETS` (translated with
/// `i18n::t_dyn`).
pub const PATTERN_NAMES: [&str; 54] = [
    tk("5%"),
    tk("10%"),
    tk("20%"),
    tk("25%"),
    tk("30%"),
    tk("40%"),
    tk("50%"),
    tk("60%"),
    tk("70%"),
    tk("75%"),
    tk("80%"),
    tk("90%"),
    tk("Light downward diagonal"),
    tk("Light upward diagonal"),
    tk("Dark downward diagonal"),
    tk("Dark upward diagonal"),
    tk("Wide downward diagonal"),
    tk("Wide upward diagonal"),
    tk("Light vertical"),
    tk("Light horizontal"),
    tk("Narrow vertical"),
    tk("Narrow horizontal"),
    tk("Dark vertical"),
    tk("Dark horizontal"),
    tk("Dashed downward diagonal"),
    tk("Dashed upward diagonal"),
    tk("Dashed horizontal"),
    tk("Dashed vertical"),
    tk("Small confetti"),
    tk("Large confetti"),
    tk("Zig zag"),
    tk("Wave"),
    tk("Diagonal brick"),
    tk("Horizontal brick"),
    tk("Weave"),
    tk("Plaid"),
    tk("Divot"),
    tk("Dotted grid"),
    tk("Dotted diamond"),
    tk("Shingle"),
    tk("Trellis"),
    tk("Sphere"),
    tk("Small grid"),
    tk("Large grid"),
    tk("Small checker board"),
    tk("Large checker board"),
    tk("Outlined diamond"),
    tk("Solid diamond"),
    tk("Horizontal"),
    tk("Vertical"),
    tk("Downward diagonal"),
    tk("Upward diagonal"),
    tk("Cross"),
    tk("Diagonal cross"),
];
