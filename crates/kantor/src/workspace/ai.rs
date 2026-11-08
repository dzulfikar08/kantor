//! Showing the AI panels (see `crate::ai`) over the workspace.

use gpui_kit::*;
use kantor_ui::ai::{AskForFormula, ExplainFormula, OpenAiSettings, RewriteSelection};
use kantor_ui::tokens;

use super::Workspace;
use super::doc::DocView;
use crate::ai::{FormulaMode, FormulaPanel, PanelEvent, RewritePanel, SettingsPanel};

pub(super) struct AiOverlay {
    view: AnyView,
    _subscription: Subscription,
}

impl Workspace {
    pub(super) fn open_ai_settings(
        &mut self,
        _: &OpenAiSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel = cx.new(|cx| SettingsPanel::new(window, cx));
        panel.update(cx, |p, cx| p.focus_first(window, cx));
        self.show_ai(panel, window, cx);
    }

    pub(super) fn ask_for_formula(
        &mut self,
        _: &AskForFormula,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_formula(FormulaMode::Ask, window, cx);
    }

    pub(super) fn explain_formula(
        &mut self,
        _: &ExplainFormula,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_formula(FormulaMode::Explain, window, cx);
    }

    fn open_formula(&mut self, mode: FormulaMode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(DocView::Sheets(sheets)) = self.focused_view() else {
            self.message = "Formulas: open a spreadsheet first".into();
            cx.notify();
            return;
        };
        let panel = cx.new(|cx| FormulaPanel::new(sheets, mode, window, cx));
        panel.update(cx, |p, cx| p.focus_first(window, cx));
        self.show_ai(panel, window, cx);
    }

    pub(super) fn rewrite_selection(
        &mut self,
        _: &RewriteSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(DocView::Docs(docs)) = self.focused_view() else {
            self.message = "Rewrite: open a document and select some text first".into();
            cx.notify();
            return;
        };
        let panel = cx.new(|cx| RewritePanel::new(docs, cx));
        panel.update(cx, |p, cx| p.focus_first(window, cx));
        self.show_ai(panel, window, cx);
    }

    fn show_ai<V: Render + EventEmitter<PanelEvent>>(
        &mut self,
        panel: Entity<V>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let subscription =
            cx.subscribe_in(&panel, window, |ws, _, event: &PanelEvent, window, cx| {
                ws.close_ai(window, cx);
                if let PanelEvent::OpenSettings = event {
                    ws.open_ai_settings(&OpenAiSettings, window, cx);
                }
            });
        self.ai = Some(AiOverlay {
            view: panel.into(),
            _subscription: subscription,
        });
        cx.notify();
    }

    pub(super) fn close_ai(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.ai.take().is_some() {
            // Return focus to the document the panel covered.
            self.activate(self.active, window, cx);
        }
    }

    pub(super) fn render_ai(&self, cx: &mut Context<Self>) -> Option<Div> {
        let view = self.ai.as_ref()?.view.clone();
        let entity = cx.entity().downgrade();
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .bg(tokens(cx).scrim)
                .occlude()
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.close_ai(window, cx));
                })
                .child(view),
        )
    }
}
