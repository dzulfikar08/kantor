//! e-meterai and certified signatures through licensed providers
//! ([`kantor_esign`]): the e-Sign Providers settings, Add e-Meterai… and
//! Sign with Provider….
//!
//! Nothing goes to a provider until the user saved its account here and
//! ticked "Allow Kantor to send documents to …". Credentials go to the
//! system keychain. The stamped or signed document comes back from the
//! provider, is checked (it must be a PDF; earlier signatures must still
//! verify, or the user agreed to lose them) and saved as a new file, which
//! is then opened.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::settings::{EsignProviderSettings, Settings};
use kantor_esign::check::Kept;
use kantor_esign::emeterai::{self, DocType, StampRequest};
use kantor_esign::signer::{SignRequest, Signer};
use kantor_esign::{Account, Credentials, Error, Info, Kind, PROVIDERS, Support, providers};
use kantor_pdf::sign::Place;
use kantor_ui::i18n::{self, t_dyn, tf, tk};
use kantor_ui::{metrics::*, tokens};

use crate::certsign::Show;
use crate::markup::Tool;
use crate::{PdfEvent, PdfView};

/// What the menu says when no provider is set up.
pub(crate) const SET_UP: &str = tk("Set up a provider in e-Sign Providers first.");

pub(crate) enum Esign {
    Providers(Box<ProvidersDlg>),
    Stamp(Box<StampDlg>),
    Sign(Box<SignDlg>),
}

pub(crate) struct ProvidersDlg {
    /// Index into [`PROVIDERS`].
    sel: usize,
    /// The settings being edited (all providers).
    draft: Vec<EsignProviderSettings>,
    /// One input per credential field of the selected provider.
    inputs: Vec<Entity<InputState>>,
    base_url: Entity<InputState>,
    /// Whether the keychain holds credentials, per provider (`None`:
    /// checking).
    saved: HashMap<&'static str, bool>,
    status: Option<(bool, String)>,
    working: bool,
}

pub(crate) struct StampDlg {
    providers: Vec<&'static Info>,
    provider: usize,
    doc_types: Vec<DocType>,
    doc_type: Option<usize>,
    loading: bool,
    doc_number: Entity<InputState>,
    reason: Entity<InputState>,
    location: Entity<InputState>,
    /// Signed signatures in the document.
    signed: usize,
    /// The user agreed the provider may break them.
    accept_break: bool,
    drawn: Option<(usize, [f32; 4])>,
    hidden: bool,
    working: bool,
    error: Option<String>,
}

pub(crate) struct SignDlg {
    providers: Vec<&'static Info>,
    provider: usize,
    signer: Entity<InputState>,
    otp: Entity<InputState>,
    /// Where the provider sent the OTP.
    otp_sent: Option<String>,
    show: Show,
    fields: Vec<String>,
    reason: Entity<InputState>,
    location: Entity<InputState>,
    signed: usize,
    accept_break: bool,
    drawn: Option<(usize, [f32; 4])>,
    hidden: bool,
    working: bool,
    error: Option<String>,
}

fn err_text(e: &Error) -> String {
    match e {
        Error::NotConfigured(p) => tf(
            "{} isn't set up: add your provider account in e-Sign Providers.",
            &[p],
        ),
        Error::NetworkOff(p) => tf(
            "Kantor isn't allowed to send documents to {}. Turn it on in e-Sign Providers.",
            &[p],
        ),
        Error::Auth(m) => tf("The provider refused the credentials: {}", &[m]),
        Error::Quota(m) => tf("No e-meterai quota left: {}", &[m]),
        Error::OtpRequired(m) => tf("Type the one-time password (OTP): {}", &[m]),
        Error::BreaksSignatures(f) => tf(
            "The provider rebuilt the file, which breaks the earlier signatures ({}). Nothing was saved.",
            &[&f.join(", ")],
        ),
        other => other.to_string(),
    }
}

fn input(
    text: &str,
    placeholder: &'static str,
    masked: bool,
    window: &mut Window,
    cx: &mut Context<PdfView>,
) -> Entity<InputState> {
    let ph = i18n::t(placeholder);
    let text = text.to_string();
    let e = cx.new(|cx| InputState::new(window, cx).masked(masked).placeholder(ph));
    e.update(cx, |i, cx| i.set_value(text, window, cx));
    e
}

fn stem(name: &str) -> String {
    std::path::Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "document".into())
}

impl PdfView {
    /// The providers the menu would use (from settings, no keychain).
    pub(crate) fn esign_configured(kind: Kind) -> Vec<&'static Info> {
        kantor_esign::configured(&Settings::load().pdf.esign_providers, kind)
    }

    // ---- e-Sign Providers ----------------------------------------------

    /// e-Sign Providers…: accounts for e-meterai and certified signatures.
    pub fn open_esign_providers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let draft = Settings::load().pdf.esign_providers;
        let base_url = input("", tk("Server address (optional)"), false, window, cx);
        let dlg = ProvidersDlg {
            sel: draft
                .first()
                .and_then(|s| PROVIDERS.iter().position(|p| p.id == s.id))
                .unwrap_or(0),
            draft,
            inputs: Vec::new(),
            base_url,
            saved: HashMap::new(),
            status: None,
            working: false,
        };
        self.esign = Some(Esign::Providers(Box::new(dlg)));
        self.esign_load_fields(window, cx);
        // Which providers have credentials saved (keychain: background).
        let task = cx.background_spawn(async move {
            PROVIDERS
                .iter()
                .map(|p| (p.id, !kantor_esign::creds::load(p.id).is_empty()))
                .collect::<HashMap<_, _>>()
        });
        cx.spawn(async move |this, cx| {
            let saved = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(Esign::Providers(d)) = &mut this.esign {
                    d.saved = saved;
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Make the inputs for the selected provider.
    fn esign_load_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Esign::Providers(d)) = &self.esign else {
            return;
        };
        let info = PROVIDERS[d.sel];
        let url = d
            .draft
            .iter()
            .find(|s| s.id == info.id)
            .map(|s| s.base_url.clone())
            .unwrap_or_default();
        let inputs: Vec<_> = info
            .fields
            .iter()
            .map(|f| {
                let ph = i18n::t(f.label).to_string();
                cx.new(|cx| InputState::new(window, cx).masked(f.secret).placeholder(ph))
            })
            .collect();
        let base = match &self.esign {
            Some(Esign::Providers(d)) => d.base_url.clone(),
            _ => return,
        };
        base.update(cx, |i, cx| i.set_value(url, window, cx));
        if let Some(Esign::Providers(d)) = &mut self.esign {
            d.inputs = inputs;
            d.status = None;
        }
        cx.notify();
    }

    fn esign_select(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Esign::Providers(d)) = &mut self.esign {
            d.sel = i;
        }
        self.esign_load_fields(window, cx);
    }

    /// The draft settings of the selected provider (made on first change).
    fn esign_draft(d: &mut ProvidersDlg) -> &mut EsignProviderSettings {
        let id = PROVIDERS[d.sel].id;
        if let Some(i) = d.draft.iter().position(|s| s.id == id) {
            return &mut d.draft[i];
        }
        d.draft.push(EsignProviderSettings {
            id: id.to_string(),
            ..Default::default()
        });
        d.draft.last_mut().expect("just pushed")
    }

    /// Typed credentials (only fields the user filled in).
    fn esign_typed(d: &ProvidersDlg, cx: &App) -> Credentials {
        let info = PROVIDERS[d.sel];
        let mut c = Credentials::default();
        for (f, e) in info.fields.iter().zip(&d.inputs) {
            let v = e.read(cx).value().trim().to_string();
            if !v.is_empty() {
                c.set(f.key, &v);
            }
        }
        c
    }

    /// Save: settings now, credentials to the keychain in the background
    /// (typed fields replace saved ones; empty fields keep them).
    fn esign_save(&mut self, test: bool, cx: &mut Context<Self>) {
        let Some(Esign::Providers(d)) = &mut self.esign else {
            return;
        };
        let url = d.base_url.read(cx).value().trim().to_string();
        Self::esign_draft(d).base_url = url;
        let typed = Self::esign_typed(d, cx);
        let info = PROVIDERS[d.sel];
        let draft = d.draft.clone();
        d.working = true;
        d.status = None;
        let task = cx.background_spawn(async move {
            let mut s = Settings::load();
            s.pdf.esign_providers = draft.clone();
            s.save().map_err(|e| e.to_string())?;
            let mut creds = kantor_esign::creds::load(info.id);
            for (k, v) in typed.0 {
                creds.set(&k, &v);
            }
            let missing: Vec<&str> = creds.missing(info.fields).iter().map(|f| f.label).collect();
            let where_ = if typed_any(&creds) {
                Some(kantor_esign::creds::save(info.id, &creds)?)
            } else {
                None
            };
            if !test {
                return Ok::<_, String>((where_, missing, None));
            }
            let account = Account::load(info.id, &draft).map_err(|e| e.to_string())?;
            let r = if info.offers(Kind::EMeterai) {
                providers::emeterai(account).and_then(|p| p.test())
            } else {
                providers::signer(account).and_then(|p| p.test())
            };
            Ok((where_, missing, Some(r)))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(Esign::Providers(d)) = &mut this.esign else {
                    return;
                };
                d.working = false;
                d.status = Some(match result {
                    Err(e) => (false, e),
                    Ok((loc, missing, test)) => {
                        if loc.is_some() {
                            d.saved.insert(info.id, true);
                        }
                        let mut text = match loc {
                            Some(kantor_ai::keys::KeyLocation::File) => i18n::t(
                                "Saved. No system keychain was found, so the credentials are in a file only you can read.",
                            )
                            .to_string(),
                            _ => i18n::t("Saved.").to_string(),
                        };
                        if !missing.is_empty() {
                            let names: Vec<String> =
                                missing.iter().map(|m| t_dyn(m).to_string()).collect();
                            text = tf("Saved. Still missing: {}.", &[&names.join(", ")]);
                        }
                        match test {
                            None => (missing.is_empty(), text),
                            Some(Ok(())) => (true, i18n::t("Connected: the provider accepted the credentials.").into()),
                            Some(Err(e)) => (false, tf("Test failed: {}", &[&err_text(&e)])),
                        }
                    }
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn esign_remove(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Esign::Providers(d)) = &mut self.esign else {
            return;
        };
        let id = PROVIDERS[d.sel].id;
        d.draft.retain(|s| s.id != id);
        d.saved.insert(id, false);
        let draft = d.draft.clone();
        cx.background_spawn(async move {
            kantor_esign::creds::delete(id);
            let mut s = Settings::load();
            s.pdf.esign_providers = draft;
            let _ = s.save();
        })
        .detach();
        self.esign_load_fields(window, cx);
        if let Some(Esign::Providers(d)) = &mut self.esign {
            d.status = Some((true, i18n::t("Removed.").into()));
        }
        cx.notify();
    }

    fn esign_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.esign = None;
        if matches!(self.markup.tool, Tool::MeteraiBox | Tool::CertBox) {
            self.markup.tool = Tool::Select;
        }
        self.focus(window, cx);
        cx.notify();
    }

    // ---- Add e-Meterai ---------------------------------------------------

    /// Can the saved document go to a provider? (Unsaved changes and
    /// password-protected files can't.)
    fn esign_doc_ok(&mut self, cx: &mut Context<Self>) -> Option<(Arc<Vec<u8>>, usize)> {
        let doc = self.doc()?;
        if !doc.model.is_base_as_is() || doc.model.is_dirty() {
            self.status = i18n::t("Save your changes first, then use the saved document.").into();
            cx.notify();
            return None;
        }
        if !doc.model.base().password.is_empty() {
            self.status = i18n::t(
                "Password-protected PDFs can't be sent to a provider. Remove the password first.",
            )
            .into();
            cx.notify();
            return None;
        }
        let bytes = doc.model.base().bytes.clone();
        let signed = kantor_esign::check::signature_count(&bytes);
        Some((bytes, signed))
    }

    /// Add e-Meterai…
    pub fn add_emeterai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = Self::esign_configured(Kind::EMeterai);
        if list.is_empty() {
            self.status = i18n::t(SET_UP).into();
            self.open_esign_providers(window, cx);
            return;
        }
        let Some((_, signed)) = self.esign_doc_ok(cx) else {
            return;
        };
        let s = Settings::load().pdf;
        let dlg = StampDlg {
            providers: list,
            provider: 0,
            doc_types: Vec::new(),
            doc_type: None,
            loading: true,
            doc_number: input("", tk("Document number (optional)"), false, window, cx),
            reason: input("", tk("Reason (optional)"), false, window, cx),
            location: input(
                &s.signing_location,
                tk("Location (optional)"),
                false,
                window,
                cx,
            ),
            signed,
            accept_break: false,
            drawn: None,
            hidden: false,
            working: false,
            error: None,
        };
        self.esign = Some(Esign::Stamp(Box::new(dlg)));
        self.esign_load_doc_types(cx);
        cx.notify();
    }

    fn esign_load_doc_types(&mut self, cx: &mut Context<Self>) {
        let Some(Esign::Stamp(d)) = &mut self.esign else {
            return;
        };
        let info = d.providers[d.provider];
        d.loading = true;
        d.doc_types.clear();
        d.doc_type = None;
        let task = cx.background_spawn(async move {
            let s = Settings::load().pdf;
            let last = s
                .esign_providers
                .iter()
                .find(|p| p.id == info.id)
                .map(|p| p.doc_type.clone())
                .unwrap_or_default();
            let account = Account::load(info.id, &s.esign_providers)?;
            let p = providers::emeterai(account)?;
            Ok::<_, Error>((p.doc_types()?, last))
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(Esign::Stamp(d)) = &mut this.esign
                    && d.providers[d.provider].id == info.id
                {
                    d.loading = false;
                    match r {
                        Ok((types, last)) => {
                            d.doc_type = types.iter().position(|t| t.code == last);
                            d.doc_types = types;
                        }
                        Err(e) => d.error = Some(err_text(&e)),
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The user placed the meterai: back to the dialog.
    pub(crate) fn meterai_box_drawn(
        &mut self,
        place: usize,
        rect: [f32; 4],
        cx: &mut Context<Self>,
    ) {
        self.markup.tool = Tool::Select;
        if let Some(Esign::Stamp(d)) = &mut self.esign {
            d.hidden = false;
            d.drawn = Some((place, rect));
            d.error = None;
        }
        cx.notify();
    }

    fn esign_place_meterai(&mut self, cx: &mut Context<Self>) {
        if let Some(Esign::Stamp(d)) = &mut self.esign {
            d.hidden = true;
            d.error = None;
        }
        if self.markup.mode == crate::Mode::View {
            self.markup.mode = crate::Mode::FillSign;
        }
        self.markup.tool = Tool::MeteraiBox;
        self.status = i18n::t("Click where the e-meterai goes, or drag a square.").into();
        cx.notify();
    }

    fn esign_stamp_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((bytes, _)) = self.esign_doc_ok(cx) else {
            return;
        };
        let dir = self.dir();
        let name = self.name.clone();
        let Some(Esign::Stamp(d)) = &mut self.esign else {
            return;
        };
        let info = d.providers[d.provider];
        let Some(dt) = d.doc_type.and_then(|i| d.doc_types.get(i)).cloned() else {
            d.error = Some(i18n::t("Choose the document type.").into());
            cx.notify();
            return;
        };
        let Some((page, rect)) = d.drawn else {
            d.error = Some(i18n::t("Place the e-meterai on a page first.").into());
            cx.notify();
            return;
        };
        if d.signed > 0 && info.rewrites_file && !d.accept_break {
            d.error = Some(i18n::t(
                "This provider rebuilds the file, which breaks the signatures already in it. Tick the box to go ahead anyway.",
            ).into());
            cx.notify();
            return;
        }
        let mut req = StampRequest::new(&name, page as u32, rect, &dt.code);
        req.doc_number = d.doc_number.read(cx).value().trim().to_string();
        req.reason = d.reason.read(cx).value().trim().to_string();
        req.location = d.location.read(cx).value().trim().to_string();
        let allow_break = d.accept_break;
        let code = dt.code.clone();
        cx.background_spawn(async move {
            let mut s = Settings::load();
            if let Some(p) = s.pdf.esign_providers.iter_mut().find(|p| p.id == info.id) {
                p.doc_type = code;
                let _ = s.save();
            }
        })
        .detach();
        let target = cx.prompt_for_new_path(&dir, Some(&format!("{}-emeterai.pdf", stem(&name))));
        d.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = target.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(Esign::Stamp(d)) = &mut this.esign {
                    d.working = true;
                }
                this.busy = Some(tf("Sending to {}…", &[&info.name]).into());
                cx.notify();
            });
            let out = path.clone();
            let result = cx
                .background_spawn(async move {
                    let s = Settings::load().pdf;
                    let account = Account::load(info.id, &s.esign_providers)?;
                    let p = providers::emeterai(account)?;
                    let (stamped, kept) =
                        emeterai::stamp_checked(p.as_ref(), &bytes, &req, allow_break)?;
                    kantor_pdf::write_atomic(&out, &stamped.pdf)
                        .map_err(|e| Error::Pdf(e.to_string()))?;
                    Ok::<_, Error>((stamped.serial, kept))
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok((serial, kept)) => {
                        this.esign = None;
                        let mut msg = match serial {
                            Some(sn) => tf(
                                "e-Meterai {} added and saved as {}",
                                &[&sn, &path.display()],
                            ),
                            None => tf("e-Meterai added and saved as {}", &[&path.display()]),
                        };
                        if let Kept::Broken(f) = kept {
                            msg = format!(
                                "{msg}. {}",
                                tf("These signatures are no longer valid: {}", &[&f.join(", ")])
                            );
                        }
                        this.status = msg.into();
                        cx.emit(PdfEvent::Open(path));
                        this.focus(window, cx);
                    }
                    Err(e) => {
                        if let Some(Esign::Stamp(d)) = &mut this.esign {
                            d.working = false;
                            d.hidden = false;
                            d.error = Some(err_text(&e));
                        }
                        this.status = i18n::t("No e-meterai added").into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ---- Sign with Provider ----------------------------------------------

    /// Sign with Provider…
    pub fn sign_with_provider(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let list = Self::esign_configured(Kind::Signing);
        if list.is_empty() {
            self.status = i18n::t(SET_UP).into();
            self.open_esign_providers(window, cx);
            return;
        }
        let Some((bytes, signed)) = self.esign_doc_ok(cx) else {
            return;
        };
        let fields: Vec<String> = kantor_pdf::Pdf::from_bytes(&bytes, None)
            .map(|p| {
                kantor_pdf::sign::signature_fields(&p)
                    .into_iter()
                    .filter(|f| !f.signed)
                    .map(|f| f.name)
                    .collect()
            })
            .unwrap_or_default();
        let s = Settings::load().pdf;
        let dlg = SignDlg {
            providers: list,
            provider: 0,
            signer: input(
                "",
                tk("Your account at the provider (e-mail, phone or user ID)"),
                false,
                window,
                cx,
            ),
            otp: input("", tk("One-time password (OTP)"), false, window, cx),
            otp_sent: None,
            show: fields
                .first()
                .map(|f| Show::Field(f.clone()))
                .unwrap_or(Show::Invisible),
            fields,
            reason: input(
                &s.signing_reason,
                tk("Reason (optional)"),
                false,
                window,
                cx,
            ),
            location: input(
                &s.signing_location,
                tk("Location (optional)"),
                false,
                window,
                cx,
            ),
            signed,
            accept_break: false,
            drawn: None,
            hidden: false,
            working: false,
            error: None,
        };
        self.esign = Some(Esign::Sign(Box::new(dlg)));
        cx.notify();
    }

    /// The signature box was drawn (Tool::CertBox while this dialog is open).
    pub(crate) fn provider_box_drawn(
        &mut self,
        place: usize,
        rect: [f32; 4],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.markup.tool = Tool::Select;
        if let Some(Esign::Sign(d)) = &mut self.esign {
            d.hidden = false;
            d.drawn = Some((place, rect));
        }
        self.esign_sign_ok(window, cx);
    }

    fn esign_send_otp(&mut self, cx: &mut Context<Self>) {
        let Some(Esign::Sign(d)) = &mut self.esign else {
            return;
        };
        let info = d.providers[d.provider];
        let who = Signer {
            id: d.signer.read(cx).value().trim().to_string(),
            otp: None,
        };
        if who.id.is_empty() {
            d.error = Some(i18n::t("Type your account at the provider.").into());
            cx.notify();
            return;
        }
        d.working = true;
        d.error = None;
        let task = cx.background_spawn(async move {
            let s = Settings::load().pdf;
            let p = providers::signer(Account::load(info.id, &s.esign_providers)?)?;
            p.request_otp(&who)
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(Esign::Sign(d)) = &mut this.esign {
                    d.working = false;
                    match r {
                        Ok(Some(to)) => d.otp_sent = Some(to),
                        Ok(None) => {
                            d.otp_sent = Some(i18n::t("No code is needed for this account.").into())
                        }
                        Err(e) => d.error = Some(err_text(&e)),
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn esign_sign_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((bytes, _)) = self.esign_doc_ok(cx) else {
            return;
        };
        let dir = self.dir();
        let name = self.name.clone();
        let Some(Esign::Sign(d)) = &mut self.esign else {
            return;
        };
        let info = d.providers[d.provider];
        let who = Signer {
            id: d.signer.read(cx).value().trim().to_string(),
            otp: Some(d.otp.read(cx).value().trim().to_string()).filter(|s| !s.is_empty()),
        };
        if who.id.is_empty() {
            d.error = Some(i18n::t("Type your account at the provider.").into());
            cx.notify();
            return;
        }
        if d.signed > 0 && !info.hash_signing && info.rewrites_file && !d.accept_break {
            d.error = Some(i18n::t(
                "This provider rebuilds the file, which breaks the signatures already in it. Tick the box to go ahead anyway.",
            ).into());
            cx.notify();
            return;
        }
        if d.show == Show::Draw && d.drawn.is_none() {
            d.hidden = true;
            d.error = None;
            if self.markup.mode == crate::Mode::View {
                self.markup.mode = crate::Mode::FillSign;
            }
            self.markup.tool = Tool::CertBox;
            self.status = i18n::t("Drag a box on the page where the signature goes.").into();
            cx.notify();
            return;
        }
        let place = match (&d.show, d.drawn) {
            (Show::Draw, Some((page, r))) => Place::New {
                page: page as u32,
                rect: r,
            },
            (Show::Field(f), _) => Place::Field(f.clone()),
            _ => Place::Invisible,
        };
        let mut req = SignRequest::new(&name, place);
        req.reason = d.reason.read(cx).value().trim().to_string();
        req.location = d.location.read(cx).value().trim().to_string();
        let allow_break = d.accept_break;
        let target = cx.prompt_for_new_path(&dir, Some(&format!("{}-signed.pdf", stem(&name))));
        d.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = target.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(Esign::Sign(d)) = &mut this.esign {
                    d.working = true;
                }
                this.busy = Some(tf("Signing with {}…", &[&info.name]).into());
                cx.notify();
            });
            let out = path.clone();
            let result = cx
                .background_spawn(async move {
                    let s = Settings::load().pdf;
                    let p = providers::signer(Account::load(info.id, &s.esign_providers)?)?;
                    let (signed, kept) =
                        kantor_esign::signer::sign(p.as_ref(), &bytes, &req, &who, allow_break)?;
                    kantor_pdf::write_atomic(&out, &signed)
                        .map_err(|e| Error::Pdf(e.to_string()))?;
                    Ok::<_, Error>(kept)
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(kept) => {
                        this.esign = None;
                        let mut msg = tf("Signed and saved as {}", &[&path.display()]);
                        if let Kept::Broken(f) = kept {
                            msg = format!(
                                "{msg}. {}",
                                tf("These signatures are no longer valid: {}", &[&f.join(", ")])
                            );
                        }
                        this.status = msg.into();
                        cx.emit(PdfEvent::Open(path));
                        this.focus(window, cx);
                    }
                    Err(e) => {
                        if let Some(Esign::Sign(d)) = &mut this.esign {
                            d.working = false;
                            d.hidden = false;
                            d.drawn = None;
                            d.error = Some(err_text(&e));
                        }
                        this.status = i18n::t("Not signed").into();
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    // ---- Rendering ---------------------------------------------------------

    pub(crate) fn render_esign(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let card = match self.esign.as_ref()? {
            Esign::Providers(d) => self.render_providers(d, cx),
            Esign::Stamp(d) if !d.hidden => self.render_stamp(d, cx),
            Esign::Sign(d) if !d.hidden => self.render_provider_sign(d, cx),
            _ => return None,
        };
        let esc = cx.entity().downgrade();
        let card = card.on_key_down(move |e, window, cx| {
            if e.keystroke.key == "escape" {
                cx.stop_propagation();
                let _ = esc.update(cx, |this, cx| this.esign_close(window, cx));
            }
        });
        Some(
            kantor_ui::overlay::backdrop()
                .child(
                    h_flex()
                        .absolute()
                        .top(CONTROL_LG + SPACE_4)
                        .left_0()
                        .right_0()
                        .justify_center()
                        .child(card),
                )
                .into_any_element(),
        )
    }

    fn render_providers(&self, d: &ProvidersDlg, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let info = PROVIDERS[d.sel];
        let cur = d
            .draft
            .iter()
            .find(|s| s.id == info.id)
            .cloned()
            .unwrap_or_default();
        let label = |s: &'static str| {
            div()
                .text_size(FONT_SMALL)
                .font_weight(FontWeight::SEMIBOLD)
                .child(i18n::t(s))
        };
        let kinds = |i: &Info| {
            let mut k = Vec::new();
            if i.offers(Kind::EMeterai) {
                k.push(i18n::t("e-Meterai").to_string());
            }
            if i.offers(Kind::Signing) {
                k.push(i18n::t("Certified signatures").to_string());
            }
            k.join(" · ")
        };
        let mut list = v_flex()
            .id("pdf-esign-list")
            .w(px(220.))
            .flex_none()
            .max_h(px(420.))
            .overflow_y_scroll()
            .rounded(RADIUS_MD)
            .border_1()
            .border_color(t.border_strong);
        for (i, p) in PROVIDERS.iter().enumerate() {
            let on = d.sel == i;
            let set_up = d.draft.iter().any(|s| s.id == p.id && s.allow_network);
            let state = match (p.support, set_up) {
                (Support::Stub(_), _) => i18n::t("API not public yet"),
                (_, true) => i18n::t("On"),
                _ => i18n::t("Not set up"),
            };
            let v = view.clone();
            list = list.child(
                v_flex()
                    .id(("pdf-esign-p", i))
                    .px(SPACE_3)
                    .py(SPACE_2)
                    .cursor_pointer()
                    .when(on, |r| r.bg(t.accent_soft))
                    .hover(|r| r.bg(t.hover))
                    .on_click(move |_, window, cx| {
                        let _ = v.update(cx, |this, cx| this.esign_select(i, window, cx));
                    })
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(p.name))
                    .child(
                        div()
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_muted)
                            .child(format!("{} · {state}", kinds(p))),
                    ),
            );
        }
        let saved = d.saved.get(info.id).copied().unwrap_or(false);
        let mut right = v_flex()
            .flex_1()
            .min_w_0()
            .gap(SPACE_3)
            .child(div().font_weight(FontWeight::SEMIBOLD).child(info.name))
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(tf("Documentation: {}", &[&info.docs])),
            );
        match info.support {
            Support::Stub(why) => {
                right = right.child(crate::dialog::note(
                    tf("Kantor can't use this provider yet: {}", &[&t_dyn(why)]),
                    false,
                    cx,
                ));
            }
            Support::Implemented => {
                right = right.child(label(tk("Your account")));
                if saved {
                    right = right.child(crate::dialog::note(
                        i18n::t("Credentials are saved in the system keychain; type a field to replace it."),
                        false,
                        cx,
                    ));
                }
                for (f, e) in info.fields.iter().zip(&d.inputs) {
                    right = right.child(crate::dialog::row(i18n::t(f.label), Input::new(e), cx));
                }
                let (va, vs) = (view.clone(), view.clone());
                right = right
                    .child(
                        Checkbox::new("pdf-esign-allow")
                            .checked(cur.allow_network)
                            .label(tf("Allow Kantor to send documents to {}", &[&info.name]))
                            .on_click(move |c, _, cx| {
                                let c = *c;
                                let _ = va.update(cx, |this, cx| {
                                    if let Some(Esign::Providers(d)) = &mut this.esign {
                                        Self::esign_draft(d).allow_network = c;
                                    }
                                    cx.notify();
                                });
                            }),
                    )
                    .when(!info.sandbox_url.is_empty(), |r| {
                        r.child(
                            Checkbox::new("pdf-esign-sandbox")
                                .checked(cur.sandbox)
                                .label(i18n::t("Use the provider's sandbox (test) environment"))
                                .on_click(move |c, _, cx| {
                                    let c = *c;
                                    let _ = vs.update(cx, |this, cx| {
                                        if let Some(Esign::Providers(d)) = &mut this.esign {
                                            Self::esign_draft(d).sandbox = c;
                                        }
                                        cx.notify();
                                    });
                                }),
                        )
                    })
                    .child(crate::dialog::row(
                        i18n::t("Server address"),
                        Input::new(&d.base_url),
                        cx,
                    ))
                    .child(crate::dialog::note(
                        i18n::t("Documents go straight from this computer to the provider, only when you stamp or sign. Credentials stay in the system keychain."),
                        false,
                        cx,
                    ));
            }
        }
        if let Some((ok, s)) = &d.status {
            right = right.child(crate::dialog::note(s.clone(), !ok, cx));
        }
        let (vc, vr, vt, vs) = (view.clone(), view.clone(), view.clone(), view.clone());
        let implemented = info.implemented();
        crate::dialog::card(i18n::t("e-Sign Providers"), 720.0, cx)
            .child(div().text_size(FONT_SMALL).text_color(t.fg_muted).child(i18n::t(
                "e-Meterai and certified electronic signatures come from licensed providers. Add the account you have with one of them.",
            )))
            .child(h_flex().items_start().gap(SPACE_4).child(list).child(right))
            .child(
                h_flex()
                    .gap(SPACE_3)
                    .child(
                        Button::new("pdf-esign-remove")
                            .ghost()
                            .small()
                            .disabled(!implemented || d.working)
                            .label(i18n::t("Remove"))
                            .on_click(move |_, window, cx| {
                                let _ = vr.update(cx, |this, cx| this.esign_remove(window, cx));
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("pdf-esign-close")
                            .ghost()
                            .small()
                            .label(i18n::t("Close"))
                            .on_click(move |_, window, cx| {
                                let _ = vc.update(cx, |this, cx| this.esign_close(window, cx));
                            }),
                    )
                    .child(
                        Button::new("pdf-esign-test")
                            .outline()
                            .small()
                            .disabled(!implemented || d.working || !cur.allow_network)
                            .label(i18n::t("Save and Test"))
                            .on_click(move |_, _, cx| {
                                let _ = vt.update(cx, |this, cx| this.esign_save(true, cx));
                            }),
                    )
                    .child(
                        Button::new("pdf-esign-save")
                            .primary()
                            .small()
                            .disabled(!implemented || d.working)
                            .label(i18n::t("Save"))
                            .on_click(move |_, _, cx| {
                                let _ = vs.update(cx, |this, cx| this.esign_save(false, cx));
                            }),
                    ),
            )
    }

    fn provider_picker(
        &self,
        id: &'static str,
        list: &[&'static Info],
        current: usize,
        on_pick: fn(&mut PdfView, usize, &mut Context<PdfView>),
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let view = cx.entity().downgrade();
        let names: Vec<&'static str> = list.iter().map(|p| p.name).collect();
        Button::new(id)
            .small()
            .outline()
            .label(names.get(current).copied().unwrap_or(""))
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (i, n) in names.iter().enumerate() {
                    let v = view.clone();
                    menu = menu.item(PopupMenuItem::new(*n).checked(i == current).on_click(
                        move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| on_pick(this, i, cx));
                        },
                    ));
                }
                menu
            })
    }

    fn render_stamp(&self, d: &StampDlg, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let info = d.providers[d.provider];
        let picker = self.provider_picker(
            "pdf-em-provider",
            &d.providers,
            d.provider,
            |this, i, cx| {
                if let Some(Esign::Stamp(d)) = &mut this.esign {
                    d.provider = i;
                    d.error = None;
                }
                this.esign_load_doc_types(cx);
                cx.notify();
            },
            cx,
        );
        let types: Vec<(String, String)> = d
            .doc_types
            .iter()
            .map(|t| (t.code.clone(), t.name.clone()))
            .collect();
        let current = d.doc_type;
        let vt = view.clone();
        let type_label = match current.and_then(|i| types.get(i)) {
            Some((_, n)) => n.clone(),
            None if d.loading => i18n::t("Loading…").into(),
            None => i18n::t("Choose…").into(),
        };
        let type_btn = Button::new("pdf-em-type")
            .small()
            .outline()
            .disabled(types.is_empty())
            .label(type_label)
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (i, (_, n)) in types.iter().enumerate() {
                    let v = vt.clone();
                    menu = menu.item(
                        PopupMenuItem::new(n.clone())
                            .checked(current == Some(i))
                            .on_click(move |_, _, cx| {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(Esign::Stamp(d)) = &mut this.esign {
                                        d.doc_type = Some(i);
                                        d.error = None;
                                    }
                                    cx.notify();
                                });
                            }),
                    );
                }
                menu
            });
        let placed = match d.drawn {
            Some((p, r)) => tf(
                "On page {}, {} × {} mm",
                &[
                    &(p + 1),
                    &format!("{:.0}", (r[2] - r[0]) / 72.0 * 25.4),
                    &format!("{:.0}", (r[3] - r[1]) / 72.0 * 25.4),
                ],
            ),
            None => i18n::t("Not placed yet").into(),
        };
        let (vp, vb, vc, vs) = (view.clone(), view.clone(), view.clone(), view.clone());
        crate::dialog::card(i18n::t("Add e-Meterai"), 560.0, cx)
            .overflow_y_scroll()
            .child(div().text_size(FONT_SMALL).text_color(t.fg_muted).child(tf(
                "An e-meterai of Rp {} from your provider account. Kantor sends the document to the provider, which stamps it; the stamped document is saved as a new file.",
                &[&"10.000"],
            )))
            .child(crate::dialog::row(i18n::t("Provider"), picker, cx))
            .child(crate::dialog::row(i18n::t("Document type"), type_btn, cx))
            .child(crate::dialog::row(i18n::t("Number"), Input::new(&d.doc_number), cx))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(div().flex_1().child(Input::new(&d.reason)))
                    .child(div().flex_1().child(Input::new(&d.location))),
            )
            .child(crate::dialog::row(
                i18n::t("Position"),
                h_flex()
                    .gap(SPACE_3)
                    .child(div().text_size(FONT_SMALL).child(placed))
                    .child(
                        Button::new("pdf-em-place")
                            .small()
                            .outline()
                            .label(if d.drawn.is_some() {
                                i18n::t("Place Again")
                            } else {
                                i18n::t("Place on Page")
                            })
                            .on_click(move |_, _, cx| {
                                let _ = vp.update(cx, |this, cx| this.esign_place_meterai(cx));
                            }),
                    ),
                cx,
            ))
            .when(d.signed > 0, |c| {
                if info.rewrites_file {
                    c.child(crate::dialog::note(
                        i18n::t("This document is signed, and this provider rebuilds files: the signatures already in it will no longer be valid."),
                        true,
                        cx,
                    ))
                    .child(
                        Checkbox::new("pdf-em-break")
                            .checked(d.accept_break)
                            .label(i18n::t("Stamp anyway"))
                            .on_click(move |c, _, cx| {
                                let c = *c;
                                let _ = vb.update(cx, |this, cx| {
                                    if let Some(Esign::Stamp(d)) = &mut this.esign {
                                        d.accept_break = c;
                                    }
                                    cx.notify();
                                });
                            }),
                    )
                } else {
                    c.child(crate::dialog::note(
                        i18n::t("This document is signed. Kantor checks the stamped file and won't save it if the provider broke the signatures."),
                        false,
                        cx,
                    ))
                }
            })
            .children(d.error.as_ref().map(|e| crate::dialog::note(e.clone(), true, cx)))
            .child(
                h_flex()
                    .justify_end()
                    .gap(SPACE_3)
                    .child(
                        Button::new("pdf-em-cancel")
                            .ghost()
                            .small()
                            .label(i18n::t("Cancel"))
                            .on_click(move |_, window, cx| {
                                let _ = vc.update(cx, |this, cx| this.esign_close(window, cx));
                            }),
                    )
                    .child(
                        Button::new("pdf-em-ok")
                            .primary()
                            .small()
                            .disabled(d.working || d.drawn.is_none() || d.doc_type.is_none())
                            .label(if d.working {
                                i18n::t("Stamping…")
                            } else {
                                i18n::t("Stamp…")
                            })
                            .on_click(move |_, window, cx| {
                                let _ = vs.update(cx, |this, cx| this.esign_stamp_ok(window, cx));
                            }),
                    ),
            )
    }

    fn render_provider_sign(&self, d: &SignDlg, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let info = d.providers[d.provider];
        let picker = self.provider_picker(
            "pdf-ps-provider",
            &d.providers,
            d.provider,
            |this, i, cx| {
                if let Some(Esign::Sign(d)) = &mut this.esign {
                    d.provider = i;
                    d.otp_sent = None;
                    d.error = None;
                }
                cx.notify();
            },
            cx,
        );
        let show_btn = |id: &'static str, text: SharedString, which: Show| {
            let v = view.clone();
            let on = d.show == which;
            Button::new(id)
                .small()
                .label(text)
                .when(on, |b| b.primary())
                .when(!on, |b| b.ghost())
                .on_click(move |_, _, cx| {
                    let which = which.clone();
                    let _ = v.update(cx, |this, cx| {
                        if let Some(Esign::Sign(d)) = &mut this.esign {
                            d.show = which;
                        }
                        cx.notify();
                    });
                })
        };
        let mut shows = h_flex()
            .gap(SPACE_2)
            .flex_wrap()
            .child(show_btn(
                "pdf-ps-draw",
                i18n::t("Draw a Box").into(),
                Show::Draw,
            ))
            .child(show_btn(
                "pdf-ps-inv",
                i18n::t("Invisible").into(),
                Show::Invisible,
            ));
        for (i, f) in d.fields.iter().enumerate().take(6) {
            let v = view.clone();
            let on = d.show == Show::Field(f.clone());
            let name = f.clone();
            shows = shows.child(
                Button::new(("pdf-ps-field", i))
                    .small()
                    .label(tf("Field “{}”", &[f]))
                    .when(on, |b| b.primary())
                    .when(!on, |b| b.ghost())
                    .on_click(move |_, _, cx| {
                        let name = name.clone();
                        let _ = v.update(cx, |this, cx| {
                            if let Some(Esign::Sign(d)) = &mut this.esign {
                                d.show = Show::Field(name);
                            }
                            cx.notify();
                        });
                    }),
            );
        }
        let (vo, vb, vc, vs) = (view.clone(), view.clone(), view.clone(), view.clone());
        let how = if info.hash_signing {
            i18n::t(
                "Kantor sends only the document's fingerprint (hash); the provider returns the signature.",
            )
        } else {
            i18n::t("Kantor sends the document to the provider, which signs it.")
        };
        crate::dialog::card(i18n::t("Sign with Provider"), 560.0, cx)
            .overflow_y_scroll()
            .child(div().text_size(FONT_SMALL).text_color(t.fg_muted).child(tf(
                "A certified electronic signature from your account at a licensed provider (PSrE). {} The signed document is saved as a new file.",
                &[&how],
            )))
            .child(crate::dialog::row(i18n::t("Provider"), picker, cx))
            .child(Input::new(&d.signer))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(div().flex_1().child(Input::new(&d.otp)))
                    .child(
                        Button::new("pdf-ps-otp")
                            .small()
                            .outline()
                            .disabled(d.working)
                            .label(i18n::t("Send Code"))
                            .on_click(move |_, _, cx| {
                                let _ = vo.update(cx, |this, cx| this.esign_send_otp(cx));
                            }),
                    ),
            )
            .children(d.otp_sent.as_ref().map(|to| {
                crate::dialog::note(tf("Code sent: {}", &[to]), false, cx)
            }))
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(i18n::t("Appearance")),
            )
            .child(shows)
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(div().flex_1().child(Input::new(&d.reason)))
                    .child(div().flex_1().child(Input::new(&d.location))),
            )
            .when(d.signed > 0 && info.rewrites_file && !info.hash_signing, |c| {
                c.child(crate::dialog::note(
                    i18n::t("This document is signed, and this provider rebuilds files: the signatures already in it will no longer be valid."),
                    true,
                    cx,
                ))
                .child(
                    Checkbox::new("pdf-ps-break")
                        .checked(d.accept_break)
                        .label(i18n::t("Sign anyway"))
                        .on_click(move |c, _, cx| {
                            let c = *c;
                            let _ = vb.update(cx, |this, cx| {
                                if let Some(Esign::Sign(d)) = &mut this.esign {
                                    d.accept_break = c;
                                }
                                cx.notify();
                            });
                        }),
                )
            })
            .children(d.error.as_ref().map(|e| crate::dialog::note(e.clone(), true, cx)))
            .child(
                h_flex()
                    .justify_end()
                    .gap(SPACE_3)
                    .child(
                        Button::new("pdf-ps-cancel")
                            .ghost()
                            .small()
                            .label(i18n::t("Cancel"))
                            .on_click(move |_, window, cx| {
                                let _ = vc.update(cx, |this, cx| this.esign_close(window, cx));
                            }),
                    )
                    .child(
                        Button::new("pdf-ps-ok")
                            .primary()
                            .small()
                            .disabled(d.working)
                            .label(if d.working {
                                i18n::t("Signing…")
                            } else {
                                i18n::t("Sign…")
                            })
                            .on_click(move |_, window, cx| {
                                let _ = vs.update(cx, |this, cx| this.esign_sign_ok(window, cx));
                            }),
                    ),
            )
    }
}

fn typed_any(c: &Credentials) -> bool {
    !c.is_empty()
}

/// The e-meterai box for a click at `pt` or a drag from `start` to `now`
/// (user space): the default size centred on a click, else a square as
/// large as the drag, kept within Peruri's sizes.
pub(crate) fn meterai_rect(start: [f32; 2], now: [f32; 2], small: bool) -> [f32; 4] {
    if small {
        return emeterai::box_at(start[0], start[1]);
    }
    let side = (now[0] - start[0])
        .abs()
        .max((now[1] - start[1]).abs())
        .clamp(emeterai::SIZE_MIN_PT, emeterai::SIZE_MAX_PT);
    let x0 = if now[0] >= start[0] {
        start[0]
    } else {
        start[0] - side
    };
    let y0 = if now[1] >= start[1] {
        start[1]
    } else {
        start[1] - side
    };
    [x0, y0, x0 + side, y0 + side]
}

// ---- Batch steps -----------------------------------------------------------

/// The Batch dialog's "Add e-Meterai" / "Sign with Provider" step.
pub(crate) struct EsignStepUi {
    /// Sign with Provider (else Add e-Meterai).
    pub sign: bool,
    providers: Vec<&'static Info>,
    provider: usize,
    /// e-meterai: the document type code; signing: the signer's account.
    text: Entity<InputState>,
    /// e-meterai on the last page (else the first).
    last_page: bool,
    /// The rest of a step loaded from a file (kept as it was).
    base: serde_json::Value,
}

fn esign_ui(d: &mut crate::dialog::Dialog, i: usize) -> Option<&mut EsignStepUi> {
    match d {
        crate::dialog::Dialog::Batch(b) => match b.steps.get_mut(i) {
            Some(crate::batch::StepUi::Esign(e)) => Some(e),
            _ => None,
        },
        _ => None,
    }
}

impl EsignStepUi {
    /// A new step with the first provider set up (`None`: none is).
    pub(crate) fn new(
        sign: bool,
        from: Option<(&str, &str, bool, serde_json::Value)>,
        window: &mut Window,
        cx: &mut Context<PdfView>,
    ) -> Option<EsignStepUi> {
        let kind = if sign { Kind::Signing } else { Kind::EMeterai };
        let providers = PdfView::esign_configured(kind);
        if providers.is_empty() {
            return None;
        }
        let saved = Settings::load().pdf.esign_providers;
        let (provider, text, last_page, base) = match from {
            Some((id, text, last, base)) => (
                providers.iter().position(|p| p.id == id)?,
                text.to_string(),
                last,
                base,
            ),
            None => {
                let doc_type = if sign {
                    String::new()
                } else {
                    saved
                        .iter()
                        .find(|s| s.id == providers[0].id)
                        .map(|s| s.doc_type.clone())
                        .unwrap_or_default()
                };
                (0, doc_type, true, serde_json::Value::Null)
            }
        };
        let ph = if sign {
            tk("Your account at the provider")
        } else {
            tk("Document type code (e.g. VD010)")
        };
        Some(EsignStepUi {
            sign,
            providers,
            provider,
            text: input(&text, ph, false, window, cx),
            last_page,
            base,
        })
    }

    /// From a step of a batch file, when this dialog can edit it.
    pub(crate) fn from_step(
        s: &kantor_pdf::batch::Step,
        window: &mut Window,
        cx: &mut Context<PdfView>,
    ) -> Option<EsignStepUi> {
        use kantor_esign::batch::{Appearance, EMETERAI, EMeteraiStep, ProviderSignStep};
        let kantor_pdf::batch::Step::Plugin { kind, options } = s else {
            return None;
        };
        if kind == EMETERAI {
            let st = EMeteraiStep::from_value(options).ok()?;
            let last = match st.page.trim() {
                "" | "last" => true,
                "first" | "1" => false,
                _ => return None,
            };
            let d = EMeteraiStep::default();
            if st.place != d.place || st.size != d.size {
                return None;
            }
            EsignStepUi::new(
                false,
                Some((&st.provider, &st.doc_type, last, options.clone())),
                window,
                cx,
            )
        } else {
            let st = ProviderSignStep::from_value(options).ok()?;
            if st.appearance != Appearance::Invisible {
                return None;
            }
            EsignStepUi::new(
                true,
                Some((&st.provider, &st.signer, true, options.clone())),
                window,
                cx,
            )
        }
    }

    pub(crate) fn to_step(&self, n: usize, cx: &App) -> Result<kantor_pdf::batch::Step, String> {
        use kantor_esign::batch::{EMeteraiStep, ProviderSignStep};
        let text = self.text.read(cx).value().trim().to_string();
        let id = self.providers[self.provider].id.to_string();
        if self.sign {
            if text.is_empty() {
                return Err(tf("Step {}: type your account at the provider", &[&n]));
            }
            let mut st = ProviderSignStep::from_value(&self.base).unwrap_or_default();
            st.provider = id;
            st.signer = text;
            Ok(st.to_step())
        } else {
            if text.is_empty() {
                return Err(tf("Step {}: type the document type", &[&n]));
            }
            let mut st = EMeteraiStep::from_value(&self.base).unwrap_or_default();
            st.provider = id;
            st.doc_type = text;
            st.page = if self.last_page { "last" } else { "first" }.into();
            Ok(st.to_step())
        }
    }
}

impl PdfView {
    pub(crate) fn render_esign_step(
        &self,
        i: usize,
        e: &EsignStepUi,
        cx: &mut Context<Self>,
    ) -> Div {
        let view = cx.entity().downgrade();
        let names: Vec<&'static str> = e.providers.iter().map(|p| p.name).collect();
        let current = e.provider;
        let weak = view.clone();
        let picker = Button::new(("pdf-batch-esign-p", i))
            .xsmall()
            .outline()
            .label(names.get(current).copied().unwrap_or(""))
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (k, n) in names.iter().enumerate() {
                    let v = weak.clone();
                    menu = menu.item(PopupMenuItem::new(*n).checked(k == current).on_click(
                        move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(d) = this.dialog.as_mut()
                                    && let Some(e) = esign_ui(d, i)
                                {
                                    e.provider = k;
                                }
                                cx.notify();
                            });
                        },
                    ));
                }
                menu
            });
        let mut body = v_flex()
            .gap(SPACE_2)
            .child(crate::dialog::row(i18n::t("Provider"), picker, cx))
            .child(crate::dialog::row(
                if e.sign {
                    i18n::t("Signer")
                } else {
                    i18n::t("Document type")
                },
                Input::new(&e.text).small(),
                cx,
            ));
        if !e.sign {
            let page_btn = |id: &'static str, label: SharedString, last: bool| {
                let v = view.clone();
                Button::new((id, i))
                    .xsmall()
                    .label(label)
                    .when_else(e.last_page == last, |b| b.primary(), |b| b.outline())
                    .on_click(move |_, _, cx| {
                        let _ = v.update(cx, |this, cx| {
                            if let Some(d) = this.dialog.as_mut()
                                && let Some(e) = esign_ui(d, i)
                            {
                                e.last_page = last;
                            }
                            cx.notify();
                        });
                    })
            };
            body = body
                .child(crate::dialog::row(
                    i18n::t("Page"),
                    h_flex()
                        .gap(SPACE_2)
                        .child(page_btn(
                            "pdf-batch-em-first",
                            i18n::t("First").into(),
                            false,
                        ))
                        .child(page_btn("pdf-batch-em-last", i18n::t("Last").into(), true)),
                    cx,
                ))
                .child(crate::dialog::note(
                    i18n::t(
                        "Bottom right of the page. Each file uses one e-meterai from your quota.",
                    ),
                    false,
                    cx,
                ));
        } else {
            body = body.child(crate::dialog::note(
                i18n::t("An invisible certified signature on every file. Providers that send a one-time password for each signature can't sign in a batch."),
                false,
                cx,
            ));
        }
        body
    }
}

/// The batch steps' plugins: providers are made (settings and keychain)
/// when a file first needs them, on the batch's worker thread.
pub(crate) fn add_batch_plugins(all: &mut kantor_pdf::batch::Plugins) {
    use kantor_esign::batch::{EMeteraiPlugin, ProviderSignPlugin};
    let saved = Settings::load().pdf.esign_providers;
    let ids = |k: Kind| -> Vec<String> {
        kantor_esign::configured(&saved, k)
            .iter()
            .map(|p| p.id.to_string())
            .collect()
    };
    all.add(Arc::new(EMeteraiPlugin::lazy(
        ids(Kind::EMeterai),
        Arc::new(|id: &str| {
            let s = Settings::load().pdf.esign_providers;
            providers::emeterai(Account::load(id, &s)?)
        }),
    )));
    all.add(Arc::new(ProviderSignPlugin::lazy(
        ids(Kind::Signing),
        Arc::new(|id: &str| {
            let s = Settings::load().pdf.esign_providers;
            providers::signer(Account::load(id, &s)?)
        }),
    )));
}

/// Text kept in kantor_esign (credential field labels, why a provider
/// can't be used, batch skip reasons) and shown with `t_dyn`: listed so the
/// catalog check sees them.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "read by the catalog check and the test below")
)]
pub(crate) const ESIGN_KEYS: &[&str] = &[
    tk("Client ID"),
    tk("Client secret"),
    tk("Token address (from VIDA; optional in the sandbox)"),
    tk("HMAC client ID"),
    tk("HMAC client secret"),
    tk("API key"),
    tk(
        "Peruri's e-Meterai API isn't public: it needs a contract with Peruri or a distributor.",
    ),
    tk(
        "Pajakku shares its e-Meterai API documentation and sandbox only after a non-disclosure agreement.",
    ),
    tk("Mitracomm's e-Meterai API documentation isn't public."),
    tk("Finnet's e-Meterai API documentation isn't public."),
    tk(
        "Privy's public documentation covers identity checks only; its signing and e-Meterai APIs aren't published.",
    ),
    tk("Tilaka's signing API, including hash signing, is documented only for partners."),
    tk("Peruri's PSrE signing API documentation isn't public."),
    tk(
        "VIDA's signing APIs are public but need the signer's e-KYC data (photo of the ID card and a selfie) with every request, and its hash signing isn't documented yet.",
    ),
    tk("Digisign's API documentation isn't public."),
    tk("Not stamped: password-protected PDFs can't be sent to the provider"),
    tk("Not stamped: the file doesn't have the step's page"),
    tk(
        "Not stamped: the file is signed and this provider rebuilds files, which would break the signatures",
    ),
    tk("Not stamped: the file is certified and allows no changes"),
    tk("Not signed: password-protected PDFs can't be sent to the provider"),
    tk("Not signed: the file is certified and allows no changes, not even another signature"),
    tk("Not signed: the file doesn't have the step's page"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn every_provider_text_is_translatable() {
        for p in PROVIDERS {
            for f in p.fields {
                assert!(
                    ESIGN_KEYS.contains(&f.label),
                    "{:?} missing from ESIGN_KEYS",
                    f.label
                );
            }
            if let Support::Stub(why) = p.support {
                assert!(ESIGN_KEYS.contains(&why), "{why:?} missing from ESIGN_KEYS");
            }
        }
        for w in kantor_esign::batch::why::ALL {
            assert!(ESIGN_KEYS.contains(&w), "{w:?} missing from ESIGN_KEYS");
        }
    }

    #[std::prelude::v1::test]
    fn meterai_box_is_square_and_within_limits() {
        let r = meterai_rect([100.0, 100.0], [100.0, 100.0], true);
        assert_eq!(r[2] - r[0], emeterai::SIZE_PT);
        let r = meterai_rect([100.0, 100.0], [400.0, 130.0], false);
        assert_eq!(
            (r[2] - r[0], r[3] - r[1]),
            (emeterai::SIZE_MAX_PT, emeterai::SIZE_MAX_PT)
        );
        let r = meterai_rect([100.0, 100.0], [90.0, 95.0], false);
        assert_eq!(r[2] - r[0], emeterai::SIZE_MIN_PT);
        assert_eq!(r[2], 100.0, "dragged left: ends at the start");
    }
}
