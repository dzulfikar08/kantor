//! The saved workspace: open tabs (with their split layouts), panel sizes.
//!
//! Stored as JSON in `$XDG_STATE_HOME/kantor/workspace.json`. Written atomically on
//! quit; a missing or corrupt file means "start fresh" (it is not overwritten
//! until the next successful save).

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::layout::{Layout, PaneId};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiState {
    pub sidebar_open: bool,
    pub sidebar_width: f32,
    pub inspector_open: bool,
    pub inspector_width: f32,
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            sidebar_open: false,
            sidebar_width: 248.,
            inspector_open: false,
            inspector_width: 300.,
        }
    }
}

impl UiState {
    pub const MIN_PANEL: f32 = 180.;
    pub const MAX_PANEL: f32 = 520.;

    pub fn clamp(w: f32) -> f32 {
        w.clamp(Self::MIN_PANEL, Self::MAX_PANEL)
    }
}

/// One pane: the file it shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PaneState {
    pub id: PaneId,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabState {
    pub layout: Layout,
    pub focused: PaneId,
    pub panes: Vec<PaneState>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub tabs: Vec<TabState>,
    /// Index into `tabs`; `None` = Home.
    pub active: Option<usize>,
    pub ui: UiState,
    /// Most recently opened files, newest first.
    pub recent: Vec<PathBuf>,
}

pub const MAX_RECENT: usize = 12;

/// `$XDG_STATE_HOME/kantor/workspace.json` (default `~/.local/state/kantor/workspace.json`).
pub fn session_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("kantor/workspace.json"))
}

impl Session {
    pub fn load_from(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn load() -> Self {
        session_path()
            .map(|p| Self::load_from(&p))
            .unwrap_or_default()
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&session_path().context("no home directory")?)
    }

    /// Record `path` as most recently opened.
    pub fn touch_recent(recent: &mut Vec<PathBuf>, path: &Path) {
        recent.retain(|p| p != path);
        recent.insert(0, path.to_path_buf());
        recent.truncate(MAX_RECENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Axis;

    #[test]
    fn round_trip_and_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/workspace.json");
        assert_eq!(Session::load_from(&path), Session::default());

        let layout = Layout::Leaf(1).split(1, 2, Axis::Horizontal).unwrap();
        let s = Session {
            tabs: vec![TabState {
                layout,
                focused: 2,
                panes: vec![
                    PaneState {
                        id: 1,
                        path: "/a.xlsx".into(),
                    },
                    PaneState {
                        id: 2,
                        path: "/b.xlsx".into(),
                    },
                ],
            }],
            active: Some(0),
            ui: UiState {
                sidebar_open: true,
                ..UiState::default()
            },
            recent: vec!["/a.xlsx".into()],
        };
        s.save_to(&path).unwrap();
        assert_eq!(Session::load_from(&path), s);

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Session::load_from(&path), Session::default());
    }

    #[test]
    fn recent_is_deduplicated_and_bounded() {
        let mut r = Vec::new();
        for i in 0..20 {
            Session::touch_recent(&mut r, Path::new(&format!("/f{i}.xlsx")));
        }
        Session::touch_recent(&mut r, Path::new("/f15.xlsx"));
        assert_eq!(r.len(), MAX_RECENT);
        assert_eq!(r[0], PathBuf::from("/f15.xlsx"));
        assert_eq!(r.iter().filter(|p| p.ends_with("f15.xlsx")).count(), 1);
    }
}
