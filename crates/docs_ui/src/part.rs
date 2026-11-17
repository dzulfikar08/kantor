//! Headers, footers, footnotes and endnotes edited on the page, as in
//! Word.
//!
//! Double-clicking a page's header or footer area starts editing the
//! header or footer that page shows (the body text dims); double-clicking
//! the body, Esc or Close Header and Footer goes back. A click in a
//! footnote or endnote, or a double click on a note's number in the text,
//! edits the note; a double click on the note's own number goes back to
//! its reference. Insert Footnote / Endnote put the caret in the new note.
//!
//! While a part is edited, the view's [`Document`] *is* that part (taken
//! out of the main document with [`Document::edit_hf`] /
//! [`Document::edit_note`]), so every editing command works there as in
//! the body: formatting, styles, tabs, fields, pictures, tables, its own
//! undo. Layout puts it back into the main document for a moment
//! ([`Document::swap_part`]) and places the caret, the selection and
//! clicks on where the part is drawn ([`Piece`]). Leaving makes it one
//! undo step of the main document.

use std::sync::Arc;

use docs_engine::{
    BoxRef, Document, HfKind, HfType, NoteKind, PartEdit, PartRef, Position, Selection,
};
use docs_layout::CaretRect;
use gpui_kit::*;

use crate::{DocsView, Laid};

/// What is being edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PartTarget {
    /// A section's header or footer for some pages (the one the page
    /// shows: following Link to Previous, it may be an earlier section's).
    Hf {
        section: usize,
        kind: HfKind,
        ty: HfType,
    },
    Note {
        kind: NoteKind,
        id: String,
    },
    /// A text box (or a group's shape's text), edited where it's drawn.
    TextBox(BoxRef),
}

/// A header, footer or note being edited.
pub(crate) struct PartSession {
    pub edit: PartEdit,
    /// The main document (the part's own is the view's while editing).
    pub main: Document,
    /// The body's selection to come back to.
    pub body_sel: Selection,
    pub target: PartTarget,
    /// The page it is edited on (a header shows on many).
    pub page: usize,
}

/// Where the part being edited is drawn: its layout's (0, 0) at (`x`,
/// `y`) on `page`. A long note has a piece on each page it runs over.
#[derive(Clone)]
pub(crate) struct Piece {
    pub page: usize,
    pub x: f32,
    pub y: f32,
    pub layout: Arc<docs_pdf::StoryLayout>,
}

impl Piece {
    fn paged(&self) -> &docs_layout::PagedDocument {
        &self.layout.paged
    }

    /// Whether a caret rectangle of the piece's layout is in the shown
    /// stretch.
    fn shows(&self, c: &CaretRect) -> bool {
        c.top >= -0.5 && c.top + c.height <= self.layout.height + 0.5
    }

    fn to_page(&self, c: CaretRect) -> CaretRect {
        CaretRect {
            page: self.page,
            x: c.x + self.x,
            top: c.top + self.y,
            height: c.height,
        }
    }
}

/// The text the caret moves in: the body's pages, or the pieces of the
/// part being edited. Same calls as [`docs_layout::PagedDocument`].
impl Laid {
    pub(crate) fn editing_part(&self) -> bool {
        self.part.is_some()
    }

    pub(crate) fn caret(&self, para: usize, index: usize) -> Option<CaretRect> {
        let Some(pieces) = &self.part else {
            return self.paged.caret(para, index);
        };
        let mut fallback = None;
        for p in pieces {
            if let Some(c) = p.paged().caret(para, index) {
                if p.shows(&c) {
                    return Some(p.to_page(c));
                }
                fallback.get_or_insert(p.to_page(c));
            }
        }
        fallback
    }

    /// [`docs_layout::PagedDocument::label_caret`] where it's drawn.
    pub(crate) fn label_caret(&self, para: usize, index: usize, at: usize) -> Option<CaretRect> {
        let Some(pieces) = &self.part else {
            return self.paged.label_caret(para, index, at);
        };
        pieces.iter().find_map(|p| {
            let c = p.paged().label_caret(para, index, at)?;
            p.shows(&c).then(|| p.to_page(c))
        })
    }

    /// [`docs_layout::PagedDocument::label_hit`] where it's drawn.
    pub(crate) fn label_hit(
        &self,
        para: usize,
        index: usize,
        page: usize,
        x: f32,
        y: f32,
    ) -> Option<usize> {
        let Some(pieces) = &self.part else {
            return self.paged.label_hit(para, index, page, x, y);
        };
        pieces
            .iter()
            .filter(|p| p.page == page)
            .find_map(|p| p.paged().label_hit(para, index, 0, x - p.x, y - p.y))
    }

    pub(crate) fn hit_test(&self, page: usize, x: f32, y: f32) -> Option<(usize, usize)> {
        let Some(pieces) = &self.part else {
            return self.paged.hit_test(page, x, y);
        };
        // The piece on that page nearest to the point.
        let dist = |p: &Piece| {
            let (top, bottom) = (p.y, p.y + p.layout.height);
            if y < top {
                top - y
            } else if y > bottom {
                y - bottom
            } else {
                0.0
            }
        };
        let p = pieces
            .iter()
            .filter(|p| p.page == page)
            .min_by(|a, b| dist(a).total_cmp(&dist(b)))
            .or_else(|| pieces.first())?;
        let ly = (y - p.y).clamp(0.0, (p.layout.height - 0.5).max(0.0));
        p.paged().hit_test(0, x - p.x, ly)
    }

    pub(crate) fn vertical(
        &self,
        para: usize,
        index: usize,
        x: f32,
        down: bool,
    ) -> Option<(usize, usize)> {
        let Some(pieces) = &self.part else {
            return self.paged.vertical(para, index, x, down);
        };
        let p = pieces
            .iter()
            .find(|p| p.paged().caret(para, index).is_some_and(|c| p.shows(&c)))?;
        p.paged().vertical(para, index, x - p.x, down)
    }

    pub(crate) fn line_range(
        &self,
        para: usize,
        index: usize,
    ) -> Option<(std::ops::Range<usize>, bool)> {
        match &self.part {
            Some(pieces) => pieces.first()?.paged().line_range(para, index),
            None => self.paged.line_range(para, index),
        }
    }

    pub(crate) fn selection_rects(
        &self,
        start: (usize, usize),
        end: (usize, usize),
    ) -> Vec<(usize, [f32; 4])> {
        let Some(pieces) = &self.part else {
            return self.paged.selection_rects(start, end);
        };
        let mut out = Vec::new();
        for p in pieces {
            for (_, [x, y, w, h]) in p.paged().selection_rects(start, end) {
                if y >= -0.5 && y + h <= p.layout.height + 0.5 {
                    out.push((p.page, [x + p.x, y + p.y, w, h]));
                }
            }
        }
        out
    }

    /// Object boxes of the text being edited on `page`.
    pub(crate) fn objects(&self, page: usize) -> Vec<docs_layout::PlacedObject> {
        let Some(pieces) = &self.part else {
            return self.paged.objects(page);
        };
        let mut out = Vec::new();
        for p in pieces.iter().filter(|p| p.page == page) {
            for mut o in p.paged().objects(0) {
                o.rect[0] += p.x;
                o.rect[1] += p.y;
                out.push(o);
            }
        }
        out
    }

    /// Whether `placed` is (a piece of) the part being edited.
    pub(crate) fn is_part(&self, placed: &docs_pdf::Placed) -> bool {
        self.part.as_ref().is_some_and(|pieces| {
            pieces
                .iter()
                .any(|p| Arc::ptr_eq(&p.layout, &placed.layout))
        })
    }
}

/// The pieces of the part `edit` edits in a laid-out document (on
/// `page`, for headers and footers: the page it's edited on, or the first
/// that shows it).
pub(crate) fn pieces(
    furniture: &[Vec<docs_pdf::Placed>],
    floats: &[Vec<docs_pdf::floats::FloatItem>],
    part: PartRef,
    page: usize,
) -> Vec<Piece> {
    if let PartRef::TextBox(b) = part {
        return box_piece(floats, b, page).into_iter().collect();
    }
    let is = |pl: &docs_pdf::Placed| {
        pl.layout.text_box.is_none()
            && match part {
                PartRef::Story(i) => {
                    pl.note.is_none() && pl.layout.note.is_none() && pl.layout.story == i
                }
                PartRef::Note(i) => pl.layout.note == Some(i) && pl.note.is_some(),
                PartRef::TextBox(_) => false,
            }
    };
    let on = |pg: usize| -> Vec<Piece> {
        furniture
            .get(pg)
            .into_iter()
            .flatten()
            .filter(|pl| is(pl))
            .map(|pl| Piece {
                page: pg,
                x: pl.x,
                y: pl.y,
                layout: pl.layout.clone(),
            })
            .collect()
    };
    match part {
        PartRef::Story(_) => {
            let here = on(page);
            if !here.is_empty() {
                return here;
            }
            (0..furniture.len())
                .map(on)
                .find(|v| !v.is_empty())
                .unwrap_or_default()
        }
        PartRef::Note(_) | PartRef::TextBox(_) => (0..furniture.len()).flat_map(on).collect(),
    }
}

/// The floating object of text box `b` on the pages (on `page` first: a
/// header's object is on every page), with the page it's on.
pub(crate) fn box_item(
    floats: &[Vec<docs_pdf::floats::FloatItem>],
    b: BoxRef,
    page: usize,
) -> Option<(usize, &docs_pdf::floats::FloatItem)> {
    let on = |pg: usize| {
        floats.get(pg)?.iter().find_map(|it| {
            (it.story == b.story && it.para == b.para && it.index == b.index).then_some((pg, it))
        })
    };
    on(page).or_else(|| (0..floats.len()).find_map(on))
}

/// Where text box `b` is drawn.
fn box_piece(floats: &[Vec<docs_pdf::floats::FloatItem>], b: BoxRef, page: usize) -> Option<Piece> {
    let (page, it) = box_item(floats, b, page)?;
    let (layout, x, y) = match b.which {
        None => it.text.clone()?,
        Some(k) => it
            .boxes
            .iter()
            .find(|t| t.which == k)
            .map(|t| (t.layout.clone(), t.x, t.y))?,
    };
    Some(Piece { page, x, y, layout })
}

impl DocsView {
    /// The main document (with the part being edited taken out of it).
    pub(crate) fn main_doc(&self) -> &Document {
        match &self.part {
            Some(s) => &s.main,
            None => &self.doc,
        }
    }

    /// The whole document as it would be saved, the part being edited
    /// included.
    pub(crate) fn whole_doc(&self) -> std::borrow::Cow<'_, Document> {
        match &self.part {
            None => std::borrow::Cow::Borrowed(&self.doc),
            Some(s) => {
                let mut main = s.main.clone();
                main.finish_part(self.fitted_edit(s), self.doc.clone(), s.body_sel);
                std::borrow::Cow::Owned(main)
            }
        }
    }

    /// The part edit `s`, with the height a "Resize shape to fit text"
    /// box is drawn at now (the shape keeps it when editing ends).
    fn fitted_edit(&self, s: &PartSession) -> PartEdit {
        let mut edit = s.edit.clone();
        if let PartTarget::TextBox(b) = s.target
            && b.which.is_none()
            && let Some((_, it)) = box_item(&self.laid.floats, b, s.page)
        {
            edit.fit_height = Some((it.rect[3] * docs_engine::object::EMU_PER_PX).round() as i64);
        }
        edit
    }

    /// The body selection (the one to come back to while a part is
    /// edited).
    pub(crate) fn body_sel(&self) -> Selection {
        self.part.as_ref().map_or(self.sel, |s| s.body_sel)
    }

    pub(crate) fn part_target(&self) -> Option<&PartTarget> {
        self.part.as_ref().map(|s| &s.target)
    }

    /// The section and kind of header or footer page `page` shows for
    /// `kind`: different first page, odd and even pages as Word decides.
    pub(crate) fn hf_type_on_page(&self, page: usize) -> (usize, HfType) {
        let doc = self.main_doc();
        let sections = doc.sections();
        let pages = &self.laid.paged.pages;
        let Some(pg) = pages.get(page) else {
            return (0, HfType::Default);
        };
        let si = pg.section.min(sections.len().saturating_sub(1));
        let first = page == 0 || pages[page - 1].section != pg.section;
        let ty = if first && sections[si].props.title_page {
            HfType::First
        } else if doc.even_odd_headers() && pg.number.is_multiple_of(2) {
            HfType::Even
        } else {
            HfType::Default
        };
        (si, ty)
    }

    /// Start editing the header or footer `page` shows (created if the
    /// page has none), caret at its start or at `at` (page coordinates).
    pub(crate) fn edit_hf_on_page(
        &mut self,
        page: usize,
        kind: HfKind,
        at: Option<(f32, f32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_part(window, cx);
        let (section, ty) = self.hf_type_on_page(page);
        let Some((edit, part)) = self.doc.edit_hf(section, kind, ty) else {
            self.status = kantor_ui::i18n::t("Kantor can't edit this header or footer").into();
            cx.notify();
            return;
        };
        self.begin_part(
            edit,
            part,
            PartTarget::Hf { section, kind, ty },
            page,
            window,
            cx,
        );
        self.fill_hf_bar(window, cx);
        let pos = at
            .and_then(|(x, y)| self.laid.hit_test(page, x, y))
            .map(|(para, off)| self.doc.clamp(Position::new(para, off)))
            .unwrap_or_default();
        self.sel = Selection::caret(pos);
        self.status = match kind {
            HfKind::Header => kantor_ui::i18n::t(
                "Editing the header: double-click the text or press Esc to go back",
            ),
            HfKind::Footer => kantor_ui::i18n::t(
                "Editing the footer: double-click the text or press Esc to go back",
            ),
        }
        .into();
        self.reveal_caret.set(true);
        cx.notify();
    }

    /// Start editing a footnote or endnote, caret at the end of its text
    /// (or at `at`, page coordinates on `page`).
    pub(crate) fn edit_note_in_place(
        &mut self,
        kind: NoteKind,
        id: &str,
        at: Option<(usize, f32, f32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.leave_part(window, cx);
        let page = self.caret_page();
        let Some((edit, part)) = self.doc.edit_note(kind, id) else {
            return;
        };
        self.begin_part(
            edit,
            part,
            PartTarget::Note {
                kind,
                id: id.to_owned(),
            },
            page,
            window,
            cx,
        );
        let end = self.doc.end();
        let pos = at
            .and_then(|(page, x, y)| self.laid.hit_test(page, x, y))
            .map(|(para, off)| self.doc.clamp(Position::new(para, off)))
            .unwrap_or(end);
        self.sel = Selection::caret(pos);
        self.status = SharedString::default();
        self.reveal_caret.set(true);
        cx.notify();
    }

    /// Start editing a text box where it's drawn (Word's click into a
    /// text box), caret at `at` (page coordinates on `page`) or at the end
    /// of its text. `false` when it can't be edited in place.
    pub(crate) fn edit_text_box_in_place(
        &mut self,
        b: BoxRef,
        page: usize,
        at: Option<(f32, f32)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.leave_part(window, cx);
        let Some((edit, part)) = self.doc.edit_text_box(b) else {
            return false;
        };
        self.begin_part(edit, part, PartTarget::TextBox(b), page, window, cx);
        let end = self.doc.end();
        let pos = at
            .and_then(|(x, y)| self.laid.hit_test(page, x, y))
            .map(|(para, off)| self.doc.clamp(Position::new(para, off)))
            .unwrap_or(end);
        self.sel = Selection::caret(pos);
        self.status =
            kantor_ui::i18n::t("Editing the text box: click outside it or press Esc to go back")
                .into();
        self.reveal_caret.set(true);
        cx.notify();
        true
    }

    fn begin_part(
        &mut self,
        edit: PartEdit,
        part: Document,
        target: PartTarget,
        page: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.float_sel = None;
        self.text_box_edit = None;
        self.typing_props = None;
        self.marked = None;
        self.goal_x = None;
        let main = std::mem::replace(&mut self.doc, part);
        self.part = Some(PartSession {
            edit,
            main,
            body_sel: self.sel,
            target,
            page,
        });
        self.sel = Selection::default();
        self.relayout();
        self.refresh_find(cx);
        self.update_title(window, cx);
    }

    /// Stop editing the header, footer or note and go back to the body:
    /// what changed becomes one undo step. Returns whether a part was
    /// being edited.
    pub(crate) fn leave_part(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.part.take() else {
            return false;
        };
        self.hf_menu = None;
        self.float_sel = None;
        self.text_box_edit = None;
        let edit = self.fitted_edit(&s);
        let part = std::mem::replace(&mut self.doc, s.main);
        self.doc.finish_part(edit, part, s.body_sel);
        self.sel = self.doc.clamp_selection(s.body_sel);
        self.typing_props = None;
        self.marked = None;
        self.goal_x = None;
        self.relayout();
        self.refresh_find(cx);
        self.refresh_spelling(cx);
        self.status = SharedString::default();
        self.update_title(window, cx);
        cx.notify();
        true
    }

    /// Esc while editing a note: back to its reference in the text.
    pub(crate) fn leave_note_to_reference(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let note = match self.part_target() {
            Some(PartTarget::Note { kind, id }) => Some((*kind, id.clone())),
            _ => None,
        };
        self.leave_part(window, cx);
        if let Some((kind, id)) = note
            && let Some(r) = self
                .doc
                .note_refs()
                .into_iter()
                .find(|r| r.kind == kind && r.id == id)
        {
            self.sel = Selection::caret(Position::new(
                r.pos.para,
                r.pos.offset + docs_engine::OBJECT.len_utf8(),
            ));
            self.reveal_caret.set(true);
        }
    }

    /// Commit the part being edited (Save, export) and carry on editing
    /// it where the caret is.
    pub(crate) fn commit_part(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &self.part else { return };
        let (target, page, sel) = (s.target.clone(), s.page, self.sel);
        self.leave_part(window, cx);
        match target {
            PartTarget::Hf { kind, .. } => self.edit_hf_on_page(page, kind, None, window, cx),
            PartTarget::Note { kind, id } => self.edit_note_in_place(kind, &id, None, window, cx),
            PartTarget::TextBox(b) => {
                self.edit_text_box_in_place(b, page, None, window, cx);
            }
        }
        if self.part.is_some() {
            self.sel = self.doc.clamp_selection(sel);
        }
    }

    /// Make the view's document the whole main document for a moment
    /// (painting pictures and floating objects, comment cards); returns
    /// whether it did ([`Self::whole_out`] undoes it).
    pub(crate) fn whole_in(&mut self) -> bool {
        let Some(s) = &mut self.part else {
            return false;
        };
        s.main.swap_part(&s.edit, &mut self.doc);
        std::mem::swap(&mut s.main, &mut self.doc);
        true
    }

    pub(crate) fn whole_out(&mut self) {
        if let Some(s) = &mut self.part {
            std::mem::swap(&mut s.main, &mut self.doc);
            s.main.swap_part(&s.edit, &mut self.doc);
        }
    }

    /// A mouse down while a part is edited: `None` when no part is edited
    /// (or the click left it: handle it as in the body), `Some(true)` when
    /// handled, `Some(false)` when it is in the part (place the caret).
    pub(crate) fn part_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        let s = self.part.as_ref()?;
        let (target, part, on_page) = (s.target.clone(), s.edit.part, s.page);
        let g = self.geometry();
        let (page, x, y) = g.screen_to_page(ev.position, self.laid.paged.pages.len());
        match target {
            PartTarget::Hf { kind, .. } => {
                let Some(k) = self.margin_at(ev.position) else {
                    // The dimmed body: a double click goes back to it.
                    if ev.click_count == 2 {
                        self.leave_part(window, cx);
                        return None;
                    }
                    return Some(true);
                };
                let (si, ty) = self.hf_type_on_page(page);
                let main = self.main_doc();
                let here = main.story_index_for(&main.sections(), si, k, ty);
                if k == kind && here.is_some() && here.map(PartRef::Story) == Some(part) {
                    if page != on_page {
                        if let Some(s) = &mut self.part {
                            s.page = page;
                        }
                        self.relayout();
                    }
                    return Some(false);
                }
                // Another header or footer: edit that one.
                self.edit_hf_on_page(page, k, Some((x, y)), window, cx);
                Some(true)
            }
            PartTarget::Note { kind, id } => {
                if let Some(((k, i), at)) = self.note_area_at(ev.position) {
                    if k == kind && i == id {
                        // A double click on the note's own number: back to
                        // its number in the text, as in Word.
                        if ev.click_count == 2
                            && let Some(pos) = self.hit(ev.position)
                            && self.at_own_note_mark(pos)
                        {
                            self.leave_note_to_reference(window, cx);
                            cx.notify();
                            return Some(true);
                        }
                        return Some(false);
                    }
                    self.edit_note_in_place(k, &i, Some(at), window, cx);
                    self.dragging = ev.click_count <= 1;
                    return Some(true);
                }
                // Notes are part of the text: a click elsewhere goes there.
                self.leave_part(window, cx);
                None
            }
            PartTarget::TextBox(b) => {
                let inside = box_item(&self.laid.floats, b, on_page).is_some_and(|(pg, it)| {
                    let [rx, ry, rw, rh] = it.rect;
                    pg == page && x >= rx && x <= rx + rw && y >= ry && y <= ry + rh
                });
                if inside {
                    return Some(false);
                }
                // Anywhere else: back to the text, and the click goes there.
                self.leave_part(window, cx);
                None
            }
        }
    }

    /// Pieces of the part being edited, after a layout.
    pub(crate) fn part_pieces(
        &self,
        furniture: &[Vec<docs_pdf::Placed>],
        floats: &[Vec<docs_pdf::floats::FloatItem>],
    ) -> Option<Vec<Piece>> {
        let s = self.part.as_ref()?;
        Some(pieces(furniture, floats, s.edit.part, s.page))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    /// The main document laid out with the part `edit` put back, and the
    /// text view onto it.
    fn laid_with(main: &mut Document, edit: &PartEdit, part: &mut Document, page: usize) -> Laid {
        main.swap_part(edit, part);
        let laid = docs_pdf::lay_out(main, &mut docs_layout::Engine::new());
        main.swap_part(edit, part);
        let pieces = pieces(&laid.furniture, &laid.floats, edit.part, page);
        Laid {
            boxes: Rc::new(crate::PageBoxes::of(&laid.paged)),
            paged: laid.paged,
            texts: Vec::new(),
            furniture: laid.furniture,
            floats: laid.floats,
            strikes: laid.strikes,
            part: Some(pieces),
        }
    }

    #[std::prelude::v1::test]
    fn text_box_caret_and_clicks_land_in_the_box() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/corpus/docx/floats.docx");
        let mut main = Document::open(&path).unwrap();
        let (para, index) = main
            .paragraphs()
            .iter()
            .enumerate()
            .find_map(|(i, p)| {
                p.objects().find_map(|(at, o)| {
                    o.float()
                        .filter(|f| f.name == "Text Box 1")
                        .map(|_| (i, at))
                })
            })
            .unwrap();
        let b = BoxRef {
            story: None,
            para,
            index,
            which: None,
        };
        let (edit, mut part) = main.edit_text_box(b).unwrap();
        part.insert_text(Selection::caret(Position::new(0, 0)), "New ", None);
        let laid = laid_with(&mut main, &edit, &mut part, 0);
        let pieces = laid.part.as_ref().unwrap();
        assert_eq!(pieces.len(), 1);
        let (pg, it) = box_item(&laid.floats, b, 0).unwrap();
        let [x, y, w, h] = it.rect;
        // The caret is inside the box, after the typed text.
        let c = laid.caret(0, 4).expect("caret in the text box");
        assert_eq!(c.page, pg);
        assert!(
            c.x > x && c.x < x + w && c.top >= y - 0.5 && c.top < y + h,
            "{c:?} {:?}",
            it.rect
        );
        // A click there finds the box's text, not the body's.
        let hit = laid.hit_test(pg, c.x, c.top + c.height / 2.0).unwrap();
        assert_eq!(hit, (0, 4));
        let second = laid.caret(1, 0).unwrap();
        assert!(second.top > c.top, "second paragraph below the first");
        assert!(main.finish_part(edit, part, Selection::default()));
        assert_eq!(
            main.float_at(para, index)
                .unwrap()
                .text_box_text()
                .as_deref(),
            Some("New Text box\nwith two paragraphs")
        );
    }

    #[std::prelude::v1::test]
    fn header_caret_and_clicks_land_in_the_header_area() {
        let mut main = Document::new();
        main.insert_text(Selection::default(), "Body text", None);
        let (edit, mut part) = main.edit_hf(0, HfKind::Header, HfType::Default).unwrap();
        part.insert_text(Selection::default(), "Header text", None);
        let laid = laid_with(&mut main, &edit, &mut part, 0);
        let pieces = laid.part.as_ref().unwrap();
        assert_eq!(pieces.len(), 1);
        let p = &pieces[0];
        assert_eq!(p.page, 0);
        let setup = laid.paged.page_setup(0);
        // 708 twips (Word's 1.25 cm) from the top edge.
        assert!((p.y - 708.0 / 15.0).abs() < 0.5, "{}", p.y);
        let c = laid.caret(0, 3).expect("caret in the header");
        assert_eq!(c.page, 0);
        assert!(c.top >= p.y - 0.5 && c.top + c.height <= setup.margin_top + 0.5);
        // Clicking there finds the header text, not the body.
        let hit = laid.hit_test(0, c.x, c.top + c.height / 2.0).unwrap();
        assert_eq!(hit, (0, 3));
        let rects = laid.selection_rects((0, 0), (0, 6));
        assert!(!rects.is_empty());
        assert!(
            rects
                .iter()
                .all(|(pg, r)| *pg == 0 && r[1] < setup.margin_top)
        );
        // The body's own caret is unaffected by the view of the part.
        let body = laid.paged.caret(0, 0).unwrap();
        assert!(body.top >= setup.margin_top - 0.5);
        main.finish_part(edit, part, Selection::default());
    }

    #[std::prelude::v1::test]
    fn footer_on_a_later_page_and_notes_at_the_bottom() {
        let mut main = Document::new();
        let mut c = Selection::default();
        for i in 0..80 {
            c = main.insert_text(c, &format!("Line {i} of the body.\n"), None);
        }
        let (c, id) = main.insert_note(NoteKind::Footnote, Selection::caret(Position::new(2, 4)));
        let (edit, mut part) = main.edit_hf(0, HfKind::Footer, HfType::Default).unwrap();
        part.insert_text(Selection::default(), "Footer", None);
        let laid = laid_with(&mut main, &edit, &mut part, 1);
        assert!(laid.paged.pages.len() > 1);
        let p = &laid.part.as_ref().unwrap()[0];
        assert_eq!(p.page, 1, "edited on the page clicked");
        let setup = laid.paged.page_setup(1);
        let caret = laid.caret(0, 0).unwrap();
        assert_eq!(caret.page, 1);
        assert!(caret.top > setup.height - setup.margin_bottom - 0.5);
        main.finish_part(edit, part, c);

        let (edit, mut note) = main.edit_note(NoteKind::Footnote, &id).unwrap();
        let end = note.end();
        note.insert_text(Selection::caret(end), "A note", None);
        let laid = laid_with(&mut main, &edit, &mut note, 0);
        let pieces = laid.part.as_ref().unwrap();
        assert_eq!(pieces.len(), 1);
        let caret = laid.caret(0, note.paragraph(0).text.len()).unwrap();
        assert_eq!(caret.page, 0);
        let s = laid.paged.page_setup(0);
        assert!(caret.top > s.height / 2.0, "at the bottom of the page");
        let hit = laid
            .hit_test(0, caret.x, caret.top + caret.height / 2.0)
            .unwrap();
        assert_eq!(hit.0, 0);
        main.finish_part(edit, note, c);
        assert!(main.note_text(NoteKind::Footnote, &id).ends_with("A note"));
    }
}
