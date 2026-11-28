//! Typed text and input-method composition through GPUI's
//! [`EntityInputHandler`].
//!
//! As in Docs, the input method sees **the caret's line** as "the text"
//! (UTF-16 offsets into it): IMEs work around the caret, and the offsets
//! stay small. A selection over several lines is reported as the caret;
//! typing still replaces all of it.
//!
//! The preedit is inserted into the buffer like typed text; its updates
//! and the commit are one undo step ([`Merge::Compose`]).

use std::ops::Range;

use gpui_kit::*;
use notes_engine::{Merge, Selection};

use crate::NotesView;

fn to_utf16(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].encode_utf16().count()
}

fn from_utf16(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, ch) in text.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += ch.len_utf16();
    }
    text.len()
}

impl NotesView {
    /// The caret's line: its start and its text.
    fn ime_line(&self) -> (usize, &str) {
        let buf = self.editor.buffer();
        let line = buf.line_of(self.editor.selection().head);
        (buf.line_start(line), buf.line(line))
    }

    fn ime_bytes(&self, r: &Range<usize>) -> Range<usize> {
        let (start, text) = self.ime_line();
        start + from_utf16(text, r.start)..start + from_utf16(text, r.end)
    }

    fn ime_utf16(&self, r: &Range<usize>) -> Range<usize> {
        let (start, text) = self.ime_line();
        let rel = |o: usize| o.saturating_sub(start).min(text.len());
        to_utf16(text, rel(r.start))..to_utf16(text, rel(r.end))
    }

    /// What an input-method edit replaces: the given range, else the
    /// composition, else the selection.
    fn ime_target(&self, range: Option<Range<usize>>) -> Range<usize> {
        range
            .map(|r| self.ime_bytes(&r))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.editor.selection().range())
    }
}

impl EntityInputHandler for NotesView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes = self.ime_bytes(&range);
        *adjusted = Some(self.ime_utf16(&bytes));
        Some(self.editor.buffer().slice(bytes).to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let sel = self.editor.selection();
        let buf = self.editor.buffer();
        let same_line = buf.line_of(sel.anchor) == buf.line_of(sel.head);
        let anchor = if same_line { sel.anchor } else { sel.head };
        let (a, b) = (anchor.min(sel.head), anchor.max(sel.head));
        Some(UTF16Selection {
            range: self.ime_utf16(&(a..b)),
            reversed: sel.head < anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| self.ime_utf16(m))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.editable() {
            return;
        }
        let target = self.ime_target(range);
        let merge = if self.marked.take().is_some() {
            Merge::Compose
        } else {
            Merge::Typing
        };
        self.editor.replace_range(target, text, None, merge);
        self.edited(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.editable() {
            return;
        }
        let target = self.ime_target(range);
        let start = target.start;
        let select = new_selected_range.as_ref().map(|r| {
            let a = from_utf16(new_text, r.start);
            let b = from_utf16(new_text, r.end);
            a..b
        });
        self.editor
            .replace_range(target, new_text, select, Merge::Compose);
        if new_selected_range.is_none() {
            let end = start + new_text.len();
            self.editor.set_selection(Selection::caret(end));
        }
        self.marked = (!new_text.is_empty()).then(|| start..start + new_text.len());
        self.edited(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let at = self.ime_bytes(&range_utf16).start;
        let buf = self.editor.buffer();
        let c = self.laid.caret(buf, buf.line_of(at), at)?;
        let _ = element_bounds;
        let o = self.origin();
        Some(Bounds::new(
            point(o.x + c.at.x, o.y + c.at.y),
            size(px(2.), c.height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let buf = self.editor.buffer();
        let at = self.laid.hit(self.doc_point(point), buf);
        let (start, text) = self.ime_line();
        (at >= start && at <= start + text.len()).then(|| to_utf16(text, at - start))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn utf16_offsets_round_trip() {
        let t = "aé😀b";
        assert_eq!(to_utf16(t, t.len()), 5);
        let emoji = t.find('😀').unwrap();
        assert_eq!(to_utf16(t, emoji), 2);
        assert_eq!(from_utf16(t, 2), emoji);
        assert_eq!(from_utf16(t, 99), t.len());
    }
}
