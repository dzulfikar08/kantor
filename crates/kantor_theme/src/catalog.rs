//! All themes Kantor can use, for the theme picker.
//!
//! Sources, in picker order:
//! 1. **Follow Omarchy**: whatever `omarchy theme set` selected (the default).
//! 2. **Omarchy themes** installed on this machine (`colors.toml` in
//!    `/usr/share/omarchy/themes`, `~/.local/share/omarchy/themes`, `~/.config/omarchy/themes`).
//! 3. **Ghostty themes**: 900+ schemes from iTerm2-Color-Schemes (MIT), embedded as a
//!    compact table (`assets/themes/ghostty.tsv`), the same set atur-terminal ships.

use std::path::PathBuf;

use crate::{Palette, Rgb, luminance, mix, omarchy_theme_dir, parse_hex};

const GHOSTTY_TSV: &str = include_str!("../../../assets/themes/ghostty.tsv");

/// Stable identifier of a theme, stored in settings as a string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ThemeId {
    FollowOmarchy,
    Omarchy(String),
    Ghostty(String),
}

impl ThemeId {
    /// Parse a settings value: `omarchy`, `omarchy:<name>` or `ghostty:<name>`.
    pub fn parse(s: &str) -> Self {
        if let Some(n) = s.strip_prefix("omarchy:") {
            Self::Omarchy(n.to_owned())
        } else if let Some(n) = s.strip_prefix("ghostty:") {
            Self::Ghostty(n.to_owned())
        } else {
            Self::FollowOmarchy
        }
    }

    pub fn to_setting(&self) -> String {
        match self {
            Self::FollowOmarchy => "omarchy".to_owned(),
            Self::Omarchy(n) => format!("omarchy:{n}"),
            Self::Ghostty(n) => format!("ghostty:{n}"),
        }
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

/// Every theme, in picker order.
pub fn all() -> Vec<ThemeEntry> {
    let mut v = vec![ThemeEntry {
        id: ThemeId::FollowOmarchy,
        name: "Follow Omarchy".to_owned(),
        group: Group::System,
        palette: Palette::current(),
    }];
    v.extend(omarchy_themes());
    v.extend(ghostty_themes());
    v
}

/// Resolve a theme id to a palette, falling back to the Omarchy theme.
pub fn resolve(id: &ThemeId) -> Palette {
    match id {
        ThemeId::FollowOmarchy => Palette::current(),
        ThemeId::Omarchy(name) => omarchy_themes()
            .into_iter()
            .find(|e| &e.name == name)
            .map(|e| e.palette)
            .unwrap_or_else(Palette::current),
        ThemeId::Ghostty(name) => ghostty_themes()
            .into_iter()
            .find(|e| &e.name == name)
            .map(|e| e.palette)
            .unwrap_or_else(Palette::current),
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
            ThemeId::FollowOmarchy,
            ThemeId::Omarchy("nord".into()),
            ThemeId::Ghostty("Tokyo Night".into()),
        ] {
            assert_eq!(ThemeId::parse(&id.to_setting()), id);
        }
        assert_eq!(ThemeId::parse("garbage"), ThemeId::FollowOmarchy);
    }

    #[test]
    fn unknown_theme_falls_back() {
        let p = resolve(&ThemeId::Ghostty("No Such Theme".into()));
        assert_eq!(p, Palette::current());
    }
}
