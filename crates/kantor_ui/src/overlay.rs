//! Layers that keep what lies under a popover or a dialog card from reacting
//! to the pointer.
//!
//! GPUI gives an element its `.hover()` style, tooltip and clicks whenever
//! the pointer is over it and no hitbox drawn above it occludes it. Kantor's
//! popovers and Sheets' dialog cards occluded only their own box, so the
//! grid row, list item or button beside them kept its hover highlight when
//! they opened and lit up again as the pointer moved, as if nothing were
//! open. Menus and dialogs in Excel, Word and PowerPoint are modal: these
//! layers occlude everything else while one is open.

use gpui_kit::*;

/// A transparent layer over its positioned parent that takes hover and
/// clicks: put a dialog card in it and nothing around the card reacts.
pub fn backdrop() -> Div {
    div().absolute().inset_0().occlude()
}

/// A popover's deferred layer: `layer` (its `anchored` panel) above a
/// transparent shield over the whole window. A click outside the panel lands
/// on the shield, so it only closes the popover (through the panel's
/// `on_mouse_down_out`), as a click outside a menu does.
pub fn popover(layer: impl IntoElement) -> Deferred {
    deferred(div().child(shield()).child(layer))
}

/// Takes hover and clicks over the whole window, below whatever is drawn
/// after it in the same layer (deferred draws keep the window's whole
/// content mask).
pub fn shield() -> impl IntoElement {
    canvas(
        |_, window, _| {
            let all = Bounds::new(point(px(0.), px(0.)), window.viewport_size());
            window.insert_hitbox(all, HitboxBehavior::BlockMouse);
        },
        |_, _, _, _| {},
    )
}
