//! Find & Replace in Slides: searches the text of every shape on every
//! slide, in reading order (slide, then shape z-order), and goes to the
//! slide and selects the matching text. Replace All is one undo step
//! ([`slides_engine::Presentation::replace_matches`]); text that can't be
//! edited (locked slides, fields) is found but skipped by Replace.

use gpui_kit::*;
use kantor_core::find::{find_all, pick};
use kantor_ui::find_bar::{FindBar, FindBarEvent};
use kantor_ui::i18n::{self, tf};
use slides_engine::{Focus, TextMatch};

use crate::SlidesView;

/// The bar and the matches of the last search.
pub(crate) struct SlidesFind {
    bar: Entity<FindBar>,
    found: Vec<TextMatch>,
    current: Option<usize>,
    _sub: Subscription,
}

impl SlidesFind {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<SlidesView>) -> Self {
        let bar = cx.new(|cx| FindBar::new(false, window, cx));
        let sub = cx.subscribe_in(&bar, window, SlidesView::on_find_event);
        Self {
            bar,
            found: Vec::new(),
            current: None,
            _sub: sub,
        }
    }

    pub(crate) fn bar(&self, cx: &App) -> Option<Entity<FindBar>> {
        self.bar.read(cx).is_open().then(|| self.bar.clone())
    }
}

impl SlidesView {
    fn on_find_event(
        &mut self,
        _: &Entity<FindBar>,
        ev: &FindBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            FindBarEvent::Search => {
                self.find_search(cx);
                let at = self.find_cursor(false);
                self.find_go(at, true, cx);
            }
            FindBarEvent::Next => self.find_step(true, cx),
            FindBarEvent::Prev => self.find_step(false, cx),
            FindBarEvent::Replace => self.find_replace(false, window, cx),
            FindBarEvent::ReplaceAll => self.find_replace(true, window, cx),
            FindBarEvent::Closed => {
                self.find.found.clear();
                self.find.current = None;
                self.focus(window, cx);
                cx.notify();
            }
        }
    }

    /// Ctrl+F / Ctrl+H, seeded with the selected text.
    pub fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self.editing.as_ref().and_then(|e| {
            let (a, b) = e.sel.ordered();
            let paras = self.pres.text_paras((self.current, e.shape))?;
            (a.para == b.para && a != b)
                .then(|| {
                    paras
                        .get(a.para)?
                        .text
                        .get(a.offset..b.offset)
                        .map(str::to_owned)
                })
                .flatten()
        });
        self.find
            .bar
            .update(cx, |bar, cx| bar.show(replace, seed, window, cx));
    }

    /// F3 / Shift+F3 outside the bar.
    pub fn find_next_doc(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.find.bar.read(cx).is_open() {
            self.open_find(false, window, cx);
            return;
        }
        self.find_step(forward, cx);
    }

    fn find_search(&mut self, cx: &mut Context<Self>) {
        let bar = self.find.bar.read(cx);
        let found = match bar.active_query(cx) {
            Some(q) => {
                let opts = bar.options();
                let mut out = Vec::new();
                for (shape, paras) in self.pres.searchable_text() {
                    for (para, text) in paras.iter().enumerate() {
                        out.extend(find_all(text, &q, opts).into_iter().map(|range| TextMatch {
                            shape,
                            para,
                            range,
                        }));
                    }
                }
                out
            }
            None => Vec::new(),
        };
        self.find.found = found;
        let sel = self.editing.as_ref().map(|e| (e.shape, e.sel.ordered()));
        self.find.current = self.find.found.iter().position(|m| {
            let s = m.selection().ordered();
            sel == Some((m.shape.1, s)) && m.shape.0 == self.current
        });
        self.report_find(cx);
    }

    fn report_find(&mut self, cx: &mut Context<Self>) {
        let (current, total) = (self.find.current, self.find.found.len());
        self.find
            .bar
            .update(cx, |bar, cx| bar.set_result(current, total, cx));
        cx.notify();
    }

    /// Where the search continues from, as a [`TextMatch::key`]: the text
    /// selection's end (`end`) or start, else the selected shape or slide.
    fn find_cursor(&self, end: bool) -> (usize, usize, usize, usize) {
        match &self.editing {
            Some(e) => {
                let (a, b) = e.sel.ordered();
                let p = if end { b } else { a };
                (self.current, e.shape, p.para, p.offset)
            }
            None => (self.current, self.selected.unwrap_or(0), 0, 0),
        }
    }

    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        self.find_search(cx);
        let at = self.find_cursor(forward);
        self.find_go(at, forward, cx);
    }

    /// Go to the match at or after `at` (forward) or the last one before.
    fn find_go(&mut self, at: (usize, usize, usize, usize), forward: bool, cx: &mut Context<Self>) {
        let keys: Vec<_> = self.find.found.iter().map(TextMatch::key).collect();
        let Some(ix) = pick(&keys, &at, forward, false) else {
            self.find.current = None;
            self.report_find(cx);
            return;
        };
        let m = self.find.found[ix].clone();
        self.find.current = Some(ix);
        self.show_match(&m, cx);
        self.report_find(cx);
    }

    /// Go to the match's slide, select its shape and the matched text.
    fn show_match(&mut self, m: &TextMatch, cx: &mut Context<Self>) {
        let (slide, shape) = m.shape;
        if slide != self.current {
            self.go_to(slide, cx);
        }
        let sel = m.selection();
        let editable = self.pres.text_replaceable(m.shape);
        if editable && self.begin_editing(shape, Some(sel.head), cx) {
            if let Some(e) = &mut self.editing {
                e.sel = sel;
            }
        } else {
            self.editing = None;
            self.selected = Some(shape);
        }
        cx.notify();
    }

    fn find_replace(&mut self, all: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.find_search(cx);
        let with = self.find.bar.read(cx).replacement(cx);
        let targets = if all {
            self.find.found.clone()
        } else {
            match self.find.current {
                Some(i) => vec![self.find.found[i].clone()],
                None => {
                    let at = self.find_cursor(true);
                    self.find_go(at, true, cx);
                    return;
                }
            }
        };
        let before = match &self.editing {
            Some(e) => Focus::text(self.current, e.shape, e.sel),
            None => Focus::slide(self.current),
        };
        let caret = targets.last().map(|m| {
            let start = m.range.start + with.len();
            (m.shape, m.para, start)
        });
        let result = self.pres.replace_matches(&targets, &with, before);
        match result {
            Ok(n) => {
                self.editing = None;
                self.after_edit(window, cx);
                self.status = match (n, all) {
                    (0, _) => i18n::t("This text can't be edited").into(),
                    (1, true) => i18n::t("Replaced 1 match").into(),
                    (n, true) => tf("Replaced {} matches", &[&n]).into(),
                    _ => SharedString::default(),
                };
                if !all && let Some(((slide, shape), para, offset)) = caret {
                    // Continue after the replacement.
                    self.find_search(cx);
                    let at = (slide, shape, para, offset);
                    self.find_go(at, true, cx);
                } else {
                    self.find_search(cx);
                }
            }
            Err(e) => self.status = format!("{e:#}").into(),
        }
        cx.notify();
    }
}
