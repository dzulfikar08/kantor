//! Group, Ungroup, Regroup, Convert to Shapes and Add Shape After /
//! Before (SmartArt), from the
//! keyboard (Ctrl+G, Ctrl+Shift+G), the palette and the canvas's
//! right-click menu, as in PowerPoint. Selecting shapes inside groups
//! (the second click) is in the view's mouse handling.

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use kantor_ui::i18n;
use slides_engine::{AddPlace, Inner, ShapeRef};

use crate::SlidesView;

/// Messages slides_engine gives for SmartArt it can't change (shown in
/// the status bar, translated).
#[allow(dead_code)] // keys for the catalog scanner (shown through `report`)
pub(crate) const ENGINE_MESSAGES: &[&str] = &[
    i18n::tk(
        "Kantor can't lay out this SmartArt layout yet: items can only be added or removed in lists, processes and cycles",
    ),
    i18n::tk("This SmartArt layout has one level: add shapes before or after"),
    i18n::tk("A SmartArt keeps at least one item"),
];

impl SlidesView {
    /// Whether Group applies: two or more slide shapes selected.
    pub fn can_group(&self) -> bool {
        self.editing.is_none() && self.inner.is_top() && self.selection().len() >= 2
    }

    /// Whether Ungroup applies: a group (or SmartArt) selected.
    pub fn can_ungroup(&self) -> bool {
        self.also.is_empty()
            && self.editing.is_none()
            && self
                .sel_ref()
                .and_then(|r| self.pres.shape(r))
                .is_some_and(|s| s.is_group() || s.smartart().is_some())
    }

    pub fn can_regroup(&self) -> bool {
        !self.pres.is_empty() && self.pres.can_regroup(self.current)
    }

    /// Whether Convert to Shapes applies: SmartArt selected.
    pub fn can_convert_to_shapes(&self) -> bool {
        self.also.is_empty()
            && self.editing.is_none()
            && self.inner.is_top()
            && self
                .sel_ref()
                .and_then(|r| self.pres.shape(r))
                .is_some_and(|s| s.smartart().is_some())
    }

    /// Whether Add Shape After / Before applies: a SmartArt (or one of its
    /// shapes) selected whose layout Kantor lays out.
    pub fn can_add_smartart_shape(&self) -> bool {
        self.also.is_empty()
            && self.editing.is_none()
            && self.sel_ref().is_some_and(|r| {
                (self.pres.in_smartart(r)
                    || self.pres.shape(r).is_some_and(|s| s.smartart().is_some()))
                    && self.pres.smartart_family(r).is_some()
            })
    }

    /// PowerPoint's Add Shape After / Before on the selected SmartArt shape.
    pub fn smartart_add_doc(
        &mut self,
        place: AddPlace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(r) = self.sel_ref() else {
            self.set_status(i18n::t("Select a SmartArt shape"), cx);
            return;
        };
        let res = self.pres.smartart_add_shape(r, place);
        if let Some(new) = self.report(res, cx) {
            self.select_at(Some(new), cx);
            self.after_edit(window, cx);
        }
    }

    /// Ctrl+G: group the selected shapes.
    pub fn group_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_group() {
            self.set_status(i18n::t("Select two or more shapes to group them"), cx);
            return;
        }
        let res = self.pres.group_shapes(self.current, &self.selection());
        if let Some(g) = self.report(res, cx) {
            self.select(Some(g), cx);
            self.after_edit(window, cx);
        }
    }

    /// Ctrl+Shift+G: ungroup the selected group (SmartArt is converted to
    /// shapes first, as PowerPoint does); its shapes stay selected.
    pub fn ungroup_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.sel_ref().filter(|_| self.can_ungroup()) else {
            self.set_status(i18n::t("Select a group to ungroup it"), cx);
            return;
        };
        let res = self.pres.ungroup(r);
        if let Some(out) = self.report(res, cx) {
            self.select_refs(&out, cx);
            self.after_edit(window, cx);
        }
    }

    /// Put the last ungrouped group back together.
    pub fn regroup_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pres.is_empty() {
            return;
        }
        let res = self.pres.regroup(self.current);
        if let Some(g) = self.report(res, cx) {
            self.select(Some(g), cx);
            self.after_edit(window, cx);
        }
    }

    /// SmartArt to a group of ordinary shapes.
    pub fn convert_to_shapes_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.sel_ref().filter(|_| self.can_convert_to_shapes()) else {
            self.set_status(i18n::t("Select a SmartArt graphic to convert it"), cx);
            return;
        };
        let res = self.pres.convert_to_shapes(r);
        if let Some(g) = self.report(res, cx) {
            self.select(Some(g), cx);
            self.after_edit(window, cx);
        }
    }

    /// Select shapes that came out of a group: several slide shapes, or
    /// the first of them inside a group.
    fn select_refs(&mut self, refs: &[ShapeRef], cx: &mut Context<Self>) {
        match refs.first() {
            Some(first) if first.is_top() => {
                self.select(Some(first.1), cx);
                self.also = refs.iter().skip(1).map(|r| r.1).collect();
            }
            Some(first) => self.select_at(Some(*first), cx),
            None => self.select(None, cx),
        }
    }

    /// Right mouse button on a shape while not editing text: select it,
    /// unless it's already selected (so the menu acts on the selection).
    pub(crate) fn right_click_select(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(i) = self.shape_at(p) else {
            return;
        };
        if self.selection().contains(&i) {
            return;
        }
        self.select_at(Some(ShapeRef(self.current, i, Inner::TOP)), cx);
    }
}

/// Group items for the canvas's right-click menu (only those that apply).
pub(crate) fn menu_items(
    mut menu: PopupMenu,
    view: &WeakEntity<SlidesView>,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(v) = view.upgrade() else {
        return menu;
    };
    let v = v.read(cx);
    type Run = fn(&mut SlidesView, &mut Window, &mut Context<SlidesView>);
    let items: [(bool, &str, Box<dyn Action>, Run); 6] = [
        (
            v.can_add_smartart_shape(),
            i18n::t("Add Shape After"),
            Box::new(crate::SmartArtAddAfter),
            |v, w, cx| v.smartart_add_doc(AddPlace::After, w, cx),
        ),
        (
            v.can_add_smartart_shape(),
            i18n::t("Add Shape Before"),
            Box::new(crate::SmartArtAddBefore),
            |v, w, cx| v.smartart_add_doc(AddPlace::Before, w, cx),
        ),
        (
            v.can_group(),
            i18n::t("Group"),
            Box::new(crate::GroupShapes),
            SlidesView::group_doc,
        ),
        (
            v.can_ungroup(),
            i18n::t("Ungroup"),
            Box::new(crate::UngroupShapes),
            SlidesView::ungroup_doc,
        ),
        (
            v.can_regroup() && v.editing.is_none(),
            i18n::t("Regroup"),
            Box::new(crate::RegroupShapes),
            SlidesView::regroup_doc,
        ),
        (
            v.can_convert_to_shapes(),
            i18n::t("Convert to Shapes"),
            Box::new(crate::ConvertToShapes),
            SlidesView::convert_to_shapes_doc,
        ),
    ];
    let shown: Vec<_> = items.into_iter().filter(|i| i.0).collect();
    if shown.is_empty() {
        return menu;
    }
    menu = menu.separator();
    for (_, label, action, run) in shown {
        let v = view.clone();
        menu = menu.item(PopupMenuItem::new(label).action(action).on_click(
            move |_, window, cx| {
                let _ = v.update(cx, |v, cx| run(v, window, cx));
            },
        ));
    }
    menu
}
