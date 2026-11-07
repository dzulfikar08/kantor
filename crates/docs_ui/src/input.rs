//! Text input and IME composition through GPUI's [`EntityInputHandler`].
//!
//! The input method sees **the caret's paragraph** as "the text": ranges
//! are UTF-16 offsets into that paragraph. That is what IMEs need (they
//! work around the caret), and it keeps the offsets small and stable. A
//! selection spanning paragraphs is reported as the caret only; typing
//! still replaces the whole selection.
//!
//! Composition (preedit) text is inserted into the document like typed
//! text and underlined; every preedit update and the commit merge into one
//! undo step ([`docs_engine::Document::compose`]).
//!
//! Untested here: fcitx5 on Wayland in a real window (plan §0 item 1),
//! candidate-window placement (`bounds_for_range`), and IMEs that reconvert
//! already committed text (`text_for_range` + replacement ranges).

use std::ops::Range;

use docs_engine::{Position, Selection};
use gpui_kit::*;

use crate::DocsView;

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

impl DocsView {
    fn head_text(&self) -> &str {
        &self.doc.paragraph(self.sel.head.para).text
    }

    fn bytes(&self, r: &Range<usize>) -> Range<usize> {
        let text = self.head_text();
        from_utf16(text, r.start)..from_utf16(text, r.end)
    }

    fn utf16(&self, r: &Range<usize>) -> Range<usize> {
        let text = self.head_text();
        to_utf16(text, r.start)..to_utf16(text, r.end)
    }

    /// What an input-method edit replaces: the given range (in the caret's
    /// paragraph), else the composition, else the selection.
    fn ime_target(&self, range: Option<Range<usize>>) -> Selection {
        let para = self.sel.head.para;
        let bytes = range
            .map(|r| self.bytes(&r))
            .or_else(|| self.marked.clone());
        match bytes {
            Some(b) => Selection {
                anchor: Position::new(para, b.start),
                head: Position::new(para, b.end),
            },
            None => self.sel,
        }
    }
}

impl EntityInputHandler for DocsView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let bytes = self.bytes(&range);
        *adjusted = Some(self.utf16(&bytes));
        Some(self.head_text()[bytes].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let head = self.sel.head;
        let anchor = if self.sel.anchor.para == head.para {
            self.sel.anchor.offset
        } else {
            head.offset
        };
        let (a, b) = (anchor.min(head.offset), anchor.max(head.offset));
        Some(UTF16Selection {
            range: self.utf16(&(a..b)),
            reversed: head.offset < anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|m| self.utf16(m))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked = None;
        self.doc.end_composition();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.ime_target(range);
        let props = self.typing_props.take();
        let sel = if self.marked.take().is_some() {
            let sel = self.doc.compose(target, text, props);
            self.doc.end_composition();
            sel
        } else {
            self.doc.insert_text(target, text, props)
        };
        self.edited(sel, window, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let target = self.ime_target(range);
        let start = self.doc.clamp(target.ordered().0);
        let after = self
            .doc
            .compose(target, new_text, self.typing_props.clone());
        let caret = match new_selected_range {
            Some(r) => start.offset + from_utf16(new_text, r.end),
            None => after.head.offset,
        };
        self.edited(
            Selection::caret(Position::new(start.para, caret)),
            window,
            cx,
        );
        self.marked = (!new_text.is_empty()).then(|| start.offset..start.offset + new_text.len());
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let para = self.sel.head.para;
        let start = self.bytes(&range_utf16).start;
        let c = self.laid.paged.caret(para, start)?;
        let mut g = self.geometry();
        g.origin = element_bounds.origin;
        let top = g.page_to_screen(c.page, c.x, c.top);
        Some(Bounds::new(top, size(px(2.), px(c.height * g.zoom))))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let pos = self.hit(point)?;
        (pos.para == self.sel.head.para).then(|| to_utf16(self.head_text(), pos.offset))
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
        assert_eq!(from_utf16(t, 4), emoji + 4);
        assert_eq!(from_utf16(t, 99), t.len());
    }
}
