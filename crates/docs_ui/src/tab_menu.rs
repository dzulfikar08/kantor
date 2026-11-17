//! The Tabs popover, as Word's Tabs dialog: the selected paragraphs' tab
//! stops (the ones they all share), a position in cm, alignment and
//! leader, and Set / Clear / Clear All; plus the document's default tab
//! stop interval. Each button is one undo step and applies at once.

use docs_engine::tabs::TabEdit;
use docs_engine::{TabKind, TabLeader, TabStop, cm_to_twips};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t_dyn, tk};
use kantor_ui::tokens::Elevation;
use kantor_ui::{components::section_label, metrics::*, tokens};

use crate::DocsView;
use crate::page_menu::{format_cm, parse_cm};

/// The open popover.
pub(crate) struct TabMenu {
    at: Point<Pixels>,
    pos: Entity<InputState>,
    default: Entity<InputState>,
    kind: TabKind,
    leader: TabLeader,
    _subs: Vec<Subscription>,
}

/// Alignment names, marked for the catalog.
const KIND_TEXT: [&str; 5] = [
    tk("Left"),
    tk("Center"),
    tk("Right"),
    tk("Decimal"),
    tk("Bar"),
];

/// The alignment's name in the interface language.
pub(crate) fn kind_label(k: TabKind) -> &'static str {
    let en = k.label();
    debug_assert!(KIND_TEXT.contains(&en));
    t_dyn(en)
}

/// Leaders the dialog offers (Word's four; heavy and middle-dot leaders
/// from files are drawn and kept).
const LEADERS: [TabLeader; 4] = [
    TabLeader::None,
    TabLeader::Dot,
    TabLeader::Hyphen,
    TabLeader::Underscore,
];

fn leader_label(l: TabLeader) -> &'static str {
    match l {
        TabLeader::None => i18n::t("None"),
        l => l.sample(),
    }
}

/// A stop's position as the dialog shows it ("2.5").
fn stop_cm(pos: i32) -> String {
    let s = format_cm(pos.unsigned_abs());
    if pos < 0 { format!("-{s}") } else { s }
}

impl DocsView {
    /// Open the Tabs popover (or close it when open), at `at` (window
    /// coordinates, its top-left corner) or near the top of the page area.
    pub fn toggle_tab_menu(
        &mut self,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.tab_menu.take().is_some() {
            cx.notify();
            return;
        }
        self.open_tab_menu(at, None, window, cx);
    }

    /// Open the Tabs popover with `stop` (if any) chosen.
    pub(crate) fn open_tab_menu(
        &mut self,
        at: Option<Point<Pixels>>,
        stop: Option<TabStop>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = at.unwrap_or_else(|| {
            let b = self.viewport.get();
            point(b.origin.x + SPACE_4, b.origin.y + SPACE_2)
        });
        let pos = cx.new(|cx| InputState::new(window, cx));
        let default = cx.new(|cx| InputState::new(window, cx));
        let subs = vec![
            cx.subscribe_in(&pos, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.tab_menu_set(window, cx);
                }
            }),
            cx.subscribe_in(&default, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.tab_menu_default(window, cx);
                }
            }),
        ];
        let stop = stop.or_else(|| self.doc.common_tab_stops(self.sel).first().copied());
        let d = format_cm(self.doc.default_tab());
        default.update(cx, |i, cx| i.set_value(d, window, cx));
        if let Some(s) = stop {
            pos.update(cx, |i, cx| i.set_value(stop_cm(s.pos), window, cx));
        }
        self.tab_menu = Some(TabMenu {
            at,
            pos: pos.clone(),
            default,
            kind: stop.map_or(TabKind::Left, |s| s.kind),
            leader: stop.map_or(TabLeader::None, |s| s.leader),
            _subs: subs,
        });
        pos.update(cx, |i, cx| i.focus(window, cx));
        cx.notify();
    }

    /// Change the selected paragraphs' tab stops (one undo step).
    pub fn edit_tabs_doc(&mut self, edit: TabEdit, window: &mut Window, cx: &mut Context<Self>) {
        self.doc.edit_tab_stops(self.sel, &edit);
        let sel = self.sel;
        self.edited(sel, window, cx);
    }

    /// The position field in twips (`None` if it doesn't parse).
    fn tab_menu_pos(&self, cx: &App) -> Option<i32> {
        let m = self.tab_menu.as_ref()?;
        let v = m.pos.read(cx).value();
        let v = v.trim();
        let (neg, v) = match v.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, v),
        };
        let tw = cm_to_twips(parse_cm(v)?.min(55.0)) as i32;
        Some(if neg { -tw } else { tw })
    }

    /// Set: a stop at the typed position with the chosen alignment and leader.
    fn tab_menu_set(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(pos), Some(m)) = (self.tab_menu_pos(cx), self.tab_menu.as_ref()) else {
            return;
        };
        let stop = TabStop::new(pos, m.kind, m.leader);
        self.edit_tabs_doc(
            TabEdit {
                set: vec![stop],
                ..TabEdit::default()
            },
            window,
            cx,
        );
    }

    /// Clear: the stop at the typed position (within half a millimetre).
    fn tab_menu_clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(pos) = self.tab_menu_pos(cx) else {
            return;
        };
        let near: Vec<i32> = self
            .doc
            .common_tab_stops(self.sel)
            .iter()
            .map(|s| s.pos)
            .filter(|p| (p - pos).abs() <= 28)
            .collect();
        self.edit_tabs_doc(
            TabEdit {
                clear: near,
                ..TabEdit::default()
            },
            window,
            cx,
        );
    }

    fn tab_menu_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = &self.tab_menu else { return };
        let Some(cm) = parse_cm(&m.default.read(cx).value()) else {
            return;
        };
        let tw = cm_to_twips(cm.min(55.0));
        if tw == 0 {
            return;
        }
        self.leave_part(window, cx);
        self.doc.set_default_tab(tw, self.sel);
        let sel = self.sel;
        self.edited(sel, window, cx);
    }

    /// Choose a listed stop: its values go into the fields.
    fn tab_menu_pick(&mut self, s: TabStop, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = &mut self.tab_menu else { return };
        m.kind = s.kind;
        m.leader = s.leader;
        m.pos
            .update(cx, |i, cx| i.set_value(stop_cm(s.pos), window, cx));
        cx.notify();
    }

    pub(crate) fn render_tab_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let menu = self.tab_menu.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let stops = self.doc.common_tab_stops(self.sel);
        let typed = self.tab_menu_pos(cx);

        let mut list = v_flex()
            .id("docs-tabs-list")
            .min_h(CONTROL_MD * 3.)
            .max_h(CONTROL_MD * 5.)
            .overflow_y_scroll()
            .mx(SPACE_4)
            .p(SPACE_1)
            .rounded(RADIUS_MD)
            .bg(t.surface)
            .border_1()
            .border_color(t.border);
        if stops.is_empty() {
            list = list.child(
                div()
                    .px(SPACE_3)
                    .py(SPACE_2)
                    .text_color(t.fg_dim)
                    .child(i18n::t("No tab stops: default stops apply")),
            );
        }
        for s in &stops {
            let s = *s;
            let v = view.clone();
            let on = typed.is_some_and(|p| (p - s.pos).abs() <= 28);
            list = list.child(
                h_flex()
                    .id(SharedString::from(format!("docs-tab-{}", s.pos)))
                    .h(CONTROL_SM)
                    .px(SPACE_3)
                    .gap(SPACE_4)
                    .rounded(RADIUS_SM)
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.selected))
                    .when(!on, |d| d.hover(|d| d.bg(t.hover)))
                    .child(
                        div()
                            .w(REM * 4.)
                            .text_color(t.fg)
                            .child(format!("{} cm", stop_cm(s.pos))),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_color(t.fg_muted)
                            .child(kind_label(s.kind)),
                    )
                    .child(
                        div()
                            .text_color(t.fg_dim)
                            .child(if s.leader == TabLeader::None {
                                String::new()
                            } else {
                                s.leader.sample().to_owned()
                            }),
                    )
                    .on_click(move |_, window, cx| {
                        let _ = v.update(cx, |this, cx| this.tab_menu_pick(s, window, cx));
                    }),
            );
        }

        type Pick = Box<dyn Fn(&mut TabMenu)>;
        let segment = |id: SharedString, label: String, on: bool, pick: Pick| {
            let v = view.clone();
            div()
                .id(id)
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
                .child(label)
                .on_click(move |_, _, cx| {
                    let _ = v.update(cx, |this, cx| {
                        if let Some(m) = &mut this.tab_menu {
                            pick(m);
                            cx.notify();
                        }
                    });
                })
        };
        let group = || {
            h_flex()
                .mx(SPACE_4)
                .p(SPACE_1)
                .gap(SPACE_1)
                .rounded(RADIUS_MD)
                .bg(t.surface)
                .border_1()
                .border_color(t.border)
        };
        let mut kinds = group();
        for k in TabKind::ALL {
            kinds = kinds.child(segment(
                format!("docs-tab-kind-{}", k.xml()).into(),
                kind_label(k).to_owned(),
                menu.kind == k,
                Box::new(move |m| m.kind = k),
            ));
        }
        let mut leaders = group();
        for l in LEADERS {
            leaders = leaders.child(segment(
                format!("docs-tab-leader-{}", l.xml().unwrap_or("none")).into(),
                leader_label(l).to_owned(),
                menu.leader == l,
                Box::new(move |m| m.leader = l),
            ));
        }

        type Act = fn(&mut DocsView, &mut Window, &mut Context<DocsView>);
        let button = |id: &'static str, label: &'static str, primary: bool, act: Act| {
            let v = view.clone();
            div()
                .id(id)
                .h(CONTROL_SM)
                .px(SPACE_5)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when(primary, |d| d.bg(t.accent).text_color(t.accent_fg))
                .when(!primary, |d| {
                    d.border_1()
                        .border_color(t.border)
                        .text_color(t.fg)
                        .hover(|s| s.bg(t.hover))
                })
                .child(i18n::t_dyn(label))
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| act(this, window, cx));
                })
        };
        let buttons = h_flex()
            .px(SPACE_4)
            .gap(SPACE_3)
            .child(button(
                "docs-tabs-set",
                tk("Set"),
                true,
                DocsView::tab_menu_set,
            ))
            .child(button(
                "docs-tabs-clear",
                tk("Clear"),
                false,
                DocsView::tab_menu_clear,
            ))
            .child(button(
                "docs-tabs-clear-all",
                tk("Clear All"),
                false,
                |this, w, cx| {
                    this.edit_tabs_doc(
                        TabEdit {
                            clear_all: true,
                            ..TabEdit::default()
                        },
                        w,
                        cx,
                    )
                },
            ));
        let default_row = h_flex()
            .px(SPACE_4)
            .gap(SPACE_3)
            .child(div().flex_1().child(Input::new(&menu.default).small()))
            .child(button(
                "docs-tabs-default",
                tk("Apply"),
                false,
                DocsView::tab_menu_default,
            ));

        let panel = v_flex()
            .id("docs-tab-menu")
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
                this.tab_menu = None;
                cx.notify();
            }))
            .child(heading(i18n::t("Tab stop position (cm)"), cx))
            .child(div().px(SPACE_4).child(Input::new(&menu.pos).small()))
            .child(list)
            .child(heading(i18n::t("Alignment"), cx))
            .child(kinds)
            .child(heading(i18n::t("Leader"), cx))
            .child(leaders)
            .child(buttons)
            .child(heading(i18n::t("Default tab stops (cm)"), cx))
            .child(default_row);
        Some(kantor_ui::overlay::popover(
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
    fn stop_positions_in_cm() {
        assert_eq!(stop_cm(1134), "2");
        assert_eq!(stop_cm(-567), "-1");
        assert_eq!(stop_cm(9026), "15.92");
        for k in TabKind::ALL {
            assert!(KIND_TEXT.contains(&k.label()));
        }
    }
}
