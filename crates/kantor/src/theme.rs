//! Maps the Omarchy palette onto GPUI Kit's theme and live-reloads it.

use std::time::Duration;

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use kantor_theme::{Palette, Rgb, mix};

fn c(rgb: Rgb) -> Hsla {
    gpui_kit::rgb(rgb).into()
}

fn ca(rgb: Rgb, alpha: f32) -> Hsla {
    let mut h: Hsla = gpui_kit::rgb(rgb).into();
    h.a = alpha;
    h
}

/// Apply `p` to the global theme and refresh every window.
pub fn apply(p: &Palette, cx: &mut App) {
    let mode = if p.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    // Load the matching base theme first, then override colors in a second update
    // (see `Theme::update` docs: a mode switch replaces colors).
    Theme::change(mode, None, cx);

    let border = mix(p.background, p.foreground, 0.16);
    let hover = mix(p.background, p.foreground, 0.06);
    Theme::update(cx, |t| {
        t.background = c(p.background);
        t.foreground = c(p.foreground);
        t.border = c(border);
        t.input = c(border);
        t.ring = c(p.accent);
        t.caret = c(p.foreground);
        t.selection = ca(p.accent, 0.35);

        t.primary = c(p.accent);
        t.primary_hover = c(mix(p.accent, p.foreground, 0.15));
        t.primary_active = c(mix(p.accent, p.background, 0.15));
        t.primary_foreground = c(p.background);
        t.secondary = c(p.lighter_background);
        t.secondary_hover = c(mix(p.lighter_background, p.foreground, 0.08));
        t.secondary_active = c(mix(p.lighter_background, p.foreground, 0.12));
        t.secondary_foreground = c(p.foreground);
        t.muted = c(p.lighter_background);
        t.muted_foreground = c(p.muted);
        t.accent = c(hover);
        t.accent_foreground = c(p.foreground);
        t.popover = c(p.dark_background);
        t.popover_foreground = c(p.foreground);
        t.danger = c(p.red);
        t.success = c(p.green);
        t.warning = c(p.yellow);
        t.info = c(p.blue);
        t.link = c(p.accent);

        t.title_bar = c(p.dark_background);
        t.title_bar_border = c(border);
        t.status_bar = c(p.dark_background);
        t.status_bar_border = c(border);

        t.table = c(p.background);
        t.table_head = c(p.dark_background);
        t.table_head_foreground = c(p.muted);
        t.table_even = c(p.background);
        t.table_hover = c(hover);
        t.table_active = ca(p.accent, 0.22);
        t.table_active_border = c(p.accent);
        t.table_row_border = c(mix(p.background, p.foreground, 0.08));

        t.list = c(p.background);
        t.list_head = c(p.dark_background);
        t.list_even = c(p.background);
        t.list_hover = c(hover);
        t.list_active = ca(p.accent, 0.22);
        t.list_active_border = c(p.accent);

        t.scrollbar = Hsla::transparent_black();
        t.scrollbar_thumb = ca(p.foreground, 0.25);
        t.scrollbar_thumb_hover = ca(p.foreground, 0.4);
        t.window_border = c(border);
    });
}

/// Apply the current Omarchy theme now and re-apply it whenever it changes.
pub fn init(cx: &mut App) {
    apply(&Palette::current(), cx);

    cx.spawn(async move |cx| {
        let mut last = kantor_theme::theme_stamp();
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let stamp = kantor_theme::theme_stamp();
            if stamp != last {
                last = stamp;
                let palette = Palette::current();
                log::info!("Omarchy theme changed; reloading");
                cx.update(|cx| apply(&palette, cx));
            }
        }
    })
    .detach();
}
