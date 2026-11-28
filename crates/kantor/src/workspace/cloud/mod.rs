//! Cloud storage in the workspace: Open from Cloud, save back, conflicts,
//! locks and offline work, for Sheets, Docs and Slides alike.
//!
//! A cloud document is an ordinary document whose file is a working copy
//! in Kantor's cache (`kantor_cloud::cache`). The apps open and save it like
//! any file; the workspace notices a save by the working copy's stamp
//! changing and uploads it in the background with the rules of
//! `kantor_cloud::sync`. Network work runs on worker threads (`jobs`).

mod accounts;
mod browser;
mod conflict;
pub(crate) mod jobs;
mod oauth;
mod sso;
mod widgets;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui_kit::assets::IconName;
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ai::keys::CLOUD;
use kantor_cloud::cache::{Cache, CacheEntry, Stamp};
use kantor_cloud::session::{self, LOCK_REFRESH, Remote};
use kantor_cloud::sync::{Action, DocSync, LockState, Status, UploadOutcome, conflict_copy_name};
use kantor_cloud::{Auth, Client};
use kantor_core::layout::PaneId;
use kantor_core::settings::{CloudAccount, Settings};
use kantor_ui::i18n::{Lang, lang, t, tf, tr};
use kantor_ui::metrics::*;
use kantor_ui::tokens;

use super::notices::{NoticeKind, upload_progress};
use super::read_only::ReadOnly;
use super::{AppKind, Workspace};
use crate::actions::{CloudAccounts, OpenFromCloud, SaveToCloud, SyncNow};
use accounts::AccountsPanel;
use browser::BrowserPanel;
use conflict::{ConflictPanel, Resolution};

pub use widgets::init;

/// How often an offline document checks whether the server is back.
const RECONNECT_EVERY: Duration = Duration::from_secs(15);

/// Waits before uploading again by itself after an upload failed because
/// the server couldn't be reached (doubling from the first to the last);
/// the server is checked every [`RECONNECT_EVERY`] meanwhile, and when it
/// answers again the upload starts at once.
const RETRY_FIRST: Duration = Duration::from_secs(15);
const RETRY_MAX: Duration = Duration::from_secs(300);
/// Wait before retrying an upload the server was too busy for.
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// How long quitting waits for its cloud locks to be released, all of them
/// together (a sign-in's token refresh included).
const QUIT_UNLOCK_WAIT: Duration = Duration::from_secs(4);

/// What a cloud panel tells the workspace.
pub enum PanelEvent {
    Close,
    AccountsChanged,
    OpenAccounts,
    Open {
        account: Box<CloudAccount>,
        remote: String,
    },
    /// Save to Cloud: upload `pane`'s document as `remote`.
    SaveTo {
        pane: PaneId,
        account: Box<CloudAccount>,
        remote: String,
    },
    Resolve {
        pane: PaneId,
        resolution: Resolution,
    },
}

/// A cloud document open in a pane.
struct CloudDoc {
    sync: DocSync,
    account: CloudAccount,
    /// The working copy's stamp when last looked at (detects saves).
    seen: Option<Stamp>,
    /// The server answers again while local saves wait: offer "Sync now".
    server_back: bool,
    /// Locked by someone else, but the lock is gone now: offer "Edit now".
    lock_free: bool,
    /// After a failed upload: when to try again by itself, and the wait
    /// before the next try after that (doubles up to [`RETRY_MAX`]).
    retry_at: Option<std::time::Instant>,
    retry_wait: Duration,
    /// Serializes this document's network work (upload, lock release) on
    /// the worker threads.
    ops: Arc<Mutex<()>>,
}

/// The workspace's cloud state.
#[derive(Default)]
pub(super) struct Cloud {
    docs: HashMap<PaneId, CloudDoc>,
    overlay: Option<(AnyView, Subscription)>,
    ticker: Option<Task<()>>,
    started: bool,
    /// The user chose "Keep Local Copy" for the close in progress.
    pub(super) close_ok: bool,
    /// Lock releases of documents closed this run, still running.
    unlocking: Vec<std::thread::JoinHandle<()>>,
    /// The user turned cloud autosave off (`[cloud] autosave = false`).
    autosave_off: bool,
}

/// The working copies (`<data dir>/cloud`: unsynced work must not live
/// where cache cleaners delete things).
pub(crate) fn cache() -> Cache {
    let root = kantor_core::dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("cloud");
    Cache::new(root)
}

/// Where working copies used to be (`<cache dir>/cloud`).
fn old_cache_root() -> Option<PathBuf> {
    Some(kantor_core::dirs::cache_dir()?.join("cloud"))
}

/// Move working copies from the old cache folder (once, at startup,
/// before the session restores their tabs).
pub(super) fn migrate_working_copies() {
    let Some(old) = old_cache_root() else {
        return;
    };
    match cache().migrate_from(&old) {
        Ok(0) => {}
        Ok(n) => log::info!("moved {n} cloud working copies to the data folder"),
        Err(e) => log::warn!("moving cloud working copies: {e}"),
    }
}

/// A session path inside the old working-copy folder, mapped to the new one.
pub(super) fn migrated_path(path: &std::path::Path) -> PathBuf {
    if let Some(old) = old_cache_root()
        && let Ok(rel) = path.strip_prefix(&old)
    {
        return cache().root().join(rel);
    }
    path.to_path_buf()
}

/// Synced working copies untouched for this long are deleted at startup.
const PRUNE_AFTER: Duration = Duration::from_secs(30 * 24 * 3600);

/// A client for `account` with its stored app password (worker thread:
/// the keychain may block).
pub(crate) fn remote_for(account: &CloudAccount) -> Result<Remote, String> {
    if account.is_oauth() {
        return oauth::remote(account);
    }
    if account.is_sso() {
        return sso::remote(account);
    }
    let password = CLOUD.load(&account.id).ok_or_else(|| {
        tr(
            "No app password is saved for this account. Add it again in Cloud Accounts.",
            "Tidak ada kata sandi aplikasi tersimpan untuk akun ini. Tambahkan lagi di Akun Cloud.",
        )
        .to_owned()
    })?;
    Ok(remote_with(account, password))
}

pub(crate) fn remote_with(account: &CloudAccount, password: String) -> Remote {
    Remote {
        client: Client::new(
            &account.dav_root(),
            Auth::Basic {
                user: account.user.clone(),
                password,
            },
        ),
        account: account.id.clone(),
        owner: format!("{} (Kantor)", account.user),
    }
}

/// Today's date, local time (`2026-11-14`).
fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

impl Workspace {
    fn cloud_notice(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.notice(text.into(), cx);
    }

    /// A failure: stays in the status bar until the user's next action.
    fn cloud_error(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        self.notice_as(text.into(), NoticeKind::Error, cx);
    }

    // ── panels ─────────────────────────────────────────────────────────────

    pub(super) fn open_from_cloud(
        &mut self,
        _: &OpenFromCloud,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.new(BrowserPanel::new);
        panel.update(cx, |p, cx| p.focus_first(window, cx));
        self.show_cloud_panel(panel, window, cx);
    }

    /// Save to Cloud: pick an account, folder and name for the focused
    /// document, which must be saved on this computer first.
    pub(super) fn save_to_cloud(
        &mut self,
        _: &SaveToCloud,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(pane) = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.focused)
        else {
            return;
        };
        let Some(view) = self.docs.get(&pane).map(|d| d.view.clone()) else {
            return;
        };
        let Some(path) = view.path(cx).filter(|_| !view.is_dirty(cx)) else {
            self.cloud_notice(
                t("Save the document on this computer first, then choose Save to Cloud."),
                cx,
            );
            return;
        };
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let panel = cx.new(|cx| BrowserPanel::for_save(pane, &name, window, cx));
        panel.update(cx, |p, cx| p.focus_first(window, cx));
        self.show_cloud_panel(panel, window, cx);
    }

    /// Upload `pane`'s file as `remote` (never over an existing file), then
    /// open the cloud copy in a new tab: its edits save to the cloud.
    fn upload_to_cloud(
        &mut self,
        pane: PaneId,
        account: CloudAccount,
        remote: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.docs.get(&pane).and_then(|d| d.view.path(cx)) else {
            return;
        };
        let place = account
            .provider_name()
            .map_or_else(|| account.label(), str::to_owned);
        let saving = tf("Saving to {}…", &[&place]);
        let notice = self.notice_as(saving.clone(), NoticeKind::Progress, cx);
        let job_account = account.clone();
        jobs::spawn_in(
            cx,
            window,
            move |progress: &dyn Fn((u64, u64))| {
                upload_new(&job_account, &path, &remote, &mut throttled(progress))
            },
            move |ws: &mut Self, (sent, total), _, cx| {
                let text = format!("{saving} {}", upload_progress(sent, total));
                if ws.notices.progress(notice, text.into()) {
                    cx.notify();
                }
            },
            move |ws: &mut Self, r, window, cx| match r {
                Ok((entry, lock)) => {
                    let local = entry.local.clone();
                    let name = entry.name().to_owned();
                    ws.open_path(local.clone(), window, cx);
                    if let Some((_, pane)) = ws.find_path(&local, cx) {
                        ws.attach_cloud(pane, DocSync::new(entry, lock), account, window, cx);
                    }
                    ws.cloud_notice(
                        tf(
                            "Saved to {}: “{}”. It's open in a new tab; edits there save to the cloud.",
                            &[&place, &name],
                        ),
                        cx,
                    );
                }
                Err(e) => ws.cloud_error(tf("Couldn't save to the cloud: {}", &[&e]), cx),
            },
        );
    }

    pub(super) fn open_cloud_accounts(
        &mut self,
        _: &CloudAccounts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.new(|cx| AccountsPanel::new(window, cx));
        panel.update(cx, |p, cx| p.focus_first(window, cx));
        self.show_cloud_panel(panel, window, cx);
    }

    fn show_cloud_panel<V: Render + EventEmitter<PanelEvent>>(
        &mut self,
        panel: Entity<V>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sub =
            cx.subscribe_in(
                &panel,
                window,
                |ws, _, event: &PanelEvent, window, cx| match event {
                    PanelEvent::Close => ws.close_cloud_panel(window, cx),
                    PanelEvent::AccountsChanged => {}
                    PanelEvent::OpenAccounts => {
                        ws.close_cloud_panel(window, cx);
                        ws.open_cloud_accounts(&CloudAccounts, window, cx);
                    }
                    PanelEvent::Open { account, remote } => {
                        let (account, remote) = ((**account).clone(), remote.clone());
                        ws.close_cloud_panel(window, cx);
                        ws.open_remote(account, remote, window, cx);
                    }
                    PanelEvent::SaveTo {
                        pane,
                        account,
                        remote,
                    } => {
                        let (pane, account, remote) = (*pane, (**account).clone(), remote.clone());
                        ws.close_cloud_panel(window, cx);
                        ws.upload_to_cloud(pane, account, remote, window, cx);
                    }
                    PanelEvent::Resolve { pane, resolution } => {
                        let (pane, resolution) = (*pane, *resolution);
                        ws.close_cloud_panel(window, cx);
                        ws.resolve_conflict(pane, resolution, window, cx);
                    }
                },
            );
        self.cloud.overlay = Some((panel.into(), sub));
        cx.notify();
    }

    pub(super) fn close_cloud_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloud.overlay.take().is_some() {
            // Closing a dialog refocuses what had focus before it; put focus
            // on the active document instead (see gpui-dialog-focus-restore).
            self.activate(self.active, window, cx);
        }
    }

    pub(super) fn render_cloud_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let view = self.cloud.overlay.as_ref()?.0.clone();
        let entity = cx.entity().downgrade();
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .bg(tokens(cx).scrim)
                .occlude()
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.close_cloud_panel(window, cx));
                })
                .child(view),
        )
    }

    // ── opening ────────────────────────────────────────────────────────────

    /// Download `remote` (or reuse its working copy when the server can't
    /// be reached) and open it in the right app.
    fn open_remote(
        &mut self,
        account: CloudAccount,
        remote: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let local = cache().local_path(&account.id, &remote);
        if let Some((ti, pane)) = self.find_path(&local, cx) {
            self.tabs[ti].focused = pane;
            self.activate(Some(ti), window, cx);
            return;
        }
        if AppKind::for_path(&local).is_none() {
            self.cloud_notice(
                format!(
                    "{} {remote}",
                    tr("Kantor can't open", "Kantor tidak dapat membuka")
                ),
                cx,
            );
            return;
        }
        self.cloud_notice(
            format!("{} {}…", tr("Downloading", "Mengunduh"), name_of(&remote)),
            cx,
        );
        let job_account = account.clone();
        jobs::spawn_in(
            cx,
            window,
            move |_: &dyn Fn(())| -> Result<(DocSync, bool), String> {
                let cache = cache();
                let opened = remote_for(&job_account).and_then(|r| {
                    session::open(&r, &cache, &remote).map_err(|e| {
                        if session::is_offline(&e) {
                            String::new()
                        } else {
                            e.to_string()
                        }
                    })
                });
                match opened {
                    Ok(o) => Ok((DocSync::new(o.entry, o.lock), false)),
                    // Unreachable: open the working copy if there is one.
                    Err(e) if e.is_empty() => session::open_offline(&cache, &job_account.id, &remote)
                        .map(|entry| (DocSync::offline(entry), true))
                        .ok_or_else(|| {
                            tr(
                                "Can't reach the server, and there is no copy of this file on this computer.",
                                "Server tidak dapat dihubungi, dan tidak ada salinan berkas ini di komputer ini.",
                            )
                            .to_owned()
                        }),
                    Err(e) => Err(e),
                }
            },
            |_, _, _, _| {},
            move |ws: &mut Self, r, window, cx| match r {
                Ok((sync, offline)) => {
                    let local = sync.entry.local.clone();
                    let read_only = sync.read_only_by().map(str::to_owned);
                    ws.open_path(local.clone(), window, cx);
                    if let Some((_, pane)) = ws.find_path(&local, cx) {
                        ws.attach_cloud(pane, sync, account, window, cx);
                    }
                    if let Some(who) = read_only {
                        ws.cloud_notice(read_only_notice(&who), cx);
                    } else if offline {
                        ws.cloud_notice(
                            tr(
                                "Offline: opened the copy on this computer. Changes sync when the server is back.",
                                "Luring: membuka salinan di komputer ini. Perubahan disinkronkan saat server kembali.",
                            ),
                            cx,
                        );
                    }
                }
                Err(e) => ws.cloud_error(
                    format!(
                        "{} {e}",
                        tr("Couldn't open from the cloud:", "Gagal membuka dari cloud:")
                    ),
                    cx,
                ),
            },
        );
    }

    fn attach_cloud(
        &mut self,
        pane: PaneId,
        sync: DocSync,
        account: CloudAccount,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let seen = Stamp::of(&sync.entry.local);
        // Local saves kept from an earlier session ("Keep Local Copy", a
        // quit while offline): send them now unless someone else has the file.
        let send = matches!(sync.state, kantor_cloud::sync::SyncState::LocalChanges)
            && !matches!(sync.lock, LockState::LockedBy(_));
        self.cloud.docs.insert(
            pane,
            CloudDoc {
                sync,
                account,
                seen,
                server_back: false,
                lock_free: false,
                retry_at: None,
                retry_wait: RETRY_FIRST,
                ops: Arc::default(),
            },
        );
        self.start_cloud_ticker(window, cx);
        self.sync_read_only(cx);
        if send {
            self.upload(pane, false, window, cx);
        }
        cx.notify();
    }

    /// Cloud documents locked by someone else are read-only (see
    /// `read_only`); others aren't, unless the user made them so.
    fn sync_read_only(&mut self, cx: &mut Context<Self>) {
        let wanted: Vec<(PaneId, Option<ReadOnly>)> = self
            .cloud
            .docs
            .iter()
            .map(|(&p, cd)| {
                let ro = match &cd.sync.lock {
                    LockState::LockedBy(owner) => Some(ReadOnly::Locked {
                        owner: owner.clone(),
                        free: cd.lock_free,
                    }),
                    _ => None,
                };
                (p, ro)
            })
            .collect();
        for (pane, ro) in wanted {
            if ro.is_none() && matches!(self.read_only(pane), Some(ReadOnly::Manual)) {
                continue;
            }
            self.set_read_only(pane, ro, cx);
        }
    }

    /// "Check again" on a locked document: is the lock still there?
    pub(super) fn check_lock(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cd) = self.cloud.docs.get(&pane) else {
            return;
        };
        let (account, remote) = (cd.account.clone(), cd.sync.entry.remote.clone());
        jobs::spawn_in(
            cx,
            window,
            move |_: &dyn Fn(())| {
                let r = remote_for(&account).ok()?;
                session::lock_holder(&r, &remote).ok()
            },
            |_, _, _, _| {},
            move |ws: &mut Self, r, _, cx| {
                let Some(holder) = r else {
                    ws.cloud_notice(
                        tr("Can't reach the server.", "Server tidak dapat dihubungi."),
                        cx,
                    );
                    return;
                };
                let Some(cd) = ws.cloud.docs.get_mut(&pane) else {
                    return;
                };
                match holder {
                    Some(owner) => {
                        let text = read_only_notice(&owner);
                        cd.sync.lock = LockState::LockedBy(owner);
                        cd.lock_free = false;
                        ws.cloud_notice(text, cx);
                    }
                    None => cd.lock_free = true,
                }
                ws.sync_read_only(cx);
            },
        );
    }

    /// After a session restore: panes showing working copies become cloud
    /// documents again (lock retaken, server checked), and locks a crashed
    /// run left behind are released.
    pub(super) fn init_cloud(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cloud.autosave_off = !Settings::load().cloud.autosave;
        let store = cache();
        let mut restored = Vec::new();
        for (&pane, doc) in &self.docs {
            if let Some(path) = doc.view.path(cx)
                && store.contains(&path)
                && let Some(entry) = store.find(&path)
            {
                restored.push((pane, entry));
            }
        }
        let accounts = Settings::load().cloud.accounts;
        if accounts.is_empty() {
            return;
        }
        let keep: Vec<(String, String)> = restored
            .iter()
            .map(|(_, e)| (e.account.clone(), e.remote.clone()))
            .collect();
        jobs::spawn_in(
            cx,
            window,
            move |_: &dyn Fn(())| {
                let cache = cache();
                let open: Vec<PathBuf> = restored.iter().map(|(_, e)| e.local.clone()).collect();
                let pruned = cache.prune(PRUNE_AFTER, &open);
                if pruned > 0 {
                    log::info!("deleted {pruned} old synced cloud working copies");
                }
                // Locks left by a previous run (crash, kill, no network at exit).
                for a in &accounts {
                    if let Ok(r) = remote_for(a) {
                        let keep: Vec<String> = keep
                            .iter()
                            .filter(|(acc, _)| acc == &a.id)
                            .map(|(_, p)| p.clone())
                            .collect();
                        let n = session::cleanup_locks(&r, &cache, &keep);
                        if n > 0 {
                            log::info!("released {n} stale cloud lock(s) for {}", a.id);
                        }
                    }
                }
                restored
                    .into_iter()
                    .filter_map(|(pane, entry)| {
                        let account = accounts.iter().find(|a| a.id == entry.account)?.clone();
                        Some((pane, reattach(&account, &cache, entry), account))
                    })
                    .collect::<Vec<_>>()
            },
            |_, _, _, _| {},
            |ws: &mut Self, docs, window, cx| {
                for (pane, (sync, reload), account) in docs {
                    if !ws.docs.contains_key(&pane) {
                        continue;
                    }
                    ws.attach_cloud(pane, sync, account, window, cx);
                    if reload {
                        ws.reload_view(pane, window, cx);
                    }
                }
            },
        );
    }

    // ── saving ─────────────────────────────────────────────────────────────

    /// A document changed (edit, save): if it's a cloud document whose
    /// working copy was just saved, sync it.
    pub(super) fn cloud_doc_changed(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.docs.get(&pane) else {
            return;
        };
        if doc.view.is_dirty(cx) || doc.view.is_saving(cx) {
            return;
        }
        let Some(cd) = self.cloud.docs.get_mut(&pane) else {
            return;
        };
        let stamp = Stamp::of(&cd.sync.entry.local);
        if stamp == cd.seen {
            return;
        }
        cd.seen = stamp;
        if stamp == cd.sync.entry.synced {
            return;
        }
        let action = cd.sync.on_local_save();
        self.run_action(pane, action, window, cx);
    }

    fn run_action(
        &mut self,
        pane: PaneId,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            Action::None => {}
            Action::Upload => self.upload(pane, false, window, cx),
            Action::ShowConflict => self.show_conflict(pane, window, cx),
            Action::NotifyReadOnly => {
                if let Some(who) = self
                    .cloud
                    .docs
                    .get(&pane)
                    .and_then(|d| d.sync.read_only_by())
                {
                    let text = format!(
                        "{} {}",
                        read_only_notice(who),
                        tr(
                            "Your changes are saved on this computer only.",
                            "Perubahan Anda hanya tersimpan di komputer ini.",
                        )
                    );
                    self.cloud_notice(text, cx);
                }
            }
            Action::RetryLater => {
                cx.spawn_in(window, async move |this, cx| {
                    cx.background_executor().timer(RETRY_AFTER).await;
                    let _ = this.update_in(cx, |ws, window, cx| {
                        let still = ws
                            .cloud
                            .docs
                            .get(&pane)
                            .is_some_and(|d| !d.sync.is_uploading() && d.sync.unsynced());
                        if still {
                            ws.upload(pane, false, window, cx);
                        }
                    });
                })
                .detach();
            }
        }
        self.sync_read_only(cx);
        cx.notify();
    }

    /// Upload the working copy (`force`: overwrite whatever the server has).
    fn upload(&mut self, pane: PaneId, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cd) = self.cloud.docs.get_mut(&pane) else {
            return;
        };
        if cd.sync.is_uploading() || cd.sync.read_only_by().is_some() {
            return;
        }
        let total = Stamp::of(&cd.sync.entry.local).map_or(0, |s| s.len);
        cd.sync.begin_upload(total);
        cd.server_back = false;
        let (account, entry, lock, ops) = (
            cd.account.clone(),
            cd.sync.entry.clone(),
            cd.sync.lock_token().cloned(),
            cd.ops.clone(),
        );
        jobs::spawn_in(
            cx,
            window,
            move |progress: &dyn Fn((u64, u64))| {
                let _ops = ops.lock().unwrap_or_else(|e| e.into_inner());
                let r = match remote_for(&account) {
                    Ok(r) => r,
                    Err(e) => return (UploadOutcome::Failed(e), entry),
                };
                // A panic in the upload must not leave the chip at
                // "Uploading" for good: report it as a failure instead.
                let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    session::upload(
                        &r,
                        &cache(),
                        &entry,
                        lock.as_ref(),
                        force,
                        &mut throttled(progress),
                    )
                }))
                .unwrap_or_else(|_| {
                    UploadOutcome::Failed(
                        tr(
                            "The upload stopped unexpectedly. Choose Sync now to try again.",
                            "Unggahan berhenti tiba-tiba. Pilih Sinkronkan sekarang untuk mencoba lagi.",
                        )
                        .to_string(),
                    )
                });
                // A single sign-on that has ended: say how to go on.
                let out = match out {
                    UploadOutcome::Failed(_) if sso::ended(&account) => {
                        UploadOutcome::Failed(sso::ended_message(&account))
                    }
                    out => out,
                };
                if let UploadOutcome::Synced {
                    server,
                    stamp,
                    sha256,
                    sha1,
                    ..
                } = &out
                {
                    let mut saved = CacheEntry {
                        synced: *stamp,
                        sha256: sha256.clone(),
                        sha1: sha1.clone(),
                        ..entry.clone()
                    };
                    if let Some(s) = server {
                        saved.rebase(s);
                    }
                    let _ = cache().save_entry(&saved);
                }
                (out, entry)
            },
            move |ws: &mut Self, (sent, total), _, cx| {
                if let Some(cd) = ws.cloud.docs.get_mut(&pane) {
                    cd.sync.on_progress(sent, total);
                    cx.notify();
                }
            },
            move |ws: &mut Self, (outcome, entry), window, cx| {
                let Some(cd) = ws.cloud.docs.get_mut(&pane) else {
                    // Closed while uploading: release its lock now.
                    return;
                };
                let synced = matches!(outcome, UploadOutcome::Synced { .. });
                let offline = matches!(outcome, UploadOutcome::Offline(_));
                let signed_out = match &outcome {
                    UploadOutcome::Failed(m) if sso::ended(&cd.account) => Some(m.clone()),
                    _ => None,
                };
                let failed = match &outcome {
                    UploadOutcome::Failed(m) if signed_out.is_none() => Some(m.clone()),
                    _ => None,
                };
                let action = cd.sync.on_upload(outcome);
                if offline {
                    cd.retry_at = Some(std::time::Instant::now() + cd.retry_wait);
                    cd.retry_wait = (cd.retry_wait * 2).min(RETRY_MAX);
                } else {
                    cd.retry_at = None;
                    cd.retry_wait = RETRY_FIRST;
                }
                // With autosave the status bar chip says it; a notice for
                // every save would be noise.
                if synced && ws.cloud.autosave_off {
                    ws.cloud_notice(
                        format!(
                            "{} {}",
                            tr("Saved to the cloud:", "Tersimpan ke cloud:"),
                            entry.name()
                        ),
                        cx,
                    );
                } else if let Some(m) = signed_out {
                    ws.cloud_error(m, cx);
                } else if let Some(m) = failed {
                    ws.cloud_error(tf("Couldn't save to the cloud: {}", &[&m]), cx);
                } else if offline {
                    ws.cloud_notice(
                        tr(
                            "Can't reach the server. Saved on this computer; it syncs when the server is back.",
                            "Server tidak dapat dihubungi. Tersimpan di komputer ini; disinkronkan saat server kembali.",
                        ),
                        cx,
                    );
                }
                ws.run_action(pane, action, window, cx);
            },
        );
        cx.notify();
    }

    /// "Sync now": upload a document with local saves the server doesn't
    /// have (offline, failed, read-only lifted), or recheck a read-only lock.
    pub(super) fn sync_now(&mut self, _: &SyncNow, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.focused)
        else {
            return;
        };
        let Some(cd) = self.cloud.docs.get_mut(&pane) else {
            self.cloud_notice(
                tr(
                    "This document isn't from the cloud.",
                    "Dokumen ini bukan dari cloud.",
                ),
                cx,
            );
            return;
        };
        if cd.sync.read_only_by().is_some() {
            if cd.lock_free {
                self.edit_now(pane, window, cx);
            } else {
                self.check_lock(pane, window, cx);
            }
            return;
        }
        if matches!(
            cd.sync.state,
            kantor_cloud::sync::SyncState::Conflict { .. }
        ) {
            self.show_conflict(pane, window, cx);
            return;
        }
        let action = cd.sync.on_reconnect();
        if action == Action::None {
            self.cloud_notice(tr("Already in sync.", "Sudah tersinkron."), cx);
        }
        self.run_action(pane, action, window, cx);
    }

    /// "Edit now": take the lock of a read-only document (the other
    /// person closed it) and reload it if they changed it.
    pub(super) fn edit_now(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cd) = self.cloud.docs.get(&pane) else {
            return;
        };
        let (account, entry) = (cd.account.clone(), cd.sync.entry.clone());
        jobs::spawn_in(
            cx,
            window,
            move |_: &dyn Fn(())| {
                let r = remote_for(&account).ok()?;
                let cache = cache();
                let mut entry = entry;
                let (lock, changed) = session::take_lock_settled(&r, &cache, &mut entry);
                let changed = changed.unwrap_or(false);
                let server_etag = r.client.stat(&entry.remote).ok().and_then(|e| e.etag);
                Some((lock, changed, server_etag, entry))
            },
            |_, _, _, _| {},
            move |ws: &mut Self, r, window, cx| {
                let Some((lock, changed, server_etag, entry)) = r else {
                    return;
                };
                let Some(cd) = ws.cloud.docs.get_mut(&pane) else {
                    return;
                };
                if !changed {
                    // The base under the ETag the lock gave it.
                    cd.sync.entry.etag = entry.etag;
                    cd.sync.entry.modified = entry.modified;
                    cd.sync.entry.size = entry.size;
                }
                if let LockState::LockedBy(who) = &lock {
                    let text = read_only_notice(who);
                    cd.sync.lock = lock;
                    cd.lock_free = false;
                    ws.cloud_notice(text, cx);
                    ws.sync_read_only(cx);
                    return;
                }
                cd.sync.lock = lock;
                cd.lock_free = false;
                let local = cd.sync.unsynced();
                let action = if changed && local {
                    cd.sync.state = kantor_cloud::sync::SyncState::Conflict { server_etag };
                    Action::ShowConflict
                } else if local {
                    cd.sync.on_reconnect()
                } else {
                    Action::None
                };
                ws.cloud_notice(
                    tr(
                        "The file is no longer locked: you can edit it.",
                        "Berkas tidak lagi dikunci: Anda bisa mengeditnya.",
                    ),
                    cx,
                );
                ws.sync_read_only(cx);
                if changed && !local {
                    // They saved while it was locked: show their version.
                    ws.reload_view_from_server(pane, window, cx);
                } else {
                    ws.run_action(pane, action, window, cx);
                }
            },
        );
    }

    // ── conflicts ──────────────────────────────────────────────────────────

    fn show_conflict(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cd) = self.cloud.docs.get(&pane) else {
            return;
        };
        let name = cd.sync.entry.name().to_owned();
        let word = match lang() {
            Lang::Id => "konflik",
            Lang::En => "conflict",
        };
        let copy = conflict_copy_name(&name, word, &cd.account.user, &today());
        let panel = cx.new(|cx| ConflictPanel::new(pane, name, copy, cx));
        panel.update(cx, |p, cx| p.focus_first(window, cx));
        self.show_cloud_panel(panel, window, cx);
    }

    fn resolve_conflict(
        &mut self,
        pane: PaneId,
        resolution: Resolution,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cd) = self.cloud.docs.get(&pane) else {
            return;
        };
        match resolution {
            Resolution::Overwrite => self.upload(pane, true, window, cx),
            Resolution::Reload => self.reload_view_from_server(pane, window, cx),
            Resolution::SaveCopy => {
                let word = match lang() {
                    Lang::Id => "konflik",
                    Lang::En => "conflict",
                };
                let copy =
                    conflict_copy_name(cd.sync.entry.name(), word, &cd.account.user, &today());
                let (account, entry, ops) =
                    (cd.account.clone(), cd.sync.entry.clone(), cd.ops.clone());
                self.cloud_notice(
                    tr(
                        "Saving your version as a copy…",
                        "Menyimpan versi Anda sebagai salinan…",
                    ),
                    cx,
                );
                jobs::spawn_in(
                    cx,
                    window,
                    move |_: &dyn Fn(())| {
                        let _ops = ops.lock().unwrap_or_else(|e| e.into_inner());
                        let r = remote_for(&account)?;
                        let path =
                            session::save_copy(&r, &entry, &copy).map_err(|e| e.to_string())?;
                        let fresh =
                            session::reload(&r, &cache(), &entry).map_err(|e| e.to_string())?;
                        Ok::<_, String>((path, fresh))
                    },
                    |_, _, _, _| {},
                    move |ws: &mut Self, r, window, cx| match r {
                        Ok((path, fresh)) => {
                            if let Some(cd) = ws.cloud.docs.get_mut(&pane) {
                                cd.sync.on_reloaded(fresh);
                                cd.seen = cd.sync.entry.synced;
                            }
                            ws.reload_view(pane, window, cx);
                            ws.cloud_notice(
                                format!(
                                    "{} “{}”. {}",
                                    tr("Your version was saved as", "Versi Anda disimpan sebagai"),
                                    name_of(&path),
                                    tr(
                                        "This window shows the server's version.",
                                        "Jendela ini menampilkan versi server."
                                    )
                                ),
                                cx,
                            );
                        }
                        Err(e) => ws.cloud_error(
                            format!(
                                "{} {e}",
                                tr("Couldn't save the copy:", "Gagal menyimpan salinan:")
                            ),
                            cx,
                        ),
                    },
                );
            }
        }
    }

    /// Download the server's version over the working copy, then reopen.
    fn reload_view_from_server(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(cd) = self.cloud.docs.get(&pane) else {
            return;
        };
        let (account, entry, ops) = (cd.account.clone(), cd.sync.entry.clone(), cd.ops.clone());
        jobs::spawn_in(
            cx,
            window,
            move |_: &dyn Fn(())| {
                let _ops = ops.lock().unwrap_or_else(|e| e.into_inner());
                let r = remote_for(&account)?;
                session::reload(&r, &cache(), &entry).map_err(|e| e.to_string())
            },
            |_, _, _, _| {},
            move |ws: &mut Self, r, window, cx| match r {
                Ok(fresh) => {
                    if let Some(cd) = ws.cloud.docs.get_mut(&pane) {
                        cd.sync.on_reloaded(fresh);
                        cd.seen = cd.sync.entry.synced;
                    }
                    ws.reload_view(pane, window, cx);
                    ws.cloud_notice(
                        tr("Loaded the server's version.", "Versi server dimuat."),
                        cx,
                    );
                }
                Err(e) => ws.cloud_error(
                    format!("{} {e}", tr("Couldn't reload:", "Gagal memuat ulang:")),
                    cx,
                ),
            },
        );
    }

    // ── locks, reconnecting ───────────────────────────────────────────────

    fn start_cloud_ticker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloud.started {
            return;
        }
        self.cloud.started = true;
        self.cloud.ticker = Some(cx.spawn_in(window, async move |this, cx| {
            let mut since_refresh = Duration::ZERO;
            loop {
                cx.background_executor().timer(RECONNECT_EVERY).await;
                since_refresh += RECONNECT_EVERY;
                let refresh = since_refresh >= LOCK_REFRESH;
                if refresh {
                    since_refresh = Duration::ZERO;
                }
                if this
                    .update_in(cx, |ws, window, cx| ws.cloud_tick(refresh, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    /// Every [`RECONNECT_EVERY`]: offline documents check the server; every
    /// [`LOCK_REFRESH`]: held locks are extended.
    fn cloud_tick(&mut self, refresh: bool, window: &mut Window, cx: &mut Context<Self>) {
        for (&pane, cd) in &self.cloud.docs {
            let offline = matches!(
                cd.sync.state,
                kantor_cloud::sync::SyncState::Offline | kantor_cloud::sync::SyncState::Failed(_)
            ) || (cd.sync.lock == LockState::None && !cd.sync.is_uploading());
            let lock = cd.sync.lock_token().cloned();
            let locked_out = cd.sync.read_only_by().is_some() && !cd.lock_free;
            if !offline && !locked_out && !(refresh && lock.is_some()) {
                continue;
            }
            let (account, remote, ops) = (
                cd.account.clone(),
                cd.sync.entry.remote.clone(),
                cd.ops.clone(),
            );
            jobs::spawn_in(
                cx,
                window,
                move |_: &dyn Fn(())| {
                    let _ops = ops.lock().unwrap_or_else(|e| e.into_inner());
                    let r = remote_for(&account).ok()?;
                    let reachable = r.client.stat(&remote).is_ok();
                    if !reachable {
                        return Some((false, None));
                    }
                    if locked_out {
                        // Don't take it: offer "Edit now" when it's free.
                        let free = matches!(session::lock_holder(&r, &remote), Ok(None));
                        return Some((true, free.then_some(LockState::None)));
                    }
                    let cache = cache();
                    let lock = match &lock {
                        Some(t) if refresh => match session::refresh_lock(&r, &remote, t) {
                            Ok(()) => None,
                            // Lost (expired while asleep, or removed): take a new one.
                            Err(_) => Some(session::take_lock(&r, &cache, &remote)),
                        },
                        Some(_) => None,
                        None => Some(session::take_lock(&r, &cache, &remote)),
                    };
                    Some((true, lock))
                },
                |_, _, _, _| {},
                move |ws: &mut Self, r, window, cx| {
                    let Some((reachable, lock)) = r else {
                        return;
                    };
                    let Some(cd) = ws.cloud.docs.get_mut(&pane) else {
                        return;
                    };
                    match lock {
                        Some(LockState::None) if locked_out => cd.lock_free = true,
                        Some(lock) => cd.sync.lock = lock,
                        None => {}
                    }
                    let mut action = Action::None;
                    if reachable && matches!(cd.sync.state, kantor_cloud::sync::SyncState::Offline)
                    {
                        // The server answers again: send the saves that
                        // waited (the conflict checks still apply). Right
                        // after a failure the next try waits for its turn.
                        let due = cd.retry_at.is_none_or(|t| std::time::Instant::now() >= t);
                        if !cd.sync.unsynced() || due {
                            action = cd.sync.on_reconnect();
                        } else {
                            // Until then the chip offers "Sync now".
                            cd.server_back = true;
                        }
                    }
                    ws.sync_read_only(cx);
                    ws.run_action(pane, action, window, cx);
                    cx.notify();
                },
            );
        }
    }

    /// A pane closed: release the cloud document's lock in the background
    /// (after any upload still running for it).
    pub(super) fn forget_cloud(&mut self, pane: PaneId) {
        let Some(cd) = self.cloud.docs.remove(&pane) else {
            return;
        };
        let Some(token) = cd.sync.lock_token().cloned() else {
            return;
        };
        let (account, remote, ops) = (cd.account, cd.sync.entry.remote.clone(), cd.ops);
        let spawned = std::thread::Builder::new()
            .name("kantor-cloud-unlock".into())
            .spawn(move || {
                let _ops = ops.lock().unwrap_or_else(|e| e.into_inner());
                if let Ok(r) = remote_for(&account)
                    && let Err(e) = session::release(&r, &cache(), &remote, &token)
                {
                    log::warn!("releasing the lock on {remote}: {e}");
                }
            });
        // Quitting soon after waits for it too.
        self.cloud.unlocking.retain(|h| !h.is_finished());
        if let Ok(h) = spawned {
            self.cloud.unlocking.push(h);
        }
    }

    /// Quitting (any way: Quit, closing the window, a quit signal, the
    /// system's logout): release every lock, all at once, waiting
    /// [`QUIT_UNLOCK_WAIT`] at most in all, so a dead network never holds
    /// up quitting. A lock that can't be released stays in the ledger with
    /// its token: the next start releases it or, when the file is open
    /// again, takes it back; the server also expires it. Called more than
    /// once, the later calls find nothing left to do.
    pub(crate) fn cloud_quit(&mut self) {
        let mut handles = std::mem::take(&mut self.cloud.unlocking);
        let mut n = 0;
        for (_, cd) in self.cloud.docs.drain() {
            let Some(token) = cd.sync.lock_token().cloned() else {
                continue;
            };
            let (account, remote) = (cd.account, cd.sync.entry.remote.clone());
            n += 1;
            if let Ok(h) = std::thread::Builder::new()
                .name("kantor-cloud-unlock".into())
                .spawn(move || {
                    let start = std::time::Instant::now();
                    let released = remote_for(&account).and_then(|r| {
                        session::release(&r, &cache(), &remote, &token).map_err(|e| e.to_string())
                    });
                    match released {
                        Ok(()) => log::debug!(
                            "quit: released the lock on {remote} in {:.2} s",
                            start.elapsed().as_secs_f64()
                        ),
                        Err(e) => {
                            log::warn!("quit: the lock on {remote} stays until the next start: {e}")
                        }
                    }
                })
            {
                handles.push(h);
            }
        }
        if handles.is_empty() {
            return;
        }
        let deadline = std::time::Instant::now() + QUIT_UNLOCK_WAIT;
        while handles.iter().any(|h| !h.is_finished()) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        let left = handles.iter().filter(|h| !h.is_finished()).count();
        if left > 0 {
            log::warn!(
                "quit: {left} cloud lock(s) not released within {} s; released at the next start",
                QUIT_UNLOCK_WAIT.as_secs()
            );
        } else {
            log::info!("quit: done with {n} cloud lock(s)");
        }
    }

    // ── status bar ─────────────────────────────────────────────────────────

    /// The cloud chip for the focused document, when it's a cloud document.
    pub(super) fn render_cloud_chip(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let pane = self.tabs.get(self.active?)?.focused;
        let cd = self.cloud.docs.get(&pane)?;
        let t = tokens(cx);
        let status = cd.sync.status();
        let dirty = self.docs.get(&pane).is_some_and(|d| d.view.is_dirty(cx));
        let (icon, text, color): (IconName, String, Hsla) = match &status {
            // Edits not saved to the working copy yet.
            Status::Synced if dirty => (
                IconName::Cloud,
                if self.cloud_autosaves(pane) == Some(true) {
                    kantor_ui::i18n::t("Edited · autosaves shortly").into()
                } else {
                    kantor_ui::i18n::t("Edited · not saved").into()
                },
                t.fg_muted,
            ),
            Status::Synced => (
                IconName::Cloud,
                tr("Synced", "Tersinkron").into(),
                t.fg_muted,
            ),
            Status::Pending => (
                IconName::CloudUpload,
                tr("Saving to the cloud…", "Menyimpan ke cloud…").into(),
                t.fg_muted,
            ),
            Status::Uploading { percent } => (
                IconName::CloudUpload,
                format!("{} {percent}%", tr("Uploading", "Mengunggah")),
                t.accent,
            ),
            Status::NotSynced => (
                IconName::CloudOff,
                if cd.server_back {
                    tr(
                        "Not synced · Sync now",
                        "Belum tersinkron · Sinkronkan sekarang",
                    )
                    .into()
                } else {
                    tr("Not synced (offline)", "Belum tersinkron (luring)").into()
                },
                t.warning,
            ),
            Status::Offline => (
                IconName::CloudOff,
                tr("Offline", "Luring").into(),
                t.fg_muted,
            ),
            Status::Conflict => (
                IconName::TriangleAlert,
                tr("Conflict · Resolve…", "Konflik · Selesaikan…").into(),
                t.danger,
            ),
            Status::ReadOnly(who) => (
                IconName::Lock,
                format!(
                    "{} {who}",
                    tr("Read-only · locked by", "Hanya baca · dikunci oleh")
                ),
                t.warning,
            ),
            Status::Failed(_) => (
                IconName::TriangleAlert,
                tr("Sync failed · Retry", "Gagal sinkron · Coba lagi").into(),
                t.danger,
            ),
        };
        let tip = match &status {
            Status::Failed(m) => Some(m.clone()),
            _ => None,
        };
        let entity = cx.entity().downgrade();
        Some(
            h_flex()
                .id("status-cloud")
                .flex_none()
                .gap(SPACE_2)
                .px(SPACE_3)
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .text_color(color)
                .child(
                    gpui_kit::component::Icon::new(icon)
                        .with_size(ICON_SM)
                        .text_color(color),
                )
                .child(text)
                .when_some(tip, |d, tip| {
                    d.tooltip(move |window, cx| {
                        gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                    })
                })
                .on_click(move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.sync_now(&SyncNow, window, cx));
                }),
        )
    }

    /// The File menu in the tab strip: new, open, open from cloud, save,
    /// sync, cloud accounts.
    pub(super) fn render_file_menu(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        use gpui_kit::component::button::{Button, ButtonVariants as _};
        use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};

        let ws = cx.entity().downgrade();
        let cloud_doc = self.focused_is_cloud();
        Button::new("file-menu")
            .ghost()
            .small()
            .label(tr("File", "Berkas"))
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                type Run = fn(&mut Workspace, &mut Window, &mut Context<Workspace>);
                let item = |label: &'static str,
                            keys: Option<&'static str>,
                            icon: Option<IconName>,
                            run: Run| {
                    let ws = ws.clone();
                    let it = match keys {
                        // Label with its shortcut, right-aligned and dim.
                        Some(k) => PopupMenuItem::element(move |_, cx| {
                            let t = tokens(cx);
                            h_flex()
                                .w_full()
                                .gap(SPACE_8)
                                .child(div().flex_1().child(label))
                                .child(div().text_color(t.fg_dim).child(k))
                        }),
                        None => PopupMenuItem::new(label),
                    }
                    .on_click(move |_, window, cx| {
                        let _ = ws.update(cx, |ws, cx| run(ws, window, cx));
                    });
                    match icon {
                        Some(icon) => it.icon(icon),
                        None => it,
                    }
                };
                let menu = menu
                    .item(item(
                        tr("New Spreadsheet", "Spreadsheet Baru"),
                        Some("Ctrl N"),
                        None,
                        |ws, w, cx| ws.new_spreadsheet(&crate::actions::NewSpreadsheet, w, cx),
                    ))
                    .item(item(
                        tr("New Document", "Dokumen Baru"),
                        Some("Ctrl Shift N"),
                        None,
                        |ws, w, cx| ws.new_document(&crate::actions::NewDocument, w, cx),
                    ))
                    .item(item(
                        tr("New Presentation", "Presentasi Baru"),
                        Some("Ctrl Alt N"),
                        None,
                        |ws, w, cx| ws.new_presentation(&crate::actions::NewPresentation, w, cx),
                    ))
                    .separator()
                    .item(item(
                        tr("Open…", "Buka…"),
                        Some("Ctrl O"),
                        Some(IconName::FolderOpen),
                        |ws, w, cx| ws.open_file(&crate::actions::OpenFile, w, cx),
                    ))
                    .item(item(
                        tr("Open from Cloud…", "Buka dari Cloud…"),
                        None,
                        Some(IconName::Cloud),
                        |ws, w, cx| ws.open_from_cloud(&OpenFromCloud, w, cx),
                    ))
                    .separator()
                    .item(item(
                        tr("Save", "Simpan"),
                        Some("Ctrl S"),
                        None,
                        |ws, w, cx| ws.with_active(w, cx, |v, w, cx| v.save(w, cx)),
                    ))
                    .item(item(
                        tr("Save As…", "Simpan Sebagai…"),
                        Some("Ctrl Shift S"),
                        None,
                        |ws, w, cx| ws.with_active(w, cx, |v, w, cx| v.save_as(w, cx)),
                    ))
                    .item(item(
                        t("Save to Cloud…"),
                        None,
                        Some(IconName::CloudUpload),
                        |ws, w, cx| ws.save_to_cloud(&SaveToCloud, w, cx),
                    ));
                let menu = if cloud_doc {
                    menu.item(item(
                        tr("Sync Now", "Sinkronkan Sekarang"),
                        None,
                        Some(IconName::RefreshCw),
                        |ws, w, cx| ws.sync_now(&SyncNow, w, cx),
                    ))
                } else {
                    menu
                };
                menu.separator().item(item(
                    tr("Cloud Accounts…", "Akun Cloud…"),
                    None,
                    None,
                    |ws, w, cx| ws.open_cloud_accounts(&CloudAccounts, w, cx),
                ))
            })
    }

    /// Cloud documents among `closing`'s panes whose saves aren't on the
    /// server (offline, failed, uploading or waiting).
    pub(super) fn unsynced_cloud(&self, closing: super::closing::Closing) -> Vec<PaneId> {
        self.closing_panes(closing)
            .into_iter()
            .filter(|p| {
                self.cloud.docs.get(p).is_some_and(|cd| {
                    cd.sync.read_only_by().is_none()
                        && (cd.sync.unsynced() || cd.sync.is_uploading())
                })
            })
            .collect()
    }

    /// Of `panes`, the cloud documents whose saves are on their way to the
    /// server by themselves (see `DocSync::upload_expected`). Saves the
    /// workspace hasn't seen yet (a close right after Save) start their
    /// upload first.
    pub(super) fn cloud_uploads_due(
        &mut self,
        panes: &[PaneId],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<PaneId> {
        for &pane in panes {
            self.cloud_doc_changed(pane, window, cx);
        }
        panes
            .iter()
            .copied()
            .filter(|p| {
                self.cloud
                    .docs
                    .get(p)
                    .is_some_and(|cd| cd.sync.upload_expected())
            })
            .collect()
    }

    /// Bytes sent and to send of the uploads running for `panes`.
    pub(super) fn cloud_upload_progress(&self, panes: &[PaneId]) -> (u64, u64) {
        panes
            .iter()
            .filter_map(|p| self.cloud.docs.get(p))
            .filter_map(|cd| match cd.sync.state {
                kantor_cloud::sync::SyncState::Uploading { sent, total } => Some((sent, total)),
                _ => None,
            })
            .fold((0, 0), |(s, t), (sent, total)| (s + sent, t + total))
    }

    /// Whether any of `panes` still has saves on their way to the server.
    pub(super) fn cloud_upload_pending(&self, panes: &[PaneId]) -> bool {
        panes.iter().any(|p| {
            self.cloud
                .docs
                .get(p)
                .is_some_and(|cd| cd.sync.upload_expected())
        })
    }

    pub(super) fn cloud_uploading(&self, panes: &[PaneId]) -> bool {
        panes.iter().any(|p| {
            self.cloud
                .docs
                .get(p)
                .is_some_and(|cd| cd.sync.is_uploading())
        })
    }

    /// Upload one pane's working copy if it has saves the server lacks.
    pub(super) fn sync_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cd) = self.cloud.docs.get_mut(&pane) else {
            return;
        };
        if cd.sync.is_uploading() {
            return;
        }
        let action = cd.sync.on_reconnect();
        self.run_action(pane, action, window, cx);
    }

    pub(super) fn cloud_autosave_on(&self) -> bool {
        !self.cloud.autosave_off
    }

    /// For a cloud document: whether autosave may save it now (switch on,
    /// not read-only, no conflict waiting). `None`: not a cloud document.
    pub(super) fn cloud_autosaves(&self, pane: PaneId) -> Option<bool> {
        let cd = self.cloud.docs.get(&pane)?;
        Some(
            !self.cloud.autosave_off
                && cd.sync.read_only_by().is_none()
                && !matches!(
                    cd.sync.state,
                    kantor_cloud::sync::SyncState::Conflict { .. }
                ),
        )
    }

    /// The status bar switch on a cloud document.
    pub(super) fn toggle_cloud_autosave(&mut self, cx: &mut Context<Self>) {
        self.cloud.autosave_off = !self.cloud.autosave_off;
        let mut settings = Settings::load();
        settings.cloud.autosave = !self.cloud.autosave_off;
        if let Err(e) = settings.save() {
            log::warn!("saving settings: {e:#}");
        }
        let text = if self.cloud.autosave_off {
            t("Autosave off for cloud documents: choose Save to send your changes to the cloud.")
        } else {
            t(
                "Autosave on for cloud documents: changes go to the cloud a few seconds after you stop typing.",
            )
        };
        self.cloud_notice(text, cx);
        cx.notify();
    }

    /// Whether the focused document is a cloud document (for the palette).
    pub(super) fn focused_is_cloud(&self) -> bool {
        self.active
            .and_then(|i| self.tabs.get(i))
            .is_some_and(|t| self.cloud.docs.contains_key(&t.focused))
    }
}

/// Re-open a restored working copy: check the server, take the lock.
/// Returns the sync state and whether the view should load a fresher
/// server version (the copy has no local changes and the server moved on).
fn reattach(account: &CloudAccount, cache: &Cache, entry: CacheEntry) -> (DocSync, bool) {
    let Ok(r) = remote_for(account) else {
        return (DocSync::offline(entry), false);
    };
    match r.client.stat(&entry.remote) {
        Err(e) if session::is_offline(&e) => (DocSync::offline(entry), false),
        Err(_) => (DocSync::new(entry, LockState::None), false),
        Ok(_) => {
            // Lock first, then check: the lock changes the ETag on
            // Nextcloud, and checking after it lets the base follow.
            let mut entry = entry;
            let (lock, changed) = session::take_lock_settled(&r, cache, &mut entry);
            let changed = changed.unwrap_or(true);
            if changed
                && !entry.has_local_changes()
                && let Ok(fresh) = session::reload(&r, cache, &entry)
            {
                return (DocSync::new(fresh, lock), true);
            }
            let mut sync = DocSync::new(entry, lock);
            if changed {
                sync.state = kantor_cloud::sync::SyncState::Conflict {
                    server_etag: r.client.stat(&sync.entry.remote).ok().and_then(|e| e.etag),
                };
            }
            (sync, false)
        }
    }
}

/// Save to Cloud's network part (worker thread): make the folder, refuse
/// an existing file, put the bytes in a working copy, upload it, take the
/// lock where the drive has locks.
fn upload_new(
    account: &CloudAccount,
    path: &std::path::Path,
    remote: &str,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<(CacheEntry, LockState), String> {
    let r = remote_for(account)?;
    let cache = cache();
    let folder = remote.rsplit_once('/').map_or("", |(f, _)| f);
    if !folder.is_empty() {
        r.client.mkcol(folder).map_err(|e| e.to_string())?;
    }
    let taken =
        || t("There's already a file with this name there. Choose another name.").to_owned();
    match r.client.stat(remote) {
        Ok(_) => return Err(taken()),
        Err(kantor_cloud::Error::NotFound) => {}
        Err(e) => return Err(e.to_string()),
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let local = cache.local_path(&r.account, remote);
    if let Some(dir) = local.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    kantor_cloud::cache::write_atomic(&local, &bytes).map_err(|e| e.to_string())?;
    let mut entry = CacheEntry {
        account: r.account.clone(),
        remote: remote.to_owned(),
        local,
        ..Default::default()
    };
    match session::upload(&r, &cache, &entry, None, false, progress) {
        UploadOutcome::Synced {
            server,
            stamp,
            sha256,
            sha1,
            ..
        } => {
            entry.synced = stamp;
            entry.sha256 = sha256;
            entry.sha1 = sha1;
            if let Some(s) = server {
                entry.rebase(&s);
            }
            cache.save_entry(&entry).map_err(|e| e.to_string())?;
            let lock = session::take_lock(&r, &cache, remote);
            Ok((entry, lock))
        }
        UploadOutcome::Conflict { .. } => Err(taken()),
        UploadOutcome::Offline(m) | UploadOutcome::Failed(m) => Err(m),
        UploadOutcome::LockedBy(who) => Err(read_only_notice(&who)),
        UploadOutcome::Busy => Err(t("The drive is busy. Try again in a moment.").to_owned()),
    }
}

/// An upload's progress callback that passes on at most ~100 reports per
/// upload (and every report where `sent` goes back: a part sent again
/// after a drop).
fn throttled(progress: &dyn Fn((u64, u64))) -> impl FnMut(u64, u64) + '_ {
    let mut last = 0u64;
    move |sent, total| {
        if total == 0 || sent == total || sent < last || (sent - last) * 100 >= total {
            last = sent;
            progress((sent, total));
        }
    }
}

fn name_of(remote: &str) -> &str {
    remote.rsplit('/').next().unwrap_or(remote)
}

fn read_only_notice(who: &str) -> String {
    format!(
        "{} {who}. {}",
        tr(
            "Read-only: this file is locked by",
            "Hanya baca: berkas ini dikunci oleh"
        ),
        tr(
            "Choose Sync now later to check again.",
            "Pilih Sinkronkan sekarang nanti untuk memeriksa lagi.",
        )
    )
}
