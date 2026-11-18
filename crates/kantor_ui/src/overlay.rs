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
//!
//! gpui-kit's own menus (a right-click menu, a drop-down button's menu) draw
//! no such layer and Kantor doesn't build them itself, so the window's root
//! view draws [`menu_shield`] under them while one has the focus.

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

/// Whether a gpui-kit menu (`PopupMenu`: a context menu, a drop-down
/// button's menu or one of their submenus) has the focus in the frame last
/// drawn. Such a menu takes the focus when it opens and gives it back when
/// it closes, so a menu that is gone (closed, or never drawn again) never
/// counts.
pub fn menu_open(window: &Window) -> bool {
    window
        .context_stack()
        .iter()
        .any(|c| c.contains(POPUP_MENU_CONTEXT))
}

/// gpui-component's key context for `PopupMenu`.
const POPUP_MENU_CONTEXT: &str = "PopupMenu";

/// A [`shield`] just under gpui-kit's menus (which draw at
/// `POPUP_PRIORITY`) while one is open ([`menu_open`]), so the grid row,
/// cell or button under the pointer beside the menu keeps no hover
/// highlight; a click outside the menu only closes it. Render it in the
/// window's root view, which draws every frame, and redraw once more when
/// the focus moves (the check reads the last frame's focus path).
pub fn menu_shield(window: &Window) -> Option<Deferred> {
    menu_open(window).then(|| deferred(shield()).with_priority(gpui_kit::base::POPUP_PRIORITY - 1))
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
