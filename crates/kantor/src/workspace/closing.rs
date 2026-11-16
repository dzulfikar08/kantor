//! Closing panes, tabs and the app without losing work.
//!
//! Every close goes through [`Workspace::request_close`]: if any document it
//! would drop has unsaved changes, a dialog offers Save / Don't Save / Cancel.
//! Save stores each document in turn (untitled ones ask for a path) and only
//! closes once all of them are saved. A workbook still being saved in the
//! background counts as unsaved until the write is done: if that save holds
//! all its changes, a "Saving…" dialog waits for it, then closes.

use super::Workspace;
use super::doc::DocView;
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::layout::PaneId;
use kantor_ui::i18n::{t, tf};
use std::time::{Duration, Instant};

/// The longest a close waits for the uploads of documents just saved.
const UPLOAD_WAIT_MAX: Duration = Duration::from_secs(300);
/// A close stops waiting for uploads that sent nothing for this long
/// (a little longer than the client's own stall limit, so a stalled
/// upload normally ends with its real outcome first).
const UPLOAD_STALL: Duration = Duration::from_secs(kantor_cloud::STALL_TIMEOUT.as_secs() + 15);

/// When a close stops waiting for uploads: once none is left, after
/// [`UPLOAD_WAIT_MAX`], or after [`UPLOAD_STALL`] without a byte sent.
/// Uploads that sent everything and wait for the server's reply aren't
/// stalled: the client checks that the server is still there meanwhile.
#[derive(Debug)]
struct UploadWait {
    start: Instant,
    sent: u64,
    moved: Instant,
}

impl UploadWait {
    fn new(now: Instant) -> Self {
        Self {
            start: now,
            sent: 0,
            moved: now,
        }
    }

    /// A check at `now`: uploads still `pending`, and the `(sent, total)`
    /// bytes of the running ones together (`sent` goes down when one
    /// restarts, which counts as moving too).
    fn keep_waiting(&mut self, now: Instant, pending: bool, (sent, total): (u64, u64)) -> bool {
        if !pending {
            return false;
        }
        if sent != self.sent || (total > 0 && sent >= total) {
            self.sent = sent;
            self.moved = now;
        }
        now.duration_since(self.start) < UPLOAD_WAIT_MAX
            && now.duration_since(self.moved) < UPLOAD_STALL
    }
}

/// Upload progress in whole percent (an upload of unknown size: 0).
fn upload_percent(sent: u64, total: u64) -> u8 {
    (sent.min(total) * 100).checked_div(total).unwrap_or(0) as u8
}

/// What a close request will remove. Tabs are named by id, not index, so a
/// request stays correct while the dialog is open.
#[derive(Debug, Clone, Copy)]
pub(super) enum Closing {
    Pane {
        tab: u64,
        pane: PaneId,
    },
    Tab(u64),
    Quit,
    /// Quit, swap in the downloaded update and start it.
    Restart,
}

impl Workspace {
    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Panes a close request would drop, in tab order.
    pub(super) fn closing_panes(&self, closing: Closing) -> Vec<PaneId> {
        match closing {
            Closing::Pane { pane, .. } => vec![pane],
            Closing::Tab(id) => self
                .tab_index(id)
                .map(|ti| self.tabs[ti].layout.panes())
                .unwrap_or_default(),
            Closing::Quit | Closing::Restart => {
                self.tabs.iter().flat_map(|t| t.layout.panes()).collect()
            }
        }
    }

    /// Documents a close request would drop that have unsaved changes or
    /// a save still being written.
    fn dirty_views(&self, closing: Closing, cx: &App) -> Vec<DocView> {
        self.closing_panes(closing)
            .into_iter()
            .filter_map(|pane| self.docs.get(&pane))
            .map(|doc| doc.view.clone())
            .filter(|view| view.is_dirty(cx) || view.is_saving(cx))
            .collect()
    }

    /// Close now if nothing would be lost; otherwise ask first.
    pub(super) fn request_close(
        &mut self,
        closing: Closing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dirty = self.dirty_views(closing, cx);
        if dirty.is_empty() {
            self.finish_close(closing, window, cx);
        } else if window.has_active_dialog(cx) {
        } else if dirty.iter().all(|v| v.save_holds_all_changes(cx)) {
            self.wait_for_saves(closing, dirty, window, cx);
        } else {
            self.ask_to_save(closing, dirty, window, cx);
        }
    }

    /// Every document in the way is being saved with all its changes: show
    /// "Saving…" until the writes finish, then try closing again (a failed
    /// save then asks as usual).
    fn wait_for_saves(
        &mut self,
        closing: Closing,
        saving: Vec<DocView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_wait += 1;
        let id = self.save_wait;
        let names: Vec<String> = saving.iter().map(|v| v.doc_name(cx)).collect();
        let title = match names.as_slice() {
            [one] => tf("Saving “{}”…", &[one]),
            many => tf("Saving {} documents…", &[&many.len()]),
        };
        let ws = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let ws = ws.clone();
            dialog
                .w(px(440.))
                .overlay_closable(false)
                .close_button(false)
                .keyboard(false)
                .title(title.clone())
                .child(v_flex().child(
                    DialogDescription::new().child(t("Kantor closes when the save has finished.")),
                ))
                .footer(
                    DialogFooter::new().child(
                        Button::new("saving-cancel")
                            .label(t("Don't Close"))
                            .ghost()
                            .on_click(move |_, window, cx| {
                                let _ = ws.update(cx, |ws, _| ws.save_wait += 1);
                                window.close_dialog(cx);
                            }),
                    ),
                )
        });
        let waits: Vec<Task<()>> = saving.iter().map(|v| v.wait_saved(cx)).collect();
        cx.spawn_in(window, async move |this, cx| {
            for wait in waits {
                wait.await;
            }
            let _ = this.update_in(cx, |ws, window, cx| {
                if ws.save_wait != id {
                    return;
                }
                window.close_dialog(cx);
                ws.request_close(closing, window, cx);
            });
        })
        .detach();
    }

    /// The window manager asked to close the window: allow it only when no
    /// work would be lost, else ask (and quit after the answer).
    pub(super) fn should_close_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.dirty_views(Closing::Quit, cx).is_empty()
            && self.unsynced_cloud(Closing::Quit).is_empty()
        {
            self.persist(cx);
            self.recovery_quit();
            self.cloud_quit();
            return true;
        }
        self.request_close(Closing::Quit, window, cx);
        false
    }

    /// Quit without asking (a quit signal: logout, shutdown, `kill`):
    /// documents with unsaved changes keep their recovery copies, offered at
    /// the next start; cloud documents whose saves haven't reached the
    /// server keep them in the working copy, synced the next time they're
    /// opened (as "Keep Local Copy"). Cloud locks are released.
    #[cfg(unix)]
    pub fn quit_unattended(&mut self, cx: &mut Context<Self>) {
        self.persist(cx);
        if self.dirty_views(Closing::Quit, cx).is_empty() {
            self.recovery_quit();
        }
        self.cloud_quit();
        cx.quit();
    }

    fn finish_close(&mut self, closing: Closing, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_close_after(closing, false, window, cx);
    }

    /// `waited`: the uploads due were waited for already (see
    /// [`Self::wait_for_uploads`]); what's still not on the server is asked
    /// about.
    fn finish_close_after(
        &mut self,
        closing: Closing,
        waited: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Cloud documents whose saves haven't reached the server: wait for
        // the uploads already on their way (a close right after Save), then
        // ask about the rest.
        if !std::mem::take(&mut self.cloud.close_ok) {
            let unsynced = self.unsynced_cloud(closing);
            if !unsynced.is_empty() {
                if !waited {
                    let due = self.cloud_uploads_due(&unsynced, window, cx);
                    if !due.is_empty() {
                        self.wait_for_uploads(closing, due, window, cx);
                        return;
                    }
                }
                self.ask_unsynced(closing, unsynced, window, cx);
                return;
            }
        }
        match closing {
            Closing::Pane { tab, pane } => {
                if let Some(ti) = self.tab_index(tab) {
                    self.remove_pane(ti, pane, window, cx);
                }
            }
            Closing::Tab(id) => {
                if let Some(ti) = self.tab_index(id) {
                    self.remove_tab(ti, window, cx);
                }
            }
            Closing::Quit => {
                self.persist(cx);
                self.recovery_quit();
                self.cloud_quit();
                cx.quit();
            }
            Closing::Restart => {
                self.persist(cx);
                if self.apply_update_and_relaunch(cx) {
                    self.recovery_quit();
                    self.cloud_quit();
                    cx.quit();
                }
            }
        }
    }

    /// Cloud documents in the way are uploading their saves (or about to):
    /// show the progress until the uploads end, at most [`UPLOAD_WAIT_MAX`]
    /// or until nothing moved for [`UPLOAD_STALL`], then close (or ask
    /// about what didn't reach the server).
    fn wait_for_uploads(
        &mut self,
        closing: Closing,
        panes: Vec<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_wait += 1;
        let id = self.save_wait;
        let names: Vec<String> = panes
            .iter()
            .filter_map(|p| self.docs.get(p))
            .map(|d| d.view.doc_name(cx))
            .collect();
        let title = match names.as_slice() {
            [one] => tf("Uploading “{}” to the cloud…", &[one]),
            many => tf("Uploading {} documents to the cloud…", &[&many.len()]),
        };
        let ws = cx.entity().downgrade();
        let opened = !window.has_active_dialog(cx);
        if opened {
            let panes = panes.clone();
            window.open_dialog(cx, move |dialog, _, cx| {
                let cancel_ws = ws.clone();
                let (sent, total) = ws
                    .upgrade()
                    .map_or((0, 0), |ws| ws.read(cx).cloud_upload_progress(&panes));
                let percent = upload_percent(sent, total);
                dialog
                    .w(px(440.))
                    .overlay_closable(false)
                    .close_button(false)
                    .keyboard(false)
                    .title(title.clone())
                    .child(
                        v_flex()
                            .gap_3()
                            .child(DialogDescription::new().child(t(
                                "Your changes are saved. Closing when they're on the server too.",
                            )))
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        Progress::new("close-upload")
                                            .flex_1()
                                            .value(percent as f32),
                                    )
                                    .child(format!("{percent}%")),
                            ),
                    )
                    .footer(
                        DialogFooter::new().child(
                            Button::new("uploading-cancel")
                                .label(t("Don't Close"))
                                .ghost()
                                .on_click(move |_, window, cx| {
                                    let _ = cancel_ws.update(cx, |ws, _| ws.save_wait += 1);
                                    window.close_dialog(cx);
                                }),
                        ),
                    )
            });
        }
        cx.spawn_in(window, async move |this, cx| {
            let mut wait = UploadWait::new(Instant::now());
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let Ok(state) = this.update(cx, |ws, cx| {
                    cx.notify();
                    (
                        ws.save_wait == id,
                        ws.cloud_upload_pending(&panes),
                        ws.cloud_upload_progress(&panes),
                    )
                }) else {
                    return;
                };
                let (current, pending, progress) = state;
                if !current {
                    return;
                }
                if !wait.keep_waiting(Instant::now(), pending, progress) {
                    break;
                }
            }
            let _ = this.update_in(cx, |ws, window, cx| {
                if ws.save_wait != id {
                    return;
                }
                if opened {
                    window.close_dialog(cx);
                }
                ws.finish_close_after(closing, true, window, cx);
            });
        })
        .detach();
    }

    /// Cloud documents with saves the server doesn't have: try syncing,
    /// keep the local copy and sync later, or cancel.
    fn ask_unsynced(
        &mut self,
        closing: Closing,
        panes: Vec<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if window.has_active_dialog(cx) {
            return;
        }
        use kantor_ui::i18n::tr;
        let names: Vec<String> = panes
            .iter()
            .filter_map(|p| self.docs.get(p))
            .map(|d| d.view.doc_name(cx))
            .collect();
        let title = match names.as_slice() {
            [one] => format!("{} “{one}”", tr("Not synced:", "Belum tersinkron:")),
            many => format!(
                "{} {}",
                many.len(),
                tr(
                    "cloud documents aren't synced",
                    "dokumen cloud belum tersinkron"
                )
            ),
        };
        let mut detail = tr(
            "Your changes are saved on this computer, but not on the server yet. If you keep the local copy, Kantor syncs it the next time you open the file.",
            "Perubahan Anda tersimpan di komputer ini, tetapi belum di server. Jika Anda menyimpan salinan lokal, Kantor menyinkronkannya saat berkas dibuka lagi.",
        )
        .to_owned();
        if names.len() > 1 {
            detail = format!(
                "{}

{detail}",
                names.join(", ")
            );
        }
        let ws = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, _, _| {
            let (sync_ws, keep_ws) = (ws.clone(), ws.clone());
            let panes = panes.clone();
            dialog
                .w(px(460.))
                .overlay_closable(false)
                .close_button(false)
                .title(title.clone())
                .child(v_flex().child(DialogDescription::new().child(detail.clone())))
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("unsynced-cancel")
                                .label(tr("Cancel", "Batal"))
                                .ghost()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("unsynced-keep")
                                .label(tr("Keep Local Copy", "Simpan Salinan Lokal"))
                                .outline()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let ws = keep_ws.clone();
                                    window.defer(cx, move |window, cx| {
                                        let _ = ws.update(cx, |ws, cx| {
                                            ws.cloud.close_ok = true;
                                            ws.finish_close(closing, window, cx);
                                        });
                                    });
                                }),
                        )
                        .child(
                            Button::new("unsynced-sync")
                                .label(tr("Sync Now", "Sinkronkan Sekarang"))
                                .primary()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    let (ws, panes) = (sync_ws.clone(), panes.clone());
                                    window.defer(cx, move |window, cx| {
                                        let _ = ws.update(cx, |ws, cx| {
                                            ws.sync_then_close(closing, panes, window, cx)
                                        });
                                    });
                                }),
                        ),
                )
        });
    }

    /// "Sync Now" in the close dialog: upload, wait, then close if all
    /// went through (else say what's left and stay open).
    fn sync_then_close(
        &mut self,
        closing: Closing,
        panes: Vec<PaneId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for &pane in &panes {
            self.sync_pane(pane, window, cx);
        }
        cx.spawn_in(window, async move |this, cx| {
            for _ in 0..1200 {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(250))
                    .await;
                let Ok(busy) = this.update(cx, |ws, _| ws.cloud_uploading(&panes)) else {
                    return;
                };
                if !busy {
                    break;
                }
            }
            let _ = this.update_in(cx, |ws, window, cx| {
                if ws.unsynced_cloud(closing).is_empty() {
                    ws.finish_close(closing, window, cx);
                } else {
                    ws.notice(
                        kantor_ui::i18n::tr(
                            "Not closed: some changes couldn't be synced.",
                            "Tidak ditutup: sebagian perubahan tidak dapat disinkronkan.",
                        )
                        .into(),
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    fn ask_to_save(
        &mut self,
        closing: Closing,
        dirty: Vec<DocView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let names: Vec<String> = dirty
            .iter()
            .map(|v| {
                let name = v.doc_name(cx);
                if v.is_saving(cx) {
                    tf("{} (saving…)", &[&name])
                } else {
                    name
                }
            })
            .collect();
        let (title, save_label) = match names.as_slice() {
            [one] => (tf("Save changes to “{}”?", &[one]), t("Save")),
            many => (
                tf("{} documents have unsaved changes", &[&many.len()]),
                t("Save All"),
            ),
        };
        let detail = match names.len() {
            1 => t("Your changes will be lost if you don't save them.").to_owned(),
            _ => format!(
                "{}\n\n{}",
                names.join(", "),
                t("Your changes will be lost if you don't save them.")
            ),
        };
        let ws = cx.entity().downgrade();

        window.open_dialog(cx, move |dialog, _, _| {
            let save = {
                let (ws, dirty) = (ws.clone(), dirty.clone());
                move |window: &mut Window, cx: &mut App| {
                    let (ws, dirty) = (ws.clone(), dirty.clone());
                    window.defer(cx, move |window, cx| {
                        let _ =
                            ws.update(cx, |ws, cx| ws.save_then_close(closing, dirty, window, cx));
                    });
                }
            };
            let discard = {
                let (ws, dirty) = (ws.clone(), dirty.clone());
                move |window: &mut Window, cx: &mut App| {
                    let (ws, dirty) = (ws.clone(), dirty.clone());
                    window.defer(cx, move |window, cx| {
                        let _ = ws.update(cx, |ws, cx| {
                            ws.discard_then_close(closing, dirty, window, cx)
                        });
                    });
                }
            };
            let on_ok = save.clone();
            dialog
                .w(px(440.))
                .overlay_closable(false)
                .close_button(false)
                .title(title.clone())
                .child(v_flex().child(DialogDescription::new().child(detail.clone())))
                // Enter saves; Escape cancels (the dialog's default).
                .on_ok(move |_, window, cx| {
                    on_ok(window, cx);
                    true
                })
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("close-cancel")
                                .label(t("Cancel"))
                                .ghost()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("close-discard")
                                .label(t("Don't Save"))
                                .danger()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    discard(window, cx);
                                }),
                        )
                        .child(
                            Button::new("close-save")
                                .label(save_label)
                                .primary()
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    save(window, cx);
                                }),
                        ),
                )
        });
    }

    /// "Don't Save": close without saving, once saves already being
    /// written have finished (a file is never left half replaced).
    fn discard_then_close(
        &mut self,
        closing: Closing,
        dirty: Vec<DocView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !dirty.iter().any(|v| v.is_saving(cx)) {
            self.finish_close(closing, window, cx);
            return;
        }
        let waits: Vec<Task<()>> = dirty.iter().map(|v| v.wait_saved(cx)).collect();
        self.message = t("Closing when the save has finished…").into();
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            for wait in waits {
                wait.await;
            }
            let _ = this.update_in(cx, |ws, window, cx| ws.finish_close(closing, window, cx));
        })
        .detach();
    }

    /// Save each document in turn, then close; stop at the first one that
    /// isn't saved (Save As cancelled or a write error).
    fn save_then_close(
        &mut self,
        closing: Closing,
        dirty: Vec<DocView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            for view in dirty {
                let Ok(saved) = this.update_in(cx, |_, window, cx| view.save_for_close(window, cx))
                else {
                    return;
                };
                if !saved.await {
                    let _ = this.update(cx, |ws, cx| {
                        let name = view.doc_name(cx);
                        ws.message = tf("Not closed: “{}” wasn't saved.", &[&name]).into();
                        cx.notify();
                    });
                    return;
                }
            }
            let _ = this.update_in(cx, |ws, window, cx| ws.finish_close(closing, window, cx));
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KB: u64 = 1_000;

    #[std::prelude::v1::test]
    fn upload_wait_ends_when_uploads_end() {
        let t0 = Instant::now();
        let mut w = UploadWait::new(t0);
        assert!(w.keep_waiting(t0 + Duration::from_secs(1), true, (0, 9 * KB)));
        assert!(w.keep_waiting(t0 + Duration::from_secs(2), true, (KB, 9 * KB)));
        assert!(!w.keep_waiting(t0 + Duration::from_secs(3), false, (0, 0)));
    }

    #[std::prelude::v1::test]
    fn upload_wait_gives_up_on_a_stalled_upload() {
        let t0 = Instant::now();
        let second = Duration::from_secs(1);
        // Connecting: nothing sent yet, still within the stall bound.
        let mut w = UploadWait::new(t0);
        assert!(w.keep_waiting(t0 + UPLOAD_STALL - second, true, (0, 9 * KB)));
        assert!(!w.keep_waiting(t0 + UPLOAD_STALL, true, (0, 9 * KB)));
        // Progress resets the stall clock.
        let mut w = UploadWait::new(t0);
        let t1 = t0 + Duration::from_secs(20);
        assert!(w.keep_waiting(t1, true, (KB, 9 * KB)));
        assert!(w.keep_waiting(t1 + UPLOAD_STALL - second, true, (KB, 9 * KB)));
        assert!(!w.keep_waiting(t1 + UPLOAD_STALL, true, (KB, 9 * KB)));
        // A restarted upload (sent goes back) counts as moving.
        let mut w = UploadWait::new(t0);
        assert!(w.keep_waiting(t0 + Duration::from_secs(10), true, (9 * KB, 9 * KB)));
        let t2 = t0 + UPLOAD_STALL + Duration::from_secs(5);
        assert!(w.keep_waiting(t2, true, (0, 9 * KB)));
    }

    /// Everything sent, the server still assembling the file: not a stall
    /// (the client gives up by itself when the server stops answering).
    #[std::prelude::v1::test]
    fn upload_wait_lets_the_server_finish() {
        let t0 = Instant::now();
        let mut w = UploadWait::new(t0);
        assert!(w.keep_waiting(t0 + Duration::from_secs(1), true, (9 * KB, 9 * KB)));
        assert!(w.keep_waiting(t0 + UPLOAD_STALL * 2, true, (9 * KB, 9 * KB)));
        assert!(!w.keep_waiting(t0 + UPLOAD_WAIT_MAX, true, (9 * KB, 9 * KB)));
    }

    #[std::prelude::v1::test]
    fn upload_wait_is_bounded_even_while_moving() {
        let t0 = Instant::now();
        let mut w = UploadWait::new(t0);
        let total = 1_000 * KB;
        let mut sent = 0;
        let mut t = t0;
        while t < t0 + UPLOAD_WAIT_MAX {
            sent += 1;
            assert!(w.keep_waiting(t, true, (sent, total)));
            t += Duration::from_secs(10);
        }
        assert!(!w.keep_waiting(t0 + UPLOAD_WAIT_MAX, true, (sent + 1, total)));
    }

    #[std::prelude::v1::test]
    fn upload_percent_rounds_down_and_caps() {
        assert_eq!(upload_percent(0, 0), 0);
        assert_eq!(upload_percent(1, 3), 33);
        assert_eq!(upload_percent(3, 3), 100);
        assert_eq!(upload_percent(5, 3), 100);
    }
}
