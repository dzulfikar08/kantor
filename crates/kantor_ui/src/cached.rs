//! Cached chrome views (the main performance pattern from Voltrus Tools).
//!
//! A `CachedView` renders one region of a model (tab strip, status bar, …) and is
//! drawn with `Entity::cached`, so it replays its last frame unless the model
//! notified since. Editing a cell then redraws the grid, not the whole window.

use gpui_kit::*;

type RenderFn<M> = Box<dyn Fn(&mut M, &mut Window, &mut Context<M>) -> AnyElement>;

pub struct CachedView<M: 'static> {
    model: WeakEntity<M>,
    render: RenderFn<M>,
    _model_changes: Subscription,
}

impl<M: 'static> CachedView<M> {
    /// Create a view rendering `render(model)`; it re-renders whenever `model` notifies.
    pub fn new(
        cx: &mut Context<M>,
        render: impl Fn(&mut M, &mut Window, &mut Context<M>) -> AnyElement + 'static,
    ) -> Entity<Self> {
        let model = cx.entity();
        cx.new(|cx| Self {
            model: model.downgrade(),
            render: Box::new(render),
            _model_changes: cx.observe(&model, |_, _, cx| cx.notify()),
        })
    }
}

impl<M: 'static> Render for CachedView<M> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _span = crate::perf::span("chrome region");
        let render = &self.render;
        self.model
            .update(cx, |model, cx| render(model, window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}
