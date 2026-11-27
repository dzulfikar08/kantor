//! Find & Replace in a note: the shared [`FindBar`] over the text, matches
//! highlighted (the current one stronger and selected). Replace All is one
//! undo step.

use std::ops::Range;
use std::rc::Rc;

use gpui_kit::*;
use kantor_core::find::{find_all, pick};
use kantor_ui::find_bar::{FindBar, FindBarEvent};
use kantor_ui::i18n::tf;
use notes_engine::{Merge, Selection};

use crate::NotesView;

pub(crate) fn new_bar(
    window: &mut Window,
    cx: &mut Context<NotesView>,
) -> (Entity<FindBar>, Subscription) {
    let bar = cx.new(|cx| FindBar::new(false, window, cx));
    let sub = cx.subscribe_in(&bar, window, NotesView::on_find_event);
    (bar, sub)
}

impl NotesView {
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
                let start = self.editor.selection().range().start;
                self.go_to_match_from(start, true, cx);
            }
            FindBarEvent::Next => self.find_step(true, cx),
            FindBarEvent::Prev => self.find_step(false, cx),
            FindBarEvent::Replace => self.replace_current(cx),
            FindBarEvent::ReplaceAll => self.replace_all(cx),
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
        let sel = self.editor.selected_text();
        let seed = (!sel.is_empty() && !sel.contains('\n')).then(|| sel.to_owned());
        self.find
            .update(cx, |bar, cx| bar.show(replace, seed, window, cx));
    }

    /// F3 / Shift+F3 outside the bar.
    pub fn find_next_doc(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find.read(cx).is_open() {
            self.open_find(false, window, cx);
            return;
        }
        self.find_step(forward, cx);
    }

    fn search(&mut self, cx: &mut Context<Self>) {
        let bar = self.find.read(cx);
        let found: Vec<Range<usize>> = match bar.active_query(cx) {
            Some(q) => find_all(self.editor.text(), &q, bar.options()),
            None => Vec::new(),
        };
        self.found = Rc::new(found);
        let r = self.editor.selection().range();
        self.found_current = self.found.iter().position(|m| *m == r);
        self.report_find(cx);
    }

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
        let r = self.editor.selection().range();
        self.go_to_match_from(if forward { r.end } else { r.start }, forward, cx);
    }

    fn go_to_match_from(&mut self, at: usize, forward: bool, cx: &mut Context<Self>) {
        let keys: Vec<usize> = self.found.iter().map(|r| r.start).collect();
        let Some(ix) = pick(&keys, &at, forward, false) else {
            self.found_current = None;
            self.report_find(cx);
            return;
        };
        let r = self.found[ix].clone();
        self.editor.set_selection(Selection::new(r.start, r.end));
        self.found_current = Some(ix);
        self.reveal_caret = true;
        self.report_find(cx);
    }

    fn replace_current(&mut self, cx: &mut Context<Self>) {
        if !self.editable() {
            return;
        }
        let Some(ix) = self.found_current else {
            self.find_step(true, cx);
            return;
        };
        let with = self.find.read(cx).replacement(cx);
        let r = self.found[ix].clone();
        self.editor.set_selection(Selection::new(r.start, r.end));
        self.editor.insert(&with, Merge::No);
        self.edited(cx);
        self.find_step(true, cx);
    }

    fn replace_all(&mut self, cx: &mut Context<Self>) {
        if !self.editable() || self.found.is_empty() {
            return;
        }
        let with = self
            .editor
            .buffer()
            .normalize(&self.find.read(cx).replacement(cx));
        let edits = self
            .found
            .iter()
            .map(|r| (r.clone(), with.clone()))
            .collect::<Vec<_>>();
        let n = edits.len();
        self.editor.edit(edits, None, Merge::No);
        self.status = tf("Replaced {} matches", &[&n]).into();
        self.edited(cx);
    }
}
