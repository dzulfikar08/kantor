//! Custom Lists, as Excel's dialog: Excel's built-in day and month lists
//! (read only) and the user's own, which are typed (one item per line, or
//! separated by commas) or imported from the selected cells. They are kept
//! in Kantor's settings (`[sheets] custom_lists`), so every workbook has
//! them: the Sort panel's Order sorts by them and the fill handle continues
//! them (`Low` → `Medium` → `High`).

use gpui_kit::component::{
    Sizable as _,
    button::Button,
    h_flex,
    input::{self, Input, InputState, Textarea, TextareaState},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::settings::Settings;
use kantor_ui::i18n::t;
use kantor_ui::{metrics::*, tokens};
use sheets_engine::{BUILTIN_LISTS, CellRange, parse_custom_list};

use crate::SheetsView;
use crate::dialog::{self, note, row};

/// The user's lists from Kantor's settings, as items.
pub(crate) fn user_lists() -> Vec<Vec<String>> {
    Settings::load()
        .sheets
        .custom_lists
        .iter()
        .map(|l| parse_custom_list(l))
        .filter(|l| !l.is_empty())
        .collect()
}

/// Every list Sort ▸ Order offers: Excel's built-in ones, then the user's.
pub(crate) fn all_lists() -> Vec<Vec<String>> {
    BUILTIN_LISTS
        .iter()
        .map(|l| parse_custom_list(l))
        .chain(user_lists())
        .collect()
}

/// A list as the dialog shows it: `Low, Medium, High`.
pub(crate) fn list_label(items: &[String]) -> String {
    items.join(", ")
}

/// The Custom Lists card.
pub(crate) struct CustomListsDialog {
    /// The user's lists (built-in ones aren't edited).
    lists: Vec<Vec<String>>,
    /// The list picked on the left: `None` is NEW LIST; built-in lists
    /// come first.
    selected: Option<usize>,
    entries: Entity<TextareaState>,
    import: Entity<InputState>,
    /// The sort level to set to the picked list on OK (opened from the
    /// Sort panel's Order).
    for_level: Option<usize>,
    error: Option<String>,
}

impl SheetsView {
    /// Open Custom Lists (`for_level`: from that Sort panel level).
    pub fn custom_lists_doc(
        &mut self,
        for_level: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() {
            return;
        }
        let entries = cx.new(|cx| TextareaState::new(window, cx).auto_grow(8, 8));
        let range = {
            let grid = self.table.read(cx).delegate();
            let r = grid.range();
            if r.is_single() {
                String::new()
            } else {
                format!(
                    "{}:{}",
                    sheets_engine::cell_name(r.top, r.left),
                    sheets_engine::cell_name(r.bottom, r.right)
                )
            }
        };
        let import = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder(t("e.g. A1:A5"));
            s.set_value(range, window, cx);
            s
        });
        self.custom_lists = Some(CustomListsDialog {
            lists: user_lists(),
            selected: None,
            entries: entries.clone(),
            import,
            for_level,
            error: None,
        });
        // After the palette or menu that opened it closes (it gives focus
        // back to what had it).
        window.defer(cx, move |window, cx| {
            entries.update(cx, |e, cx| e.focus(window, cx));
        });
        cx.notify();
    }

    fn close_custom_lists(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.custom_lists.take().is_some() {
            if self.sort_panel.is_none() {
                self.focus_grid(window, cx);
            }
            cx.notify();
        }
    }

    /// Pick list `i` on the left (`None`: NEW LIST): its items go to List
    /// entries.
    fn pick_custom_list(&mut self, i: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.custom_lists.as_mut() else {
            return;
        };
        d.selected = i;
        d.error = None;
        let text = match i {
            None => String::new(),
            Some(i) if i < BUILTIN_LISTS.len() => parse_custom_list(BUILTIN_LISTS[i]).join("\n"),
            Some(i) => d
                .lists
                .get(i - BUILTIN_LISTS.len())
                .map(|l| l.join("\n"))
                .unwrap_or_default(),
        };
        let entries = d.entries.clone();
        entries.update(cx, |e, cx| e.set_value(text, window, cx));
        cx.notify();
    }

    /// Add (NEW LIST) or change (a list of the user's) from List entries.
    fn add_custom_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.custom_lists.as_mut() else {
            return;
        };
        let items = parse_custom_list(&d.entries.read(cx).value());
        if items.len() < 2 {
            d.error =
                Some(t("Type at least two entries, one per line or separated by commas").into());
            cx.notify();
            return;
        }
        let at = match d.selected {
            Some(i) if i >= BUILTIN_LISTS.len() && i - BUILTIN_LISTS.len() < d.lists.len() => {
                d.lists[i - BUILTIN_LISTS.len()] = items;
                i
            }
            _ => {
                d.lists.push(items);
                BUILTIN_LISTS.len() + d.lists.len() - 1
            }
        };
        self.pick_custom_list(Some(at), window, cx);
    }

    /// Delete the picked list (the user's lists only).
    fn delete_custom_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.custom_lists.as_mut() else {
            return;
        };
        match d.selected {
            Some(i) if i >= BUILTIN_LISTS.len() => {
                d.lists.remove(i - BUILTIN_LISTS.len());
                self.pick_custom_list(None, window, cx);
            }
            _ => {
                d.error = Some(t("Excel's built-in lists can't be deleted").into());
                cx.notify();
            }
        }
    }

    /// Import: the cells of the range typed (a row or a column) become a new
    /// list, as their values show.
    fn import_custom_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.custom_lists.as_ref() else {
            return;
        };
        let text = d.import.read(cx).value().to_string();
        let grid = self.table.read(cx).delegate();
        let range = crate::dialog::parse_cells(&text.replace('$', ""));
        let items: Option<Vec<String>> = range.map(|r: CellRange| {
            let mut out = Vec::new();
            for row in r.top..=r.bottom {
                for col in r.left..=r.right {
                    let v = grid.book.display_value(grid.sheet, row, col);
                    if !v.trim().is_empty() {
                        out.push(v.trim().to_owned());
                    }
                }
            }
            out
        });
        let Some(d) = self.custom_lists.as_mut() else {
            return;
        };
        match items {
            Some(items) if items.len() >= 2 => {
                d.lists.push(items);
                let at = BUILTIN_LISTS.len() + d.lists.len() - 1;
                self.pick_custom_list(Some(at), window, cx);
            }
            Some(_) => {
                d.error = Some(t("Those cells hold fewer than two entries").into());
                cx.notify();
            }
            None => {
                d.error = Some(t("Type the cells to import, such as A1:A5").into());
                cx.notify();
            }
        }
    }

    /// OK: keep the lists in Kantor's settings; from the Sort panel, sort
    /// that level by the picked list.
    fn custom_lists_ok(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.custom_lists.as_ref() else {
            return;
        };
        // Entries typed for NEW LIST and not added yet are added, as Excel's
        // OK does.
        let typed = parse_custom_list(&d.entries.read(cx).value());
        if d.selected.is_none() && typed.len() >= 2 {
            self.add_custom_list(window, cx);
        }
        let Some(d) = self.custom_lists.as_ref() else {
            return;
        };
        let lists = d.lists.clone();
        let picked = d.selected.and_then(|i| {
            if i < BUILTIN_LISTS.len() {
                Some(parse_custom_list(BUILTIN_LISTS[i]))
            } else {
                lists.get(i - BUILTIN_LISTS.len()).cloned()
            }
        });
        let level = d.for_level;
        let mut s = Settings::load();
        let saved: Vec<String> = lists.iter().map(|l| list_label(l)).collect();
        if s.sheets.custom_lists != saved {
            s.sheets.custom_lists = saved;
            if let Err(e) = s.save() {
                log::error!("saving settings: {e:#}");
                if let Some(d) = self.custom_lists.as_mut() {
                    d.error = Some(format!("{e:#}"));
                }
                cx.notify();
                return;
            }
        }
        self.table.update(cx, |table, _| {
            table.delegate_mut().book.set_custom_lists(lists.clone());
        });
        if let (Some(level), Some(list)) = (level, picked) {
            self.set_sort_level_list(level, list, cx);
        }
        self.status = t("Custom lists saved").into();
        self.close_custom_lists(window, cx);
        self.update_title(window, cx);
    }

    pub(crate) fn render_custom_lists(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.custom_lists.as_ref()?;
        let view = cx.entity().downgrade();
        let t_ = tokens(cx);
        // The lists on the left: NEW LIST, Excel's, the user's.
        let mut names = v_flex()
            .id("custom-lists-names")
            .w(px(250.))
            .h(px(200.))
            .overflow_y_scroll()
            .border_1()
            .border_color(t_.border)
            .rounded(RADIUS_SM)
            .p(SPACE_1);
        let entries: Vec<(Option<usize>, String)> =
            std::iter::once((None, t("NEW LIST").to_owned()))
                .chain(
                    BUILTIN_LISTS
                        .iter()
                        .enumerate()
                        .map(|(i, l)| (Some(i), (*l).to_owned())),
                )
                .chain(
                    d.lists
                        .iter()
                        .enumerate()
                        .map(|(i, l)| (Some(BUILTIN_LISTS.len() + i), list_label(l))),
                )
                .collect();
        for (k, (i, label)) in entries.into_iter().enumerate() {
            let v = view.clone();
            let on = d.selected == i;
            names = names.child(
                div()
                    .id(("custom-list", k))
                    .px(SPACE_2)
                    .py(px(2.))
                    .rounded(RADIUS_SM)
                    .text_size(FONT_SMALL)
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .when(on, |d| d.bg(t_.selected))
                    .hover(|d| d.bg(t_.hover))
                    .child(label)
                    .on_click(move |_, window, cx| {
                        let _ = v.update(cx, |this, cx| this.pick_custom_list(i, window, cx));
                    }),
            );
        }
        let builtin = d.selected.is_some_and(|i| i < BUILTIN_LISTS.len());
        let button =
            |id: &'static str,
             label: &'static str,
             run: fn(&mut SheetsView, &mut Window, &mut Context<SheetsView>)| {
                let v = view.clone();
                Button::new(id)
                    .outline()
                    .small()
                    .label(label)
                    .on_click(move |_, window, cx| {
                        let _ = v.update(cx, |this, cx| run(this, window, cx));
                    })
            };
        let add_label = if d.selected.is_some() && !builtin {
            t("Change")
        } else {
            t("Add")
        };
        let card = dialog::card("custom-lists", t("Custom Lists"), px(600.), cx)
            .key_context("KantorCustomLists")
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_custom_lists(w, cx)))
            .child(
                h_flex()
                    .gap(SPACE_5)
                    .items_start()
                    .child(
                        v_flex()
                            .gap(SPACE_2)
                            .child(note(t("Custom lists:"), false, cx))
                            .child(names),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .gap(SPACE_2)
                            .child(note(t("List entries:"), false, cx))
                            .child(Textarea::new(&d.entries).small())
                            .child(
                                h_flex()
                                    .gap(SPACE_2)
                                    .child(button("custom-list-add", add_label, Self::add_custom_list))
                                    .child(button(
                                        "custom-list-delete",
                                        t("Delete"),
                                        Self::delete_custom_list,
                                    )),
                            ),
                    ),
            )
            .child(note(
                t("Type one entry per line (or separate them with commas), then Add. Sort ▸ Order and the fill handle use these lists in every workbook."),
                false,
                cx,
            ))
            .child(row(
                t("Import list from cells"),
                h_flex()
                    .gap(SPACE_2)
                    .child(div().flex_1().child(Input::new(&d.import).small()))
                    .child(button("custom-list-import", t("Import"), Self::import_custom_list)),
                cx,
            ))
            .children(d.error.clone().map(|e| note(e, true, cx)))
            .child(dialog::buttons(
                "custom-lists-buttons",
                t("OK"),
                true,
                &view,
                Self::custom_lists_ok,
                Self::close_custom_lists,
            ));
        Some(dialog::place(card, self, cx).into_any_element())
    }
}
