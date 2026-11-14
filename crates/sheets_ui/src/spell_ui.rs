//! Review > Spelling (F7) in Sheets, as in Excel: no squiggles in the grid,
//! a dialog that walks through the misspelled words of the text cells
//! (not formulas, numbers, dates or booleans) from the active cell, or of
//! the selection when it spans several cells.
//!
//! The dialog offers Ignore Once, Ignore All, Add to Dictionary, Change and
//! Change All, and a dictionary picker. Change is one undo step; Change All
//! replaces the word in every checked cell as one undo step. Ignore All and
//! Add to Dictionary are shared with Docs and Slides ([`kantor_spell`]).
//!
//! Finding the next misspelled word runs off the UI thread (a big sheet
//! has many words, and the dictionary may still be loading). The card is a
//! sibling of the grid, never inside it (the grid's key handler would take
//! the input's typing).

use std::ops::Range;
use std::sync::Arc;

use gpui_kit::component::{
    Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{self, Input, InputEvent, InputState},
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_spell::{Lang, SpellItem, Speller, words};
use kantor_ui::{metrics::*, tokens};

use crate::{KEY_CONTEXT, SheetsView};

actions!(sheets, [Spelling, SpellCancel, SpellChange]);

const DIALOG_CONTEXT: &str = "KantorSpellDialog";

/// Excel's words when the check reaches the end.
pub(crate) const COMPLETE: &str = "Spell check complete. You're good to go!";

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("f7", Spelling, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", SpellCancel, Some(DIALOG_CONTEXT)),
        KeyBinding::new("enter", SpellChange, Some(DIALOG_CONTEXT)),
    ]);
}

/// A text cell being checked: `(row, col, text)`.
type TextCell = (u32, u32, String);

/// The flagged word: cell `cell` of the dialog's list, `range` in its text.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Found {
    cell: usize,
    range: Range<usize>,
    word: String,
    suggestions: Vec<String>,
}

enum Step {
    /// Looking for the next word (or loading the dictionary).
    Checking,
    Found(Found),
    Done,
}

pub(crate) struct SpellDialog {
    sheet: u32,
    lang: Lang,
    /// The text cells to check, in the order they're checked.
    cells: Arc<Vec<TextCell>>,
    /// Where to look next: cell index and byte offset in its text.
    at: (usize, usize),
    step: Step,
    /// The selected suggestion.
    pick: Option<usize>,
    /// "Not in Dictionary": the flagged word, which can be edited.
    word: Entity<InputState>,
    /// Number of changes made, for the status line.
    changes: usize,
    focus: FocusHandle,
    error: Option<String>,
    _task: Option<Task<()>>,
    _sub: Subscription,
}

/// The text cells in `scope`, starting at the first cell at or after
/// `from` in reading order and wrapping round to the start, as Excel
/// continues "at the beginning of the sheet".
fn ordered(mut cells: Vec<TextCell>, from: (u32, u32)) -> Vec<TextCell> {
    let start = cells.iter().position(|c| (c.0, c.1) >= from).unwrap_or(0);
    cells.rotate_left(start);
    cells
}

/// The next misspelled word at or after `at` (cell index, byte offset).
fn next_misspelling(
    speller: &Speller,
    lang: Lang,
    cells: &[TextCell],
    at: (usize, usize),
) -> Option<(usize, Range<usize>)> {
    for (i, (_, _, text)) in cells.iter().enumerate().skip(at.0) {
        let from = if i == at.0 { at.1 } else { 0 };
        let bad = words(text)
            .into_iter()
            .find(|r| r.start >= from && speller.check(lang, &text[r.clone()]) == Some(false));
        if let Some(r) = bad {
            return Some((i, r));
        }
    }
    None
}

/// `text` with every whole word equal to `word` replaced by `with`, if any.
fn replace_word(text: &str, word: &str, with: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for r in words(text) {
        if &text[r.clone()] == word {
            out.push_str(&text[last..r.start]);
            out.push_str(with);
            last = r.end;
        }
    }
    (last > 0).then(|| {
        out.push_str(&text[last..]);
        out
    })
}

/// The cell input that makes a text cell hold `new` again: `input` is
/// what the cell showed in the formula bar for its old text `old` (it may
/// start with a quote). A text that would read as a number, a boolean or
/// a formula keeps a leading quote, so it stays text.
fn text_input(input: &str, old: &str, new: &str) -> Option<String> {
    let prefix = input.strip_suffix(old)?;
    if !prefix.is_empty() {
        return Some(format!("{prefix}{new}"));
    }
    let t = new.trim();
    let reads_as_value = t.starts_with(['=', '+', '-', '@'])
        || t.parse::<f64>().is_ok()
        || t.eq_ignore_ascii_case("true")
        || t.eq_ignore_ascii_case("false");
    Some(if reads_as_value {
        format!("'{new}")
    } else {
        new.to_owned()
    })
}

impl SheetsView {
    /// Review > Spelling (F7).
    pub fn spelling_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let lang = self
            .spell_dialog
            .as_ref()
            .map_or_else(|| kantor_ui::spelling::default_lang(cx), |d| d.lang);
        let (sheet, cells) = {
            let grid = self.table.read(cx).delegate();
            let sel = grid.range();
            let mut cells = grid.book.text_cells(grid.sheet);
            let from = if sel.is_single() {
                self.selected
            } else {
                cells.retain(|c| sel.contains(c.0, c.1));
                (sel.top, sel.left)
            };
            (grid.sheet, ordered(cells, from))
        };
        let word = cx.new(|cx| InputState::new(window, cx));
        let sub = cx.subscribe_in(&word, window, |this, _, ev: &InputEvent, w, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.spell_change(false, w, cx);
            }
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        self.spell_dialog = Some(SpellDialog {
            sheet,
            lang,
            cells: Arc::new(cells),
            at: (0, 0),
            step: Step::Checking,
            pick: None,
            word,
            changes: 0,
            focus,
            error: None,
            _task: None,
            _sub: sub,
        });
        self.spell_next(window, cx);
    }

    /// Look for the next misspelled word in the background, then show it.
    fn spell_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.spell_dialog.as_mut() else {
            return;
        };
        d.step = Step::Checking;
        d.pick = None;
        let (cells, lang, at) = (d.cells.clone(), d.lang, d.at);
        let task = cx.spawn_in(window, async move |this, cx| {
            let found = cx
                .background_spawn(async move {
                    let speller = Speller::global();
                    speller.load(lang);
                    next_misspelling(speller, lang, &cells, at).map(|(i, r)| {
                        let word = cells[i].2[r.clone()].to_owned();
                        let suggestions = speller
                            .suggest(lang, &word)
                            .into_iter()
                            .filter(|s| *s != word)
                            .collect();
                        Found {
                            cell: i,
                            range: r,
                            word,
                            suggestions,
                        }
                    })
                })
                .await;
            let _ = this.update_in(cx, |this, window, cx| this.spell_show(found, window, cx));
        });
        d._task = Some(task);
        cx.notify();
    }

    fn spell_show(&mut self, found: Option<Found>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.spell_dialog.as_mut() else {
            return;
        };
        d._task = None;
        let Some(f) = found else {
            d.step = Step::Done;
            if d.changes > 0 {
                self.status = format!(
                    "Spelling: {} change{}",
                    d.changes,
                    if d.changes == 1 { "" } else { "s" }
                )
                .into();
            }
            cx.notify();
            return;
        };
        d.pick = (!f.suggestions.is_empty()).then_some(0);
        let (row, col) = (d.cells[f.cell].0, d.cells[f.cell].1);
        let word = f.word.clone();
        d.word
            .update(cx, |i, cx| i.set_value(word.clone(), window, cx));
        d.step = Step::Found(f);
        let focus = d.focus.clone();
        window.focus(&focus, cx);
        if self.selected != (row, col) {
            self.select_cell(row, col, cx);
            self.selected = (row, col);
        }
        cx.notify();
    }

    fn close_spelling(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.spell_dialog.take().is_some() {
            self.focus_grid(window, cx);
            cx.notify();
        }
    }

    /// The flagged word, if the dialog is showing one on the visible sheet
    /// (another sheet chosen meanwhile starts the check again there).
    fn spell_found(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Found> {
        let d = self.spell_dialog.as_ref()?;
        if d.sheet != self.table.read(cx).delegate().sheet {
            self.spelling_doc(window, cx);
            return None;
        }
        match &d.step {
            Step::Found(f) => Some(f.clone()),
            _ => None,
        }
    }

    /// Ignore Once, Ignore All or Add to Dictionary.
    fn spell_skip(&mut self, item: Option<SpellItem>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.spell_found(window, cx) else {
            return;
        };
        if let Some(item) = item {
            kantor_ui::spelling::accept_word(&item, &f.word, cx);
        }
        if let Some(d) = self.spell_dialog.as_mut() {
            d.at = (f.cell, f.range.end);
        }
        self.spell_next(window, cx);
    }

    /// Change (or Change All) the flagged word to the edited word or the
    /// chosen suggestion.
    fn spell_change(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.spell_found(window, cx) else {
            return;
        };
        let Some(d) = self.spell_dialog.as_ref() else {
            return;
        };
        let typed = d.word.read(cx).value().to_string();
        let with = if typed != f.word && !typed.trim().is_empty() {
            typed
        } else if let Some(s) = d.pick.and_then(|i| f.suggestions.get(i)) {
            s.clone()
        } else {
            return;
        };
        let sheet = d.sheet;
        // New texts for the cells that change, by index in the list.
        let new_texts: Vec<(usize, String)> = if all {
            d.cells
                .iter()
                .enumerate()
                .filter_map(|(i, c)| replace_word(&c.2, &f.word, &with).map(|t| (i, t)))
                .collect()
        } else {
            let old = &d.cells[f.cell].2;
            let t = format!("{}{with}{}", &old[..f.range.start], &old[f.range.end..]);
            vec![(f.cell, t)]
        };
        let (inputs, stale) = {
            let grid = self.table.read(cx).delegate();
            let mut inputs = Vec::new();
            let mut stale = false;
            for (i, new) in &new_texts {
                let (r, c, old) = &d.cells[*i];
                match text_input(&grid.book.input(sheet, *r, *c), old, new) {
                    Some(input) => inputs.push((*r, *c, input)),
                    // Edited since the check began: leave it.
                    None => stale |= *i == f.cell,
                }
            }
            (inputs, stale)
        };
        if stale {
            // The cell changed under the dialog: check it again as it is now.
            self.spelling_doc(window, cx);
            return;
        }
        // Go on after the replacement. Change All also replaced the word
        // earlier in that cell, which moved it.
        let next_at = {
            let old = &d.cells[f.cell].2;
            let before = if all {
                words(old)
                    .into_iter()
                    .filter(|r| r.end <= f.range.start && old[r.clone()] == f.word)
                    .count()
            } else {
                0
            };
            let start = (f.range.start + before * with.len()) - before * f.word.len();
            (f.cell, start + with.len())
        };
        let n = inputs.len();
        let result = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let res = grid.book.set_inputs(sheet, &inputs);
            grid.resize();
            table.refresh(cx);
            res
        });
        if let Err(err) = result {
            if let Some(d) = self.spell_dialog.as_mut() {
                d.error = Some(format!("{err:#}"));
            }
            cx.notify();
            return;
        }
        if let Some(d) = self.spell_dialog.as_mut() {
            let cells = Arc::make_mut(&mut d.cells);
            for (i, new) in new_texts {
                cells[i].2 = new;
            }
            d.changes += n;
            d.error = None;
            d.at = next_at;
        }
        self.load_selected_into_formula(window, cx);
        self.update_title(window, cx);
        self.spell_next(window, cx);
    }

    fn spell_set_lang(&mut self, lang: Lang, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.spell_dialog.as_mut() else {
            return;
        };
        if d.lang == lang {
            return;
        }
        d.lang = lang;
        // Check the flagged word again in the new language.
        if let Step::Found(f) = &d.step {
            d.at = (f.cell, f.range.start);
        }
        if !matches!(d.step, Step::Done) {
            self.spell_next(window, cx);
        }
    }

    pub(crate) fn spell_actions(&self, root: Div, cx: &mut Context<Self>) -> Div {
        root.on_action(cx.listener(|this, _: &Spelling, w, cx| this.spelling_doc(w, cx)))
    }

    pub(crate) fn render_spell_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.spell_dialog.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let button = |id: &'static str, label: &'static str, enabled: bool, run: Run| {
            let v = view.clone();
            Button::new(id)
                .small()
                .outline()
                .w_full()
                .label(label)
                .disabled(!enabled)
                .on_click(move |_, w, cx| {
                    let _ = v.update(cx, |this, cx| run(this, w, cx));
                })
        };
        type Run = fn(&mut SheetsView, &mut Window, &mut Context<SheetsView>);

        let lang_view = view.clone();
        let current = d.lang;
        let lang_picker = Button::new("spell-lang")
            .small()
            .outline()
            .label(current.label())
            .dropdown_caret(true)
            .dropdown_menu(move |mut menu, _, _| {
                for lang in Lang::ALL {
                    let v = lang_view.clone();
                    menu = menu.item(
                        PopupMenuItem::new(lang.label())
                            .checked(lang == current)
                            .on_click(move |_, w, cx| {
                                let _ = v.update(cx, |this, cx| this.spell_set_lang(lang, w, cx));
                            }),
                    );
                }
                menu
            });
        let label = |text: &'static str| {
            div()
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(text)
        };

        let body = match &d.step {
            Step::Done => v_flex()
                .gap(SPACE_5)
                .child(div().child(COMPLETE))
                .child(
                    h_flex().justify_end().child(
                        Button::new("spell-ok")
                            .small()
                            .primary()
                            .label("OK")
                            .on_click({
                                let v = view.clone();
                                move |_, w, cx| {
                                    let _ = v.update(cx, |this, cx| this.close_spelling(w, cx));
                                }
                            }),
                    ),
                )
                .into_any_element(),
            step => {
                let found = match step {
                    Step::Found(f) => Some(f),
                    _ => None,
                };
                let ready = found.is_some();
                let mut list = v_flex()
                    .id("spell-suggestions")
                    .h(px(132.))
                    .overflow_y_scroll()
                    .p(SPACE_1)
                    .rounded(RADIUS_SM)
                    .bg(t.bg)
                    .border_1()
                    .border_color(t.border);
                match found {
                    Some(f) if f.suggestions.is_empty() => {
                        list = list.child(
                            div()
                                .px(SPACE_3)
                                .py(SPACE_1)
                                .text_color(t.fg_dim)
                                .child("(No Suggestions)"),
                        );
                    }
                    Some(f) => {
                        for (i, s) in f.suggestions.iter().enumerate() {
                            let v = view.clone();
                            let picked = d.pick == Some(i);
                            list = list.child(
                                div()
                                    .id(("spell-sug", i))
                                    .px(SPACE_3)
                                    .py(SPACE_1)
                                    .rounded(RADIUS_SM)
                                    .when(picked, |r| r.bg(t.selected))
                                    .when(!picked, |r| r.hover(|r| r.bg(t.hover)))
                                    .cursor_pointer()
                                    .child(s.clone())
                                    .on_click(move |ev, w, cx| {
                                        let twice = ev.click_count() >= 2;
                                        let _ = v.update(cx, |this, cx| {
                                            if let Some(d) = this.spell_dialog.as_mut() {
                                                d.pick = Some(i);
                                                // The pick wins over an
                                                // edited word.
                                                if let Step::Found(f) = &d.step {
                                                    let word = f.word.clone();
                                                    d.word.update(cx, |i, cx| {
                                                        i.set_value(word, w, cx)
                                                    });
                                                }
                                            }
                                            if twice {
                                                this.spell_change(false, w, cx);
                                            }
                                            cx.notify();
                                        });
                                    }),
                            );
                        }
                    }
                    None => {
                        list = list.child(
                            div()
                                .px(SPACE_3)
                                .py(SPACE_1)
                                .text_color(t.fg_dim)
                                .child("Checking…"),
                        );
                    }
                }
                let left = v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(SPACE_2)
                    .child(label("Not in Dictionary:"))
                    .child(Input::new(&d.word).small().disabled(!ready))
                    .child(div().h(SPACE_2))
                    .child(label("Suggestions:"))
                    .child(list);
                let right = v_flex()
                    .flex_none()
                    .w(px(150.))
                    .gap(SPACE_2)
                    .pt(px(18.))
                    .child(button("spell-ignore", "Ignore Once", ready, |v, w, cx| {
                        v.spell_skip(None, w, cx)
                    }))
                    .child(button(
                        "spell-ignore-all",
                        "Ignore All",
                        ready,
                        |v, w, cx| v.spell_skip(Some(SpellItem::IgnoreAll), w, cx),
                    ))
                    .child(button(
                        "spell-add",
                        "Add to Dictionary",
                        ready,
                        |v, w, cx| v.spell_skip(Some(SpellItem::AddToDictionary), w, cx),
                    ))
                    .child(div().h(px(26.)))
                    .child(button("spell-change", "Change", ready, |v, w, cx| {
                        v.spell_change(false, w, cx)
                    }))
                    .child(button(
                        "spell-change-all",
                        "Change All",
                        ready,
                        |v, w, cx| v.spell_change(true, w, cx),
                    ));
                v_flex()
                    .gap(SPACE_5)
                    .child(h_flex().items_start().gap(SPACE_5).child(left).child(right))
                    .children(d.error.clone().map(|e| crate::dialog::note(e, true, cx)))
                    .child(
                        h_flex()
                            .gap(SPACE_3)
                            .child(label("Dictionary language:"))
                            .child(lang_picker)
                            .child(div().flex_1())
                            .child(
                                Button::new("spell-cancel")
                                    .small()
                                    .ghost()
                                    .label("Cancel")
                                    .on_click({
                                        let v = view.clone();
                                        move |_, w, cx| {
                                            let _ =
                                                v.update(cx, |this, cx| this.close_spelling(w, cx));
                                        }
                                    }),
                            ),
                    )
                    .into_any_element()
            }
        };
        let title = match d.lang {
            Lang::EnUs => "Spelling: English (United States)",
            Lang::IdId => "Spelling: Indonesian",
        };
        let card = crate::dialog::card("spelling", title, px(470.), cx)
            .track_focus(&d.focus)
            .key_context(DIALOG_CONTEXT)
            .on_action(cx.listener(|this, _: &SpellCancel, w, cx| this.close_spelling(w, cx)))
            .on_action(cx.listener(|this, _: &input::Escape, w, cx| this.close_spelling(w, cx)))
            .on_action(cx.listener(|this, _: &SpellChange, w, cx| {
                if matches!(
                    this.spell_dialog.as_ref().map(|d| &d.step),
                    Some(Step::Done)
                ) {
                    this.close_spelling(w, cx);
                } else {
                    this.spell_change(false, w, cx);
                }
            }))
            .child(body);
        // On the right, so the flagged cell (selected and scrolled into
        // view) usually stays visible beside it.
        Some(
            div()
                .absolute()
                .top(CONTROL_LG * 2. + SPACE_5)
                .right(SPACE_6)
                .child(card)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speller() -> &'static Speller {
        static S: std::sync::OnceLock<Speller> = std::sync::OnceLock::new();
        S.get_or_init(|| Speller::with_personal(None))
    }

    fn cells(texts: &[(u32, u32, &str)]) -> Vec<TextCell> {
        texts
            .iter()
            .map(|&(r, c, t)| (r, c, t.to_owned()))
            .collect()
    }

    #[std::prelude::v1::test]
    fn walks_the_misspelled_words_from_the_active_cell_and_wraps() {
        let s = speller();
        s.load(Lang::EnUs);
        let list = ordered(
            cells(&[
                (0, 0, "Teh header"),
                (1, 0, "fine words"),
                (1, 1, "a recieve and mesage"),
                (3, 2, "wrld"),
            ]),
            (1, 1),
        );
        assert_eq!(list[0].2, "a recieve and mesage");
        assert_eq!(list[3].2, "fine words");
        let mut at = (0, 0);
        let mut seen = Vec::new();
        while let Some((i, r)) = next_misspelling(s, Lang::EnUs, &list, at) {
            seen.push(list[i].2[r.clone()].to_owned());
            at = (i, r.end);
        }
        assert_eq!(seen, ["recieve", "mesage", "wrld", "Teh"]);
        // A selection after every text cell starts from the first one.
        assert_eq!(ordered(cells(&[(0, 0, "x")]), (9, 9))[0].0, 0);
    }

    #[std::prelude::v1::test]
    fn replaces_whole_words_and_keeps_text_cells_text() {
        assert_eq!(
            replace_word("teh cat, teh hat; tehx", "teh", "the").as_deref(),
            Some("the cat, the hat; tehx")
        );
        assert_eq!(replace_word("nothing here", "teh", "the"), None);
        assert_eq!(text_input("Hte", "Hte", "The").as_deref(), Some("The"));
        // A quoted text keeps its quote; one that would become a value gets
        // one.
        assert_eq!(
            text_input("'1 Jna", "1 Jna", "1 Jan").as_deref(),
            Some("'1 Jan")
        );
        assert_eq!(text_input("Ture", "Ture", "True").as_deref(), Some("'True"));
        // The cell was edited meanwhile.
        assert_eq!(text_input("other", "Hte", "The"), None);
    }
}
