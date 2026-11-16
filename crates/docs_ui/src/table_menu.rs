//! The Table Properties popover, as Word's Table Properties › Table tab:
//! the table's alignment between the margins and its indent from left.
//! Each change is one undo step and applies at once.

use docs_engine::{TableAlign, TableProps, cm_to_twips};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, tk};
use kantor_ui::tokens::Elevation;
use kantor_ui::{components::section_label, metrics::*, tokens};

use crate::DocsView;
use crate::page_menu::{format_cm, parse_cm};

/// The open popover.
pub(crate) struct TableMenu {
    at: Point<Pixels>,
    indent: Entity<InputState>,
    _subs: Vec<Subscription>,
}

const ALIGNS: [(TableAlign, &str); 3] = [
    (TableAlign::Left, tk("Left")),
    (TableAlign::Center, tk("Center")),
    (TableAlign::Right, tk("Right")),
];

/// An indent as the field shows it ("1.25", "-0.5").
pub(crate) fn indent_cm(tw: i32) -> String {
    let s = format_cm(tw.unsigned_abs());
    if tw < 0 { format!("-{s}") } else { s }
}

/// A typed indent in twips (negative allowed, as in Word).
pub(crate) fn parse_indent(s: &str) -> Option<i32> {
    let s = s.trim();
    let (neg, v) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let tw = cm_to_twips(parse_cm(v)?.min(55.0)) as i32;
    Some(if neg { -tw } else { tw })
}

impl DocsView {
    /// Open the Table Properties popover for the table at the caret (or
    /// close it when open).
    pub fn toggle_table_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.table_menu.take().is_some() {
            cx.notify();
            return;
        }
        let Some(props) = self.doc.table_props(self.sel) else {
            self.status = i18n::t("The caret isn't in a table").into();
            cx.notify();
            return;
        };
        let b = self.viewport.get();
        let at = point(b.origin.x + SPACE_4, b.origin.y + SPACE_2);
        let indent = cx.new(|cx| InputState::new(window, cx));
        indent.update(cx, |i, cx| i.set_value(indent_cm(props.indent), window, cx));
        let subs =
            vec![
                cx.subscribe_in(&indent, window, |this, _, ev: &InputEvent, window, cx| {
                    if let InputEvent::PressEnter { .. } = ev {
                        this.table_menu_apply(None, window, cx);
                    }
                }),
            ];
        self.table_menu = Some(TableMenu {
            at,
            indent: indent.clone(),
            _subs: subs,
        });
        indent.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    /// Apply the popover's indent, and `align` when given.
    fn table_menu_apply(
        &mut self,
        align: Option<TableAlign>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(m) = &self.table_menu else { return };
        let Some(cur) = self.doc.table_props(self.sel) else {
            return;
        };
        let indent = parse_indent(&m.indent.read(cx).value()).unwrap_or(cur.indent);
        let props = TableProps {
            align: align.unwrap_or(cur.align),
            indent,
        };
        self.set_table_props_doc(props, window, cx);
    }

    /// Set the table's alignment and indent (one undo step).
    pub fn set_table_props_doc(
        &mut self,
        props: TableProps,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.doc.set_table_props(self.sel, props) {
            Ok(()) => {
                let sel = self.sel;
                self.edited(sel, window, cx);
            }
            Err(msg) => {
                self.status = i18n::t_dyn(&msg).into();
                cx.notify();
            }
        }
    }

    pub(crate) fn render_table_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let menu = self.table_menu.as_ref()?;
        let props = self.doc.table_props(self.sel)?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();

        let mut aligns = h_flex()
            .mx(SPACE_4)
            .p(SPACE_1)
            .gap(SPACE_1)
            .rounded(RADIUS_MD)
            .bg(t.surface)
            .border_1()
            .border_color(t.border);
        for (a, label) in ALIGNS {
            let v = view.clone();
            let on = props.align == a;
            aligns = aligns.child(
                div()
                    .id(SharedString::from(format!("docs-table-align-{}", a.jc())))
                    .flex_1()
                    .h(CONTROL_SM)
                    .px(SPACE_2)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(RADIUS_SM)
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.selected).text_color(t.fg))
                    .when(!on, |d| d.text_color(t.fg_muted).hover(|s| s.bg(t.hover)))
                    .child(i18n::t_dyn(label))
                    .on_click(move |_, window, cx| {
                        let _ = v.update(cx, |this, cx| this.table_menu_apply(Some(a), window, cx));
                    }),
            );
        }
        let left = props.align == TableAlign::Left;
        let v = view.clone();
        let indent_row = h_flex()
            .px(SPACE_4)
            .gap(SPACE_3)
            .child(
                div()
                    .flex_1()
                    .child(Input::new(&menu.indent).small().disabled(!left)),
            )
            .child(
                div()
                    .id("docs-table-indent-apply")
                    .h(CONTROL_SM)
                    .px(SPACE_5)
                    .flex()
                    .items_center()
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(t.border)
                    .when(left, |d| {
                        d.cursor_pointer()
                            .text_color(t.fg)
                            .hover(|s| s.bg(t.hover))
                            .on_click(move |_, window, cx| {
                                let _ = v
                                    .update(cx, |this, cx| this.table_menu_apply(None, window, cx));
                            })
                    })
                    .when(!left, |d| d.text_color(t.fg_dim))
                    .child(i18n::t("Apply")),
            );

        let panel = v_flex()
            .id("docs-table-menu")
            .key_context(crate::POPOVER_CONTEXT)
            .on_action(|_: &crate::PopoverKey, _, _| {})
            .occlude()
            .w(REM * 20.)
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
                this.table_menu = None;
                cx.notify();
            }))
            .child(heading(i18n::t("Table alignment"), cx))
            .child(aligns)
            .child(heading(i18n::t("Indent from left (cm)"), cx))
            .child(indent_row);
        Some(deferred(
            anchored()
                .position(menu.at)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel),
        ))
    }
}

fn heading(text: &'static str, cx: &App) -> Div {
    section_label(text, cx).px(SPACE_4).pt(SPACE_2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn indents_in_cm() {
        assert_eq!(indent_cm(567), "1");
        assert_eq!(indent_cm(-284), "-0.5");
        assert_eq!(parse_indent("-0,5"), Some(-284));
        assert_eq!(parse_indent("x"), None);
    }
}
