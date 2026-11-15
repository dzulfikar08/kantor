//! Footnotes and endnotes in the Docs view, edited where they are (see
//! [`crate::part`]): Insert Footnote (Ctrl+Alt+F) and Insert Endnote
//! (Ctrl+Alt+D) put a numbered reference at the caret and the caret in the
//! new note; a click in a note at the bottom of a page (or at the end)
//! edits it; a double click on a note's number in the text goes to the
//! note, and on the note's own number back to the text, as in Word.

use docs_engine::{NoteKind, Position, Selection};
use gpui_kit::*;
use kantor_ui::i18n::t;

use crate::DocsView;

/// A note: its kind and id.
type NoteId = (NoteKind, String);
/// A point on a page: page index, x, y.
type PagePoint = (usize, f32, f32);

impl DocsView {
    /// Insert a footnote or endnote at the caret and start writing it.
    pub fn insert_note_doc(&mut self, kind: NoteKind, window: &mut Window, cx: &mut Context<Self>) {
        // Notes go in the body text (Word has none in headers or notes).
        self.leave_part(window, cx);
        let (sel, id) = self.doc.insert_note(kind, self.sel);
        self.edited(sel, window, cx);
        self.edit_note_in_place(kind, &id, None, window, cx);
    }

    /// Edit the note whose reference is at the caret (palette), or go back
    /// to the text from a note.
    pub fn edit_note_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.part_target(),
            Some(crate::part::PartTarget::Note { .. })
        ) {
            self.leave_note_to_reference(window, cx);
            cx.notify();
            return;
        }
        match self.doc.note_at(self.sel.head) {
            Some((kind, id)) => self.edit_note_in_place(kind, &id, None, window, cx),
            None => {
                self.status = t("Put the caret after a footnote or endnote number").into();
                cx.notify();
            }
        }
    }

    /// The note drawn at a window point (at the bottom of a page or at the
    /// end), with the point in page coordinates.
    pub(crate) fn note_area_at(&self, p: Point<Pixels>) -> Option<(NoteId, PagePoint)> {
        let g = self.geometry();
        let (page, x, y) = g.screen_to_page(p, self.laid.paged.pages.len());
        for placed in self.laid.furniture.get(page).into_iter().flatten() {
            let Some(note) = &placed.note else { continue };
            let [px, py, w, h] = placed.rect();
            if x >= px - 4.0 && x <= px + w + 4.0 && y >= py && y <= py + h {
                return Some((note.clone(), (page, x, y)));
            }
        }
        None
    }

    /// The note whose number in the body text is at a window point.
    pub(crate) fn note_ref_at(&self, p: Point<Pixels>) -> Option<(NoteKind, String)> {
        if self.part.is_some() {
            return None;
        }
        let g = self.geometry();
        let paged = &self.laid.paged;
        let (page, x, y) = g.screen_to_page(p, paged.pages.len());
        let (para, index) = paged.hit_test(page, x, y)?;
        let c = paged.caret(para, index)?;
        // Only right next to the number.
        ((c.x - x).abs() < 12.0 && c.page == page)
            .then(|| self.doc.note_at(Position::new(para, index)))
            .flatten()
    }

    /// Whether `pos` in the note being edited is at its own number.
    pub(crate) fn at_own_note_mark(&self, pos: Position) -> bool {
        let p = self.doc.paragraph(pos.para);
        let before = pos.offset.saturating_sub(docs_engine::OBJECT.len_utf8());
        [pos.offset, before]
            .into_iter()
            .filter_map(|o| p.object_at(o))
            .any(|o| {
                matches!(o, docs_engine::Object::Opaque(op)
                    if NoteKind::of_element(docs_engine::model::element_name(&op.xml)).is_some())
            })
    }

    /// Delete the note being edited, or the one whose number is at the
    /// caret, with its number in the text (palette).
    pub fn delete_note_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let note = match self.part_target() {
            Some(crate::part::PartTarget::Note { kind, id }) => Some((*kind, id.clone())),
            _ => None,
        };
        if note.is_some() {
            self.leave_part(window, cx);
        }
        let Some((kind, id)) = note.or_else(|| self.doc.note_at(self.sel.head)) else {
            self.status = t("Put the caret after a footnote or endnote number").into();
            cx.notify();
            return;
        };
        let sel = self.sel;
        if self.doc.delete_note(kind, &id, sel) {
            let sel = self.doc.clamp_selection(sel);
            self.edited(Selection::caret(sel.head), window, cx);
            self.status = match kind {
                NoteKind::Footnote => t("Footnote deleted"),
                NoteKind::Endnote => t("Endnote deleted"),
            }
            .into();
        }
        cx.notify();
    }
}
