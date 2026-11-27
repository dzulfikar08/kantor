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
//! Text boxes (a shape's own, or one of a group's shapes) are edited the
//! same way ([`Document::edit_text_box`]): their content is a small
//! document too ([`crate::float::TextBox`]), and leaving makes the whole
//! edit one undo step that rewrites the box's `w:txbxContent` (and the
//! VML fallback's) on save.
//!
//! A header or footer that doesn't exist yet is created when editing
//! starts (the section gets its reference) and dropped again if nothing
//! was typed in it, so just looking leaves the file as it was.

use std::sync::Arc;

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
    /// A floating object's text box.
    TextBox(BoxRef),
}

/// A text box: its object's place and which of its text boxes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BoxRef {
    /// The header or footer story the object is anchored in (`None`: the
    /// body).
    pub story: Option<usize>,
    /// The object's paragraph and byte index.
    pub para: usize,
    pub index: usize,
    /// A group's shape number ([`crate::Float::boxes`]); `None` for the
    /// shape's own text box.
    pub which: Option<usize>,
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
    /// A text box's anchor paragraph as it was before the edit.
    owner: Option<Paragraph>,
    /// A "Resize shape to fit text" box's height for its new text (EMU),
    /// worked out by the layout: [`Document::finish_part`] sizes the
    /// shape to it, as Word does when the text changes.
    pub fit_height: Option<i64>,
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
        self.take_story(index, before, para, created)
    }

    /// Start editing header or footer story `index` (see
    /// [`Document::stories`]) as it is, for a program going through every
    /// header and footer (Replace All). `None` for one Kantor won't edit.
    pub fn edit_story(&mut self, index: usize) -> Option<(PartEdit, Document)> {
        if self.stories.get(index)?.locked.is_some() {
            return None;
        }
        let before = self.side_state();
        self.take_story(index, before, None, false)
    }

    fn take_story(
        &mut self,
        index: usize,
        before: HfState,
        para: Option<(usize, Paragraph)>,
        created: bool,
    ) -> Option<(PartEdit, Document)> {
        let props = std::sync::Arc::new(crate::fields::Props::of(self));
        let mut doc = std::mem::take(&mut self.stories[index].doc);
        doc.media_stem = media_stem(&self.stories[index].part);
        doc.field_props = Some(props);
        doc.numbering = self.numbering.clone();
        // Edits in it are tracked as the body's are.
        doc.track = self.track;
        doc.author = self.author.clone();
        Some((
            PartEdit {
                part: PartRef::Story(index),
                before,
                para,
                created,
                owner: None,
                fit_height: None,
            },
            doc,
        ))
    }

    /// Every text box Kantor can edit in place: the body's (`story: None`)
    /// and those of the headers and footers it edits, a group's shapes'
    /// included, in document order.
    pub fn text_boxes(&self) -> Vec<BoxRef> {
        let mut out = Vec::new();
        let owners = std::iter::once((None, self)).chain(
            self.stories
                .iter()
                .enumerate()
                .filter(|(_, st)| st.locked.is_none())
                .map(|(i, st)| (Some(i), &st.doc)),
        );
        for (story, doc) in owners {
            for (para, p) in doc.paragraphs.iter().enumerate() {
                for (r, run) in p.run_ranges() {
                    let Some(crate::Object::Float(f)) = &run.object else {
                        continue;
                    };
                    let at = |which| BoxRef {
                        story,
                        para,
                        index: r.start,
                        which,
                    };
                    if f.text.is_some() {
                        out.push(at(None));
                    }
                    out.extend((0..f.boxes.len()).map(|k| at(Some(k))));
                }
            }
        }
        out
    }

    /// Start editing a footnote or endnote.
    pub fn edit_note(&mut self, kind: NoteKind, id: &str) -> Option<(PartEdit, Document)> {
        let index = self.note_index(kind, id)?;
        let before = self.side_state();
        let props = std::sync::Arc::new(crate::fields::Props::of(self));
        let mut doc = std::mem::take(&mut self.notes[index].doc);
        doc.field_props = Some(props);
        doc.media_stem = format!("{}{}_", kind.element(), id.trim_start_matches('-'));
        doc.numbering = self.numbering.clone();
        doc.track = self.track;
        doc.author = self.author.clone();
        Some((
            PartEdit {
                part: PartRef::Note(index),
                before,
                para: None,
                created: false,
                owner: None,
                fit_height: None,
            },
            doc,
        ))
    }

    /// Start editing a text box in place, pictures and tables in it
    /// included. `None` when there's no such text box.
    pub fn edit_text_box(&mut self, b: BoxRef) -> Option<(PartEdit, Document)> {
        let owner = self.box_owner(b.story)?;
        let f = owner.float_at(b.para, b.index)?;
        let tb = f.text_box_in(b.which)?;
        // New pictures and tables fit the box's text area, as in Word.
        let text_width = {
            let (w, _) = f.at.size_px();
            let w_tw = (w * 15.0).round() as i64;
            let ins = (tb.insets[0] + tb.insets[2]) / 635;
            (w_tw - ins).max(720) as u32
        };
        let media = owner.media.clone();
        let para = owner.paragraphs.get(b.para)?.clone();
        let floor = self
            .stories
            .iter()
            .map(|s| s.doc.max_doc_pr())
            .fold(self.max_doc_pr(), u32::max);
        let stem = match b.story {
            Some(s) => self.stories.get(s).map(|st| media_stem(&st.part))?,
            None => String::new(),
        };
        let before = self.side_state();
        let props = Arc::new(crate::fields::Props::of(self));
        let numbering = self.numbering.clone();
        let (track, author) = (self.track, self.author.clone());
        let mut doc = std::mem::take(self.box_doc_mut(b)?);
        doc.clear_history();
        doc.field_props = Some(props);
        doc.numbering = numbering;
        doc.media_stem = stem;
        doc.doc_pr_floor = floor;
        for (id, m) in media {
            doc.media.entry(id).or_insert(m);
        }
        doc.page = crate::PageGeometry {
            width: text_width,
            margin_left: 0,
            margin_right: 0,
            ..doc.page
        };
        doc.track = track;
        doc.author = author;
        Some((
            PartEdit {
                part: PartRef::TextBox(b),
                before,
                para: None,
                created: false,
                owner: Some(para),
                fit_height: None,
            },
            doc,
        ))
    }

    /// The document a text box's object is anchored in.
    fn box_owner(&self, story: Option<usize>) -> Option<&Document> {
        match story {
            Some(s) => self.stories.get(s).map(|st| &st.doc),
            None => Some(self),
        }
    }

    /// The text box's own document, in place (the object and its text box
    /// copied first if anything else shares them, as the undo history).
    fn box_doc_mut(&mut self, b: BoxRef) -> Option<&mut Document> {
        let owner = match b.story {
            Some(s) => &mut self.stories.get_mut(s)?.doc,
            None => self,
        };
        box_in_para(owner.paragraphs.get_mut(b.para)?, b.index, b.which)
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
            PartRef::TextBox(b) => {
                if let Some(d) = self.box_doc_mut(b) {
                    std::mem::swap(d, doc);
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
            PartRef::TextBox(b) => e.owner.as_ref().and_then(|p| old_box(p, b)),
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
        doc.track = false;
        // Lists started in the part: their definitions are the main
        // document's (numbering.xml).
        if changed && doc.numbering.nums.len() > self.numbering.nums.len() {
            self.numbering = doc.numbering.clone();
        }
        if let PartRef::TextBox(b) = e.part {
            return self.finish_text_box(e, b, doc, changed, sel);
        }
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
            PartRef::TextBox(_) => unreachable!("finished above"),
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

    /// [`Document::finish_part`] for a text box: the anchor paragraph
    /// gets the edited box (one undo step), or is put back as it was.
    fn finish_text_box(
        &mut self,
        e: PartEdit,
        b: BoxRef,
        mut doc: Document,
        changed: bool,
        sel: Selection,
    ) -> bool {
        let Some(old) = e.owner else {
            return false;
        };
        let pictures = std::mem::take(&mut doc.new_media);
        doc.doc_pr_floor = 0;
        if let Some(was) = old_box(&old, b) {
            doc.page = was.page;
        }
        let mut para = old.clone();
        if changed && let Some(d) = box_in_para(&mut para, b.index, b.which) {
            *d = doc;
            mark_edited(&mut para, b.index, b.which);
            if let Some(cy) = e.fit_height {
                fit_shape(&mut para, b.index, b.which, cy);
            }
        }
        let Some(owner) = (match b.story {
            Some(s) => self.stories.get_mut(s).map(|st| &mut st.doc),
            None => Some(&mut *self),
        }) else {
            return false;
        };
        let Some(slot) = owner.paragraphs.get_mut(b.para) else {
            return false;
        };
        if !changed {
            *slot = old;
            return false;
        }
        *slot = para;
        for m in &pictures {
            owner.media.insert(
                m.rel_id.clone(),
                crate::docx::Media {
                    part: m.part.clone(),
                    bytes: Some(m.bytes.clone()),
                },
            );
        }
        owner.new_media.extend(pictures);
        match b.story {
            Some(s) => {
                self.stories[s].modified = true;
                self.push_hf(e.before, None, sel);
            }
            None => self.push_hf(e.before, Some((b.para, old)), sel),
        }
        true
    }

    /// Tracked changes in headers, footers, footnotes and endnotes (each
    /// insertion, deletion or formatting change counted once).
    pub fn part_changes(&self) -> usize {
        self.part_docs()
            .map(|d| d.changes().len() + d.other_changes())
            .sum()
    }

    /// The documents of the headers, footers and notes Kantor edits.
    fn part_docs(&self) -> impl Iterator<Item = &Document> {
        self.stories
            .iter()
            .filter(|s| s.locked.is_none())
            .map(|s| &s.doc)
            .chain(self.notes.iter().filter(|n| !n.separator).map(|n| &n.doc))
    }

    /// Accept or reject every tracked change in the headers, footers and
    /// notes (part of Accept All / Reject All). Returns whether any
    /// changed; Save then writes those parts.
    pub(crate) fn resolve_parts(&mut self, accept: bool) -> bool {
        let mut changed = false;
        for i in 0..self.stories.len() {
            if self.stories[i].locked.is_some() || !self.stories[i].doc.has_changes() {
                continue;
            }
            let mut d = std::mem::take(&mut self.stories[i].doc);
            d.resolve_all(accept, Selection::default());
            d.clear_history();
            self.stories[i].doc = d;
            self.stories[i].modified = true;
            changed = true;
        }
        for i in 0..self.notes.len() {
            if self.notes[i].separator || !self.notes[i].doc.has_changes() {
                continue;
            }
            let mut d = std::mem::take(&mut self.notes[i].doc);
            d.resolve_all(accept, Selection::default());
            d.clear_history();
            self.notes[i].doc = d;
            self.notes[i].xml = None;
            changed = true;
        }
        changed
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

/// The document of the text box `which` of the object at byte `index` of
/// `p`, in place.
fn box_in_para(p: &mut Paragraph, index: usize, which: Option<usize>) -> Option<&mut Document> {
    let mut start = 0;
    for run in &mut p.runs {
        if start == index
            && let Some(crate::Object::Float(f)) = &mut run.object
        {
            let f = Arc::make_mut(f);
            let tb = match which {
                None => f.text.as_mut()?,
                Some(k) => &mut f.boxes.get_mut(k)?.text,
            };
            return Some(&mut Arc::make_mut(tb).doc);
        }
        start += run.len;
    }
    None
}

/// Mark the text box `which` of the object at `index` of `p` as changed
/// in Kantor (its `w:txbxContent` is written from it).
fn mark_edited(p: &mut Paragraph, index: usize, which: Option<usize>) {
    let mut start = 0;
    for run in &mut p.runs {
        if start == index
            && let Some(crate::Object::Float(f)) = &mut run.object
        {
            let f = Arc::make_mut(f);
            match which {
                None => f.text_edited = true,
                Some(k) => {
                    if let Some(g) = f.boxes.get_mut(k) {
                        g.edited = true;
                    }
                }
            }
            return;
        }
        start += run.len;
    }
}

/// Size a "Resize shape to fit text" shape at `index` of `p` to the
/// height `cy` (EMU) its text needs.
fn fit_shape(p: &mut Paragraph, index: usize, which: Option<usize>, cy: i64) -> bool {
    let mut start = 0;
    for run in &mut p.runs {
        if start == index
            && let Some(crate::Object::Float(f)) = &mut run.object
        {
            if which.is_none()
                && f.text.as_ref().is_some_and(|t| t.auto_fit)
                && cy > 0
                && (f.at.cy - cy).abs() > crate::object::EMU_PER_PX as i64 / 2
            {
                Arc::make_mut(f).at.cy = cy;
                return true;
            }
            return false;
        }
        start += run.len;
    }
    false
}

impl Document {
    /// While "Resize shape to fit text" box `b` is edited: give its shape
    /// height `cy` (EMU) in this (the main) document, so the text around
    /// it wraps around its new size as it grows. Not an edit of its own:
    /// leaving the box saves the size ([`PartEdit::fit_height`]) or puts
    /// the paragraph back. Returns whether the size changed.
    pub fn fit_text_box_live(&mut self, b: BoxRef, cy: i64) -> bool {
        let owner = match b.story {
            Some(s) => self.stories.get_mut(s).map(|st| &mut st.doc),
            None => Some(&mut *self),
        };
        let Some(p) = owner.and_then(|o| o.paragraphs.get_mut(b.para)) else {
            return false;
        };
        fit_shape(p, b.index, b.which, cy)
    }
}

/// The text box `b` in the anchor paragraph `p` as it was.
fn old_box(p: &Paragraph, b: BoxRef) -> Option<&Document> {
    let f = p.object_at(b.index)?.float()?;
    f.text_box_in(b.which).map(|t| &t.doc)
}

/// The stem new pictures in a part are named with (`header1_`), so they
/// never clash with the body's.
fn media_stem(part: &str) -> String {
    let name = part.rsplit('/').next().unwrap_or(part);
    let stem = name.split('.').next().unwrap_or(name);
    format!("{stem}_")
}
