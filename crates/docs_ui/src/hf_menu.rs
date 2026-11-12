//! Headers and footers: the toolbar's header/footer button and its popover
//! (the caret's section): "Different first page", "Different odd and even
//! pages", and the text of each header or footer the section uses, with
//! buttons that insert the page number (`{PAGE}`) and page count
//! (`{PAGES}`) and alignment. Apply writes each changed one as one undo
//! step. Double-clicking in a page's top or bottom margin opens it too.
//!
//! The text is plain: a header with pictures, tables or other objects is
//! shown on the page and kept as it is, but can't be edited here yet.

use std::time::{Duration, Instant};

use docs_engine::{Align, HfKind, HfType};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, tooltip::Tooltip, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::tokens::Elevation;
use kantor_ui::{components::section_label, metrics::*, tokens};

use crate::DocsView;

/// One header or footer being edited.
struct Field {
    ty: HfType,
    input: Entity<TextareaState>,
    align: Option<Align>,
    /// Why it can't be edited, if so.
    locked: Option<String>,
}

/// The open popover.
pub(crate) struct HfMenu {
    at: Point<Pixels>,
    kind: HfKind,
    section: usize,
    fields: Vec<Field>,
    _subs: Vec<Subscription>,
}

fn type_label(ty: HfType, first: bool, even: bool) -> &'static str {
    match ty {
        HfType::First => "First page",
        HfType::Even => "Even pages",
        HfType::Default if even => "Odd pages",
        HfType::Default if first => "Other pages",
        HfType::Default => "Every page",
    }
}

impl DocsView {
    /// Open the header/footer popover (or close it when open).
    pub fn toggle_hf_menu(
        &mut self,
        kind: HfKind,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.hf_menu.take().is_some()
            || self
                .hf_menu_closed
                .is_some_and(|t| t.elapsed() < Duration::from_millis(250))
        {
            self.hf_menu_closed = None;
            cx.notify();
            return;
        }
        self.open_hf_menu(kind, at, window, cx);
    }

    /// Open the popover on `kind` for the caret's section.
    pub fn open_hf_menu(
        &mut self,
        kind: HfKind,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = at
            .or(self.hf_menu.as_ref().map(|m| m.at))
            .unwrap_or_else(|| {
                let b = self.viewport.get();
                point(b.origin.x + b.size.width - SPACE_4, b.origin.y + SPACE_2)
            });
        let section = self.doc.section_of(self.sel.head.para);
        let props = &self.doc.sections()[section].props;
        let mut types = Vec::new();
        if props.title_page {
            types.push(HfType::First);
        }
        types.push(HfType::Default);
        if self.doc.even_odd_headers() {
            types.push(HfType::Even);
        }
        let mut subs = Vec::new();
        let fields = types
            .into_iter()
            .map(|ty| {
                let (text, align) = self.doc.hf_text(section, kind, ty);
                let locked = self.doc.hf_locked(section, kind, ty);
                let input = cx.new(|cx| {
                    let mut s = TextareaState::new(window, cx)
                        .auto_grow(1, 4)
                        .placeholder(format!("No {}", kind.label().to_lowercase()));
                    s.set_value(text, window, cx);
                    s
                });
                subs.push(cx.subscribe_in(
                    &input,
                    window,
                    |this, _, ev: &InputEvent, window, cx| {
                        if let InputEvent::PressEnter {
                            secondary: true, ..
                        } = ev
                        {
                            this.apply_hf(window, cx);
                        }
                    },
                ));
                Field {
                    ty,
                    input,
                    align,
                    locked,
                }
            })
            .collect();
        self.hf_menu = Some(HfMenu {
            at,
            kind,
            section,
            fields,
            _subs: subs,
        });
        cx.notify();
    }

    /// Write every changed field (one undo step each).
    fn apply_hf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &self.hf_menu else { return };
        let (kind, section) = (menu.kind, menu.section);
        let edits: Vec<(HfType, String, Option<Align>)> = menu
            .fields
            .iter()
            .filter(|f| f.locked.is_none())
            .map(|f| (f.ty, f.input.read(cx).value().to_string(), f.align))
            .collect();
        let mut changed = false;
        for (ty, text, align) in edits {
            let (old, old_align) = self.doc.hf_text(section, kind, ty);
            if old == text && (align.is_none() || align == old_align) {
                continue;
            }
            changed |= self
                .doc
                .set_hf_text(section, kind, ty, &text, align, self.sel);
        }
        if changed {
            let sel = self.sel;
            self.edited(sel, window, cx);
            self.status = format!("{} updated", kind.label()).into();
        }
        self.hf_menu = None;
        cx.notify();
    }

    fn set_title_page_doc(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &self.hf_menu else { return };
        let (kind, section) = (menu.kind, menu.section);
        self.doc.set_title_page(section, on, self.sel);
        let sel = self.sel;
        self.edited(sel, window, cx);
        self.hf_menu = None;
        self.open_hf_menu(kind, None, window, cx);
    }

    fn set_even_odd_doc(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let kind = self.hf_menu.as_ref().map_or(HfKind::Header, |m| m.kind);
        self.doc.set_even_odd_headers(on, self.sel);
        let sel = self.sel;
        self.edited(sel, window, cx);
        self.hf_menu = None;
        self.open_hf_menu(kind, None, window, cx);
    }

    /// Footer "Page N of M", centred, on every page of the caret's
    /// section (palette: Add Page Numbers).
    pub fn add_page_numbers_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let section = self.doc.section_of(self.sel.head.para);
        let (text, _) = self.doc.hf_text(section, HfKind::Footer, HfType::Default);
        if text.contains("{PAGE}") {
            self.status = "This section's footer already has page numbers".into();
            cx.notify();
            return;
        }
        let new = if text.trim().is_empty() {
            "Page {PAGE} of {PAGES}".to_owned()
        } else {
            format!("{text}\nPage {{PAGE}} of {{PAGES}}")
        };
        if self.doc.set_hf_text(
            section,
            HfKind::Footer,
            HfType::Default,
            &new,
            Some(Align::Center),
            self.sel,
        ) {
            let sel = self.sel;
            self.edited(sel, window, cx);
            self.status = "Page numbers added to the footer".into();
        } else {
            self.status = self
                .doc
                .hf_locked(section, HfKind::Footer, HfType::Default)
                .unwrap_or_default()
                .into();
        }
        cx.notify();
    }

    /// A page number field at the caret (body text).
    pub fn insert_page_number_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self
            .doc
            .insert_page_field(self.sel, docs_engine::PageField::Page);
        self.edited(sel, window, cx);
    }

    /// Section break (next page) at the caret.
    pub fn insert_section_break_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.doc.insert_section_break(self.sel) {
            Some(sel) => {
                self.edited(sel, window, cx);
                self.status =
                    "Section break inserted: Page setup now changes this section only".into();
            }
            None => self.status = "A section break can't go inside a table".into(),
        }
        cx.notify();
    }

    pub(crate) fn render_hf_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let menu = self.hf_menu.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let sections = self.doc.sections();
        let props = &sections[menu.section.min(sections.len() - 1)].props;
        let (first, even) = (props.title_page, self.doc.even_odd_headers());

        let check =
            |id: &'static str,
             label: &'static str,
             on: bool,
             apply: fn(&mut DocsView, bool, &mut Window, &mut Context<DocsView>)| {
                let v = view.clone();
                h_flex()
                    .id(id)
                    .h(CONTROL_MD)
                    .px(SPACE_4)
                    .gap(SPACE_4)
                    .rounded(RADIUS_MD)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .child(
                        div()
                            .flex_none()
                            .size(ICON_MD)
                            .when(on, |d| d.child(Icon::new(Lucide::Check).with_size(ICON_MD))),
                    )
                    .child(div().text_color(t.fg).child(label))
                    .on_click(move |_, window, cx| {
                        let _ = v.update(cx, |this, cx| apply(this, !on, window, cx));
                    })
            };
        let kind_tab = |k: HfKind| {
            let v = view.clone();
            let on = menu.kind == k;
            div()
                .id(k.label())
                .flex_1()
                .h(CONTROL_SM)
                .flex()
                .items_center()
                .justify_center()
                .rounded(RADIUS_SM)
                .cursor_pointer()
                .when(on, |d| d.bg(t.selected).text_color(t.fg))
                .when(!on, |d| d.text_color(t.fg_muted).hover(|s| s.bg(t.hover)))
                .child(k.label())
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| {
                        this.hf_menu = None;
                        this.open_hf_menu(k, None, window, cx);
                    });
                })
        };
        let tabs = h_flex()
            .mx(SPACE_4)
            .p(SPACE_1)
            .gap(SPACE_1)
            .rounded(RADIUS_MD)
            .bg(t.surface)
            .border_1()
            .border_color(t.border)
            .child(kind_tab(HfKind::Header))
            .child(kind_tab(HfKind::Footer));

        let mut fields = v_flex().gap(SPACE_3).px(SPACE_4);
        for (i, f) in menu.fields.iter().enumerate() {
            let label = type_label(f.ty, first, even);
            let mut col = v_flex()
                .gap(SPACE_1)
                .child(div().text_color(t.fg_muted).child(label));
            if let Some(why) = &f.locked {
                col = col.child(
                    div()
                        .p(SPACE_3)
                        .rounded(RADIUS_MD)
                        .bg(t.surface)
                        .text_color(t.fg_muted)
                        .child(why.clone()),
                );
                fields = fields.child(col);
                continue;
            }
            let token =
                |id: SharedString, text: &'static str, tip: &'static str, insert: &'static str| {
                    let input = f.input.clone();
                    div()
                        .id(id)
                        .h(CONTROL_SM)
                        .px(SPACE_3)
                        .flex()
                        .items_center()
                        .rounded(RADIUS_SM)
                        .border_1()
                        .border_color(t.border)
                        .text_color(t.fg_muted)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.hover).text_color(t.fg))
                        .child(text)
                        .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                        .on_click(move |_, window, cx| {
                            input.update(cx, |s, cx| s.insert(insert, window, cx));
                        })
                };
            let align_btn = |a: Align, icon: Lucide, tip: &'static str| {
                let v = view.clone();
                let on = f.align.unwrap_or(Align::Left) == a;
                div()
                    .id(SharedString::from(format!("hf-align-{i}-{tip}")))
                    .size(CONTROL_SM)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(RADIUS_SM)
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.selected).text_color(t.fg))
                    .when(!on, |d| d.text_color(t.fg_muted).hover(|s| s.bg(t.hover)))
                    .child(Icon::new(icon).with_size(ICON_SM))
                    .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                    .on_click(move |_, _, cx| {
                        let _ = v.update(cx, |this, cx| {
                            if let Some(m) = &mut this.hf_menu
                                && let Some(field) = m.fields.get_mut(i)
                            {
                                field.align = Some(a);
                            }
                            cx.notify();
                        });
                    })
            };
            col = col.child(Textarea::new(&f.input).small()).child(
                h_flex()
                    .gap(SPACE_2)
                    .child(token(
                        format!("hf-page-{i}").into(),
                        "Page number",
                        "Insert the page number: {PAGE}",
                        "{PAGE}",
                    ))
                    .child(token(
                        format!("hf-pages-{i}").into(),
                        "Page count",
                        "Insert the number of pages: {PAGES}",
                        "{PAGES}",
                    ))
                    .child(div().flex_1())
                    .child(align_btn(Align::Left, Lucide::TextAlignStart, "Left"))
                    .child(align_btn(Align::Center, Lucide::TextAlignCenter, "Centre"))
                    .child(align_btn(Align::Right, Lucide::TextAlignEnd, "Right")),
            );
            fields = fields.child(col);
        }
        let apply = {
            let v = view.clone();
            div()
                .id("docs-hf-apply")
                .flex_none()
                .h(CONTROL_SM)
                .px(SPACE_5)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .bg(t.accent)
                .text_color(t.accent_fg)
                .cursor_pointer()
                .child("Apply")
                .tooltip(|window, cx| Tooltip::new("Apply  Ctrl+Enter").build(window, cx))
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| this.apply_hf(window, cx));
                })
        };
        let note = (sections.len() > 1).then(|| {
            div().px(SPACE_4).text_color(t.fg_muted).child(format!(
                "Section {} of {}; sections without their own use the one before",
                menu.section + 1,
                sections.len()
            ))
        });

        let panel = v_flex()
            .id("docs-hf-menu")
            .occlude()
            .w(REM * 24.)
            .gap(SPACE_3)
            .p(SPACE_4)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .rounded(RADIUS_LG)
            .shadow(t.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .text_color(t.fg)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.hf_menu = None;
                this.hf_menu_closed = Some(Instant::now());
                cx.notify();
            }))
            .child(tabs)
            .children(note)
            .child(fields)
            .child(section_label("Options", cx).px(SPACE_4).pt(SPACE_2))
            .child(check(
                "hf-first",
                "Different first page",
                first,
                DocsView::set_title_page_doc,
            ))
            .child(check(
                "hf-even",
                "Different odd and even pages",
                even,
                DocsView::set_even_odd_doc,
            ))
            .child(h_flex().px(SPACE_4).child(div().flex_1()).child(apply));
        Some(deferred(
            anchored()
                .position(menu.at)
                .anchor(Anchor::TopRight)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel),
        ))
    }
}

/// The toolbar's header/footer button.
pub(crate) fn hf_button(view: &DocsView, cx: &mut Context<DocsView>) -> Stateful<Div> {
    let t = tokens(cx);
    let v = cx.entity().downgrade();
    let open = view.hf_menu.is_some();
    div()
        .id("docs-hf")
        .flex_none()
        .size(CONTROL_SM)
        .flex()
        .items_center()
        .justify_center()
        .rounded(RADIUS_MD)
        .cursor_pointer()
        .when(open, |d| d.bg(t.selected).text_color(t.fg))
        .when(!open, |d| {
            d.text_color(t.fg_muted)
                .hover(|s| s.bg(t.hover).text_color(t.fg))
        })
        .child(Icon::new(Lucide::PanelTop).with_size(ICON_MD))
        .tooltip(|window, cx| Tooltip::new("Header and footer, page numbers").build(window, cx))
        .on_click(move |ev, window, cx| {
            let p = ev.position();
            let at = point(p.x + SPACE_7, p.y + SPACE_7);
            let _ = v.update(cx, |this, cx| {
                this.toggle_hf_menu(HfKind::Header, Some(at), window, cx)
            });
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn labels_follow_the_options() {
        assert_eq!(type_label(HfType::Default, false, false), "Every page");
        assert_eq!(type_label(HfType::Default, true, false), "Other pages");
        assert_eq!(type_label(HfType::Default, true, true), "Odd pages");
        assert_eq!(type_label(HfType::Even, false, true), "Even pages");
    }
}
