//! Nobody loses work to a crash, a power cut or a forgotten save.
//!
//! * **Recovery copies** (always on): a document with unsaved changes gets a
//!   copy in the recovery store (`kantor_core::recovery`) two minutes after
//!   its first unsaved change (at a pause in typing), and right away when the
//!   window loses focus. The UI thread only snapshots the document; the copy
//!   is built and written in the background. Copies go away when the
//!   document is saved, closed (Don't Save included) or Kantor quits normally.
//! * **After a crash** the next Kantor offers the copies it finds ("Recover
//!   unsaved work"). Restore opens a copy as the original document, unsaved;
//!   Save writes to the original, asking first if that file changed since.
//! * **Autosave** (`[autosave] enabled` in settings.toml, off by default):
//!   documents with a file in their own format are saved after
//!   `idle_seconds` without changes. Imported files (.csv, .ods, .xls, .ppt) and
//!   files with content Kantor can't keep only ever get recovery copies.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::layout::{Layout, PaneId};
use kantor_core::recovery::{self, DocInfo, FileStamp, Found, Scan, Slot, Store};
use kantor_core::settings::{AutosaveSettings, Settings};
use kantor_ui::disk_guard::DiskGuard;
use kantor_ui::metrics::*;
use kantor_ui::tokens;

use super::doc::DocView;
use super::{AppKind, Tab, Workspace};

/// A copy is due this long after a document's first unsaved change.
const INTERVAL: Duration = Duration::from_secs(120);
/// ...at a pause in typing at least this long.
const PAUSE: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_secs(1);

/// The workspace's recovery and autosave state.
#[derive(Default)]
pub(super) struct Recovery {
    store: Option<Store>,
    docs: HashMap<PaneId, DocState>,
    autosave: AutosaveSettings,
    interval: Option<Duration>,
    /// A disk error was reported (once per session, not every tick).
    warned: bool,
    _ticker: Option<Task<()>>,
    _activation: Option<Subscription>,
}

/// One document's recovery bookkeeping.
struct DocState {
    slot: Option<Arc<Slot>>,
    /// The revision last seen, and when it changed.
    seen: Option<u64>,
    changed_at: Instant,
    /// The revision the slot's copy holds.
    copied: Option<u64>,
    /// When the oldest change not in a copy was made.
    first_uncopied: Option<Instant>,
    copying: bool,
    autosaving: bool,
    /// Restored from a copy: the slot holds the first revision seen.
    restored: bool,
    was_dirty: bool,
    /// The document's file as Kantor last read or wrote it.
    baseline: Option<(PathBuf, Option<FileStamp>)>,
    /// Autosave was skipped because the file changed on disk (said once).
    autosave_blocked: bool,
}

impl DocState {
    fn new(now: Instant) -> Self {
        Self {
            slot: None,
            seen: None,
            changed_at: now,
            copied: None,
            first_uncopied: None,
            copying: false,
            autosaving: false,
            restored: false,
            was_dirty: false,
            baseline: None,
            autosave_blocked: false,
        }
    }

    fn baseline_stamp(&self, path: Option<&std::path::Path>) -> Option<FileStamp> {
        let (p, stamp) = self.baseline.as_ref()?;
        (Some(p.as_path()) == path).then_some(*stamp)?
    }
}

impl Recovery {
    fn interval(&self) -> Duration {
        self.interval.unwrap_or(INTERVAL)
    }
}

fn app_of(kind: AppKind) -> recovery::App {
    match kind {
        AppKind::Sheets => recovery::App::Sheets,
        AppKind::Docs => recovery::App::Docs,
        AppKind::Slides => recovery::App::Slides,
    }
}

fn kind_of(app: recovery::App) -> AppKind {
    match app {
        recovery::App::Sheets => AppKind::Sheets,
        recovery::App::Docs => AppKind::Docs,
        recovery::App::Slides => AppKind::Slides,
    }
}

/// "just now", "5 minutes ago", "yesterday", "3 days ago".
pub(super) fn ago(saved_at: u64, now: u64) -> String {
    let s = now.saturating_sub(saved_at);
    let plural = |n: u64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match s {
        0..60 => "just now".into(),
        60..3600 => plural(s / 60, "minute"),
        3600..86_400 => plural(s / 3600, "hour"),
        86_400..172_800 => "yesterday".into(),
        _ => plural(s / 86_400, "day"),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The copies on offer while the "Recover unsaved work" dialog is open.
struct Offer {
    found: Vec<Found>,
    damaged: usize,
    damaged_dir: PathBuf,
}

impl Workspace {
    /// Start recovery copies and autosave, and offer copies left by a crash.
    pub(super) fn init_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rec = &mut self.recovery;
        rec.store = Store::default_location();
        rec.autosave = Settings::load().autosave;
        rec.interval = std::env::var("KANTOR_RECOVERY_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(Duration::from_secs);
        rec._ticker = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this
                    .update_in(cx, |ws, window, cx| ws.recovery_tick(false, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        }));
        rec._activation = Some(cx.observe_window_activation(window, |ws, window, cx| {
            if !window.is_window_active() {
                ws.recovery_tick(true, window, cx);
            }
        }));
        let Some(store) = rec.store.clone() else {
            log::warn!("no data directory: recovery copies are off");
            return;
        };
        let scan = cx.background_spawn(async move { store.scan() });
        cx.spawn_in(window, async move |this, cx| {
            let scan = scan.await;
            let _ = this.update_in(cx, |ws, window, cx| ws.offer_recovery(scan, window, cx));
        })
        .detach();
    }

    /// Turn autosave on or off (palette) and remember it.
    pub(super) fn toggle_autosave(&mut self, cx: &mut Context<Self>) {
        let mut settings = Settings::load();
        settings.autosave.enabled = !self.recovery.autosave.enabled;
        self.recovery.autosave = settings.autosave.clone();
        if let Err(e) = settings.save() {
            log::warn!("saving settings: {e:#}");
        }
        let text = if settings.autosave.enabled {
            format!(
                "Autosave on: documents with a file save {} s after your last change",
                settings.autosave.idle().as_secs()
            )
        } else {
            "Autosave off (recovery copies still protect unsaved work)".into()
        };
        self.notice(text, cx);
    }

    /// Look for copies again (after "Not Now", or to check).
    pub(super) fn reoffer_recovery(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(store) = self.recovery.store.clone() else {
            return;
        };
        let scan = cx.background_spawn(async move { store.scan() });
        cx.spawn_in(window, async move |this, cx| {
            let scan = scan.await;
            let _ = this.update_in(cx, |ws, window, cx| {
                if scan.found.is_empty() && scan.damaged.is_empty() {
                    ws.notice("No unsaved work to recover".into(), cx);
                }
                ws.offer_recovery(scan, window, cx);
            });
        })
        .detach();
    }

    pub(super) fn autosave_enabled(&self) -> bool {
        self.recovery.autosave.enabled
    }

    /// Every second, and with `now` when the window loses focus: copy
    /// documents whose changes are due a recovery copy, drop copies of saved
    /// documents, autosave.
    fn recovery_tick(&mut self, now: bool, window: &mut Window, cx: &mut Context<Self>) {
        let t = Instant::now();
        let views: Vec<(PaneId, DocView)> = self
            .docs
            .iter()
            .map(|(id, d)| (*id, d.view.clone()))
            .collect();
        let interval = self.recovery.interval();
        for (pane, view) in views {
            let Some(rev) = view.revision(cx) else {
                continue;
            };
            let dirty = view.is_dirty(cx);
            let path = view.path(cx);
            let st = self
                .recovery
                .docs
                .entry(pane)
                .or_insert_with(|| DocState::new(t));
            if st.seen != Some(rev) {
                if st.seen.is_none() && st.restored {
                    st.copied = Some(rev);
                }
                st.seen = Some(rev);
                st.changed_at = t;
            }
            if !dirty {
                // Opened, saved, or undone back to the saved state.
                if st.was_dirty || st.baseline.as_ref().map(|b| &b.0) != path.as_ref() {
                    st.baseline = path.map(|p| {
                        let stamp = FileStamp::of(&p);
                        (p, stamp)
                    });
                    st.autosave_blocked = false;
                }
                st.was_dirty = false;
                if st.copied.is_some() || st.restored || st.copying {
                    st.copied = None;
                    st.restored = false;
                    st.first_uncopied = None;
                    if let Some(slot) = st.slot.clone() {
                        let upto = slot.mark_cleared();
                        cx.background_spawn(async move {
                            if let Err(e) = slot.delete_cleared(upto) {
                                log::warn!("removing a recovery copy: {e}");
                            }
                        })
                        .detach();
                    }
                }
                continue;
            }
            st.was_dirty = true;
            if st.copied != Some(rev) && !st.copying {
                let first = *st.first_uncopied.get_or_insert(t);
                let paused = t.duration_since(st.changed_at) >= PAUSE;
                if now || (t.duration_since(first) >= interval && paused) {
                    self.start_copy(pane, &view, rev, cx);
                }
            }
            self.maybe_autosave(pane, &view, t, window, cx);
        }
        let took = t.elapsed();
        if took > Duration::from_millis(30) {
            log::info!("recovery tick took {took:.1?}");
        }
    }

    /// Snapshot `view` now and write its recovery copy in the background.
    fn start_copy(&mut self, pane: PaneId, view: &DocView, rev: u64, cx: &mut Context<Self>) {
        let Some(store) = self.recovery.store.clone() else {
            return;
        };
        let st = self.recovery.docs.get_mut(&pane).expect("state");
        let slot = match &st.slot {
            Some(slot) => slot.clone(),
            None => match store.create_slot() {
                Ok(slot) => {
                    let slot = Arc::new(slot);
                    st.slot = Some(slot.clone());
                    slot
                }
                Err(e) => {
                    st.first_uncopied = Some(Instant::now());
                    self.recovery_failed(&view.doc_name(cx), &e.into(), cx);
                    return;
                }
            },
        };
        let ticket = slot.ticket();
        let t = Instant::now();
        let Some((snapshot, info)) = view.recovery_snapshot(cx) else {
            return;
        };
        log::info!(
            "recovery snapshot of {} took {:.1?} on the UI thread",
            view.doc_name(cx),
            t.elapsed()
        );
        let st = self.recovery.docs.get_mut(&pane).expect("state");
        let original_stamp = st.baseline_stamp(info.original.as_deref());
        let doc = DocInfo {
            app: app_of(view.kind()),
            name: view.doc_name(cx),
            original: info.original,
            original_stamp,
            source_format: info.source_format,
            unsupported: info.unsupported,
        };
        st.copying = true;
        let write = cx.background_spawn(async move {
            let t = Instant::now();
            let bytes = snapshot.render()?;
            let rendered = t.elapsed();
            let written = slot.write(ticket, &doc, &bytes)?;
            log::info!(
                "recovery copy of {}: built in {rendered:.1?}, written in {:.1?} ({} KB)",
                doc.name,
                t.elapsed() - rendered,
                bytes.len() / 1024
            );
            Ok::<_, anyhow::Error>(written)
        });
        cx.spawn(async move |this, cx| {
            let result = write.await;
            let _ = this.update(cx, |ws, cx| ws.copy_done(pane, rev, result, cx));
        })
        .detach();
    }

    fn copy_done(
        &mut self,
        pane: PaneId,
        rev: u64,
        result: anyhow::Result<bool>,
        cx: &mut Context<Self>,
    ) {
        let Some(st) = self.recovery.docs.get_mut(&pane) else {
            return;
        };
        st.copying = false;
        match result {
            Ok(_) => {
                st.copied = Some(rev);
                st.first_uncopied = (st.seen != Some(rev)).then_some(st.changed_at);
            }
            Err(e) => {
                st.first_uncopied = Some(Instant::now());
                let name = self
                    .docs
                    .get(&pane)
                    .map(|d| d.view.doc_name(cx))
                    .unwrap_or_default();
                self.recovery_failed(&name, &e, cx);
            }
        }
    }

    fn recovery_failed(&mut self, name: &str, e: &anyhow::Error, cx: &mut Context<Self>) {
        log::warn!("recovery copy of {name}: {e:#}");
        if !self.recovery.warned {
            self.recovery.warned = true;
            self.notice(
                format!("Couldn't keep a recovery copy of “{name}” ({e}). Save your work."),
                cx,
            );
        }
    }

    fn maybe_autosave(
        &mut self,
        pane: PaneId,
        view: &DocView,
        now: Instant,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let settings = &self.recovery.autosave;
        if !settings.enabled {
            return;
        }
        let idle = settings.idle();
        let st = self.recovery.docs.get_mut(&pane).expect("state");
        if st.autosaving || now.duration_since(st.changed_at) < idle || !view.can_autosave(cx) {
            return;
        }
        let Some(path) = view.path(cx) else {
            return;
        };
        // Never over a file someone else changed since Kantor read it.
        let unchanged = st
            .baseline
            .as_ref()
            .is_some_and(|(p, stamp)| *p == path && FileStamp::of(p) == *stamp);
        if !unchanged {
            if !st.autosave_blocked {
                st.autosave_blocked = true;
                let name = view.doc_name(cx);
                self.notice(
                    format!("“{name}” changed on disk since it was opened: not autosaved"),
                    cx,
                );
            }
            return;
        }
        st.autosaving = true;
        let task = view.autosave(window, cx);
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |ws, cx| {
                let Some(st) = ws.recovery.docs.get_mut(&pane) else {
                    return;
                };
                st.autosaving = false;
                match result {
                    Ok(()) => {
                        st.baseline = Some((path.clone(), FileStamp::of(&path)));
                        cx.notify();
                    }
                    Err(e) => log::warn!("autosave of {}: {e:#}", path.display()),
                }
            });
        })
        .detach();
    }

    /// `pane` was closed (saved, or "Don't Save"): its copy goes.
    pub(super) fn forget_recovery(&mut self, pane: PaneId, cx: &mut Context<Self>) {
        if let Some(slot) = self.recovery.docs.remove(&pane).and_then(|s| s.slot) {
            slot.mark_cleared();
            cx.background_spawn(async move {
                if let Err(e) = slot.remove() {
                    log::warn!("removing a recovery copy: {e}");
                }
            })
            .detach();
        }
    }

    /// Normal quit: every copy of this window goes (waits for writes in
    /// progress; they are skipped).
    pub(super) fn recovery_quit(&mut self) {
        for (_, st) in self.recovery.docs.drain() {
            if let Some(slot) = st.slot
                && let Err(e) = slot.remove()
            {
                log::warn!("removing a recovery copy: {e}");
            }
        }
    }

    // ── offering copies after a crash ──────────────────────────────────────

    fn offer_recovery(&mut self, scan: Scan, window: &mut Window, cx: &mut Context<Self>) {
        if scan.found.is_empty() && scan.damaged.is_empty() {
            return;
        }
        let damaged_dir = self
            .recovery
            .store
            .as_ref()
            .map(|s| s.root().join("damaged"))
            .unwrap_or_default();
        if scan.found.is_empty() {
            self.notice(
                format!(
                    "{} damaged recovery cop{} couldn't be read; kept in {}",
                    scan.damaged.len(),
                    if scan.damaged.len() == 1 { "y" } else { "ies" },
                    damaged_dir.display()
                ),
                cx,
            );
            return;
        }
        let offer = Rc::new(RefCell::new(Offer {
            found: scan.found,
            damaged: scan.damaged.len(),
            damaged_dir,
        }));
        self.open_recovery_dialog(offer, window, cx);
    }

    fn open_recovery_dialog(
        &mut self,
        offer: Rc<RefCell<Offer>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            // Another dialog first; ask again shortly.
            cx.spawn_in(window, async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
                let _ = this.update_in(cx, |ws, window, cx| {
                    ws.open_recovery_dialog(offer, window, cx)
                });
            })
            .detach();
            return;
        }
        let ws = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, cx| {
            let t = tokens(cx);
            let o = offer.borrow();
            let now = unix_now();
            let rows = o.found.iter().enumerate().map(|(i, f)| {
                let doc = &f.manifest.doc;
                let kind = kind_of(doc.app);
                let place = match &doc.original {
                    Some(p) => p.display().to_string(),
                    None => "Never saved".to_owned(),
                };
                let id = f.id.clone();
                let restore = {
                    let (ws, offer, id) = (ws.clone(), offer.clone(), id.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let (ws, offer, id) = (ws.clone(), offer.clone(), id.clone());
                        window.defer(cx, move |window, cx| {
                            let _ = ws.update(cx, |ws, cx| {
                                ws.resolve_offer(&offer, &[id], true, window, cx)
                            });
                        });
                    }
                };
                let discard = {
                    let (ws, offer) = (ws.clone(), offer.clone());
                    move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                        let (ws, offer, id) = (ws.clone(), offer.clone(), id.clone());
                        window.defer(cx, move |window, cx| {
                            let _ = ws.update(cx, |ws, cx| {
                                ws.resolve_offer(&offer, &[id], false, window, cx)
                            });
                        });
                    }
                };
                h_flex()
                    .id(("recover-row", i))
                    .w_full()
                    .gap(SPACE_4)
                    .py(SPACE_3)
                    .px(SPACE_4)
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.surface)
                    .child(
                        div()
                            .flex_none()
                            .size(px(10.))
                            .rounded_full()
                            .bg(kind.color(&t)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(FONT_BODY)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.fg)
                                    .truncate()
                                    .child(doc.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(FONT_SMALL)
                                    .text_color(t.fg_muted)
                                    .truncate()
                                    .child(format!(
                                        "{} · {} · changes from {}",
                                        kind.name(),
                                        place,
                                        ago(f.manifest.saved_at, now)
                                    )),
                            ),
                    )
                    .child(
                        Button::new(("recover-discard", i))
                            .label("Discard")
                            .ghost()
                            .small()
                            .on_click(discard),
                    )
                    .child(
                        Button::new(("recover-restore", i))
                            .label("Restore")
                            .primary()
                            .small()
                            .on_click(restore),
                    )
            });
            let n = o.found.len();
            let detail = if n == 1 {
                "Kantor closed before this document was saved. Restore it to keep working on \
                 your changes; nothing is written to the original file until you save."
                    .to_owned()
            } else {
                format!(
                    "Kantor closed before these {n} documents were saved. Restore them to keep \
                     working on your changes; nothing is written to the original files until \
                     you save."
                )
            };
            let damaged = (o.damaged > 0).then(|| {
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(format!(
                        "{} damaged recovery cop{} couldn't be read; kept in {}",
                        o.damaged,
                        if o.damaged == 1 { "y" } else { "ies" },
                        o.damaged_dir.display()
                    ))
            });
            let all = |restore: bool| {
                let (ws, offer) = (ws.clone(), offer.clone());
                move |window: &mut Window, cx: &mut App| {
                    let (ws, offer) = (ws.clone(), offer.clone());
                    window.defer(cx, move |window, cx| {
                        let ids: Vec<String> =
                            offer.borrow().found.iter().map(|f| f.id.clone()).collect();
                        let _ = ws.update(cx, |ws, cx| {
                            ws.resolve_offer(&offer, &ids, restore, window, cx)
                        });
                    });
                }
            };
            let on_ok = all(true);
            let restore_all = all(true);
            let discard_all = all(false);
            dialog
                .w(px(560.))
                .overlay_closable(false)
                .close_button(false)
                .title("Recover unsaved work")
                .child(
                    v_flex()
                        .gap(SPACE_4)
                        .child(DialogDescription::new().child(detail))
                        .child(
                            v_flex()
                                .id("recover-list")
                                .gap(SPACE_2)
                                .max_h(px(320.))
                                .overflow_y_scroll()
                                .children(rows),
                        )
                        .children(damaged),
                )
                // Enter restores everything; Escape is Not Now.
                .on_ok(move |_, window, cx| {
                    on_ok(window, cx);
                    false
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("recover-later")
                                .label("Not Now")
                                .ghost()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .when(n > 1, |f| {
                            f.child(
                                Button::new("recover-discard-all")
                                    .label("Discard All")
                                    .danger()
                                    .on_click(move |_, window, cx| discard_all(window, cx)),
                            )
                            .child(
                                Button::new("recover-restore-all")
                                    .label("Restore All")
                                    .primary()
                                    .on_click(move |_, window, cx| restore_all(window, cx)),
                            )
                        }),
                )
        });
    }

    /// Restore or discard the copies `ids` on offer; close the dialog once
    /// none are left.
    fn resolve_offer(
        &mut self,
        offer: &Rc<RefCell<Offer>>,
        ids: &[String],
        restore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = self.recovery.store.clone() else {
            return;
        };
        let picked: Vec<Found> = offer
            .borrow()
            .found
            .iter()
            .filter(|f| ids.contains(&f.id))
            .cloned()
            .collect();
        for found in picked {
            let result = if restore {
                self.restore(&store, &found, window, cx)
            } else {
                store.discard(&found.id).map_err(Into::into)
            };
            match result {
                Ok(()) => offer.borrow_mut().found.retain(|f| f.id != found.id),
                Err(e) => {
                    log::warn!("recovery copy {}: {e:#}", found.id);
                    let what = if restore { "restore" } else { "discard" };
                    self.notice(
                        format!("Couldn't {what} “{}”: {e}", found.manifest.doc.name),
                        cx,
                    );
                    // Another Kantor took it, or it's gone: no longer on offer.
                    offer.borrow_mut().found.retain(|f| f.id != found.id);
                }
            }
        }
        if offer.borrow().found.is_empty() {
            window.close_dialog(cx);
            // Closing the dialog gives focus back to what had it before;
            // the restored document should have it.
            let ws = cx.entity().downgrade();
            window.defer(cx, move |window, cx| {
                let _ = ws.update(cx, |ws, cx| {
                    let active = ws.active;
                    ws.activate(active, window, cx);
                });
            });
        }
        window.refresh();
    }

    /// Open the copy `found` in a new tab as its original, unsaved.
    fn restore(
        &mut self,
        store: &Store,
        found: &Found,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        let slot = store.claim(&found.id)?;
        let doc = &found.manifest.doc;
        // The original open, unchanged (the session reopened it): the
        // restored copy takes its place.
        if let Some(orig) = &doc.original
            && let Some((_, pane)) = self.find_path(orig, cx)
            && self.docs.get(&pane).is_some_and(|d| !d.view.is_dirty(cx))
        {
            self.remove_doc(pane, window, cx);
        }
        let view = DocView::new_recovered(found, window, cx)?;
        if doc.original.is_none() {
            view.set_untitled_name(doc.name.clone(), cx);
        }
        let guard = doc
            .original
            .clone()
            .map(|p| DiskGuard::new(p, doc.original_stamp));
        view.set_disk_guard(guard, cx);
        let pane = self.insert_doc(view, window, cx);
        let mut st = DocState::new(Instant::now());
        st.slot = Some(Arc::new(slot));
        st.restored = true;
        st.was_dirty = true;
        st.baseline = doc.original.clone().map(|p| (p, doc.original_stamp));
        self.recovery.docs.insert(pane, st);
        let id = self.alloc_id();
        self.tabs.push(Tab {
            id,
            layout: Layout::Leaf(pane),
            focused: pane,
        });
        self.activate(Some(self.tabs.len() - 1), window, cx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn ago_reads_naturally() {
        let now = 1_000_000;
        assert_eq!(ago(now - 5, now), "just now");
        assert_eq!(ago(now - 60, now), "1 minute ago");
        assert_eq!(ago(now - 600, now), "10 minutes ago");
        assert_eq!(ago(now - 7200, now), "2 hours ago");
        assert_eq!(ago(now - 90_000, now), "yesterday");
        assert_eq!(ago(now - 3 * 86_400, now), "3 days ago");
        assert_eq!(ago(now + 50, now), "just now", "clock skew");
    }
}
