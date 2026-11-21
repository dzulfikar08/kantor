//! The Batch dialog's "Sign with Certificate" step: the certificate
//! picker and the options of the Sign with Certificate dialog, saved in
//! batch files as `pdf_engine::signing::batch::SignStep` (no password).
//! The password or PIN is asked for once when the batch starts.

use std::path::Path;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{
    Sizable as _,
    button::Button,
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::settings::Settings;
use kantor_pdf::batch::{self, Anchor, Place, Step};
use kantor_ui::i18n::{self, tf, tk};
use kantor_ui::{metrics::*, tokens};
use pdf_engine::signing::batch::{Appearance, Certificate, LevelName, SignStep, why};

use crate::PdfView;
use crate::batch::StepUi;
use crate::dialog::{Dialog, choices, note, row};

/// Where the signature shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Show {
    Invisible,
    Box,
    Field,
}

/// Which page a new box goes on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OnPage {
    First,
    Last,
    Number,
}

pub(crate) struct SignUi {
    /// The certificate and the name shown for it.
    pub cert: Option<(Certificate, String)>,
    /// Certificates to pick from (remembered files, tokens, the system
    /// store), listed in the background.
    pub found: Vec<(Certificate, String)>,
    pub level: LevelName,
    pub tsa: Entity<InputState>,
    /// 0: approval signature; 1–3: certify.
    pub certify: u8,
    pub show: Show,
    pub field: Entity<InputState>,
    pub page: OnPage,
    pub page_no: Entity<InputState>,
    pub anchor: Anchor,
    pub reason: Entity<InputState>,
    pub location: Entity<InputState>,
    /// (what the batch file says, name shown).
    pub picture: Option<(String, String)>,
    /// The options as loaded: what this dialog doesn't edit (contact,
    /// PSS, the box's size and exact place) is kept from it.
    pub base: SignStep,
}

/// Messages from `pdf_engine::signing::batch::why`, shown with `t_dyn`:
/// listed so the catalog check sees them.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "read by the catalog check and the test below")
)]
pub(crate) const WHY_KEYS: &[&str] = &[
    tk(
        "Not signed: the file's security settings don't allow signing with the password that opens it (give the owner password)",
    ),
    tk("Not signed: the file is certified and allows no changes, not even another signature"),
    tk(
        "Not signed: the file is already signed, and only the first signature can certify a document",
    ),
    tk("Not signed: the file has no signature field with the step's name"),
    tk("Not signed: the step's signature field is already signed"),
    tk(
        "Not signed: the file is certified, and a new visible signature would change its pages (sign an empty signature field, or invisibly)",
    ),
    tk("Not signed: the file doesn't have the step's page"),
];

/// A skip reason from the sign step, in the user's language.
pub(crate) fn why_text(s: &str) -> String {
    if why::ALL.contains(&s) || kantor_esign::batch::why::ALL.contains(&s) {
        i18n::t_dyn(s).to_owned()
    } else {
        s.to_owned()
    }
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn shown(c: &Certificate) -> String {
    match c {
        Certificate::File(p) => file_name(Path::new(p)),
        Certificate::Token { name, .. } if !name.is_empty() => tf("Token: {}", &[name]),
        Certificate::Token { module, .. } => tf("Token: {}", &[&file_name(Path::new(module))]),
        Certificate::System { name } => {
            format!("{} ({})", name, i18n::t("System certificate store"))
        }
    }
}

fn input(
    window: &mut Window,
    cx: &mut Context<PdfView>,
    value: &str,
    hint: &str,
) -> Entity<InputState> {
    let i = cx.new(|cx| InputState::new(window, cx).placeholder(hint.to_owned()));
    i.update(cx, |i, cx| i.set_value(value.to_owned(), window, cx));
    i
}

/// Signature pictures from My Signatures & Stamps (PNG/JPEG only).
fn my_signatures() -> Vec<(String, String)> {
    let Some(dir) = crate::sign::stamps_dir() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut v: Vec<String> = rd.flatten().map(|e| file_name(&e.path())).collect();
    v.sort();
    v.into_iter()
        .filter(|n| n.contains(".sig.") && (n.ends_with(".png") || n.ends_with(".jpg")))
        .enumerate()
        .map(|(k, n)| (n, tf("My signature {}", &[&(k + 1)])))
        .collect()
}

impl SignUi {
    pub(crate) fn new(step: SignStep, window: &mut Window, cx: &mut Context<PdfView>) -> SignUi {
        let s = Settings::load().pdf;
        let found: Vec<(Certificate, String)> = s
            .certificate_files
            .iter()
            // App Sandbox: reopened through the bookmark made when picked.
            .filter(|p| kantor_sandbox::access(Path::new(p)) && Path::new(p).exists())
            .map(|p| {
                let c = Certificate::File(p.clone());
                let n = shown(&c);
                (c, n)
            })
            .collect();
        let empty_file = matches!(&step.certificate, Certificate::File(p) if p.is_empty());
        let cert = if empty_file {
            found.first().cloned()
        } else {
            Some((step.certificate.clone(), shown(&step.certificate)))
        };
        let (show, field, page, page_no, anchor) = match &step.appearance {
            Appearance::Invisible => (
                Show::Invisible,
                String::new(),
                OnPage::Last,
                String::new(),
                Anchor::BottomRight,
            ),
            Appearance::Field { name } => (
                Show::Field,
                name.clone(),
                OnPage::Last,
                String::new(),
                Anchor::BottomRight,
            ),
            Appearance::Box { page, place, .. } => {
                let (p, n) = match page.trim().to_ascii_lowercase().as_str() {
                    "first" => (OnPage::First, String::new()),
                    "last" => (OnPage::Last, String::new()),
                    other => (OnPage::Number, other.to_owned()),
                };
                let a = match place {
                    Place::Anchor { anchor, .. } => *anchor,
                    _ => Anchor::BottomRight,
                };
                (Show::Box, String::new(), p, n, a)
            }
        };
        let reason = if step.reason.is_empty() {
            s.signing_reason.clone()
        } else {
            step.reason.clone()
        };
        let location = if step.location.is_empty() {
            s.signing_location.clone()
        } else {
            step.location.clone()
        };
        let tsa = if step.tsa.is_empty() {
            s.signing_tsa_url.clone()
        } else {
            step.tsa.clone()
        };
        let picture = (!step.image.is_empty()).then(|| {
            let shown = file_name(Path::new(&step.image));
            (step.image.clone(), shown)
        });
        SignUi {
            cert,
            found,
            level: step.level,
            tsa: input(
                window,
                cx,
                &tsa,
                i18n::t("Time-stamp server address (https://…)"),
            ),
            certify: step.certify.unwrap_or(0),
            show,
            field: input(window, cx, &field, i18n::t("Signature field name")),
            page,
            page_no: input(window, cx, &page_no, ""),
            anchor,
            reason: input(window, cx, &reason, i18n::t("Reason (optional)")),
            location: input(window, cx, &location, i18n::t("Location (optional)")),
            picture,
            base: step,
        }
    }

    /// The step for the batch file (`n`: its number, for messages).
    pub(crate) fn to_step(&self, n: usize, cx: &App) -> Result<Step, String> {
        let Some((cert, _)) = &self.cert else {
            return Err(tf("Step {}: choose a certificate", &[&n]));
        };
        let mut s = self.base.clone();
        s.certificate = cert.clone();
        s.level = self.level;
        s.tsa = self.tsa.read(cx).value().trim().to_owned();
        if s.level != LevelName::BB && s.tsa.is_empty() {
            return Err(tf(
                "Step {}: type the address of a time-stamp server",
                &[&n],
            ));
        }
        s.certify = (self.certify > 0).then_some(self.certify);
        s.reason = self.reason.read(cx).value().trim().to_owned();
        s.location = self.location.read(cx).value().trim().to_owned();
        s.appearance = match self.show {
            Show::Invisible => Appearance::Invisible,
            Show::Field => {
                let name = self.field.read(cx).value().trim().to_owned();
                if name.is_empty() {
                    return Err(tf("Step {}: type the signature field's name", &[&n]));
                }
                Appearance::Field { name }
            }
            Show::Box => {
                let page = match self.page {
                    OnPage::First => "first".to_owned(),
                    OnPage::Last => "last".to_owned(),
                    OnPage::Number => {
                        let t = self.page_no.read(cx).value().trim().to_owned();
                        if !t.parse::<u32>().is_ok_and(|p| p >= 1) {
                            return Err(tf("Step {}: type a page number", &[&n]));
                        }
                        t
                    }
                };
                let (width, height, kept) = match &self.base.appearance {
                    Appearance::Box {
                        width,
                        height,
                        place,
                        ..
                    } => (*width, *height, Some(*place)),
                    _ => (180.0, 60.0, None),
                };
                // Keep the loaded margins when the corner is unchanged.
                let place = match kept {
                    Some(p @ Place::Anchor { anchor, .. }) if anchor == self.anchor => p,
                    _ => Place::Anchor {
                        anchor: self.anchor,
                        dx: 36.0,
                        dy: 36.0,
                    },
                };
                Appearance::Box {
                    page,
                    place,
                    width,
                    height,
                }
            }
        };
        s.image = match (&self.picture, self.show) {
            (Some((spec, _)), Show::Box | Show::Field) => spec.clone(),
            _ => String::new(),
        };
        // Check what the engine checks, with the step's number.
        s.validate().map_err(|e| tf("Step {}: {}", &[&n, &e]))?;
        Ok(s.to_step())
    }
}

fn sign_ui(b: &mut Dialog, i: usize) -> Option<&mut SignUi> {
    match b {
        Dialog::Batch(b) => match b.steps.get_mut(i) {
            Some(StepUi::Sign(s)) => Some(s),
            _ => None,
        },
        _ => None,
    }
}

impl PdfView {
    /// A new sign step: list the certificates in the background.
    pub(crate) fn batch_sign_step(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> StepUi {
        // Visible or not as the Sign with Certificate dialog was last used.
        let step = SignStep {
            appearance: if Settings::load().pdf.signing_visible {
                Appearance::Box {
                    page: "last".into(),
                    place: Place::default(),
                    width: 180.0,
                    height: 60.0,
                }
            } else {
                Appearance::Invisible
            },
            ..SignStep::default()
        };
        let ui = SignUi::new(step, window, cx);
        self.batch_list_certificates(cx);
        StepUi::Sign(Box::new(ui))
    }

    /// Tokens and the system store can be slow: look in the background.
    pub(crate) fn batch_list_certificates(&mut self, cx: &mut Context<Self>) {
        let s = Settings::load().pdf;
        let modules: Vec<String> = s
            .pkcs11_modules
            .iter()
            .filter(|_| pdf_engine::signing::stores::TOKEN_MODULES)
            .cloned()
            .chain(pdf_engine::signing::stores::common_modules())
            .collect();
        let task = cx.background_spawn(async move {
            crate::certsign::list_stores(modules)
                .into_iter()
                .map(|c| {
                    let name = c.cert.subject_name().display_name();
                    match c.source() {
                        pdf_engine::signing::Source::Token { module, .. } => {
                            Certificate::Token { module, name }
                        }
                        _ => Certificate::System { name },
                    }
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let found = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(Dialog::Batch(b)) = this.dialog.as_mut() {
                    for s in b.steps.iter_mut() {
                        if let StepUi::Sign(s) = s {
                            for c in &found {
                                if !s.found.iter().any(|(x, _)| x == c) {
                                    s.found.push((c.clone(), shown(c)));
                                }
                            }
                            if s.cert.is_none() {
                                s.cert = s.found.first().cloned();
                            }
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn batch_sign_pick_file(
        &mut self,
        i: usize,
        picture: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(if picture {
                i18n::t("Choose a PNG or JPEG picture").into()
            } else {
                i18n::t("Choose a certificate file (.p12 or .pfx)").into()
            }),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(p) = paths.into_iter().next() else {
                return;
            };
            kantor_sandbox::remember(&p);
            let _ = this.update(cx, |this, cx| {
                if let Some(d) = this.dialog.as_mut()
                    && let Some(s) = sign_ui(d, i)
                {
                    let spec = p.to_string_lossy().into_owned();
                    if picture {
                        s.picture = Some((spec, file_name(&p)));
                    } else {
                        let c = Certificate::File(spec);
                        let n = shown(&c);
                        if !s.found.iter().any(|(x, _)| *x == c) {
                            s.found.insert(0, (c.clone(), n.clone()));
                        }
                        s.cert = Some((c, n));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn render_sign_step(&self, i: usize, s: &SignUi, cx: &mut Context<Self>) -> Div {
        let view = cx.entity().downgrade();
        let t = tokens(cx);
        // The certificate.
        let weak = view.clone();
        let found = s.found.clone();
        let cert_btn = Button::new(("pdf-batch-cert", i))
            .xsmall()
            .outline()
            .label(
                s.cert
                    .as_ref()
                    .map_or_else(|| i18n::t("Choose…").to_owned(), |c| c.1.clone()),
            )
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for (c, name) in found.clone() {
                    let v = weak.clone();
                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                        let pick = (c.clone(), name.clone());
                        let _ = v.update(cx, |this, cx| {
                            if let Some(d) = this.dialog.as_mut()
                                && let Some(s) = sign_ui(d, i)
                            {
                                s.cert = Some(pick);
                            }
                            cx.notify();
                        });
                    }));
                }
                let v = weak.clone();
                menu.separator().item(
                    PopupMenuItem::new(i18n::t("Open Certificate File…")).on_click(
                        move |_, w, cx| {
                            let _ =
                                v.update(cx, |this, cx| this.batch_sign_pick_file(i, false, w, cx));
                        },
                    ),
                )
            });
        // Level.
        let levels: Vec<((usize, LevelName), String)> = [
            (LevelName::BB, tk("Signature only")),
            (LevelName::BT, tk("Time stamp")),
            (LevelName::BLt, tk("Time stamp and LTV")),
        ]
        .into_iter()
        .map(|(l, label)| ((i, l), i18n::t(label).to_owned()))
        .chain((s.level == LevelName::BLta).then(|| ((i, LevelName::BLta), "B-LTA".to_owned())))
        .collect();
        let set_level: fn(&mut Dialog, (usize, LevelName)) = |d, (i, v)| {
            if let Some(s) = sign_ui(d, i) {
                s.level = v;
            }
        };
        // Appearance.
        let shows: Vec<((usize, Show), String)> = [
            (Show::Box, tk("Box on a Page")),
            (Show::Field, tk("Signature Field")),
            (Show::Invisible, tk("Invisible")),
        ]
        .into_iter()
        .map(|(v, l)| ((i, v), i18n::t(l).to_owned()))
        .collect();
        let set_show: fn(&mut Dialog, (usize, Show)) = |d, (i, v)| {
            if let Some(s) = sign_ui(d, i) {
                s.show = v;
            }
        };
        let pages: Vec<((usize, OnPage), String)> = [
            (OnPage::First, tk("First")),
            (OnPage::Last, tk("Last")),
            (OnPage::Number, tk("Page")),
        ]
        .into_iter()
        .map(|(v, l)| ((i, v), i18n::t(l).to_owned()))
        .collect();
        let set_page: fn(&mut Dialog, (usize, OnPage)) = |d, (i, v)| {
            if let Some(s) = sign_ui(d, i) {
                s.page = v;
            }
        };
        let mut grid = v_flex().gap(px(2.));
        for rw in 0..3 {
            let mut line = h_flex().gap(px(2.));
            for col in 0..3 {
                let a = Anchor::ALL[rw * 3 + col];
                let v = view.clone();
                line = line.child(
                    div()
                        .id(("pdf-batch-sign-anchor", i * 9 + rw * 3 + col))
                        .size(px(14.))
                        .rounded(px(2.))
                        .border_1()
                        .border_color(t.border_strong)
                        .when(a == s.anchor, |d| d.bg(t.accent))
                        .hover(|d| d.bg(t.hover))
                        .cursor_pointer()
                        .on_click(move |_, _, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if let Some(d) = this.dialog.as_mut()
                                    && let Some(s) = sign_ui(d, i)
                                {
                                    s.anchor = a;
                                }
                                cx.notify();
                            });
                        }),
                );
            }
            grid = grid.child(line);
        }
        // Picture beside the text.
        let weak = view.clone();
        let pic = Button::new(("pdf-batch-sign-pic", i))
            .xsmall()
            .outline()
            .label(
                s.picture
                    .as_ref()
                    .map_or_else(|| i18n::t("No Picture").to_owned(), |p| p.1.clone()),
            )
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                let v = weak.clone();
                menu = menu.item(PopupMenuItem::new(i18n::t("No Picture")).on_click(
                    move |_, _, cx| {
                        let _ = v.update(cx, |this, cx| {
                            if let Some(d) = this.dialog.as_mut()
                                && let Some(s) = sign_ui(d, i)
                            {
                                s.picture = None;
                            }
                            cx.notify();
                        });
                    },
                ));
                for (file, name) in my_signatures() {
                    let v = weak.clone();
                    let spec = format!("{}{file}", batch::MY_STAMPS);
                    menu = menu.item(PopupMenuItem::new(name.clone()).on_click(move |_, _, cx| {
                        let pick = (spec.clone(), name.clone());
                        let _ = v.update(cx, |this, cx| {
                            if let Some(d) = this.dialog.as_mut()
                                && let Some(s) = sign_ui(d, i)
                            {
                                s.picture = Some(pick);
                            }
                            cx.notify();
                        });
                    }));
                }
                let v = weak.clone();
                menu.separator().item(
                    PopupMenuItem::new(i18n::t("Choose a Picture File…")).on_click(
                        move |_, w, cx| {
                            let _ =
                                v.update(cx, |this, cx| this.batch_sign_pick_file(i, true, w, cx));
                        },
                    ),
                )
            });
        let certify: Vec<((usize, u8), String)> = [
            (0u8, tk("Don't Certify")),
            (1, tk("No Changes Allowed")),
            (2, tk("Form Filling and Signing")),
            (3, tk("Also Comments")),
        ]
        .into_iter()
        .map(|(v, l)| ((i, v), i18n::t(l).to_owned()))
        .collect();
        let set_certify: fn(&mut Dialog, (usize, u8)) = |d, (i, v)| {
            if let Some(s) = sign_ui(d, i) {
                s.certify = v;
            }
        };
        let online = crate::online_signature_checks_enabled(cx);
        v_flex()
            .gap(SPACE_2)
            .child(row(i18n::t("Certificate"), h_flex().child(cert_btn), cx))
            .child(row(
                i18n::t("Time stamp"),
                choices("pdf-batch-sign-level", &levels, (i, s.level), &view, set_level),
                cx,
            ))
            .when(s.level != LevelName::BB, |c| {
                c.child(row("", Input::new(&s.tsa).small(), cx))
                    .when(!online, |c| {
                        c.child(note(
                            i18n::t("Time stamps go online: turn on “Let Signature Checks Use the Internet” in the command palette."),
                            false,
                            cx,
                        ))
                    })
            })
            .child(row(
                i18n::t("Appearance"),
                choices("pdf-batch-sign-show", &shows, (i, s.show), &view, set_show),
                cx,
            ))
            .when(s.show == Show::Box, |c| {
                c.child(row(
                    i18n::t("Page"),
                    h_flex()
                        .gap(SPACE_2)
                        .child(choices("pdf-batch-sign-page", &pages, (i, s.page), &view, set_page))
                        .when(s.page == OnPage::Number, |r| {
                            r.child(div().w(px(70.)).child(Input::new(&s.page_no).small()))
                        }),
                    cx,
                ))
                .child(row(i18n::t("Position"), grid, cx))
            })
            .when(s.show == Show::Field, |c| {
                c.child(row(i18n::t("Field name"), Input::new(&s.field).small(), cx))
            })
            .when(s.show != Show::Invisible, |c| {
                c.child(row(i18n::t("Picture"), h_flex().child(pic), cx))
            })
            .child(row(
                i18n::t("Reason"),
                h_flex()
                    .gap(SPACE_2)
                    .child(div().flex_1().child(Input::new(&s.reason).small()))
                    .child(div().flex_1().child(Input::new(&s.location).small())),
                cx,
            ))
            .child(row(
                i18n::t("Certify"),
                choices("pdf-batch-sign-certify", &certify, (i, s.certify), &view, set_certify),
                cx,
            ))
            .child(note(
                i18n::t("Always the last step, so the signature covers everything the batch did. The password or PIN is asked for when the batch runs; it is never saved."),
                false,
                cx,
            ))
    }
}

/// Make relative certificate and picture paths of a loaded sign step
/// absolute (the dialog runs it from the document's folder).
pub(crate) fn absolute(mut s: SignStep, base: &Path) -> SignStep {
    let abs = |p: &str| -> String {
        if p.is_empty() || p.starts_with(batch::MY_STAMPS) || Path::new(p).is_absolute() {
            p.to_owned()
        } else {
            base.join(p).to_string_lossy().into_owned()
        }
    };
    if let Certificate::File(p) = &s.certificate {
        s.certificate = Certificate::File(abs(p));
    }
    s.image = abs(&s.image);
    s
}

/// For the unlock prompt: the certificate being unlocked.
pub(crate) fn unlock_label(c: &Certificate) -> String {
    shown(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn skip_reasons_are_in_the_catalog() {
        for w in why::ALL {
            assert!(WHY_KEYS.contains(&w), "{w:?} missing from WHY_KEYS");
            assert!(i18n::indonesian(w).is_some(), "{w:?} has no Indonesian");
        }
    }

    #[std::prelude::v1::test]
    fn loaded_paths_become_absolute() {
        let s = SignStep {
            certificate: Certificate::File("certs/me.p12".into()),
            image: "my-stamps/a.sig.png".into(),
            ..SignStep::default()
        };
        let a = absolute(s, Path::new("/work"));
        assert_eq!(
            a.certificate,
            Certificate::File(
                Path::new("/work")
                    .join("certs/me.p12")
                    .to_string_lossy()
                    .into_owned()
            )
        );
        assert_eq!(a.image, "my-stamps/a.sig.png");
    }
}
