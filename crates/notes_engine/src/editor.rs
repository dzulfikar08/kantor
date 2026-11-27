//! The note being edited: buffer, parse, selection, undo history and the
//! editing commands (typing, deleting, Enter that continues lists, Tab that
//! indents them, Ctrl+B/I/K wrapping, task check boxes).
//!
//! Every change is a set of plain text replacements on the buffer. Undo
//! replays them backwards; nothing is ever rebuilt from the parse, so a
//! file only changes where the user changed it.

use std::ops::Range;

use crate::buffer::{Buffer, OpenError};
use crate::parse::{self, Parsed};
use crate::prefix::line_prefix;

/// A selection: `anchor` stays, `head` moves (the caret). Byte offsets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(at: usize) -> Self {
        Self {
            anchor: at,
            head: at,
        }
    }

    pub fn new(anchor: usize, head: usize) -> Self {
        Self { anchor, head }
    }

    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

/// One replacement, as applied: `old` at `start` became `new`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Change {
    start: usize,
    old: String,
    new: String,
}

/// How an edit joins the undo step before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Merge {
    /// Its own undo step.
    No,
    /// Typing: joins the previous typing step while words go on.
    Typing,
    /// Backspace / Delete runs.
    Deleting,
    /// Input-method composition updates.
    Compose,
}

#[derive(Clone, Debug)]
struct Step {
    changes: Vec<Change>,
    before: Selection,
    after: Selection,
    merge: Merge,
    /// Identifies the text before / after this step (for "is it saved?").
    id_before: u64,
    id_after: u64,
}

/// A note: its text and what the editor knows about it.
#[derive(Clone, Debug)]
pub struct Editor {
    buf: Buffer,
    parsed: Parsed,
    sel: Selection,
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// The text's identity: changes with every edit; undo brings back the
    /// identity the text had before.
    state: u64,
    next_state: u64,
    saved: u64,
    /// Bumped by every change, undo and redo (never goes back).
    revision: u64,
    /// The last step may not take more typing (the caret moved).
    sealed: bool,
}

/// The line ending new lines get.
fn eol_str(buf: &Buffer) -> &'static str {
    buf.eol().as_str()
}

impl Default for Editor {
    fn default() -> Self {
        Self::with_buffer(Buffer::default())
    }
}

impl Editor {
    pub fn new(text: &str) -> Self {
        Self::with_buffer(Buffer::new(text))
    }

    /// A note from a file's bytes (must be UTF-8).
    pub fn open(bytes: &[u8]) -> Result<Self, OpenError> {
        Ok(Self::with_buffer(Buffer::from_bytes(bytes)?))
    }

    fn with_buffer(buf: Buffer) -> Self {
        let parsed = parse::parse(&buf);
        Self {
            buf,
            parsed,
            sel: Selection::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            state: 0,
            next_state: 1,
            saved: 0,
            revision: 0,
            sealed: false,
        }
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buf
    }

    pub fn text(&self) -> &str {
        self.buf.text()
    }

    pub fn parsed(&self) -> &Parsed {
        &self.parsed
    }

    pub fn selection(&self) -> Selection {
        self.sel
    }

    pub fn selected_text(&self) -> &str {
        self.buf.slice(self.sel.range())
    }

    /// Move the selection (clamped to valid offsets). Ends a typing run.
    pub fn set_selection(&mut self, sel: Selection) {
        let sel = Selection::new(self.buf.clamp(sel.anchor), self.buf.clamp(sel.head));
        if sel != self.sel {
            self.sealed = true;
        }
        self.sel = sel;
    }

    pub fn select_all(&mut self) {
        self.set_selection(Selection::new(0, self.buf.len()));
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn is_dirty(&self) -> bool {
        self.state != self.saved
    }

    /// The text as it is now was written to the file.
    pub fn mark_saved(&mut self) {
        self.saved = self.state;
        // Typing after a save is a new undo step (undo returns to the save).
        self.sealed = true;
    }

    /// The bytes Save writes.
    pub fn to_bytes(&self) -> Vec<u8> {
        self.buf.to_bytes()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    // ── changes ────────────────────────────────────────────────────────────

    /// Apply `edits` (ranges in the current text, not overlapping, any
    /// order) as one step, then select `after` (in the new text, or `None`:
    /// a caret after the last edit).
    pub fn edit(
        &mut self,
        mut edits: Vec<(Range<usize>, String)>,
        after: Option<Selection>,
        merge: Merge,
    ) {
        if edits.iter().all(|(r, t)| r.is_empty() && t.is_empty()) {
            return;
        }
        // Back to front, so earlier ranges stay valid.
        edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        let before = self.sel;
        let mut changes = Vec::with_capacity(edits.len());
        let mut caret = 0;
        let mut first = true;
        for (r, t) in edits {
            let old = self.buf.replace(r.clone(), &t);
            if first {
                caret = r.start + t.len();
                first = false;
            } else {
                // An earlier edit shifts the caret of a later one.
                caret = (caret as isize + t.len() as isize - old.len() as isize) as usize;
            }
            changes.push(Change {
                start: r.start,
                old,
                new: t,
            });
        }
        let after = after.unwrap_or(Selection::caret(caret));
        self.sel = Selection::new(self.buf.clamp(after.anchor), self.buf.clamp(after.head));
        self.record(changes, before, merge);
        self.changed();
    }

    fn record(&mut self, changes: Vec<Change>, before: Selection, merge: Merge) {
        self.redo.clear();
        let can_join = !self.sealed
            && merge != Merge::No
            && self.undo.last().is_some_and(|s| {
                s.merge == merge && s.id_after == self.state && joins(s, &changes, merge)
            });
        if can_join {
            let s = self.undo.last_mut().expect("checked");
            s.changes.extend(changes);
            s.after = self.sel;
            s.id_after = self.next_state;
        } else {
            self.undo.push(Step {
                changes,
                before,
                after: self.sel,
                merge,
                id_before: self.state,
                id_after: self.next_state,
            });
        }
        self.state = self.next_state;
        self.next_state += 1;
        self.sealed = false;
    }

    fn changed(&mut self) {
        self.parsed = parse::parse(&self.buf);
        self.revision += 1;
    }

    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else {
            return false;
        };
        for c in step.changes.iter().rev() {
            self.buf.replace(c.start..c.start + c.new.len(), &c.old);
        }
        self.sel = step.before;
        self.state = step.id_before;
        self.redo.push(step);
        self.sealed = true;
        self.changed();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else {
            return false;
        };
        for c in &step.changes {
            self.buf.replace(c.start..c.start + c.old.len(), &c.new);
        }
        self.sel = step.after;
        self.state = step.id_after;
        self.undo.push(step);
        self.sealed = true;
        self.changed();
        true
    }

    // ── typing and deleting ────────────────────────────────────────────────

    /// Replace the selection with `text` (its line breaks become the note's).
    pub fn insert(&mut self, text: &str, merge: Merge) {
        let text = self.buf.normalize(text);
        let r = self.sel.range();
        self.edit(vec![(r, text)], None, merge);
    }

    /// Typed text.
    pub fn type_text(&mut self, text: &str) {
        self.insert(text, Merge::Typing);
    }

    /// Replace `range` (in the current text) with `text`, selecting
    /// `select` within the new text (relative to `range.start`), e.g. an
    /// input method's preedit.
    pub fn replace_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        select: Option<Range<usize>>,
        merge: Merge,
    ) {
        let text = self.buf.normalize(text);
        let start = range.start;
        let after = select.map(|s| Selection::new(start + s.start, start + s.end));
        self.edit(vec![(range, text)], after, merge);
    }

    fn delete_range(&mut self, r: Range<usize>) {
        if !r.is_empty() {
            self.edit(vec![(r, String::new())], None, Merge::Deleting);
        }
    }

    pub fn backspace(&mut self) {
        if !self.sel.is_empty() {
            return self.delete_range(self.sel.range());
        }
        let at = self.sel.head;
        // At a list item's text start: the marker goes first (the line
        // becomes plain text, or a level up when nested).
        let line = self.buf.line_of(at);
        let start = self.buf.line_start(line);
        let p = line_prefix(self.buf.line(line));
        if p.list.is_some() && at == start + p.len && at > start {
            let keep = p.quote.len() + p.indent.len();
            return self.delete_range(start + keep..at);
        }
        let from = self.buf.prev_grapheme(at);
        self.delete_range(from..at);
    }

    pub fn delete(&mut self) {
        if !self.sel.is_empty() {
            return self.delete_range(self.sel.range());
        }
        let to = self.buf.next_grapheme(self.sel.head);
        self.delete_range(self.sel.head..to);
    }

    pub fn delete_word_back(&mut self) {
        if !self.sel.is_empty() {
            return self.delete_range(self.sel.range());
        }
        let from = self.buf.prev_word(self.sel.head);
        self.delete_range(from..self.sel.head);
    }

    pub fn delete_word_forward(&mut self) {
        if !self.sel.is_empty() {
            return self.delete_range(self.sel.range());
        }
        let to = self.buf.next_word(self.sel.head);
        self.delete_range(self.sel.head..to);
    }

    // ── Markdown helpers ───────────────────────────────────────────────────

    /// Enter: a new line that continues the list or quote; on an empty
    /// item it ends the list instead (the marker is removed).
    pub fn newline(&mut self) {
        let r = self.sel.range();
        let line = self.buf.line_of(r.start);
        let start = self.buf.line_start(line);
        let text = self.buf.line(line).to_owned();
        let p = line_prefix(&text);
        let in_code = self.in_code_block(line);
        let eol = eol_str(&self.buf);
        if !in_code && r.start >= start + p.len {
            let empty_item = p.list.is_some() && text[p.len..].trim().is_empty();
            if empty_item && r.is_empty() {
                // `- ` alone: end the list (keep quote and outer indentation).
                let keep = p.quote.len() + p.indent.len();
                let end = self.buf.line_range(line).end;
                self.edit(vec![(start + keep..end, String::new())], None, Merge::No);
                return;
            }
            if p.list.is_none() && !p.quote.is_empty() && text[p.len..].trim().is_empty() {
                // `> ` alone: leave the quote.
                let end = self.buf.line_range(line).end;
                self.edit(vec![(start..end, String::new())], None, Merge::No);
                return;
            }
            let cont = p.continuation();
            self.edit(vec![(r, format!("{eol}{cont}"))], None, Merge::No);
            return;
        }
        // Code keeps the line's indentation.
        let indent: String = if in_code {
            text.chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect()
        } else {
            String::new()
        };
        self.edit(vec![(r, format!("{eol}{indent}"))], None, Merge::No);
    }

    /// Shift+Enter: a plain new line.
    pub fn plain_newline(&mut self) {
        let eol = eol_str(&self.buf).to_owned();
        self.edit(vec![(self.sel.range(), eol)], None, Merge::No);
    }

    fn in_code_block(&self, line: usize) -> bool {
        matches!(
            self.parsed.lines.get(line).map(|l| &l.kind),
            Some(parse::LineKind::Code { .. })
        )
    }

    /// Lines the selection covers (a selection ending at a line start
    /// leaves that line out).
    fn selected_lines(&self) -> Range<usize> {
        let r = self.sel.range();
        let first = self.buf.line_of(r.start);
        let mut last = self.buf.line_of(r.end);
        if last > first && r.end == self.buf.line_start(last) {
            last -= 1;
        }
        first..last + 1
    }

    /// Tab: indent the selected list items (a level deeper); elsewhere
    /// four spaces, or every selected line when several are selected.
    pub fn indent(&mut self) {
        let lines = self.selected_lines();
        let items = lines
            .clone()
            .any(|l| line_prefix(self.buf.line(l)).list.is_some());
        if !items && lines.len() == 1 {
            return self.insert("    ", Merge::No);
        }
        let mut edits = Vec::new();
        for l in lines {
            let text = self.buf.line(l);
            if text.trim().is_empty() {
                continue;
            }
            let p = line_prefix(text);
            let unit = self.indent_unit(l);
            let at = self.buf.line_start(l) + p.quote.len();
            edits.push((at..at, unit));
        }
        self.edit_keeping_selection(edits);
    }

    /// Shift+Tab: one level less.
    pub fn outdent(&mut self) {
        let mut edits = Vec::new();
        for l in self.selected_lines() {
            let p = line_prefix(self.buf.line(l));
            if p.indent.is_empty() {
                continue;
            }
            let unit = self.indent_unit(l).len();
            let take = if p.indent.starts_with('\t') {
                1
            } else {
                p.indent
                    .bytes()
                    .take_while(|&b| b == b' ')
                    .count()
                    .min(unit)
            };
            let at = self.buf.line_start(l) + p.quote.len();
            edits.push((at..at + take, String::new()));
        }
        self.edit_keeping_selection(edits);
    }

    /// The indentation one list level adds at `line`: the width of the
    /// marker of the item above it (2 for `- `, 3 for `1. `), or a tab in
    /// lists indented with tabs.
    fn indent_unit(&self, line: usize) -> String {
        let own = line_prefix(self.buf.line(line));
        for l in (0..line).rev() {
            let p = line_prefix(self.buf.line(l));
            if p.list.is_none() || p.quote != own.quote {
                if self.buf.line(l).trim().is_empty() {
                    continue;
                }
                break;
            }
            if p.indent.len() <= own.indent.len() {
                if p.indent.contains('\t') {
                    return "\t".into();
                }
                return " ".repeat(p.child_indent().max(2));
            }
        }
        if own.indent.contains('\t') {
            return "\t".into();
        }
        " ".repeat(own.child_indent().clamp(2, 4))
    }

    /// Apply line edits and move the selection along with the text.
    fn edit_keeping_selection(&mut self, edits: Vec<(Range<usize>, String)>) {
        if edits.is_empty() {
            return;
        }
        let map = |o: usize| {
            let mut out = o as isize;
            for (r, t) in &edits {
                if r.start < o {
                    let removed = r.end.min(o) - r.start;
                    out += t.len() as isize - removed as isize;
                }
            }
            out.max(0) as usize
        };
        let after = Selection::new(map(self.sel.anchor), map(self.sel.head));
        self.edit(edits, Some(after), Merge::No);
    }

    /// Ctrl+B / Ctrl+I / strikethrough / inline code: wrap the selection in
    /// `mark`, or unwrap it when it is wrapped already. With no selection,
    /// the word at the caret; between words, an empty pair to type into.
    pub fn toggle_wrap(&mut self, mark: &str) {
        let mut r = self.sel.range();
        if r.is_empty() {
            let w = self.buf.word_at(r.start);
            if !w.is_empty() && self.buf.slice(w.clone()).chars().any(char::is_alphanumeric) {
                r = w;
            }
        }
        let text = self.buf.text();
        let m = mark.len();
        // Wrapped from outside: **[sel]**.
        if r.start >= m
            && r.end + m <= text.len()
            && text.is_char_boundary(r.start - m)
            && text.is_char_boundary(r.end + m)
            && &text[r.start - m..r.start] == mark
            && &text[r.end..r.end + m] == mark
            && !is_longer_run(text, r.start - m, r.end + m, mark)
        {
            let after = Selection::new(r.start - m, r.end - m);
            self.edit(
                vec![
                    (r.start - m..r.start, String::new()),
                    (r.end..r.end + m, String::new()),
                ],
                Some(after),
                Merge::No,
            );
            return;
        }
        // Selected with the marks: [**sel**].
        let inner = &text[r.clone()];
        if inner.len() >= 2 * m && inner.starts_with(mark) && inner.ends_with(mark) {
            let after = Selection::new(r.start, r.end - 2 * m);
            self.edit(
                vec![
                    (r.start..r.start + m, String::new()),
                    (r.end - m..r.end, String::new()),
                ],
                Some(after),
                Merge::No,
            );
            return;
        }
        let after = Selection::new(r.start + m, r.end + m);
        self.edit(
            vec![(r.start..r.start, mark.into()), (r.end..r.end, mark.into())],
            Some(after),
            Merge::No,
        );
    }

    /// Ctrl+K: `[selection](|)`, or `[|](url)` when the selection is a URL.
    pub fn insert_link(&mut self) {
        let r = self.sel.range();
        let sel = self.buf.slice(r.clone()).to_owned();
        let looks_like_url =
            sel.starts_with("http://") || sel.starts_with("https://") || sel.starts_with("mailto:");
        if looks_like_url {
            let at = r.start + 1;
            self.edit(
                vec![(r, format!("[]({sel})"))],
                Some(Selection::caret(at)),
                Merge::No,
            );
        } else {
            let at = r.start + 1 + sel.len() + 2;
            self.edit(
                vec![(r, format!("[{sel}]()"))],
                Some(Selection::caret(at)),
                Merge::No,
            );
        }
    }

    /// Check or uncheck the task item on `line`. The selection stays.
    pub fn toggle_task(&mut self, line: usize) -> bool {
        if line >= self.buf.line_count() {
            return false;
        }
        let p = line_prefix(self.buf.line(line));
        let Some(item) = &p.list else {
            return false;
        };
        let Some(checked) = item.task else {
            return false;
        };
        let start = self.buf.line_start(line);
        let bullet = item.bullet.text().len() + item.gap.len();
        let at = start + p.quote.len() + p.indent.len() + bullet + 1;
        let sel = self.sel;
        self.edit(
            vec![(at..at + 1, if checked { " " } else { "x" }.into())],
            Some(sel),
            Merge::No,
        );
        true
    }

    /// Toolbar list and quote buttons: give the selected lines `prefix`
    /// (`- `, `1. `, `- [ ] `, `> `), or take it off when they all have it.
    /// A list marker of another kind is replaced.
    pub fn toggle_line_prefix(&mut self, prefix: &str) {
        #[derive(PartialEq)]
        enum Kind {
            Quote,
            Bullet,
            Number,
            Task,
        }
        let kind_of = |p: &crate::prefix::LinePrefix| match &p.list {
            Some(item) if item.task.is_some() => Some(Kind::Task),
            Some(item) if matches!(item.bullet, crate::prefix::Bullet::Number { .. }) => {
                Some(Kind::Number)
            }
            Some(_) => Some(Kind::Bullet),
            None => None,
        };
        let want = if prefix.starts_with('>') {
            Kind::Quote
        } else if prefix.contains('[') {
            Kind::Task
        } else if prefix.starts_with(|c: char| c.is_ascii_digit()) {
            Kind::Number
        } else {
            Kind::Bullet
        };
        let lines = self.selected_lines();
        let several = lines.len() > 1;
        let has = |l: usize| {
            let p = line_prefix(self.buf.line(l));
            if want == Kind::Quote {
                !p.quote.is_empty()
            } else {
                kind_of(&p) == Some(Kind::Bullet) && want == Kind::Bullet
                    || kind_of(&p) == Some(Kind::Number) && want == Kind::Number
                    || kind_of(&p) == Some(Kind::Task) && want == Kind::Task
            }
        };
        let targets: Vec<usize> = lines
            .clone()
            .filter(|&l| !several || !self.buf.line(l).trim().is_empty())
            .collect();
        let remove = !targets.is_empty() && targets.iter().all(|&l| has(l));
        let mut edits = Vec::new();
        let mut n = 0u64;
        for l in targets {
            let start = self.buf.line_start(l);
            let p = line_prefix(self.buf.line(l));
            let marker_at = start + p.quote.len() + p.indent.len();
            if want == Kind::Quote {
                if remove {
                    edits.push((start..start + p.quote.len(), String::new()));
                } else {
                    edits.push((start..start, "> ".to_owned()));
                }
                continue;
            }
            let old_end = if p.list.is_some() {
                start + p.len
            } else {
                marker_at
            };
            if remove {
                edits.push((marker_at..old_end, String::new()));
            } else {
                n += 1;
                let new = if want == Kind::Number {
                    format!("{n}. ")
                } else {
                    prefix.to_owned()
                };
                edits.push((marker_at..old_end, new));
            }
        }
        self.edit_keeping_selection(edits);
    }

    /// Make the caret's line a heading of `level` (0: plain text).
    pub fn set_heading(&mut self, level: u8) {
        let line = self.buf.line_of(self.sel.head);
        let start = self.buf.line_start(line);
        let text = self.buf.line(line);
        let p = line_prefix(text);
        let rest = &text[p.quote.len()..];
        let hashes = rest.bytes().take_while(|&b| b == b'#').count();
        let old_len = if (1..=6).contains(&hashes) {
            hashes + rest[hashes..].bytes().take_while(|&b| b == b' ').count()
        } else {
            0
        };
        let new = if level == 0 {
            String::new()
        } else {
            format!("{} ", "#".repeat(level.min(6) as usize))
        };
        let at = start + p.quote.len();
        let delta = new.len() as isize - old_len as isize;
        let shift = |o: usize| {
            if o >= at + old_len {
                (o as isize + delta) as usize
            } else {
                at + new.len()
            }
        };
        let after = Selection::new(shift(self.sel.anchor), shift(self.sel.head));
        self.edit(
            vec![(at..at + old_len, new.clone())],
            Some(after),
            Merge::No,
        );
    }
}

/// `text[start..end]` is the inside of a longer run of `mark`'s character
/// (`***x***` when unwrapping `*`): unwrapping there would break it.
fn is_longer_run(text: &str, start: usize, end: usize, mark: &str) -> bool {
    let c = mark.as_bytes()[0];
    let before = start > 0 && text.as_bytes()[start - 1] == c;
    let after = end < text.len() && text.as_bytes()[end] == c;
    before && after && mark.len() == 1
}

/// Whether `changes` continue `step` (typing at its caret, deleting next
/// to it).
fn joins(step: &Step, changes: &[Change], merge: Merge) -> bool {
    let [c] = changes else {
        return false;
    };
    let Some(last) = step.changes.last() else {
        return false;
    };
    match merge {
        Merge::Typing => {
            // A space after a word starts a new step.
            let space_after_word =
                c.new.starts_with(char::is_whitespace) && !last.new.ends_with(char::is_whitespace);
            c.old.is_empty() && c.start == last.start + last.new.len() && !space_after_word
        }
        Merge::Deleting => {
            c.new.is_empty() && (c.start + c.old.len() == last.start || c.start == last.start)
        }
        Merge::Compose => true,
        Merge::No => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ed(text: &str, at: usize) -> Editor {
        let mut e = Editor::new(text);
        e.set_selection(Selection::caret(at));
        e
    }

    #[test]
    fn saving_unedited_writes_the_same_bytes() {
        let src = b"\xef\xbb\xbf# T\r\n\r\n- a\r\n- b";
        let e = Editor::open(src).unwrap();
        assert_eq!(e.to_bytes(), src);
        assert!(!e.is_dirty());
    }

    #[test]
    fn typing_merges_into_one_undo_step_per_word() {
        let mut e = ed("", 0);
        for c in ["h", "i", " ", "y", "o"] {
            e.type_text(c);
        }
        assert_eq!(e.text(), "hi yo");
        assert!(e.undo());
        assert_eq!(e.text(), "hi");
        assert!(e.undo());
        assert_eq!(e.text(), "");
        assert!(!e.is_dirty());
        assert!(e.redo());
        assert!(e.redo());
        assert_eq!(e.text(), "hi yo");
        assert!(e.is_dirty());
    }

    #[test]
    fn undo_back_to_saved_is_clean() {
        let mut e = ed("abc", 3);
        e.type_text("d");
        e.mark_saved();
        e.type_text("e");
        assert!(e.is_dirty());
        e.undo();
        // "e" merged into "d"'s step after the save? No: the save sealed it.
        assert_eq!(e.text(), "abcd");
        assert!(!e.is_dirty());
    }

    #[test]
    fn enter_continues_lists_and_ends_empty_items() {
        let mut e = ed("- one", 5);
        e.newline();
        assert_eq!(e.text(), "- one\n- ");
        e.newline();
        assert_eq!(e.text(), "- one\n");
        let mut e = ed("1. a\r\n", 4);
        e.newline();
        assert_eq!(e.text(), "1. a\r\n2. \r\n");
        let mut e = ed("> - [x] done", 12);
        e.newline();
        assert_eq!(e.text(), "> - [x] done\n> - [ ] ");
        let mut e = ed("plain", 5);
        e.newline();
        assert_eq!(e.text(), "plain\n");
    }

    #[test]
    fn enter_in_code_keeps_indentation() {
        let mut e = ed("```\n    x\n```", 9);
        e.newline();
        assert_eq!(e.text(), "```\n    x\n    \n```");
    }

    #[test]
    fn tab_indents_list_items() {
        let mut e = ed("- a\n- b", 7);
        e.indent();
        assert_eq!(e.text(), "- a\n  - b");
        assert_eq!(e.selection(), Selection::caret(9));
        e.outdent();
        assert_eq!(e.text(), "- a\n- b");
        let mut e = ed("1. a\n2. b", 9);
        e.indent();
        assert_eq!(e.text(), "1. a\n   2. b");
    }

    #[test]
    fn wrap_and_unwrap() {
        let mut e = Editor::new("make bold now");
        e.set_selection(Selection::new(5, 9));
        e.toggle_wrap("**");
        assert_eq!(e.text(), "make **bold** now");
        assert_eq!(e.selected_text(), "bold");
        e.toggle_wrap("**");
        assert_eq!(e.text(), "make bold now");
        let mut e = ed("x  y", 2);
        e.toggle_wrap("*");
        assert_eq!(e.text(), "x ** y");
        assert_eq!(e.selection(), Selection::caret(3));
    }

    #[test]
    fn links_and_tasks() {
        let mut e = Editor::new("see docs");
        e.set_selection(Selection::new(4, 8));
        e.insert_link();
        assert_eq!(e.text(), "see [docs]()");
        assert_eq!(e.selection(), Selection::caret(11));
        let mut e = ed("- [ ] a\n- [x] b", 0);
        assert!(e.toggle_task(0));
        assert!(e.toggle_task(1));
        assert_eq!(e.text(), "- [x] a\n- [ ] b");
        assert!(!e.toggle_task(5));
    }

    #[test]
    fn backspace_removes_a_list_marker_first() {
        let mut e = ed("  - item", 4);
        e.backspace();
        assert_eq!(e.text(), "  item");
    }

    #[test]
    fn paste_takes_the_notes_line_ending() {
        let mut e = ed("a\r\nb", 1);
        e.insert("x\ny", Merge::No);
        assert_eq!(e.text(), "ax\r\ny\r\nb");
    }

    #[test]
    fn list_buttons() {
        let mut e = Editor::new("a\nb");
        e.select_all();
        e.toggle_line_prefix("1. ");
        assert_eq!(e.text(), "1. a\n2. b");
        e.select_all();
        e.toggle_line_prefix("- ");
        assert_eq!(e.text(), "- a\n- b");
        e.select_all();
        e.toggle_line_prefix("- ");
        assert_eq!(e.text(), "a\nb");
        let mut e = ed("x", 1);
        e.toggle_line_prefix("> ");
        assert_eq!(e.text(), "> x");
        assert_eq!(e.selection(), Selection::caret(3));
        e.toggle_line_prefix("> ");
        assert_eq!(e.text(), "x");
    }

    #[test]
    fn headings() {
        let mut e = ed("Title", 2);
        e.set_heading(2);
        assert_eq!(e.text(), "## Title");
        assert_eq!(e.selection(), Selection::caret(5));
        e.set_heading(0);
        assert_eq!(e.text(), "Title");
    }
}
