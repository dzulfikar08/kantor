//! Applies the Omarchy palette (via `kantor_ui` tokens) and live-reloads it.

use std::time::Duration;

use gpui_kit::*;
use kantor_theme::Palette;
use kantor_ui::tokens::apply_palette;

/// Apply the current Omarchy theme now and re-apply it whenever it changes.
pub fn init(cx: &mut App) {
    apply_palette(&Palette::current(), cx);

    cx.spawn(async move |cx| {
        let mut last = kantor_theme::theme_stamp();
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let stamp = kantor_theme::theme_stamp();
            if stamp != last {
                last = stamp;
                let palette = Palette::current();
                log::info!("Omarchy theme changed; reloading");
                cx.update(|cx| {
                    apply_palette(&palette, cx);
                    cx.refresh_windows();
                });
            }
        }
    })
    .detach();
}
