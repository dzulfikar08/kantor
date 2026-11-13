//! The AI panels: settings, the Sheets formula assistant and Docs rewrite.
//!
//! Each is a small overlay card (like the palette). Requests run on a worker
//! thread ([`run`]) and stream back to the UI. Nothing is sent while AI is
//! switched off, and each feature sends only what its privacy line says.

mod formula;
mod rewrite;
mod settings;
pub(crate) mod widgets;

use gpui_kit::*;
use kantor_ai::provider::ProviderId;
use kantor_ai::{AiError, Cancel};
use kantor_core::settings::{AiSettings, Settings};

pub use formula::{FormulaMode, FormulaPanel};
pub use rewrite::RewritePanel;
pub use settings::SettingsPanel;

/// Key context of every AI panel.
pub const AI_PANEL: &str = "AiPanel";

actions!(kantor_ai_panel, [ClosePanel]);

pub fn init(cx: &mut App) {
    refresh_enabled(&Settings::load().ai, cx);
    cx.bind_keys([KeyBinding::new("escape", ClosePanel, Some(AI_PANEL))]);
}

/// Update the global switch the apps read to show AI buttons.
pub fn refresh_enabled(ai: &AiSettings, cx: &mut App) {
    cx.set_global(kantor_ui::ai::AiEnabled(client_allowed(ai)));
    cx.refresh_windows();
}

/// AI is switched on and a provider is chosen.
pub fn client_allowed(ai: &AiSettings) -> bool {
    ai.enabled && ProviderId::from_key(&ai.provider).is_some()
}

/// The chosen provider's name for privacy lines ("Anthropic (Claude)").
pub fn provider_label(ai: &AiSettings) -> &'static str {
    ProviderId::from_key(&ai.provider).map_or("no provider", |p| p.info().label)
}

enum Msg<T> {
    Text(String),
    Done(Result<T, AiError>),
}

/// Run `job` on a worker thread. `on_text` gets streamed text, `on_done`
/// the result, both on the UI thread while the view is alive.
pub fn run<V: 'static, T: Send + 'static>(
    cx: &mut Context<V>,
    job: impl FnOnce(&Cancel, &mut dyn FnMut(&str)) -> Result<T, AiError> + Send + 'static,
    mut on_text: impl FnMut(&mut V, &str, &mut Context<V>) + 'static,
    on_done: impl FnOnce(&mut V, Result<T, AiError>, &mut Context<V>) + 'static,
) -> Cancel {
    let cancel = Cancel::default();
    let (tx, rx) = async_channel::unbounded::<Msg<T>>();
    let worker_cancel = cancel.clone();
    let spawned = std::thread::Builder::new()
        .name("kantor-ai".into())
        .spawn(move || {
            let text_tx = tx.clone();
            let result = job(&worker_cancel, &mut |t| {
                let _ = text_tx.send_blocking(Msg::Text(t.to_owned()));
            });
            let _ = tx.send_blocking(Msg::Done(result));
        });
    let mut on_done = Some(on_done);
    if let Err(err) = spawned {
        if let Some(f) = on_done.take() {
            f_now(
                cx,
                f,
                AiError::Network(format!("can't start a worker: {err}")),
            );
        }
        return cancel;
    }
    cx.spawn(async move |this, cx| {
        while let Ok(msg) = rx.recv().await {
            let alive = this.update(cx, |view, cx| match msg {
                Msg::Text(t) => on_text(view, &t, cx),
                Msg::Done(result) => {
                    if let Some(f) = on_done.take() {
                        f(view, result, cx);
                    }
                }
            });
            if alive.is_err() {
                break;
            }
        }
    })
    .detach();
    cancel
}

fn f_now<V: 'static, T>(
    cx: &mut Context<V>,
    f: impl FnOnce(&mut V, Result<T, AiError>, &mut Context<V>) + 'static,
    err: AiError,
) {
    cx.spawn(async move |this, cx| {
        let _ = this.update(cx, |view, cx| f(view, Err(err), cx));
    })
    .detach();
}

/// The configured client, built on the worker thread (the keychain may block).
pub fn client(ai: &AiSettings) -> Result<kantor_ai::Client, AiError> {
    let cfg = kantor_ai::ProviderConfig::from_settings(ai, |p| kantor_ai::keys::load_key(p.key()))?;
    Ok(kantor_ai::Client::new(
        cfg,
        std::sync::Arc::new(kantor_ai::UreqTransport::default()),
    ))
}

/// What an AI panel tells the workspace.
pub enum PanelEvent {
    /// Close the panel and return focus to the document.
    Close,
    /// Open AI Settings instead (from a panel shown while AI is off).
    OpenSettings,
}
