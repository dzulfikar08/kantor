//! Spell checking as you type: red wavy underlines under misspelled words
//! and the right-click menu's suggestions (see [`kantor_spell`] and
//! [`kantor_ui::spelling`]).
//!
//! Results are cached per paragraph, keyed by a hash of the paragraph (its
//! text and runs, so the `w:lang` / `w:noProof` they carry), its table style
//! and the default dictionary. After an edit only the paragraphs whose key
//! changed are checked: a few small ones right away, anything bigger (a
//! freshly opened 100-page document, a large paste) on a background thread,
//! with the underlines appearing when it's done. Painting only adds
//! underlines on the visible pages and never changes the layout.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use docs_engine::{Position, Proofing, Selection};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use kantor_spell::{Lang, SpellItem, SpellMenu, Speller};

use crate::DocsView;

/// Misspelled byte ranges of one paragraph.
pub(crate) type Flags = Arc<[Range<usize>]>;

/// Check at most this many bytes on the UI thread; more goes to the
/// background.
const SYNC_BYTES: usize = 4096;

#[derive(Default)]
pub(crate) struct SpellState {
    /// Per paragraph (same indices as the document): its misspelled words,
    /// `None` while not checked yet.
    pub flags: Rc<Vec<Option<Flags>>>,
    cache: HashMap<u64, Flags>,
    in_flight: HashSet<u64>,
    keys: Vec<u64>,
    lang: Option<Lang>,
    /// The caret is where the user is typing: don't flag the word it's in
    /// until it moves on (as Word).
    pub typing: bool,
    /// The flagged word under the last right-click: `(paragraph, menu)`.
    pub menu: Option<(usize, SpellMenu)>,
}

/// One paragraph to check.
struct Job {
    key: u64,
    text: String,
    spans: Vec<(Range<usize>, Option<Lang>)>,
}

/// Which dictionary checks each stretch of a paragraph: its `w:lang`, else
/// `default`; `None` (not checked) for "do not check spelling" and for
/// languages without a bundled dictionary.
fn dictionaries(
    spans: Vec<(Range<usize>, Proofing)>,
    default: Lang,
) -> Vec<(Range<usize>, Option<Lang>)> {
    spans
        .into_iter()
        .map(|(r, p)| {
            let lang = match p {
                Proofing::Skip => None,
                Proofing::Check(None) => Some(default),
                Proofing::Check(Some(tag)) => Lang::from_tag(&tag),
            };
            (r, lang)
        })
        .collect()
}

impl SpellState {
    /// Checking is on in this view (it has been brought up to date since
    /// it was switched on).
    pub fn is_on(&self) -> bool {
        self.lang.is_some()
    }
}

impl DocsView {
    /// Bring the underlines up to date after the document changed (or
    /// checking was switched on or off).
    pub(crate) fn refresh_spelling(&mut self, cx: &mut Context<Self>) {
        let st = &mut self.spell;
        if !kantor_ui::spelling::enabled(cx) {
            if !st.flags.is_empty() || !st.cache.is_empty() {
                *st = SpellState::default();
            }
            return;
        }
        let lang = kantor_ui::spelling::default_lang(cx);
        if st.lang != Some(lang) {
            st.cache.clear();
            st.lang = Some(lang);
        }
        let n = self.doc.len();
        let mut keys = Vec::with_capacity(n);
        let mut jobs = Vec::new();
        for i in 0..n {
            let p = self.doc.paragraph(i);
            let mut h =
                std::hash::BuildHasher::build_hasher(&foldhash::fast::FixedState::default());
            p.hash(&mut h);
            self.doc.table_style_of(p).hash(&mut h);
            lang.hash(&mut h);
            let key = h.finish();
            keys.push(key);
            if !st.cache.contains_key(&key) && !st.in_flight.contains(&key) {
                if p.text.trim().is_empty() {
                    st.cache.insert(key, Arc::from([]));
                    continue;
                }
                jobs.push(Job {
                    key,
                    text: p.text.clone(),
                    spans: dictionaries(self.doc.proofing_spans(i), lang),
                });
            }
        }
        if st.cache.len() > keys.len() * 2 + 64 {
            let live: HashSet<u64> = keys.iter().copied().collect();
            st.cache.retain(|k, _| live.contains(k));
        }
        st.keys = keys;
        let sp = Speller::global();
        let bytes: usize = jobs.iter().map(|j| j.text.len()).sum();
        let loaded = jobs
            .iter()
            .flat_map(|j| j.spans.iter().filter_map(|s| s.1))
            .all(|l| sp.is_loaded(l));
        if !jobs.is_empty() && bytes <= SYNC_BYTES && loaded {
            for j in jobs.drain(..) {
                let flags = sp.misspelled_in(&j.text, &j.spans);
                st.cache.insert(j.key, flags.into());
            }
        }
        if !jobs.is_empty() {
            st.in_flight.extend(jobs.iter().map(|j| j.key));
            let task = cx.background_spawn(async move {
                let sp = Speller::global();
                jobs.into_iter()
                    .map(|j| {
                        let flags: Flags = sp.misspelled_in(&j.text, &j.spans).into();
                        (j.key, flags)
                    })
                    .collect::<Vec<_>>()
            });
            cx.spawn(async move |this, cx| {
                let done = task.await;
                let _ = this.update(cx, |this, cx| {
                    let st = &mut this.spell;
                    for (key, flags) in done {
                        st.in_flight.remove(&key);
                        if st.lang == Some(lang) {
                            st.cache.insert(key, flags);
                        }
                    }
                    this.spell_flags_from_cache();
                    cx.notify();
                });
            })
            .detach();
        }
        self.spell_flags_from_cache();
    }

    fn spell_flags_from_cache(&mut self) {
        let st = &mut self.spell;
        let flags = st.keys.iter().map(|k| st.cache.get(k).cloned()).collect();
        st.flags = Rc::new(flags);
    }

    /// The flagged words to underline on screen: `(paragraph, range)`,
    /// without the word being typed and words accepted since the check.
    pub(crate) fn spell_ranges_in(&self, paras: Range<usize>) -> Vec<(usize, Range<usize>)> {
        let flags = &self.spell.flags;
        let caret = self.sel.is_empty().then_some(self.sel.head);
        let sp = Speller::global();
        let mut out = Vec::new();
        for para in paras.start..paras.end.min(flags.len()) {
            let Some(f) = &flags[para] else { continue };
            let text = &self.doc.paragraph(para).text;
            for r in f.iter() {
                let typing = self.spell.typing
                    && caret.is_some_and(|c| {
                        c.para == para && r.start <= c.offset && c.offset <= r.end
                    });
                let accepted = text.get(r.clone()).is_none_or(|w| sp.is_accepted(w));
                if !typing && !accepted {
                    out.push((para, r.clone()));
                }
            }
        }
        out
    }

    /// Right mouse button down: remember the flagged word under the
    /// pointer for the context menu, and move the caret there unless the
    /// click is inside the selection (as Word).
    pub(crate) fn on_right_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.spell.menu = None;
        let Some(pos) = self.hit(ev.position) else {
            return;
        };
        let (a, b) = self.sel.ordered();
        if self.sel.is_empty() || pos < a || pos > b {
            self.sel = Selection::caret(pos);
            self.goal_x = None;
            self.typing_props = None;
        }
        self.spell.typing = false;
        if kantor_ui::spelling::enabled(cx)
            && let Some(Some(flags)) = self.spell.flags.get(pos.para)
        {
            let default = kantor_ui::spelling::default_lang(cx);
            let spans = dictionaries(self.doc.proofing_spans(pos.para), default);
            let text = &self.doc.paragraph(pos.para).text;
            if let Some(r) = kantor_spell::flagged_at(flags, pos.offset)
                && let Some(lang) = kantor_spell::lang_at(&spans, r.start)
            {
                self.spell.menu =
                    SpellMenu::at(text, pos.offset, flags, lang).map(|m| (pos.para, m));
            }
        }
        cx.notify();
    }

    /// Replace the flagged word with a suggestion (one undo step), or
    /// accept it.
    pub(crate) fn apply_spell_item(
        &mut self,
        para: usize,
        menu: &SpellMenu,
        item: &SpellItem,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match item {
            SpellItem::Replace(with) => {
                let text = &self.doc.paragraph(para).text;
                if text.get(menu.range.clone()) != Some(menu.word.as_str()) {
                    return; // the text changed under the menu
                }
                let sel = Selection {
                    anchor: Position::new(para, menu.range.start),
                    head: Position::new(para, menu.range.end),
                };
                let sel = self.doc.insert_text(sel, with, None);
                self.marked = None;
                self.edited(sel, window, cx);
                self.spell.typing = false;
            }
            other => {
                kantor_ui::spelling::accept_word(other, &menu.word, cx);
                cx.notify();
            }
        }
    }
}

/// The right-click menu of the page: spelling items for a flagged word,
/// then Cut, Copy and Paste.
pub(crate) fn context_menu(
    mut menu: PopupMenu,
    view: &WeakEntity<DocsView>,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    let Some(v) = view.upgrade() else {
        return menu;
    };
    let target = v.read(cx).spell.menu.clone();
    if let Some((para, m)) = target {
        let weak = view.clone();
        let m2 = m.clone();
        menu = kantor_ui::spelling::menu_items(
            menu,
            &m,
            Rc::new(move |item, window, cx| {
                let _ = weak.update(cx, |v, cx| v.apply_spell_item(para, &m2, item, window, cx));
            }),
        );
    }
    type Run = fn(&mut DocsView, &mut Window, &mut Context<DocsView>);
    // The actions only label the items with their shortcuts (Ctrl+X, ⌘X):
    // the click handler runs instead of dispatching them.
    let items: [(&str, Box<dyn Action>, Run); 3] = [
        ("Cut", Box::new(crate::Cut), DocsView::cut_doc),
        ("Copy", Box::new(crate::Copy), DocsView::copy_doc),
        ("Paste", Box::new(crate::Paste), DocsView::paste_doc),
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

    #[std::prelude::v1::test]
    fn runs_pick_their_dictionary() {
        let spans = vec![
            (0..6, Proofing::Check(None)),
            (6..16, Proofing::Check(Some("id-ID".into()))),
            (16..20, Proofing::Check(Some("fr-FR".into()))),
            (20..30, Proofing::Skip),
        ];
        assert_eq!(
            dictionaries(spans.clone(), Lang::EnUs),
            [
                (0..6, Some(Lang::EnUs)),
                (6..16, Some(Lang::IdId)),
                (16..20, None),
                (20..30, None)
            ]
        );
        assert_eq!(dictionaries(spans, Lang::IdId)[0], (0..6, Some(Lang::IdId)));
    }

    #[std::prelude::v1::test]
    fn checks_a_mixed_language_paragraph() {
        let text = "Teh cat. Saya makn nasi. Bonjour mondee. wrng";
        let spans = dictionaries(
            vec![
                (0..9, Proofing::Check(None)),
                (9..25, Proofing::Check(Some("id-ID".into()))),
                (25..40, Proofing::Check(Some("fr-FR".into()))),
                (40..text.len(), Proofing::Skip),
            ],
            Lang::EnUs,
        );
        let sp = Speller::with_personal(None);
        let bad = sp.misspelled_in(text, &spans);
        let words: Vec<&str> = bad.iter().map(|r| &text[r.clone()]).collect();
        assert_eq!(words, ["Teh", "makn"]);
    }
}
