//! Paparan AI in the app: the account and balance the panels show, error
//! messages with what to do next (Top up, Sign in), and the low-credit hint.
//! The network side is `kantor_ai::paparan`.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{Sizable as _, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ai::paparan::{self, Me, Paparan};
use kantor_ai::{AiError, ProviderId, UreqTransport};
use kantor_core::settings::AiSettings;
use kantor_ui::i18n::{self, tf};
use kantor_ui::metrics::*;

/// The signed-in Paparan account, shared by every AI panel.
#[derive(Clone, Debug, Default)]
pub struct PaparanState {
    /// `None` until checked.
    pub signed_in: Option<bool>,
    /// The account and balance from the last `me`.
    pub me: Option<Me>,
    pub refreshing: bool,
    /// Why the last check failed (offline…).
    pub error: Option<String>,
}

impl Global for PaparanState {}

pub fn state(cx: &App) -> PaparanState {
    cx.try_global::<PaparanState>().cloned().unwrap_or_default()
}

fn update_state(cx: &mut App, f: impl FnOnce(&mut PaparanState)) {
    let mut s = state(cx);
    f(&mut s);
    cx.set_global(s);
}

/// Whether the chosen provider is Paparan AI.
pub fn chosen(ai: &AiSettings) -> bool {
    ProviderId::from_key(&ai.provider) == Some(ProviderId::Paparan)
}

/// The account API over the real network.
pub fn api() -> Paparan {
    Paparan::new(Arc::new(UreqTransport::new(Duration::from_secs(30))))
}

enum Fetched {
    SignedOut,
    Me(Me),
    Failed(AiError),
}

/// Fetch the account and balance in the background (once at a time).
pub fn refresh(cx: &mut App) {
    if state(cx).refreshing {
        return;
    }
    update_state(cx, |s| s.refreshing = true);
    let (tx, rx) = async_channel::bounded::<Fetched>(1);
    let spawned = std::thread::Builder::new()
        .name("kantor-paparan".into())
        .spawn(move || {
            let r = match paparan::load_token() {
                None => Fetched::SignedOut,
                Some(token) => match api().me(&token) {
                    Ok(me) => Fetched::Me(me),
                    Err(AiError::SignedOut) => {
                        // The server no longer takes this token.
                        paparan::delete_token();
                        Fetched::SignedOut
                    }
                    Err(e) => Fetched::Failed(e),
                },
            };
            let _ = tx.send_blocking(r);
        });
    if spawned.is_err() {
        update_state(cx, |s| s.refreshing = false);
        return;
    }
    cx.spawn(async move |cx| {
        let r = rx.recv().await;
        cx.update(|cx| {
            update_state(cx, |s| {
                s.refreshing = false;
                match r {
                    Ok(Fetched::SignedOut) => {
                        s.signed_in = Some(false);
                        s.me = None;
                        s.error = None;
                    }
                    Ok(Fetched::Me(me)) => {
                        s.signed_in = Some(true);
                        s.me = Some(me);
                        s.error = None;
                    }
                    Ok(Fetched::Failed(e)) => {
                        // Still signed in (a token is stored), just unreachable.
                        s.signed_in = Some(true);
                        s.error = Some(describe(&e));
                    }
                    Err(_) => {}
                }
            });
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Refresh once if the account hasn't been checked this run and Paparan AI
/// is the chosen provider.
pub fn refresh_if_unknown(ai: &AiSettings, cx: &mut App) {
    if chosen(ai) && state(cx).signed_in.is_none() {
        refresh(cx);
    }
}

/// After any AI job: take the balance a Paparan reply reported, and fetch
/// the account again when credits were spent or the sign-in failed.
pub fn after_call(err: Option<&AiError>, cx: &mut App) {
    let seen = paparan::take_credits();
    if let Some(c) = seen {
        update_state(cx, |s| {
            if let Some(me) = &mut s.me {
                me.credits.remaining = c.remaining;
                if let Some(low) = c.low {
                    me.credits.low = low;
                }
            }
        });
    }
    let signed_out = matches!(err, Some(AiError::SignedOut));
    if signed_out {
        update_state(cx, |s| {
            s.signed_in = Some(false);
            s.me = None;
        });
    }
    if seen.is_some() || signed_out || matches!(err, Some(AiError::InsufficientCredits { .. })) {
        refresh(cx);
    }
}

/// A sign-in or sign-out just happened.
pub fn set_signed_out(cx: &mut App) {
    update_state(cx, |s| {
        s.signed_in = Some(false);
        s.me = None;
        s.error = None;
    });
}

/// The web app's address (`https://app.paparanai.id`), for links the
/// server didn't send.
pub fn web_base() -> String {
    let base = paparan::api_base();
    match base.find("/api/") {
        Some(i) => base[..i].to_owned(),
        None => base,
    }
}

/// Where to buy credits: the error's link, then the account's, then the
/// web app.
pub fn topup_url(err_url: Option<&str>, cx: &App) -> String {
    err_url
        .map(str::to_owned)
        .or_else(|| state(cx).me.and_then(|m| m.topup_url))
        .unwrap_or_else(web_base)
}

/// An error in the interface language.
pub fn describe(e: &AiError) -> String {
    match e {
        AiError::SignedOut => {
            i18n::t("Signed out of Paparan AI. Sign in again in AI Settings.").to_owned()
        }
        AiError::InsufficientCredits { .. } => {
            i18n::t("Not enough Paparan AI credits. Top up to continue.").to_owned()
        }
        AiError::RateLimited {
            retry_after: Some(s),
            ..
        } => tf("Too many requests. Try again in {} s.", &[s]),
        AiError::RateLimited { .. } => {
            i18n::t("Too many requests. Try again in a moment.").to_owned()
        }
        other => other.to_string(),
    }
}

/// What a failed request's panel offers besides the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FailAction {
    /// Open this page to buy credits.
    TopUp(String),
    /// Open AI Settings to sign in.
    SignIn,
}

/// A failed request's message and the button to show with it.
pub fn failure(e: &AiError, cx: &App) -> (String, Option<FailAction>) {
    let action = match e {
        AiError::InsufficientCredits { topup_url: u, .. } => {
            Some(FailAction::TopUp(topup_url(u.as_deref(), cx)))
        }
        AiError::SignedOut => Some(FailAction::SignIn),
        _ => None,
    };
    (describe(e), action)
}

/// A footer button for `action`; Sign in calls `sign_in`.
pub fn action_button(action: &FailAction, sign_in: impl Fn(&mut App) + 'static) -> Button {
    match action {
        FailAction::TopUp(url) => {
            let url = url.clone();
            Button::new("paparan-top-up")
                .primary()
                .small()
                .label(i18n::t("Top up"))
                .on_click(move |_, _, cx| cx.open_url(&url))
        }
        FailAction::SignIn => Button::new("paparan-sign-in-again")
            .primary()
            .small()
            .label(i18n::t("Sign in…"))
            .on_click(move |_, _, cx| sign_in(cx)),
    }
}

/// "Credits are running low" with a Top up link, when Paparan AI is the
/// chosen provider and its balance is low.
pub fn low_credit_hint(ai: &AiSettings, cx: &App) -> Option<Div> {
    if !chosen(ai) {
        return None;
    }
    let me = state(cx).me?;
    if !me.credits.low {
        return None;
    }
    let t = kantor_ui::tokens(cx);
    let url = topup_url(None, cx);
    Some(
        h_flex()
            .gap(SPACE_3)
            .child(
                div()
                    .flex_1()
                    .text_size(FONT_SMALL)
                    .text_color(t.warning)
                    .child(tf(
                        "Paparan AI credits are running low: {} left.",
                        &[&me.credits.display()],
                    )),
            )
            .child(
                Button::new("paparan-low-top-up")
                    .outline()
                    .small()
                    .label(i18n::t("Top up"))
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            ),
    )
}

/// Run `job` on a worker thread that can report steps; `on_step` and
/// `on_done` run on the UI thread while the view is alive.
pub fn run_steps<V: 'static, P: Send + 'static, T: Send + 'static>(
    cx: &mut Context<V>,
    job: impl FnOnce(&dyn Fn(P)) -> T + Send + 'static,
    mut on_step: impl FnMut(&mut V, P, &mut Context<V>) + 'static,
    on_done: impl FnOnce(&mut V, T, &mut Context<V>) + 'static,
) {
    enum Msg<P, T> {
        Step(P),
        Done(T),
    }
    let (tx, rx) = async_channel::unbounded::<Msg<P, T>>();
    let spawned = std::thread::Builder::new()
        .name("kantor-paparan".into())
        .spawn(move || {
            let step_tx = tx.clone();
            let result = job(&move |p| {
                let _ = step_tx.send_blocking(Msg::Step(p));
            });
            let _ = tx.send_blocking(Msg::Done(result));
        });
    if spawned.is_err() {
        return;
    }
    let mut on_done = Some(on_done);
    cx.spawn(async move |this, cx| {
        while let Ok(msg) = rx.recv().await {
            let alive = this.update(cx, |view, cx| match msg {
                Msg::Step(p) => on_step(view, p, cx),
                Msg::Done(t) => {
                    if let Some(f) = on_done.take() {
                        f(view, t, cx);
                    }
                }
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn errors_say_what_to_do() {
        let e = AiError::RateLimited {
            retry_after: Some(30),
            message: String::new(),
        };
        assert!(describe(&e).contains("30"));
        assert_eq!(
            describe(&AiError::Timeout),
            AiError::Timeout.to_string(),
            "other errors keep their text"
        );
    }
}
