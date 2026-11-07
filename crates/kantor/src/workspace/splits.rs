//! Rendering a tab's split tree (Voltrus Tools `views/splits.rs` pattern):
//! first child sized by ratio, a 1 px divider with a wider invisible hit area,
//! the second child takes the rest. Each split records its bounds at paint time
//! so divider drags can convert pixels to ratios.

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::layout::{Axis, Branch, Layout, PaneId, Path as LayoutPath};
use kantor_ui::tokens;

use super::{Drag, Workspace};

/// Invisible hit area around a 1 px divider.
const DIVIDER_HIT: Pixels = px(7.);

impl Workspace {
    pub(super) fn render_layout_root(&mut self, ti: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(tab) = self.tabs.get(ti) else {
            return div().into_any_element();
        };
        let layout = tab.layout.clone();
        let focused = tab.focused;
        let multi = layout.panes().len() > 1;
        self.split_bounds.borrow_mut().clear();
        self.render_node(&layout, Vec::new(), focused, multi, cx)
    }

    fn render_node(
        &self,
        node: &Layout,
        path: LayoutPath,
        focused: PaneId,
        multi: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match node {
            Layout::Leaf(pane) => self.render_pane(*pane, focused, multi, cx),
            Layout::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let t = tokens(cx);
                let (axis, ratio) = (*axis, *ratio);
                let mut first_path = path.clone();
                first_path.push(Branch::First);
                let mut second_path = path.clone();
                second_path.push(Branch::Second);
                let first_el = self.render_node(first, first_path, focused, multi, cx);
                let second_el = self.render_node(second, second_path, focused, multi, cx);

                let record = {
                    let map = self.split_bounds.clone();
                    let path = path.clone();
                    canvas(
                        move |bounds, _, _| {
                            map.borrow_mut().insert(path, bounds);
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0()
                };

                let drag_path = path.clone();
                let hit = div()
                    .id(SharedString::from(format!("divider-{path:?}")))
                    .absolute()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |ws, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            ws.start_drag(
                                Drag::Split {
                                    path: drag_path.clone(),
                                    axis,
                                },
                                cx,
                            );
                        }),
                    );
                match axis {
                    Axis::Horizontal => h_flex()
                        .relative()
                        .size_full()
                        .child(record)
                        .child(
                            div()
                                .flex_none()
                                .h_full()
                                .w(relative(ratio))
                                .child(first_el),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_none()
                                .w(px(1.))
                                .h_full()
                                .bg(t.border)
                                .child(
                                    hit.top_0()
                                        .bottom_0()
                                        .left(-(DIVIDER_HIT - px(1.)) / 2.)
                                        .w(DIVIDER_HIT)
                                        .cursor(CursorStyle::ResizeLeftRight),
                                ),
                        )
                        .child(div().flex_1().min_w_0().h_full().child(second_el))
                        .into_any_element(),
                    Axis::Vertical => v_flex()
                        .relative()
                        .size_full()
                        .child(record)
                        .child(
                            div()
                                .flex_none()
                                .w_full()
                                .h(relative(ratio))
                                .child(first_el),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_none()
                                .h(px(1.))
                                .w_full()
                                .bg(t.border)
                                .child(
                                    hit.left_0()
                                        .right_0()
                                        .top(-(DIVIDER_HIT - px(1.)) / 2.)
                                        .h(DIVIDER_HIT)
                                        .cursor(CursorStyle::ResizeUpDown),
                                ),
                        )
                        .child(div().flex_1().min_h_0().w_full().child(second_el))
                        .into_any_element(),
                }
            }
        }
    }

    fn render_pane(
        &self,
        pane: PaneId,
        focused: PaneId,
        multi: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = tokens(cx);
        let Some(doc) = self.docs.get(&pane) else {
            return div().size_full().into_any_element();
        };
        div()
            .id(("pane", pane as usize))
            .relative()
            .size_full()
            .overflow_hidden()
            .capture_any_mouse_down(cx.listener(move |ws, _: &MouseDownEvent, window, cx| {
                ws.focus_pane(pane, window, cx);
            }))
            .child(doc.view.clone().cached(div().size_full().style().clone()))
            // In a split, the focused pane gets an accent edge.
            .when(multi && pane == focused, |d| {
                d.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(px(2.))
                        .bg(t.accent),
                )
            })
            .into_any_element()
    }
}
