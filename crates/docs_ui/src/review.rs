//! Review in the Docs view: comment cards in a markup column to the right
//! of the pages (each beside the text it's on, joined by a line; the text
//! is highlighted), adding, editing and deleting comments, and the Review
//! menu: accept or reject the tracked change at the caret, all of them,
//! and go to the next or previous change. Tracked insertions and deletions
//! are coloured by author on the page itself (`docs_pdf::build`).

use docs_engine::Selection;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{
    Icon, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenu, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t, t_dyn, tf, tk};
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};

use crate::DocsView;

/// The body's tracked changes and comment anchors, kept until it changes:
/// every frame asks for them, and a reviewed long document has thousands.
#[derive(Default)]
pub(crate) struct ReviewMemo {
    /// Revision, paragraph storage, paragraphs and comments of the
    /// document they're from.
    key: Option<(u64, usize, usize, usize)>,
    changes: Option<std::rc::Rc<Vec<docs_engine::Change>>>,
    anchors: Option<std::rc::Rc<Vec<(String, docs_engine::Anchor)>>>,
    /// Some change shows as a balloon (formatting, field code edits).
    balloons: Option<bool>,
}

/// Width of the markup column, screen px.
pub(crate) const MARKUP_W: f32 = 264.0;
/// Card width inside it.
const CARD_W: f32 = 232.0;

/// A story tracked changes can be in (see `change_stories`).
#[derive(Clone, Debug, PartialEq)]
enum ChangeStory {
    Body,
    /// A header or footer: the first page showing it, and its index in
    /// the main document's stories.
    Hf {
        page: usize,
        kind: docs_engine::HfKind,
        story: usize,
    },
    Note {
        kind: docs_engine::NoteKind,
        id: String,
        index: usize,
    },
}

/// A comment being written or edited.
pub(crate) struct CommentEdit {
    id: String,
    input: Entity<TextareaState>,
    /// Added just now: cancelling removes it again.
    new: bool,
    _sub: Subscription,
}

/// A comment card placed in the markup column.
#[derive(Clone, Debug)]
pub(crate) struct Card {
    pub id: String,
    /// The anchor on the page, window coordinates.
    pub anchor: Point<Pixels>,
    /// The card's top-left corner, window coordinates.
    pub at: Point<Pixels>,
    pub active: bool,
    /// A formatting change's balloon instead of a comment.
    pub change: Option<ChangeNote>,
}

/// What a formatting change's balloon shows (Word's "Formatted: Bold").
#[derive(Clone, Debug)]
pub(crate) struct ChangeNote {
    pub author: String,
    pub date: String,
    pub label: String,
    pub start: docs_engine::Position,
    pub end: docs_engine::Position,
}

/// Word's balloon text for a formatting change: "Formatted: Font: Bold".
pub(crate) fn format_label(items: &[docs_engine::FormatItem]) -> String {
    use docs_engine::{Align, FormatItem as F};
    // Cells merged down (`w:cellMerge`): Word's "Merged Cells".
    if items.contains(&F::MergedCells) {
        return t("Merged Cells").to_owned();
    }
    // A table's, rows' or cells' properties: Word's "Formatted Table".
    if !items.is_empty()
        && items
            .iter()
            .all(|it| matches!(it, F::TableProps | F::RowProps | F::CellProps))
    {
        return t("Formatted Table").to_owned();
    }
    let parts: Vec<String> = items
        .iter()
        .map(|it| match it {
            F::Bold(true) => t("Font: Bold").to_owned(),
            F::Bold(false) => t("Font: Not Bold").to_owned(),
            F::Italic(true) => t("Font: Italic").to_owned(),
            F::Italic(false) => t("Font: Not Italic").to_owned(),
            F::Underline(true) => t("Underline").to_owned(),
            F::Underline(false) => t("No underline").to_owned(),
            F::Font(Some(f)) => tf("Font: {}", &[f]),
            F::Font(None) => t("Font: (Default)").to_owned(),
            F::Size(Some(hp)) => tf("Font: {} pt", &[&(*hp as f32 / 2.0)]),
            F::Size(None) => t("Font size: (Default)").to_owned(),
            F::FontColor => t("Font color").to_owned(),
            F::CharStyle(n) | F::Style(n) => tf("Style: {}", &[n]),
            F::Align(Align::Left) => t("Left").to_owned(),
            F::Align(Align::Center) => t("Centered").to_owned(),
            F::Align(Align::Right) => t("Right").to_owned(),
            F::Align(Align::Justify) => t("Justified").to_owned(),
            F::Indent => t("Indent").to_owned(),
            F::Spacing => t("Spacing").to_owned(),
            F::LineSpacing => t("Line spacing").to_owned(),
            F::List(true) => t("Bulleted or numbered").to_owned(),
            F::List(false) => t("Not a list").to_owned(),
            F::PageBreakBefore => t("Page break before").to_owned(),
            F::TabStops => t("Tab stops").to_owned(),
            F::TableProps => t("Table").to_owned(),
            F::RowProps => t("Row").to_owned(),
            F::CellProps => t("Cell").to_owned(),
            F::MergedCells => t("Merged Cells").to_owned(),
            F::Other => t("Other formatting").to_owned(),
        })
        .collect();
    tf("Formatted: {}", &[&parts.join(", ")])
}

/// A short date for a card: `12 Nov 2026` from Word's ISO date.
fn short_date(iso: &str) -> String {
    const M: [&str; 12] = [
        tk("Jan"),
        tk("Feb"),
        tk("Mar"),
        tk("Apr"),
        tk("May"),
        tk("Jun"),
        tk("Jul"),
        tk("Aug"),
        tk("Sep"),
        tk("Oct"),
        tk("Nov"),
        tk("Dec"),
    ];
    let d: Vec<&str> = iso.get(..10).unwrap_or("").split('-').collect();
    match (
        d.first(),
        d.get(1).and_then(|m| m.parse::<usize>().ok()),
        d.get(2),
    ) {
        (Some(y), Some(m), Some(day)) if (1..=12).contains(&m) => {
            format!("{} {} {y}", day.trim_start_matches('0'), t_dyn(M[m - 1]))
        }
        _ => String::new(),
    }
}

impl DocsView {
    /// The memo for the body as it is now (`None` while a header, footer
    /// or note is edited: the view's document is then that part, or the
    /// part swapped in for a moment).
    fn memo(&self) -> Option<std::cell::RefMut<'_, ReviewMemo>> {
        if self.part.is_some() {
            return None;
        }
        let d = &self.doc;
        let key = (
            d.revision(),
            d.paragraphs().as_ptr() as usize,
            d.len(),
            d.comments().len(),
        );
        let mut m = self.review_memo.borrow_mut();
        if m.key != Some(key) {
            *m = ReviewMemo {
                key: Some(key),
                ..ReviewMemo::default()
            };
        }
        Some(m)
    }

    /// The view's document's tracked changes.
    pub(crate) fn doc_changes(&self) -> std::rc::Rc<Vec<docs_engine::Change>> {
        match self.memo() {
            Some(mut m) => m
                .changes
                .get_or_insert_with(|| std::rc::Rc::new(self.doc.changes()))
                .clone(),
            None => std::rc::Rc::new(self.doc.changes()),
        }
    }

    /// The view's document's comment anchors.
    pub(crate) fn doc_comment_anchors(&self) -> std::rc::Rc<Vec<(String, docs_engine::Anchor)>> {
        match self.memo() {
            Some(mut m) => m
                .anchors
                .get_or_insert_with(|| std::rc::Rc::new(self.doc.comment_anchors()))
                .clone(),
            None => std::rc::Rc::new(self.doc.comment_anchors()),
        }
    }

    /// Whether the markup column shows (the document has comments or
    /// changes shown in balloons).
    pub(crate) fn markup_visible(&self) -> bool {
        if !self.comments_visible {
            return false;
        }
        if !self.main_doc().comments().is_empty() || self.comment_edit.is_some() {
            return true;
        }
        let balloons = |doc: &docs_engine::Document, changes: &[docs_engine::Change]| {
            changes.iter().any(|c| {
                c.kind == docs_engine::RevKind::Format
                    || (c.kind == docs_engine::RevKind::Insert
                        && doc.tracked_field_code(c.start).is_some())
            })
        };
        if self.part.is_some() {
            let main = self.main_doc();
            return balloons(main, &main.changes()[..]);
        }
        if let Some(b) = self.memo().and_then(|m| m.balloons) {
            return b;
        }
        let b = balloons(&self.doc, &self.doc_changes()[..]);
        if let Some(mut m) = self.memo() {
            m.balloons = Some(b);
        }
        b
    }

    pub fn toggle_comments_doc(&mut self, cx: &mut Context<Self>) {
        self.comments_visible = !self.comments_visible;
        self.clamp_scroll();
        cx.notify();
    }

    /// Comment on the selection (or the word at the caret) and start
    /// writing it.
    pub fn new_comment_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_part(window, cx);
        self.save_comment_edit(window, cx);
        let author = kantor_core::settings::Settings::load().author_name();
        let Some(id) = self.doc.add_comment(self.sel, "", &author) else {
            return;
        };
        self.comments_visible = true;
        let sel = self.sel;
        self.edited(sel, window, cx);
        self.edit_comment(id, true, window, cx);
    }

    fn edit_comment(&mut self, id: String, new: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .doc
            .comments()
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.text.clone())
            .unwrap_or_default();
        let input = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder(t("Write a comment"));
            s.set_value(text, window, cx);
            s
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = ev
            {
                this.save_comment_edit(window, cx);
                this.focus(window, cx);
            }
        });
        input.update(cx, |s, cx| s.focus(window, cx));
        self.comment_edit = Some(CommentEdit {
            id,
            input,
            new,
            _sub: sub,
        });
        cx.notify();
    }

    /// Keep what's typed in the open comment editor.
    pub(crate) fn save_comment_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.comment_edit.take() else {
            return;
        };
        let text = edit.input.read(cx).value().trim_end().to_string();
        if text.is_empty() && edit.new {
            // An empty new comment isn't kept.
            self.doc.undo();
            let sel = self.sel;
            self.edited(sel, window, cx);
            return;
        }
        if self.doc.set_comment_text(&edit.id, &text, self.sel) {
            let sel = self.sel;
            self.edited(sel, window, cx);
        }
        cx.notify();
    }

    fn cancel_comment_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.comment_edit.take() else {
            return;
        };
        if edit.new {
            self.doc.undo();
            let sel = self.sel;
            self.edited(sel, window, cx);
        }
        self.focus(window, cx);
        cx.notify();
    }

    /// Reply to the thread `id` is in and start writing the reply.
    fn reply_comment_doc(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.save_comment_edit(window, cx);
        let author = kantor_core::settings::Settings::load().author_name();
        let Some(new) = self.doc.reply_comment(id, "", &author) else {
            return;
        };
        let sel = self.sel;
        self.edited(sel, window, cx);
        self.edit_comment(new, true, window, cx);
    }

    /// Resolve or reopen the thread `id` is in.
    fn resolve_comment_doc(
        &mut self,
        id: &str,
        done: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.save_comment_edit(window, cx);
        if self.doc.set_comment_done(id, done, self.sel) {
            let sel = self.sel;
            self.edited(sel, window, cx);
            self.status = if done {
                t("Comment resolved")
            } else {
                t("Comment reopened")
            }
            .into();
        }
        cx.notify();
    }

    fn delete_comment_doc(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.comment_edit = None;
        if self.doc.delete_comment(id, self.sel) {
            let sel = self.sel;
            self.edited(sel, window, cx);
            self.status = t("Comment deleted").into();
        }
        cx.notify();
    }

    fn go_to_comment(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, a)) = self
            .doc
            .comment_anchors()
            .into_iter()
            .find(|(i, _)| i == id)
        {
            self.sel = Selection {
                anchor: a.start,
                head: a.end,
            };
            self.reveal_caret.set(true);
            self.focus(window, cx);
            cx.notify();
        }
    }

    // ── tracked changes ────────────────────────────────────────────────────

    /// Review > Track Changes: record the user's own edits as insertions
    /// and deletions (Word's Ctrl+Shift+E).
    pub fn toggle_track_changes_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_part(window, cx);
        let author = kantor_core::settings::Settings::load().author_name();
        self.doc.set_author(&author);
        let on = !self.doc.track_changes();
        self.doc.set_track_changes(on, self.sel);
        let sel = self.sel;
        self.edited(sel, window, cx);
        self.status = if on {
            let msg = tk("Track Changes on: your edits are marked as {}'s");
            tf(msg, &[&author]).into()
        } else {
            t("Track Changes off").into()
        };
    }

    pub fn accept_change_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resolve_change(true, window, cx);
    }

    pub fn reject_change_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.resolve_change(false, window, cx);
    }

    /// Tracked changes in the whole document: text, headers, footers and
    /// notes (the one being edited included).
    pub(crate) fn change_count(&self) -> usize {
        let count = |d: &docs_engine::Document| d.changes().len() + d.other_changes();
        match &self.part {
            Some(s) => count(&s.main) + s.main.part_changes() + count(&self.doc),
            None => count(&self.doc) + self.doc.part_changes(),
        }
    }

    /// Accept or reject the change at the caret: in the text, or in the
    /// header, footer or note being edited.
    fn resolve_change(&mut self, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let done = if accept {
            self.doc.accept_change(self.sel)
        } else {
            self.doc.reject_change(self.sel)
        };
        match done {
            Some(sel) => {
                self.edited(sel, window, cx);
                // Move on to the next change, as Word does.
                self.go_to_change(true, window, cx);
                self.status = self.changes_status(accept);
            }
            None => self.status = t("No tracked change at the caret").into(),
        }
        cx.notify();
    }

    pub fn resolve_all_doc(&mut self, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_part(window, cx);
        let n = self.change_count();
        if n == 0 {
            self.status = t("This document has no tracked changes").into();
            cx.notify();
            return;
        }
        let sel = self.doc.resolve_all(accept, self.sel);
        self.edited(sel, window, cx);
        self.status = match (accept, n) {
            (true, 1) => t("Accepted 1 tracked change").into(),
            (true, n) => tf("Accepted {} tracked changes", &[&n]).into(),
            (false, 1) => t("Rejected 1 tracked change").into(),
            (false, n) => tf("Rejected {} tracked changes", &[&n]).into(),
        };
        cx.notify();
    }

    fn changes_status(&self, accept: bool) -> SharedString {
        let left = self.change_count();
        match (accept, left) {
            (true, 0) => t("Accepted: no tracked changes left").into(),
            (true, 1) => t("Accepted: 1 tracked change left").into(),
            (true, n) => tf("Accepted: {} tracked changes left", &[&n]).into(),
            (false, 0) => t("Rejected: no tracked changes left").into(),
            (false, 1) => t("Rejected: 1 tracked change left").into(),
            (false, n) => tf("Rejected: {} tracked changes left", &[&n]).into(),
        }
    }

    /// The stories tracked changes can be in, in the order Next change
    /// visits them (Word's): the text, then each header and footer in page
    /// order, then the notes in the order of their references.
    fn change_stories(&self) -> Vec<ChangeStory> {
        let main = self.main_doc();
        let sections = main.sections();
        let mut out = vec![ChangeStory::Body];
        let mut seen: Vec<usize> = Vec::new();
        for page in 0..self.laid.paged.pages.len() {
            let (si, ty) = self.hf_type_on_page(page);
            for kind in [docs_engine::HfKind::Header, docs_engine::HfKind::Footer] {
                let Some(story) = main.story_index_for(&sections, si, kind, ty) else {
                    continue;
                };
                if seen.contains(&story) || main.stories()[story].locked.is_some() {
                    continue;
                }
                seen.push(story);
                out.push(ChangeStory::Hf { page, kind, story });
            }
        }
        let notes = main.notes();
        for r in main.note_refs() {
            if let Some(i) = notes.iter().position(|n| n.kind == r.kind && n.id == r.id)
                && !out
                    .iter()
                    .any(|s| matches!(s, ChangeStory::Note { index, .. } if *index == i))
            {
                out.push(ChangeStory::Note {
                    kind: r.kind,
                    id: r.id.clone(),
                    index: i,
                });
            }
        }
        out
    }

    /// The story being edited now.
    fn current_story(&self, stories: &[ChangeStory]) -> usize {
        let Some(s) = &self.part else { return 0 };
        stories
            .iter()
            .position(|st| match (st, &s.edit.part) {
                (ChangeStory::Hf { story, .. }, docs_engine::PartRef::Story(i)) => story == i,
                (ChangeStory::Note { index, .. }, docs_engine::PartRef::Note(i)) => index == i,
                _ => false,
            })
            .unwrap_or(0)
    }

    /// A story's tracked changes (the one being edited is the view's
    /// document).
    fn story_changes(
        &self,
        k: usize,
        current: usize,
        st: &ChangeStory,
    ) -> Vec<docs_engine::Change> {
        if k == current {
            return self.doc.changes();
        }
        let main = self.main_doc();
        match st {
            ChangeStory::Body => main.changes(),
            ChangeStory::Hf { story, .. } => main
                .stories()
                .get(*story)
                .map(|s| s.doc.changes())
                .unwrap_or_default(),
            ChangeStory::Note { index, .. } => main
                .notes()
                .get(*index)
                .map(|n| n.doc.changes())
                .unwrap_or_default(),
        }
    }

    /// Select the next (or previous) tracked change after the caret: in
    /// the text, or in a header, footer or note (opened for editing), as
    /// Word's Next goes through every story.
    pub fn go_to_change(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let stories = self.change_stories();
        let current = self.current_story(&stories);
        let (a, b) = self.sel.ordered();
        // Every change as (story, start, end), in visiting order.
        let mut all: Vec<(usize, docs_engine::Position, docs_engine::Position)> = Vec::new();
        for (k, st) in stories.iter().enumerate() {
            for c in self.story_changes(k, current, st) {
                all.push((k, c.start, c.end));
            }
        }
        let found = if forward {
            all.iter()
                .find(|(k, s, e)| {
                    *k > current || (*k == current && (*s > a || (*s == a && *e > b)))
                })
                .or(all.first())
        } else {
            all.iter()
                .rev()
                .find(|(k, s, _)| *k < current || (*k == current && *s < a))
                .or(all.last())
        };
        let Some(&(k, start, end)) = found else {
            return false;
        };
        if k != current {
            match stories[k].clone() {
                ChangeStory::Body => {
                    self.leave_part(window, cx);
                }
                ChangeStory::Hf { page, kind, .. } => {
                    self.edit_hf_on_page(page, kind, None, window, cx);
                }
                ChangeStory::Note { kind, id, .. } => {
                    self.edit_note_in_place(kind, &id, None, window, cx);
                }
            }
            if self.current_story(&stories) != k {
                return false;
            }
        }
        self.sel = Selection {
            anchor: self.doc.clamp(start),
            head: self.doc.clamp(end),
        };
        self.reveal_caret.set(true);
        cx.notify();
        true
    }

    pub fn next_change_doc(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.go_to_change(forward, window, cx) {
            self.status = t("This document has no tracked changes").into();
            cx.notify();
        }
    }

    /// Change bars for the visible pages: every line holding a tracked
    /// change, `(page, top, height)`.
    pub(crate) fn change_bars(&self) -> Vec<(usize, f32, f32)> {
        let changes = self.doc_changes();
        if changes.is_empty() {
            return Vec::new();
        }
        let visible = self.geometry().visible_pages(self.laid.paged.pages.len());
        // Changes in paragraphs the visible pages show.
        let shown = if self.laid.editing_part() {
            Some((0, usize::MAX))
        } else {
            self.laid.paged.paras_on(visible.clone())
        };
        let Some((lo, hi)) = shown else {
            return Vec::new();
        };
        let mut out: Vec<(usize, f32, f32)> = Vec::new();
        for c in changes
            .iter()
            .filter(|c| c.end.para >= lo && c.start.para <= hi)
        {
            let mut rects: Vec<(usize, f32, f32)> = self
                .laid
                .selection_rects_on(
                    (c.start.para, c.start.offset),
                    (c.end.para, c.end.offset),
                    visible.clone(),
                )
                .into_iter()
                .map(|(p, r)| (p, r[1], r[3]))
                .collect();
            if let Some(k) = self.laid.caret(c.start.para, c.start.offset) {
                rects.push((k.page, k.top, k.height));
            }
            for r in rects {
                if visible.contains(&r.0) && !out.contains(&r) {
                    out.push(r);
                }
            }
        }
        out
    }

    // ── cards ──────────────────────────────────────────────────────────────

    /// Comment cards for the comments on visible pages, stacked so they
    /// don't overlap.
    pub(crate) fn comment_cards(&self) -> Vec<Card> {
        if !self.markup_visible() {
            return Vec::new();
        }
        let g = self.geometry();
        let paged = &self.laid.paged;
        let visible = g.visible_pages(paged.pages.len());
        let (sa, sb) = self.sel.ordered();
        let mut cards: Vec<(f32, Card, f32)> = Vec::new();
        let anchors = self.doc_comment_anchors();
        for (id, a) in anchors.iter() {
            // On a visible page (asked first: there can be thousands).
            let Some(c) = paged.caret(a.start.para, a.start.offset) else {
                continue;
            };
            if !visible.contains(&c.page) {
                continue;
            }
            // Replies show in their thread's card.
            let Some(root) = self.doc.thread_root(id) else {
                continue;
            };
            if root != *id {
                continue;
            }
            let (id, a) = (id.clone(), *a);
            let anchor = g.page_to_screen(c.page, c.x, c.top + c.height * 0.85);
            let (pw, _) = g.page_size(c.page);
            let right = g.page_to_screen(c.page, pw, c.top);
            let at = point(right.x + px(16.), g.page_to_screen(c.page, 0., c.top).y);
            let replies = self.doc.thread_replies(&id);
            let in_thread = |x: &str| x == id || replies.iter().any(|r| r.id == x);
            let active = (a.start <= sb && a.end >= sa)
                || self.comment_edit.as_ref().is_some_and(|e| in_thread(&e.id));
            let done = self
                .doc
                .comments()
                .iter()
                .find(|x| x.id == id)
                .is_some_and(|x| x.done);
            // Rough height: per comment a header, ~32 characters a line;
            // a resolved thread shows its first comment only.
            // Padding and the Reply / Resolve row, the Resolved label.
            let mut h = 64.0 + if done { 22.0 } else { 0.0 };
            let shown: Vec<&docs_engine::Comment> = self
                .doc
                .comments()
                .iter()
                .filter(|x| x.id == id || (!done && replies.iter().any(|r| r.id == x.id)))
                .collect();
            for x in shown {
                let editing = self.comment_edit.as_ref().is_some_and(|e| e.id == x.id);
                let lines = (x.text.chars().count() as f32 / 32.0).ceil().max(1.0);
                h += if editing { 130.0 } else { 58.0 + lines * 20.0 };
            }
            cards.push((
                f32::from(at.y),
                Card {
                    id,
                    anchor,
                    at,
                    active,
                    change: None,
                },
                h,
            ));
        }
        // Formatting changes: a balloon each, as Word shows them (the
        // text's; a header, footer or note being edited shows its changes
        // on the page only).
        let body_changes = if self.part.is_none() {
            self.doc_changes()
        } else {
            std::rc::Rc::default()
        };
        for c in body_changes.iter() {
            if !matches!(
                c.kind,
                docs_engine::RevKind::Format | docs_engine::RevKind::Insert
            ) {
                continue;
            }
            let Some(pc) = paged.caret(c.start.para, c.start.offset) else {
                continue;
            };
            if !visible.contains(&pc.page) {
                continue;
            }
            // A field code edited with Track Changes on: one balloon (on
            // its insertion; the codes are in the field's start mark).
            let code = (c.kind == docs_engine::RevKind::Insert)
                .then(|| self.doc.tracked_field_code(c.start))
                .flatten();
            if c.kind != docs_engine::RevKind::Format && code.is_none() {
                continue;
            }
            let anchor = g.page_to_screen(pc.page, pc.x, pc.top + pc.height * 0.85);
            let (pw, _) = g.page_size(pc.page);
            let right = g.page_to_screen(pc.page, pw, pc.top);
            let at = point(right.x + px(16.), g.page_to_screen(pc.page, 0., pc.top).y);
            let active = c.start <= sb && c.end >= sa;
            let label = match &code {
                Some((old, new)) => tf("Field code: {} (was {})", &[new, old]),
                None => format_label(&c.format),
            };
            let lines = (label.chars().count() as f32 / 32.0).ceil().max(1.0);
            cards.push((
                f32::from(at.y),
                Card {
                    id: format!("fmt-{}-{}-{}", c.start.para, c.start.offset, c.para as u8),
                    anchor,
                    at,
                    active,
                    change: Some(ChangeNote {
                        author: c.author.clone(),
                        date: c.date.clone(),
                        label,
                        start: c.start,
                        end: c.end,
                    }),
                },
                40.0 + lines * 18.0,
            ));
        }
        cards.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut bottom = f32::MIN;
        cards
            .into_iter()
            .map(|(y, mut card, h)| {
                let top = y.max(bottom + 8.0);
                card.at.y = px(top);
                bottom = top + h;
                card
            })
            .collect()
    }

    /// The comment cards over the page area.
    pub(crate) fn render_cards(&self, cards: &[Card], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = tokens(cx);
        let origin = self.viewport.get().origin;
        let view = cx.entity().downgrade();
        cards
            .iter()
            .filter_map(|card| {
                if let Some(note) = &card.change {
                    return Some(change_card(card, note, &view, &t, origin));
                }
                let c = self.doc.comments().iter().find(|c| c.id == card.id)?;
                let id = card.id.clone();
                let replies = self.doc.thread_replies(&id);
                let editing_here = self
                    .comment_edit
                    .as_ref()
                    .is_some_and(|e| e.id == id || replies.iter().any(|r| r.id == e.id));
                let mut col = v_flex().gap(SPACE_2);
                if c.done {
                    col = col.child(
                        h_flex()
                            .gap(SPACE_1)
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_dim)
                            .child(Icon::new(Lucide::CircleCheck).xsmall().text_color(t.fg_dim))
                            .child(i18n::t("Resolved")),
                    );
                }
                col = col.child(self.comment_block(c, true, &view, &t));
                if !c.done {
                    for r in &replies {
                        col = col.child(
                            div()
                                .pl(SPACE_3)
                                .border_l_2()
                                .border_color(t.border)
                                .child(self.comment_block(r, false, &view, &t)),
                        );
                    }
                } else if !replies.is_empty() {
                    let n = replies.len();
                    col = col.child(div().text_size(FONT_SMALL).text_color(t.fg_dim).child(
                        if n == 1 {
                            i18n::t("1 reply").to_owned()
                        } else {
                            tf("{} replies", &[&n])
                        },
                    ));
                }
                if !editing_here {
                    let (v1, v2, i1, i2) = (view.clone(), view.clone(), id.clone(), id.clone());
                    let done = c.done;
                    col = col.child(
                        h_flex()
                            .gap(SPACE_2)
                            .when(!done, |h| {
                                h.child(
                                    Button::new(SharedString::from(format!("comment-reply-{id}")))
                                        .ghost()
                                        .xsmall()
                                        .icon(Lucide::Reply)
                                        .label(i18n::t("Reply"))
                                        .on_click(move |_, window, cx| {
                                            let _ = v1.update(cx, |this, cx| {
                                                this.reply_comment_doc(&i1, window, cx)
                                            });
                                        }),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                Button::new(SharedString::from(format!("comment-resolve-{id}")))
                                    .ghost()
                                    .xsmall()
                                    .when(!done, |b| b.icon(Lucide::CircleCheck))
                                    .label(if done {
                                        i18n::t("Reopen")
                                    } else {
                                        i18n::t("Resolve")
                                    })
                                    .on_click(move |_, window, cx| {
                                        let _ = v2.update(cx, |this, cx| {
                                            this.resolve_comment_doc(&i2, !done, window, cx)
                                        });
                                    }),
                            ),
                    );
                }
                let v = view.clone();
                let go = id.clone();
                Some(
                    v_flex()
                        .id(SharedString::from(format!("comment-card-{id}")))
                        .absolute()
                        .left(card.at.x - origin.x)
                        .top(card.at.y - origin.y)
                        .w(px(CARD_W))
                        .p(SPACE_3)
                        .rounded(RADIUS_LG)
                        .bg(t.elevated)
                        .border_1()
                        .border_color(if card.active { t.accent } else { t.border })
                        .when(card.active, |d| d.shadow(t.shadow(Elevation::Popover)))
                        .when(c.done && !card.active, |d| d.opacity(0.7))
                        .text_size(FONT_SMALL)
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(move |_, window, cx| {
                            let _ = v.update(cx, |this, cx| {
                                if this.comment_edit.is_none() {
                                    this.go_to_comment(&go, window, cx)
                                }
                            });
                        })
                        .child(col)
                        .into_any_element(),
                )
            })
            .collect()
    }

    /// One comment of a thread: who wrote it when, its text (or its
    /// editor), and Edit / Delete buttons (Delete on the first comment
    /// deletes the thread, as in Word).
    fn comment_block(
        &self,
        c: &docs_engine::Comment,
        root: bool,
        view: &WeakEntity<DocsView>,
        t: &kantor_ui::tokens::Tokens,
    ) -> AnyElement {
        let id = c.id.clone();
        let editing = self.comment_edit.as_ref().filter(|e| e.id == c.id);
        let header = h_flex()
            .gap(SPACE_2)
            .child(
                div()
                    .flex_none()
                    .size(px(20.))
                    .rounded_full()
                    .bg(t.accent_soft)
                    .text_color(t.accent)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(FONT_SMALL)
                    .child(if c.initials.is_empty() {
                        c.author
                            .chars()
                            .next()
                            .map(String::from)
                            .unwrap_or_default()
                    } else {
                        c.initials.clone()
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_color(t.fg).truncate().child(c.author.clone()))
                    .child(
                        div()
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_dim)
                            .child(short_date(&c.date)),
                    ),
            )
            .when(editing.is_none(), |h| {
                let (v1, v2, i1, i2) = (view.clone(), view.clone(), id.clone(), id.clone());
                h.child(
                    Button::new(SharedString::from(format!("comment-edit-{id}")))
                        .ghost()
                        .xsmall()
                        .icon(Lucide::SquarePen)
                        .tooltip(i18n::t("Edit comment"))
                        .on_click(move |_, window, cx| {
                            let _ = v1.update(cx, |this, cx| {
                                this.save_comment_edit(window, cx);
                                this.edit_comment(i1.clone(), false, window, cx)
                            });
                        }),
                )
                .child(
                    Button::new(SharedString::from(format!("comment-delete-{id}")))
                        .ghost()
                        .xsmall()
                        .icon(Lucide::Trash)
                        .tooltip(if root {
                            i18n::t("Delete thread")
                        } else {
                            i18n::t("Delete comment")
                        })
                        .on_click(move |_, window, cx| {
                            let _ =
                                v2.update(cx, |this, cx| this.delete_comment_doc(&i2, window, cx));
                        }),
                )
            });
        let body: AnyElement = match editing {
            Some(e) => {
                let (v1, v2) = (view.clone(), view.clone());
                v_flex()
                    .gap(SPACE_2)
                    .child(Textarea::new(&e.input).small())
                    .child(
                        h_flex()
                            .gap(SPACE_2)
                            .child(div().flex_1())
                            .child(
                                Button::new("comment-cancel")
                                    .ghost()
                                    .xsmall()
                                    .label(i18n::t("Cancel"))
                                    .on_click(move |_, window, cx| {
                                        let _ = v1.update(cx, |this, cx| {
                                            this.cancel_comment_edit(window, cx)
                                        });
                                    }),
                            )
                            .child(
                                Button::new("comment-save")
                                    .primary()
                                    .xsmall()
                                    .label(if root {
                                        i18n::t("Comment")
                                    } else {
                                        i18n::t("Reply")
                                    })
                                    .tooltip("Ctrl+Enter")
                                    .on_click(move |_, window, cx| {
                                        let _ = v2.update(cx, |this, cx| {
                                            this.save_comment_edit(window, cx);
                                            this.focus(window, cx);
                                        });
                                    }),
                            ),
                    )
                    .into_any_element()
            }
            None => div()
                .text_color(t.fg)
                .whitespace_normal()
                .child(c.text.clone())
                .into_any_element(),
        };
        v_flex()
            .gap(SPACE_2)
            .child(header)
            .child(body)
            .into_any_element()
    }
}

/// A formatting change's balloon: who made it, when, and what it did;
/// a click selects the changed text, the buttons accept or reject it.
fn change_card(
    card: &Card,
    note: &ChangeNote,
    view: &WeakEntity<DocsView>,
    t: &kantor_ui::tokens::Tokens,
    origin: Point<Pixels>,
) -> AnyElement {
    let color = docs_pdf::build::author_color(&note.author);
    let ink = rgb((color.0[0] as u32) << 16 | (color.0[1] as u32) << 8 | color.0[2] as u32);
    let sel = Selection {
        anchor: note.start,
        head: note.end,
    };
    let caret = Selection::caret(note.start);
    let (v1, v2, v3) = (view.clone(), view.clone(), view.clone());
    let id = card.id.clone();
    v_flex()
        .id(SharedString::from(format!("change-card-{id}")))
        .absolute()
        .left(card.at.x - origin.x)
        .top(card.at.y - origin.y)
        .w(px(CARD_W))
        .gap(SPACE_1)
        .p(SPACE_2)
        .rounded(RADIUS_LG)
        .bg(t.elevated)
        .border_1()
        .border_l_4()
        .border_color(if card.active { t.accent } else { t.border })
        .when(card.active, |d| d.shadow(t.shadow(Elevation::Popover)))
        .text_size(FONT_SMALL)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| {
            let _ = v1.update(cx, |this, cx| {
                this.sel = sel;
                this.reveal_caret.set(true);
                this.focus(window, cx);
                cx.notify();
            });
        })
        .child(
            h_flex()
                .gap(SPACE_2)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(ink)
                        .child(note.author.clone()),
                )
                .child(div().text_color(t.fg_dim).child(short_date(&note.date)))
                .child(
                    Button::new(SharedString::from(format!("change-accept-{id}")))
                        .ghost()
                        .xsmall()
                        .icon(Lucide::Check)
                        .tooltip(i18n::t("Accept change"))
                        .on_click(move |_, window, cx| {
                            let _ = v2.update(cx, |this, cx| {
                                this.sel = caret;
                                this.accept_change_doc(window, cx)
                            });
                        }),
                )
                .child(
                    Button::new(SharedString::from(format!("change-reject-{id}")))
                        .ghost()
                        .xsmall()
                        .icon(Lucide::X)
                        .tooltip(i18n::t("Reject change"))
                        .on_click(move |_, window, cx| {
                            let _ = v3.update(cx, |this, cx| {
                                this.sel = caret;
                                this.reject_change_doc(window, cx)
                            });
                        }),
                ),
        )
        .child(
            div()
                .text_color(t.fg)
                .whitespace_normal()
                .child(note.label.clone()),
        )
        .into_any_element()
}

/// The toolbar's Review menu.
pub(crate) fn review_button(view: &DocsView, cx: &mut Context<DocsView>) -> impl IntoElement {
    let v = cx.entity().downgrade();
    let changes = view.change_count();
    let comments = view.main_doc().comments().len();
    let track = view.doc.track_changes();
    let tip = match (changes, comments) {
        (0, 0) => t("Review: comments and tracked changes").to_owned(),
        (1, 0) => t("Review: 1 tracked change").to_owned(),
        (c, 0) => tf("Review: {} tracked changes", &[&c]),
        (0, 1) => t("Review: 1 comment").to_owned(),
        (0, n) => tf("Review: {} comments", &[&n]),
        (c, n) => tf("Review: {} tracked changes, {} comments", &[&c, &n]),
    };
    let tip = if track {
        tf("{} (Track Changes is on)", &[&tip])
    } else {
        tip
    };
    Button::new("docs-review")
        .ghost()
        .xsmall()
        .icon(if track {
            Lucide::FileDiff
        } else {
            Lucide::MessageSquare
        })
        .dropdown_caret(true)
        .tooltip(tip)
        .dropdown_menu(move |menu, _, cx| {
            let on = kantor_ui::spelling::enabled(cx);
            review_items(menu, &v, track).separator().item(
                PopupMenuItem::new(t("Check Spelling as You Type"))
                    .checked(on)
                    .on_click(|_, _, cx| kantor_ui::spelling::toggle(cx)),
            )
        })
}

fn review_items(menu: PopupMenu, view: &WeakEntity<DocsView>, track: bool) -> PopupMenu {
    type Run = fn(&mut DocsView, &mut Window, &mut Context<DocsView>);
    let item = |menu: PopupMenu, label: &'static str, icon: Option<Lucide>, run: Run| {
        let v = view.clone();
        let mut it = PopupMenuItem::new(t(label)).on_click(move |_, window, cx| {
            let _ = v.update(cx, |this, cx| run(this, window, cx));
        });
        if let Some(i) = icon {
            it = it.icon(i);
        }
        menu.item(it)
    };
    let v = view.clone();
    let menu = menu
        .item(
            PopupMenuItem::new(t("Track Changes  Ctrl+Shift+E"))
                .checked(track)
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| this.toggle_track_changes_doc(window, cx));
                }),
        )
        .separator();
    let menu = item(menu, tk("Insert footnote  Ctrl+Alt+F"), None, |v, w, cx| {
        v.insert_note_doc(docs_engine::NoteKind::Footnote, w, cx)
    });
    let menu = item(menu, tk("Insert endnote  Ctrl+Alt+D"), None, |v, w, cx| {
        v.insert_note_doc(docs_engine::NoteKind::Endnote, w, cx)
    })
    .separator();
    let menu = item(
        menu,
        tk("New comment  Ctrl+Alt+M"),
        Some(Lucide::MessageSquarePlus),
        |v, w, cx| v.new_comment_doc(w, cx),
    );
    let menu = item(menu, tk("Show or hide comments"), None, |v, _, cx| {
        v.toggle_comments_doc(cx)
    })
    .separator();
    let menu = item(
        menu,
        tk("Accept change"),
        Some(Lucide::Check),
        |v, w, cx| v.accept_change_doc(w, cx),
    );
    let menu = item(menu, tk("Reject change"), Some(Lucide::X), |v, w, cx| {
        v.reject_change_doc(w, cx)
    });
    let menu = item(
        menu,
        tk("Next change"),
        Some(Lucide::ChevronDown),
        |v, w, cx| v.next_change_doc(true, w, cx),
    );
    let menu = item(
        menu,
        tk("Previous change"),
        Some(Lucide::ChevronUp),
        |v, w, cx| v.next_change_doc(false, w, cx),
    )
    .separator();
    let menu = item(
        menu,
        tk("Accept all changes"),
        Some(Lucide::CheckCheck),
        |v, w, cx| v.resolve_all_doc(true, w, cx),
    );
    item(menu, tk("Reject all changes"), None, |v, w, cx| {
        v.resolve_all_doc(false, w, cx)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_engine::Position;

    #[std::prelude::v1::test]
    fn short_dates() {
        assert_eq!(short_date("2026-11-03T09:00:00Z"), "3 Nov 2026");
        assert_eq!(short_date(""), "");
    }

    #[std::prelude::v1::test]
    fn format_labels_read_like_words() {
        use docs_engine::{Align, FormatItem as F};
        assert_eq!(format_label(&[F::Bold(true)]), "Formatted: Font: Bold");
        assert_eq!(
            format_label(&[F::Align(Align::Center), F::Indent]),
            "Formatted: Centered, Indent"
        );
        assert_eq!(format_label(&[F::Size(Some(24))]), "Formatted: Font: 12 pt");
    }

    #[std::prelude::v1::test]
    fn position_is_ordered() {
        assert!(Position::new(1, 0) > Position::new(0, 9));
    }
}
