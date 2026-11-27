//! The sheet tabs' commands, as Excel's tab menu has them: Insert, Delete,
//! Rename, Move or Copy…, Move Left / Right, Tab Color…, Hide and
//! Unhide…, for worksheets and chart sheets alike (one menu). Each is one
//! undo step
//! (`sheets_engine::sheet_tabs`, `sheets_engine::chart_sheet`).
//!
//! The dialogs (Rename, Delete's question, Move or Copy, Tab Color,
//! Unhide) are cards over the view, siblings of the grid like the other
//! Sheets dialogs.

use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    checkbox::Checkbox,
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{t, t_dyn, tf, tk};
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{STANDARD_COLORS, Tab, TabColor, theme_tints};

use crate::SheetsView;
use crate::dialog::{self, note};

/// Engine messages the dialogs show, listed for the catalogue.
#[allow(dead_code)]
const ENGINE_ERRORS: &[&str] = &[
    tk("A sheet name can't be blank"),
    tk("A sheet name can have at most 31 characters"),
    tk("A sheet name can't contain \\ / ? * [ ] or :"),
    tk("A sheet name can't begin or end with an apostrophe"),
    tk("“History” is a name Excel keeps for itself"),
    tk("That name is already taken. Try a different one."),
    tk("A workbook must contain at least one visible worksheet"),
];

fn engine_error(e: &anyhow::Error) -> String {
    t_dyn(&format!("{e:#}")).to_owned()
}

/// Excel's theme columns: Background 1, Text 1, Background 2, Text 2,
/// Accent 1-6.
const THEME_SLOTS: [u8; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

/// A tab dialog that is open.
pub(crate) enum TabDialog {
    Rename {
        tab: Tab,
        input: Entity<InputState>,
        error: Option<String>,
        _sub: Subscription,
    },
    /// Delete's question, for a sheet with something on it.
    Delete {
        tab: Tab,
        name: String,
    },
    /// Move or Copy: the tab, the "Before sheet" chosen (an index into
    /// the shown tabs; their count: "(move to end)") and "Create a copy".
    MoveCopy {
        tab: Tab,
        before: usize,
        copy: bool,
    },
    Color {
        tab: Tab,
    },
    /// Unhide: the hidden sheets and the one chosen.
    Unhide {
        chosen: usize,
    },
}

/// The tab commands the command palette runs on the sheet shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabCommand {
    Insert,
    Delete,
    Rename,
    MoveOrCopy,
    TabColor,
    Hide,
    Unhide,
}

/// A hidden sheet Unhide lists.
#[derive(Clone)]
struct Hidden {
    tab: Tab,
    name: String,
}

impl SheetsView {
    /// The tab shown now (a chart sheet or the grid's worksheet).
    pub(crate) fn active_tab(&self, cx: &App) -> Tab {
        match self.chart_tab.filter(|_| self.on_chart_sheet(cx)) {
            Some(k) => Tab::Chart(k),
            None => Tab::Sheet(self.table.read(cx).delegate().sheet),
        }
    }

    fn tab_name(&self, tab: Tab, cx: &App) -> String {
        let book = &self.table.read(cx).delegate().book;
        match tab {
            Tab::Sheet(i) => book
                .sheet_names()
                .get(i as usize)
                .cloned()
                .unwrap_or_default(),
            Tab::Chart(k) => book.chart_sheet_name(k).unwrap_or_default(),
        }
    }

    fn hidden_tabs(&self, cx: &App) -> Vec<Hidden> {
        let book = &self.table.read(cx).delegate().book;
        let mut out: Vec<Hidden> = book
            .hidden_sheets()
            .into_iter()
            .map(|(i, name)| Hidden {
                tab: Tab::Sheet(i),
                name,
            })
            .collect();
        out.extend(
            book.hidden_chart_sheets()
                .into_iter()
                .map(|(k, name)| Hidden {
                    tab: Tab::Chart(k),
                    name,
                }),
        );
        out
    }

    /// Run a tab command on the workbook (one undo step), show the sheet
    /// the workbook selects after it (as Excel: a new or copied sheet, the
    /// next one after a deletion) and say what happened. `false` (and the
    /// reason in the status bar) when it was refused.
    pub(crate) fn tab_edit(
        &mut self,
        done: String,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut sheets_engine::Workbook) -> anyhow::Result<()>,
    ) -> Result<(), String> {
        if self.editing.is_some() {
            self.commit_edit(None, window, cx);
        }
        if self.blocked(cx) {
            return Err(String::new());
        }
        let shown = {
            let grid = self.table.read(cx).delegate();
            grid.book.sheet_id(grid.sheet)
        };
        let res = self.table.update(cx, |table, cx| {
            let r = f(&mut table.delegate_mut().book);
            table.refresh(cx);
            r
        });
        let res = res.map_err(|e| engine_error(&e));
        self.status = match &res {
            Ok(()) => done.into(),
            Err(e) => e.clone().into(),
        };
        if res.is_ok() {
            self.follow_sheet(shown, window, cx);
        }
        self.update_title(window, cx);
        cx.notify();
        res
    }

    /// After sheets were added, removed, moved or hidden (an edit, undo or
    /// redo): show the worksheet the workbook has selected when it isn't
    /// the one with `sheetId` `shown`, or that one where it is now; leave a
    /// chart sheet that went away.
    pub(crate) fn follow_sheet(&mut self, shown: u32, window: &mut Window, cx: &mut Context<Self>) {
        if self.chart_tab.is_some() && !self.on_chart_sheet(cx) {
            self.leave_chart_sheet(window, cx);
        }
        let (now, active, at) = {
            let grid = self.table.read(cx).delegate();
            let book = &grid.book;
            let count = book.sheet_names().len() as u32;
            let at = (0..count).find(|&i| book.sheet_id(i) == shown);
            (grid.sheet, book.active_sheet(), at)
        };
        let target = match at {
            Some(i) if i == active => i,
            _ => active,
        };
        if target != now || at != Some(now) {
            self.show_sheet(target, cx);
            if at != Some(target) {
                self.select_cell(0, 0, cx);
            }
            self.load_selected_into_formula(window, cx);
        }
    }

    /// The sheet the tab menu or a command acts on: the right-clicked tab,
    /// or the one shown.
    fn sheet_of(&self, tab: Option<Tab>, cx: &App) -> Tab {
        tab.unwrap_or_else(|| self.active_tab(cx))
    }

    // ------------------------------------------------------------ commands

    /// A tab command on the sheet shown (the command palette).
    pub fn tab_command(&mut self, cmd: TabCommand, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            TabCommand::Insert => self.insert_sheet_tab(None, window, cx),
            TabCommand::Delete => self.delete_sheet_tab(None, window, cx),
            TabCommand::Rename => self.rename_sheet_tab(None, window, cx),
            TabCommand::MoveOrCopy => self.move_copy_tab(None, window, cx),
            TabCommand::TabColor => self.tab_color_tab(None, cx),
            TabCommand::Hide => self.hide_sheet_tab(None, window, cx),
            TabCommand::Unhide => self.unhide_sheet_tab(cx),
        }
    }

    /// Insert: a new worksheet before `tab` (Excel's tab menu), or after
    /// the shown sheet (`None`: the + button, Shift+F11's place in Kantor).
    pub(crate) fn insert_sheet_tab(
        &mut self,
        tab: Option<Tab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs = self.table.read(cx).delegate().book.tabs();
        let at = match tab {
            Some(tab) => tabs.iter().position(|t| *t == tab).unwrap_or(tabs.len()),
            None => {
                let active = self.active_tab(cx);
                tabs.iter()
                    .position(|t| *t == active)
                    .map_or(tabs.len(), |p| p + 1)
            }
        };
        let inserted = self.tab_edit(t("Inserted a sheet").into(), window, cx, |b| {
            b.insert_sheet(at).map(|_| ())
        });
        self.select_cell(0, 0, cx);
        self.load_selected_into_formula(window, cx);
        self.focus_grid(window, cx);
        if inserted.is_ok() {
            let sheet = self.grid_sheet(cx);
            self.queue_macro_events(vec![sheets_script::Event::NewSheet { sheet }], window, cx);
        }
    }

    /// Delete: asks first when the sheet has something on it, as Excel.
    pub(crate) fn delete_sheet_tab(
        &mut self,
        tab: Option<Tab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self.sheet_of(tab, cx);
        let ask = match tab {
            Tab::Sheet(i) => self.table.read(cx).delegate().book.sheet_has_data(i),
            Tab::Chart(_) => true,
        };
        if ask {
            let name = self.tab_name(tab, cx);
            self.tab_dialog = Some(TabDialog::Delete { tab, name });
            cx.notify();
        } else {
            self.delete_now(tab, window, cx);
        }
    }

    fn delete_now(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.tab_name(tab, cx);
        let done = tf("Deleted “{}” (Undo brings it back)", &[&name]);
        let _ = self.tab_edit(done, window, cx, |b| match tab {
            Tab::Sheet(i) => b.delete_sheet(i),
            Tab::Chart(k) => b.delete_chart_sheet(k),
        });
        self.refocus(window, cx);
    }

    fn confirm_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabDialog::Delete { tab, .. }) = self.tab_dialog.take() {
            self.delete_now(tab, window, cx);
        }
    }

    /// Rename: a prompt over the tabs (also a double click on a tab).
    pub(crate) fn rename_sheet_tab(
        &mut self,
        tab: Option<Tab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let tab = self.sheet_of(tab, cx);
        let name = self.tab_name(tab, cx);
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t("Sheet name")));
        input.update(cx, |i, cx| i.set_value(name, window, cx));
        let sub = cx.subscribe_in(&input, window, Self::on_rename_input);
        // Focused once the menu that asked for it has closed (closing it
        // gives focus back to what had it).
        let focus = input.clone();
        window.defer(cx, move |window, cx| {
            focus.update(cx, |i, cx| i.focus(window, cx));
        });
        self.tab_dialog = Some(TabDialog::Rename {
            tab,
            input,
            error: None,
            _sub: sub,
        });
        cx.notify();
    }

    fn on_rename_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { .. } => {
                let Some(TabDialog::Rename { tab, .. }) = self.tab_dialog else {
                    return;
                };
                let name = input.read(cx).value().trim().to_owned();
                let done = tf("Renamed to “{}”", &[&name]);
                match self.tab_edit(done, window, cx, |b| match tab {
                    Tab::Sheet(i) => b.rename_sheet(i, &name),
                    Tab::Chart(k) => b.rename_chart_sheet(k, &name),
                }) {
                    Ok(()) => self.close_tab_dialog(window, cx),
                    Err(e) => {
                        if let Some(TabDialog::Rename { error, .. }) = self.tab_dialog.as_mut() {
                            *error = Some(e);
                        }
                        cx.notify();
                    }
                }
            }
            InputEvent::Blur => self.close_tab_dialog(window, cx),
            _ => {}
        }
    }

    /// Move or Copy…: the dialog, "Before sheet" on the sheet after it.
    pub(crate) fn move_copy_tab(
        &mut self,
        tab: Option<Tab>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.blocked(cx) {
            return;
        }
        let tab = self.sheet_of(tab, cx);
        let tabs = self.table.read(cx).delegate().book.tabs();
        let before = tabs
            .iter()
            .position(|t| *t == tab)
            .map_or(tabs.len(), |p| p + 1);
        self.tab_dialog = Some(TabDialog::MoveCopy {
            tab,
            before,
            copy: false,
        });
        cx.notify();
    }

    fn apply_move_copy(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(TabDialog::MoveCopy { tab, before, copy }) = self.tab_dialog.take() else {
            return;
        };
        let tabs = self.table.read(cx).delegate().book.tabs();
        let name = self.tab_name(tab, cx);
        let _ = if copy {
            self.tab_edit(tf("Copied “{}”", &[&name]), window, cx, |b| match tab {
                Tab::Sheet(i) => b.copy_sheet(i, before).map(|_| ()),
                Tab::Chart(k) => b.copy_chart_sheet(k, before).map(|_| ()),
            })
        } else {
            let from = tabs.iter().position(|t| *t == tab).unwrap_or(0);
            // Counted once the tab is taken out.
            let to = if before > from { before - 1 } else { before };
            self.tab_edit(t("Moved").into(), window, cx, |b| b.move_tab(tab, to))
        };
        self.refocus(window, cx);
    }

    /// Move Left (`-1`) or Move Right (`1`).
    pub(crate) fn move_tab_by(
        &mut self,
        tab: Tab,
        by: isize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tabs = self.table.read(cx).delegate().book.tabs();
        let Some(at) = tabs.iter().position(|t| *t == tab) else {
            return;
        };
        let Some(to) = at.checked_add_signed(by).filter(|to| *to < tabs.len()) else {
            return;
        };
        let _ = self.tab_edit(t("Moved").into(), window, cx, |b| b.move_tab(tab, to));
        self.refocus(window, cx);
    }

    /// Tab Color…
    pub(crate) fn tab_color_tab(&mut self, tab: Option<Tab>, cx: &mut Context<Self>) {
        let tab = self.sheet_of(tab, cx);
        self.tab_dialog = Some(TabDialog::Color { tab });
        cx.notify();
    }

    fn pick_tab_color(
        &mut self,
        color: Option<TabColor>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(TabDialog::Color { tab }) = self.tab_dialog.take() else {
            return;
        };
        let done = match color {
            Some(_) => t("Tab colour set"),
            None => t("Tab colour removed"),
        };
        let _ = self.tab_edit(done.into(), window, cx, |b| match tab {
            Tab::Sheet(sheet) => b.set_tab_color(sheet, color),
            Tab::Chart(k) => b.set_chart_sheet_tab_color(k, color),
        });
        self.refocus(window, cx);
    }

    /// Hide.
    pub(crate) fn hide_sheet_tab(
        &mut self,
        tab: Option<Tab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = self.sheet_of(tab, cx);
        let name = self.tab_name(tab, cx);
        let _ = self.tab_edit(tf("Hid “{}”", &[&name]), window, cx, |b| match tab {
            Tab::Sheet(i) => b.set_sheet_hidden(i, true),
            Tab::Chart(k) => b.set_chart_sheet_hidden(k, true),
        });
        self.refocus(window, cx);
    }

    /// Unhide…: the list of hidden sheets (none: nothing to do).
    pub(crate) fn unhide_sheet_tab(&mut self, cx: &mut Context<Self>) {
        if self.hidden_tabs(cx).is_empty() {
            self.status = t("No sheet is hidden").into();
        } else {
            self.tab_dialog = Some(TabDialog::Unhide { chosen: 0 });
        }
        cx.notify();
    }

    fn apply_unhide(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(TabDialog::Unhide { chosen }) = self.tab_dialog.take() else {
            return;
        };
        let Some(h) = self.hidden_tabs(cx).get(chosen).cloned() else {
            return;
        };
        let done = tf("Unhid “{}”", &[&h.name]);
        let ok = self
            .tab_edit(done, window, cx, |b| match h.tab {
                Tab::Sheet(i) => b.set_sheet_hidden(i, false),
                Tab::Chart(k) => b.set_chart_sheet_hidden(k, false),
            })
            .is_ok();
        // As Excel: the sheet unhidden is shown.
        if ok {
            match h.tab {
                Tab::Sheet(i) => self.select_sheet(i, window, cx),
                Tab::Chart(k) => self.select_chart_sheet(k, window, cx),
            }
        }
    }

    pub(crate) fn close_tab_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.tab_dialog.take().is_some() {
            self.refocus(window, cx);
            cx.notify();
        }
    }

    fn cancel_tab_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tab_dialog(window, cx);
    }

    // ------------------------------------------------------------ rendering

    /// The open tab dialog, if any.
    pub(crate) fn render_tab_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.tab_dialog.as_ref()?;
        Some(match d {
            TabDialog::Rename { input, error, .. } => self.render_rename(input, error, cx),
            TabDialog::Delete { tab, name } => self.render_delete(*tab, name, cx),
            TabDialog::MoveCopy { tab, before, copy } => {
                self.render_move_copy(*tab, *before, *copy, cx)
            }
            TabDialog::Color { tab } => self.render_tab_colors(*tab, cx),
            TabDialog::Unhide { chosen } => self.render_unhide(*chosen, cx),
        })
    }

    fn render_rename(
        &self,
        input: &Entity<InputState>,
        error: &Option<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t_ = tokens(cx);
        div()
            .absolute()
            .bottom(CONTROL_LG + SPACE_3)
            .left(SPACE_6)
            .w(px(320.))
            .p(SPACE_4)
            .rounded(RADIUS_MD)
            .bg(t_.surface)
            .border_1()
            .border_color(t_.border_strong)
            .shadow_md()
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_tab_dialog(w, cx)))
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t_.fg_muted)
                    .mb(SPACE_3)
                    .child(t("Rename sheet")),
            )
            .child(
                h_flex()
                    .gap(SPACE_3)
                    .child(div().flex_1().child(Input::new(input).small()))
                    .child(
                        div()
                            .text_size(FONT_SMALL)
                            .text_color(t_.fg_dim)
                            .child(t("Enter to apply")),
                    ),
            )
            .children(
                error
                    .clone()
                    .map(|e| div().mt(SPACE_2).child(note(e, true, cx))),
            )
            .into_any_element()
    }

    fn render_delete(&self, tab: Tab, name: &str, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        // Formulas can't refer to a chart sheet: no #REF! to warn about.
        let text = match tab {
            Tab::Sheet(_) => tf(
                "“{}” will be deleted with everything on it. Formulas that use it will show #REF!.",
                &[&name],
            ),
            Tab::Chart(_) => tf("“{}” will be deleted with its chart.", &[&name]),
        };
        let card = dialog::card("tab-delete", t("Delete sheet"), px(380.), cx)
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_tab_dialog(w, cx)))
            .child(div().child(text))
            .child(note(t("Undo brings it back."), false, cx))
            .child(dialog::buttons(
                "tab-delete-buttons",
                t("Delete"),
                true,
                &view,
                Self::confirm_delete,
                Self::cancel_tab_dialog,
            ));
        dialog::place(card, self, cx).into_any_element()
    }

    fn render_move_copy(
        &self,
        tab: Tab,
        before: usize,
        copy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t_ = tokens(cx);
        let view = cx.entity().downgrade();
        let book = &self.table.read(cx).delegate().book;
        let names = book.sheet_names();
        let mut items: Vec<String> = book
            .tabs()
            .into_iter()
            .map(|t| match t {
                Tab::Sheet(i) => names[i as usize].clone(),
                Tab::Chart(k) => book.chart_sheet_name(k).unwrap_or_default(),
            })
            .collect();
        items.push(t("(move to end)").to_owned());
        let name = self.tab_name(tab, cx);
        let list = v_flex()
            .id("tab-move-list")
            .h(px(180.))
            .overflow_y_scroll()
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(t_.border)
            .bg(t_.bg)
            .children(items.into_iter().enumerate().map(|(i, label)| {
                let v = view.clone();
                let on = i == before;
                div()
                    .id(("tab-move-item", i))
                    .px(SPACE_4)
                    .py(SPACE_1)
                    .text_size(FONT_SMALL)
                    .cursor_pointer()
                    .when_else(
                        on,
                        |d| d.bg(t_.accent).text_color(t_.accent_fg),
                        |d| d.hover(|s| s.bg(t_.hover)),
                    )
                    .child(label)
                    .on_click(move |_, _, cx| {
                        let _ = v.update(cx, |this, cx| {
                            if let Some(TabDialog::MoveCopy { before, .. }) =
                                this.tab_dialog.as_mut()
                            {
                                *before = i;
                            }
                            cx.notify();
                        });
                    })
            }));
        let cv = view.clone();
        let card = dialog::card("tab-move", t("Move or Copy"), px(360.), cx)
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_tab_dialog(w, cx)))
            .child(div().child(tf("Move “{}”", &[&name])))
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t_.fg_muted)
                    .child(t("Before sheet:")),
            )
            .child(list)
            .child(
                Checkbox::new("tab-move-copy")
                    .label(t("Create a copy"))
                    .checked(copy)
                    .on_click(move |on, _, cx| {
                        let on = *on;
                        let _ = cv.update(cx, |this, cx| {
                            if let Some(TabDialog::MoveCopy { copy, .. }) = this.tab_dialog.as_mut()
                            {
                                *copy = on;
                            }
                            cx.notify();
                        });
                    }),
            )
            .child(dialog::buttons(
                "tab-move-buttons",
                t("OK"),
                true,
                &view,
                Self::apply_move_copy,
                Self::cancel_tab_dialog,
            ));
        dialog::place(card, self, cx).into_any_element()
    }

    fn render_tab_colors(&self, tab: Tab, cx: &mut Context<Self>) -> AnyElement {
        let t_ = tokens(cx);
        let view = cx.entity().downgrade();
        let book = &self.table.read(cx).delegate().book;
        let current = match tab {
            Tab::Sheet(sheet) => book.tab_color(sheet),
            Tab::Chart(k) => book.chart_sheet_tab_color(k),
        };
        let swatch = |id: usize, color: TabColor, tip: String| {
            let v = view.clone();
            let rgb = book.tab_color_rgb(color);
            let on = current == Some(color);
            div()
                .id(("tab-color", id))
                .size(px(18.))
                .rounded(px(3.))
                .border_1()
                .border_color(if on { t_.accent } else { t_.border_strong })
                .when(on, |d| d.border_2())
                .bg(crate::look::hsla(rgb))
                .cursor_pointer()
                .hover(|s| s.border_color(t_.accent))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx)
                })
                .on_click(move |_, w, cx| {
                    let _ = v.update(cx, |this, cx| this.pick_tab_color(Some(color), w, cx));
                })
        };
        let heading = |s: &'static str| {
            div()
                .text_size(FONT_SMALL)
                .text_color(t_.fg_muted)
                .child(t(s))
        };
        // Theme colours: a row of the colours, then five rows of tints.
        let mut theme = v_flex().gap(SPACE_1);
        for row in 0..6usize {
            let mut r = h_flex().gap(SPACE_1);
            for (c, &slot) in THEME_SLOTS.iter().enumerate() {
                let tint = if row == 0 {
                    0.0
                } else {
                    theme_tints(slot)[row - 1]
                };
                let color = TabColor::Theme { slot, tint };
                let tip = if tint == 0.0 {
                    theme_name(slot)
                } else {
                    let pct = (tint.abs() * 100.0).round();
                    let word = if tint > 0.0 {
                        t("Lighter")
                    } else {
                        t("Darker")
                    };
                    format!("{}, {word} {pct}%", theme_name(slot))
                };
                r = r.child(swatch(row * 10 + c, color, tip));
            }
            theme = theme.child(r);
        }
        let standard = h_flex().gap(SPACE_1).children(
            STANDARD_COLORS
                .iter()
                .enumerate()
                .map(|(i, &rgb)| swatch(100 + i, TabColor::Rgb(rgb), format!("#{rgb:06X}"))),
        );
        let nv = view.clone();
        let card = dialog::card("tab-colors", t("Tab Color"), px(300.), cx)
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_tab_dialog(w, cx)))
            .child(heading(tk("Theme Colors")))
            .child(theme)
            .child(heading(tk("Standard Colors")))
            .child(standard)
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        Button::new("tab-color-none")
                            .small()
                            .outline()
                            .label(t("No Color"))
                            .disabled(current.is_none())
                            .on_click(move |_, w, cx| {
                                let _ = nv.update(cx, |this, cx| this.pick_tab_color(None, w, cx));
                            }),
                    )
                    .child({
                        let v = view.clone();
                        Button::new("tab-color-cancel")
                            .small()
                            .ghost()
                            .label(t("Cancel"))
                            .on_click(move |_, w, cx| {
                                let _ = v.update(cx, |this, cx| this.close_tab_dialog(w, cx));
                            })
                    }),
            );
        dialog::place(card, self, cx).into_any_element()
    }

    fn render_unhide(&self, chosen: usize, cx: &mut Context<Self>) -> AnyElement {
        let t_ = tokens(cx);
        let view = cx.entity().downgrade();
        let hidden = self.hidden_tabs(cx);
        let list = v_flex()
            .id("tab-unhide-list")
            .max_h(px(180.))
            .overflow_y_scroll()
            .rounded(RADIUS_SM)
            .border_1()
            .border_color(t_.border)
            .bg(t_.bg)
            .children(hidden.into_iter().enumerate().map(|(i, h)| {
                let v = view.clone();
                div()
                    .id(("tab-unhide-item", i))
                    .px(SPACE_4)
                    .py(SPACE_1)
                    .text_size(FONT_SMALL)
                    .cursor_pointer()
                    .when_else(
                        i == chosen,
                        |d| d.bg(t_.accent).text_color(t_.accent_fg),
                        |d| d.hover(|s| s.bg(t_.hover)),
                    )
                    .child(h.name)
                    .on_click(move |e, w, cx| {
                        let twice = e.click_count() >= 2;
                        let _ = v.update(cx, |this, cx| {
                            if let Some(TabDialog::Unhide { chosen }) = this.tab_dialog.as_mut() {
                                *chosen = i;
                            }
                            if twice {
                                this.apply_unhide(w, cx);
                            }
                            cx.notify();
                        });
                    })
            }));
        let card = dialog::card("tab-unhide", t("Unhide"), px(320.), cx)
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_tab_dialog(w, cx)))
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t_.fg_muted)
                    .child(t("Unhide sheet:")),
            )
            .child(list)
            .child(dialog::buttons(
                "tab-unhide-buttons",
                t("OK"),
                true,
                &view,
                Self::apply_unhide,
                Self::cancel_tab_dialog,
            ));
        dialog::place(card, self, cx).into_any_element()
    }
}

/// Excel's names of the theme colours.
fn theme_name(slot: u8) -> String {
    match slot {
        0 => t("Background 1").to_owned(),
        1 => t("Text 1").to_owned(),
        2 => t("Background 2").to_owned(),
        3 => t("Text 2").to_owned(),
        n => tf("Accent {}", &[&(n - 3)]),
    }
}

/// The right-click menu of a tab, as Excel's: worksheets and chart sheets
/// alike.
pub(crate) fn tab_menu(
    menu: PopupMenu,
    tab: Tab,
    first: bool,
    last: bool,
    any_hidden: bool,
    view: &WeakEntity<SheetsView>,
) -> PopupMenu {
    let item = |label: &'static str,
                f: fn(&mut SheetsView, Tab, &mut Window, &mut Context<SheetsView>)| {
        let view = view.clone();
        PopupMenuItem::new(t(label)).on_click(move |_, window, cx| {
            let _ = view.update(cx, |v, cx| f(v, tab, window, cx));
        })
    };
    menu.item(item(tk("Insert"), |v, tab, w, cx| {
        v.insert_sheet_tab(Some(tab), w, cx)
    }))
    .item(item(tk("Delete"), |v, tab, w, cx| {
        v.delete_sheet_tab(Some(tab), w, cx)
    }))
    .item(item(tk("Rename"), |v, tab, w, cx| {
        v.rename_sheet_tab(Some(tab), w, cx)
    }))
    .item(item(tk("Move or Copy…"), |v, tab, w, cx| {
        v.move_copy_tab(Some(tab), w, cx)
    }))
    .separator()
    .item(
        item(tk("Move Left"), |v, tab, w, cx| {
            v.move_tab_by(tab, -1, w, cx)
        })
        .disabled(first),
    )
    .item(
        item(tk("Move Right"), |v, tab, w, cx| {
            v.move_tab_by(tab, 1, w, cx)
        })
        .disabled(last),
    )
    .separator()
    .item(item(tk("Tab Color…"), |v, tab, _, cx| {
        v.tab_color_tab(Some(tab), cx)
    }))
    .separator()
    .item(item(tk("Hide"), |v, tab, w, cx| {
        v.hide_sheet_tab(Some(tab), w, cx)
    }))
    .item(item(tk("Unhide…"), |v, _, _, cx| v.unhide_sheet_tab(cx)).disabled(!any_hidden))
}
