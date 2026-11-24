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
    /// Outlines of controls the user must find to use (checkbox and radio
    /// boxes, input fields): at least 3:1 against every surface they sit
    /// on (WCAG 1.4.11), unlike `border`, which only separates areas.
    pub control_border: Hsla,
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
    /// Docs: the desk behind the pages, the paper, ink on paper (caret) and
    /// the selection on paper. Paper stays white in dark themes, like Word.
    pub desk: Hsla,
    pub paper: Hsla,
    pub paper_ink: Hsla,
    pub paper_selection: Hsla,
    /// Red wavy underline under misspelled words (on paper and slides, so
    /// the same in every theme, as Word's).
    pub paper_misspelled: Hsla,
    /// Find matches on the page (Docs) and the current match, stronger.
    pub find_match: Hsla,
    pub find_match_current: Hsla,
    /// Sheets: cell gridlines (quiet, but visible on both dark and light),
    /// the column-header row and row-number gutter, and those headers for
    /// the selected rows/columns (opaque, so text over them stays crisp).
    pub grid_line: Hsla,
    pub grid_header: Hsla,
    pub grid_header_active: Hsla,
    /// Colours of a formula's references while it is typed (cell outlines and
    /// formula text), distinct and readable on `bg` in every theme.
    pub refs: [Hsla; kantor_theme::refs::REF_COLORS],
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

/// WCAG relative luminance (sRGB channels linearised).
fn relative_luminance(rgb: Rgb) -> f32 {
    let ch = |s: u32| {
        let v = ((rgb >> s) & 0xff) as f32 / 255.0;
        if v <= 0.040_45 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(16) + 0.7152 * ch(8) + 0.0722 * ch(0)
}

/// WCAG contrast ratio of two colours (1 to 21).
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (relative_luminance(a), relative_luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// The palette's foreground blended over its background, the least that
/// reaches `ratio` against every colour of `on` (all of them when it can't).
fn contrasting(p: &Palette, on: &[Rgb], ratio: f32) -> Rgb {
    (10..=100)
        .map(|k| mix(p.background, p.foreground, k as f32 / 100.0))
        .find(|c| on.iter().all(|s| contrast(*c, *s) >= ratio))
        .unwrap_or(p.foreground)
}

/// `color` moved toward the palette's foreground until it reaches `ratio`
/// against every colour of `on`, for status colours used as text and icons
/// (a light theme's bright yellow is unreadable on white).
fn readable(p: &Palette, color: Rgb, on: &[Rgb], ratio: f32) -> Rgb {
    (0..=20)
        .map(|k| mix(color, p.foreground, k as f32 / 20.0))
        .find(|c| on.iter().all(|s| contrast(*c, *s) >= ratio))
        .unwrap_or(p.foreground)
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
        // Where controls sit: the document, chrome, cards and menus (the
        // amounts below), with a little margin over 3:1 for rounding.
        let surfaces: Vec<Rgb> = if p.dark {
            [0.0, 0.03, 0.05, 0.07]
                .iter()
                .map(|a| mix(p.background, p.foreground, *a))
                .collect()
        } else {
            [0.0, 0.025, 0.035]
                .iter()
                .map(|a| mix(p.background, p.foreground, *a))
                .chain([0xffffff])
                .collect()
        };
        let control_border = c(contrasting(p, &surfaces, 3.1));
        // Status colours are drawn as text and icons on the same surfaces.
        let status = |color: Rgb| c(readable(p, color, &surfaces, 3.0));
        // Sheets headers sit between chrome and surface, a step above the cells.
        let header = mix(
            p.background,
            p.foreground,
            if p.dark { 0.06 } else { 0.045 },
        );
        let (chrome, surface, elevated, border, border_strong) = if p.dark {
            (
                on_bg(0.03),
                on_bg(0.05),
                on_bg(0.07),
                on_bg(0.10),
                on_bg(0.18),
            )
        } else {
            (
                on_bg(0.025),
                on_bg(0.035),
                c(0xffffff),
                on_bg(0.11),
                on_bg(0.20),
            )
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
            control_border,
            hover: alpha(p.foreground, 0.06),
            selected: alpha(p.accent, if p.dark { 0.16 } else { 0.12 }),
            accent: c(p.accent),
            accent_soft: alpha(p.accent, 0.12),
            accent_fg: if luminance(p.accent) > 0.6 {
                c(0x000000)
            } else {
                c(0xffffff)
            },
            success: status(p.green),
            warning: status(p.yellow),
            danger: status(p.red),
            info: status(p.blue),
            docs: c(0x4f8ff7),
            sheets: c(0x2fbf71),
            slides: c(0xf47a3a),
            scrim: Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: if p.dark { 0.45 } else { 0.2 },
            },
            desk: if p.dark { on_bg(0.06) } else { on_bg(0.07) },
            paper: c(0xffffff),
            paper_ink: c(0x111111),
            paper_selection: alpha(p.accent, 0.30),
            paper_misspelled: c(0xe02020),
            find_match: alpha(p.yellow, 0.35),
            find_match_current: alpha(p.yellow, 0.80),
            grid_line: if p.dark { on_bg(0.12) } else { on_bg(0.14) },
            grid_header: c(header),
            grid_header_active: c(mix(header, p.accent, if p.dark { 0.22 } else { 0.18 })),
            refs: kantor_theme::refs::reference_colors(p).map(c),
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
            inset: false,
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
        // Checkbox and radio boxes and input fields are outlined in it.
        k.input = t.control_border;
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
        // The only DataTable is the Sheets grid, which draws its own active
        // cell (an outline, no fill), gridlines and header highlights.
        k.table_head = t.grid_header;
        k.table_head_foreground = t.fg_muted;
        k.table_even = t.bg;
        k.table_hover = solid_hover;
        k.table_active = Hsla::transparent_black();
        k.table_active_border = t.accent;
        k.table_row_border = t.grid_line;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn light() -> Palette {
        Palette {
            dark: false,
            background: 0xffffff,
            foreground: 0x1f1f1f,
            accent: 0x1a73e8,
            ..Palette::default()
        }
    }

    /// Gridlines and headers must stand apart from the cells in both modes.
    #[std::prelude::v1::test]
    fn grid_tokens_differ_from_cells() {
        for p in [Palette::default(), light()] {
            let t = Tokens::from_palette(&p);
            let l = |h: Hsla| h.l;
            assert!(
                (l(t.grid_line) - l(t.bg)).abs() > 0.05,
                "grid line, dark={}",
                p.dark
            );
            assert!(
                (l(t.grid_header) - l(t.bg)).abs() > 0.02,
                "header, dark={}",
                p.dark
            );
            assert_ne!(t.grid_header_active, t.grid_header);
            assert_eq!(t.grid_header_active.a, 1.0);
        }
    }

    fn rgb_of(h: Hsla) -> Rgb {
        let r = gpui_kit::Rgba::from(h);
        let ch = |v: f32| (v * 255.0).round() as u32;
        (ch(r.r) << 16) | (ch(r.g) << 8) | ch(r.b)
    }

    /// An unchecked checkbox's box (GPUI Kit draws it in the `input`
    /// colour) stands 3:1 apart from the document, chrome, cards and menus
    /// in dark and light themes (it was 1.3:1 in dark ones).
    #[std::prelude::v1::test]
    fn control_borders_reach_three_to_one() {
        let themes = [
            Palette::default(),
            light(),
            // Catppuccin Mocha and Latte, Nord-like, Gruvbox-like.
            Palette {
                dark: true,
                background: 0x1e1e2e,
                foreground: 0xcdd6f4,
                ..Palette::default()
            },
            Palette {
                dark: false,
                background: 0xeff1f5,
                foreground: 0x4c4f69,
                ..Palette::default()
            },
            Palette {
                dark: true,
                background: 0x2e3440,
                foreground: 0xd8dee9,
                ..Palette::default()
            },
            Palette {
                dark: true,
                background: 0x282828,
                foreground: 0xd4be98,
                ..Palette::default()
            },
        ];
        for p in themes {
            let t = Tokens::from_palette(&p);
            let border = rgb_of(t.control_border);
            for (name, s) in [
                ("bg", t.bg),
                ("chrome", t.chrome),
                ("surface", t.surface),
                ("elevated", t.elevated),
            ] {
                let r = contrast(border, rgb_of(s));
                assert!(
                    r >= 3.0,
                    "{name} {r:.2}, dark={} {:06x}",
                    p.dark,
                    p.background
                );
            }
            // Still quieter than text.
            assert!(contrast(border, p.background) < contrast(p.foreground, p.background));
        }
    }

    /// Equal but for rounding through `Hsla`.
    fn close(a: Rgb, b: Rgb) -> bool {
        [16, 8, 0]
            .iter()
            .all(|s| ((a >> s) & 0xff).abs_diff((b >> s) & 0xff) <= 1)
    }

    /// Status colours (the warning banner, unsaved dots, sync status) stay
    /// readable on light themes whose yellow is meant for dark terminals,
    /// and Kantor's own palettes keep theirs as they are.
    #[std::prelude::v1::test]
    fn status_colours_readable_on_light() {
        let bright = Palette {
            yellow: 0xf9e2af,
            green: 0xa6e3a1,
            ..light()
        };
        for p in [
            bright,
            Palette::kantor_light(),
            Palette::kantor_dark(),
            Palette::default(),
        ] {
            let t = Tokens::from_palette(&p);
            for (name, h) in [
                ("warning", t.warning),
                ("success", t.success),
                ("danger", t.danger),
                ("info", t.info),
            ] {
                for (on, s) in [("bg", t.bg), ("chrome", t.chrome), ("surface", t.surface)] {
                    let r = contrast(rgb_of(h), rgb_of(s));
                    assert!(r >= 2.95, "{name} on {on} {r:.2}, dark={}", p.dark);
                }
            }
        }
        let k = Palette::kantor_light();
        let t = Tokens::from_palette(&k);
        assert!(close(rgb_of(t.warning), k.yellow));
        assert!(close(rgb_of(t.accent), k.accent));
    }

    /// Kantor Light gets the light recipe: white document and menus, chrome
    /// and headers a little darker, gridlines visible, text 4.5:1 on them.
    #[std::prelude::v1::test]
    fn kantor_light_tokens() {
        let p = Palette::kantor_light();
        let t = Tokens::from_palette(&p);
        assert!(!t.dark);
        assert!(close(rgb_of(t.bg), 0xffffff));
        assert!(close(rgb_of(t.elevated), 0xffffff));
        assert!(t.chrome.l < t.bg.l && t.grid_header.l < t.bg.l && t.desk.l < t.bg.l);
        assert!(contrast(rgb_of(t.grid_line), 0xffffff) > 1.15);
        for s in [t.bg, t.chrome, t.surface, t.grid_header, t.desk] {
            assert!(contrast(p.foreground, rgb_of(s)) >= 4.5);
        }
        assert!(close(rgb_of(t.accent_fg), 0xffffff));
    }
}
