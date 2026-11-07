//! Appearance: theme and font choice, persisted in settings, applied through
//! `kantor_ui`. "Follow Omarchy" live-reloads when `omarchy theme set` runs.

use std::time::Duration;

use gpui_kit::*;
use kantor_core::settings::Settings;
use kantor_theme::Palette;
use kantor_theme::catalog::{self, ThemeId};
use kantor_ui::{fonts, tokens::apply_palette};

/// The committed appearance (what's saved), as opposed to a live preview.
pub struct Appearance {
    pub theme: ThemeId,
    pub ui_font: Option<String>,
}

impl Global for Appearance {}

pub fn current(cx: &App) -> &Appearance {
    cx.global::<Appearance>()
}

pub fn init(cx: &mut App) {
    let settings = Settings::load();
    let appearance = Appearance {
        theme: ThemeId::parse(&settings.theme),
        ui_font: settings.ui_font,
    };
    apply_palette(&catalog::resolve(&appearance.theme), cx);
    fonts::apply(appearance.ui_font.as_deref(), cx);
    cx.set_global(appearance);

    cx.spawn(async move |cx| {
        let mut last = kantor_theme::theme_stamp();
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let stamp = kantor_theme::theme_stamp();
            if stamp != last {
                last = stamp;
                cx.update(|cx| {
                    if current(cx).theme == ThemeId::FollowOmarchy {
                        log::info!("Omarchy theme changed; reloading");
                        preview_palette(&Palette::current(), cx);
                    }
                });
            }
        }
    })
    .detach();
}

fn save(cx: &App) {
    let a = current(cx);
    let settings = Settings {
        theme: a.theme.to_setting(),
        ui_font: a.ui_font.clone(),
    };
    if let Err(err) = settings.save() {
        log::error!("saving settings: {err:#}");
    }
}

/// Show a palette without committing it (theme picker highlight).
pub fn preview_palette(p: &Palette, cx: &mut App) {
    apply_palette(p, cx);
    // Palette changes replace font fields on the GPUI Kit theme; re-apply them.
    let font = current(cx).ui_font.clone();
    fonts::apply(font.as_deref(), cx);
    cx.refresh_windows();
}

/// Commit and save a theme.
pub fn set_theme(id: ThemeId, cx: &mut App) {
    let palette = catalog::resolve(&id);
    cx.global_mut::<Appearance>().theme = id;
    preview_palette(&palette, cx);
    save(cx);
}

/// Undo previews: re-apply the committed theme and font.
pub fn restore(cx: &mut App) {
    let palette = catalog::resolve(&current(cx).theme);
    preview_palette(&palette, cx);
}

/// Show a UI font without committing it.
pub fn preview_font(font: Option<&str>, cx: &mut App) {
    fonts::apply(font, cx);
    cx.refresh_windows();
}

/// Commit and save a UI font (`None` = automatic).
pub fn set_font(font: Option<String>, cx: &mut App) {
    fonts::apply(font.as_deref(), cx);
    cx.global_mut::<Appearance>().ui_font = font;
    cx.refresh_windows();
    save(cx);
}
