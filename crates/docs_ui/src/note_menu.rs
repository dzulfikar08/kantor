//! Footnotes and endnotes in the Docs view: Insert Footnote (Ctrl+Alt+F)
//! and Insert Endnote (Ctrl+Alt+D) put a numbered reference at the caret
//! and open the note's popover; double-clicking a note at the bottom of a
//! page (or its reference in the text) opens it again. The popover edits
//! the note's text (plain: a note with pictures or tables is shown and
//! kept, but not edited here), and Delete removes the note with its
//! reference. Every change is one undo step.

use docs_engine::{NoteKind, Position};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{Sizable as _, h_flex, tooltip::Tooltip, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};

use crate::DocsView;

/// The note being edited.
pub(crate) struct NoteEdit {
    kind: NoteKind,
    id: String,
    at: Point<Pixels>,
    input: Entity<TextareaState>,
    locked: Option<String>,
    _sub: Subscription,
}

impl DocsView {
    /// Insert a footnote or endnote at the caret and start writing it.
    pub fn insert_note_doc(&mut self, kind: NoteKind, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_note(window, cx);
        let (sel, id) = self.doc.insert_note(kind, self.sel);
        self.edited(sel, window, cx);
        self.open_note(kind, id, None, window, cx);
    }

    /// Edit the note whose reference is at the caret (palette).
    pub fn edit_note_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.doc.note_at(self.sel.head) {
            Some((kind, id)) => self.open_note(kind, id, None, window, cx),
            None => {
                self.status = "Put the caret after a footnote or endnote number".into();
                cx.notify();
            }
        }
    }

    /// The note shown at a window point: a note area at the bottom of a
    /// page, or a reference in the text.
    pub(crate) fn note_at_point(&self, p: Point<Pixels>) -> Option<(NoteKind, String)> {
        let g = self.geometry();
        let paged = &self.laid.paged;
        let (page, x, y) = g.screen_to_page(p, paged.pages.len());
        for placed in self.laid.furniture.get(page).into_iter().flatten() {
            let Some(note) = &placed.note else { continue };
            let [px, py, w, h] = placed.rect();
            if x >= px && x <= px + w && y >= py && y <= py + h {
                return Some(note.clone());
            }
        }
        let (para, index) = paged.hit_test(page, x, y)?;
        let pos = Position::new(para, index);
        let c = paged.caret(para, index)?;
        // Only right next to the number.
        ((c.x - x).abs() < 12.0 && c.page == page)
            .then(|| self.doc.note_at(pos))
            .flatten()
    }

    /// Open the popover on a note (near the caret, or at `at`).
    pub(crate) fn open_note(
        &mut self,
        kind: NoteKind,
        id: String,
        at: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let at = at.unwrap_or_else(|| {
            let g = self.geometry();
            let h = self.sel.head;
            match self.laid.paged.caret(h.para, h.offset) {
                Some(c) => {
                    let p = g.page_to_screen(c.page, c.x, c.top + c.height);
                    point(p.x, p.y + SPACE_3)
                }
                None => {
                    let b = self.viewport.get();
                    point(b.origin.x + SPACE_7, b.origin.y + SPACE_7)
                }
            }
        });
        let text = self.doc.note_text(kind, &id);
        let locked = self.doc.note_locked(kind, &id);
        let input = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder(format!("Write the {}", kind.label().to_lowercase()));
            s.set_value(text, window, cx);
            s
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = ev
            {
                this.apply_note(window, cx);
                this.focus(window, cx);
            }
        });
        if locked.is_none() {
            input.update(cx, |s, cx| s.focus(window, cx));
        }
        self.note_edit = Some(NoteEdit {
            kind,
            id,
            at,
            input,
            locked,
            _sub: sub,
        });
        cx.notify();
    }

    /// Write the open note's text (one undo step) and close the popover.
    pub(crate) fn apply_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.note_edit.take() else {
            return;
        };
        if edit.locked.is_some() {
            cx.notify();
            return;
        }
        // Ctrl+Enter may have put a line break in before it closes.
        let value = edit.input.read(cx).value().to_string();
        let text = value.trim_end_matches(['\n', '\r']);
        if self.doc.note_text(edit.kind, &edit.id) != text
            && self.doc.set_note_text(edit.kind, &edit.id, text, self.sel)
        {
            let sel = self.sel;
            self.edited(sel, window, cx);
            self.status = format!("{} updated", edit.kind.label()).into();
        }
        cx.notify();
    }

    fn delete_open_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.note_edit.take() else {
            return;
        };
        if self.doc.delete_note(edit.kind, &edit.id, self.sel) {
            let sel = self.sel;
            self.edited(sel, window, cx);
            self.status = format!("{} deleted", edit.kind.label()).into();
        }
        self.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn render_note_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let edit = self.note_edit.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let label = self
            .doc
            .note_refs()
            .into_iter()
            .find(|r| r.kind == edit.kind && r.id == edit.id)
            .map(|r| r.label)
            .unwrap_or_default();
        let title = format!("{} {label}", edit.kind.label());
        let button = |id: &'static str, text: &'static str, primary: bool| {
            div()
                .id(id)
                .flex_none()
                .h(CONTROL_SM)
                .px(SPACE_5)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when(primary, |d| d.bg(t.accent).text_color(t.accent_fg))
                .when(!primary, |d| {
                    d.text_color(t.fg_muted)
                        .hover(|s| s.bg(t.hover).text_color(t.fg))
                })
                .child(text)
        };
        let body = match &edit.locked {
            Some(why) => div()
                .p(SPACE_3)
                .rounded(RADIUS_MD)
                .bg(t.surface)
                .text_color(t.fg_muted)
                .child(why.clone())
                .into_any_element(),
            None => Textarea::new(&edit.input).small().into_any_element(),
        };
        let delete = {
            let v = view.clone();
            button("docs-note-delete", "Delete", false)
                .tooltip(|window, cx| {
                    Tooltip::new("Delete the note and its number in the text").build(window, cx)
                })
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| this.delete_open_note(window, cx));
                })
        };
        let done = {
            let v = view.clone();
            button("docs-note-apply", "Done", true)
                .tooltip(|window, cx| Tooltip::new("Done  Ctrl+Enter").build(window, cx))
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| {
                        this.apply_note(window, cx);
                        this.focus(window, cx);
                    });
                })
        };
        let panel = v_flex()
            .id("docs-note-menu")
            .occlude()
            .w(REM * 22.)
            .gap(SPACE_3)
            .p(SPACE_4)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .rounded(RADIUS_LG)
            .shadow(t.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .text_color(t.fg)
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                this.apply_note(window, cx);
            }))
            .child(div().text_color(t.fg_muted).child(title))
            .child(body)
            .child(
                h_flex()
                    .gap(SPACE_2)
                    .child(delete)
                    .child(div().flex_1())
                    .child(done),
            );
        Some(deferred(
            anchored()
                .position(edit.at)
                .anchor(Anchor::TopLeft)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel),
        ))
    }
}
