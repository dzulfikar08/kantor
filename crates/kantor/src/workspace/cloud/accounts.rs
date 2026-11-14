//! Cloud Accounts: add a WebDAV account (server, user name, app password),
//! test it, remove it. The password goes to the OS keychain
//! (`kantor_ai::keys::CLOUD`), never to settings.toml.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{self, Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ai::keys::{CLOUD, KeyLocation};
use kantor_core::settings::{CloudAccount, Settings};
use kantor_ui::i18n::tr;
use kantor_ui::metrics::*;
use kantor_ui::tokens;

use super::jobs;
use super::widgets::{CLOUD_PANEL, ClosePanel, Status, card, field, footer, note, status_line};
use super::{PanelEvent, remote_for};

pub struct AccountsPanel {
    accounts: Vec<CloudAccount>,
    server: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    status: Status,
    busy: bool,
    /// Empty the form at the next render (needs the window).
    clear_form: bool,
    focus: FocusHandle,
}

impl EventEmitter<PanelEvent> for AccountsPanel {}

impl AccountsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let server =
            cx.new(|cx| InputState::new(window, cx).placeholder("https://drive.example.go.id"));
        let user =
            cx.new(|cx| InputState::new(window, cx).placeholder(tr("User name", "Nama pengguna")));
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(tr("App password", "Kata sandi aplikasi"))
        });
        Self {
            accounts: Settings::load().cloud.accounts,
            server,
            user,
            password,
            status: Status::None,
            busy: false,
            clear_form: false,
            focus: cx.focus_handle(),
        }
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.server.update(cx, |i, cx| i.focus(window, cx));
    }

    /// The account typed in the form (not saved yet), with its password.
    fn typed(&self, cx: &App) -> Option<(CloudAccount, String)> {
        let v = |i: &Entity<InputState>| i.read(cx).value().trim().to_owned();
        let (mut server, user, password) = (v(&self.server), v(&self.user), v(&self.password));
        if server.is_empty() || user.is_empty() || password.is_empty() {
            return None;
        }
        if !server.starts_with("http://") && !server.starts_with("https://") {
            server = format!("https://{server}");
        }
        let id = account_id(&server, &user);
        Some((
            CloudAccount {
                id,
                server,
                user,
                dav_url: String::new(),
            },
            password,
        ))
    }

    fn test_connection(&mut self, cx: &mut Context<Self>) {
        let Some((account, password)) = self.typed(cx) else {
            self.status = Status::Error(
                tr(
                    "Fill in the server, user name and app password first.",
                    "Isi dulu server, nama pengguna dan kata sandi aplikasi.",
                )
                .into(),
            );
            cx.notify();
            return;
        };
        self.busy = true;
        self.status = Status::Busy(tr("Connecting…", "Menghubungkan…").into());
        jobs::spawn(
            cx,
            move |_: &dyn Fn(())| test(&account, Some(password)),
            |_, _, _| {},
            |this: &mut Self, r, cx| {
                this.busy = false;
                this.status = r;
                cx.notify();
            },
        );
        cx.notify();
    }

    fn add(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((account, password)) = self.typed(cx) else {
            self.status = Status::Error(
                tr(
                    "Fill in the server, user name and app password first.",
                    "Isi dulu server, nama pengguna dan kata sandi aplikasi.",
                )
                .into(),
            );
            cx.notify();
            return;
        };
        self.busy = true;
        self.status = Status::Busy(tr("Checking and saving…", "Memeriksa dan menyimpan…").into());
        let _ = window;
        jobs::spawn(
            cx,
            move |_: &dyn Fn(())| {
                // Only save an account that works.
                let checked = test(&account, Some(password.clone()));
                if !matches!(checked, Status::Ok(_)) {
                    return (account, Err(checked));
                }
                let saved = CLOUD.save(&account.id, &password);
                (account, Ok(saved))
            },
            |_, _, _| {},
            |this: &mut Self, (account, r), cx| {
                this.busy = false;
                match r {
                    Err(status) => this.status = status,
                    Ok(Err(e)) => {
                        this.status = Status::Error(format!(
                            "{} {e}",
                            tr(
                                "Couldn't save the password:",
                                "Kata sandi tidak dapat disimpan:"
                            )
                        ))
                    }
                    Ok(Ok(location)) => {
                        let mut settings = Settings::load();
                        settings.cloud.accounts.retain(|a| a.id != account.id);
                        settings.cloud.accounts.push(account);
                        if let Err(e) = settings.save() {
                            this.status = Status::Error(format!("{e:#}"));
                        } else {
                            this.accounts = settings.cloud.accounts;
                            this.clear_form = true;
                            this.status = match location {
                                KeyLocation::Keychain => Status::Ok(
                                    tr(
                                        "Account added. The app password is in the system keychain.",
                                        "Akun ditambahkan. Kata sandi aplikasi tersimpan di keychain sistem.",
                                    )
                                    .into(),
                                ),
                                KeyLocation::File => Status::Warn(
                                    tr(
                                        "Account added. No system keychain was found, so the password is in cloud-keys.toml next to settings.toml (readable only by you).",
                                        "Akun ditambahkan. Keychain sistem tidak ditemukan, jadi kata sandi ada di cloud-keys.toml di sebelah settings.toml (hanya bisa dibaca oleh Anda).",
                                    )
                                    .into(),
                                ),
                            };
                            cx.emit(PanelEvent::AccountsChanged);
                        }
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn test_saved(&mut self, account: CloudAccount, cx: &mut Context<Self>) {
        self.busy = true;
        self.status = Status::Busy(tr("Connecting…", "Menghubungkan…").into());
        jobs::spawn(
            cx,
            move |_: &dyn Fn(())| test(&account, None),
            |_, _, _| {},
            |this: &mut Self, r, cx| {
                this.busy = false;
                this.status = r;
                cx.notify();
            },
        );
        cx.notify();
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        let mut settings = Settings::load();
        settings.cloud.accounts.retain(|a| a.id != id);
        settings.cloud.last_folder.remove(&id);
        if let Err(e) = settings.save() {
            self.status = Status::Error(format!("{e:#}"));
            cx.notify();
            return;
        }
        self.accounts = settings.cloud.accounts;
        jobs::spawn(
            cx,
            move |_: &dyn Fn(())| CLOUD.delete(&id),
            |_, _, _| {},
            |this: &mut Self, _, cx| {
                this.status = Status::Ok(
                    tr(
                        "Account removed, and its password deleted from the keychain.",
                        "Akun dihapus, dan kata sandinya dihapus dari keychain.",
                    )
                    .into(),
                );
                cx.emit(PanelEvent::AccountsChanged);
                cx.notify();
            },
        );
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        cx.emit(PanelEvent::Close);
    }
}

/// Sign in and list the root folder.
fn test(account: &CloudAccount, password: Option<String>) -> Status {
    let remote = match password {
        Some(p) => Ok(super::remote_with(account, p)),
        None => remote_for(account),
    };
    let remote = match remote {
        Ok(r) => r,
        Err(e) => return Status::Error(e),
    };
    match remote.client.list("") {
        Ok(entries) => Status::Ok(format!(
            "{} · {} {}",
            tr("Connected", "Terhubung"),
            entries.len(),
            match (entries.len(), kantor_ui::i18n::lang()) {
                (_, kantor_ui::i18n::Lang::Id) => "item di folder teratas",
                (1, _) => "item in the top folder",
                _ => "items in the top folder",
            }
        )),
        Err(kantor_cloud::Error::Auth(_)) => Status::Error(
            tr(
                "The server refused the user name or app password.",
                "Server menolak nama pengguna atau kata sandi aplikasi.",
            )
            .into(),
        ),
        Err(kantor_cloud::Error::NotFound) => Status::Error(
            tr(
                "No WebDAV folder at that address. Check the server address and user name.",
                "Tidak ada folder WebDAV di alamat itu. Periksa alamat server dan nama pengguna.",
            )
            .into(),
        ),
        Err(e) => Status::Error(format!(
            "{} {e}",
            tr("Can't connect:", "Tidak dapat terhubung:")
        )),
    }
}

/// A stable id from server and user (`drive.example.go.id-budi`), safe as a
/// folder and keychain name.
fn account_id(server: &str, user: &str) -> String {
    let host = server
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('/')
        .next()
        .unwrap_or("");
    format!("{host}-{user}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-._".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

impl Render for AccountsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.clear_form) {
            for input in [&self.server, &self.user, &self.password] {
                input.update(cx, |i, cx| i.set_value("", window, cx));
            }
            window.focus(&self.focus, cx);
        }
        let t = tokens(cx);
        let mut list = v_flex().gap(SPACE_2);
        if self.accounts.is_empty() {
            list = list.child(note(
                tr("No cloud accounts yet.", "Belum ada akun cloud."),
                cx,
            ));
        }
        for (i, a) in self.accounts.iter().enumerate() {
            let (test_a, remove_id) = (a.clone(), a.id.clone());
            list = list.child(
                h_flex()
                    .id(("cloud-account", i))
                    .px(SPACE_4)
                    .py(SPACE_3)
                    .gap(SPACE_4)
                    .rounded(RADIUS_MD)
                    .bg(t.surface)
                    .border_1()
                    .border_color(t.border)
                    .child(
                        gpui_kit::component::Icon::new(IconName::Cloud)
                            .with_size(ICON_MD)
                            .text_color(t.accent),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_color(t.fg).truncate().child(a.label()))
                            .child(
                                div()
                                    .text_size(FONT_CAPTION)
                                    .text_color(t.fg_dim)
                                    .truncate()
                                    .child(a.dav_root()),
                            ),
                    )
                    .child(
                        Button::new(("cloud-test", i))
                            .ghost()
                            .small()
                            .label(tr("Test connection", "Uji koneksi"))
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.test_saved(test_a.clone(), cx)
                            })),
                    )
                    .child(
                        Button::new(("cloud-remove", i))
                            .ghost()
                            .small()
                            .label(tr("Remove", "Hapus"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.remove(remove_id.clone(), cx)
                            })),
                    ),
            );
        }

        let body = v_flex()
            .p(SPACE_7)
            .gap(SPACE_6)
            .child(list)
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.fg)
                    .child(tr("Add an account", "Tambah akun")),
            )
            .child(field(tr("Server", "Server"), Input::new(&self.server), cx))
            .child(field(tr("User name", "Nama pengguna"), Input::new(&self.user), cx))
            .child(
                field(
                    tr("App password", "Kata sandi aplikasi"),
                    Input::new(&self.password).mask_toggle(),
                    cx,
                )
                .child(note(
                    tr(
                        "Make an app password in Nextcloud (Settings → Security → Devices & sessions) or OpenCloud (App tokens). It is kept in the system keychain, never in the settings file.",
                        "Buat kata sandi aplikasi di Nextcloud (Pengaturan → Keamanan → Perangkat & sesi) atau OpenCloud (Token aplikasi). Disimpan di keychain sistem, tidak pernah di berkas pengaturan.",
                    ),
                    cx,
                )),
            );

        div()
            .id("cloud-accounts")
            .key_context(CLOUD_PANEL)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ClosePanel, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &input::Escape, _, cx| this.close(cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                card(IconName::Cloud, tr("Cloud Accounts", "Akun Cloud"), cx)
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
                                Button::new("cloud-test-new")
                                    .outline()
                                    .small()
                                    .label(tr("Test connection", "Uji koneksi"))
                                    .disabled(self.busy)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.test_connection(cx)),
                                    ),
                            )
                            .child(
                                Button::new("cloud-close")
                                    .ghost()
                                    .small()
                                    .label(tr("Close", "Tutup"))
                                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                            )
                            .child(
                                Button::new("cloud-add")
                                    .primary()
                                    .small()
                                    .label(tr("Add account", "Tambah akun"))
                                    .disabled(self.busy)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| this.add(window, cx)),
                                    ),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn account_ids_are_safe_names() {
        assert_eq!(
            account_id("https://drive.example.go.id/", "budi"),
            "drive.example.go.id-budi"
        );
        assert_eq!(
            account_id("http://10.0.0.5:8080", "a b/c"),
            "10.0.0.5_8080-a_b_c"
        );
    }
}
