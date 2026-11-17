//! Columns, as Word's Layout ▸ Columns: the toolbar's Columns control and
//! its popover (One, Two, Three, Left, Right, a line between, Insert
//! Column Break) and "More Columns…" (number of columns, each column's
//! width and spacing or equal ones, line between, and where it applies:
//! the whole document, this section, this point forward or the selected
//! text). Every change is one undo step.

use std::time::{Duration, Instant};

use docs_engine::{
    Column, ColumnPreset, Columns, ColumnsApply, cm_to_twips,
    columns::{DEFAULT_SPACE, MAX_COLUMNS},
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, Sizable as _, h_flex, tooltip::Tooltip, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t_dyn, tk};
use kantor_ui::tokens::Elevation;
use kantor_ui::{components::section_label, metrics::*, tokens};

use crate::DocsView;
use crate::page_menu::{format_cm, parse_cm};

/// Rows of width and spacing fields "More Columns…" shows (Word shows
/// three and scrolls; more columns than this are made equal).
const ROWS: usize = 6;

/// Preset names from the engine, marked for the catalog.
const PRESETS: [&str; 5] = [tk("One"), tk("Two"), tk("Three"), tk("Left"), tk("Right")];

/// "Apply to" choices from the engine, marked for the catalog.
const APPLY: [&str; 4] = [
    tk("Whole document"),
    tk("This section"),
    tk("This point forward"),
    tk("Selected text"),
];

/// An engine label (preset or "Apply to" choice) in the interface language.
fn label(s: &'static str) -> &'static str {
    if PRESETS.contains(&s) || APPLY.contains(&s) {
        t_dyn(s)
    } else {
        s
    }
}

/// The open popover.
pub(crate) struct ColumnsMenu {
    at: Point<Pixels>,
    /// "More Columns…" is open.
    more: Option<MoreColumns>,
}

/// The fields of "More Columns…".
pub(crate) struct MoreColumns {
    number: Entity<InputState>,
    widths: Vec<Entity<InputState>>,
    spacings: Vec<Entity<InputState>>,
    equal: bool,
    sep: bool,
    apply: ColumnsApply,
    _subs: Vec<Subscription>,
}

impl DocsView {
    /// The columns of the caret's section.
    pub fn section_columns(&self) -> Columns {
        self.main_doc().section_columns(self.body_sel().head.para)
    }

    fn text_width(&self) -> u32 {
        self.main_doc()
            .section_text_width(self.body_sel().head.para)
    }

    /// Set columns (one undo step) and lay out again.
    pub fn set_columns_doc(
        &mut self,
        cols: Columns,
        apply: ColumnsApply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_part(window, cx);
        let sel = self.doc.set_columns(self.sel, &cols, apply);
        self.edited(sel, window, cx);
    }

    /// One of Word's presets, for the selected text or the caret's section
    /// (keeping its line between).
    pub fn column_preset_doc(
        &mut self,
        preset: ColumnPreset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut cols = preset.columns(self.text_width());
        cols.sep = self.section_columns().sep;
        let apply = if self.sel.is_empty() {
            ColumnsApply::ThisSection
        } else {
            ColumnsApply::SelectedText
        };
        self.set_columns_doc(cols, apply, window, cx);
    }

    /// Turn the line between columns on or off for the caret's section.
    pub fn toggle_column_line_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut cols = self.section_columns();
        cols.sep = !cols.sep;
        self.set_columns_doc(cols, ColumnsApply::ThisSection, window, cx);
    }

    /// Insert ▸ Break ▸ Column (Ctrl+Shift+Enter).
    pub fn insert_column_break_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.part.is_some() {
            return;
        }
        self.float_sel = None;
        let sel = self.doc.insert_column_break(self.sel);
        self.marked = None;
        self.edited(sel, window, cx);
    }

    /// Open the Columns popover at `at` (window coordinates, its top-right
    /// corner), with "More Columns…" open when `more`; clicking the
    /// toolbar control while it's open closes it.
    pub fn toggle_columns_menu(
        &mut self,
        at: Option<Point<Pixels>>,
        more: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if (self.columns_menu.take().is_some() && !more)
            || self
                .columns_menu_closed
                .is_some_and(|t| t.elapsed() < Duration::from_millis(250))
        {
            self.columns_menu_closed = None;
            cx.notify();
            return;
        }
        let at = at.unwrap_or_else(|| {
            let b = self.viewport.get();
            point(b.origin.x + b.size.width - SPACE_4, b.origin.y + SPACE_2)
        });
        self.columns_menu = Some(ColumnsMenu { at, more: None });
        if more {
            self.open_more_columns(window, cx);
        }
        cx.notify();
    }

    /// "More Columns…": the fields, filled from the caret's section.
    pub fn open_more_columns(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut fields: Vec<Entity<InputState>> = Vec::with_capacity(ROWS * 2 + 1);
        for _ in 0..ROWS * 2 + 1 {
            fields.push(cx.new(|cx| InputState::new(window, cx)));
        }
        let spacings = fields.split_off(ROWS + 1);
        let widths = fields.split_off(1);
        let number = fields.remove(0);
        let mut subs = Vec::new();
        subs.push(
            cx.subscribe_in(&number, window, |this, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    this.more_columns_refill(window, cx);
                }
            }),
        );
        for e in widths.iter().chain(&spacings) {
            subs.push(cx.subscribe_in(e, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.more_columns_apply(window, cx);
                }
            }));
        }
        let cols = self.section_columns();
        let apply = if !self.sel.is_empty() {
            ColumnsApply::SelectedText
        } else if self.main_doc().sections().len() > 1 {
            ColumnsApply::ThisSection
        } else {
            ColumnsApply::WholeDocument
        };
        let Some(menu) = &mut self.columns_menu else {
            return;
        };
        menu.more = Some(MoreColumns {
            number,
            widths,
            spacings,
            equal: cols.equal || cols.count() > ROWS,
            sep: cols.sep,
            apply,
            _subs: subs,
        });
        self.fill_more_columns(&cols, window, cx);
        cx.notify();
    }

    /// Show `cols` in the fields.
    fn fill_more_columns(&mut self, cols: &Columns, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text_width();
        let Some(m) = self.columns_menu.as_mut().and_then(|m| m.more.as_mut()) else {
            return;
        };
        let lay = cols.layout(text);
        let n = lay.len();
        m.number
            .update(cx, |i, cx| i.set_value(n.to_string(), window, cx));
        for k in 0..ROWS {
            let (w, s) = match lay.get(k) {
                Some(&(x, w)) => {
                    let s = lay
                        .get(k + 1)
                        .map_or(0, |next| next.0.saturating_sub(x + w));
                    (format_cm(w), if k + 1 < n { format_cm(s) } else { String::new() })
                }
                None => (String::new(), String::new()),
            };
            m.widths[k].update(cx, |i, cx| i.set_value(w, window, cx));
            m.spacings[k].update(cx, |i, cx| i.set_value(s, window, cx));
        }
    }

    /// The columns the fields describe.
    fn more_columns_value(&self, cx: &App) -> Option<Columns> {
        let m = self.columns_menu.as_ref()?.more.as_ref()?;
        let text = self.text_width();
        let n: u16 = m.number.read(cx).value().trim().parse().ok()?;
        let n = n.clamp(1, MAX_COLUMNS);
        let cm = |e: &Entity<InputState>| parse_cm(&e.read(cx).value()).map(cm_to_twips);
        let space = cm(&m.spacings[0]).unwrap_or(DEFAULT_SPACE);
        if m.equal || n as usize > ROWS || n == 1 {
            let mut c = Columns::equal(n, if n == 1 { DEFAULT_SPACE } else { space });
            c.sep = m.sep;
            return Some(c);
        }
        let mut cols: Vec<Column> = (0..n as usize)
            .map(|k| Column {
                width: cm(&m.widths[k]).unwrap_or(0),
                space: if k + 1 < n as usize {
                    cm(&m.spacings[k]).unwrap_or(DEFAULT_SPACE)
                } else {
                    0
                },
            })
            .collect();
        // Columns left empty share what the others leave, as Word does.
        let used: u32 = cols.iter().map(|c| c.width + c.space).sum();
        let empty = cols.iter().filter(|c| c.width == 0).count() as u32;
        if empty > 0 {
            let each = (text.saturating_sub(used) / empty).max(cm_to_twips(1.27));
            for c in cols.iter_mut().filter(|c| c.width == 0) {
                c.width = each;
            }
        }
        Some(Columns {
            num: n,
            space,
            equal: false,
            sep: m.sep,
            cols,
        })
    }

    /// The number of columns changed: new widths for the rest.
    fn more_columns_refill(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.columns_menu.as_ref().and_then(|m| m.more.as_ref()) else {
            return;
        };
        let Ok(n) = m.number.read(cx).value().trim().parse::<u16>() else {
            return;
        };
        let space = parse_cm(&m.spacings[0].read(cx).value())
            .map(cm_to_twips)
            .unwrap_or(DEFAULT_SPACE);
        let mut cols = Columns::equal(n.clamp(1, MAX_COLUMNS), space);
        if !m.equal && (n as usize) <= ROWS {
            cols = cols.to_unequal(self.text_width());
        }
        self.fill_more_columns(&cols, window, cx);
    }

    fn set_more_equal(&mut self, on: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut cols) = self.more_columns_value(cx) else {
            return;
        };
        if let Some(m) = self.columns_menu.as_mut().and_then(|m| m.more.as_mut()) {
            m.equal = on;
        }
        cols = if on {
            Columns::equal(cols.num, cols.cols.first().map_or(cols.space, |c| c.space))
        } else {
            cols.to_unequal(self.text_width())
        };
        self.fill_more_columns(&cols, window, cx);
        cx.notify();
    }

    fn more_columns_apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cols) = self.more_columns_value(cx) else {
            self.status = i18n::t("Type a number of columns").into();
            cx.notify();
            return;
        };
        let Some(apply) = self
            .columns_menu
            .as_ref()
            .and_then(|m| m.more.as_ref())
            .map(|m| m.apply)
        else {
            return;
        };
        self.columns_menu = None;
        self.set_columns_doc(cols, apply, window, cx);
        self.focus(window, cx);
    }

    pub(crate) fn render_columns_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let menu = self.columns_menu.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let cols = self.section_columns();
        let current = ColumnPreset::of(&cols, self.text_width());
        type Apply = Box<dyn Fn(&mut DocsView, &mut Window, &mut Context<DocsView>)>;
        let row = |id: SharedString, label: &str, detail: &str, on: bool, apply: Apply| {
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
                .child(div().text_color(t.fg).child(label.to_owned()))
                .child(div().flex_1())
                .child(div().text_color(t.fg_dim).child(detail.to_owned()))
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| apply(this, window, cx));
                })
        };
        let mut presets = v_flex();
        for p in ColumnPreset::ALL {
            presets = presets.child(row(
                format!("docs-columns-{}", p.label()).into(),
                label(p.label()),
                "",
                current == Some(p),
                Box::new(move |v, w, cx| {
                    v.columns_menu = None;
                    v.column_preset_doc(p, w, cx)
                }),
            ));
        }
        let more_open = menu.more.is_some();
        let mut panel = v_flex()
            .id("docs-columns-menu")
            .key_context(crate::POPOVER_CONTEXT)
            .on_action(|_: &crate::PopoverKey, _, _| {})
            .occlude()
            .w(REM * 22.)
            .gap(SPACE_2)
            .p(SPACE_4)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .rounded(RADIUS_LG)
            .shadow(t.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .text_color(t.fg)
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.columns_menu = None;
                this.columns_menu_closed = Some(Instant::now());
                cx.notify();
            }));
        if !more_open {
            panel = panel
                .child(heading(i18n::t("Columns"), cx))
                .child(presets)
                .child(row(
                    "docs-columns-line".into(),
                    i18n::t("Line between"),
                    "",
                    cols.sep,
                    Box::new(|v, w, cx| v.toggle_column_line_doc(w, cx)),
                ))
                .child(row(
                    "docs-columns-break".into(),
                    i18n::t("Insert Column Break"),
                    "Ctrl+Shift+Enter",
                    false,
                    Box::new(|v, w, cx| {
                        v.columns_menu = None;
                        v.insert_column_break_doc(w, cx)
                    }),
                ))
                .child(row(
                    "docs-columns-more".into(),
                    i18n::t("More Columns…"),
                    "",
                    false,
                    Box::new(|v, w, cx| v.open_more_columns(w, cx)),
                ));
        }
        if let Some(m) = &menu.more {
            panel = panel.child(self.render_more_columns(m, cx));
        }
        Some(deferred(
            anchored()
                .position(menu.at)
                .anchor(Anchor::TopRight)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel),
        ))
    }

    fn render_more_columns(&self, m: &MoreColumns, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let n = m
            .number
            .read(cx)
            .value()
            .trim()
            .parse::<usize>()
            .unwrap_or(1)
            .clamp(1, MAX_COLUMNS as usize);
        let check = |id: &'static str, label: &'static str, on: bool, enabled: bool| {
            h_flex()
                .id(id)
                .h(CONTROL_SM)
                .px(SPACE_4)
                .gap(SPACE_3)
                .rounded(RADIUS_MD)
                .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(t.hover)))
                .child(
                    div()
                        .flex_none()
                        .size(ICON_MD)
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(RADIUS_SM)
                        .border_1()
                        .border_color(if on { t.accent } else { t.border_strong })
                        .when(on, |d| {
                            d.bg(t.accent).child(
                                Icon::new(Lucide::Check)
                                    .with_size(ICON_SM)
                                    .text_color(t.accent_fg),
                            )
                        }),
                )
                .child(
                    div()
                        .text_color(if enabled { t.fg } else { t.fg_dim })
                        .child(i18n::t(label)),
                )
        };
        let v = view.clone();
        let equal_enabled = n > 1 && n <= ROWS;
        let equal = check("docs-columns-equal", "Equal column width", m.equal, equal_enabled)
            .when(equal_enabled, |d| {
                let on = m.equal;
                d.on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| this.set_more_equal(!on, window, cx));
                })
            });
        let v = view.clone();
        let line = check("docs-columns-sep", "Line between", m.sep, n > 1).on_click(
            move |_, _, cx| {
                let _ = v.update(cx, |this, cx| {
                    if let Some(m) = this.columns_menu.as_mut().and_then(|m| m.more.as_mut()) {
                        m.sep = !m.sep;
                    }
                    cx.notify();
                });
            },
        );
        // Width and spacing, one row per column (only the first editable
        // when they're equal: the others follow it).
        let mut grid = v_flex().px(SPACE_4).gap(SPACE_2).child(
            h_flex()
                .gap(SPACE_3)
                .text_color(t.fg_muted)
                .child(div().w(REM * 2.5).child(i18n::t("Col #")))
                .child(div().flex_1().child(i18n::t("Width (cm)")))
                .child(div().flex_1().child(i18n::t("Spacing (cm)"))),
        );
        for k in 0..n.min(ROWS) {
            let width_on = !m.equal;
            let space_on = k + 1 < n && (!m.equal || k == 0);
            grid = grid.child(
                h_flex()
                    .gap(SPACE_3)
                    .child(div().w(REM * 2.5).child(format!("{}:", k + 1)))
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&m.widths[k]).small().disabled(!width_on)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&m.spacings[k]).small().disabled(!space_on)),
                    ),
            );
        }
        let has_sel = !self.sel.is_empty();
        let several = self.main_doc().sections().len() > 1;
        let mut applies = v_flex();
        for a in [
            ColumnsApply::WholeDocument,
            ColumnsApply::ThisSection,
            ColumnsApply::ThisPointForward,
            ColumnsApply::SelectedText,
        ] {
            let shown = match a {
                ColumnsApply::ThisSection => several,
                ColumnsApply::SelectedText => has_sel,
                ColumnsApply::ThisPointForward => !has_sel,
                ColumnsApply::WholeDocument => true,
            };
            if !shown {
                continue;
            }
            let v = view.clone();
            let on = m.apply == a;
            applies = applies.child(
                h_flex()
                    .id(SharedString::from(format!("docs-columns-apply-{a:?}")))
                    .h(CONTROL_SM)
                    .px(SPACE_4)
                    .gap(SPACE_3)
                    .rounded(RADIUS_MD)
                    .cursor_pointer()
                    .hover(|s| s.bg(t.hover))
                    .child(
                        div()
                            .flex_none()
                            .size(ICON_MD)
                            .when(on, |d| d.child(Icon::new(Lucide::Check).with_size(ICON_MD))),
                    )
                    .child(label(a.label()))
                    .on_click(move |_, _, cx| {
                        let _ = v.update(cx, |this, cx| {
                            if let Some(m) =
                                this.columns_menu.as_mut().and_then(|m| m.more.as_mut())
                            {
                                m.apply = a;
                            }
                            cx.notify();
                        });
                    }),
            );
        }
        let v = view.clone();
        let ok = div()
            .id("docs-columns-ok")
            .h(CONTROL_SM)
            .px(SPACE_5)
            .flex()
            .items_center()
            .rounded(RADIUS_MD)
            .cursor_pointer()
            .bg(t.accent)
            .text_color(t.accent_fg)
            .child(i18n::t("Apply"))
            .tooltip(|window, cx| Tooltip::new(i18n::t("Apply these columns  Enter")).build(window, cx))
            .on_click(move |_, window, cx| {
                let _ = v.update(cx, |this, cx| this.more_columns_apply(window, cx));
            });
        v_flex()
            .gap(SPACE_2)
            .child(heading(i18n::t("More Columns"), cx))
            .child(
                h_flex()
                    .px(SPACE_4)
                    .gap(SPACE_3)
                    .child(div().text_color(t.fg_muted).child(i18n::t("Number of columns")))
                    .child(div().w(REM * 4.).child(Input::new(&m.number).small())),
            )
            .child(heading(i18n::t("Width and spacing"), cx))
            .child(grid)
            .child(equal)
            .child(line)
            .child(heading(i18n::t("Apply to"), cx))
            .child(applies)
            .child(h_flex().px(SPACE_4).justify_end().child(ok))
    }
}

fn heading(text: &'static str, cx: &App) -> Div {
    section_label(text, cx).px(SPACE_4).pt(SPACE_2)
}

/// The toolbar's Columns control: opens the popover.
pub(crate) fn columns_button(view: &DocsView, cx: &mut Context<DocsView>) -> Stateful<Div> {
    let t = tokens(cx);
    let v = cx.entity().downgrade();
    let open = view.columns_menu.is_some();
    h_flex()
        .id("docs-columns")
        .flex_none()
        .size(CONTROL_SM)
        .justify_center()
        .rounded(RADIUS_MD)
        .cursor_pointer()
        .when(open, |d| d.bg(t.selected).text_color(t.fg))
        .when(!open, |d| {
            d.text_color(t.fg_muted)
                .hover(|s| s.bg(t.hover).text_color(t.fg))
        })
        .child(Icon::new(Lucide::Columns2).with_size(ICON_MD))
        .tooltip(|window, cx| {
            Tooltip::new(i18n::t("Columns: one, two, three, more…")).build(window, cx)
        })
        .on_click(move |ev, window, cx| {
            let p = ev.position();
            let at = point(p.x + SPACE_7, p.y + SPACE_7);
            let _ = v.update(cx, |this, cx| {
                this.toggle_columns_menu(Some(at), false, window, cx)
            });
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn engine_labels_are_in_the_catalog() {
        for p in ColumnPreset::ALL {
            assert!(PRESETS.contains(&p.label()), "{}", p.label());
        }
        for a in [
            ColumnsApply::WholeDocument,
            ColumnsApply::ThisSection,
            ColumnsApply::ThisPointForward,
            ColumnsApply::SelectedText,
        ] {
            assert!(APPLY.contains(&a.label()), "{}", a.label());
        }
    }
}
