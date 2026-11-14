//! Read-only documents, for any app and any reason: a cloud file locked by
//! someone else, or a document the user made read-only.
//!
//! A read-only pane is wrapped in `kantor_ui::read_only::Gate`, which stops
//! editing actions and typed text before the view sees them, and shows a
//! banner with the reason. Edits that get through another way (a toolbar
//! button, a dialog, a drag) are undone as soon as the document reports a
//! change ([`Workspace::revert_read_only_edits`]). MCP edits and AI panels
//! are refused. Selecting, copying, finding, zooming, printing, exporting
//! and Save As keep working.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::layout::PaneId;
use kantor_ui::i18n::tr;
use kantor_ui::metrics::*;
use kantor_ui::read_only::Guard;
use kantor_ui::tokens;

use super::Workspace;

/// Why a document is read-only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum ReadOnly {
    /// The user chose it (palette: Make Read-Only).
    Manual,
    /// A cloud file locked by `owner`; `free` once the lock is gone.
    Locked { owner: String, free: bool },
}

impl ReadOnly {
    fn text(&self) -> String {
        match self {
            Self::Manual => tr(
                "Read-only. Choose Allow Editing to change it.",
                "Hanya baca. Pilih Izinkan Pengeditan untuk mengubahnya.",
            )
            .into(),
            Self::Locked { owner, free: false } => format!(
                "{} {owner}. {}",
                tr("Read-only: locked by", "Hanya baca: dikunci oleh"),
                tr(
                    "You can read, copy, print and export it.",
                    "Anda dapat membaca, menyalin, mencetak dan mengekspornya.",
                )
            ),
            Self::Locked { owner, free: true } => format!(
                "{owner} {}",
                tr(
                    "closed the file. It's no longer locked.",
                    "telah menutup berkas. Berkas tidak lagi dikunci.",
                )
            ),
        }
    }
}

impl Workspace {
    pub(super) fn read_only(&self, pane: PaneId) -> Option<&ReadOnly> {
        self.docs.get(&pane)?.read_only.as_ref()
    }

    pub(super) fn set_read_only(
        &mut self,
        pane: PaneId,
        ro: Option<ReadOnly>,
        cx: &mut Context<Self>,
    ) {
        if let Some(doc) = self.docs.get_mut(&pane)
            && doc.read_only != ro
        {
            doc.read_only = ro;
            cx.notify();
        }
    }

    /// The focused pane, if it is read-only: say so and return `true`
    /// (for commands that would edit it).
    pub(super) fn refuse_if_read_only(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(pane) = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.focused)
        else {
            return false;
        };
        if self.read_only(pane).is_some() {
            self.refused(cx);
            return true;
        }
        false
    }

    fn refused(&mut self, cx: &mut Context<Self>) {
        self.notice(
            tr("This document is read-only.", "Dokumen ini hanya baca.").into(),
            cx,
        );
    }

    /// The gate's guard for a read-only pane.
    pub(super) fn read_only_guard(&self, pane: PaneId, cx: &mut Context<Self>) -> Option<Guard> {
        let doc = self.docs.get(&pane)?;
        doc.read_only.as_ref()?;
        let ws = cx.entity().downgrade();
        Some(Guard {
            editor: Some(doc.view.editor_focus(cx)),
            block_double_click: doc.view.kind() == super::AppKind::Sheets,
            refuse: Rc::new(move |_, cx| {
                let _ = ws.update(cx, |ws, cx| ws.refused(cx));
            }),
        })
    }

    /// Undo whatever reached a read-only document (see the module docs).
    pub(super) fn revert_read_only_edits(
        &mut self,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(doc) = self.docs.get(&pane) else {
            return;
        };
        if doc.read_only.is_none() || !doc.view.is_dirty(cx) {
            return;
        }
        let view = doc.view.clone();
        for _ in 0..50 {
            if !view.is_dirty(cx) {
                break;
            }
            view.undo(window, cx);
        }
        self.refused(cx);
    }

    /// Palette: Make Read-Only.
    pub(super) fn make_read_only(&mut self, cx: &mut Context<Self>) {
        let Some(pane) = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.focused)
        else {
            return;
        };
        let dirty = self.docs.get(&pane).is_some_and(|d| d.view.is_dirty(cx));
        if dirty {
            self.notice(
                tr(
                    "Save or discard your changes first.",
                    "Simpan atau buang perubahan Anda dulu.",
                )
                .into(),
                cx,
            );
            return;
        }
        if self.read_only(pane).is_none() {
            self.set_read_only(pane, Some(ReadOnly::Manual), cx);
        }
    }

    /// Palette: Allow Editing (a manual read-only), or Edit Now (a cloud
    /// file whose lock is gone: takes the lock, reloads if it changed).
    pub(super) fn allow_editing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pane) = self
            .active
            .and_then(|i| self.tabs.get(i))
            .map(|t| t.focused)
        else {
            return;
        };
        match self.read_only(pane) {
            Some(ReadOnly::Manual) => self.set_read_only(pane, None, cx),
            Some(ReadOnly::Locked { .. }) => self.edit_now(pane, window, cx),
            None => {}
        }
    }

    /// The banner above a read-only pane.
    pub(super) fn render_read_only_banner(
        &self,
        pane: PaneId,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        let ro = self.read_only(pane)?.clone();
        let t = tokens(cx);
        let ws = cx.entity().downgrade();
        let (label, color) = match &ro {
            ReadOnly::Locked { free: true, .. } => (tr("Edit now", "Edit sekarang"), t.success),
            ReadOnly::Locked { .. } => (tr("Check again", "Periksa lagi"), t.warning),
            ReadOnly::Manual => (tr("Allow Editing", "Izinkan Pengeditan"), t.warning),
        };
        Some(
            h_flex()
                .flex_none()
                .w_full()
                .px(SPACE_6)
                .py(SPACE_3)
                .gap(SPACE_4)
                .bg(color.opacity(0.14))
                .border_b_1()
                .border_color(color.opacity(0.4))
                .text_size(FONT_SMALL)
                .text_color(t.fg)
                .child(
                    gpui_kit::component::Icon::new(IconName::Lock)
                        .with_size(ICON_SM)
                        .text_color(color),
                )
                .child(div().flex_1().min_w_0().child(ro.text()))
                .child(
                    Button::new(("read-only-action", pane as usize))
                        .small()
                        .when_else(
                            matches!(ro, ReadOnly::Locked { free: true, .. }),
                            |b| b.primary(),
                            |b| b.outline(),
                        )
                        .label(label)
                        .on_click(move |_, window, cx| {
                            let _ = ws.update(cx, |ws, cx| {
                                if let Some(i) = ws
                                    .tabs
                                    .iter()
                                    .position(|t| t.layout.panes().contains(&pane))
                                {
                                    ws.tabs[i].focused = pane;
                                }
                                match ws.read_only(pane) {
                                    Some(ReadOnly::Locked { free: false, .. }) => {
                                        ws.check_lock(pane, window, cx)
                                    }
                                    _ => ws.allow_editing(window, cx),
                                }
                            });
                        }),
                ),
        )
    }
}
