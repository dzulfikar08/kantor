//! Vaults: folders of notes shared by every note view opened in them.
//!
//! A [`VaultState`] entity holds the folder's link index
//! ([`notes_engine::vault::Vault`]), read in the background when the vault
//! opens and kept up to date as notes are saved, created and renamed. The
//! app keeps one per folder ([`Vaults`]); a note opened inside a known
//! vault, or inside a folder with Obsidian's `.obsidian` settings folder,
//! joins it. A note's views ([`crate::NotesView`]) register themselves so
//! renames can edit open notes through their editors (one undo step each)
//! instead of behind their backs.

use std::hash::{Hash as _, Hasher as _};
use std::path::{Path, PathBuf};

use gpui_kit::*;
use notes_engine::vault::{Splice, Vault};

use crate::NotesView;

/// One folder of notes.
pub struct VaultState {
    pub index: Vault,
    /// Still reading the folder.
    pub loading: bool,
    /// Counts changes to the index (graphs and panels rebuild on it).
    pub revision: u64,
    /// Note views open in this vault.
    views: Vec<WeakEntity<NotesView>>,
    /// The last rename, for Undo Rename.
    pub last_rename: Option<RenameUndo>,
}

/// What undoes a rename: the file's move and the link splices.
#[derive(Clone, Debug)]
pub struct RenameUndo {
    pub old_path: PathBuf,
    pub new_path: PathBuf,
    /// Per changed file (its path after the rename): the splices that put
    /// its links back, and a hash of the text they apply to.
    pub files: Vec<(PathBuf, Vec<Splice>, u64)>,
}

/// The app's open vaults.
#[derive(Default)]
pub struct Vaults(pub Vec<Entity<VaultState>>);

impl Global for Vaults {}

/// A hash of a note's text (to check a file is as a rename left it).
pub fn text_hash(text: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// `p` with symlinks and `..` resolved (as given if that fails).
pub fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// The open vault holding `path`, or the folder with Obsidian settings
/// above it (opened as a vault).
pub fn vault_for(path: &Path, cx: &mut App) -> Option<Entity<VaultState>> {
    let path = canonical(path);
    if let Some(v) = cx.try_global::<Vaults>().and_then(|vs| {
        vs.0.iter()
            .filter(|v| path.starts_with(&v.read(cx).index.root))
            // The innermost vault wins.
            .max_by_key(|v| v.read(cx).index.root.components().count())
            .cloned()
    }) {
        return Some(v);
    }
    let root = path
        .ancestors()
        .skip(1)
        .find(|d| d.join(".obsidian").is_dir())?
        .to_path_buf();
    Some(open_vault(root, cx))
}

/// The vault for folder `root` (read in the background the first time).
pub fn open_vault(root: PathBuf, cx: &mut App) -> Entity<VaultState> {
    let root = canonical(&root);
    if let Some(v) = cx
        .try_global::<Vaults>()
        .and_then(|vs| vs.0.iter().find(|v| v.read(cx).index.root == root).cloned())
    {
        return v;
    }
    let entity = cx.new(|_| VaultState {
        index: Vault::from_notes(&root, Vec::new()),
        loading: true,
        revision: 0,
        views: Vec::new(),
        last_rename: None,
    });
    cx.default_global::<Vaults>().0.push(entity.clone());
    let weak = entity.downgrade();
    let scan = cx.background_spawn(async move { Vault::scan(&root) });
    cx.spawn(async move |cx| {
        let index = scan.await;
        let _ = weak.update(cx, |v, cx| {
            v.index = index;
            v.loading = false;
            v.revision += 1;
            cx.notify();
        });
    })
    .detach();
    entity
}

impl VaultState {
    pub fn root(&self) -> &Path {
        &self.index.root
    }

    pub(crate) fn add_view(&mut self, view: WeakEntity<NotesView>) {
        self.views.retain(|v| v.upgrade().is_some());
        self.views.push(view);
    }

    /// The open view of the note at `path`, other than `skip` (a view
    /// being updated can't be read).
    pub(crate) fn view_of(
        &self,
        path: &Path,
        cx: &App,
        skip: Option<EntityId>,
    ) -> Option<Entity<NotesView>> {
        let path = canonical(path);
        self.views
            .iter()
            .filter_map(WeakEntity::upgrade)
            .filter(|v| Some(v.entity_id()) != skip)
            .find(|v| v.read(cx).path(cx).is_some_and(|p| canonical(&p) == path))
    }

    /// Vault path of `path`.
    pub fn rel(&self, path: &Path) -> Option<String> {
        notes_engine::vault::rel_path(&self.index.root, &canonical(path))
    }

    /// A note was saved (or created) with `text`.
    pub fn note_saved(&mut self, path: &Path, text: &str, bom: bool, cx: &mut Context<Self>) {
        if let Some(rel) = self.rel(path)
            && notes_engine::is_note_path(path)
        {
            self.index.upsert(&rel, text, bom);
            self.revision += 1;
            cx.notify();
        }
    }

    /// Read the whole folder again (files changed outside Kantor).
    pub fn rescan(&mut self, cx: &mut Context<Self>) {
        let root = self.index.root.clone();
        self.loading = true;
        cx.notify();
        let scan = cx.background_spawn(async move { Vault::scan(&root) });
        cx.spawn(async move |this, cx| {
            let index = scan.await;
            let _ = this.update(cx, |v, cx| {
                v.index = index;
                v.loading = false;
                v.revision += 1;
                cx.notify();
            });
        })
        .detach();
    }
}

/// Write `text` to `path` (with a byte-order mark when `bom`).
pub(crate) fn write_note(path: &Path, text: &str, bom: bool) -> anyhow::Result<()> {
    let mut bytes = Vec::with_capacity(text.len() + 3);
    if bom {
        bytes.extend_from_slice("\u{feff}".as_bytes());
    }
    bytes.extend_from_slice(text.as_bytes());
    Ok(kantor_core::fsx::write_atomic(path, &bytes)?)
}

/// A file's text and whether it had a byte-order mark.
pub(crate) fn read_note(path: &Path) -> Option<(String, bool)> {
    let bytes = std::fs::read(path).ok()?;
    let b = notes_engine::Buffer::from_bytes(&bytes).ok()?;
    Some((b.text().to_owned(), b.has_bom()))
}
