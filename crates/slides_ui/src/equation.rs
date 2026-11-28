//! Insert ▸ Equation in Slides: Docs' equation popover
//! ([`docs_ui::EquationDialog`]); the equation goes in a new text box, or
//! replaces the one in the shape double-clicked.

use docs_ui::{EquationDialog, EquationEvent};
use gpui_kit::*;
use kantor_ui::i18n::t;
use slides_engine::group::ShapeRef;

use crate::SlidesView;

pub(crate) struct SlidesEquation {
    dialog: Entity<EquationDialog>,
    /// The shape whose equation is changed (`None`: a new text box).
    target: Option<ShapeRef>,
    _sub: Subscription,
}

impl SlidesView {
    /// Insert ▸ Equation, or edit the selected shape's equation.
    pub fn insert_equation_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let target = self
            .sel_ref()
            .filter(|r| self.pres.equation_in(*r).is_some());
        let existing = target
            .and_then(|r| self.pres.equation_in(r))
            .map(|(_, _, e)| e);
        let at = point(window.viewport_size().width / 2., px(96.));
        let bridge = self.painter.bridge.clone();
        let dialog =
            cx.new(|cx| EquationDialog::new(existing.as_ref(), true, at, bridge, window, cx));
        let sub = cx.subscribe_in(
            &dialog,
            window,
            |this, _, ev: &EquationEvent, window, cx| {
                let Some(open) = this.equation.take() else {
                    return;
                };
                if let EquationEvent::Done(eq) = ev {
                    match open.target {
                        Some(r) => {
                            let res = this.pres.set_equation(r, eq);
                            if this.report(res, cx).is_some() {
                                this.after_edit(window, cx);
                            }
                        }
                        None => {
                            let res = this.pres.add_equation(this.current, eq);
                            this.inserted(res, false, window, cx);
                        }
                    }
                    this.status =
                        t("Equation saved as an Office equation (editable in Word)").into();
                }
                cx.notify();
            },
        );
        self.equation = Some(SlidesEquation {
            dialog,
            target,
            _sub: sub,
        });
        cx.notify();
    }

    pub(crate) fn render_equation(&self) -> Option<Entity<EquationDialog>> {
        self.equation.as_ref().map(|e| e.dialog.clone())
    }
}
