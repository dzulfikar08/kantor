//! Closing panes, tabs and the app without losing work.
//!
//! Every close goes through [`Workspace::request_close`]: if any document it
//! would drop has unsaved changes, a dialog offers Save / Don't Save / Cancel.
//! Save stores each document in turn (untitled ones ask for a path) and only
//! closes once all of them are saved.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::layout::PaneId;
use sheets_ui::SheetsView;

use super::Workspace;

/// What a close request will remove. Tabs are named by id, not index, so a
/// request stays correct while the dialog is open.
#[derive(Debug, Clone, Copy)]
pub(super) enum Closing {
    Pane { tab: u64, pane: PaneId },
    Tab(u64),
    Quit,
}

impl Workspace {
    fn tab_index(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|t| t.id == id)
    }

    /// Panes a close request would drop, in tab order.
    fn closing_panes(&self, closing: Closing) -> Vec<PaneId> {
        match closing {
            Closing::Pane { pane, .. } => vec![pane],
            Closing::Tab(id) => self
                .tab_index(id)
                .map(|ti| self.tabs[ti].layout.panes())
                .unwrap_or_default(),
            Closing::Quit => self.tabs.iter().flat_map(|t| t.layout.panes()).collect(),
        }
    }

    /// Documents a close request would drop that have unsaved changes.
    fn dirty_views(&self, closing: Closing, cx: &App) -> Vec<Entity<SheetsView>> {
        self.closing_panes(closing)
            .into_iter()
            .filter_map(|pane| self.docs.get(&pane))
            .map(|doc| doc.view.clone())
            .filter(|view| view.read(cx).is_dirty(cx))
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
        } else if !window.has_active_dialog(cx) {
            self.ask_to_save(closing, dirty, window, cx);
        }
    }

    /// The window manager asked to close the window: allow it only when no
    /// work would be lost, else ask (and quit after the answer).
    pub(super) fn should_close_window(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.dirty_views(Closing::Quit, cx).is_empty() {
            self.persist(cx);
            return true;
        }
        self.request_close(Closing::Quit, window, cx);
        false
    }

    fn finish_close(&mut self, closing: Closing, window: &mut Window, cx: &mut Context<Self>) {
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
                cx.quit();
            }
        }
    }

    fn ask_to_save(
        &mut self,
        closing: Closing,
        dirty: Vec<Entity<SheetsView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let names: Vec<String> = dirty.iter().map(|v| v.read(cx).doc_name(cx)).collect();
        let (title, save_label) = match names.as_slice() {
            [one] => (format!("Save changes to “{one}”?"), "Save"),
            many => (
                format!("{} documents have unsaved changes", many.len()),
                "Save All",
            ),
        };
        let detail = match names.len() {
            1 => "Your changes will be lost if you don't save them.".to_owned(),
            _ => format!(
                "{}\n\nYour changes will be lost if you don't save them.",
                names.join(", ")
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
                let ws = ws.clone();
                move |window: &mut Window, cx: &mut App| {
                    let ws = ws.clone();
                    window.defer(cx, move |window, cx| {
                        let _ = ws.update(cx, |ws, cx| ws.finish_close(closing, window, cx));
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
                                .label("Cancel")
                                .ghost()
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("close-discard")
                                .label("Don't Save")
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

    /// Save each document in turn, then close; stop at the first one that
    /// isn't saved (Save As cancelled or a write error).
    fn save_then_close(
        &mut self,
        closing: Closing,
        dirty: Vec<Entity<SheetsView>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |this, cx| {
            for view in dirty {
                let Ok(saved) = view.update_in(cx, |v, window, cx| v.save_for_close(window, cx))
                else {
                    return;
                };
                if !saved.await {
                    let _ = this.update(cx, |ws, cx| {
                        let name = view.read(cx).doc_name(cx);
                        ws.message = format!("Not closed: “{name}” wasn't saved.").into();
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
