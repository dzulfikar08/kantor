//! Headers and footers: the toolbar's header/footer button, and the
//! Header & Footer bar shown while one is edited on the page (Word's
//! Header & Footer tab): Different First Page, Different Odd & Even
//! Pages, Link to Previous, the header's distance from the top edge and
//! the footer's from the bottom, Page Number and Page Count fields, going
//! to the footer (header) and Close. Every option is one undo step.

use docs_engine::{HfKind, HfType, PageField};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, tooltip::Tooltip};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t, tf, tk};
use kantor_ui::{metrics::*, tokens};

use crate::DocsView;
use crate::page_menu::{format_cm, parse_cm};
use crate::part::PartTarget;

/// The Header & Footer bar's distance fields.
pub(crate) struct HfMenu {
    header: Entity<InputState>,
    footer: Entity<InputState>,
    _subs: Vec<Subscription>,
}

/// Word's name for the header or footer being edited, e.g. "First Page
/// Header".
pub(crate) fn part_label(kind: HfKind, ty: HfType, even_odd: bool) -> &'static str {
    match (kind, ty) {
        (HfKind::Header, HfType::First) => t("First Page Header"),
        (HfKind::Footer, HfType::First) => t("First Page Footer"),
        (HfKind::Header, HfType::Even) => t("Even Page Header"),
        (HfKind::Footer, HfType::Even) => t("Even Page Footer"),
        (HfKind::Header, HfType::Default) if even_odd => t("Odd Page Header"),
        (HfKind::Footer, HfType::Default) if even_odd => t("Odd Page Footer"),
        (HfKind::Header, HfType::Default) => t("Header"),
        (HfKind::Footer, HfType::Default) => t("Footer"),
    }
}

impl DocsView {
    /// Edit the header (or footer) of the caret's page, or stop editing it
    /// (toolbar button, palette).
    pub fn toggle_hf_menu(
        &mut self,
        kind: HfKind,
        _at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.part_target(), Some(PartTarget::Hf { .. })) {
            self.leave_part(window, cx);
            return;
        }
        self.open_hf_menu(kind, None, window, cx);
    }

    /// Start editing the caret page's header or footer.
    pub fn open_hf_menu(
        &mut self,
        kind: HfKind,
        _at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let page = match &self.part {
            Some(s) => s.page,
            None => self.caret_page(),
        };
        self.edit_hf_on_page(page, kind, None, window, cx);
        self.focus(window, cx);
    }

    /// The Header & Footer bar's fields, filled with the section's
    /// distances (made when editing starts).
    pub(crate) fn fill_hf_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(PartTarget::Hf { section, .. }) = self.part_target().cloned() else {
            self.hf_menu = None;
            return;
        };
        if self.hf_menu.is_none() {
            let header = cx.new(|cx| InputState::new(window, cx));
            let footer = cx.new(|cx| InputState::new(window, cx));
            let subs = [&header, &footer]
                .into_iter()
                .map(|i| {
                    cx.subscribe_in(i, window, |this, _, ev: &InputEvent, window, cx| match ev {
                        InputEvent::PressEnter { .. } => {
                            this.apply_hf_distances(window, cx);
                            this.focus(window, cx);
                        }
                        InputEvent::Blur => this.apply_hf_distances(window, cx),
                        _ => {}
                    })
                })
                .collect();
            self.hf_menu = Some(HfMenu {
                header,
                footer,
                _subs: subs,
            });
        }
        let props = self.main_doc().sections()[section].props.clone();
        if let Some(m) = &self.hf_menu {
            let (h, f) = (m.header.clone(), m.footer.clone());
            h.update(cx, |i, cx| {
                i.set_value(format_cm(props.header_dist), window, cx)
            });
            f.update(cx, |i, cx| {
                i.set_value(format_cm(props.footer_dist), window, cx)
            });
        }
    }

    /// Change a header/footer setting of the section being edited: leave
    /// the part (its edits become their own step), apply `f` to the main
    /// document, then edit the header or footer the page shows now.
    fn hf_option(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut docs_engine::Document, usize, HfKind, HfType, docs_engine::Selection),
    ) {
        let Some(PartTarget::Hf { section, kind, ty }) = self.part_target().cloned() else {
            return;
        };
        let page = self.part.as_ref().map_or(0, |s| s.page);
        self.leave_part(window, cx);
        let sel = self.sel;
        f(&mut self.doc, section, kind, ty, sel);
        self.edited(sel, window, cx);
        self.edit_hf_on_page(page, kind, None, window, cx);
        self.focus(window, cx);
    }

    fn set_title_page_doc(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.hf_option(window, cx, |doc, si, _, _, sel| {
            doc.set_title_page(si, on, sel)
        });
    }

    fn set_even_odd_doc(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.hf_option(window, cx, |doc, _, _, _, sel| {
            doc.set_even_odd_headers(on, sel)
        });
    }

    fn set_linked_doc(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.hf_option(window, cx, |doc, si, kind, ty, sel| {
            doc.set_hf_linked(si, kind, ty, on, sel)
        });
    }

    fn apply_hf_distances(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(m), Some(PartTarget::Hf { section, .. })) = (&self.hf_menu, self.part_target())
        else {
            return;
        };
        let props = self.main_doc().sections()[*section].props.clone();
        let read = |i: &Entity<InputState>, old: u32| {
            parse_cm(&i.read(cx).value()).map_or(old, |cm| docs_engine::cm_to_twips(cm.min(20.0)))
        };
        let (h, f) = (
            read(&m.header, props.header_dist),
            read(&m.footer, props.footer_dist),
        );
        if (h, f) == (props.header_dist, props.footer_dist) {
            return;
        }
        self.hf_option(window, cx, |doc, si, _, _, sel| {
            doc.set_hf_distances(si, h, f, sel)
        });
    }

    /// Edit the footer of the same page (from the header), or the header.
    fn switch_hf_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(PartTarget::Hf { kind, .. }) = self.part_target().cloned() else {
            return;
        };
        let page = self.part.as_ref().map_or(0, |s| s.page);
        let other = match kind {
            HfKind::Header => HfKind::Footer,
            HfKind::Footer => HfKind::Header,
        };
        self.edit_hf_on_page(page, other, None, window, cx);
        self.focus(window, cx);
    }

    /// Footer "Page N of M", centred, on every page of the caret's
    /// section (palette: Add Page Numbers).
    pub fn add_page_numbers_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_part(window, cx);
        let section = self.doc.section_of(self.sel.head.para);
        let text = t("Page {PAGE} of {PAGES}");
        if self.doc.add_page_numbers(section, text, self.sel) {
            let sel = self.sel;
            self.edited(sel, window, cx);
            self.status = t("Page numbers added to the footer").into();
        } else {
            self.status = t("This section's footer already has page numbers").into();
        }
        cx.notify();
    }

    /// A page number field at the caret (body, header or footer).
    pub fn insert_page_number_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.insert_page_field_doc(PageField::Page, window, cx);
    }

    pub(crate) fn insert_page_field_doc(
        &mut self,
        field: PageField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sel = self.doc.insert_page_field(self.sel, field);
        self.edited(sel, window, cx);
    }

    /// Section break (next page) at the caret.
    pub fn insert_section_break_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_part(window, cx);
        match self.doc.insert_section_break(self.sel) {
            Some(sel) => {
                self.edited(sel, window, cx);
                self.status =
                    t("Section break inserted: Page setup now changes this section only").into();
            }
            None => self.status = t("A section break can't go inside a table").into(),
        }
        cx.notify();
    }

    /// The Header & Footer bar, while a header or footer is edited.
    pub(crate) fn render_hf_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let Some(PartTarget::Hf { section, kind, ty }) = self.part_target().cloned() else {
            return None;
        };
        let menu = self.hf_menu.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let main = self.main_doc();
        let sections = main.sections();
        let props = &sections[section.min(sections.len() - 1)].props;
        let (first, even) = (props.title_page, main.even_odd_headers());
        let linked = main.hf_linked(section, kind, ty);

        let check =
            |id: &'static str,
             label: &'static str,
             on: bool,
             enabled: bool,
             apply: fn(&mut DocsView, bool, &mut Window, &mut Context<DocsView>)| {
                let v = view.clone();
                h_flex()
                    .id(id)
                    .flex_none()
                    .h(CONTROL_SM)
                    .px(SPACE_3)
                    .gap(SPACE_2)
                    .rounded(RADIUS_MD)
                    .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(t.hover)))
                    .when(!enabled, |d| d.opacity(0.5))
                    .child(
                        div()
                            .flex_none()
                            .size(ICON_MD)
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(RADIUS_SM)
                            .border_1()
                            .border_color(t.border_strong)
                            .when(on, |d| {
                                d.bg(t.accent)
                                    .border_color(t.accent)
                                    .text_color(t.accent_fg)
                                    .child(Icon::new(Lucide::Check).with_size(ICON_SM))
                            }),
                    )
                    .child(div().text_color(t.fg).child(label))
                    .when(enabled, |d| {
                        d.on_click(move |_, window, cx| {
                            let _ = v.update(cx, |this, cx| apply(this, !on, window, cx));
                        })
                    })
            };
        let button = |id: &'static str, label: &'static str, tip: &'static str| {
            div()
                .id(id)
                .flex_none()
                .h(CONTROL_SM)
                .px(SPACE_3)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .border_1()
                .border_color(t.border)
                .text_color(t.fg)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover))
                .child(label)
                .tooltip(move |window, cx| Tooltip::new(i18n::t(tip)).build(window, cx))
        };
        let field_btn = |id: &'static str, label: &'static str, tip: &'static str, f: PageField| {
            let v = view.clone();
            button(id, label, tip).on_click(move |_, window, cx| {
                let _ = v.update(cx, |this, cx| {
                    this.insert_page_field_doc(f, window, cx);
                    this.focus(window, cx);
                });
            })
        };
        let distance = |label: &'static str, input: &Entity<InputState>| {
            h_flex()
                .flex_none()
                .gap(SPACE_2)
                .child(div().text_color(t.fg_muted).child(label))
                .child(div().w(REM * 4.5).child(Input::new(input).small()))
                .child(div().text_color(t.fg_muted).child("cm"))
        };
        let switch = {
            let v = view.clone();
            let label = match kind {
                HfKind::Header => i18n::t("Go to Footer"),
                HfKind::Footer => i18n::t("Go to Header"),
            };
            button(
                "docs-hf-switch",
                label,
                tk("Edit the other one on this page"),
            )
            .on_click(move |_, window, cx| {
                let _ = v.update(cx, |this, cx| this.switch_hf_doc(window, cx));
            })
        };
        let close = {
            let v = view.clone();
            div()
                .id("docs-hf-close")
                .flex_none()
                .h(CONTROL_SM)
                .px(SPACE_4)
                .flex()
                .items_center()
                .gap(SPACE_2)
                .rounded(RADIUS_MD)
                .bg(t.accent)
                .text_color(t.accent_fg)
                .cursor_pointer()
                .child(Icon::new(Lucide::X).with_size(ICON_SM))
                .child(i18n::t("Close Header and Footer"))
                .tooltip(|window, cx| {
                    Tooltip::new(format!(
                        "{}  Esc",
                        i18n::t("Back to the document text (or double-click it)")
                    ))
                    .build(window, cx)
                })
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| {
                        this.leave_part(window, cx);
                        this.focus(window, cx);
                    });
                })
        };
        let title = {
            let mut s = part_label(kind, ty, even).to_owned();
            if sections.len() > 1 {
                s = format!("{s} · {}", tf("Section {}", &[&(section + 1)]));
            }
            if linked {
                s = format!("{s} · {}", i18n::t("Same as Previous"));
            }
            s
        };
        Some(
            h_flex()
                .id("docs-hf-bar")
                .flex_none()
                .flex_wrap()
                .items_center()
                .gap(SPACE_3)
                .px(SPACE_6)
                .py(SPACE_2)
                .bg(t.chrome)
                .border_b_1()
                .border_color(t.border)
                .text_size(FONT_SMALL)
                .text_color(t.fg)
                .child(
                    div()
                        .flex_none()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(t.accent)
                        .child(title),
                )
                .child(check(
                    "hf-first",
                    i18n::t("Different first page"),
                    first,
                    true,
                    DocsView::set_title_page_doc,
                ))
                .child(check(
                    "hf-even",
                    i18n::t("Different odd and even pages"),
                    even,
                    true,
                    DocsView::set_even_odd_doc,
                ))
                .child(check(
                    "hf-link",
                    i18n::t("Link to Previous"),
                    linked,
                    section > 0,
                    DocsView::set_linked_doc,
                ))
                .child(field_btn(
                    "hf-page",
                    i18n::t("Page number"),
                    tk("Insert the page number"),
                    PageField::Page,
                ))
                .child(field_btn(
                    "hf-pages",
                    i18n::t("Page count"),
                    tk("Insert the number of pages"),
                    PageField::NumPages,
                ))
                .child(distance(i18n::t("Header from top"), &menu.header))
                .child(distance(i18n::t("Footer from bottom"), &menu.footer))
                .child(div().flex_1())
                .child(switch)
                .child(close),
        )
    }
}

/// The toolbar's header/footer button.
pub(crate) fn hf_button(view: &DocsView, cx: &mut Context<DocsView>) -> Stateful<Div> {
    let t = tokens(cx);
    let v = cx.entity().downgrade();
    let open = matches!(view.part_target(), Some(PartTarget::Hf { .. }));
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
        .tooltip(|window, cx| Tooltip::new(i18n::t("Edit the header and footer")).build(window, cx))
        .on_click(move |_, window, cx| {
            let _ = v.update(cx, |this, cx| {
                this.toggle_hf_menu(HfKind::Header, None, window, cx)
            });
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn labels_follow_the_options() {
        assert_eq!(part_label(HfKind::Header, HfType::Default, false), "Header");
        assert_eq!(
            part_label(HfKind::Header, HfType::Default, true),
            "Odd Page Header"
        );
        assert_eq!(
            part_label(HfKind::Footer, HfType::First, false),
            "First Page Footer"
        );
        assert_eq!(
            part_label(HfKind::Footer, HfType::Even, true),
            "Even Page Footer"
        );
    }
}
