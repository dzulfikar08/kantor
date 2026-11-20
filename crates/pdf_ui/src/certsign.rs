//! Sign with Certificate: a digital signature (PAdES) with the user's own
//! certificate, as Acrobat's "Use a certificate" does. The work is
//! [`pdf_engine::signing`]; this is the dialog.
//!
//! Certificates come from .p12/.pfx files the user opens, from USB tokens
//! and smart cards (PKCS #11 modules in the usual places, or one the user
//! adds), and from the macOS keychain or Windows certificate store. The
//! password or PIN is typed into the dialog, used once and dropped: it is
//! never saved or logged. A time stamp (and long-term validation data)
//! goes online only when the user ticked it *and* "Let Signature Checks
//! Use the Internet" is on. The signed document is saved as a new file
//! (an incremental update of this one) and opened.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::settings::Settings;
use kantor_pdf::annot::Art;
use kantor_pdf::sign::{Look, Place};
use kantor_ui::i18n::{self, tf};
use kantor_ui::{metrics::*, tokens};
use pdf_engine::signatures::{self as sig, Cert};
use pdf_engine::signing::{self, Identity, Level, stores};

use crate::markup::{Placed, Tool};
use crate::{PdfEvent, PdfView};

/// Where the signature shows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Show {
    Invisible,
    /// In a box the user draws on a page.
    Draw,
    /// In this empty signature field.
    Field(String),
}

pub(crate) enum Source {
    File(PathBuf),
    Store(Arc<stores::Candidate>),
}

pub(crate) struct Entry {
    pub source: Source,
    /// Known once a file is unlocked (or right away for stores).
    pub cert: Option<Arc<Cert>>,
}

impl Entry {
    fn title(&self) -> String {
        match (&self.cert, &self.source) {
            (Some(c), _) => c.subject_name().display_name(),
            (None, Source::File(p)) => p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            (None, Source::Store(_)) => String::new(),
        }
    }

    fn where_text(&self) -> String {
        match &self.source {
            Source::File(p) => tf("File: {}", &[&p.display()]),
            Source::Store(c) => match c.source() {
                signing::Source::Token { token, .. } => tf("Token: {}", &[&token]),
                _ => i18n::t("System certificate store").into(),
            },
        }
    }

    fn needs_secret(&self) -> bool {
        match &self.source {
            Source::File(_) => true,
            Source::Store(c) => matches!(c.place, stores::Place::Token { .. }) && !c.pin_pad(),
        }
    }
}

pub(crate) struct CertSign {
    pub entries: Vec<Entry>,
    pub selected: Option<usize>,
    pub loading: bool,
    pub secret: Entity<InputState>,
    pub reason: Entity<InputState>,
    pub location: Entity<InputState>,
    pub tsa: Entity<InputState>,
    pub show: Show,
    /// Empty signature fields of the document.
    pub fields: Vec<String>,
    pub certify: Option<u8>,
    /// Index into the saved signatures (My Signatures) for the picture.
    pub picture: Option<usize>,
    pub pictures: Vec<Placed>,
    pub timestamp: bool,
    pub ltv: bool,
    pub error: Option<String>,
    pub working: bool,
    /// Hidden while the user draws the box.
    pub hidden: bool,
    /// The drawn box: page and rectangle (user space).
    pub drawn: Option<(usize, [f32; 4])>,
}

/// Turn a remembered signature into the appearance's picture.
fn art_of(p: &Placed) -> Option<Art> {
    match p {
        Placed::Image { bytes, .. } => Some(Art::Image(bytes.clone())),
        Placed::Typed { segs, .. } => Some(Art::Paths(segs.clone())),
        Placed::Ink { .. } => None,
    }
}

impl PdfView {
    /// Sign with Certificate…
    pub fn sign_with_certificate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.doc() else {
            return;
        };
        if !doc.model.is_base_as_is() || doc.model.is_dirty() {
            self.status = i18n::t("Save your changes first, then sign the saved document.").into();
            cx.notify();
            return;
        }
        let bytes = doc.model.base().bytes.clone();
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
        let input = |text: String,
                     placeholder: &'static str,
                     masked: bool,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            let ph = i18n::t(placeholder);
            let e = cx.new(|cx| InputState::new(window, cx).masked(masked).placeholder(ph));
            e.update(cx, |i, cx| i.set_value(text, window, cx));
            e
        };
        let secret = input(String::new(), "Password or PIN", true, window, cx);
        let reason = input(
            s.signing_reason.clone(),
            "Reason (optional)",
            false,
            window,
            cx,
        );
        let location = input(
            s.signing_location.clone(),
            "Location (optional)",
            false,
            window,
            cx,
        );
        let tsa = input(
            s.signing_tsa_url.clone(),
            "Time-stamp server address (https://…)",
            false,
            window,
            cx,
        );
        let mut entries: Vec<Entry> = s
            .certificate_files
            .iter()
            .map(PathBuf::from)
            .filter(|p| p.exists())
            .map(|p| Entry {
                source: Source::File(p),
                cert: None,
            })
            .collect();
        entries.dedup_by(
            |a, b| matches!((&a.source, &b.source), (Source::File(x), Source::File(y)) if x == y),
        );
        let pictures: Vec<Placed> = crate::sign::load_saved()
            .into_iter()
            .filter(|s| s.signature && art_of(&s.what).is_some())
            .map(|s| s.what)
            .collect();
        let show = if !fields.is_empty() {
            Show::Field(fields[0].clone())
        } else if s.signing_visible {
            Show::Draw
        } else {
            Show::Invisible
        };
        self.cert_sign = Some(CertSign {
            selected: (!entries.is_empty()).then_some(0),
            entries,
            loading: true,
            secret,
            reason,
            location,
            tsa,
            show,
            fields,
            certify: None,
            picture: None,
            pictures,
            timestamp: s.signing_timestamp,
            ltv: s.signing_ltv,
            error: None,
            working: false,
            hidden: false,
            drawn: None,
        });
        // Tokens and system stores can be slow: list them in the background.
        let modules: Vec<String> = s
            .pkcs11_modules
            .iter()
            .cloned()
            .chain(stores::common_modules())
            .collect();
        let task = cx.background_spawn(async move { list_stores(modules) });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = &mut this.cert_sign {
                    d.loading = false;
                    for c in found {
                        let cert = c.cert.clone();
                        d.entries.push(Entry {
                            source: Source::Store(Arc::new(c)),
                            cert: Some(cert),
                        });
                    }
                    if d.selected.is_none() && !d.entries.is_empty() {
                        d.selected = Some(0);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Open Certificate File…: add a .p12/.pfx to the list.
    fn cert_sign_open_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(i18n::t("Choose a certificate file (.p12 or .pfx)").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(p) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                if let Some(d) = &mut this.cert_sign {
                    let at = d
                        .entries
                        .iter()
                        .position(|e| matches!(&e.source, Source::File(x) if *x == p));
                    let i = at.unwrap_or_else(|| {
                        d.entries.insert(
                            0,
                            Entry {
                                source: Source::File(p),
                                cert: None,
                            },
                        );
                        0
                    });
                    d.selected = Some(i);
                    d.error = None;
                    let secret = d.secret.clone();
                    secret.update(cx, |s, cx| {
                        s.set_value("", window, cx);
                        s.focus(window, cx);
                    });
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Add Token Module…: a PKCS #11 library from the token's maker.
    fn cert_sign_add_module(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(i18n::t("Choose the token's PKCS #11 module").into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(p) = paths.into_iter().next() else {
                return;
            };
            let module = p.to_string_lossy().into_owned();
            let m = module.clone();
            let found = cx
                .background_spawn(async move { stores::token_candidates(&m) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let Some(d) = &mut this.cert_sign else {
                    return;
                };
                match found {
                    Ok(list) => {
                        if list.is_empty() {
                            d.error = Some(i18n::t(
                                "No signing certificates on the tokens this module sees. Is the token plugged in?",
                            ).into());
                        }
                        for c in list {
                            let cert = c.cert.clone();
                            d.entries.push(Entry {
                                source: Source::Store(Arc::new(c)),
                                cert: Some(cert),
                            });
                            d.selected = Some(d.entries.len() - 1);
                        }
                        cx.background_spawn(async move {
                            let mut s = Settings::load();
                            if !s.pdf.pkcs11_modules.contains(&module) {
                                s.pdf.pkcs11_modules.push(module);
                                let _ = s.save();
                            }
                        })
                        .detach();
                    }
                    Err(e) => d.error = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The box was drawn: back to the dialog, then sign.
    pub(crate) fn cert_box_drawn(
        &mut self,
        place: usize,
        rect: [f32; 4],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.markup.tool = Tool::Select;
        if let Some(d) = &mut self.cert_sign {
            d.hidden = false;
            d.drawn = Some((place, rect));
        }
        self.cert_sign_ok(window, cx);
    }

    fn cert_sign_close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cert_sign = None;
        if self.markup.tool == Tool::CertBox {
            self.markup.tool = Tool::Select;
        }
        self.focus(window, cx);
        cx.notify();
    }

    /// Sign…: draw the box if needed, ask where to save, then sign in the
    /// background.
    fn cert_sign_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let online = crate::online_signature_checks_enabled(cx);
        let Some(doc) = self.doc() else {
            return;
        };
        let bytes = doc.model.base().bytes.clone();
        let dir = self.dir();
        let stem = std::path::Path::new(&self.name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "document".into());
        let Some(d) = &mut self.cert_sign else {
            return;
        };
        let Some(entry) = d.selected.and_then(|i| d.entries.get(i)) else {
            d.error = Some(i18n::t("Choose a certificate first.").into());
            cx.notify();
            return;
        };
        let secret = d.secret.read(cx).value().to_string();
        if entry.needs_secret() && secret.is_empty() && matches!(entry.source, Source::Store(_)) {
            d.error = Some(i18n::t("Type the token's PIN.").into());
            cx.notify();
            return;
        }
        let tsa = d.tsa.read(cx).value().trim().to_string();
        if (d.timestamp || d.ltv) && !online {
            d.error = Some(i18n::t(
                "Time stamps and validation data need the internet. Turn on “Let Signature Checks Use the Internet” first.",
            ).into());
            cx.notify();
            return;
        }
        if d.timestamp && tsa.is_empty() {
            d.error = Some(i18n::t("Type the address of a time-stamp server.").into());
            cx.notify();
            return;
        }
        if d.show == Show::Draw && d.drawn.is_none() {
            d.hidden = true;
            d.error = None;
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
        let reason = d.reason.read(cx).value().trim().to_string();
        let location = d.location.read(cx).value().trim().to_string();
        let source = match &entry.source {
            Source::File(p) => Source::File(p.clone()),
            Source::Store(c) => Source::Store(c.clone()),
        };
        let picture = d.picture.and_then(|i| d.pictures.get(i)).and_then(art_of);
        let job = Job {
            source,
            secret: (!secret.is_empty()).then_some(secret),
            place,
            reason,
            location,
            certify: d.certify,
            level: match (d.timestamp, d.ltv) {
                (_, true) => Level::Lt,
                (true, false) => Level::T,
                _ => Level::B,
            },
            tsa,
            picture,
            words: signing::LookText {
                signed_by: i18n::t("Digitally signed by {}").into(),
                date: i18n::t("Date: {}").into(),
                reason: i18n::t("Reason: {}").into(),
                location: i18n::t("Location: {}").into(),
            },
            bytes,
        };
        if job.level > Level::B && job.tsa.is_empty() {
            d.error = Some(i18n::t("Type the address of a time-stamp server.").into());
            cx.notify();
            return;
        }
        // Remember the choices (never the password).
        let remember = (
            job.reason.clone(),
            job.location.clone(),
            job.tsa.clone(),
            d.timestamp,
            d.ltv,
            d.show != Show::Invisible,
            match &job.source {
                Source::File(p) => Some(p.to_string_lossy().into_owned()),
                _ => None,
            },
        );
        cx.background_spawn(async move {
            let mut s = Settings::load();
            let p = &mut s.pdf;
            p.signing_reason = remember.0;
            p.signing_location = remember.1;
            p.signing_tsa_url = remember.2;
            p.signing_timestamp = remember.3;
            p.signing_ltv = remember.4;
            p.signing_visible = remember.5;
            if let Some(f) = remember.6 {
                p.certificate_files.retain(|x| *x != f);
                p.certificate_files.insert(0, f);
                p.certificate_files.truncate(8);
            }
            let _ = s.save();
        })
        .detach();
        let target = cx.prompt_for_new_path(&dir, Some(&format!("{stem}-signed.pdf")));
        d.error = None;
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = target.await else {
                return;
            };
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = &mut this.cert_sign {
                    d.working = true;
                }
                this.busy = Some(i18n::t("Signing…").into());
                cx.notify();
            });
            let out = path.clone();
            let result = cx
                .background_spawn(async move {
                    let signed = job.run()?;
                    kantor_pdf::write_atomic(&out, &signed)
                        .map_err(|e| signing::Error::Pdf(e.to_string()))?;
                    Ok::<_, signing::Error>(())
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = None;
                match result {
                    Ok(()) => {
                        this.cert_sign = None;
                        this.status = tf("Signed and saved as {}", &[&path.display()]).into();
                        cx.emit(PdfEvent::Open(path));
                        this.focus(window, cx);
                    }
                    Err(e) => {
                        if let Some(d) = &mut this.cert_sign {
                            d.working = false;
                            d.hidden = false;
                            d.drawn = None;
                            d.error = Some(match e {
                                signing::Error::WrongPassword => {
                                    i18n::t("The password or PIN is wrong.").into()
                                }
                                signing::Error::Locked => i18n::t(
                                    "The token is locked after too many wrong PINs.",
                                )
                                .into(),
                                signing::Error::Offline => i18n::t(
                                    "Time stamps and validation data need the internet. Turn on “Let Signature Checks Use the Internet” first.",
                                )
                                .into(),
                                e => tf("Not signed: {}", &[&e]),
                            });
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

    pub(crate) fn render_cert_sign(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.cert_sign.as_ref().filter(|d| !d.hidden)?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let upd = |f: fn(&mut CertSign, usize), i: usize| {
            let v = view.clone();
            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                let _ = v.update(cx, |this, cx| {
                    if let Some(d) = &mut this.cert_sign {
                        f(d, i);
                        d.error = None;
                    }
                    cx.notify();
                });
            }
        };
        let label = |s: &'static str| {
            div()
                .text_size(FONT_SMALL)
                .font_weight(FontWeight::SEMIBOLD)
                .child(i18n::t(s))
        };

        // The certificates.
        let mut list = v_flex()
            .id("pdf-cert-list")
            .max_h(px(150.))
            .overflow_y_scroll()
            .rounded(RADIUS_MD)
            .border_1()
            .border_color(t.border_strong);
        for (i, e) in d.entries.iter().enumerate() {
            let on = d.selected == Some(i);
            list = list.child(
                v_flex()
                    .id(("pdf-cert", i))
                    .px(SPACE_3)
                    .py(SPACE_2)
                    .cursor_pointer()
                    .when(on, |r| r.bg(t.accent_soft))
                    .hover(|r| r.bg(t.hover))
                    .on_click(upd(|d, i| d.selected = Some(i), i))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(e.title()))
                    .child(
                        div()
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_muted)
                            .child(e.where_text()),
                    ),
            );
        }
        if d.entries.is_empty() {
            list = list.child(
                div()
                    .p(SPACE_3)
                    .text_color(t.fg_muted)
                    .text_size(FONT_SMALL)
                    .child(if d.loading {
                        i18n::t("Looking for certificates…")
                    } else {
                        i18n::t(
                            "No certificates found. Open your certificate file (.p12 or .pfx), or add your token's module.",
                        )
                    }),
            );
        }
        let details = d
            .selected
            .and_then(|i| d.entries.get(i))
            .and_then(|e| e.cert.as_ref())
            .map(|c| {
                let line = |k: &'static str, v: String| {
                    h_flex()
                        .gap(SPACE_2)
                        .text_size(FONT_SMALL)
                        .child(div().w(px(110.)).text_color(t.fg_muted).child(i18n::t(k)))
                        .child(div().flex_1().child(v))
                };
                v_flex()
                    .gap(SPACE_1)
                    .child(line("Issued by", c.issuer_name().display_name()))
                    .child(line("Valid to", sig::format_time(c.not_after)))
                    .child(line("Public key", c.key.describe()))
            });
        let needs_secret = d
            .selected
            .and_then(|i| d.entries.get(i))
            .is_some_and(Entry::needs_secret);
        let (vo, vm) = (view.clone(), view.clone());

        // Appearance.
        let show_btn = |id: &'static str, text: &'static str, which: Show| {
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
                        if let Some(d) = &mut this.cert_sign {
                            d.show = which;
                        }
                        cx.notify();
                    });
                })
        };
        let mut shows = h_flex()
            .gap(SPACE_2)
            .flex_wrap()
            .child(show_btn("pdf-cs-draw", i18n::t("Draw a Box"), Show::Draw))
            .child(show_btn(
                "pdf-cs-invisible",
                i18n::t("Invisible"),
                Show::Invisible,
            ));
        for (i, f) in d.fields.iter().enumerate().take(6) {
            let v = view.clone();
            let on = d.show == Show::Field(f.clone());
            let name = f.clone();
            shows = shows.child(
                Button::new(("pdf-cs-field", i))
                    .small()
                    .label(tf("Field “{}”", &[f]))
                    .when(on, |b| b.primary())
                    .when(!on, |b| b.ghost())
                    .on_click(move |_, _, cx| {
                        let name = name.clone();
                        let _ = v.update(cx, |this, cx| {
                            if let Some(d) = &mut this.cert_sign {
                                d.show = Show::Field(name);
                            }
                            cx.notify();
                        });
                    }),
            );
        }
        let mut pics = h_flex().gap(SPACE_2).flex_wrap().child(
            Button::new("pdf-cs-nopic")
                .small()
                .label(i18n::t("No Picture"))
                .when(d.picture.is_none(), |b| b.primary())
                .when(d.picture.is_some(), |b| b.ghost())
                .on_click(upd(|d, _| d.picture = None, 0)),
        );
        for i in 0..d.pictures.len().min(4) {
            pics = pics.child(
                Button::new(("pdf-cs-pic", i))
                    .small()
                    .label(tf("Signature {}", &[&(i + 1)]))
                    .when(d.picture == Some(i), |b| b.primary())
                    .when(d.picture != Some(i), |b| b.ghost())
                    .on_click(upd(|d, i| d.picture = Some(i), i)),
            );
        }
        let certify_btn = |id: &'static str, text: &'static str, which: Option<u8>| {
            Button::new(id)
                .small()
                .label(i18n::t(text))
                .when(d.certify == which, |b| b.primary())
                .when(d.certify != which, |b| b.ghost())
                .on_click(upd(
                    |d, w| d.certify = (w > 0).then_some(w as u8),
                    which.unwrap_or(0) as usize,
                ))
        };
        let (vt, vl) = (view.clone(), view.clone());
        let online = crate::online_signature_checks_enabled(cx);
        let card = crate::dialog::card(i18n::t("Sign with Certificate"), 560.0, cx)
            .overflow_y_scroll()
            .child(
                div().text_size(FONT_SMALL).text_color(t.fg_muted).child(i18n::t(
                    "A digital signature with your certificate: it shows who signed and that the document hasn't changed since. The signed document is saved as a new file.",
                )),
            )
            .child(label("Certificate"))
            .child(list)
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(
                        Button::new("pdf-cs-file")
                            .small()
                            .ghost()
                            .label(i18n::t("Open Certificate File…"))
                            .on_click(move |_, window, cx| {
                                let _ = vo.update(cx, |this, cx| this.cert_sign_open_file(window, cx));
                            }),
                    )
                    .child(
                        Button::new("pdf-cs-module")
                            .small()
                            .ghost()
                            .label(i18n::t("Add Token Module…"))
                            .on_click(move |_, window, cx| {
                                let _ = vm.update(cx, |this, cx| this.cert_sign_add_module(window, cx));
                            }),
                    ),
            )
            .children(details)
            .when(needs_secret, |c| c.child(Input::new(&d.secret)))
            .child(label("Appearance"))
            .child(shows)
            .when(d.show != Show::Invisible, |c| c.child(pics))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(div().flex_1().child(Input::new(&d.reason)))
                    .child(div().flex_1().child(Input::new(&d.location))),
            )
            .child(label("Certify"))
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .flex_wrap()
                    .child(certify_btn("pdf-cs-c0", "Don't Certify", None))
                    .child(certify_btn("pdf-cs-c1", "No Changes Allowed", Some(1)))
                    .child(certify_btn("pdf-cs-c2", "Form Filling and Signing", Some(2)))
                    .child(certify_btn("pdf-cs-c3", "Also Comments", Some(3))),
            )
            .child(
                h_flex()
                    .gap(SPACE_4)
                    .child(
                        Checkbox::new("pdf-cs-ts")
                            .checked(d.timestamp)
                            .label(i18n::t("Add a time stamp"))
                            .on_click(move |c, _, cx| {
                                let c = *c;
                                let _ = vt.update(cx, |this, cx| {
                                    if let Some(d) = &mut this.cert_sign {
                                        d.timestamp = c;
                                        if !c {
                                            d.ltv = false;
                                        }
                                    }
                                    cx.notify();
                                });
                            }),
                    )
                    .child(
                        Checkbox::new("pdf-cs-ltv")
                            .checked(d.ltv)
                            .label(i18n::t("Add validation data (LTV)"))
                            .on_click(move |c, _, cx| {
                                let c = *c;
                                let _ = vl.update(cx, |this, cx| {
                                    if let Some(d) = &mut this.cert_sign {
                                        d.ltv = c;
                                        if c {
                                            d.timestamp = true;
                                        }
                                    }
                                    cx.notify();
                                });
                            }),
                    ),
            )
            .when(d.timestamp, |c| c.child(Input::new(&d.tsa)))
            .when((d.timestamp || d.ltv) && !online, |c| {
                c.child(div().text_size(FONT_SMALL).text_color(t.fg_muted).child(i18n::t(
                    "Time stamps go online: turn on “Let Signature Checks Use the Internet” in the command palette.",
                )))
            })
            .children(
                d.error
                    .as_ref()
                    .map(|e| div().text_size(FONT_SMALL).text_color(t.danger).child(e.clone())),
            )
            .child({
                let (vc, vs) = (view.clone(), view.clone());
                h_flex()
                    .justify_end()
                    .gap(SPACE_3)
                    .child(
                        Button::new("pdf-cs-cancel")
                            .ghost()
                            .small()
                            .label(i18n::t("Cancel"))
                            .on_click(move |_, window, cx| {
                                let _ = vc.update(cx, |this, cx| this.cert_sign_close(window, cx));
                            }),
                    )
                    .child(
                        Button::new("pdf-cs-sign")
                            .primary()
                            .small()
                            .disabled(d.working || d.selected.is_none())
                            .label(if d.working {
                                i18n::t("Signing…")
                            } else {
                                i18n::t("Sign…")
                            })
                            .on_click(move |_, window, cx| {
                                let _ = vs.update(cx, |this, cx| this.cert_sign_ok(window, cx));
                            }),
                    )
            });
        let esc = view.clone();
        let card = card.on_key_down(move |e, window, cx| {
            if e.keystroke.key == "escape" {
                cx.stop_propagation();
                let _ = esc.update(cx, |this, cx| this.cert_sign_close(window, cx));
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
}

fn list_stores(modules: Vec<String>) -> Vec<stores::Candidate> {
    let mut out = Vec::new();
    if stores::SYSTEM_STORE {
        match stores::system_candidates() {
            Ok(c) => out.extend(c),
            Err(e) => log::warn!("system certificate store: {e}"),
        }
    }
    let mut seen = std::collections::HashSet::new();
    for m in modules {
        if !seen.insert(m.clone()) {
            continue;
        }
        match stores::token_candidates(&m) {
            Ok(c) => out.extend(c),
            Err(e) => log::info!("PKCS #11 module {m}: {e}"),
        }
    }
    out
}

/// One signing, done in the background.
struct Job {
    source: Source,
    secret: Option<String>,
    place: Place,
    reason: String,
    location: String,
    certify: Option<u8>,
    level: Level,
    tsa: String,
    picture: Option<Art>,
    words: signing::LookText,
    bytes: Arc<Vec<u8>>,
}

impl Job {
    fn run(self) -> Result<Vec<u8>, signing::Error> {
        let id: Identity = match &self.source {
            Source::File(p) => {
                let data = std::fs::read(p)
                    .map_err(|e| signing::Error::Key(format!("{}: {e}", p.display())))?;
                Identity::from_p12(&data, self.secret.as_deref().unwrap_or(""), p)?
            }
            Source::Store(c) => c.unlock(self.secret.as_deref())?,
        };
        let http = sig::Http::new();
        let now = sig::now();
        let nonempty = |s: &str| (!s.is_empty()).then(|| s.to_string());
        let mut o = signing::Options::new(self.place.clone());
        o.reason = nonempty(&self.reason);
        o.location = nonempty(&self.location);
        o.certify = self.certify;
        o.level = self.level;
        o.tsa = nonempty(&self.tsa);
        // Only reached with the user's consent (checked by the dialog).
        o.fetch = (self.level > Level::B).then_some(&http as &dyn sig::Fetch);
        o.now = now;
        if !matches!(self.place, Place::Invisible) {
            o.look = Some(Look {
                lines: signing::look_lines(
                    &id.name(),
                    now,
                    o.reason.as_deref(),
                    o.location.as_deref(),
                    &self.words,
                ),
                art: self.picture.clone(),
            });
        }
        signing::sign(&self.bytes, &id, &o)
    }
}
