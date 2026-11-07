//! UI and monospace font selection.
//!
//! Kantor never bundles proprietary fonts. It uses SF Pro when the user has it
//! installed, else the best free alternative.

use gpui_kit::component::Theme;
use gpui_kit::*;

/// Preferred UI fonts, best first.
pub const UI_FONTS: &[&str] = &[
    "SF Pro Text",
    "SF Pro",
    "Inter",
    "Inter Variable",
    "Adwaita Sans",
    "Noto Sans",
    "Cantarell",
];

/// Preferred monospace fonts, best first.
pub const MONO_FONTS: &[&str] = &[
    "SF Mono",
    "JetBrainsMono Nerd Font",
    "JetBrains Mono",
    "Adwaita Mono",
    "Noto Sans Mono",
];

/// Installed font families, sorted.
pub fn installed(cx: &App) -> Vec<String> {
    cx.text_system().all_font_names()
}

fn first_installed(installed: &[String], prefs: &[&str]) -> Option<String> {
    prefs
        .iter()
        .find(|p| installed.iter().any(|f| f == *p))
        .map(|p| (*p).to_owned())
}

/// The UI font Kantor uses when the user hasn't chosen one.
pub fn automatic_ui_font(cx: &App) -> Option<String> {
    first_installed(&installed(cx), UI_FONTS)
}

/// Apply `ui_font` (or the automatic choice) and the best monospace font to the theme.
pub fn apply(ui_font: Option<&str>, cx: &mut App) {
    let fonts = installed(cx);
    let ui = ui_font
        .filter(|f| fonts.iter().any(|i| i == f))
        .map(str::to_owned)
        .or_else(|| first_installed(&fonts, UI_FONTS));
    let mono = first_installed(&fonts, MONO_FONTS);
    Theme::update(cx, |theme| {
        if let Some(ui) = ui {
            theme.font_family = ui.into();
        }
        if let Some(mono) = mono {
            theme.mono_font_family = mono.into();
        }
    });
}
