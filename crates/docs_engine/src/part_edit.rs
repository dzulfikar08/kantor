//! Headers, footers, footnotes and endnotes edited in place, with the same
//! editor as the body (Word's header/footer and note editing).
//!
//! Each of them is a small [`Document`] of its own (see [`crate::section`]
//! and [`crate::notes`]). Editing one takes that document out of the main
//! one ([`Document::edit_hf`], [`Document::edit_note`]): the editor then
//! works on it like on any document — runs, styles, alignment, tabs,
//! fields, pictures and tables, with its own undo while it's open — and
//! [`Document::swap_part`] puts it back for a moment whenever the page
//! layout needs the whole document. [`Document::finish_part`] puts it back
//! for good: everything typed there becomes one undo step of the main
//! document, and Save writes that header, footer or notes part (and only
//! that one).
//!
//! A header or footer that doesn't exist yet is created when editing
//! starts (the section gets its reference) and dropped again if nothing
//! was typed in it, so just looking leaves the file as it was.

use crate::document::Document;
use crate::model::{Paragraph, Selection};
use crate::notes::NoteKind;
use crate::section::{HfKind, HfState, HfType, insert_reference_el};

/// Which part is being edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PartRef {
    /// Index into [`Document::stories`].
    Story(usize),
    /// Index into [`Document::notes`].
    Note(usize),
}

/// An edit of a header, footer or note in progress (see the module docs).
#[derive(Clone, Debug)]
pub struct PartEdit {
    pub part: PartRef,
    before: HfState,
    /// The paragraph holding the section's sectPr before a reference was
    /// added to it (a header or footer created for this edit).
    para: Option<(usize, Paragraph)>,
    /// Created for this edit: dropped again if it's left empty.
    created: bool,
}

impl PartEdit {
    /// Whether the header or footer was created for this edit.
    pub fn created(&self) -> bool {
        self.created
    }
}

impl Document {
    /// Start editing the header or footer section `si` uses for `ty`
    /// (following Link to Previous, as Word does: editing a linked header
    /// edits the one it's linked to). A section with none gets a new,
    /// empty one. Returns the edit and the part's document to edit;
    /// `None` when the part can't be read (it's kept as it is).
    pub fn edit_hf(&mut self, si: usize, kind: HfKind, ty: HfType) -> Option<(PartEdit, Document)> {
        let sections = self.sections();
        let si = si.min(sections.len().saturating_sub(1));
        let before = self.side_state();
        let mut para = None;
        let mut created = false;
        let index = match self.story_index_for(&sections, si, kind, ty) {
            Some(i) => i,
            None => {
                let (i, p) = self.add_story(&sections, si, kind, ty);
                para = p;
                created = true;
                i
            }
        };
        if self.stories[index].locked.as_deref().is_some_and(|l| {
            // Unreadable parts stay as they are.
            l.starts_with("Kantor can't read")
        }) {
            return None;
        }
        let props = std::sync::Arc::new(crate::fields::Props::of(self));
        let mut doc = std::mem::take(&mut self.stories[index].doc);
        doc.media_stem = media_stem(&self.stories[index].part);
        doc.field_props = Some(props);
        Some((
            PartEdit {
                part: PartRef::Story(index),
                before,
                para,
                created,
            },
            doc,
        ))
    }

    /// Start editing a footnote or endnote.
    pub fn edit_note(&mut self, kind: NoteKind, id: &str) -> Option<(PartEdit, Document)> {
        let index = self.note_index(kind, id)?;
        let before = self.side_state();
        let props = std::sync::Arc::new(crate::fields::Props::of(self));
        let mut doc = std::mem::take(&mut self.notes[index].doc);
        doc.field_props = Some(props);
        doc.media_stem = format!("{}{}_", kind.element(), id.trim_start_matches('-'));
        Some((
            PartEdit {
                part: PartRef::Note(index),
                before,
                para: None,
                created: false,
            },
            doc,
        ))
    }

    /// Exchange the edited part's document with `doc`: call it twice
    /// around anything that needs the whole document (layout, export).
    pub fn swap_part(&mut self, e: &PartEdit, doc: &mut Document) {
        match e.part {
            PartRef::Story(i) => {
                if let Some(st) = self.stories.get_mut(i) {
                    std::mem::swap(&mut st.doc, doc);
                }
            }
            PartRef::Note(i) => {
                if let Some(n) = self.notes.get_mut(i) {
                    std::mem::swap(&mut n.doc, doc);
                }
            }
        }
    }

    /// Whether `doc` (the part being edited) differs from what it was when
    /// editing started.
    pub fn part_changed(&self, e: &PartEdit, doc: &Document) -> bool {
        let old = match e.part {
            PartRef::Story(i) => e.before.stories.get(i).map(|s| &s.doc),
            PartRef::Note(i) => e.before.notes.get(i).map(|n| &n.doc),
        };
        old.is_none_or(|o| o.paragraphs != doc.paragraphs || o.tables != doc.tables)
    }

    /// Put the edited part back and end the edit. Anything changed becomes
    /// one undo step (with the selection `sel` in the body to come back
    /// to) and Save writes the part; a header or footer created for the
    /// edit and left empty is removed again. Returns whether it changed.
    pub fn finish_part(&mut self, e: PartEdit, mut doc: Document, sel: Selection) -> bool {
        let changed = self.part_changed(&e, &doc);
        doc.clear_history();
        doc.media_stem.clear();
        doc.field_props = None;
        let empty = doc.paragraphs.iter().all(|p| p.text.is_empty()) && doc.tables.is_empty();
        match e.part {
            PartRef::Story(i) => {
                if let Some(st) = self.stories.get_mut(i) {
                    st.doc = doc;
                    if changed {
                        st.modified = true;
                    }
                }
            }
            PartRef::Note(i) => {
                if let Some(n) = self.notes.get_mut(i) {
                    n.doc = doc;
                    if changed {
                        n.xml = None;
                    }
                }
            }
        }
        if e.created && (!changed || empty) {
            // Nothing written in a new header: as if it was never made.
            self.set_hf_state(e.before);
            if let Some((i, old)) = e.para {
                self.paragraphs[i] = old;
            }
            return false;
        }
        if !changed && !e.created {
            return false;
        }
        self.push_hf(e.before, e.para, sel);
        true
    }

    /// A new, empty header or footer for section `si`'s `ty` pages, with
    /// its reference in the section. Returns its index and the paragraph
    /// holding the section's old sectPr (if the sectPr is a paragraph's).
    fn add_story(
        &mut self,
        sections: &[crate::section::Section],
        si: usize,
        kind: HfKind,
        ty: HfType,
    ) -> (usize, Option<(usize, Paragraph)>) {
        let story = self.new_story(kind);
        let id = story.rel_id.clone();
        self.stories.push(story);
        let xml = self.sect_xml(sections, si);
        let xml = insert_reference_el(self, &xml, kind, ty, &id);
        let para = self.set_sect_xml(sections, si, xml);
        (self.stories.len() - 1, para)
    }
}

/// The stem new pictures in a part are named with (`header1_`), so they
/// never clash with the body's.
fn media_stem(part: &str) -> String {
    let name = part.rsplit('/').next().unwrap_or(part);
    let stem = name.split('.').next().unwrap_or(name);
    format!("{stem}_")
}
