//! KANTOR PATCH (row heights): the table body as a list of rows of their own
//! heights, in place of upstream's `uniform_list`.
//!
//! It works like `uniform_list` (only the visible rows are built, scrolling
//! goes through the same [`UniformListScrollHandle`], so the scrollbar, the
//! scroll mask and the bounce keep working), but rows are placed from a
//! [`RowHeights`] (prefix sums in device pixels): finding the rows on screen
//! and where they go is O(visible · log n), whatever the sheet's size.
//! Hidden rows (height 0) are skipped without being built.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ContentMask, DeferredScrollToItem, Div, Element,
    ElementId, GlobalElementId, Hitbox, InspectorElementId, InteractiveElement, Interactivity,
    IntoElement, LayoutId, Pixels, ScrollStrategy, Size, Stateful, StatefulInteractiveElement as _,
    StyleRefinement, Styled, UniformListScrollHandle, Window, div, point, px, size,
};

use crate::row_heights::RowHeights;

type RenderRows = Box<dyn Fn(Vec<usize>, &mut Window, &mut App) -> Vec<AnyElement>>;

/// A vertical list of `count` rows placed by `heights`.
pub(crate) fn row_list(
    id: impl Into<ElementId>,
    heights: Rc<RefCell<RowHeights>>,
    count: usize,
    render: impl Fn(Vec<usize>, &mut Window, &mut App) -> Vec<AnyElement> + 'static,
) -> RowList {
    let id = id.into();
    RowList {
        id: id.clone(),
        heights,
        count,
        render: Box::new(render),
        base: div().id(id).overflow_y_scroll(),
        scroll_handle: None,
    }
}

pub(crate) struct RowList {
    id: ElementId,
    heights: Rc<RefCell<RowHeights>>,
    count: usize,
    render: RenderRows,
    /// Scrolls (the wheel goes to its interactivity) and tracks the handle.
    base: Stateful<Div>,
    scroll_handle: Option<UniformListScrollHandle>,
}

impl RowList {
    pub(crate) fn track_scroll(mut self, handle: &UniformListScrollHandle) -> Self {
        self.base = self.base.track_scroll(&handle.0.borrow().base_handle);
        self.scroll_handle = Some(handle.clone());
        self
    }
}

/// Where the list's top goes (logical px from the first row) to carry out a
/// deferred scroll, from `scroll_top` in a viewport `viewport` high; the same
/// rules as `uniform_list` (`Nearest` and non-strict requests leave a fully
/// visible row alone).
pub(crate) fn scroll_target(
    heights: &RowHeights,
    count: usize,
    deferred: DeferredScrollToItem,
    scroll_top: f64,
    viewport: f64,
) -> f64 {
    let count = count.min(heights.len());
    if count == 0 {
        return 0.0;
    }
    let ix = deferred.item_index.min(count - 1);
    let item_top = heights.top(ix);
    let item_bottom = item_top + heights.height(ix);
    let max_top = (heights.top(count) - viewport).max(0.0);
    let is_above = item_top < scroll_top;
    let is_below = item_bottom > scroll_top + viewport;
    if !(deferred.scroll_strict || is_above || is_below) {
        return scroll_top;
    }
    let strategy = match deferred.strategy {
        ScrollStrategy::Nearest if is_above => ScrollStrategy::Top,
        ScrollStrategy::Nearest if is_below => ScrollStrategy::Bottom,
        s => s,
    };
    let top = match strategy {
        ScrollStrategy::Top => item_top,
        ScrollStrategy::Center => (item_top + item_bottom) / 2.0 - viewport / 2.0,
        ScrollStrategy::Bottom => item_bottom - viewport,
        ScrollStrategy::Nearest => scroll_top,
    };
    top.clamp(0.0, max_top)
}

pub(crate) struct RowListFrame {
    items: Vec<AnyElement>,
}

impl Styled for RowList {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for RowList {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl IntoElement for RowList {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for RowList {
    type RequestLayoutState = RowListFrame;
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let layout_id = self.base.interactivity().request_layout(
            global_id,
            inspector_id,
            window,
            cx,
            |style, window, cx| {
                window.with_text_style(style.text_style().cloned(), |window| {
                    window.request_layout(style, None, cx)
                })
            },
        );
        (layout_id, RowListFrame { items: Vec::new() })
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        frame: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Hitbox> {
        let count = self.count.min(self.heights.borrow().len());
        let total = self.heights.borrow().top(count);
        let content_size = Size {
            width: bounds.size.width,
            height: px(total as f32),
        };
        let deferred = self.scroll_handle.as_ref().and_then(|handle| {
            let mut handle = handle.0.borrow_mut();
            handle.last_item_size = Some(gpui::ItemSize {
                item: bounds.size,
                contents: content_size,
            });
            handle.deferred_scroll_to_item.take()
        });
        let base_handle = self
            .scroll_handle
            .as_ref()
            .map(|h| h.0.borrow().base_handle.clone());
        // Clamp the offset (and carry out a pending scroll) before the
        // interactivity reads it.
        let viewport = f64::from(f32::from(bounds.size.height));
        let max_top = (total - viewport).max(0.0);
        if let Some(handle) = &base_handle {
            let offset = handle.offset();
            let mut top = f64::from(-f32::from(offset.y)).clamp(0.0, max_top);
            if let Some(deferred) = deferred {
                top = scroll_target(&self.heights.borrow(), count, deferred, top, viewport);
            }
            if f64::from(-f32::from(offset.y)) != top {
                handle.set_offset(point(offset.x, px(-(top as f32))));
            }
        }
        let heights = self.heights.clone();
        let render = &self.render;

        self.base.interactivity().prepaint(
            global_id,
            inspector_id,
            bounds,
            content_size,
            window,
            cx,
            |_style, scroll_offset, hitbox, window, cx| {
                if count == 0 {
                    return hitbox;
                }
                let top = f64::from(-f32::from(scroll_offset.y)).clamp(0.0, max_top);

                // The rows on screen and where they go, then build them
                // (with the heights released: cells read them too).
                let placed: Vec<(usize, f32, f32)> = {
                    let h = heights.borrow();
                    let first = h.to_dp(top);
                    let last = h.to_dp(top + viewport) + 1;
                    h.shown_in(first, last)
                        .into_iter()
                        .filter(|&row| row < count)
                        .map(|row| (row, (h.top(row) - top) as f32, h.height(row) as f32))
                        .collect()
                };
                let rows: Vec<usize> = placed.iter().map(|p| p.0).collect();
                let items = render(rows, window, cx);

                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    for (mut item, (_, y, height)) in items.into_iter().zip(placed) {
                        let origin = bounds.origin + point(scroll_offset.x, px(y));
                        let space = size(
                            AvailableSpace::Definite(bounds.size.width),
                            AvailableSpace::Definite(px(height)),
                        );
                        item.layout_as_root(space, window, cx);
                        item.prepaint_at(origin, window, cx);
                        frame.items.push(item);
                    }
                });
                hitbox
            },
        )
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        frame: &mut Self::RequestLayoutState,
        hitbox: &mut Option<Hitbox>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.base.interactivity().paint(
            global_id,
            inspector_id,
            bounds,
            hitbox.as_ref(),
            window,
            cx,
            |_, window, cx| {
                for item in &mut frame.items {
                    item.paint(window, cx);
                }
            },
        )
    }
}

/// The rows of `heights` shown in a viewport `viewport` high from `top`.
pub(crate) fn visible_rows(heights: &RowHeights, top: f64, viewport: f64) -> Range<usize> {
    let rows = heights.shown_in(heights.to_dp(top), heights.to_dp(top + viewport) + 1);
    match (rows.first(), rows.last()) {
        (Some(&a), Some(&b)) => a..b + 1,
        _ => 0..0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deferred(ix: usize, strategy: ScrollStrategy) -> DeferredScrollToItem {
        DeferredScrollToItem {
            item_index: ix,
            strategy,
            offset: 0,
            scroll_strict: false,
        }
    }

    #[std::prelude::v1::test]
    fn scrolling_to_a_row_uses_its_height() {
        // 100 rows of 30 px, row 50 is 300 px tall.
        let mut h = vec![30u32; 100];
        h[50] = 300;
        let rows = RowHeights::from_heights(h, 1.0);
        let view = 200.0;
        // Visible: no scroll.
        assert_eq!(
            scroll_target(&rows, 100, deferred(3, ScrollStrategy::Nearest), 0.0, view),
            0.0
        );
        // Below: its bottom at the viewport's bottom.
        assert_eq!(
            scroll_target(&rows, 100, deferred(51, ScrollStrategy::Nearest), 0.0, view),
            30.0 * 50.0 + 300.0 + 30.0 - view
        );
        // Above: its top at the top.
        assert_eq!(
            scroll_target(
                &rows,
                100,
                deferred(10, ScrollStrategy::Nearest),
                2000.0,
                view
            ),
            300.0
        );
        // Centred on the tall row.
        assert_eq!(
            scroll_target(&rows, 100, deferred(50, ScrollStrategy::Center), 0.0, view),
            1500.0 + 150.0 - 100.0
        );
        // Never past the end, whatever the index.
        let end = rows.total() - view;
        assert_eq!(
            scroll_target(
                &rows,
                100,
                deferred(usize::MAX, ScrollStrategy::Top),
                0.0,
                view
            ),
            end
        );
    }

    #[std::prelude::v1::test]
    fn visible_rows_follow_heights() {
        let mut h = vec![30u32; 100];
        h[2] = 0; // hidden
        h[4] = 90;
        let rows = RowHeights::from_heights(h, 1.0);
        // 0..60: rows 0, 1 (row 2 hidden), 60..90: row 3, 90..180: row 4.
        assert_eq!(visible_rows(&rows, 0.0, 100.0), 0..5);
        assert_eq!(visible_rows(&rows, 95.0, 50.0), 4..5);
        assert_eq!(visible_rows(&rows, 175.0, 10.0), 4..6);
    }
}
