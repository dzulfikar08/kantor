//! The conflict dialog: the server's copy changed since this one was opened.
//! Save mine as a copy next to it, reload theirs (after a confirm), or
//! overwrite theirs (after a warning).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input;
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::tr;
use kantor_ui::metrics::*;
use kantor_ui::tokens;

use super::PanelEvent;
use super::widgets::{CLOUD_PANEL, ClosePanel, card, footer, note};

/// What the user chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    SaveCopy,
    Reload,
    Overwrite,
}

pub struct ConflictPanel {
    pane: u64,
    name: String,
    copy_name: String,
    /// A choice waiting for its confirmation.
    confirming: Option<Resolution>,
    focus: FocusHandle,
}

impl EventEmitter<PanelEvent> for ConflictPanel {}

impl ConflictPanel {
    pub fn new(pane: u64, name: String, copy_name: String, cx: &mut Context<Self>) -> Self {
        Self {
            pane,
            name,
            copy_name,
            confirming: None,
            focus: cx.focus_handle(),
        }
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    fn choose(&mut self, r: Resolution, cx: &mut Context<Self>) {
        let needs_confirm = matches!(r, Resolution::Reload | Resolution::Overwrite);
        if needs_confirm && self.confirming != Some(r) {
            self.confirming = Some(r);
            cx.notify();
            return;
        }
        cx.emit(PanelEvent::Resolve {
            pane: self.pane,
            resolution: r,
        });
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if self.confirming.take().is_some() {
            cx.notify();
            return;
        }
        cx.emit(PanelEvent::Close);
    }
}

impl Render for ConflictPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        let option = |id: &'static str,
                      icon: IconName,
                      title: String,
                      detail: String,
                      r: Resolution,
                      cx: &mut Context<Self>| {
            h_flex()
                .id(id)
                .p(SPACE_5)
                .gap(SPACE_5)
                .rounded(RADIUS_MD)
                .border_1()
                .border_color(t.border)
                .bg(t.surface)
                .cursor_pointer()
                .hover(|s| s.border_color(t.border_strong).bg(t.hover))
                .child(
                    gpui_kit::component::Icon::new(icon)
                        .with_size(ICON_LG)
                        .text_color(t.accent),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(SPACE_1)
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.fg)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(FONT_SMALL)
                                .text_color(t.fg_muted)
                                .child(detail),
                        ),
                )
                .on_click(cx.listener(move |this, _, _, cx| this.choose(r, cx)))
        };

        let body = match self.confirming {
            None => v_flex()
                .p(SPACE_7)
                .gap(SPACE_5)
                .child(
                    div().text_color(t.fg).child(format!(
                        "{} “{}” {}",
                        tr("Someone changed", "Seseorang mengubah"),
                        self.name,
                        tr(
                            "on the server after you opened it. Your changes are safe on this computer; choose what to keep.",
                            "di server setelah Anda membukanya. Perubahan Anda aman di komputer ini; pilih yang ingin disimpan.",
                        )
                    )),
                )
                .child(option(
                    "conflict-copy",
                    IconName::Copy,
                    tr("Save my version as a copy", "Simpan versi saya sebagai salinan").into(),
                    format!(
                        "{} “{}”. {}",
                        tr("Uploaded next to it as", "Diunggah di sebelahnya sebagai"),
                        self.copy_name,
                        tr(
                            "This window then shows the server's version.",
                            "Jendela ini lalu menampilkan versi server.",
                        )
                    ),
                    Resolution::SaveCopy,
                    cx,
                ))
                .child(option(
                    "conflict-reload",
                    IconName::RefreshCw,
                    tr("Reload the server's version", "Muat ulang versi server").into(),
                    tr(
                        "Discards your changes to this file.",
                        "Membuang perubahan Anda pada berkas ini.",
                    )
                    .into(),
                    Resolution::Reload,
                    cx,
                ))
                .child(option(
                    "conflict-overwrite",
                    IconName::TriangleAlert,
                    tr("Overwrite the server's version", "Timpa versi server").into(),
                    tr(
                        "Replaces their changes with yours. They lose their work.",
                        "Mengganti perubahan mereka dengan milik Anda. Pekerjaan mereka hilang.",
                    )
                    .into(),
                    Resolution::Overwrite,
                    cx,
                )),
            Some(r) => {
                let (warning, button) = match r {
                    Resolution::Reload => (
                        tr(
                            "Discard your changes and load the server's version? This can't be undone.",
                            "Buang perubahan Anda dan muat versi server? Ini tidak bisa dibatalkan.",
                        ),
                        tr("Discard mine and reload", "Buang milik saya dan muat ulang"),
                    ),
                    _ => (
                        tr(
                            "Overwrite the server's version? The other person's changes will be lost for everyone.",
                            "Timpa versi server? Perubahan orang lain akan hilang untuk semua orang.",
                        ),
                        tr("Overwrite theirs", "Timpa milik mereka"),
                    ),
                };
                v_flex()
                    .p(SPACE_7)
                    .gap(SPACE_5)
                    .child(
                        h_flex()
                            .gap(SPACE_4)
                            .p(SPACE_5)
                            .rounded(RADIUS_MD)
                            .bg(t.danger.opacity(0.12))
                            .child(
                                gpui_kit::component::Icon::new(IconName::TriangleAlert)
                                    .with_size(ICON_LG)
                                    .text_color(t.danger),
                            )
                            .child(div().flex_1().text_color(t.fg).child(warning)),
                    )
                    .child(
                        h_flex()
                            .gap(SPACE_4)
                            .child(div().flex_1())
                            .child(
                                Button::new("conflict-back")
                                    .ghost()
                                    .small()
                                    .label(tr("Back", "Kembali"))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.confirming = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("conflict-confirm")
                                    .danger()
                                    .small()
                                    .label(button)
                                    .on_click(cx.listener(move |this, _, _, cx| this.choose(r, cx))),
                            ),
                    )
            }
        };

        div()
            .id("cloud-conflict")
            .key_context(CLOUD_PANEL)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ClosePanel, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &input::Escape, _, cx| this.close(cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                card(
                    IconName::TriangleAlert,
                    tr("Changed on the server", "Berubah di server"),
                    cx,
                )
                .child(body)
                .child(
                    footer(cx)
                        .child(div().flex_1().min_w_0().child(note(
                            tr(
                                "Until you choose, saving keeps your changes on this computer only.",
                                "Sampai Anda memilih, menyimpan hanya menyimpan perubahan di komputer ini.",
                            ),
                            cx,
                        )))
                        .child(
                            Button::new("conflict-later")
                                .ghost()
                                .small()
                                .label(tr("Decide later", "Putuskan nanti"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.confirming = None;
                                    this.close(cx)
                                })),
                        ),
                ),
            )
    }
}
