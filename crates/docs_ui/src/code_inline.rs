//! Field codes typed in the text, as with Word's Alt+F9: a field showing
//! its code (`{ DATE \@ "d MMMM yyyy" }`) takes the caret between its
//! braces. The arrows move through the code, typing, Backspace and Delete
//! edit it, and the field's result stays as it was until F9 updates it.
//!
//! The code isn't text of the paragraph (it lives in the field's begin
//! mark), so the caret in it is a small session on top of the document's:
//! the document caret sits at the begin mark, the session holds the code
//! being typed and the caret's byte in it, and each edit writes the code
//! back into the mark (one undo step, [`docs_engine::Document::set_field_instr`]).
//! Any other edit or caret move ends the session.

use docs_engine::{Position, Selection};
use docs_layout::CaretRect;
use gpui_kit::prelude::*;
use gpui_kit::*;

use crate::DocsView;

/// The caret in a field's code.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InlineCode {
    /// The field's begin mark.
    begin: Position,
    /// The code as typed (spaces at its end kept while typing).
    text: String,
    /// The caret's byte in `text`.
    at: usize,
    /// The document's revision the session belongs to.
    rev: u64,
}

/// The code's text as the label shows it: `{ code }`.
const OPEN: usize = 2;

fn prev_char(s: &str, at: usize) -> usize {
    s[..at].char_indices().next_back().map_or(0, |(i, _)| i)
}

fn next_char(s: &str, at: usize) -> usize {
    s[at..].chars().next().map_or(at, |c| at + c.len_utf8())
}

/// `text` with leading spaces dropped, `at` moved with it.
fn trim_start(text: String, at: usize) -> (String, usize) {
    let cut = text.len() - text.trim_start().len();
    (text[cut..].to_owned(), at.saturating_sub(cut))
}

impl DocsView {
    /// The session, while it's still the caret's: the document unchanged
    /// since and its field still showing the code.
    pub(crate) fn code_session(&self) -> Option<&InlineCode> {
        let s = self.code_inline.as_ref()?;
        if self.sel != Selection::caret(s.begin) || self.doc.revision() != s.rev {
            return None;
        }
        let f = self.field_starting(s.begin)?;
        self.codes().shows(f.mark_id()).then_some(s)
    }

    /// Put the caret at byte `at` of the code of the field at `begin`.
    pub(crate) fn start_code(
        &mut self,
        begin: Position,
        at: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.field_starting(begin) else {
            return;
        };
        let text = f.shown_code();
        let at = at.unwrap_or(text.len()).min(text.len());
        let at = (0..=at)
            .rev()
            .find(|&i| text.is_char_boundary(i))
            .unwrap_or(0);
        self.sel = Selection::caret(begin);
        self.goal_x = None;
        self.typing_props = None;
        self.marked = None;
        self.code_inline = Some(InlineCode {
            begin,
            text,
            at,
            rev: self.doc.revision(),
        });
        cx.notify();
    }

    /// The field whose begin mark is at `begin`.
    fn field_starting(&self, begin: Position) -> Option<docs_engine::FieldSpan> {
        self.doc.fields().into_iter().find(|f| f.begin == begin)
    }

    /// F9 with the caret in a code: the caret goes into the field so its
    /// result is the one updated.
    pub(crate) fn code_to_field(&mut self) {
        if let Some(s) = self.code_session() {
            let inside = Position::new(
                s.begin.para,
                s.begin.offset + docs_engine::OBJECT.len_utf8(),
            );
            self.sel = Selection::caret(inside);
            self.code_inline = None;
        }
    }

    /// The caret in a field's code, for painting.
    pub(crate) fn code_caret(&self) -> Option<CaretRect> {
        let s = self.code_session()?;
        let label = format!("{{ {} }}", s.text.trim());
        // Spaces typed at the end sit before the closing brace.
        let at = (OPEN + s.at).min(label.len() - 1);
        self.laid.label_caret(s.begin.para, s.begin.offset, at)
    }

    /// Left/Right in a code: move through it; past its ends the caret
    /// leaves the field (before it, or after its end). `true` when handled.
    pub(crate) fn code_move(&mut self, forward: bool, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.code_session().cloned() else {
            return false;
        };
        let at = if forward {
            (s.at < s.text.len()).then(|| next_char(&s.text, s.at))
        } else {
            (s.at > 0).then(|| prev_char(&s.text, s.at))
        };
        match at {
            Some(at) => {
                if let Some(c) = &mut self.code_inline {
                    c.at = at;
                }
                cx.notify();
            }
            None => {
                self.code_inline = None;
                let to = if forward {
                    match self.field_starting(s.begin) {
                        Some(f) => self.doc.next_position(f.end),
                        None => s.begin,
                    }
                } else {
                    s.begin
                };
                self.sel = Selection::caret(to);
                cx.notify();
            }
        }
        true
    }

    /// Home/End in a code: its start or end.
    pub(crate) fn code_edge(&mut self, end: bool, cx: &mut Context<Self>) -> bool {
        if self.code_session().is_none() {
            return false;
        }
        if let Some(c) = &mut self.code_inline {
            c.at = if end { c.text.len() } else { 0 };
        }
        cx.notify();
        true
    }

    /// A caret move from `from` to `to` (Left/Right) that lands inside a
    /// field showing its code goes into the code instead: at its start
    /// going right, its end going left.
    pub(crate) fn code_enter(
        &mut self,
        from: Position,
        to: Position,
        forward: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let codes = self.codes();
        let hit = self.doc.fields().into_iter().find(|f| {
            codes.shows(f.mark_id())
                && if forward {
                    from == f.begin && to > f.begin
                } else {
                    from > f.begin && to > f.begin && to <= f.end
                }
        });
        let Some(f) = hit else { return false };
        self.start_code(f.begin, if forward { Some(0) } else { None }, cx);
        true
    }

    /// A click on a code shown in the text puts the caret there.
    pub(crate) fn code_click(&mut self, p: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let codes = self.codes();
        if !codes.all && codes.ids.is_empty() {
            return false;
        }
        let g = self.geometry();
        let (page, x, y) = g.screen_to_page(p, self.laid.paged.pages.len());
        let hit = self.doc.fields().into_iter().find_map(|f| {
            if !codes.shows(f.mark_id()) {
                return None;
            }
            let b = self
                .laid
                .label_hit(f.begin.para, f.begin.offset, page, x, y)?;
            Some((f, b))
        });
        let Some((f, b)) = hit else { return false };
        let code_len = f.shown_code().len();
        self.start_code(f.begin, Some(b.saturating_sub(OPEN).min(code_len)), cx);
        self.dragging = false;
        true
    }

    /// Typing in a code.
    pub(crate) fn code_type(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(s) = self.code_session().cloned() else {
            return false;
        };
        let typed: String = text.chars().filter(|c| !c.is_control()).collect();
        if typed.is_empty() {
            return true;
        }
        let mut code = s.text.clone();
        code.insert_str(s.at, &typed);
        let at = s.at + typed.len();
        self.write_code(s, code, at, window, cx);
        true
    }

    /// Backspace (`forward == false`) or Delete in a code.
    pub(crate) fn code_delete(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(s) = self.code_session().cloned() else {
            return false;
        };
        let (a, b) = if forward {
            (s.at, next_char(&s.text, s.at))
        } else {
            (prev_char(&s.text, s.at), s.at)
        };
        if a == b {
            return true;
        }
        let mut code = s.text.clone();
        code.replace_range(a..b, "");
        self.write_code(s, code, a, window, cx);
        true
    }

    /// Write `code` into the field (one undo step) and go on typing at
    /// `at`. An empty code keeps the field as it was.
    fn write_code(
        &mut self,
        s: InlineCode,
        code: String,
        at: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (code, at) = trim_start(code, at);
        let old = self.field_starting(s.begin).map(|f| f.mark_id());
        if !code.trim().is_empty() {
            self.doc.set_field_instr(s.begin, &code, self.sel);
        }
        // The field's mark is new: keep it showing its code.
        let new = self.field_starting(s.begin).map(|f| f.mark_id());
        if let (Some(old), Some(new)) = (old, new)
            && old != new
        {
            let mut v = self.codes();
            if v.ids.remove(&old) {
                v.ids.insert(new);
            }
            match &mut self.part {
                Some(p) => p.main.set_field_codes(Some(v)),
                None => self.doc.set_field_codes(Some(v)),
            }
        }
        let shown = self
            .field_starting(s.begin)
            .map(|f| f.shown_code())
            .unwrap_or_default();
        // What the field shows now (nested braces may be rewritten).
        let (text, at) = if shown == code.trim() {
            (code, at)
        } else {
            let at = at.min(shown.len());
            let at = (0..=at)
                .rev()
                .find(|&i| shown.is_char_boundary(i))
                .unwrap_or(0);
            (shown, at)
        };
        let sel = Selection::caret(s.begin);
        self.edited(sel, window, cx);
        self.code_inline = Some(InlineCode {
            begin: s.begin,
            text,
            at,
            rev: self.doc.revision(),
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn caret_steps_over_whole_characters() {
        let s = "DATE \\@ \"d MMMM\" é";
        let end = s.len();
        let before = prev_char(s, end);
        assert_eq!(&s[before..], "é");
        assert_eq!(next_char(s, before), end);
        assert_eq!(next_char(s, end), end);
        assert_eq!(prev_char(s, 0), 0);
        assert_eq!(trim_start("  PAGE".into(), 3), ("PAGE".into(), 1));
    }
}
