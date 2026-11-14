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
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::layout::PaneId;
use kantor_ui::i18n::{t, tf};

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

    fn finish_close(&mut self, closing: Closing, window: &mut Window, cx: &mut Context<Self>) {
        // Cloud documents whose saves haven't reached the server: ask first.
        if !std::mem::take(&mut self.cloud.close_ok) {
            let unsynced = self.unsynced_cloud(closing);
            if !unsynced.is_empty() {
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
