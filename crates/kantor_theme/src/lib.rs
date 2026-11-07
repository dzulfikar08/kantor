//! Omarchy theme bridge.
//!
//! Omarchy writes the active theme to `~/.local/state/omarchy/current/theme/`,
//! including a `colors.toml` palette, and bumps `../theme.generation` on every
//! `omarchy theme set`. This crate parses that palette into plain RGB values.
//! It deliberately has no GPUI dependency: the app shell maps a [`Palette`]
//! onto the UI theme.

pub mod catalog;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow};
use serde::Deserialize;

/// A 24-bit RGB color, `0xRRGGBB`.
pub type Rgb = u32;

/// The subset of the Omarchy palette Kantor uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Palette {
    pub dark: bool,
    pub background: Rgb,
    pub dark_background: Rgb,
    pub lighter_background: Rgb,
    pub foreground: Rgb,
    pub dark_foreground: Rgb,
    pub accent: Rgb,
    pub selection: Rgb,
    pub muted: Rgb,
    pub red: Rgb,
    pub green: Rgb,
    pub yellow: Rgb,
    pub blue: Rgb,
    pub magenta: Rgb,
    pub cyan: Rgb,
}

impl Default for Palette {
    /// A neutral dark palette used when no Omarchy theme is found.
    fn default() -> Self {
        Self {
            dark: true,
            background: 0x1e1e2e,
            dark_background: 0x181825,
            lighter_background: 0x313244,
            foreground: 0xcdd6f4,
            dark_foreground: 0x6c7086,
            accent: 0x89b4fa,
            selection: 0x45475a,
            muted: 0x7f849c,
            red: 0xf38ba8,
            green: 0xa6e3a1,
            yellow: 0xf9e2af,
            blue: 0x89b4fa,
            magenta: 0xcba6f7,
            cyan: 0x94e2d5,
        }
    }
}

#[derive(Deserialize)]
struct RawColors {
    mode: Option<String>,
    background: String,
    foreground: String,
    accent: Option<String>,
    selection: Option<String>,
    muted: Option<String>,
    dark_background: Option<String>,
    lighter_background: Option<String>,
    dark_foreground: Option<String>,
    red: Option<String>,
    green: Option<String>,
    yellow: Option<String>,
    blue: Option<String>,
    magenta: Option<String>,
    cyan: Option<String>,
}

/// Parse `#rrggbb` / `rrggbb` (an optional alpha byte is ignored).
pub fn parse_hex(s: &str) -> Result<Rgb> {
    let h = s.trim().trim_start_matches('#');
    let h = match h.len() {
        6 => h,
        8 => &h[..6],
        _ => return Err(anyhow!("not a #rrggbb color: {s:?}")),
    };
    u32::from_str_radix(h, 16).with_context(|| format!("not a hex color: {s:?}"))
}

/// Relative luminance in 0..=1 (sRGB, unlinearised; good enough to pick light/dark).
pub fn luminance(c: Rgb) -> f32 {
    let r = ((c >> 16) & 0xff) as f32;
    let g = ((c >> 8) & 0xff) as f32;
    let b = (c & 0xff) as f32;
    (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255.0
}

/// Linear mix of two colors; `t = 0` gives `a`, `t = 1` gives `b`.
pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let ch = |c: Rgb, s: u32| ((c >> s) & 0xff) as f32;
    let m = |s| {
        (ch(a, s) + (ch(b, s) - ch(a, s)) * t)
            .round()
            .clamp(0.0, 255.0) as u32
    };
    (m(16) << 16) | (m(8) << 8) | m(0)
}

impl Palette {
    /// Parse the contents of an Omarchy `colors.toml`.
    pub fn from_colors_toml(src: &str) -> Result<Self> {
        let raw: RawColors = toml::from_str(src).context("parsing colors.toml")?;
        let background = parse_hex(&raw.background)?;
        let foreground = parse_hex(&raw.foreground)?;
        let opt = |v: &Option<String>, fallback: Rgb| -> Result<Rgb> {
            v.as_deref()
                .map(parse_hex)
                .transpose()
                .map(|c| c.unwrap_or(fallback))
        };
        let dark = match raw.mode.as_deref() {
            Some("light") => false,
            Some("dark") => true,
            _ => luminance(background) < 0.5,
        };
        let accent = opt(&raw.accent, opt(&raw.blue, foreground)?)?;
        Ok(Self {
            dark,
            background,
            foreground,
            accent,
            dark_background: opt(&raw.dark_background, mix(background, 0x000000, 0.15))?,
            lighter_background: opt(&raw.lighter_background, mix(background, foreground, 0.1))?,
            dark_foreground: opt(&raw.dark_foreground, mix(foreground, background, 0.6))?,
            selection: opt(&raw.selection, mix(background, accent, 0.3))?,
            muted: opt(&raw.muted, mix(foreground, background, 0.45))?,
            red: opt(&raw.red, 0xe06c75)?,
            green: opt(&raw.green, 0x98c379)?,
            yellow: opt(&raw.yellow, 0xe5c07b)?,
            blue: opt(&raw.blue, accent)?,
            magenta: opt(&raw.magenta, 0xc678dd)?,
            cyan: opt(&raw.cyan, 0x56b6c2)?,
        })
    }

    /// Load the palette from an Omarchy theme directory.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let path = dir.join("colors.toml");
        let src = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        Self::from_colors_toml(&src)
    }

    /// Load the current Omarchy palette, falling back to [`Palette::default`].
    pub fn current() -> Self {
        omarchy_theme_dir()
            .and_then(|d| Self::load_from_dir(&d).ok())
            .unwrap_or_default()
    }
}

/// `$XDG_STATE_HOME/omarchy/current` (default `~/.local/state/omarchy/current`).
pub fn omarchy_current_dir() -> Option<PathBuf> {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(state.join("omarchy/current"))
}

/// The active theme directory, if Omarchy is installed.
pub fn omarchy_theme_dir() -> Option<PathBuf> {
    omarchy_current_dir()
        .map(|d| d.join("theme"))
        .filter(|d| d.join("colors.toml").is_file())
}

/// Name of the active Omarchy theme (e.g. `vantablack`), if any.
pub fn theme_name() -> Option<String> {
    let name = std::fs::read_to_string(omarchy_current_dir()?.join("theme.name")).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// A cheap token that changes whenever the theme changes. Poll it to live-reload.
///
/// Uses `theme.generation` (bumped by `omarchy theme set`), falling back to the
/// modification time of `colors.toml`.
pub fn theme_stamp() -> Option<String> {
    let cur = omarchy_current_dir()?;
    if let Ok(g) = std::fs::read_to_string(cur.join("theme.generation")) {
        return Some(g.trim().to_owned());
    }
    let meta = std::fs::metadata(cur.join("theme/colors.toml")).ok()?;
    Some(format!("{:?}", meta.modified().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VANTABLACK: &str = r##"
mode = "dark"
accent = "#8d8d8d"
selection = "#1a1a1a"
muted = "#7a7a7a"
background = "#000000"
dark_background = "#090909"
lighter_background = "#1a1a1a"
foreground = "#ffffff"
dark_foreground = "#505050"
red = "#a4a4a4"
blue = "#8d8d8d"
"##;

    #[test]
    fn parses_full_palette() {
        let p = Palette::from_colors_toml(VANTABLACK).unwrap();
        assert!(p.dark);
        assert_eq!(p.background, 0x000000);
        assert_eq!(p.foreground, 0xffffff);
        assert_eq!(p.accent, 0x8d8d8d);
        assert_eq!(p.lighter_background, 0x1a1a1a);
        assert_eq!(p.red, 0xa4a4a4);
        assert_eq!(p.green, 0x98c379); // fallback
    }

    #[test]
    fn infers_light_mode_and_fills_gaps() {
        let p = Palette::from_colors_toml("background = \"#fafafa\"\nforeground = \"#111111\"\n")
            .unwrap();
        assert!(!p.dark);
        assert_eq!(p.accent, 0x111111);
    }

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("#a1B2c3").unwrap(), 0xa1b2c3);
        assert_eq!(parse_hex("a1b2c3ff").unwrap(), 0xa1b2c3);
        assert!(parse_hex("#abc").is_err());
        assert!(parse_hex("#zzzzzz").is_err());
    }

    #[test]
    fn mixing() {
        assert_eq!(mix(0x000000, 0xffffff, 0.0), 0x000000);
        assert_eq!(mix(0x000000, 0xffffff, 1.0), 0xffffff);
        assert_eq!(mix(0x000000, 0xfefefe, 0.5), 0x7f7f7f);
    }

    #[test]
    fn loads_from_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("colors.toml"), VANTABLACK).unwrap();
        assert_eq!(Palette::load_from_dir(dir.path()).unwrap().muted, 0x7a7a7a);
    }
}
