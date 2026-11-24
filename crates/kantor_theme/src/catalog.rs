//! All themes Kantor can use, for the theme picker.
//!
//! Sources, in picker order:
//! 1. **System**: *Follow system* (light or dark as the OS is set), *Kantor Light*,
//!    *Kantor Dark*, and *Follow Omarchy* (whatever `omarchy theme set` selected) when
//!    Omarchy is installed.
//! 2. **Omarchy themes** installed on this machine (`colors.toml` in
//!    `/usr/share/omarchy/themes`, `~/.local/share/omarchy/themes`, `~/.config/omarchy/themes`).
//! 3. **Ghostty themes**: 900+ schemes from iTerm2-Color-Schemes (MIT), embedded as a
//!    compact table (`assets/themes/ghostty.tsv`), the same set atur-terminal ships.
//!
//! The default is *Follow Omarchy* where Omarchy is installed and *Follow system*
//! everywhere else.

use std::path::PathBuf;

use crate::{Palette, Rgb, luminance, mix, omarchy_theme_dir, parse_hex};

const GHOSTTY_TSV: &str = include_str!("../../../assets/themes/ghostty.tsv");

/// Stable identifier of a theme, stored in settings as a string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ThemeId {
    /// Kantor Light or Kantor Dark, as the OS appearance is set.
    System,
    /// Kantor Light.
    Light,
    /// Kantor Dark.
    Dark,
    FollowOmarchy,
    Omarchy(String),
    Ghostty(String),
}

impl ThemeId {
    /// Parse a settings value (`system`, `light`, `dark`, `omarchy`,
    /// `omarchy:<name>` or `ghostty:<name>`) on this machine.
    pub fn parse(s: &str) -> Self {
        Self::parse_with(s, omarchy_available())
    }

    /// [`ThemeId::parse`] for a machine with or without Omarchy: there,
    /// `omarchy` and unknown values mean *Follow system*.
    pub fn parse_with(s: &str, omarchy: bool) -> Self {
        if let Some(n) = s.strip_prefix("omarchy:") {
            return Self::Omarchy(n.to_owned());
        }
        if let Some(n) = s.strip_prefix("ghostty:") {
            return Self::Ghostty(n.to_owned());
        }
        match s.trim() {
            "system" => Self::System,
            "light" => Self::Light,
            "dark" => Self::Dark,
            "omarchy" if omarchy => Self::FollowOmarchy,
            _ => Self::default_for(omarchy),
        }
    }

    /// The default theme: *Follow Omarchy* with Omarchy, *Follow system* without.
    pub fn default_for(omarchy: bool) -> Self {
        if omarchy {
            Self::FollowOmarchy
        } else {
            Self::System
        }
    }

    pub fn to_setting(&self) -> String {
        match self {
            Self::System => "system".to_owned(),
            Self::Light => "light".to_owned(),
            Self::Dark => "dark".to_owned(),
            Self::FollowOmarchy => "omarchy".to_owned(),
            Self::Omarchy(n) => format!("omarchy:{n}"),
            Self::Ghostty(n) => format!("ghostty:{n}"),
        }
    }

    /// The theme's name in English (the picker translates the built-in ones).
    pub fn name(&self) -> &str {
        match self {
            Self::System => "Follow System",
            Self::Light => "Kantor Light",
            Self::Dark => "Kantor Dark",
            Self::FollowOmarchy => "Follow Omarchy",
            Self::Omarchy(n) | Self::Ghostty(n) => n,
        }
    }

    /// Does the palette depend on the OS light/dark setting?
    pub fn follows_system(&self) -> bool {
        *self == Self::System
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    System,
    Omarchy,
    Ghostty,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Omarchy => "Omarchy",
            Self::Ghostty => "Ghostty",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThemeEntry {
    pub id: ThemeId,
    pub name: String,
    pub group: Group,
    pub palette: Palette,
}

impl ThemeEntry {
    /// Colours for the picker's swatch strip (after the background/"Ab" cell).
    pub fn swatch(&self) -> [Rgb; 6] {
        let p = &self.palette;
        [p.red, p.green, p.yellow, p.blue, p.magenta, p.cyan]
    }
}

/// Build a palette from a Ghostty scheme (`bg fg selection p0..p15`).
fn ghostty_palette(bg: Rgb, fg: Rgb, sel: Rgb, pal: &[Rgb; 16]) -> Palette {
    Palette {
        dark: luminance(bg) < 0.5,
        background: bg,
        dark_background: mix(bg, 0x000000, 0.15),
        lighter_background: mix(bg, fg, 0.1),
        foreground: fg,
        dark_foreground: mix(fg, bg, 0.6),
        accent: pal[4],
        selection: sel,
        muted: pal[8],
        red: pal[1],
        green: pal[2],
        yellow: pal[3],
        blue: pal[4],
        magenta: pal[5],
        cyan: pal[6],
    }
}

/// The embedded Ghostty themes.
pub fn ghostty_themes() -> Vec<ThemeEntry> {
    GHOSTTY_TSV
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .filter_map(|line| {
            let mut f = line.split('\t');
            let name = f.next()?.to_owned();
            let cols: Vec<Rgb> = f.map(parse_hex).collect::<Result<_, _>>().ok()?;
            if cols.len() != 19 {
                return None;
            }
            let pal: [Rgb; 16] = cols[3..19].try_into().ok()?;
            Some(ThemeEntry {
                id: ThemeId::Ghostty(name.clone()),
                palette: ghostty_palette(cols[0], cols[1], cols[2], &pal),
                name,
                group: Group::Ghostty,
            })
        })
        .collect()
}

fn omarchy_theme_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/usr/share/omarchy/themes")];
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        dirs.push(home.join(".local/share/omarchy/themes"));
        dirs.push(home.join(".config/omarchy/themes"));
    }
    dirs
}

/// Omarchy themes installed on this machine (later directories override earlier ones).
pub fn omarchy_themes() -> Vec<ThemeEntry> {
    let mut found: Vec<ThemeEntry> = Vec::new();
    for dir in omarchy_theme_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
                continue;
            };
            let Ok(palette) = Palette::load_from_dir(&path) else {
                continue;
            };
            found.retain(|e| e.name != name);
            found.push(ThemeEntry {
                id: ThemeId::Omarchy(name.clone()),
                name,
                group: Group::Omarchy,
                palette,
            });
        }
    }
    found.sort_by_key(|e| e.name.to_lowercase());
    found
}

/// Every theme, in picker order. `system_dark` is the OS appearance, for
/// *Follow system*.
pub fn all(system_dark: bool) -> Vec<ThemeEntry> {
    let omarchy = omarchy_available();
    let mut v: Vec<ThemeEntry> = builtin_ids(omarchy)
        .into_iter()
        .map(|id| ThemeEntry {
            name: id.name().to_owned(),
            palette: resolve_with(&id, system_dark, omarchy),
            group: Group::System,
            id,
        })
        .collect();
    v.extend(omarchy_themes());
    v.extend(ghostty_themes());
    v
}

/// The System group of the picker: *Follow system*, *Kantor Light*,
/// *Kantor Dark*, then *Follow Omarchy* when Omarchy is installed.
pub fn builtin_ids(omarchy: bool) -> Vec<ThemeId> {
    let mut v = vec![ThemeId::System, ThemeId::Light, ThemeId::Dark];
    if omarchy {
        v.push(ThemeId::FollowOmarchy);
    }
    v
}

/// Resolve a theme id to a palette on this machine. `system_dark` is the OS
/// appearance, used by *Follow system* (and as the fallback without Omarchy).
pub fn resolve(id: &ThemeId, system_dark: bool) -> Palette {
    resolve_with(id, system_dark, omarchy_available())
}

/// [`resolve`] for a machine with or without Omarchy. A theme that can't be
/// found falls back to the default theme.
pub fn resolve_with(id: &ThemeId, system_dark: bool, omarchy: bool) -> Palette {
    let fallback = || {
        if omarchy {
            Palette::current()
        } else {
            Palette::kantor(system_dark)
        }
    };
    match id {
        ThemeId::System => Palette::kantor(system_dark),
        ThemeId::Light => Palette::kantor_light(),
        ThemeId::Dark => Palette::kantor_dark(),
        ThemeId::FollowOmarchy => fallback(),
        ThemeId::Omarchy(name) => omarchy_themes()
            .into_iter()
            .find(|e| &e.name == name)
            .map(|e| e.palette)
            .unwrap_or_else(fallback),
        ThemeId::Ghostty(name) => ghostty_themes()
            .into_iter()
            .find(|e| &e.name == name)
            .map(|e| e.palette)
            .unwrap_or_else(fallback),
    }
}

/// Is the Omarchy theme directory present (i.e. can "Follow Omarchy" follow anything)?
pub fn omarchy_available() -> bool {
    omarchy_theme_dir().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghostty_table_parses() {
        let themes = ghostty_themes();
        assert!(themes.len() > 800, "only {} themes", themes.len());
        let dracula = themes
            .iter()
            .find(|t| t.name == "Dracula")
            .expect("Dracula");
        assert!(dracula.palette.dark);
        assert_eq!(dracula.palette.background, 0x282a36);
    }

    #[test]
    fn ids_round_trip() {
        for id in [
            ThemeId::System,
            ThemeId::Light,
            ThemeId::Dark,
            ThemeId::FollowOmarchy,
            ThemeId::Omarchy("nord".into()),
            ThemeId::Ghostty("Tokyo Night".into()),
        ] {
            assert_eq!(ThemeId::parse_with(&id.to_setting(), true), id);
        }
        assert_eq!(ThemeId::System.to_setting(), "system");
        assert_eq!(ThemeId::parse_with("system", false), ThemeId::System);
        assert_eq!(ThemeId::parse_with("light", false), ThemeId::Light);
        assert_eq!(ThemeId::parse_with("dark", false), ThemeId::Dark);
    }

    /// Follow Omarchy where Omarchy is installed, Follow system elsewhere
    /// (also for a saved `omarchy` and for values Kantor doesn't know).
    #[test]
    fn default_depends_on_omarchy() {
        assert_eq!(ThemeId::default_for(true), ThemeId::FollowOmarchy);
        assert_eq!(ThemeId::default_for(false), ThemeId::System);
        assert_eq!(ThemeId::parse_with("garbage", true), ThemeId::FollowOmarchy);
        assert_eq!(ThemeId::parse_with("garbage", false), ThemeId::System);
        assert_eq!(ThemeId::parse_with("", false), ThemeId::System);
        assert_eq!(ThemeId::parse_with("omarchy", true), ThemeId::FollowOmarchy);
        assert_eq!(ThemeId::parse_with("omarchy", false), ThemeId::System);
        // A named theme stays as chosen.
        assert_eq!(
            ThemeId::parse_with("ghostty:Dracula", false),
            ThemeId::Ghostty("Dracula".into())
        );
    }

    #[test]
    fn builtins_hide_follow_omarchy_without_omarchy() {
        assert_eq!(
            builtin_ids(false),
            [ThemeId::System, ThemeId::Light, ThemeId::Dark]
        );
        assert_eq!(builtin_ids(true).last(), Some(&ThemeId::FollowOmarchy));
        assert_eq!(builtin_ids(true)[0], ThemeId::System);
    }

    #[test]
    fn system_follows_the_os_appearance() {
        for omarchy in [false, true] {
            let light = resolve_with(&ThemeId::System, false, omarchy);
            let dark = resolve_with(&ThemeId::System, true, omarchy);
            assert!(!light.dark);
            assert!(dark.dark);
            assert_eq!(light, Palette::kantor_light());
            assert_eq!(dark, Palette::kantor_dark());
        }
        // Fixed choices ignore it.
        assert!(!resolve_with(&ThemeId::Light, true, false).dark);
        assert!(resolve_with(&ThemeId::Dark, false, false).dark);
        // Follow Omarchy without Omarchy acts as Follow system.
        assert!(!resolve_with(&ThemeId::FollowOmarchy, false, false).dark);
    }

    #[test]
    fn unknown_theme_falls_back() {
        let p = resolve_with(&ThemeId::Ghostty("No Such Theme".into()), false, false);
        assert_eq!(p, Palette::kantor_light());
        let p = resolve_with(&ThemeId::Omarchy("No Such Theme".into()), true, false);
        assert_eq!(p, Palette::kantor_dark());
    }

    /// Kantor Light reads well: body text 4.5:1, status colours and the
    /// accent 3:1 on white (WCAG AA for text and UI parts).
    #[test]
    fn kantor_light_contrast() {
        use crate::refs::contrast;
        for p in [Palette::kantor_light(), Palette::kantor_dark()] {
            let bg = p.background;
            assert!(contrast(p.foreground, bg) >= 4.5, "text, dark={}", p.dark);
            assert!(
                contrast(p.foreground, p.dark_background) >= 4.5,
                "text on dark_background, dark={}",
                p.dark
            );
            for (name, c) in [
                ("accent", p.accent),
                ("red", p.red),
                ("green", p.green),
                ("yellow", p.yellow),
                ("blue", p.blue),
                ("muted", p.muted),
            ] {
                let r = contrast(c, bg);
                assert!(r >= 3.0, "{name} {r:.2}, dark={}", p.dark);
            }
        }
        let light = Palette::kantor_light();
        assert!(!light.dark);
        assert!(luminance(light.background) > 0.9);
    }
}
