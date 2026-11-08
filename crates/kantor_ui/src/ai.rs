//! Hooks the apps use to reach the AI features, which live in the shell:
//! actions they dispatch and whether AI is switched on.

use gpui_kit::*;

use crate::components::{IconButtonExt as _, icon_button};

actions!(
    kantor_ai,
    [
        /// Open the AI settings.
        OpenAiSettings,
        /// Sheets: describe a formula and get it written.
        AskForFormula,
        /// Sheets: explain the active cell's formula.
        ExplainFormula,
        /// Docs: rewrite or translate the selected paragraphs.
        RewriteSelection,
    ]
);

/// Whether AI features are on (master switch on and a provider chosen); the
/// shell keeps it up to date. AI buttons are hidden while it's off.
#[derive(Clone, Copy, Default)]
pub struct AiEnabled(pub bool);

impl Global for AiEnabled {}

pub fn ai_enabled(cx: &App) -> bool {
    cx.try_global::<AiEnabled>().is_some_and(|a| a.0)
}

/// A toolbar sparkle button that dispatches `action`.
pub fn ai_button(
    id: &'static str,
    tip: &str,
    action: impl Action + Clone,
    cx: &App,
) -> gpui_kit::component::button::Button {
    icon_button(id, gpui_kit::assets::IconName::Sparkles, false, cx)
        .tip(tip, None)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}
