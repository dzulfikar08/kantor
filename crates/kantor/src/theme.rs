//! Appearance: theme and font choice, persisted in settings, applied through
//! `kantor_ui`. "Follow Omarchy" live-reloads when `omarchy theme set` runs,
//! "Follow system" when the OS switches between light and dark.

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

/// Is the OS (or, on macOS, the app's forced appearance) dark? On Linux
/// this comes from the desktop portal's colour scheme; without an answer
/// the vendored GPUI reports dark (see vendor/gpui-pre-linux).
pub fn system_dark(cx: &App) -> bool {
    matches!(
        cx.window_appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    )
}

/// Resolve a theme against the current OS appearance.
pub fn resolve(id: &ThemeId, cx: &App) -> Palette {
    catalog::resolve(id, system_dark(cx))
}

/// Match the native window chrome (macOS title bar and border) to the
/// theme: follow the OS for "Follow system", else force light or dark.
/// A no-op on other platforms.
fn sync_native_appearance(id: &ThemeId, palette: &Palette, cx: &App) {
    cx.set_window_appearance(if id.follows_system() {
        None
    } else if palette.dark {
        Some(WindowAppearance::Dark)
    } else {
        Some(WindowAppearance::Light)
    });
}

pub fn init(cx: &mut App) {
    let settings = Settings::load();
    let appearance = Appearance {
        theme: ThemeId::parse(&settings.theme),
        ui_font: settings.ui_font,
    };
    // Clear any forced appearance before reading the system's.
    cx.set_window_appearance(None);
    let palette = resolve(&appearance.theme, cx);
    sync_native_appearance(&appearance.theme, &palette, cx);
    apply_palette(&palette, cx);
    fonts::apply(appearance.ui_font.as_deref(), cx);
    cx.set_global(appearance);

    cx.spawn(async move |cx| {
        let mut last = kantor_theme::theme_stamp();
        let mut last_dark = cx.update(|cx| system_dark(cx));
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let stamp = kantor_theme::theme_stamp();
            let omarchy_changed = stamp != last;
            last = stamp;
            cx.update(|cx| {
                let theme = current(cx).theme.clone();
                let dark = system_dark(cx);
                let system_changed = dark != last_dark;
                last_dark = dark;
                let reload = match theme {
                    ThemeId::FollowOmarchy => omarchy_changed,
                    ThemeId::System => system_changed,
                    _ => false,
                };
                if reload {
                    log::info!("{} changed; reloading the theme", theme.name());
                    preview_palette(&resolve(&theme, cx), cx);
                }
            });
        }
    })
    .detach();
}

fn save(cx: &App) {
    let a = current(cx);
    let settings = Settings {
        theme: a.theme.to_setting(),
        ui_font: a.ui_font.clone(),
        // Keep settings owned by other parts of the app (e.g. the Docs ruler).
        ..Settings::load()
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
    if id.follows_system() {
        cx.set_window_appearance(None);
    }
    let palette = resolve(&id, cx);
    sync_native_appearance(&id, &palette, cx);
    cx.global_mut::<Appearance>().theme = id;
    preview_palette(&palette, cx);
    save(cx);
}

/// Undo previews: re-apply the committed theme and font.
pub fn restore(cx: &mut App) {
    let palette = resolve(&current(cx).theme, cx);
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
