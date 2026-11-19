//! Digital signatures: when a signed PDF opens, its signatures are checked
//! in the background ([`pdf_engine::signatures`]); a banner above the pages
//! gives the verdict, the Signatures panel in the sidebar lists each
//! signature (signer, time, type, what changed after signing), a dialog
//! shows the certificates, and View Signed Version opens the exact bytes
//! that were signed, read-only.
//!
//! Network: nothing is fetched unless "Let Signature Checks Use the
//! Internet" is on (`[pdf] online_signature_checks`, default on) *and* the
//! user clicks Check Online for this document (see PRIVACY.md).

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::settings::Settings;
use kantor_ui::i18n::{self, tf, tk};
use kantor_ui::{metrics::*, tokens};
use pdf_engine::signatures::{
    self as sig, Arg, Change, Msg, PathEnd, Report, RootSource, Signature, Status,
};

use crate::{Panel, PdfEvent, PdfView};

/// The signature check of one view.
#[derive(Default)]
pub(crate) struct Sigs {
    pub report: Option<Arc<Report>>,
    pub checking: bool,
    pub banner_closed: bool,
    /// Signatures whose details are open in the panel.
    pub open: HashSet<usize>,
    /// The certificate dialog: signature, certificate on its path.
    pub cert: Option<(usize, usize)>,
    /// The file's bytes as opened (what was checked).
    bytes: Option<Arc<Vec<u8>>>,
    _task: Option<Task<()>>,
}

impl Sigs {
    pub fn signed(&self) -> bool {
        self.report.as_ref().is_some_and(|r| r.is_signed())
    }
}

/// The setting, read once per session.
struct OnlinePolicy {
    on: bool,
}
impl Global for OnlinePolicy {}

fn policy(cx: &mut App) -> &mut OnlinePolicy {
    if !cx.has_global::<OnlinePolicy>() {
        cx.set_global(OnlinePolicy {
            on: Settings::load().pdf.online_signature_checks,
        });
    }
    cx.global_mut::<OnlinePolicy>()
}

/// "Let Signature Checks Use the Internet" is on.
pub fn online_signature_checks_enabled(cx: &mut App) -> bool {
    policy(cx).on
}

/// Turn online signature checks on or off, and remember it. Returns the
/// new state.
pub fn toggle_online_signature_checks(cx: &mut App) -> bool {
    let p = policy(cx);
    p.on = !p.on;
    let on = p.on;
    cx.background_spawn(async move {
        let mut s = Settings::load();
        if s.pdf.online_signature_checks != on {
            s.pdf.online_signature_checks = on;
            if let Err(err) = s.save() {
                log::error!("saving settings: {err:#}");
            }
        }
    })
    .detach();
    cx.refresh_windows();
    on
}

/// The roots to trust: the system's, Kantor's bundled ones and, for tests
/// and screenshots, the PEM file named by `KANTOR_PDF_TRUST`.
fn trust_store() -> sig::TrustStore {
    let mut t = sig::TrustStore::standard();
    if let Some(path) = std::env::var_os("KANTOR_PDF_TRUST")
        && let Ok(data) = std::fs::read(path)
    {
        sig::x509::parse_pem_or_der(&data)
            .into_iter()
            .for_each(|c| t.add(c));
    }
    t
}

impl PdfView {
    /// Check the signatures of `bytes` (the file as opened) in the
    /// background. `online`: the user allowed fetching for this check.
    pub(crate) fn check_signatures(
        &mut self,
        bytes: Arc<Vec<u8>>,
        online: bool,
        cx: &mut Context<Self>,
    ) {
        self.sigs.bytes = Some(bytes.clone());
        self.sigs.checking = true;
        let work = cx.background_spawn(async move {
            if !online && !sig::has_signatures(&bytes) {
                return None;
            }
            let http = sig::Http::new();
            let opts = sig::Options {
                trust: trust_store(),
                fetch: if online { Some(&http) } else { None },
                now: sig::now(),
            };
            Some(Arc::new(sig::verify(&bytes, &opts)))
        });
        self.sigs._task = Some(cx.spawn(async move |this, cx| {
            let report = work.await;
            let _ = this.update(cx, |v, cx| {
                v.sigs.checking = false;
                let first = v.sigs.report.is_none();
                if let Some(r) = &report
                    && r.is_signed()
                    && first
                    && !r.all_valid()
                {
                    // Problems show at once, as in Acrobat: the panel opens
                    // with the details of the signatures that have them.
                    if v.panel.is_none() {
                        v.panel = Some(Panel::Signatures);
                    }
                    v.sigs.open = r
                        .signatures
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| !s.status.is_ok())
                        .map(|(i, _)| i)
                        .collect();
                }
                v.sigs.report = report;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn recheck_signatures(&mut self, online: bool, cx: &mut Context<Self>) {
        if let Some(bytes) = self.sigs.bytes.clone() {
            self.check_signatures(bytes, online, cx);
        }
    }

    /// Open the revision signature `i` signed, read-only, in a new tab.
    fn view_signed_version(&mut self, i: usize, cx: &mut Context<Self>) {
        let (Some(report), Some(bytes)) = (&self.sigs.report, &self.sigs.bytes) else {
            return;
        };
        let Some(s) = report.signatures.get(i) else {
            return;
        };
        let data = sig::signed_version(bytes, s);
        let stem = self
            .path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.name.clone());
        let stem = tf("{} (signed version, revision {})", &[&stem, &s.revision]);
        let written = kantor_core::dirs::print_pdf_path(&stem)
            .and_then(|path| std::fs::write(&path, data).map(|()| path));
        match written {
            Ok(path) => cx.emit(PdfEvent::OpenSignedVersion(path)),
            Err(e) => {
                log::warn!("writing the signed version: {e}");
                self.status = i18n::t("Kantor couldn't open the signed version").into();
                cx.notify();
            }
        }
    }
}

/// A message in the interface language.
pub(crate) fn text(m: &Msg) -> String {
    let args: Vec<String> = m
        .args
        .iter()
        .map(|a| match a {
            Arg::Word(w) => i18n::t(w).to_owned(),
            Arg::Text(t) => t.clone(),
        })
        .collect();
    let args: Vec<&dyn std::fmt::Display> =
        args.iter().map(|a| a as &dyn std::fmt::Display).collect();
    i18n::fill(i18n::t_dyn(m.template), &args)
}

fn status_look(status: Status, cx: &App) -> (Lucide, Hsla) {
    let t = tokens(cx);
    match status {
        Status::Valid | Status::ValidWithChanges => (Lucide::ShieldCheck, t.success),
        Status::Unknown => (Lucide::ShieldQuestionMark, t.warning),
        Status::Invalid => (Lucide::ShieldX, t.danger),
    }
}

fn status_text(status: Status) -> &'static str {
    match status {
        Status::Valid => i18n::t("Valid"),
        Status::ValidWithChanges => i18n::t("Valid, with allowed changes"),
        Status::Unknown => i18n::t("Validity unknown"),
        Status::Invalid => i18n::t("Invalid"),
    }
}

fn icon(icon: Lucide, color: Hsla, size: Pixels) -> impl IntoElement {
    gpui_kit::component::Icon::new(icon)
        .with_size(size)
        .text_color(color)
}

/// The bar above the pages of a signed document.
pub(crate) fn banner(view: &PdfView, cx: &mut Context<PdfView>) -> Option<AnyElement> {
    let s = &view.sigs;
    if s.banner_closed {
        return None;
    }
    let report = s.report.as_ref().filter(|r| r.is_signed())?;
    let t = tokens(cx);
    let worst = report.overall().unwrap_or(Status::Unknown);
    let (ic, color) = status_look(worst, cx);
    let msg = if report.all_valid() {
        if report.signatures.len() == 1 {
            i18n::t("Signed and the signature is valid.")
        } else {
            i18n::t("Signed and all signatures are valid.")
        }
    } else {
        i18n::t("At least one signature has problems.")
    };
    let weak = cx.entity().downgrade();
    let close = weak.clone();
    let panel_open = view.panel == Some(Panel::Signatures);
    Some(
        h_flex()
            .flex_none()
            .w_full()
            .px(SPACE_6)
            .py(SPACE_2)
            .gap(SPACE_4)
            .bg(color.opacity(0.12))
            .border_b_1()
            .border_color(color.opacity(0.4))
            .text_size(FONT_SMALL)
            .text_color(t.fg)
            .child(icon(ic, color, ICON_MD))
            .child(div().flex_1().min_w_0().truncate().child(msg))
            .when(!panel_open, |d| {
                d.child(
                    Button::new("pdf-sig-open-panel")
                        .small()
                        .outline()
                        .label(i18n::t("Signature Panel"))
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |v, cx| {
                                v.panel = Some(Panel::Signatures);
                                cx.notify();
                            });
                        }),
                )
            })
            .child(
                Button::new("pdf-sig-banner-close")
                    .ghost()
                    .xsmall()
                    .icon(Lucide::X)
                    .tooltip(i18n::t("Close"))
                    .on_click(move |_, _, cx| {
                        let _ = close.update(cx, |v, cx| {
                            v.sigs.banner_closed = true;
                            cx.notify();
                        });
                    }),
            )
            .into_any_element(),
    )
}

fn detail_row(label: &str, value: impl Into<SharedString>, cx: &App) -> Div {
    let t = tokens(cx);
    h_flex()
        .items_start()
        .gap(SPACE_3)
        .child(
            div()
                .flex_none()
                .w(px(64.))
                .text_color(t.fg_muted)
                .child(label.to_owned()),
        )
        .child(div().flex_1().min_w_0().child(value.into()))
}

fn time_text(t: i64) -> String {
    sig::format_time(t)
}

/// The Signatures panel's body.
pub(crate) fn panel(view: &mut PdfView, cx: &mut Context<PdfView>) -> AnyElement {
    let t = tokens(cx);
    let weak = cx.entity().downgrade();
    let muted = |s: &str| {
        div()
            .p(SPACE_6)
            .text_size(FONT_SMALL)
            .text_color(t.fg_muted)
            .child(s.to_owned())
            .into_any_element()
    };
    let Some(report) = view.sigs.report.clone() else {
        return if view.sigs.checking {
            muted(i18n::t("Checking signatures…"))
        } else {
            muted(i18n::t("This document isn't signed."))
        };
    };
    if let Some(e) = &report.error {
        log::warn!("signature check: {e}");
        return muted(i18n::t("Kantor can't read this document's signatures."));
    }
    if !report.is_signed() && report.empty_fields.is_empty() {
        return muted(i18n::t("This document isn't signed."));
    }
    let mut list = v_flex().gap(SPACE_3).p(SPACE_3);
    for (i, s) in report.signatures.iter().enumerate() {
        list = list.child(signature_card(i, s, view.sigs.open.contains(&i), cx));
    }
    for f in &report.empty_fields {
        let page = f.page.map(|p| tf("page {}", &[&p])).unwrap_or_default();
        list = list.child(
            h_flex()
                .gap(SPACE_2)
                .px(SPACE_3)
                .py(SPACE_2)
                .rounded(RADIUS_MD)
                .border_1()
                .border_color(t.border)
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(icon(Lucide::Signature, t.fg_muted, ICON_SM))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(tf("Empty signature field {} {}", &[&f.name, &page])),
                ),
        );
    }
    // Online checks and checking again.
    let allowed = online_signature_checks_enabled(cx);
    let mut foot = v_flex().gap(SPACE_2).px(SPACE_3).pb(SPACE_4);
    if report.online_would_help && !report.checked_online {
        if allowed {
            let w = weak.clone();
            foot = foot
                .child(
                    div()
                        .text_size(FONT_SMALL)
                        .text_color(t.fg_muted)
                        .child(i18n::t("Some certificates can be checked online: Kantor asks the certificate authorities for missing certificates and whether they were revoked.")),
                )
                .child(
                    Button::new("pdf-sig-online")
                        .small()
                        .outline()
                        .icon(Lucide::Globe)
                        .label(i18n::t("Check Online"))
                        .loading(view.sigs.checking)
                        .on_click(move |_, _, cx| {
                            let _ = w.update(cx, |v, cx| v.recheck_signatures(true, cx));
                        }),
                );
        } else {
            foot = foot.child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(i18n::t("Turn on Let Signature Checks Use the Internet (command palette) to check certificates online.")),
            );
        }
    }
    let w = weak.clone();
    foot = foot.child(
        Button::new("pdf-sig-recheck")
            .small()
            .ghost()
            .icon(Lucide::RefreshCw)
            .label(i18n::t("Check Again"))
            .loading(view.sigs.checking)
            .on_click(move |_, _, cx| {
                let _ = w.update(cx, |v, cx| v.recheck_signatures(false, cx));
            }),
    );
    div()
        .id("pdf-sig-panel")
        .size_full()
        .overflow_y_scroll()
        .child(list)
        .child(foot)
        .into_any_element()
}

fn signature_card(i: usize, s: &Signature, open: bool, cx: &mut Context<PdfView>) -> Div {
    let t = tokens(cx);
    let weak = cx.entity().downgrade();
    let (ic, color) = status_look(s.status, cx);
    let title = if s.kind == sig::SigKind::DocTimestamp {
        i18n::t("Document time stamp").to_owned()
    } else {
        tf("Signed by {}", &[&s.signer_name()])
    };
    let toggle = weak.clone();
    let head = h_flex()
        .id(("pdf-sig-head", i))
        .items_start()
        .gap(SPACE_2)
        .cursor_pointer()
        .child(div().pt(px(1.)).child(icon(ic, color, ICON_MD)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.fg)
                        .child(title),
                )
                .child(
                    div()
                        .text_color(color)
                        .child(status_text(s.status).to_owned()),
                )
                .children(
                    s.time()
                        .map(|tm| div().text_color(t.fg_muted).child(time_text(tm))),
                ),
        )
        .child(icon(
            if open {
                Lucide::ChevronDown
            } else {
                Lucide::ChevronRight
            },
            t.fg_muted,
            ICON_SM,
        ))
        .on_click(move |_, _, cx| {
            let _ = toggle.update(cx, |v, cx| {
                if !v.sigs.open.remove(&i) {
                    v.sigs.open.insert(i);
                }
                cx.notify();
            });
        });
    let mut card = v_flex()
        .gap(SPACE_2)
        .p(SPACE_3)
        .rounded(RADIUS_MD)
        .bg(t.surface)
        .border_1()
        .border_color(t.border)
        .text_size(FONT_SMALL)
        .text_color(t.fg)
        .child(head);
    if !open {
        return card;
    }
    let mut notes = v_flex().gap(SPACE_1);
    for n in &s.notes {
        notes = notes.child(div().child(text(n)));
    }
    card = card.child(notes);
    let mut rows = v_flex().gap(SPACE_1).text_color(t.fg);
    if let Some(org) = s.organisation() {
        rows = rows.child(detail_row(i18n::t("Org."), org, cx));
    }
    if let Some(r) = &s.reason {
        rows = rows.child(detail_row(i18n::t("Reason"), r.clone(), cx));
    }
    if let Some(l) = &s.location {
        rows = rows.child(detail_row(i18n::t("Location"), l.clone(), cx));
    }
    if let Some(tm) = s.time() {
        let how = if s.timestamp.as_ref().is_some_and(sig::Timestamp::trusted) {
            i18n::t("trusted time stamp")
        } else if s.kind == sig::SigKind::DocTimestamp {
            i18n::t("time stamp")
        } else {
            i18n::t("signer's computer clock")
        };
        rows = rows.child(detail_row(
            i18n::t("Time"),
            format!("{} ({how})", time_text(tm)),
            cx,
        ));
    }
    rows = rows.child(detail_row(i18n::t("Type"), text(&s.kind.label()), cx));
    if let Some(p) = s.certification {
        let level = match p {
            1 => i18n::t("Certified: no changes allowed"),
            3 => i18n::t("Certified: form filling, signing and comments allowed"),
            _ => i18n::t("Certified: form filling and signing allowed"),
        };
        rows = rows.child(detail_row(i18n::t("Certifies"), level, cx));
    }
    if let Some(ts) = &s.timestamp {
        let by = ts
            .tsa
            .as_ref()
            .map(|c| c.subject_name().display_name())
            .unwrap_or_default();
        let state = if ts.trusted() {
            i18n::t("valid")
        } else if ts.integrity == sig::Integrity::Intact {
            i18n::t("authority not trusted")
        } else {
            i18n::t("invalid")
        };
        rows = rows.child(detail_row(
            i18n::t("Time stamp"),
            format!("{by} ({state})"),
            cx,
        ));
    }
    if let Some(path) = &s.path {
        let end = match path.end {
            PathEnd::Trusted(RootSource::System) => i18n::t("Trusted root (system)"),
            PathEnd::Trusted(RootSource::Bundled) => i18n::t("Trusted root (bundled with Kantor)"),
            PathEnd::Trusted(RootSource::Extra) => i18n::t("Trusted root (added)"),
            PathEnd::UntrustedRoot => i18n::t("Root not trusted"),
            PathEnd::Incomplete => i18n::t("Chain incomplete"),
        };
        let root = path
            .certs
            .last()
            .map(|c| c.cert.subject_name().display_name())
            .unwrap_or_default();
        rows = rows.child(detail_row(
            i18n::t("Issued by"),
            format!("{root} · {end}"),
            cx,
        ));
    }
    rows = rows.child(detail_row(
        i18n::t("Revision"),
        tf("{} of {}", &[&s.revision, &s.revisions]),
        cx,
    ));
    card = card.child(rows);
    // Changes after signing.
    if !s.covers_whole_file {
        let mut changes = v_flex().gap(SPACE_1).child(
            div()
                .text_color(t.fg_muted)
                .child(i18n::t("Changes after signing")),
        );
        if s.changes.is_empty() {
            changes = changes.child(div().child(i18n::t("None")));
        }
        for c in &s.changes {
            changes = changes.child(change_row(c, c.allowed(s.docmdp), cx));
        }
        card = card.child(changes);
    }
    // Actions.
    let (w1, w2, w3) = (weak.clone(), weak.clone(), weak.clone());
    let mut actions = h_flex().flex_wrap().gap(SPACE_2).pt(SPACE_1);
    if s.path.is_some() {
        actions = actions.child(
            Button::new(("pdf-sig-cert", i))
                .xsmall()
                .outline()
                .label(i18n::t("Certificate…"))
                .on_click(move |_, window, cx| {
                    let _ = w1.update(cx, |v, cx| {
                        v.sigs.cert = Some((i, 0));
                        window.focus(&v.dialog_focus, cx);
                        cx.notify();
                    });
                }),
        );
    }
    if !s.covers_whole_file && s.signed_len > 0 {
        actions = actions.child(
            Button::new(("pdf-sig-signed", i))
                .xsmall()
                .outline()
                .label(i18n::t("View Signed Version"))
                .on_click(move |_, _, cx| {
                    let _ = w2.update(cx, |v, cx| v.view_signed_version(i, cx));
                }),
        );
    }
    if let Some(p) = s.page {
        actions = actions.child(
            Button::new(("pdf-sig-page", i))
                .xsmall()
                .ghost()
                .label(tf("Go to page {}", &[&p]))
                .on_click(move |_, _, cx| {
                    let _ = w3.update(cx, |v, cx| v.go_to_page(p - 1, cx));
                }),
        );
    }
    card.child(actions)
}

fn change_row(c: &Change, allowed: bool, cx: &App) -> Div {
    let t = tokens(cx);
    let (ic, color) = if allowed {
        (Lucide::Check, t.fg_muted)
    } else {
        (Lucide::TriangleAlert, t.danger)
    };
    h_flex()
        .items_start()
        .gap(SPACE_2)
        .child(div().pt(px(2.)).child(icon(ic, color, ICON_SM)))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(if allowed { t.fg } else { t.danger })
                .child(text(&c.msg())),
        )
}

/// The certificate dialog.
pub(crate) fn cert_dialog(view: &PdfView, cx: &mut Context<PdfView>) -> Option<AnyElement> {
    let (si, ci) = view.sigs.cert?;
    let report = view.sigs.report.as_ref()?;
    let s = report.signatures.get(si)?;
    // The signer's path, then the time stamp authority's.
    let mut certs: Vec<(&sig::PathCert, bool)> = s
        .path
        .iter()
        .flat_map(|p| p.certs.iter().map(|c| (c, false)))
        .collect();
    if let Some(p) = s.timestamp.as_ref().and_then(|ts| ts.path.as_ref()) {
        certs.extend(p.certs.iter().map(|c| (c, true)));
    }
    let (pc, _) = certs.get(ci).copied().or_else(|| certs.first().copied())?;
    let t = tokens(cx);
    let weak = cx.entity().downgrade();
    let mut chain = v_flex().gap(SPACE_1).w(px(220.)).flex_none();
    let mut last_tsa = false;
    for (k, (c, tsa)) in certs.iter().enumerate() {
        if *tsa && !last_tsa {
            chain = chain.child(
                div()
                    .pt(SPACE_3)
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(i18n::t("Time stamp authority")),
            );
        }
        last_tsa = *tsa;
        let w = weak.clone();
        let depth = if *tsa {
            certs.iter().take(k).filter(|(_, x)| *x).count()
        } else {
            k
        };
        let (ic, color) = if c.revocation == sig::Revocation::Unknown || !c.problems.is_empty() {
            (Lucide::ShieldQuestionMark, t.warning)
        } else if matches!(c.revocation, sig::Revocation::Revoked { .. }) {
            (Lucide::ShieldX, t.danger)
        } else {
            (Lucide::ShieldCheck, t.success)
        };
        chain = chain.child(
            h_flex()
                .id(("pdf-cert-row", k))
                .gap(SPACE_2)
                .pl(px(8. + 12. * depth as f32))
                .pr(SPACE_2)
                .py(SPACE_1)
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .when(k == ci, |d| d.bg(t.selected))
                .hover(|d| d.bg(t.hover))
                .text_size(FONT_SMALL)
                .child(icon(ic, color, ICON_SM))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(c.cert.subject_name().display_name()),
                )
                .on_click(move |_, _, cx| {
                    let _ = w.update(cx, |v, cx| {
                        v.sigs.cert = Some((si, k));
                        cx.notify();
                    });
                }),
        );
    }
    let c = &pc.cert;
    let row = |label: &str, value: String| {
        h_flex()
            .items_start()
            .gap(SPACE_4)
            .child(
                div()
                    .flex_none()
                    .w(px(120.))
                    .text_color(t.fg_muted)
                    .child(label.to_owned()),
            )
            .child(div().flex_1().min_w_0().child(value))
    };
    let ku = c
        .ext
        .key_usage
        .map(|k| {
            sig::x509::key_usage_names(k)
                .into_iter()
                .map(|n| i18n::t_dyn(n).to_owned())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| i18n::t("Not limited").to_owned());
    let eku = c
        .ext
        .eku
        .iter()
        .map(|e| sig::x509::eku_name(e))
        .collect::<Vec<_>>()
        .join(", ");
    let mut problems = Vec::new();
    for p in &pc.problems {
        problems.push(match p {
            sig::CertProblem::Expired => i18n::t("Expired at the time checked"),
            sig::CertProblem::NotYetValid => i18n::t("Not yet valid at the time checked"),
            sig::CertProblem::NotACa => i18n::t("Not allowed to issue certificates"),
            sig::CertProblem::KeyUsage => i18n::t("Key usage doesn't allow this use"),
            sig::CertProblem::UnknownCritical(_) => {
                i18n::t("Has a critical extension Kantor doesn't understand")
            }
        });
    }
    let mut details = v_flex()
        .flex_1()
        .min_w_0()
        .gap(SPACE_2)
        .text_size(FONT_SMALL)
        .child(row(i18n::t("Issued to"), c.subject_name().to_string()))
        .child(row(i18n::t("Issued by"), c.issuer_name().to_string()))
        .child(row(i18n::t("Valid from"), time_text(c.not_before)))
        .child(row(i18n::t("Valid to"), time_text(c.not_after)))
        .child(row(
            i18n::t("Serial number"),
            sig::x509::colon_hex(&c.serial),
        ))
        .child(row(i18n::t("Public key"), c.key.describe()))
        .child(row(i18n::t("Key usage"), ku));
    if !eku.is_empty() {
        details = details.child(row(i18n::t("Extended key usage"), eku));
    }
    details = details
        .child(row(i18n::t("Revocation"), text(&pc.revocation.msg())))
        .child(row(
            i18n::t("SHA-256 fingerprint"),
            sig::x509::colon_hex(&c.sha256()),
        ))
        .child(row(
            i18n::t("SHA-1 fingerprint"),
            sig::x509::colon_hex(&c.sha1()),
        ));
    if !problems.is_empty() {
        details = details.child(div().text_color(t.danger).child(problems.join(" · ")));
    }
    let close = weak.clone();
    let card = v_flex()
        .id("pdf-cert-dialog")
        .occlude()
        .w(px(760.))
        .max_h(px(560.))
        .gap(SPACE_5)
        .p(SPACE_6)
        .rounded(RADIUS_MD)
        .bg(t.surface)
        .border_1()
        .border_color(t.border_strong)
        .shadow_md()
        .text_size(FONT_BODY)
        .text_color(t.fg)
        .track_focus(&view.dialog_focus)
        .on_key_down(move |e, window, cx| {
            if e.keystroke.key == "escape" || e.keystroke.key == "enter" {
                cx.stop_propagation();
                let _ = close.update(cx, |v, cx| {
                    v.sigs.cert = None;
                    v.focus(window, cx);
                    cx.notify();
                });
            }
        })
        .child(
            div()
                .text_size(FONT_EMPHASIS)
                .font_weight(FontWeight::SEMIBOLD)
                .child(i18n::t("Certificate Details")),
        )
        .child(
            h_flex()
                .items_start()
                .gap(SPACE_5)
                .min_h_0()
                .child(chain)
                .child(details),
        )
        .child(
            h_flex().justify_end().child(
                Button::new("pdf-cert-close")
                    .primary()
                    .label(i18n::t("Close"))
                    .on_click(move |_, window, cx| {
                        let _ = weak.update(cx, |v, cx| {
                            v.sigs.cert = None;
                            v.focus(window, cx);
                            cx.notify();
                        });
                    }),
            ),
        );
    Some(
        kantor_ui::overlay::backdrop()
            .child(
                h_flex()
                    .absolute()
                    .top(CONTROL_LG * 2. + SPACE_5)
                    .left_0()
                    .right_0()
                    .justify_center()
                    .child(card),
            )
            .into_any_element(),
    )
}

/// Message templates and words from [`pdf_engine::signatures`], shown
/// with [`text`] (`t_dyn`): listed here so the catalog check sees them.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "read by the catalog check and the test below")
)]
pub(crate) const ENGINE_KEYS: &[&str] = &[
    tk("PKCS #7, detached (adbe.pkcs7.detached)"),
    tk("PKCS #7 with SHA-1 (adbe.pkcs7.sha1)"),
    tk("CAdES / PAdES (ETSI.CAdES.detached)"),
    tk("Document time stamp (RFC 3161)"),
    tk("Other ({})"),
    tk("The signature's byte range doesn't cover the signed revision correctly."),
    tk(
        "The document was altered or damaged after signing: the signed bytes don't match the signature.",
    ),
    tk("The signature doesn't match the signer's certificate."),
    tk("The signature names a different signing certificate than the one it carries."),
    tk("The signer's certificate isn't in the signature."),
    tk("Kantor can't check this kind of signature ({})."),
    tk("The signature data can't be read."),
    tk("A certificate in the chain was revoked on {}."),
    tk("The document was modified after it was signed."),
    tk("The document was changed after signing in ways the certifying signer didn't allow."),
    tk(
        "The signer's identity can't be confirmed: the certificate chain ends at a root that isn't trusted.",
    ),
    tk("The signer's identity can't be confirmed: an issuer's certificate is missing."),
    tk("A certificate in the chain had expired at the time it was checked."),
    tk("A certificate in the chain wasn't valid yet at the time it was checked."),
    tk("A certificate in the chain isn't allowed to issue certificates."),
    tk("A certificate's key usage doesn't allow it to be used this way."),
    tk("A certificate has a critical extension Kantor doesn't understand."),
    tk("Revocation not checked: Kantor couldn't confirm that the certificates weren't revoked."),
    tk("The signature's time stamp couldn't be verified."),
    tk("The time stamp authority isn't trusted; the signature was checked as of now."),
    tk("The document was changed after signing, in ways the signer allowed."),
    tk("The document hasn't been changed since it was signed."),
    tk("The signed revision is unchanged; only signatures or validation data were added after it."),
    tk("Long-term validation data added"),
    tk("Signature added ({})"),
    tk("Document time stamp added"),
    tk("Form field filled in"),
    tk("Form field filled in: {}"),
    tk("{} added on page {}"),
    tk("{} added"),
    tk("{} changed on page {}"),
    tk("{} changed"),
    tk("Annotation removed on page {}"),
    tk("Page content changed on page {}"),
    tk("Page content changed"),
    tk("Pages added, removed or reordered"),
    tk("Document properties changed"),
    tk("Other change ({})"),
    tk("Trusted root (not checked)"),
    tk("Not revoked (OCSP response in the file)"),
    tk("Not revoked (CRL in the file)"),
    tk("Not revoked (checked online, OCSP)"),
    tk("Not revoked (checked online, CRL)"),
    tk("Revoked on {}"),
    tk("The issuer publishes no revocation information"),
    tk("Not checked (OCSP responder certificate)"),
    tk("Not checked"),
    tk("Stamp"),
    tk("Comment"),
    tk("Text box"),
    tk("Highlight"),
    tk("Underline"),
    tk("Strikethrough"),
    tk("Squiggly underline"),
    tk("Drawing"),
    tk("Rectangle"),
    tk("Oval"),
    tk("Line"),
    tk("Polygon"),
    tk("Polyline"),
    tk("Attachment"),
    tk("Link"),
    tk("Insertion mark"),
    tk("Redaction"),
    tk("Sound"),
    tk("Annotation"),
    tk("Digital Signature"),
    tk("Non-Repudiation"),
    tk("Key Encipherment"),
    tk("Data Encipherment"),
    tk("Key Agreement"),
    tk("Certificate Sign"),
    tk("CRL Sign"),
    tk("Encipher Only"),
    tk("Decipher Only"),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every message the checker gives for the fixtures is a catalog key
    /// listed in [`ENGINE_KEYS`] with an Indonesian translation.
    #[std::prelude::v1::test]
    fn engine_messages_are_translated() {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../pdf_engine/tests/fixtures/signatures"
        );
        let mut trust = sig::TrustStore::bundled();
        let pem = std::fs::read(format!("{dir}/test-root.pem")).unwrap();
        sig::x509::parse_pem_or_der(&pem)
            .into_iter()
            .for_each(|c| trust.add(c));
        let opts = sig::Options {
            trust,
            fetch: None,
            now: sig::now(),
        };
        let mut msgs: Vec<Msg> = Vec::new();
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            if e.path().extension().is_none_or(|x| x != "pdf") {
                continue;
            }
            let r = sig::verify(&std::fs::read(e.path()).unwrap(), &opts);
            for s in &r.signatures {
                msgs.extend(s.notes.iter().cloned());
                msgs.extend(s.changes.iter().map(Change::msg));
                msgs.push(s.kind.label());
                for p in s.path.iter() {
                    msgs.extend(p.certs.iter().map(|c| c.revocation.msg()));
                }
            }
        }
        assert!(msgs.len() > 20);
        let mut keys: Vec<&str> = Vec::new();
        for m in &msgs {
            keys.push(m.template);
            for a in &m.args {
                if let Arg::Word(w) = a {
                    keys.push(w);
                }
            }
        }
        for k in keys {
            assert!(ENGINE_KEYS.contains(&k), "{k:?} missing from ENGINE_KEYS");
            assert!(i18n::indonesian(k).is_some(), "{k:?} has no Indonesian");
        }
    }
}
