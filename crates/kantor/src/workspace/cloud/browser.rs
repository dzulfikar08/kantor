//! Open from Cloud: browse an account's folders (PROPFIND depth 1) and pick
//! a file. The workspace downloads it (see `super::open_remote`).

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_cloud::Entry;
use kantor_core::settings::{CloudAccount, Settings};
use kantor_ui::i18n::tr;
use kantor_ui::metrics::*;
use kantor_ui::tokens;

use super::jobs;
use super::widgets::{CLOUD_PANEL, ClosePanel, Status, card, footer, note, status_line};
use super::{PanelEvent, remote_for};
use crate::workspace::AppKind;

pub struct BrowserPanel {
    accounts: Vec<CloudAccount>,
    account: usize,
    folder: String,
    entries: Vec<Entry>,
    status: Status,
    loading: bool,
    /// Ignore replies to listings the user already navigated away from.
    generation: u64,
    focus: FocusHandle,
    scroll: ScrollHandle,
}

impl EventEmitter<PanelEvent> for BrowserPanel {}

impl BrowserPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let accounts = settings.cloud.accounts;
        let folder = accounts
            .first()
            .and_then(|a| settings.cloud.last_folder.get(&a.id).cloned())
            .unwrap_or_default();
        let mut this = Self {
            accounts,
            account: 0,
            folder,
            entries: Vec::new(),
            status: Status::None,
            loading: false,
            generation: 0,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
        };
        this.load(cx);
        this
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    fn current(&self) -> Option<&CloudAccount> {
        self.accounts.get(self.account)
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(account) = self.current().cloned() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        self.status = Status::Busy(tr("Loading…", "Memuat…").into());
        let folder = self.folder.clone();
        jobs::spawn(
            cx,
            move |_: &dyn Fn(())| {
                let remote = remote_for(&account)?;
                remote.client.list(&folder).map_err(|e| e.to_string())
            },
            |_, _, _| {},
            move |this: &mut Self, r, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match r {
                    Ok(mut entries) => {
                        entries.sort_by(|a, b| {
                            b.is_dir
                                .cmp(&a.is_dir)
                                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                        });
                        this.entries = entries;
                        this.status = Status::None;
                        this.remember_folder();
                    }
                    Err(e) => {
                        this.entries.clear();
                        this.status = Status::Error(e);
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn remember_folder(&self) {
        let Some(account) = self.current() else {
            return;
        };
        let mut settings = Settings::load();
        if settings.cloud.last_folder.get(&account.id) == Some(&self.folder) {
            return;
        }
        settings
            .cloud
            .last_folder
            .insert(account.id.clone(), self.folder.clone());
        if let Err(e) = settings.save() {
            log::warn!("saving the last cloud folder: {e:#}");
        }
    }

    fn go(&mut self, folder: String, cx: &mut Context<Self>) {
        self.folder = folder;
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.load(cx);
    }

    fn up(&mut self, cx: &mut Context<Self>) {
        let parent = self
            .folder
            .trim_end_matches('/')
            .rsplit_once('/')
            .map_or(String::new(), |(p, _)| p.to_owned());
        self.go(parent, cx);
    }

    fn set_account(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.account = ix;
        let id = self.current().map(|a| a.id.clone()).unwrap_or_default();
        let folder = Settings::load()
            .cloud
            .last_folder
            .get(&id)
            .cloned()
            .unwrap_or_default();
        self.go(folder, cx);
    }

    fn pick(&mut self, entry: &Entry, cx: &mut Context<Self>) {
        let path = join(&self.folder, &entry.name);
        if entry.is_dir {
            self.go(path, cx);
            return;
        }
        let Some(account) = self.current().cloned() else {
            return;
        };
        cx.emit(PanelEvent::Open {
            account,
            remote: path,
        });
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(PanelEvent::Close);
    }
}

/// `folder/name` (no leading slash at the root).
fn join(folder: &str, name: &str) -> String {
    let folder = folder.trim_matches('/');
    if folder.is_empty() {
        name.to_owned()
    } else {
        format!("{folder}/{name}")
    }
}

fn icon_for(entry: &Entry) -> IconName {
    if entry.is_dir {
        return IconName::Folder;
    }
    match AppKind::for_path(std::path::Path::new(&entry.name)) {
        Some(AppKind::Sheets) => IconName::FileSpreadsheet,
        Some(AppKind::Docs) => IconName::FileText,
        Some(AppKind::Slides) => IconName::Presentation,
        None => IconName::File,
    }
}

fn human_size(n: u64) -> String {
    match n {
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
        n if n >= 1 << 10 => format!("{} KB", n >> 10),
        n => format!("{n} B"),
    }
}

impl Render for BrowserPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        let view = cx.entity().downgrade();

        if self.accounts.is_empty() {
            return div()
                .id("cloud-browser")
                .key_context(CLOUD_PANEL)
                .track_focus(&self.focus)
                .on_action(cx.listener(|this, _: &ClosePanel, _, cx| this.close(cx)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    card(IconName::Cloud, tr("Open from Cloud", "Buka dari Cloud"), cx)
                        .child(v_flex().p(SPACE_7).gap(SPACE_4).child(note(
                            tr(
                                "Add a cloud account first (Nextcloud, OpenCloud or any WebDAV server).",
                                "Tambahkan akun cloud dulu (Nextcloud, OpenCloud atau server WebDAV lain).",
                            ),
                            cx,
                        )))
                        .child(
                            footer(cx)
                                .child(div().flex_1())
                                .child(
                                    Button::new("cloud-browser-close")
                                        .ghost()
                                        .small()
                                        .label(tr("Close", "Tutup"))
                                        .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                                )
                                .child(
                                    Button::new("cloud-browser-add")
                                        .primary()
                                        .small()
                                        .label(tr("Add account…", "Tambah akun…"))
                                        .on_click(cx.listener(|_, _, _, cx| {
                                            cx.emit(PanelEvent::OpenAccounts)
                                        })),
                                ),
                        ),
                );
        }

        let account_label = self.current().map(|a| a.label()).unwrap_or_default();
        let current = self.account;
        let names: Vec<String> = self.accounts.iter().map(|a| a.label()).collect();
        let account_menu = Button::new("cloud-account-menu")
            .outline()
            .small()
            .icon(IconName::Cloud)
            .label(account_label)
            .dropdown_caret(self.accounts.len() > 1)
            .dropdown_menu(move |mut menu, _, _| {
                for (i, name) in names.iter().enumerate() {
                    let view = view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(i == current)
                            .on_click(move |_, _, cx| {
                                let _ = view.update(cx, |v, cx| v.set_account(i, cx));
                            }),
                    );
                }
                menu
            });

        // Breadcrumb: Home / a / b
        let mut crumbs = h_flex().gap(SPACE_2).flex_wrap().text_size(FONT_SMALL);
        let mut acc = String::new();
        let segs: Vec<String> = self
            .folder
            .split('/')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        crumbs = crumbs.child(
            div()
                .id("crumb-root")
                .px(SPACE_2)
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .text_color(if segs.is_empty() { t.fg } else { t.accent })
                .child(tr("All files", "Semua berkas"))
                .on_click(cx.listener(|this, _, _, cx| this.go(String::new(), cx))),
        );
        for (i, seg) in segs.iter().enumerate() {
            acc = join(&acc, seg);
            let target = acc.clone();
            let last = i + 1 == segs.len();
            crumbs = crumbs.child(div().text_color(t.fg_dim).child("/")).child(
                div()
                    .id(("crumb", i))
                    .px(SPACE_2)
                    .rounded(RADIUS_SM)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .text_color(if last { t.fg } else { t.accent })
                    .child(seg.clone())
                    .on_click(cx.listener(move |this, _, _, cx| this.go(target.clone(), cx))),
            );
        }

        let mut rows = v_flex().gap(px(1.));
        if !self.folder.is_empty() {
            rows = rows.child(
                h_flex()
                    .id("cloud-up")
                    .px(SPACE_4)
                    .py(SPACE_3)
                    .gap(SPACE_4)
                    .rounded(RADIUS_MD)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .child(
                        gpui_kit::component::Icon::new(IconName::ArrowUp)
                            .with_size(ICON_MD)
                            .text_color(t.fg_muted),
                    )
                    .child(div().text_color(t.fg_muted).child(tr("Up", "Naik")))
                    .on_click(cx.listener(|this, _, _, cx| this.up(cx))),
            );
        }
        for (i, e) in self.entries.iter().enumerate() {
            let openable = e.is_dir || AppKind::for_path(std::path::Path::new(&e.name)).is_some();
            let entry = e.clone();
            let detail = if e.is_dir {
                String::new()
            } else {
                e.size.map(human_size).unwrap_or_default()
            };
            rows = rows.child(
                h_flex()
                    .id(("cloud-entry", i))
                    .px(SPACE_4)
                    .py(SPACE_3)
                    .gap(SPACE_4)
                    .rounded(RADIUS_MD)
                    .when(openable, |d| {
                        d.cursor_pointer()
                            .hover(|s| s.bg(t.hover))
                            .on_click(cx.listener(move |this, _, _, cx| this.pick(&entry, cx)))
                    })
                    .child(
                        gpui_kit::component::Icon::new(icon_for(e))
                            .with_size(ICON_MD)
                            .text_color(if e.is_dir { t.accent } else { t.fg_muted }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if openable { t.fg } else { t.fg_dim })
                            .child(e.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(FONT_CAPTION)
                            .text_color(t.fg_dim)
                            .child(detail),
                    ),
            );
        }
        if self.entries.is_empty() && !self.loading && matches!(self.status, Status::None) {
            rows = rows.child(
                div()
                    .px(SPACE_4)
                    .py(SPACE_3)
                    .child(note(tr("This folder is empty.", "Folder ini kosong."), cx)),
            );
        }

        let body = v_flex()
            .p(SPACE_7)
            .gap(SPACE_5)
            .child(
                h_flex()
                    .gap(SPACE_4)
                    .child(account_menu)
                    .child(div().flex_1())
                    .child(
                        Button::new("cloud-refresh")
                            .ghost()
                            .small()
                            .icon(IconName::RefreshCw)
                            .label(tr("Refresh", "Muat ulang"))
                            .disabled(self.loading)
                            .on_click(cx.listener(|this, _, _, cx| this.load(cx))),
                    ),
            )
            .child(crumbs)
            .child(
                div()
                    .id("cloud-entries")
                    .h(px(320.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(t.border)
                    .bg(t.surface)
                    .p(SPACE_2)
                    .child(rows),
            );

        div()
            .id("cloud-browser")
            .key_context(CLOUD_PANEL)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ClosePanel, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &input::Escape, _, cx| this.close(cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                card(
                    IconName::Cloud,
                    tr("Open from Cloud", "Buka dari Cloud"),
                    cx,
                )
                .child(body)
                .child(
                    footer(cx)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .children(status_line(&self.status, cx)),
                        )
                        .child(
                            Button::new("cloud-accounts-link")
                                .ghost()
                                .small()
                                .label(tr("Accounts…", "Akun…"))
                                .on_click(
                                    cx.listener(|_, _, _, cx| cx.emit(PanelEvent::OpenAccounts)),
                                ),
                        )
                        .child(
                            Button::new("cloud-browser-cancel")
                                .ghost()
                                .small()
                                .label(tr("Cancel", "Batal"))
                                .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                        ),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn joins_and_sizes() {
        assert_eq!(join("", "a.xlsx"), "a.xlsx");
        assert_eq!(join("Keuangan/2026/", "a.xlsx"), "Keuangan/2026/a.xlsx");
        assert_eq!(human_size(49_531), "48 KB");
        assert_eq!(human_size(2_033_339), "1.9 MB");
        assert_eq!(human_size(12), "12 B");
    }
}
