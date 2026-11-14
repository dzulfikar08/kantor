//! Hooks for "Rewrite / Translate selection", which lives in the shell: the
//! selected paragraphs' text, and replacing them as one undo step.

use std::ops::Range;

use docs_engine::LINE_BREAK;
use gpui_kit::*;
use kantor_ui::i18n::t;

use crate::DocsView;

impl DocsView {
    /// The paragraphs the selection touches (the caret's when nothing is
    /// selected) and their text, line breaks as `\n`.
    pub fn rewrite_source(&self) -> Result<(Range<usize>, Vec<String>), String> {
        self.doc.rewrite_source(self.sel)
    }

    /// Replace paragraphs `range` with `texts`, if they still read
    /// `expected` (the user may have edited meanwhile). One undo step.
    pub fn replace_paragraphs_doc(
        &mut self,
        range: Range<usize>,
        expected: &[String],
        texts: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let unchanged = range.end <= self.doc.len()
            && self.doc.paragraphs()[range.clone()]
                .iter()
                .map(|p| p.text.replace(LINE_BREAK, "\n"))
                .eq(expected.iter().cloned());
        if !unchanged {
            return Err(t("The text changed since it was sent; run the rewrite again.").into());
        }
        let sel = self.doc.replace_paragraph_texts(range, texts)?;
        self.edited(sel, window, cx);
        self.status = t("Rewritten (Undo restores the original)").into();
        cx.notify();
        Ok(())
    }
}
