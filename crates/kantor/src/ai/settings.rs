//! AI Settings: master switch, provider, API key (keychain), base URL, model,
//! Test connection.

use std::collections::HashMap;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{self, Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{Disableable as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ai::keys::{self, KeyLocation};
use kantor_ai::{Cancel, PROVIDERS, ProviderConfig, ProviderId};
use kantor_core::settings::{AiProviderSettings, AiSettings, Settings};
use kantor_ui::metrics::*;

use super::widgets::{Status, card, field, footer, note, status_line};
use super::{AI_PANEL, ClosePanel, PanelEvent};

const PRIVACY: &str = "Kantor has no AI service of its own: requests go straight from this \
computer to the provider chosen here, and only when you use an AI command. Each command sends \
only what it needs: for formulas, the column headers, up to 20 sample rows near the selection \
and the active cell's address; for rewriting, the selected paragraphs. Nothing is sent while AI \
features are off. API keys are kept in the system keychain, never in the settings file.";

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
    focus: FocusHandle,
}

impl EventEmitter<PanelEvent> for SettingsPanel {}

impl SettingsPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = Settings::load().ai;
        let provider = ProviderId::from_key(&draft.provider).unwrap_or(ProviderId::Anthropic);
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
            focus: cx.focus_handle(),
        };
        this.load_provider_fields(window, cx);
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
                    "Paste your API key"
                } else {
                    "Optional"
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
                    "Model id"
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
        self.status = Status::Busy(format!("Connecting to {}…", provider.info().label));
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
                    Ok(model) => Status::Ok(format!("Connected: {model} answered.")),
                    Err(e) => Status::Error(e.to_string()),
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
                this.status = Status::Ok("Key removed.".into());
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
            self.status = Status::Error(format!("Couldn't save settings: {err:#}"));
            cx.notify();
            return;
        }
        super::refresh_enabled(&self.draft, cx);
        let keys: Vec<(ProviderId, String)> = self.typed_keys.drain().collect();
        if keys.is_empty() {
            cx.emit(PanelEvent::Close);
            return;
        }
        self.status = Status::Busy("Saving the key in the system keychain…".into());
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
                            "Saved. No system keychain was found, so the key is in \
                             ~/.config/kantor/ai-keys.toml (readable only by you)."
                                .into(),
                        );
                    }
                    Err(e) => {
                        this.status = Status::Error(format!("Couldn't save the key: {e}"));
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
        cx.emit(PanelEvent::Close);
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
            .label(info.label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for p in PROVIDERS {
                    let view = view.clone();
                    let id = p.id;
                    menu = menu.item(PopupMenuItem::new(p.label).checked(id == current).on_click(
                        move |_, window, cx| {
                            let _ = view.update(cx, |v, cx| v.set_provider(id, window, cx));
                        },
                    ));
                }
                menu
            });

        let has_key = self.has_key.get(&self.provider).copied();
        let key_note = match has_key {
            Some(true) => format!(
                "A key is saved in the system keychain; type a new one to replace it. {}.",
                info.key_hint
            ),
            _ => format!("{}.", info.key_hint),
        };
        let testing = self.test.is_some();

        let body = v_flex()
            .p(SPACE_7)
            .gap(SPACE_6)
            .child(
                h_flex()
                    .gap(SPACE_4)
                    .child(
                        Switch::new("ai-enabled")
                            .checked(self.draft.enabled)
                            .label("AI features")
                            .on_click(cx.listener(|this, on: &bool, _, cx| {
                                this.draft.enabled = *on;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1())
                    .child(note(
                        if self.draft.enabled {
                            "On"
                        } else {
                            "Off: Kantor never contacts an AI provider"
                        },
                        cx,
                    )),
            )
            .child(field("Provider", provider_menu, cx))
            .child(
                field(
                    "API key",
                    h_flex()
                        .gap(SPACE_3)
                        .child(div().flex_1().child(Input::new(&self.key).mask_toggle()))
                        .when(has_key == Some(true), |d| {
                            d.child(
                                Button::new("ai-forget-key")
                                    .ghost()
                                    .small()
                                    .label("Remove")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.forget_key(window, cx)
                                    })),
                            )
                        }),
                    cx,
                )
                .child(note(key_note, cx)),
            )
            .when(info.base_url_editable, |d| {
                d.child(field("Base URL", Input::new(&self.base_url), cx))
            })
            .child(field("Model", Input::new(&self.model), cx).child(note(
                "Any model id the provider offers; empty uses the default shown.",
                cx,
            )))
            .child(note(PRIVACY, cx));

        div()
            .id("ai-settings")
            .key_context(AI_PANEL)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ClosePanel, _, cx| this.close(cx)))
            .on_action(cx.listener(|this, _: &input::Escape, _, cx| this.close(cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                card("AI Settings", None, cx).child(body).child(
                    footer(cx)
                        .child(div().flex_1().children(status_line(&self.status, cx)))
                        .child(
                            Button::new("ai-test")
                                .outline()
                                .small()
                                .label(if testing {
                                    "Testing…"
                                } else {
                                    "Test connection"
                                })
                                .disabled(testing)
                                .on_click(cx.listener(|this, _, _, cx| this.test_connection(cx))),
                        )
                        .when(!self.saved, |d| {
                            d.child(
                                Button::new("ai-cancel")
                                    .ghost()
                                    .small()
                                    .label("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                            )
                        })
                        .child(
                            Button::new("ai-save")
                                .primary()
                                .small()
                                .label(if self.saved { "Done" } else { "Save" })
                                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                        ),
                ),
            )
    }
}
