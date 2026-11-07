//! Colour tokens derived from the Omarchy palette.
//!
//! Recipe from Voltrus Tools' `Theme::from_scheme`: chrome, surfaces and borders are
//! the foreground blended over the background at small fixed amounts, so every
//! Omarchy theme yields a consistent, layered UI.

use gpui_kit::component::{Theme, ThemeMode};
use gpui_kit::*;
use kantor_theme::{Palette, Rgb, mix};

/// Semantic colours for Kantor's own views. Read with [`tokens`].
#[derive(Debug, Clone, Copy)]
pub struct Tokens {
    pub dark: bool,
    /// Document background.
    pub bg: Hsla,
    /// Tab strip, sidebar, status bar.
    pub chrome: Hsla,
    /// Cards, inputs, table headers.
    pub surface: Hsla,
    /// Menus, dialogs, the command palette.
    pub elevated: Hsla,
    pub fg: Hsla,
    pub fg_muted: Hsla,
    pub fg_dim: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    pub hover: Hsla,
    pub selected: Hsla,
    pub accent: Hsla,
    pub accent_soft: Hsla,
    pub accent_fg: Hsla,
    pub success: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    pub info: Hsla,
    /// App colours: Docs blue, Sheets green, Slides orange (match the icons).
    pub docs: Hsla,
    pub sheets: Hsla,
    pub slides: Hsla,
    pub scrim: Hsla,
}

impl Global for Tokens {}

/// The active tokens (defaults until [`apply_palette`] runs).
pub fn tokens(cx: &App) -> Tokens {
    cx.try_global::<Tokens>()
        .copied()
        .unwrap_or_else(|| Tokens::from_palette(&Palette::default()))
}

fn c(rgb: Rgb) -> Hsla {
    gpui_kit::rgb(rgb).into()
}

fn alpha(rgb: Rgb, a: f32) -> Hsla {
    let mut h = c(rgb);
    h.a = a;
    h
}

fn luminance(rgb: Rgb) -> f32 {
    let ch = |s: u32| ((rgb >> s) & 0xff) as f32 / 255.0;
    0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
}

#[derive(Debug, Clone, Copy)]
pub enum Elevation {
    Raised,
    Popover,
    Modal,
}

impl Tokens {
    pub fn from_palette(p: &Palette) -> Self {
        let on_bg = |a: f32| c(mix(p.background, p.foreground, a));
        let (chrome, surface, elevated, border, border_strong) = if p.dark {
            (on_bg(0.03), on_bg(0.05), on_bg(0.07), on_bg(0.10), on_bg(0.18))
        } else {
            (on_bg(0.025), on_bg(0.035), c(0xffffff), on_bg(0.11), on_bg(0.20))
        };
        Self {
            dark: p.dark,
            bg: c(p.background),
            chrome,
            surface,
            elevated,
            fg: c(p.foreground),
            fg_muted: alpha(p.foreground, 0.68),
            fg_dim: alpha(p.foreground, 0.42),
            border,
            border_strong,
            hover: alpha(p.foreground, 0.06),
            selected: alpha(p.accent, if p.dark { 0.16 } else { 0.12 }),
            accent: c(p.accent),
            accent_soft: alpha(p.accent, 0.12),
            accent_fg: if luminance(p.accent) > 0.6 {
                c(0x000000)
            } else {
                c(0xffffff)
            },
            success: c(p.green),
            warning: c(p.yellow),
            danger: c(p.red),
            info: c(p.blue),
            docs: c(0x4f8ff7),
            sheets: c(0x2fbf71),
            slides: c(0xf47a3a),
            scrim: Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: if p.dark { 0.45 } else { 0.2 },
            },
        }
    }

    /// Shadow stack for an elevation (Voltrus values; lighter in light mode).
    pub fn shadow(&self, elevation: Elevation) -> Vec<BoxShadow> {
        let k = if self.dark { 1.0 } else { 0.55 };
        let s = |y: f32, blur: f32, a: f32| BoxShadow {
            color: Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: a * k,
            },
            offset: point(px(0.), px(y)),
            blur_radius: px(blur),
            spread_radius: px(0.),
        };
        match elevation {
            Elevation::Raised => vec![s(1., 2., 0.18)],
            Elevation::Popover => vec![s(2., 6., 0.22), s(8., 24., 0.18)],
            Elevation::Modal => vec![s(4., 12., 0.25), s(16., 48., 0.30)],
        }
    }
}

/// Install `p` as the active palette: Kantor tokens plus GPUI Kit's theme.
pub fn apply_palette(p: &Palette, cx: &mut App) {
    let t = Tokens::from_palette(p);
    cx.set_global(t);

    let mode = if p.dark {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    // Load the matching base theme first, then override colours in a second update
    // (a mode switch replaces colours; see `Theme::update`).
    Theme::change(mode, None, cx);

    let solid_hover = c(mix(p.background, p.foreground, 0.06));
    Theme::update(cx, |theme| {
        theme.font_size = crate::metrics::REM;
        theme.radius = crate::metrics::RADIUS_MD;
        theme.radius_lg = crate::metrics::RADIUS_XL;
        theme.shadow = true;

        let k = &mut theme.colors;
        k.background = t.bg;
        k.foreground = t.fg;
        k.border = t.border;
        k.input = t.border;
        k.ring = t.accent;
        k.caret = t.fg;
        k.selection = alpha(p.accent, 0.35);

        k.primary = t.accent;
        k.primary_hover = c(mix(p.accent, p.foreground, 0.12));
        k.primary_active = c(mix(p.accent, p.background, 0.12));
        k.primary_foreground = t.accent_fg;
        k.secondary = t.surface;
        k.secondary_hover = solid_hover;
        k.secondary_active = c(mix(p.background, p.foreground, 0.10));
        k.secondary_foreground = t.fg;
        k.muted = t.surface;
        k.muted_foreground = t.fg_muted;
        k.accent = solid_hover;
        k.accent_foreground = t.fg;
        k.popover = t.elevated;
        k.popover_foreground = t.fg;
        k.danger = t.danger;
        k.success = t.success;
        k.warning = t.warning;
        k.info = t.info;
        k.link = t.accent;

        k.title_bar = t.chrome;
        k.title_bar_border = t.border;
        k.status_bar = t.chrome;
        k.status_bar_border = t.border;
        k.tab_bar = t.chrome;
        k.tab = t.chrome;
        k.tab_active = t.bg;
        k.tab_foreground = t.fg_muted;
        k.tab_active_foreground = t.fg;

        k.table = t.bg;
        k.table_head = t.surface;
        k.table_head_foreground = t.fg_muted;
        k.table_even = t.bg;
        k.table_hover = solid_hover;
        k.table_active = t.selected;
        k.table_active_border = t.accent;
        k.table_row_border = c(mix(p.background, p.foreground, 0.07));

        k.list = t.bg;
        k.list_head = t.surface;
        k.list_even = t.bg;
        k.list_hover = solid_hover;
        k.list_active = t.selected;
        k.list_active_border = t.accent;

        k.scrollbar = Hsla::transparent_black();
        k.scrollbar_thumb = alpha(p.foreground, 0.22);
        k.scrollbar_thumb_hover = alpha(p.foreground, 0.38);
        k.window_border = t.border;
    });
}
