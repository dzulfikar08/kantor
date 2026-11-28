//! The vault around a note: the files (a folder tree) and search on the
//! left, backlinks on the right, wiki-link following, new notes, and
//! renaming a note with every link to it updated.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::{IconButtonExt as _, icon_button, section_label};
use kantor_ui::i18n::{t, tf};
use kantor_ui::{metrics::*, tokens};
use notes_engine::vault::{self as nv, Splice};
use notes_engine::{Merge, Selection};

use crate::vault::{self, RenameUndo, VaultState};
use crate::{Jump, NotesEvent, NotesView};

/// What the left panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeftPanel {
    Hidden,
    Files,
    Search,
}

pub(crate) const LEFT_W: Pixels = px(240.);
pub(crate) const RIGHT_W: Pixels = px(280.);
/// Search hits listed at most.
const MAX_HITS: usize = 300;
/// Unlinked mentions listed at most.
const MAX_UNLINKED: usize = 100;

/// A line of `text` around `range`, cut to about `max` bytes: the snippet
/// and where the hit is in it.
pub(crate) fn snippet(text: &str, range: Range<usize>, max: usize) -> (String, Range<usize>) {
    let range = range.start.min(text.len())..range.end.min(text.len());
    let ls = text[..range.start].rfind('\n').map_or(0, |i| i + 1);
    let le = text[range.end..]
        .find('\n')
        .map_or(text.len(), |i| range.end + i);
    let line = text[ls..le].trim_end_matches('\r');
    let (a, b) = (
        (range.start - ls).min(line.len()),
        (range.end - ls).min(line.len()),
    );
    let mut s = a.saturating_sub(max / 3);
    while s > 0 && !line.is_char_boundary(s) {
        s -= 1;
    }
    // Leading spaces and list markers aren't worth the room.
    if s == 0 {
        s = (line.len() - line.trim_start().len()).min(a);
    }
    let mut e = (s + max).min(line.len()).max(b);
    while e < line.len() && !line.is_char_boundary(e) {
        e += 1;
    }
    let pre = if s > 0 { "…" } else { "" };
    let post = if e < line.len() { "…" } else { "" };
    let out = format!("{pre}{}{post}", &line[s..e]);
    let o = pre.len();
    (out, o + a - s..o + b - s)
}

/// A snippet with its hit highlighted.
fn snippet_text(text: String, hit: Range<usize>, color: Hsla) -> StyledText {
    let hit = if hit.end <= text.len() && hit.start < hit.end {
        vec![(
            hit,
            HighlightStyle {
                background_color: Some(color),
                ..Default::default()
            },
        )]
    } else {
        Vec::new()
    };
    StyledText::new(text).with_highlights(hit)
}

/// One hit or mention, ready to draw.
#[derive(Clone)]
pub(crate) struct Row {
    pub path: PathBuf,
    pub offset: usize,
    pub text: String,
    pub hit: Range<usize>,
}

/// Hits grouped by note: (note name, path, rows).
pub(crate) type Groups = Vec<(String, PathBuf, Vec<Row>)>;

fn group(v: &nv::Vault, items: impl Iterator<Item = (usize, Range<usize>)>) -> Groups {
    let mut out: Groups = Vec::new();
    let mut at: HashMap<usize, usize> = HashMap::new();
    for (note, range) in items {
        let n = &v.notes[note];
        let (text, hit) = snippet(&n.text, range.clone(), 110);
        let row = Row {
            path: v.path_of(note),
            offset: range.start,
            text,
            hit,
        };
        match at.get(&note) {
            Some(&i) => out[i].2.push(row),
            None => {
                at.insert(note, out.len());
                out.push((n.stem().to_owned(), v.path_of(note), vec![row]));
            }
        }
    }
    out
}

/// What the backlinks panel shows for one note (cached per vault revision).
pub(crate) struct LinksData {
    pub key: (u64, String),
    pub linked: Groups,
    pub linked_count: usize,
    pub unlinked: Groups,
    pub unlinked_count: usize,
    /// Links out of the note that go nowhere.
    pub broken: Vec<String>,
}

/// Search results (cached per vault revision and query).
pub(crate) struct SearchData {
    pub key: (u64, String),
    pub files: Vec<(String, PathBuf)>,
    pub groups: Groups,
    pub count: usize,
}

/// What renaming a note changes.
pub(crate) struct RenamePlan {
    pub old_path: PathBuf,
    pub new_path: PathBuf,
    pub old_rel: String,
    pub new_rel: String,
    /// Files whose links change (the note itself included when it links
    /// to itself): their current text and the splices.
    pub files: Vec<(PathBuf, String, bool, Vec<Splice>)>,
}

impl NotesView {
    // ── the vault ──────────────────────────────────────────────────────────

    /// Join `vault`: the panels show it, saves keep its index current.
    pub(crate) fn attach_vault(&mut self, v: Entity<VaultState>, cx: &mut Context<Self>) {
        if self.vault.as_ref() == Some(&v) {
            return;
        }
        let me = cx.entity().downgrade();
        v.update(cx, |s, _| s.add_view(me));
        self._vault_sub = Some(cx.observe(&v, |this, _, cx| {
            this.links_cache = None;
            this.search_cache = None;
            cx.notify();
        }));
        self.vault = Some(v);
        if self.left == LeftPanel::Hidden {
            self.left = LeftPanel::Files;
        }
        cx.notify();
    }

    /// The note's vault, opening its folder as one if it has none.
    pub(crate) fn ensure_vault(&mut self, cx: &mut Context<Self>) -> bool {
        if self.vault.is_some() {
            return true;
        }
        let Some(dir) = self.dir().map(Path::to_path_buf) else {
            self.status = t("Save the note first: a vault is the note's folder").into();
            cx.notify();
            return false;
        };
        let v = vault::open_vault(dir, cx);
        self.attach_vault(v, cx);
        true
    }

    /// Open folder `root` as this note's vault.
    pub fn open_vault_folder(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        let v = vault::open_vault(root, cx);
        self.vault = None;
        self.attach_vault(v, cx);
    }

    pub fn vault_root(&self, cx: &App) -> Option<PathBuf> {
        Some(self.vault.as_ref()?.read(cx).root().to_path_buf())
    }

    pub fn toggle_files(&mut self, cx: &mut Context<Self>) {
        if !self.ensure_vault(cx) {
            return;
        }
        self.left = if self.left == LeftPanel::Files {
            LeftPanel::Hidden
        } else {
            LeftPanel::Files
        };
        cx.notify();
    }

    /// Show the search pane and put the caret in its field.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ensure_vault(cx) {
            return;
        }
        self.left = LeftPanel::Search;
        let input = match &self.search_input {
            Some(i) => i.clone(),
            None => {
                let i =
                    cx.new(|cx| InputState::new(window, cx).placeholder(t("Search in the vault")));
                let sub = cx.subscribe_in(&i, window, |this, _, ev: &InputEvent, _, cx| {
                    if matches!(ev, InputEvent::Change) {
                        cx.notify();
                    }
                    let _ = this;
                });
                self._search_sub = Some(sub);
                self.search_input = Some(i.clone());
                i
            }
        };
        input.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    pub fn toggle_backlinks(&mut self, cx: &mut Context<Self>) {
        if !self.ensure_vault(cx) {
            return;
        }
        self.show_backlinks = !self.show_backlinks;
        cx.notify();
    }

    /// Go to `path` (at `jump`): in this view when it is this note, else
    /// in its tab.
    pub(crate) fn open_target(
        &mut self,
        path: PathBuf,
        jump: Option<Jump>,
        cx: &mut Context<Self>,
    ) {
        let same = self
            .path
            .as_deref()
            .is_some_and(|p| vault::canonical(p) == vault::canonical(&path));
        if same {
            if let Some(j) = jump {
                self.jump(&j, cx);
            }
            return;
        }
        cx.emit(match jump {
            Some(j) => NotesEvent::OpenAt(path, j),
            None => NotesEvent::Open(path),
        });
    }

    /// Follow `[[target#heading]]`: the note it names (created when there
    /// is none yet), at the heading.
    pub(crate) fn follow_wiki(
        &mut self,
        target: &str,
        heading: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let jump = heading
            .filter(|h| !h.trim().is_empty())
            .map(|h| Jump::Heading(h.trim().to_owned()));
        if target.trim().is_empty() {
            if let Some(j) = jump {
                self.jump(&j, cx);
            }
            return;
        }
        if let Some(p) = self.resolve_wiki(target, cx) {
            self.open_target(p, jump, cx);
            return;
        }
        // A link to a note that doesn't exist yet makes it (as Obsidian).
        let Some(dir) = self.new_note_dir(target, cx) else {
            self.status = t("Save the note first: a vault is the note's folder").into();
            cx.notify();
            return;
        };
        let name = target.rsplit(['/', '\\']).next().unwrap_or(target);
        let path = dir.join(nv::note_file_name(name));
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, b"")) {
            self.status = tf("Couldn't make the note: {}", &[&e]).into();
            cx.notify();
            return;
        }
        if let Some(v) = &self.vault {
            v.update(cx, |s, cx| s.note_saved(&path, "", false, cx));
        }
        self.folder_index = None;
        self.status = tf(
            "Made the new note {}",
            &[&path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()],
        )
        .into();
        self.open_target(path, None, cx);
    }

    /// Where a new note named `target` goes: under the vault's root for a
    /// path (`Folder/Note`), else in this note's folder.
    fn new_note_dir(&self, target: &str, cx: &App) -> Option<PathBuf> {
        let here = self.dir().map(Path::to_path_buf);
        let target = target.replace('\\', "/");
        match target.rsplit_once('/') {
            Some((folder, _)) => {
                let root = self.vault_root(cx).or(here)?;
                let folder: PathBuf = folder
                    .split('/')
                    .filter(|s| !s.is_empty() && *s != "." && *s != "..")
                    .collect();
                Some(root.join(folder))
            }
            None => here,
        }
    }

    /// The file `[[target]]` goes to, if it exists.
    pub(crate) fn resolve_wiki(&mut self, target: &str, cx: &App) -> Option<PathBuf> {
        let rel = self.my_rel(cx);
        if let Some(s) = self.vault.as_ref().map(|v| v.read(cx))
            && !s.loading
        {
            return s
                .index
                .resolve(rel.as_deref(), target)
                .map(|i| s.index.path_of(i));
        }
        // No vault (or still reading it): the note's folder.
        let idx = self.folder_index()?;
        idx.resolve(
            rel.as_deref().map(|r| r.rsplit('/').next().unwrap_or(r)),
            target,
        )
        .map(|i| idx.path_of(i))
    }

    /// This note's vault path.
    pub(crate) fn my_rel(&self, cx: &App) -> Option<String> {
        let p = self.path.as_deref()?;
        match &self.vault {
            Some(v) => v.read(cx).rel(p),
            None => p.file_name().map(|n| n.to_string_lossy().into_owned()),
        }
    }

    /// The notes in this note's folder (names only), for links when the
    /// note is in no vault. Read again every few seconds.
    pub(crate) fn folder_index(&mut self) -> Option<Rc<nv::Vault>> {
        let dir = self.dir()?.to_path_buf();
        if let Some((at, idx)) = &self.folder_index
            && at.elapsed() < std::time::Duration::from_secs(5)
        {
            return Some(idx.clone());
        }
        let names: Vec<(String, String)> = std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .filter(|e| notes_engine::is_note_path(&e.path()))
            .map(|e| (e.file_name().to_string_lossy().into_owned(), String::new()))
            .collect();
        let idx = Rc::new(nv::Vault::from_notes(&dir, names));
        self.folder_index = Some((std::time::Instant::now(), idx.clone()));
        Some(idx)
    }

    /// A new, empty note in this note's folder (or the vault's root).
    pub fn new_vault_note(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self
            .dir()
            .map(Path::to_path_buf)
            .or_else(|| self.vault_root(cx))
        else {
            return;
        };
        let path = nv::unique_path(&dir, &format!("{}.md", t("Untitled")));
        if let Err(e) = std::fs::write(&path, b"") {
            self.status = tf("Couldn't make the note: {}", &[&e]).into();
            cx.notify();
            return;
        }
        if let Some(v) = &self.vault {
            v.update(cx, |s, cx| s.note_saved(&path, "", false, cx));
        }
        cx.emit(NotesEvent::Open(path));
    }

    /// After a save: the vault's index takes the new text.
    pub(crate) fn saved_to_vault(&mut self, cx: &mut Context<Self>) {
        self.folder_index = None;
        // Saved into an open vault for the first time (Save As).
        if self.vault.is_none()
            && let Some(p) = self.path.clone()
            && let Some(v) = vault::vault_for(&p, cx)
        {
            self.attach_vault(v, cx);
        }
        let (Some(v), Some(p)) = (self.vault.clone(), self.path.clone()) else {
            return;
        };
        let text = self.editor.text().to_owned();
        let bom = self.editor.buffer().has_bom();
        v.update(cx, |s, cx| s.note_saved(&p, &text, bom, cx));
    }

    // ── renaming ───────────────────────────────────────────────────────────

    /// What renaming this note to `name` would change.
    pub(crate) fn plan_rename(&self, name: &str, cx: &App) -> Result<RenamePlan, String> {
        let old_path = self
            .path
            .clone()
            .ok_or_else(|| t("Save the note first").to_owned())?;
        let v = self
            .vault
            .as_ref()
            .ok_or_else(|| t("Save the note first").to_owned())?
            .read(cx);
        if v.loading {
            return Err(t("Still reading the vault…").to_owned());
        }
        let file = nv::note_file_name(name);
        let new_path = old_path.with_file_name(&file);
        if new_path == old_path {
            return Err(String::new());
        }
        let same_file = new_path
            .to_string_lossy()
            .eq_ignore_ascii_case(&old_path.to_string_lossy());
        if new_path.exists() && !same_file {
            return Err(tf("There is already a note named {}", &[&file]));
        }
        let old_rel = v
            .rel(&old_path)
            .ok_or_else(|| t("Save the note first").to_owned())?;
        let new_rel = match old_rel.rsplit_once('/') {
            Some((folder, _)) => format!("{folder}/{file}"),
            None => file.clone(),
        };
        let mut files = Vec::new();
        let Some(me) = v.index.find(&old_rel) else {
            // Not indexed yet (a brand-new note): nothing links to it.
            return Ok(RenamePlan {
                old_path,
                new_path,
                old_rel,
                new_rel,
                files,
            });
        };
        let mut linking: Vec<usize> = v.index.backlinks(me).iter().map(|m| m.note).collect();
        linking.push(me);
        linking.sort_unstable();
        linking.dedup();
        for i in linking {
            let path = v.index.path_of(i);
            let rel = &v.index.notes[i].rel;
            // An open note's own text (with its unsaved edits), else the file.
            let (text, bom) = if i == me {
                (
                    self.editor.text().to_owned(),
                    self.editor.buffer().has_bom(),
                )
            } else if let Some(view) = v.view_of(&path, cx, Some(cx_entity_id(self))) {
                let e = &view.read(cx).editor;
                (e.text().to_owned(), e.buffer().has_bom())
            } else {
                vault::read_note(&path)
                    .unwrap_or_else(|| (v.index.notes[i].text.clone(), v.index.notes[i].bom))
            };
            let sp = v.index.rename_splices(me, &new_rel, rel, &text);
            if !sp.is_empty() {
                files.push((path, text, bom, sp));
            }
        }
        Ok(RenamePlan {
            old_path,
            new_path,
            old_rel,
            new_rel,
            files,
        })
    }

    /// Ask for the new name (showing how many notes' links change), then
    /// rename.
    pub fn rename_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ensure_vault(cx) || self.path.is_none() {
            return;
        }
        if self.editable() && self.editor.is_dirty() {
            self.save_doc(window, cx);
        }
        let stem = self
            .path
            .as_deref()
            .and_then(Path::file_stem)
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(stem.clone()));
        let preview: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let view = cx.entity().downgrade();
        let update_preview = {
            let (view, preview, input) = (view.clone(), preview.clone(), input.clone());
            move |cx: &mut App| {
                let name = input.read(cx).value().to_string();
                let text = view
                    .upgrade()
                    .map(|v| match v.read(cx).plan_rename(&name, cx) {
                        Ok(p) => rename_summary(&p),
                        Err(e) => e,
                    })
                    .unwrap_or_default();
                *preview.borrow_mut() = text;
            }
        };
        // (This view is being updated: plan here, not through the handle.)
        *preview.borrow_mut() = match self.plan_rename(&stem, cx) {
            Ok(p) => rename_summary(&p),
            Err(e) => e,
        };
        let submit: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new({
            let (view, input) = (view.clone(), input.clone());
            move |window, cx| {
                let name = input.read(cx).value().to_string();
                window.close_dialog(cx);
                let _ = view.update(cx, |this, cx| this.do_rename(&name, cx));
            }
        });
        let sub = window.subscribe(&input, cx, {
            let (submit, update_preview) = (submit.clone(), update_preview.clone());
            move |_, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => submit(window, cx),
                InputEvent::Change => {
                    update_preview(cx);
                    window.refresh();
                }
                _ => {}
            }
        });
        input.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        let title: SharedString = t("Rename note").into();
        window.open_dialog(cx, move |dialog, _, _| {
            let _keep = &sub;
            let ok = {
                let submit = submit.clone();
                move |_: &ClickEvent, window: &mut Window, cx: &mut App| submit(window, cx)
            };
            dialog
                .w(px(440.))
                .title(title.clone())
                .child(
                    v_flex()
                        .gap_3()
                        .child(Input::new(&input).cleanable(false))
                        .child(DialogDescription::new().child(preview.borrow().clone())),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("notes-rename-cancel")
                                .label(t("Cancel"))
                                .ghost()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("notes-rename-ok")
                                .label(t("Rename"))
                                .primary()
                                .on_click(ok),
                        ),
                )
        });
    }

    /// Rename this note to `name` and update every link to it.
    pub fn do_rename(&mut self, name: &str, cx: &mut Context<Self>) {
        let plan = match self.plan_rename(name, cx) {
            Ok(p) => p,
            Err(e) => {
                if !e.is_empty() {
                    self.status = e.into();
                    cx.notify();
                }
                return;
            }
        };
        let Some(v) = self.vault.clone() else {
            return;
        };
        let me = cx.entity_id();
        let mut undo = Vec::new();
        let mut failed = Vec::new();
        for (path, old_text, bom, splices) in &plan.files {
            let is_me = *path == plan.old_path;
            let new_text = nv::apply_splices(old_text, splices);
            let result = if is_me {
                self.apply_link_splices(splices, cx);
                self.disk_guard = None;
                self.write_to(path)
            } else if let Some(view) = v.read(cx).view_of(path, cx, Some(me)) {
                view.update(cx, |w, cx| {
                    w.apply_link_splices(splices, cx);
                    w.disk_guard = None;
                    let r = w.write_to(path);
                    cx.notify();
                    r
                })
            } else {
                vault::write_note(path, &new_text, *bom)
            };
            match result {
                Ok(()) => {
                    let after = if is_me {
                        plan.new_path.clone()
                    } else {
                        path.clone()
                    };
                    undo.push((
                        after,
                        nv::inverse_splices(old_text, splices),
                        vault::text_hash(&new_text),
                    ));
                    v.update(cx, |s, cx| s.note_saved(path, &new_text, *bom, cx));
                }
                Err(e) => failed.push(format!(
                    "{}: {e:#}",
                    path.file_name()
                        .map(|n| n.to_string_lossy())
                        .unwrap_or_default()
                )),
            }
        }
        if let Err(e) = std::fs::rename(&plan.old_path, &plan.new_path) {
            self.status = tf("Couldn't rename the note: {}", &[&e]).into();
            cx.notify();
            return;
        }
        self.moved_to(plan.new_path.clone());
        v.update(cx, |s, cx| {
            s.index.move_note(&plan.old_rel, &plan.new_rel);
            s.revision += 1;
            s.last_rename = Some(RenameUndo {
                old_path: plan.old_path.clone(),
                new_path: plan.new_path.clone(),
                files: undo,
            });
            cx.notify();
        });
        let others = plan.files.iter().filter(|f| f.0 != plan.old_path).count();
        self.status = if failed.is_empty() {
            tf(
                "Renamed to {}; links updated in {} other notes",
                &[&nv::stem_of(&plan.new_rel), &others],
            )
            .into()
        } else {
            tf(
                "Renamed, but some links couldn't be updated: {}",
                &[&failed.join("; ")],
            )
            .into()
        };
        cx.notify();
    }

    /// Put back the vault's last rename (file name and links), where the
    /// files are still as the rename left them.
    pub fn undo_rename(&mut self, cx: &mut Context<Self>) {
        let Some(v) = self.vault.clone() else {
            return;
        };
        let Some(u) = v.update(cx, |s, _| s.last_rename.take()) else {
            self.status = t("No rename to undo").into();
            cx.notify();
            return;
        };
        if u.old_path.exists() || !u.new_path.exists() {
            self.status = t("Can't undo the rename: the files changed since").into();
            cx.notify();
            return;
        }
        if let Err(e) = std::fs::rename(&u.new_path, &u.old_path) {
            self.status = tf("Couldn't rename the note: {}", &[&e]).into();
            cx.notify();
            return;
        }
        let me = cx.entity_id();
        let mine = self
            .path
            .as_deref()
            .is_some_and(|p| vault::canonical(p) == vault::canonical(&u.new_path));
        if mine {
            self.moved_to(u.old_path.clone());
        } else if let Some(view) = v.read(cx).view_of(&u.new_path, cx, Some(me)) {
            view.update(cx, |w, _| w.moved_to(u.old_path.clone()));
        }
        let mut skipped = 0;
        for (after, inverse, hash) in &u.files {
            let path = if *after == u.new_path {
                u.old_path.clone()
            } else {
                after.clone()
            };
            let is_me = self
                .path
                .as_deref()
                .is_some_and(|p| vault::canonical(p) == vault::canonical(&path));
            let done = if is_me {
                if vault::text_hash(self.editor.text()) == *hash {
                    self.apply_link_splices(inverse, cx);
                    self.write_to(&path).is_ok()
                } else {
                    false
                }
            } else if let Some(view) = v.read(cx).view_of(&path, cx, Some(me)) {
                view.update(cx, |w, cx| {
                    if vault::text_hash(w.editor.text()) == *hash {
                        w.apply_link_splices(inverse, cx);
                        w.write_to(&path).is_ok()
                    } else {
                        false
                    }
                })
            } else {
                match vault::read_note(&path) {
                    Some((text, bom)) if vault::text_hash(&text) == *hash => {
                        vault::write_note(&path, &nv::apply_splices(&text, inverse), bom).is_ok()
                    }
                    _ => false,
                }
            };
            if !done {
                skipped += 1;
            }
        }
        v.update(cx, |s, cx| s.rescan(cx));
        self.status = if skipped == 0 {
            t("Rename undone").into()
        } else {
            tf(
                "Rename undone; {} notes had changed and kept their links",
                &[&skipped],
            )
            .into()
        };
        cx.notify();
    }

    /// Splice link text into the note (one undo step), keeping the
    /// selection on the same text.
    pub(crate) fn apply_link_splices(&mut self, splices: &[Splice], cx: &mut Context<Self>) {
        if splices.is_empty() {
            return;
        }
        let mut sorted = splices.to_vec();
        sorted.sort_by_key(|s| s.range.start);
        let sel = self.editor.selection();
        let after = Selection::new(
            nv::map_offset(sel.anchor, &sorted),
            nv::map_offset(sel.head, &sorted),
        );
        let edits = sorted
            .iter()
            .map(|s| (s.range.clone(), s.text.clone()))
            .collect();
        self.editor.edit(edits, Some(after), Merge::No);
        self.marked = None;
        self.refresh_find(cx);
        cx.notify();
    }

    /// The note's file moved (renamed): show and save it there.
    pub(crate) fn moved_to(&mut self, path: PathBuf) {
        self.path = Some(path);
        self.disk_guard = None;
        self.links_cache = None;
    }

    // ── drawing ────────────────────────────────────────────────────────────

    pub(crate) fn render_left(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.left == LeftPanel::Hidden {
            return None;
        }
        let v = self.vault.clone()?;
        let t = tokens(cx);
        let (root_name, loading, count, has_undo) = {
            let s = v.read(cx);
            (
                s.root()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                s.loading,
                s.index.len(),
                s.last_rename.is_some(),
            )
        };
        let tab = |id: &'static str,
                   label: &'static str,
                   on: bool,
                   which: LeftPanel,
                   cx: &mut Context<Self>| {
            Button::new(id)
                .label(label)
                .xsmall()
                .when(on, |b| b.primary())
                .when(!on, |b| b.ghost())
                .on_click(cx.listener(move |this, _, window, cx| match which {
                    LeftPanel::Search => this.open_search(window, cx),
                    _ => {
                        this.left = which;
                        cx.notify();
                    }
                }))
        };
        let header =
            v_flex()
                .flex_none()
                .gap(SPACE_2)
                .px(SPACE_4)
                .pt(SPACE_4)
                .pb(SPACE_3)
                .border_b_1()
                .border_color(t.border)
                .child(
                    h_flex()
                        .gap(SPACE_2)
                        .child(
                            gpui_kit::component::Icon::new(Lucide::FolderOpen)
                                .with_size(ICON_MD)
                                .text_color(t.fg_muted),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(root_name),
                        )
                        .child(
                            icon_button("notes-vault-new", Lucide::FilePlus, false, cx)
                                .tip(t("New note"), None)
                                .on_click(cx.listener(|this, _, _, cx| this.new_vault_note(cx))),
                        )
                        .child(
                            icon_button("notes-vault-rename", Lucide::Pencil, false, cx)
                                .tip(t("Rename note"), None)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.rename_note(window, cx)),
                                ),
                        )
                        .child(
                            icon_button("notes-vault-rescan", Lucide::RefreshCw, false, cx)
                                .tip(t("Read the folder again"), None)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(v) = &this.vault {
                                        v.update(cx, |s, cx| s.rescan(cx));
                                    }
                                })),
                        ),
                )
                .child(
                    h_flex()
                        .gap(SPACE_2)
                        .child(tab(
                            "notes-tab-files",
                            t("Files"),
                            self.left == LeftPanel::Files,
                            LeftPanel::Files,
                            cx,
                        ))
                        .child(tab(
                            "notes-tab-search",
                            t("Search"),
                            self.left == LeftPanel::Search,
                            LeftPanel::Search,
                            cx,
                        ))
                        .child(div().flex_1())
                        .child(div().text_size(FONT_CAPTION).text_color(t.fg_dim).child(
                            if loading {
                                t("Reading…").to_owned()
                            } else {
                                tf("{} notes", &[&count])
                            },
                        )),
                );
        let body = match self.left {
            LeftPanel::Search => self.search_pane(&v, cx),
            _ => self.files_pane(&v, cx),
        };
        let undo = has_undo.then(|| {
            div()
                .flex_none()
                .p(SPACE_3)
                .border_t_1()
                .border_color(t.border)
                .child(
                    Button::new("notes-undo-rename")
                        .label(t("Undo rename"))
                        .icon(Lucide::Undo2)
                        .ghost()
                        .xsmall()
                        .on_click(cx.listener(|this, _, _, cx| this.undo_rename(cx))),
                )
        });
        Some(
            v_flex()
                .flex_none()
                .w(LEFT_W)
                .h_full()
                .min_h_0()
                .bg(t.chrome)
                .border_r_1()
                .border_color(t.border)
                .text_size(FONT_BODY)
                .child(header)
                .child(body)
                .children(undo)
                .into_any_element(),
        )
    }

    fn files_pane(&mut self, v: &Entity<VaultState>, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens(cx);
        let current = self.path.as_deref().and_then(|p| v.read(cx).rel(p));
        // (folder rows and note rows), collected before drawing.
        enum Item {
            Folder {
                key: String,
                name: String,
                depth: usize,
                open: bool,
            },
            Note {
                rel: String,
                name: String,
                depth: usize,
                path: PathBuf,
            },
        }
        let mut items = Vec::new();
        {
            let s = v.read(cx);
            let mut emitted: Vec<String> = Vec::new();
            for (i, n) in s.index.notes.iter().enumerate() {
                let parts: Vec<&str> = n.rel.split('/').collect();
                let folders = &parts[..parts.len() - 1];
                for d in 0..folders.len() {
                    let key = folders[..=d].join("/");
                    if emitted.get(d) != Some(&key) {
                        emitted.truncate(d);
                        emitted.push(key.clone());
                        if !self.folder_hidden(&folders[..d]) {
                            items.push(Item::Folder {
                                open: !self.collapsed.contains(&key),
                                key,
                                name: folders[d].to_owned(),
                                depth: d,
                            });
                        }
                    }
                }
                emitted.truncate(folders.len());
                if self.folder_hidden(folders) {
                    continue;
                }
                items.push(Item::Note {
                    rel: n.rel.clone(),
                    name: n.stem().to_owned(),
                    depth: folders.len(),
                    path: s.index.path_of(i),
                });
            }
        }
        let mut list = v_flex().py(SPACE_2);
        for it in items {
            match it {
                Item::Folder {
                    key,
                    name,
                    depth,
                    open,
                } => {
                    let k = key.clone();
                    list = list.child(
                        h_flex()
                            .id(SharedString::from(format!("notes-folder-{key}")))
                            .h(ROW_HEIGHT - px(4.))
                            .pl(SPACE_4 + px(14.) * depth as f32)
                            .pr(SPACE_4)
                            .gap(SPACE_2)
                            .cursor_pointer()
                            .text_color(t.fg_muted)
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.collapsed.remove(&k) {
                                    this.collapsed.insert(k.clone());
                                }
                                cx.notify();
                            }))
                            .child(
                                gpui_kit::component::Icon::new(if open {
                                    Lucide::ChevronDown
                                } else {
                                    Lucide::ChevronRight
                                })
                                .with_size(ICON_SM),
                            )
                            .child(div().truncate().child(name)),
                    );
                }
                Item::Note {
                    rel,
                    name,
                    depth,
                    path,
                } => {
                    let here = current.as_deref() == Some(rel.as_str());
                    list = list.child(
                        h_flex()
                            .id(SharedString::from(format!("notes-file-{rel}")))
                            .h(ROW_HEIGHT - px(4.))
                            .pl(SPACE_4 + px(14.) * depth as f32 + px(16.))
                            .pr(SPACE_4)
                            .gap(SPACE_2)
                            .cursor_pointer()
                            .when(here, |d| d.bg(t.selected).text_color(t.fg))
                            .when(!here, |d| d.text_color(t.fg_muted).hover(|s| s.bg(t.hover)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_target(path.clone(), None, cx)
                            }))
                            .child(div().truncate().child(name)),
                    );
                }
            }
        }
        div()
            .id("notes-files")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(list)
            .into_any_element()
    }

    fn folder_hidden(&self, folders: &[&str]) -> bool {
        (1..=folders.len()).any(|k| self.collapsed.contains(&folders[..k].join("/")))
    }

    fn search_pane(&mut self, v: &Entity<VaultState>, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens(cx);
        let query = self
            .search_input
            .as_ref()
            .map(|i| i.read(cx).value().to_string())
            .unwrap_or_default();
        let rev = v.read(cx).revision;
        let key = (rev, query.clone());
        if self.search_cache.as_ref().is_none_or(|c| c.key != key) {
            let s = v.read(cx);
            let hits = s.index.search(&query, MAX_HITS);
            let count = hits.len();
            let groups = group(&s.index, hits.into_iter().map(|h| (h.note, h.range)));
            let files = s
                .index
                .search_names(&query)
                .into_iter()
                .take(30)
                .map(|i| (s.index.notes[i].rel.clone(), s.index.path_of(i)))
                .collect();
            self.search_cache = Some(SearchData {
                key,
                files,
                groups,
                count,
            });
        }
        let mut col = v_flex().gap(SPACE_3).p(SPACE_4);
        if let Some(input) = &self.search_input {
            col = col.child(Input::new(input).small().cleanable(true));
        }
        let data = self.search_cache.as_ref().expect("filled above");
        if !query.trim().is_empty() {
            col = col.child(div().text_size(FONT_CAPTION).text_color(t.fg_dim).child(
                if data.count >= MAX_HITS {
                    tf("{}+ matches", &[&MAX_HITS])
                } else {
                    tf("{} matches", &[&data.count])
                },
            ));
        }
        if !data.files.is_empty() {
            let mut files = v_flex().gap(SPACE_1).child(section_label(t("Files"), cx));
            for (rel, path) in data.files.clone() {
                files = files.child(
                    div()
                        .id(SharedString::from(format!("notes-search-file-{rel}")))
                        .px(SPACE_2)
                        .rounded(RADIUS_SM)
                        .cursor_pointer()
                        .truncate()
                        .text_color(t.accent)
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_target(path.clone(), None, cx)
                        }))
                        .child(rel),
                );
            }
            col = col.child(files);
        }
        let groups = data.groups.clone();
        col = col.child(self.groups_list("search", groups, cx));
        div()
            .id("notes-search")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(col)
            .into_any_element()
    }

    /// Hits grouped by note: the note's name, then a line per hit.
    fn groups_list(&mut self, id: &str, groups: Groups, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let mut col = v_flex().gap(SPACE_3);
        for (gi, (name, path, rows)) in groups.into_iter().enumerate() {
            let p = path.clone();
            let mut g = v_flex().gap(SPACE_1).child(
                div()
                    .id(SharedString::from(format!("notes-{id}-note-{gi}")))
                    .font_weight(FontWeight::SEMIBOLD)
                    .truncate()
                    .cursor_pointer()
                    .hover(|s| s.text_color(t.accent))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_target(p.clone(), None, cx)),
                    )
                    .child(name),
            );
            for (ri, r) in rows.into_iter().enumerate() {
                let (path, offset) = (r.path.clone(), r.offset);
                g = g.child(
                    div()
                        .id(SharedString::from(format!("notes-{id}-hit-{gi}-{ri}")))
                        .px(SPACE_3)
                        .py(SPACE_2)
                        .rounded(RADIUS_SM)
                        .bg(t.surface)
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_target(path.clone(), Some(Jump::Offset(offset)), cx)
                        }))
                        .child(snippet_text(r.text, r.hit, t.find_match)),
                );
            }
            col = col.child(g);
        }
        col
    }

    pub(crate) fn render_right(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.show_backlinks {
            return None;
        }
        let v = self.vault.clone()?;
        let t = tokens(cx);
        let rel = self.my_rel(cx).unwrap_or_default();
        let rev = v.read(cx).revision;
        let key = (rev, rel.clone());
        if self.links_cache.as_ref().is_none_or(|c| c.key != key) {
            let s = v.read(cx);
            let data = match s.index.find(&rel) {
                Some(me) => {
                    let linked = s.index.backlinks(me);
                    let unlinked = s.index.unlinked_mentions(me, MAX_UNLINKED);
                    let n = &s.index.notes[me];
                    let mut broken: Vec<String> = n
                        .links
                        .iter()
                        .filter(|l| !l.target.is_empty() && s.index.target_of(me, l).is_none())
                        .map(|l| l.target.clone())
                        .collect();
                    broken.dedup();
                    LinksData {
                        key,
                        linked_count: linked.len(),
                        linked: group(&s.index, linked.into_iter().map(|m| (m.note, m.range))),
                        unlinked_count: unlinked.len(),
                        unlinked: group(&s.index, unlinked.into_iter().map(|m| (m.note, m.range))),
                        broken,
                    }
                }
                None => LinksData {
                    key,
                    linked: Vec::new(),
                    linked_count: 0,
                    unlinked: Vec::new(),
                    unlinked_count: 0,
                    broken: Vec::new(),
                },
            };
            self.links_cache = Some(data);
        }
        let data = self.links_cache.as_ref().expect("filled above");
        let (linked, unlinked, broken) = (
            data.linked.clone(),
            data.unlinked.clone(),
            data.broken.clone(),
        );
        let (lc, uc) = (data.linked_count, data.unlinked_count);
        let mut col = v_flex().gap(SPACE_4).p(SPACE_4);
        col = col.child(
            h_flex()
                .gap(SPACE_2)
                .child(section_label(tf("Linked mentions ({})", &[&lc]), cx)),
        );
        if lc == 0 {
            col = col.child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_dim)
                    .child(t("No notes link here yet.")),
            );
        }
        col = col.child(self.groups_list("linked", linked, cx));
        col = col.child(section_label(tf("Unlinked mentions ({})", &[&uc]), cx));
        col = col.child(self.groups_list("unlinked", unlinked, cx));
        if !broken.is_empty() {
            let mut b = v_flex()
                .gap(SPACE_1)
                .child(section_label(t("Links to notes not made yet"), cx));
            for name in broken {
                let n = name.clone();
                b = b.child(
                    div()
                        .id(SharedString::from(format!("notes-broken-{name}")))
                        .px(SPACE_2)
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_dim)
                        .cursor_pointer()
                        .hover(|s| s.text_color(t.accent))
                        .on_click(cx.listener(move |this, _, _, cx| this.follow_wiki(&n, None, cx)))
                        .child(format!("[[{name}]]")),
                );
            }
            col = col.child(b);
        }
        Some(
            v_flex()
                .flex_none()
                .w(RIGHT_W)
                .h_full()
                .min_h_0()
                .bg(t.chrome)
                .border_l_1()
                .border_color(t.border)
                .text_size(FONT_BODY)
                .child(
                    h_flex()
                        .flex_none()
                        .px(SPACE_4)
                        .h(CONTROL_LG + SPACE_3)
                        .border_b_1()
                        .border_color(t.border)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(t("Backlinks")),
                )
                .child(
                    div()
                        .id("notes-backlinks")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(col),
                )
                .into_any_element(),
        )
    }
}

/// The entity id of the view `v` (to leave it out while it is updated).
fn cx_entity_id(v: &NotesView) -> EntityId {
    v.entity_id
}

/// "3 other notes link to it: A, B, C" for the rename dialog.
fn rename_summary(p: &RenamePlan) -> String {
    let others: Vec<String> = p
        .files
        .iter()
        .filter(|f| f.0 != p.old_path)
        .map(|f| {
            f.0.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .collect();
    let links: usize = p.files.iter().map(|f| f.3.len()).sum();
    if others.is_empty() && links == 0 {
        return t("No other notes link to this one.").to_owned();
    }
    let mut names = others
        .iter()
        .take(6)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if others.len() > 6 {
        names.push_str(", …");
    }
    if others.is_empty() {
        return tf("{} links in this note will be updated.", &[&links]);
    }
    tf(
        "{} links in {} notes will be updated: {}",
        &[&links, &others.len(), &names],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn snippets_cut_around_the_hit() {
        let text =
            "first\n   - a long line with the word needle somewhere in the middle of it all\nlast";
        let at = text.find("needle").unwrap();
        let (s, hit) = snippet(text, at..at + 6, 30);
        assert_eq!(&s[hit.clone()], "needle");
        assert!(s.len() <= 30 + 2 * "…".len() + 6, "{s}");
        let (s, hit) = snippet("needle", 0..6, 40);
        assert_eq!((s.as_str(), hit), ("needle", 0..6));
        let (s, hit) = snippet("  - item text", 4..8, 40);
        assert_eq!(s, "- item text");
        assert_eq!(&s[hit], "item");
    }
}
