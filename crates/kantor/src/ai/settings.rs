//! AI Settings: master switch, provider, API key (keychain), base URL, model,
//! Test connection. Paparan AI (hosted credits) shows its account instead of
//! the key: Sign in with a device code, balance, Top up, Sign out.

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{self, Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ai::keys::{self, KeyLocation};
use kantor_ai::paparan::{self as paparan_api, DeviceStart, SignInError};
use kantor_ai::{Cancel, PROVIDERS, ProviderConfig, ProviderId};
use kantor_core::settings::{AiProviderSettings, AiSettings, Settings};
use kantor_ui::i18n::{self, t_dyn, tf};
use kantor_ui::metrics::*;

use super::paparan;
use super::widgets::{Status, card, field, footer, note, status_line};
use super::{AI_PANEL, ClosePanel, PanelEvent};

const KEY_SAVED: &str =
    kantor_ui::i18n::tk("A key is saved in the system keychain; type a new one to replace it. {}.");

const PRIVACY: &str = i18n::tk(
    "With your own API key, requests go straight from this computer to the provider chosen here, and only when you use an AI command. Each command sends only what it needs: for formulas, the column headers, up to 20 sample rows near the selection and the active cell's address; for rewriting, the selected paragraphs. Nothing is sent while AI features are off. API keys are kept in the system keychain, never in the settings file.",
);

pub struct SettingsPanel {
    draft: AiSettings,
    provider: ProviderId,
    key: Entity<InputState>,
    base_url: Entity<InputState>,
    model: Entity<InputState>,
    /// Keys typed but not saved yet, per provider.
    typed_keys: HashMap<ProviderId, String>,
    /// Whether the keychain holds a key for the shown provider (`None`: checking).
    has_key: HashMap<ProviderId, bool>,
    status: Status,
    test: Option<Cancel>,
    /// Saved, but the panel stays open to show a warning.
    saved: bool,
    /// A Paparan sign-in waiting for the browser: its cancel flag and,
    /// once known, the code to show.
    sign_in: Option<(Cancel, Option<DeviceStart>)>,
    /// Signing out of Paparan AI.
    signing_out: bool,
    focus: FocusHandle,
    _paparan: Subscription,
}

const PAPARAN_PRIVACY: &str = i18n::tk(
    "With Paparan AI, requests go to Paparan's servers (app.paparanai.id), which pass them to the AI model, and only when you use an AI command. Each command sends only what it needs, as with your own key. The sign-in is kept in the system keychain, never in the settings file.",
);

/// Paparan AI's models and their names in the menu.
const PAPARAN_MODELS: &[(&str, &str)] = &[
    ("paparan-auto", i18n::tk("Auto (recommended)")),
    ("paparan-fast", i18n::tk("Fast")),
    ("paparan-smart", i18n::tk("Smart")),
];

impl EventEmitter<PanelEvent> for SettingsPanel {}

impl SettingsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = Settings::load().ai;
        let provider = ProviderId::from_key(&draft.provider).unwrap_or(ProviderId::Paparan);
        let key = cx.new(|cx| InputState::new(window, cx).masked(true));
        let base_url = cx.new(|cx| InputState::new(window, cx));
        let model = cx.new(|cx| InputState::new(window, cx));
        let mut this = Self {
            draft,
            provider,
            key,
            base_url,
            model,
            typed_keys: HashMap::new(),
            has_key: HashMap::new(),
            status: Status::None,
            test: None,
            saved: false,
            sign_in: None,
            signing_out: false,
            focus: cx.focus_handle(),
            _paparan: cx.observe_global::<paparan::PaparanState>(|_, cx| cx.notify()),
        };
        this.load_provider_fields(window, cx);
        paparan::refresh(cx);
        this
    }

    pub fn focus_first(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    /// Fill the inputs for `self.provider` and check its stored key.
    fn load_provider_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let info = self.provider.info();
        let saved = self
            .draft
            .providers
            .get(info.key)
            .cloned()
            .unwrap_or_default();
        let typed = self
            .typed_keys
            .get(&self.provider)
            .cloned()
            .unwrap_or_default();
        self.key.update(cx, |i, cx| {
            i.set_value(typed, window, cx);
            i.set_placeholder(
                if info.needs_key {
                    i18n::t("Paste your API key")
                } else {
                    i18n::t("Optional")
                },
                window,
                cx,
            );
        });
        self.base_url.update(cx, |i, cx| {
            i.set_value(saved.base_url.unwrap_or_default(), window, cx);
            i.set_placeholder(info.default_base_url, window, cx);
        });
        self.model.update(cx, |i, cx| {
            i.set_value(saved.model.unwrap_or_default(), window, cx);
            i.set_placeholder(
                if info.default_model.is_empty() {
                    i18n::t("Model id")
                } else {
                    info.default_model
                },
                window,
                cx,
            );
        });
        if !self.has_key.contains_key(&self.provider) {
            let provider = self.provider;
            super::run(
                cx,
                move |_, _| Ok(keys::load_key(provider.key()).is_some()),
                |_, _, _| {},
                move |this: &mut Self, r, cx| {
                    this.has_key.insert(provider, r.unwrap_or(false));
                    cx.notify();
                },
            );
        }
        cx.notify();
    }

    /// Keep what's typed for the shown provider in the draft.
    fn store_provider_fields(&mut self, cx: &App) {
        let info = self.provider.info();
        let text = |i: &Entity<InputState>| {
            let v = i.read(cx).value().trim().to_owned();
            (!v.is_empty()).then_some(v)
        };
        let entry = AiProviderSettings {
            model: text(&self.model),
            base_url: if info.base_url_editable {
                text(&self.base_url)
            } else {
                None
            },
        };
        if entry == AiProviderSettings::default() {
            self.draft.providers.remove(info.key);
        } else {
            self.draft.providers.insert(info.key.to_owned(), entry);
        }
        match text(&self.key) {
            Some(k) => self.typed_keys.insert(self.provider, k),
            None => self.typed_keys.remove(&self.provider),
        };
    }

    fn set_provider(&mut self, id: ProviderId, window: &mut Window, cx: &mut Context<Self>) {
        if id == self.provider {
            return;
        }
        self.store_provider_fields(cx);
        self.provider = id;
        self.status = Status::None;
        self.load_provider_fields(window, cx);
    }

    fn test_connection(&mut self, cx: &mut Context<Self>) {
        self.store_provider_fields(cx);
        if let Some(c) = self.test.take() {
            c.cancel();
        }
        let provider = self.provider;
        let mut ai = self.draft.clone();
        ai.enabled = true;
        ai.provider = provider.key().to_owned();
        let typed = self.typed_keys.get(&provider).cloned();
        self.status = Status::Busy(tf("Connecting to {}…", &[&t_dyn(provider.info().label)]));
        self.test = Some(super::run(
            cx,
            move |cancel, _| {
                let key = typed.or_else(|| keys::load_key(provider.key()));
                let cfg = ProviderConfig::resolve(provider, &ai, key)?;
                let model = cfg.model.clone();
                let client = kantor_ai::Client::new(
                    cfg,
                    std::sync::Arc::new(kantor_ai::UreqTransport::default()),
                );
                client.test_connection(cancel)?;
                Ok(model)
            },
            |_, _, _| {},
            |this: &mut Self, r, cx| {
                this.test = None;
                this.status = match r {
                    Ok(model) => Status::Ok(tf("Connected: {} answered.", &[&model])),
                    Err(e) => Status::Error(paparan::describe(&e)),
                };
                cx.notify();
            },
        ));
        cx.notify();
    }

    fn forget_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let provider = self.provider;
        self.typed_keys.remove(&provider);
        self.key.update(cx, |i, cx| i.set_value("", window, cx));
        super::run(
            cx,
            move |_, _| {
                keys::delete_key(provider.key());
                Ok(())
            },
            |_, _, _| {},
            move |this: &mut Self, _, cx| {
                this.has_key.insert(provider, false);
                this.status = Status::Ok(i18n::t("Key removed.").into());
                cx.notify();
            },
        );
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saved {
            cx.emit(PanelEvent::Close);
            return;
        }
        self.store_provider_fields(cx);
        self.draft.provider = self.provider.key().to_owned();
        let mut settings = Settings::load();
        settings.ai = self.draft.clone();
        if let Err(err) = settings.save() {
            self.status = Status::Error(tf("Couldn't save settings: {}", &[&format!("{err:#}")]));
            cx.notify();
            return;
        }
        super::refresh_enabled(&self.draft, cx);
        let keys: Vec<(ProviderId, String)> = self.typed_keys.drain().collect();
        if keys.is_empty() {
            cx.emit(PanelEvent::Close);
            return;
        }
        self.status = Status::Busy(i18n::t("Saving the key in the system keychain…").into());
        let _ = window;
        super::run(
            cx,
            move |_, _| {
                let mut file = false;
                for (p, k) in &keys {
                    match keys::save_key(p.key(), k) {
                        Ok(KeyLocation::File) => file = true,
                        Ok(KeyLocation::Keychain) => {}
                        Err(e) => return Err(kantor_ai::AiError::Network(e)),
                    }
                }
                Ok(file)
            },
            |_, _, _| {},
            |this: &mut Self, r, cx| {
                match r {
                    Ok(false) => cx.emit(PanelEvent::Close),
                    Ok(true) => {
                        this.saved = true;
                        this.status = Status::Warn(
                            i18n::t("Saved. No system keychain was found, so the key is in ~/.config/kantor/ai-keys.toml (readable only by you).").into(),
                        );
                    }
                    Err(e) => {
                        this.status = Status::Error(tf("Couldn't save the key: {}", &[&e]));
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if let Some(c) = self.test.take() {
            c.cancel();
        }
        self.cancel_sign_in(cx);
        cx.emit(PanelEvent::Close);
    }

    /// Start the Paparan device sign-in: the browser opens the page, the
    /// code shows here until it's confirmed there.
    fn paparan_sign_in(&mut self, cx: &mut Context<Self>) {
        self.cancel_sign_in(cx);
        let cancel = Cancel::default();
        self.sign_in = Some((cancel.clone(), None));
        self.status = Status::Busy(i18n::t("Connecting to Paparan AI…").into());
        paparan::run_steps(
            cx,
            move |step: &dyn Fn(DeviceStart)| -> Result<
                (paparan_api::Account, KeyLocation),
                SignInError,
            > {
                let api = paparan::api();
                let done = paparan_api::sign_in(
                    &api,
                    &paparan_api::device_name(),
                    &cancel,
                    &mut |s| step(s.clone()),
                    &mut paparan_api::sleep_unless_cancelled,
                )?;
                let location = paparan_api::save_token(&done.token)
                    .map_err(|e| SignInError::Ai(kantor_ai::AiError::Network(e)))?;
                Ok((done.account, location))
            },
            |this: &mut Self, start: DeviceStart, cx| {
                // Still waiting for this sign-in (not cancelled meanwhile)?
                if let Some((_, code)) = &mut this.sign_in {
                    cx.open_url(&start.verification_url);
                    *code = Some(start);
                    this.status =
                        Status::Busy(i18n::t("Waiting for you to confirm in the browser…").into());
                    cx.notify();
                }
            },
            |this: &mut Self, r, cx| {
                if this.sign_in.take().is_none() {
                    return;
                }
                this.status = match r {
                    Ok((account, location)) => {
                        paparan::refresh(cx);
                        let who = if account.email.is_empty() {
                            account.name
                        } else {
                            account.email
                        };
                        match location {
                            KeyLocation::Keychain => Status::Ok(tf("Signed in as {}.", &[&who])),
                            KeyLocation::File => Status::Warn(tf(
                                "Signed in as {}. No system keychain was found, so the sign-in is kept in ai-keys.toml next to settings.toml (readable only by you).",
                                &[&who],
                            )),
                        }
                    }
                    Err(SignInError::Ai(kantor_ai::AiError::Cancelled)) => Status::None,
                    Err(SignInError::Denied) => {
                        Status::Error(i18n::t("The sign-in was declined.").into())
                    }
                    Err(SignInError::Expired) => {
                        Status::Error(i18n::t("The sign-in code expired. Sign in again.").into())
                    }
                    Err(SignInError::Ai(e)) => Status::Error(paparan::describe(&e)),
                };
                cx.notify();
            },
        );
        cx.notify();
    }

    fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        if let Some((cancel, _)) = self.sign_in.take() {
            cancel.cancel();
            self.status = Status::None;
            cx.notify();
        }
    }

    /// Sign out: end the sign-in on the server (if reachable) and forget it
    /// here either way.
    fn paparan_sign_out(&mut self, cx: &mut Context<Self>) {
        self.signing_out = true;
        self.status = Status::Busy(i18n::t("Signing out…").into());
        super::run(
            cx,
            |_, _| {
                if let Some(token) = paparan_api::load_token()
                    && let Err(e) = paparan::api().revoke(&token)
                {
                    log::info!("Paparan AI sign-out not confirmed by the server: {e}");
                }
                paparan_api::delete_token();
                Ok(())
            },
            |_, _, _| {},
            |this: &mut Self, _, cx| {
                this.signing_out = false;
                paparan::set_signed_out(cx);
                this.status = Status::Ok(i18n::t("Signed out of Paparan AI.").into());
                cx.notify();
            },
        );
        cx.notify();
    }

    fn set_paparan_model(&mut self, model: &str, window: &mut Window, cx: &mut Context<Self>) {
        let value = if model == paparan_api::DEFAULT_MODEL {
            ""
        } else {
            model
        };
        self.model
            .update(cx, |i, cx| i.set_value(value.to_owned(), window, cx));
        cx.notify();
    }

    /// The Paparan AI account: sign in, or who's signed in and the balance.
    fn paparan_section(&self, cx: &mut Context<Self>) -> Div {
        let t = kantor_ui::tokens(cx);
        let st = paparan::state(cx);
        let mut section = v_flex().gap(SPACE_4);

        if let Some((_, code)) = &self.sign_in {
            let mut dialog = v_flex()
                .gap(SPACE_3)
                .p(SPACE_5)
                .rounded(RADIUS_MD)
                .bg(t.surface)
                .border_1()
                .border_color(t.accent);
            match code {
                None => {
                    dialog = dialog.child(note(i18n::t("Starting the sign-in…"), cx));
                }
                Some(start) => {
                    let url = start.verification_url.clone();
                    dialog = dialog
                        .child(div().text_color(t.fg).child(tf(
                            "Your browser opened {}. Check that it shows this code, then confirm:",
                            &[&start.verification_url],
                        )))
                        .child(
                            div()
                                .text_size(FONT_TITLE)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(t.fg)
                                .child(start.user_code.clone()),
                        )
                        .child(
                            h_flex()
                                .gap(SPACE_3)
                                .child(
                                    Button::new("paparan-open-again")
                                        .outline()
                                        .small()
                                        .label(i18n::t("Open the page again"))
                                        .on_click(move |_, _, cx| cx.open_url(&url)),
                                )
                                .child(
                                    Button::new("paparan-cancel-sign-in")
                                        .ghost()
                                        .small()
                                        .label(i18n::t("Cancel"))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.cancel_sign_in(cx)),
                                        ),
                                ),
                        );
                }
            }
            return section.child(dialog);
        }

        match (st.signed_in, &st.me) {
            (Some(true), Some(me)) => {
                let who = if me.account.name.is_empty() || me.account.name == me.account.email {
                    me.account.email.clone()
                } else {
                    format!("{} ({})", me.account.name, me.account.email)
                };
                let credits_color = if me.credits.low { t.warning } else { t.fg };
                let topup = paparan::topup_url(None, cx);
                let manage = me.manage_url.clone().unwrap_or_else(paparan::web_base);
                section = section
                    .child(
                        h_flex()
                            .gap(SPACE_4)
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap(SPACE_1)
                                    .child(div().text_color(t.fg).child(who))
                                    .when(!me.plan.is_empty(), |d| {
                                        d.child(note(tf("Plan: {}", &[&me.plan]), cx))
                                    }),
                            )
                            .child(
                                v_flex()
                                    .items_end()
                                    .gap(SPACE_1)
                                    .child(
                                        div()
                                            .text_size(FONT_TITLE)
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(credits_color)
                                            .child(me.credits.display()),
                                    )
                                    .child(note(
                                        if me.credits.low {
                                            i18n::t("Running low")
                                        } else {
                                            i18n::t("Credits left")
                                        },
                                        cx,
                                    )),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap(SPACE_3)
                            .child(
                                Button::new("paparan-top-up")
                                    .primary()
                                    .small()
                                    .label(i18n::t("Top up"))
                                    .on_click(move |_, _, cx| cx.open_url(&topup)),
                            )
                            .child(
                                Button::new("paparan-manage")
                                    .outline()
                                    .small()
                                    .label(i18n::t("Manage account"))
                                    .on_click(move |_, _, cx| cx.open_url(&manage)),
                            )
                            .child(
                                Button::new("paparan-refresh")
                                    .ghost()
                                    .small()
                                    .label(if st.refreshing {
                                        i18n::t("Refreshing…")
                                    } else {
                                        i18n::t("Refresh")
                                    })
                                    .disabled(st.refreshing)
                                    .on_click(|_, _, cx| paparan::refresh(cx)),
                            )
                            .child(div().flex_1())
                            .child(
                                Button::new("paparan-sign-out")
                                    .ghost()
                                    .small()
                                    .label(i18n::t("Sign out"))
                                    .disabled(self.signing_out)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.paparan_sign_out(cx)),
                                    ),
                            ),
                    );
            }
            (Some(true), None) => {
                // A token is stored but the account couldn't be fetched.
                section = section
                    .child(note(
                        match &st.error {
                            Some(e) => {
                                tf("Signed in, but the balance couldn't be loaded: {}", &[e])
                            }
                            None => i18n::t("Loading your account…").to_owned(),
                        },
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap(SPACE_3)
                            .child(
                                Button::new("paparan-refresh")
                                    .outline()
                                    .small()
                                    .label(i18n::t("Refresh"))
                                    .disabled(st.refreshing)
                                    .on_click(|_, _, cx| paparan::refresh(cx)),
                            )
                            .child(
                                Button::new("paparan-sign-out")
                                    .ghost()
                                    .small()
                                    .label(i18n::t("Sign out"))
                                    .disabled(self.signing_out)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.paparan_sign_out(cx)),
                                    ),
                            ),
                    );
            }
            (None, _) => {
                section = section.child(note(i18n::t("Checking your Paparan AI sign-in…"), cx));
            }
            (Some(false), _) => {
                section = section
                    .child(note(
                        i18n::t("No API key needed: sign in with your Paparan account and buy credits by QRIS or virtual account."),
                        cx,
                    ))
                    .child(
                        h_flex().child(
                            Button::new("paparan-sign-in")
                                .primary()
                                .small()
                                .label(i18n::t("Sign in to Paparan AI"))
                                .on_click(cx.listener(|this, _, _, cx| this.paparan_sign_in(cx))),
                        ),
                    );
            }
        }
        section
    }

    /// The model menu for Paparan AI.
    fn paparan_model_menu(&self, view: WeakEntity<Self>, cx: &App) -> Button {
        let typed = self.model.read(cx).value().trim().to_owned();
        let current = if typed.is_empty() {
            paparan_api::DEFAULT_MODEL.to_owned()
        } else {
            typed
        };
        let label = PAPARAN_MODELS
            .iter()
            .find(|(id, _)| *id == current)
            .map_or(current.clone(), |(_, name)| t_dyn(name).to_owned());
        Button::new("paparan-model")
            .outline()
            .small()
            .label(label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for &(id, name) in PAPARAN_MODELS {
                    let view = view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(t_dyn(name).to_owned())
                            .checked(id == current)
                            .on_click(move |_, window, cx| {
                                let _ =
                                    view.update(cx, |v, cx| v.set_paparan_model(id, window, cx));
                            }),
                    );
                }
                menu
            })
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let info = self.provider.info();
        let view = cx.entity().downgrade();
        let current = self.provider;
        let provider_menu = Button::new("ai-provider")
            .outline()
            .small()
            .label(t_dyn(info.label).to_owned())
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for p in PROVIDERS {
                    let view = view.clone();
                    let id = p.id;
                    // Paparan AI first; then the free ones with the user's
                    // own key (or none, for local models).
                    if p.id == PROVIDERS[1].id {
                        menu = menu
                            .separator()
                            .item(PopupMenuItem::label(i18n::t("Use your own API key (free)")));
                    }
                    menu = menu.item(
                        PopupMenuItem::new(t_dyn(p.label).to_owned())
                            .checked(id == current)
                            .on_click(move |_, window, cx| {
                                let _ = view.update(cx, |v, cx| v.set_provider(id, window, cx));
                            }),
                    );
                }
                menu
            });

        let has_key = self.has_key.get(&self.provider).copied();
        let key_note = match has_key {
            Some(true) => {
                kantor_ui::i18n::fill(kantor_ui::i18n::t(KEY_SAVED), &[&t_dyn(info.key_hint)])
            }
            _ => format!("{}.", t_dyn(info.key_hint)),
        };
        let testing = self.test.is_some();
        let is_paparan = self.provider == ProviderId::Paparan;
        let paparan_section = is_paparan.then(|| self.paparan_section(cx));
        let paparan_model =
            is_paparan.then(|| self.paparan_model_menu(cx.entity().downgrade(), cx));

        let body = v_flex()
            .p(SPACE_7)
            .gap(SPACE_6)
            .child(
                h_flex()
                    .gap(SPACE_4)
                    .child(
                        Switch::new("ai-enabled")
                            .checked(self.draft.enabled)
                            .label(i18n::t("AI features"))
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.draft.enabled = *on;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(note(
                        if self.draft.enabled {
                            i18n::t("On")
                        } else {
                            i18n::t("Off: Kantor never contacts an AI provider")
                        },
                        cx,
                    )),
            )
            .child(field(i18n::t("Provider"), provider_menu, cx))
            .children(paparan_section)
            .when(!is_paparan, |d| d.child(
                field(
                    i18n::t("API key"),
                    h_flex()
                        .gap(SPACE_3)
                        .child(div().flex_1().child(Input::new(&self.key).mask_toggle()))
                        .when(has_key == Some(true), |d| {
                            d.child(
                                Button::new("ai-forget-key")
                                    .ghost()
                                    .small()
                                    .label(i18n::t("Remove"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.forget_key(window, cx)
                                    })),
                            )
                        }),
                    cx,
                )
                .child(note(key_note, cx)),
            ))
            .when(info.base_url_editable, |d| {
                d.child(field(i18n::t("Base URL"), Input::new(&self.base_url), cx))
            })
            .map(|d| match paparan_model {
                Some(menu) => d.child(
                    field(i18n::t("Model"), h_flex().child(menu), cx).child(note(
                        i18n::t("Auto picks a model for each task; Fast costs fewer credits; Smart is best for long documents."),
                        cx,
                    )),
                ),
                None => d.child(
                    field(i18n::t("Model"), Input::new(&self.model), cx).child(note(
                        i18n::t("Any model id the provider offers; empty uses the default shown."),
                        cx,
                    )),
                ),
            })
            .child(note(
                t_dyn(if is_paparan { PAPARAN_PRIVACY } else { PRIVACY }),
                cx,
            ));

        div()
            .id("ai-settings")
            .key_context(AI_PANEL)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ClosePanel, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &input::Escape, _, cx| this.close(cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                card(i18n::t("AI Settings"), None, cx).child(body).child(
                    footer(cx)
                        .child(div().flex_1().children(status_line(&self.status, cx)))
                        .child(
                            Button::new("ai-test")
                                .outline()
                                .small()
                                .label(if testing {
                                    i18n::t("Testing…")
                                } else {
                                    i18n::t("Test connection")
                                })
                                .disabled(testing)
                                .on_click(cx.listener(|this, _, _, cx| this.test_connection(cx))),
                        )
                        .when(!self.saved, |d| {
                            d.child(
                                Button::new("ai-cancel")
                                    .ghost()
                                    .small()
                                    .label(i18n::t("Cancel"))
                                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                            )
                        })
                        .child(
                            Button::new("ai-save")
                                .primary()
                                .small()
                                .label(if self.saved {
                                    i18n::t("Done")
                                } else {
                                    i18n::t("Save")
                                })
                                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                        ),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    use kantor_ai::PROVIDERS;
    use kantor_ui::i18n::{indonesian, tk};

    /// Provider labels and key hints come from kantor_ai and are shown with
    /// `t_dyn`; these are their catalog keys.
    const PROVIDER_TEXTS: &[&str] = &[
        tk(
            "No API key: sign in with your Paparan account and pay for credits by QRIS or virtual account",
        ),
        tk("Ollama (local)"),
        tk("Other (OpenAI-compatible)"),
        tk("Create a key at console.anthropic.com"),
        tk("Create a key at platform.openai.com"),
        tk("Create a key at openrouter.ai/keys"),
        tk("Create a key at console.groq.com"),
        tk("Create a key at platform.deepseek.com"),
        tk("Create a key at aistudio.google.com"),
        tk("Runs on this computer; nothing leaves it. Install from ollama.com"),
        tk("Optional; sent as a Bearer token"),
    ];

    #[std::prelude::v1::test]
    fn provider_hints_are_translated() {
        for p in PROVIDERS {
            assert!(
                PROVIDER_TEXTS.contains(&p.key_hint),
                "key hint {:?} is not in PROVIDER_TEXTS",
                p.key_hint
            );
        }
        for s in PROVIDER_TEXTS {
            assert!(indonesian(s).is_some(), "{s:?} has no Indonesian");
        }
    }
}
