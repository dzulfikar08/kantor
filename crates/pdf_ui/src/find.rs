//! Find in a PDF: the shared Find bar; every page's text is read on the
//! text thread (pages from the current one on first), and hits on a page
//! are added as its text arrives, so the first hits show at once even in a
//! long document.

use std::ops::Range;

use gpui_kit::*;
use kantor_core::find::FindOptions;
use kantor_ui::find_bar::{FindBar, FindBarEvent};
use pdf_engine::{Match, PageText, find_in};

use crate::PdfView;

pub(crate) struct PdfFind {
    bar: Entity<FindBar>,
    query: Option<(String, FindOptions)>,
    /// Hits in document order.
    hits: Vec<Match>,
    current: Option<usize>,
    /// Go to the first hit once one is found (a new search).
    pending_go: bool,
    _sub: Subscription,
}

impl PdfFind {
    pub fn new(window: &mut Window, cx: &mut Context<PdfView>) -> Self {
        let bar = cx.new(|cx| FindBar::new(false, window, cx));
        let sub = cx.subscribe_in(&bar, window, PdfView::on_find_event);
        Self {
            bar,
            query: None,
            hits: Vec::new(),
            current: None,
            pending_go: false,
            _sub: sub,
        }
    }

    pub fn bar(&self, cx: &App) -> Option<Entity<FindBar>> {
        self.bar.read(cx).is_open().then(|| self.bar.clone())
    }

    pub fn current_hit(&self) -> Option<&Match> {
        self.hits.get(self.current?)
    }

    /// Hits on `page`, each with whether it is the current one.
    pub fn hits_on(&self, page: usize, current: Option<&Match>) -> Vec<(Range<usize>, bool)> {
        let start = self.hits.partition_point(|m| m.page < page);
        self.hits[start..]
            .iter()
            .take_while(|m| m.page == page)
            .map(|m| (m.range.clone(), Some(m) == current))
            .collect()
    }

    /// A page's text arrived: add its hits.
    pub fn on_text(&mut self, page: usize, text: &PageText) {
        let Some((q, o)) = &self.query else {
            return;
        };
        let found = find_in(page, text, q, o.match_case, o.whole_word);
        if found.is_empty() {
            return;
        }
        let at = self.hits.partition_point(|m| m.page < page);
        let current = self.current.map(|c| if c >= at { c + found.len() } else { c });
        self.hits.splice(at..at, found);
        self.current = current;
    }
}

impl PdfView {
    fn on_find_event(
        &mut self,
        _: &Entity<FindBar>,
        ev: &FindBarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match ev {
            FindBarEvent::Search => self.find_search(cx),
            FindBarEvent::Next => self.find_step(true, cx),
            FindBarEvent::Prev => self.find_step(false, cx),
            // PDFs can't be edited: no replacing.
            FindBarEvent::Replace | FindBarEvent::ReplaceAll => {}
            FindBarEvent::Closed => {
                self.find.query = None;
                self.find.hits.clear();
                self.find.current = None;
                self.focus(window, cx);
                cx.notify();
            }
        }
    }

    /// Ctrl+F, seeded with the selected text (no replacing in a PDF:
    /// `_replace` is ignored).
    pub fn open_find(&mut self, _replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self.selected_text();
        self.find
            .bar
            .update(cx, |bar, cx| bar.show(false, seed, window, cx));
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
        let query = bar.active_query(cx).map(|q| (q, bar.options()));
        self.find.query = query.clone();
        self.find.hits.clear();
        self.find.current = None;
        self.find.pending_go = query.is_some();
        if let Some((q, o)) = &query {
            let mut pages: Vec<usize> = self.texts.keys().copied().collect();
            pages.sort_unstable();
            for p in pages {
                let found = find_in(p, &self.texts[&p], q, o.match_case, o.whole_word);
                self.find.hits.extend(found);
            }
            // Read the rest, from the current page on.
            let n = self.page_count();
            let cur = self.current_page();
            self.ask_text_in_order((cur..n).chain(0..cur));
        }
        self.find_after_text(cx);
        cx.notify();
    }

    fn ask_text_in_order(&mut self, pages: impl Iterator<Item = usize>) {
        let Some(w) = self.worker.clone() else {
            return;
        };
        let new: Vec<usize> = pages.filter(|p| self.asked_text.insert(*p)).collect();
        if !new.is_empty() {
            w.want_text(new);
        }
    }

    /// After a search or new text: go to the first hit from the current
    /// page on (once), and update the count.
    pub(crate) fn find_after_text(&mut self, cx: &mut Context<Self>) {
        if self.find.query.is_none() {
            return;
        }
        if self.find.pending_go && !self.find.hits.is_empty() {
            let cur = self.current_page();
            let i = self.find.hits.partition_point(|m| m.page < cur);
            // Only once the pages before the hit (from the current one)
            // are known, so it really is the next one.
            // None after the current page: wrap to the first one, once the
            // rest of the document is known to have no other.
            let n = self.page_count();
            let target = if i < self.find.hits.len() {
                Some(i)
            } else if (cur..n).all(|p| self.texts.contains_key(&p)) {
                Some(0)
            } else {
                None
            };
            if let Some(i) = target
                && (cur..=self.find.hits[i].page).all(|p| self.texts.contains_key(&p))
            {
                self.find.pending_go = false;
                self.find.current = Some(i);
                self.reveal_hit(cx);
            }
        }
        let (current, total) = (self.find.current, self.find.hits.len());
        self.find
            .bar
            .update(cx, |bar, cx| bar.set_result(current, total, cx));
    }

    fn find_step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let n = self.find.hits.len();
        if n == 0 {
            return;
        }
        self.find.pending_go = false;
        self.find.current = kantor_core::find::step(self.find.current, n, forward);
        self.reveal_hit(cx);
        self.find_after_text(cx);
    }

    fn reveal_hit(&mut self, cx: &mut Context<Self>) {
        let Some(m) = self.find.current_hit().cloned() else {
            return;
        };
        let Some(text) = self.texts.get(&m.page).cloned() else {
            return;
        };
        if let Some(r) = text.rects(m.range).first() {
            self.reveal(m.page, *r, cx);
        }
        cx.notify();
    }
}
