//! The Table Properties popover, as Word's Table Properties › Table tab:
//! the table's alignment between the margins and its indent from left.
//! Each change is one undo step and applies at once. Also Merge Cells and
//! the Split Cells popover (number of columns and rows).

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

/// The open Split Cells popover.
pub(crate) struct SplitMenu {
    at: Point<Pixels>,
    columns: Entity<InputState>,
    rows: Entity<InputState>,
    _subs: Vec<Subscription>,
}

/// Merge and split messages from docs_engine, marked for the catalog.
const MESSAGES: [&str; 5] = [
    tk("The caret isn't in a table"),
    tk("Select two or more cells to merge"),
    tk("These cells can't be merged"),
    tk("The number of rows must divide the rows the merged cell spans"),
    tk("A table can have at most 63 columns"),
];

/// Word's note when a table edit can't be tracked.
const NOT_TRACKED: &str = tk("This action will not be marked as a change.");

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

    /// Merge the selected cells (one undo step).
    pub fn merge_cells_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let r = self.doc.merge_cells(self.sel);
        self.table_structure_done(r, window, cx);
    }

    /// Split the cell at the caret into `columns` × `rows` (one undo step).
    pub fn split_cells_doc(
        &mut self,
        columns: usize,
        rows: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let r = self.doc.split_cells(self.sel, rows, columns);
        self.table_structure_done(r, window, cx);
    }

    fn table_structure_done(
        &mut self,
        r: Result<(docs_engine::Selection, bool), &'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match r {
            Ok((sel, untracked)) => {
                self.edited(sel, window, cx);
                if untracked {
                    self.status = i18n::t(NOT_TRACKED).into();
                    cx.notify();
                }
            }
            Err(msg) => {
                debug_assert!(MESSAGES.contains(&msg), "{msg}");
                self.status = i18n::t_dyn(msg).into();
                cx.notify();
            }
        }
    }

    /// Open the Split Cells popover (or close it when open).
    pub fn toggle_split_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.split_menu.take().is_some() {
            cx.notify();
            return;
        }
        if self.doc.cell_at(self.sel.head).is_none() {
            self.status = i18n::t("The caret isn't in a table").into();
            cx.notify();
            return;
        }
        let b = self.viewport.get();
        let at = point(b.origin.x + SPACE_4, b.origin.y + SPACE_2);
        let columns = cx.new(|cx| InputState::new(window, cx));
        let rows = cx.new(|cx| InputState::new(window, cx));
        columns.update(cx, |i, cx| i.set_value("2", window, cx));
        rows.update(cx, |i, cx| i.set_value("1", window, cx));
        let enter = |this: &mut Self,
                     _: &Entity<InputState>,
                     ev: &InputEvent,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            if let InputEvent::PressEnter { .. } = ev {
                this.split_menu_apply(window, cx);
            }
        };
        let subs = vec![
            cx.subscribe_in(&columns, window, enter),
            cx.subscribe_in(&rows, window, enter),
        ];
        self.split_menu = Some(SplitMenu {
            at,
            columns: columns.clone(),
            rows,
            _subs: subs,
        });
        columns.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    fn split_menu_apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = &self.split_menu else { return };
        let n = |e: &Entity<InputState>| e.read(cx).value().trim().parse::<usize>().ok();
        let (Some(c), Some(r)) = (n(&m.columns), n(&m.rows)) else {
            return;
        };
        if c == 0 || r == 0 {
            return;
        }
        self.split_menu = None;
        self.split_cells_doc(c, r, window, cx);
        self.focus(window, cx);
    }

    pub(crate) fn render_split_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let menu = self.split_menu.as_ref()?;
        let t = tokens(cx);
        let v = cx.entity().downgrade();
        let ok = div()
            .id("docs-split-ok")
            .h(CONTROL_SM)
            .px(SPACE_5)
            .flex()
            .items_center()
            .rounded(RADIUS_MD)
            .cursor_pointer()
            .bg(t.accent)
            .text_color(t.accent_fg)
            .child(i18n::t("Split"))
            .on_click(move |_, window, cx| {
                let _ = v.update(cx, |this, cx| this.split_menu_apply(window, cx));
            });
        let panel = v_flex()
            .id("docs-split-menu")
            .key_context(crate::POPOVER_CONTEXT)
            .on_action(|_: &crate::PopoverKey, _, _| {})
            .occlude()
            .w(REM * 16.)
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
                this.split_menu = None;
                cx.notify();
            }))
            .child(heading(i18n::t("Number of columns"), cx))
            .child(div().px(SPACE_4).child(Input::new(&menu.columns).small()))
            .child(heading(i18n::t("Number of rows"), cx))
            .child(div().px(SPACE_4).child(Input::new(&menu.rows).small()))
            .child(h_flex().px(SPACE_4).justify_end().child(ok));
        Some(deferred(
            anchored()
                .position(menu.at)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel),
        ))
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
    fn engine_messages_are_in_the_catalog() {
        use docs_engine::table_merge as m;
        for msg in [
            m::NOT_IN_TABLE,
            m::SELECT_CELLS,
            m::NOT_RECTANGLE,
            m::ROWS_DIVIDE,
            m::TOO_MANY_COLUMNS,
        ] {
            assert!(MESSAGES.contains(&msg), "{msg}");
        }
    }

    #[std::prelude::v1::test]
    fn indents_in_cm() {
        assert_eq!(indent_cm(567), "1");
        assert_eq!(indent_cm(-284), "-0.5");
        assert_eq!(parse_indent("-0,5"), Some(-283));
        assert_eq!(parse_indent("x"), None);
    }
}
