//! Cloud Accounts: add a WebDAV account (server, user name, app password,
//! or the organisation's single sign-on), a Microsoft OneDrive or a Google
//! Drive account (browser sign-in), test it, sign in again, remove it. The
//! password or the sign-in's refresh token goes to the OS keychain
//! (`kantor_ai::keys::CLOUD`), never to settings.toml.

use gpui_kit::assets::IconName;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{self, Input, InputState};
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ai::keys::{CLOUD, KeyLocation};
use kantor_core::settings::{CloudAccount, Settings};
use kantor_ui::i18n::{self, t, tf, tr};
use kantor_ui::metrics::*;
use kantor_ui::tokens;

use super::jobs;
use super::oauth;
use super::sso::{self, SignInError, SsoRequest, SsoStep};
use super::widgets::{CLOUD_PANEL, ClosePanel, Status, card, field, footer, note, status_line};
use super::{PanelEvent, remote_for};
use kantor_cloud::oauth::Provider;

/// How a new account signs in.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    AppPassword,
    Sso,
    /// OneDrive or Google Drive.
    Oauth(Provider),
}

pub struct AccountsPanel {
    accounts: Vec<CloudAccount>,
    mode: Mode,
    server: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    /// Single sign-on: the sign-in server (issuer) and client id, optional.
    issuer: Entity<InputState>,
    client: Entity<InputState>,
    /// A sign-in in progress: set to cancel it.
    cancel: Option<sso::Cancel>,
    /// A device sign-in's address and code, while it waits.
    device_code: Option<(String, String)>,
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
        let issuer = cx.new(|cx| {
            InputState::new(window, cx).placeholder("https://sso.example.go.id/realms/pemda")
        });
        let client = cx.new(|cx| InputState::new(window, cx).placeholder("kantor"));
        Self {
            accounts: Settings::load().cloud.accounts,
            mode: Mode::AppPassword,
            server,
            user,
            password,
            issuer,
            client,
            cancel: None,
            device_code: None,
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
                ..CloudAccount::default()
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

    /// Sign in with the organisation (`device`: with a code), for a new
    /// account or `again` for an existing one.
    fn sso_sign_in(&mut self, device: bool, again: Option<CloudAccount>, cx: &mut Context<Self>) {
        let v = |i: &Entity<InputState>| i.read(cx).value().trim().to_owned();
        let req = match &again {
            Some(a) => SsoRequest {
                server: a.server.clone(),
                issuer: a.issuer.clone(),
                client_id: a.client_id.clone(),
                device,
                again: Some(a.clone()),
            },
            None => SsoRequest {
                server: v(&self.server),
                issuer: v(&self.issuer),
                client_id: v(&self.client),
                device,
                again: None,
            },
        };
        if req.server.is_empty() {
            self.status = Status::Error(t("Fill in the server address first.").into());
            cx.notify();
            return;
        }
        let cancel = sso::Cancel::default();
        self.cancel = Some(cancel.clone());
        self.device_code = None;
        self.busy = true;
        self.status = Status::Busy(t("Finding where to sign in…").into());
        let again_id = again.as_ref().map(|a| a.id.clone());
        self.run_sign_in(
            again_id,
            move |step: &dyn Fn(SsoStep)| sso::sign_in(req, &cancel, step),
            cx,
        );
    }

    /// Sign in to OneDrive or Google Drive in the browser, for a new
    /// account or `again` for an existing one.
    fn oauth_sign_in(&mut self, p: Provider, again: Option<CloudAccount>, cx: &mut Context<Self>) {
        let cancel = sso::Cancel::default();
        self.cancel = Some(cancel.clone());
        self.device_code = None;
        self.busy = true;
        self.status = Status::Busy(t("Waiting for you to sign in in the browser…").into());
        let again_id = again.as_ref().map(|a| a.id.clone());
        self.run_sign_in(
            again_id,
            move |step: &dyn Fn(SsoStep)| oauth::sign_in(p, again, &cancel, step),
            cx,
        );
    }

    /// Run a sign-in on a worker thread, then keep the account it made
    /// (in place of `again_id`'s, when signing in again).
    fn run_sign_in(
        &mut self,
        again_id: Option<String>,
        job: impl FnOnce(&dyn Fn(SsoStep)) -> Result<(CloudAccount, KeyLocation), SignInError>
        + Send
        + 'static,
        cx: &mut Context<Self>,
    ) {
        jobs::spawn(
            cx,
            job,
            |this: &mut Self, step, cx| {
                match step {
                    SsoStep::OpenBrowser(url) => {
                        cx.open_url(&url);
                        this.status =
                            Status::Busy(t("Waiting for you to sign in in the browser…").into());
                    }
                    SsoStep::DeviceCode { uri, code, open } => {
                        if let Some(u) = &open {
                            cx.open_url(u);
                        }
                        this.status = Status::Busy(t("Waiting for you to enter the code…").into());
                        this.device_code = Some((uri, code));
                    }
                    SsoStep::Progress(text) => {
                        this.device_code = None;
                        this.status = Status::Busy(text);
                    }
                }
                cx.notify();
            },
            move |this: &mut Self, r, cx| {
                this.busy = false;
                // Taken already when the user pressed Cancel here.
                let cancelled_here = this.cancel.take().is_none();
                this.device_code = None;
                match r {
                    Err(SignInError::Failed(e)) => this.status = Status::Error(e),
                    Err(SignInError::Cancelled) => {
                        this.status = if cancelled_here {
                            Status::None
                        } else {
                            Status::Busy(t("Sign-in cancelled. Nothing was changed.").into())
                        }
                    }
                    Ok((account, location)) => {
                        let mut settings = Settings::load();
                        let old = again_id.unwrap_or_else(|| account.id.clone());
                        match settings.cloud.accounts.iter_mut().find(|a| a.id == old) {
                            Some(a) => *a = account,
                            None => settings.cloud.accounts.push(account),
                        }
                        if let Err(e) = settings.save() {
                            this.status = Status::Error(format!("{e:#}"));
                        } else {
                            this.accounts = settings.cloud.accounts;
                            this.clear_form = true;
                            this.status = match location {
                                KeyLocation::Keychain => Status::Ok(
                                    t("Signed in. The sign-in is kept in the system keychain.")
                                        .into(),
                                ),
                                KeyLocation::File => Status::Warn(
                                    t("Signed in. No system keychain was found, so the sign-in is kept in cloud-keys.toml next to settings.toml (readable only by you).")
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

    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.cancel.take() {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.device_code = None;
        self.status = Status::None;
        cx.notify();
    }

    fn remove(&mut self, id: String, cx: &mut Context<Self>) {
        let mut settings = Settings::load();
        let removed = settings.cloud.accounts.iter().find(|a| a.id == id).cloned();
        settings.cloud.accounts.retain(|a| a.id != id);
        settings.cloud.last_folder.remove(&id);
        if let Err(e) = settings.save() {
            self.status = Status::Error(format!("{e:#}"));
            cx.notify();
            return;
        }
        self.accounts = settings.cloud.accounts;
        let sso = removed.as_ref().is_some_and(|a| a.is_sso() || a.is_oauth());
        jobs::spawn(
            cx,
            move |_: &dyn Fn(())| {
                match removed {
                    Some(a) if a.is_oauth() => oauth::sign_out(&a),
                    Some(a) if a.is_sso() => sso::sign_out(&a),
                    _ => {}
                }
                CLOUD.delete(&id)
            },
            |_, _, _| {},
            move |this: &mut Self, _, cx| {
                this.status = Status::Ok(if sso {
                    t("Account removed: signed out, and its sign-in deleted from the keychain.")
                        .into()
                } else {
                    tr(
                        "Account removed, and its password deleted from the keychain.",
                        "Akun dihapus, dan kata sandinya dihapus dari keychain.",
                    )
                    .into()
                });
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
        Err(kantor_cloud::Error::Auth(_)) if account.is_oauth() => {
            Status::Error(sso::ended_message(account))
        }
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
pub(super) fn account_id(server: &str, user: &str) -> String {
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
            for input in [
                &self.server,
                &self.user,
                &self.password,
                &self.issuer,
                &self.client,
            ] {
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
                                    .child(if let Some(p) = a.provider_name() {
                                        p.to_owned()
                                    } else if a.is_sso() {
                                        tf("Single sign-on · {}", &[&a.dav_root()])
                                    } else {
                                        a.dav_root()
                                    }),
                            ),
                    )
                    .when(a.is_sso() || a.is_oauth(), |row| {
                        let again = a.clone();
                        row.child(
                            Button::new(("cloud-sso-again", i))
                                .ghost()
                                .small()
                                .label(i18n::t("Sign in again"))
                                .disabled(self.busy)
                                .on_click(cx.listener(
                                    move |this, _, _, cx| match oauth::provider(&again) {
                                        Some(p) => this.oauth_sign_in(p, Some(again.clone()), cx),
                                        None => this.sso_sign_in(false, Some(again.clone()), cx),
                                    },
                                )),
                        )
                    })
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
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(
                        Button::new("cloud-mode-password")
                            .small()
                            .map(|b| {
                                if self.mode == Mode::AppPassword {
                                    b.primary()
                                } else {
                                    b.ghost()
                                }
                            })
                            .label(tr("App password", "Kata sandi aplikasi"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.mode = Mode::AppPassword;
                                this.status = Status::None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("cloud-mode-sso")
                            .small()
                            .map(|b| {
                                if self.mode == Mode::Sso {
                                    b.primary()
                                } else {
                                    b.ghost()
                                }
                            })
                            .label(i18n::t("Organisation sign-in (SSO)"))
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.mode = Mode::Sso;
                                this.status = Status::None;
                                cx.notify();
                            })),
                    )
                    .children(oauth::available().into_iter().enumerate().map(|(ix, p)| {
                        Button::new(("cloud-mode-oauth", ix))
                            .small()
                            .map(|b| {
                                if self.mode == Mode::Oauth(p) {
                                    b.primary()
                                } else {
                                    b.ghost()
                                }
                            })
                            .label(p.name())
                            .disabled(self.busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.mode = Mode::Oauth(p);
                                this.status = Status::None;
                                cx.notify();
                            }))
                    })),
            )
            .when(!matches!(self.mode, Mode::Oauth(_)), |b| {
                b.child(field(tr("Server", "Server"), Input::new(&self.server), cx))
            });
        let body = match self.mode {
            Mode::Oauth(p) => body.child(note(
                match p {
                    Provider::OneDrive => i18n::t("Sign in with your Microsoft account (personal, work or school) in the browser. Kantor can then open and save the files in your OneDrive. The sign-in is kept in the system keychain; no password is stored."),
                    Provider::GoogleDrive => i18n::t("Sign in with your Google account in the browser. Kantor only gets the files it creates or opens, not the rest of your Drive; new files go in a “Kantor” folder. The sign-in is kept in the system keychain; no password is stored."),
                },
                cx,
            )),
            Mode::AppPassword => body
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
                ),
            Mode::Sso => body
                .child(
                    field(i18n::t("Sign-in server (optional)"), Input::new(&self.issuer), cx).child(
                        note(
                            i18n::t("Only needed when the drive doesn't say where to sign in (Nextcloud): your organisation's identity provider."),
                            cx,
                        ),
                    ),
                )
                .child(field(i18n::t("Client ID (optional)"), Input::new(&self.client), cx))
                .child(note(
                    i18n::t("Kantor opens your browser to sign in with your organisation's account. Without a browser on this computer, use a code instead. The sign-in is kept in the system keychain; no password is stored."),
                    cx,
                ))
                .when_some(self.device_code.clone(), |d, (uri, code)| {
                    d.child(
                        v_flex()
                            .gap(SPACE_2)
                            .p(SPACE_4)
                            .rounded(RADIUS_MD)
                            .bg(t.surface)
                            .border_1()
                            .border_color(t.accent)
                            .child(
                                div()
                                    .text_color(t.fg)
                                    .child(tf("In a browser on any device, go to {} and enter this code:", &[&uri])),
                            )
                            .child(
                                div()
                                    .text_size(FONT_TITLE)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(t.fg)
                                    .child(code),
                            ),
                    )
                }),
        };
        // Single sign-on has three buttons in the footer: its messages go
        // above them at full width rather than in a narrow column beside.
        let status_above = self.mode != Mode::AppPassword;
        let body = body.when(status_above, |b| b.children(status_line(&self.status, cx)));

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
                                div().flex_1().min_w_0().when(!status_above, |d| {
                                    d.children(status_line(&self.status, cx))
                                }),
                            )
                            .when(self.mode == Mode::AppPassword, |f| {
                                f.child(
                                    Button::new("cloud-test-new")
                                        .outline()
                                        .small()
                                        .label(tr("Test connection", "Uji koneksi"))
                                        .disabled(self.busy)
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.test_connection(cx)),
                                        ),
                                )
                            })
                            .child(
                                Button::new("cloud-close")
                                    .ghost()
                                    .small()
                                    .label(tr("Close", "Tutup"))
                                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                            )
                            .when(self.cancel.is_some(), |f| {
                                f.child(
                                    Button::new("cloud-sso-cancel")
                                        .outline()
                                        .small()
                                        .label(i18n::t("Cancel"))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.cancel_sign_in(cx)),
                                        ),
                                )
                            })
                            .when_some(
                                match self.mode {
                                    Mode::Oauth(p) if self.cancel.is_none() => Some(p),
                                    _ => None,
                                },
                                |f, p| {
                                    f.child(
                                        Button::new("cloud-oauth-sign-in")
                                            .primary()
                                            .small()
                                            .label(match p {
                                                Provider::OneDrive => {
                                                    i18n::t("Sign in with Microsoft")
                                                }
                                                Provider::GoogleDrive => {
                                                    i18n::t("Sign in with Google")
                                                }
                                            })
                                            .disabled(self.busy)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.oauth_sign_in(p, None, cx)
                                            })),
                                    )
                                },
                            )
                            .when(self.mode == Mode::Sso && self.cancel.is_none(), |f| {
                                f.child(
                                    Button::new("cloud-sso-device")
                                        .outline()
                                        .small()
                                        .label(i18n::t("Use a code instead"))
                                        .disabled(self.busy)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.sso_sign_in(true, None, cx)
                                        })),
                                )
                                .child(
                                    Button::new("cloud-sso-sign-in")
                                        .primary()
                                        .small()
                                        .label(i18n::t("Sign in with your organisation"))
                                        .disabled(self.busy)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.sso_sign_in(false, None, cx)
                                        })),
                                )
                            })
                            .when(self.mode == Mode::AppPassword, |f| {
                                f.child(
                                    Button::new("cloud-add")
                                        .primary()
                                        .small()
                                        .label(tr("Add account", "Tambah akun"))
                                        .disabled(self.busy)
                                        .on_click(
                                            cx.listener(|this, _, window, cx| this.add(window, cx)),
                                        ),
                                )
                            }),
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
