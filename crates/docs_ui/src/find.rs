//! Find & Replace in Docs: the shared [`FindBar`] over the page, matches
//! highlighted on the page (all of them light, the current one stronger),
//! the current match selected and scrolled into view. Replace All is one
//! undo step ([`docs_engine::Document::replace_ranges`]).

use std::ops::Range;
use std::rc::Rc;

use docs_engine::{Position, Selection};
use gpui_kit::*;
use kantor_core::find::{find_all, pick};
use kantor_ui::find_bar::{FindBar, FindBarEvent};
use kantor_ui::i18n::{t, tf};

use crate::DocsView;

/// Matches: `(paragraph, byte range)` in document order.
pub(crate) type Found = Rc<Vec<(usize, Range<usize>)>>;

pub(crate) fn new_bar(
    window: &mut Window,
    cx: &mut Context<DocsView>,
) -> (Entity<FindBar>, Subscription) {
    let bar = cx.new(|cx| FindBar::new(false, window, cx));
    let sub = cx.subscribe_in(&bar, window, DocsView::on_find_event);
    (bar, sub)
}

impl DocsView {
    fn on_find_event(
        &mut self,
        _: &Entity<FindBar>,
        ev: &FindBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            FindBarEvent::Search => {
                self.search(cx);
                // Incremental: go to the first match from the selection start.
                let (start, _) = self.sel.ordered();
                self.go_to_match_from(start, true, cx);
            }
            FindBarEvent::Next => self.find_step(true, cx),
            FindBarEvent::Prev => self.find_step(false, cx),
            FindBarEvent::Replace => self.replace_current(window, cx),
            FindBarEvent::ReplaceAll => self.replace_all(window, cx),
            FindBarEvent::Closed => {
                self.found = Rc::default();
                self.found_current = None;
                self.focus(window, cx);
                cx.notify();
            }
        }
    }

    /// Ctrl+F / Ctrl+H: open the bar, seeded with the selected text.
    pub fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (start, end) = self.sel.ordered();
        let seed = (start != end && start.para == end.para).then(|| self.doc.text_in(start, end));
        self.find
            .update(cx, |bar, cx| bar.show(replace, seed, window, cx));
    }

    /// F3 / Shift+F3 outside the bar: next / previous match of the last query.
    pub fn find_next_doc(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find.read(cx).is_open() {
            self.open_find(false, window, cx);
            return;
        }
        self.find_step(forward, cx);
    }

    /// Search again (query, options or the document changed).
    pub(crate) fn search(&mut self, cx: &mut Context<Self>) {
        let bar = self.find.read(cx);
        let found = match bar.active_query(cx) {
            Some(q) => {
                let opts = bar.options();
                self.doc
                    .paragraphs()
                    .iter()
                    .enumerate()
                    .flat_map(|(i, p)| find_all(&p.text, &q, opts).into_iter().map(move |r| (i, r)))
                    .collect()
            }
            None => Vec::new(),
        };
        self.found = Rc::new(found);
        // The current match is the selected one, if the selection is a match.
        let (start, end) = self.sel.ordered();
        self.found_current = self.found.iter().position(|(p, r)| {
            start == Position::new(*p, r.start) && end == Position::new(*p, r.end)
        });
        self.report_find(cx);
    }

    /// After an edit: keep the matches in step with the text.
    pub(crate) fn refresh_find(&mut self, cx: &mut Context<Self>) {
        if self.find.read(cx).is_open() {
            self.search(cx);
        }
    }

    fn report_find(&mut self, cx: &mut Context<Self>) {
        let (current, total) = (self.found_current, self.found.len());
        self.find
            .update(cx, |bar, cx| bar.set_result(current, total, cx));
        cx.notify();
    }

    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let (start, end) = self.sel.ordered();
        if forward {
            self.go_to_match_from(end, true, cx);
        } else {
            self.go_to_match_from(start, false, cx);
        }
    }

    /// Select the match at or after `at` (`forward`) or the last one before
    /// it, wrapping around.
    fn go_to_match_from(&mut self, at: Position, forward: bool, cx: &mut Context<Self>) {
        let keys: Vec<(usize, usize)> = self.found.iter().map(|(p, r)| (*p, r.start)).collect();
        let Some(ix) = pick(&keys, &(at.para, at.offset), forward, false) else {
            self.found_current = None;
            self.report_find(cx);
            return;
        };
        let (para, r) = self.found[ix].clone();
        self.sel = Selection {
            anchor: Position::new(para, r.start),
            head: Position::new(para, r.end),
        };
        self.goal_x = None;
        self.typing_props = None;
        self.found_current = Some(ix);
        self.reveal_caret.set(true);
        self.report_find(cx);
    }

    /// Replace the current match (if it is what's selected), then go to
    /// the next one. With no current match, just go to the next.
    fn replace_current(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.found_current else {
            self.find_step(true, cx);
            return;
        };
        let with = self.find.read(cx).replacement(cx);
        let (para, r) = self.found[ix].clone();
        let sel = Selection {
            anchor: Position::new(para, r.start),
            head: Position::new(para, r.end),
        };
        let after = self.doc.replace_ranges(&[(para, r)], &with, sel);
        self.edited(after, window, cx);
        self.go_to_match_from(after.head, true, cx);
    }

    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.found.len();
        if n == 0 {
            return;
        }
        let with = self.find.read(cx).replacement(cx);
        let found = self.found.clone();
        let after = self.doc.replace_ranges(&found, &with, self.sel);
        self.edited(after, window, cx);
        self.status = if n == 1 {
            t("Replaced 1 match").into()
        } else {
            tf("Replaced {} matches", &[&n]).into()
        };
        cx.notify();
    }
}
