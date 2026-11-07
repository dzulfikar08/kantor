//! Text input and IME composition through GPUI's [`EntityInputHandler`].
//!
//! As in Docs, the input method sees **the caret's paragraph** of the text
//! being edited; ranges are UTF-16 offsets into it. Typing while a text
//! shape is selected (not yet edited) starts editing at the end of its
//! text. Preedit updates and the commit are one undo step.

use std::ops::Range;

use gpui_kit::*;
use slides_engine::{Position, Selection};

use crate::SlidesView;
use crate::draw::text_origin;

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

impl SlidesView {
    fn head_text(&self) -> String {
        let Some(e) = &self.editing else {
            return String::new();
        };
        self.pres
            .text_paras((self.current, e.shape))
            .and_then(|p| p.get(e.sel.head.para))
            .map(|p| p.text.clone())
            .unwrap_or_default()
    }

    fn ime_target(&self, range: Option<Range<usize>>) -> Option<Selection> {
        let e = self.editing.as_ref()?;
        let text = self.head_text();
        let para = e.sel.head.para;
        let bytes = range
            .map(|r| from_utf16(&text, r.start)..from_utf16(&text, r.end))
            .or_else(|| e.marked.clone());
        Some(match bytes {
            Some(b) => Selection {
                anchor: Position::new(para, b.start),
                head: Position::new(para, b.end),
            },
            None => e.sel,
        })
    }

    /// Make sure a text shape is being edited (typing into a selected one).
    fn ensure_editing(&mut self, cx: &mut Context<Self>) -> bool {
        if self.editing.is_some() {
            return true;
        }
        match self.selected {
            Some(i) if self.presenting.is_none() => self.begin_editing(i, None, cx),
            _ => false,
        }
    }
}

impl EntityInputHandler for SlidesView {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        self.editing.as_ref()?;
        let text = self.head_text();
        let b = from_utf16(&text, range.start)..from_utf16(&text, range.end);
        *adjusted = Some(to_utf16(&text, b.start)..to_utf16(&text, b.end));
        Some(text[b].to_owned())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let e = self.editing.as_ref()?;
        let text = self.head_text();
        let head = e.sel.head;
        let anchor = if e.sel.anchor.para == head.para {
            e.sel.anchor.offset
        } else {
            head.offset
        };
        let (a, b) = (anchor.min(head.offset), anchor.max(head.offset));
        Some(UTF16Selection {
            range: to_utf16(&text, a)..to_utf16(&text, b),
            reversed: head.offset < anchor,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        let e = self.editing.as_ref()?;
        let m = e.marked.as_ref()?;
        let text = self.head_text();
        Some(to_utf16(&text, m.start)..to_utf16(&text, m.end))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(e) = &mut self.editing {
            e.marked = None;
        }
        self.pres.seal_history();
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.ensure_editing(cx) {
            return;
        }
        let Some(target) = self.ime_target(range) else {
            return;
        };
        let composing = self.editing.as_ref().is_some_and(|e| e.marked.is_some());
        let text = text.to_owned();
        self.text_edit(window, cx, move |p, r, e| {
            let props = e.typing_props.take();
            e.marked = None;
            let sel = if composing {
                p.compose(r, target, &text, props)
            } else {
                p.insert_text(r, target, &text, props)
            };
            if composing {
                p.seal_history();
            }
            sel
        });
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.ensure_editing(cx) {
            return;
        }
        let Some(target) = self.ime_target(range) else {
            return;
        };
        let start = target.ordered().0;
        let text = new_text.to_owned();
        self.text_edit(window, cx, move |p, r, e| {
            let after = p.compose(r, target, &text, e.typing_props.clone())?;
            let caret = match new_selected_range {
                Some(sr) => start.offset + from_utf16(&text, sr.end),
                None => after.head.offset,
            };
            e.marked = (!text.is_empty()).then(|| start.offset..start.offset + text.len());
            Ok(Selection::caret(Position::new(start.para, caret)))
        });
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let e = self.editing.clone()?;
        let text = self.head_text();
        let start = from_utf16(&text, range_utf16.start);
        let (xfrm, laid) = self.laid(e.shape)?;
        let c = laid.layout.paged.caret(e.sel.head.para, start)?;
        let v = self.view();
        let o = text_origin(&laid, &xfrm, v);
        let s = v.px_scale();
        Some(Bounds::new(
            point(o.x + px(c.x * s), o.y + px(c.top * s)),
            size(px(2.), px(c.height * s)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let e = self.editing.clone()?;
        let pos = self.text_hit(e.shape, point)?;
        (pos.para == e.sel.head.para).then(|| to_utf16(&self.head_text(), pos.offset))
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
        assert_eq!(from_utf16(t, 2), emoji);
        assert_eq!(from_utf16(t, 99), t.len());
    }
}
