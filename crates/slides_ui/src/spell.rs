//! Spell checking in the text box being edited: the same red wavy
//! underlines and right-click suggestions as Docs (see
//! [`kantor_ui::spelling`]). A text box is small, so its paragraphs are
//! checked right away (cached by content); only loading a dictionary for
//! the first time happens in the background.

use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use kantor_spell::{Lang, SpellItem, SpellMenu, Speller};
use slides_engine::{Position, Selection, TextPara};

use crate::SlidesView;

#[derive(Default)]
pub(crate) struct SpellState {
    cache: HashMap<u64, Arc<[Range<usize>]>>,
    loading: bool,
    /// The caret is where the user is typing (don't flag that word yet).
    pub typing: bool,
    /// The flagged word under the last right-click: `(shape, paragraph, menu)`.
    pub menu: Option<(usize, usize, SpellMenu)>,
}

/// Which dictionary checks each run: its `lang`, else `default`. PowerPoint
/// (and Kantor's own templates) write `lang="en-US"` on nearly every run
/// whatever the text is, so that one counts as "not set"; `noProof` runs and
/// languages without a bundled dictionary aren't checked.
pub(crate) fn dictionaries(p: &TextPara, default: Lang) -> Vec<(Range<usize>, Option<Lang>)> {
    let mut at = 0;
    p.runs
        .iter()
        .map(|run| {
            let range = at..at + run.len;
            at += run.len;
            let attr = |name: &str| {
                run.props
                    .extra
                    .attrs
                    .iter()
                    .find(|(k, _)| k == name)
                    .map(|(_, v)| v.as_str())
            };
            let lang = if matches!(attr("noProof"), Some("1" | "true")) {
                None
            } else {
                match attr("lang") {
                    None | Some("en-US") => Some(default),
                    Some(tag) => Lang::from_tag(tag),
                }
            };
            (range, lang)
        })
        .collect()
}

impl SlidesView {
    /// Misspelled words in the edited text box: `(paragraph, range)`, empty
    /// while not editing, switched off or loading a dictionary.
    pub(crate) fn misspelled_in_edit(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Vec<(usize, Range<usize>)> {
        let Some(e) = &self.editing else {
            return Vec::new();
        };
        if !kantor_ui::spelling::enabled(cx) {
            return Vec::new();
        }
        let Some(paras) = self.pres.text_paras((self.current, e.shape)) else {
            return Vec::new();
        };
        let default = kantor_ui::spelling::default_lang(cx);
        let sp = Speller::global();
        let caret = e.sel.is_empty().then_some(e.sel.head);
        let mut out = Vec::new();
        let mut missing = None;
        for (i, p) in paras.iter().enumerate() {
            let spans = dictionaries(p, default);
            if let Some(l) = spans.iter().filter_map(|s| s.1).find(|l| !sp.is_loaded(*l)) {
                missing = Some(l);
                continue;
            }
            let mut h = std::collections::hash_map::DefaultHasher::new();
            p.hash(&mut h);
            default.hash(&mut h);
            let flags = self
                .spell
                .cache
                .entry(h.finish())
                .or_insert_with(|| sp.misspelled_in(&p.text, &spans).into());
            for r in flags.iter() {
                let typing = self.spell.typing
                    && caret
                        .is_some_and(|c| c.para == i && r.start <= c.offset && c.offset <= r.end);
                if !typing && !sp.is_accepted(&p.text[r.clone()]) {
                    out.push((i, r.clone()));
                }
            }
        }
        if self.spell.cache.len() > 512 {
            self.spell.cache.clear();
        }
        if let Some(lang) = missing
            && !self.spell.loading
        {
            self.spell.loading = true;
            let task = cx.background_spawn(async move {
                Speller::global().load(lang);
            });
            cx.spawn(async move |this, cx| {
                task.await;
                let _ = this.update(cx, |this, cx| {
                    this.spell.loading = false;
                    cx.notify();
                });
            })
            .detach();
        }
        out
    }

    /// Right mouse button down while editing text: put the caret there
    /// (unless inside the selection) and remember a flagged word under it.
    pub(crate) fn on_right_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.spell.menu = None;
        let Some(e) = self.editing.clone() else {
            return;
        };
        let Some(pos) = self.text_hit(e.shape, ev.position) else {
            return;
        };
        let (a, b) = e.sel.ordered();
        if (e.sel.is_empty() || pos < a || pos > b)
            && let Some(ed) = &mut self.editing
        {
            ed.sel = Selection::caret(pos);
            ed.goal_x = None;
            ed.typing_props = None;
        }
        self.spell.typing = false;
        let flagged: Vec<Range<usize>> = self
            .misspelled_in_edit(cx)
            .into_iter()
            .filter(|(p, _)| *p == pos.para)
            .map(|(_, r)| r)
            .collect();
        if let Some(p) = self
            .pres
            .text_paras((self.current, e.shape))
            .and_then(|ps| ps.get(pos.para))
            && let Some(r) = kantor_spell::flagged_at(&flagged, pos.offset)
        {
            let spans = dictionaries(p, kantor_ui::spelling::default_lang(cx));
            if let Some(lang) = kantor_spell::lang_at(&spans, r.start) {
                self.spell.menu = SpellMenu::at(&p.text, pos.offset, &flagged, lang)
                    .map(|m| (e.shape, pos.para, m));
            }
        }
        cx.notify();
    }

    fn apply_spell_item(
        &mut self,
        shape: usize,
        para: usize,
        menu: &SpellMenu,
        item: &SpellItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match item {
            SpellItem::Replace(with) => {
                let same = self.editing.as_ref().is_some_and(|e| e.shape == shape)
                    && self
                        .pres
                        .text_paras((self.current, shape))
                        .and_then(|ps| ps.get(para))
                        .and_then(|p| p.text.get(menu.range.clone()))
                        == Some(menu.word.as_str());
                if !same {
                    return;
                }
                if let Some(e) = &mut self.editing {
                    e.sel = Selection {
                        anchor: Position::new(para, menu.range.start),
                        head: Position::new(para, menu.range.end),
                    };
                    e.marked = None;
                }
                let with = with.clone();
                self.text_edit(window, cx, |p, r, e| p.insert_text(r, e.sel, &with, None));
                self.spell.typing = false;
            }
            other => {
                kantor_ui::spelling::accept_word(other, &menu.word, cx);
                cx.notify();
            }
        }
    }
}

/// The canvas's right-click menu: spelling items for a flagged word in the
/// edited text, then Cut, Copy and Paste.
pub(crate) fn context_menu(
    mut menu: PopupMenu,
    view: &WeakEntity<SlidesView>,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(v) = view.upgrade() else {
        return menu;
    };
    let target = v.read(cx).spell.menu.clone();
    if let Some((shape, para, m)) = target {
        let weak = view.clone();
        let m2 = m.clone();
        menu = kantor_ui::spelling::menu_items(
            menu,
            &m,
            Rc::new(move |item, window, cx| {
                let _ = weak.update(cx, |v, cx| {
                    v.apply_spell_item(shape, para, &m2, item, window, cx)
                });
            }),
        );
    }
    type Run = fn(&mut SlidesView, &mut Window, &mut Context<SlidesView>);
    // The actions only label the items with their shortcuts (Ctrl+X, ⌘X):
    // the click handler runs instead of dispatching them.
    let items: [(&str, Box<dyn Action>, Run); 3] = [
        (
            kantor_ui::i18n::t("Cut"),
            Box::new(crate::Cut),
            SlidesView::cut_doc,
        ),
        (
            kantor_ui::i18n::t("Copy"),
            Box::new(crate::Copy),
            SlidesView::copy_doc,
        ),
        (
            kantor_ui::i18n::t("Paste"),
            Box::new(crate::Paste),
            SlidesView::paste_doc,
        ),
    ];
    for (label, action, run) in items {
        let v = view.clone();
        menu = menu.item(PopupMenuItem::new(label).action(action).on_click(
            move |_, window, cx| {
                let _ = v.update(cx, |v, cx| run(v, window, cx));
            },
        ));
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;
    use slides_engine::{Run, RunProps};

    fn run(len: usize, attrs: &[(&str, &str)]) -> Run {
        let mut props = RunProps::default();
        props.extra.attrs = attrs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Run { len, props }
    }

    #[std::prelude::v1::test]
    fn run_languages_pick_the_dictionary() {
        let p = TextPara {
            text: "Hello apa kabar bonjour code".into(),
            runs: vec![
                run(6, &[("lang", "en-US")]),
                run(10, &[("lang", "id-ID")]),
                run(8, &[("lang", "fr-FR")]),
                run(4, &[("lang", "id-ID"), ("noProof", "1")]),
            ],
            ..TextPara::default()
        };
        let d = dictionaries(&p, Lang::IdId);
        assert_eq!(
            d,
            [
                (0..6, Some(Lang::IdId)),
                (6..16, Some(Lang::IdId)),
                (16..24, None),
                (24..28, None)
            ]
        );
    }
}
