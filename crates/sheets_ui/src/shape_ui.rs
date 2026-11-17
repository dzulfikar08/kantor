//! Insert ▸ Text Box and Insert ▸ Shapes, and editing a shape's text, as
//! in Excel.
//!
//! The toolbar's Shapes menu puts a text box or a shape at the active cell
//! and selects it (a new text box opens for typing at once). Double-click a
//! shape or text box to edit its text: a box below the shape with the text
//! (one paragraph per line) and a toolbar formatting the selected text
//! (bold, italic, underline, font, size, colour, alignment; with nothing
//! selected, all of it). The shape shows the formatted text as you type,
//! and runs keep their own formatting through edits (see `rich`). Escape
//! or a click on the grid keeps it (one undo step). The box is rendered
//! from the Sheets root, never inside the grid (the grid's key handler
//! would take the typing).

use gpui_kit::component::Sizable as _;
use gpui_kit::component::input::{self, InputEvent, Position, Textarea, TextareaState};
use gpui_kit::component::menu::{PopupMenu, PopupMenuItem};
use gpui_kit::*;
use kantor_ui::i18n::t;
use kantor_ui::metrics::*;
use kantor_ui::tokens::Elevation;
use sheets_engine::ShapePreset;
use sheets_engine::drawing::DrawingKey;
use slides_engine::TextPara;

use crate::SheetsView;
use crate::rich::{self, RichOp, RichState};

/// The editor's key context (Escape ends the edit).
const SHAPE_TEXT_CONTEXT: &str = "KantorSheetsShapeText";

/// The text being written into a shape.
pub(crate) struct ShapeEditor {
    key: DrawingKey,
    input: Entity<TextareaState>,
    /// The text's paragraphs with their formatting, following the box
    /// (`None`: text Kantor edits as plain lines, see `with_text`).
    paras: Option<Vec<TextPara>>,
    /// The paragraphs as the edit began (an edit that changes nothing
    /// leaves the shape alone).
    orig: Option<Vec<TextPara>>,
    _sub: Subscription,
}

/// A preset's name in the menu.
fn label(p: ShapePreset) -> &'static str {
    match p {
        ShapePreset::TextBox => t("Text Box"),
        ShapePreset::Rectangle => t("Rectangle"),
        ShapePreset::RoundedRectangle => t("Rectangle: Rounded Corners"),
        ShapePreset::Oval => t("Oval"),
        ShapePreset::Line => t("Line"),
        ShapePreset::Arrow => t("Line Arrow"),
        ShapePreset::RightArrow => t("Arrow: Right"),
        ShapePreset::Callout => t("Speech Bubble: Rectangle"),
    }
}

/// The toolbar's Shapes menu: Text Box, then the shapes.
pub(crate) fn shape_menu(mut menu: PopupMenu, view: &WeakEntity<SheetsView>) -> PopupMenu {
    let item = |p: ShapePreset| {
        let v = view.clone();
        PopupMenuItem::new(label(p)).on_click(move |_, w, cx| {
            let _ = v.update(cx, |this, cx| this.insert_shape_doc(p, w, cx));
        })
    };
    menu = menu.item(item(ShapePreset::TextBox)).separator();
    for p in ShapePreset::SHAPES {
        menu = menu.item(item(p));
    }
    let v = view.clone();
    menu.separator().item(
        PopupMenuItem::new(t("Format Shape…")).on_click(move |_, w, cx| {
            let _ = v.update(cx, |this, cx| this.format_shape_doc(w, cx));
        }),
    )
}

/// The caret at the end of `text`.
fn end_of(text: &str) -> Position {
    let line = text.split('\n').count().saturating_sub(1);
    let last = text.rsplit('\n').next().unwrap_or_default();
    Position::new(line as u32, last.encode_utf16().count() as u32)
}

impl SheetsView {
    /// Insert ▸ Text Box / Shapes: `preset` at the active cell, selected.
    pub fn insert_shape_doc(
        &mut self,
        preset: ShapePreset,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.editing.is_some() || self.blocked(cx) {
            return;
        }
        let (row, col) = self.selected;
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let sheet = grid.sheet;
            let r = grid.book.insert_shape(sheet, preset, row, col);
            grid.resize();
            table.refresh(cx);
            r
        });
        match res {
            Ok(key) => {
                self.drawings.borrow_mut().selected = Some(key);
                self.status = t("Inserted").into();
                if preset == ShapePreset::TextBox {
                    self.open_shape_editor(key, window, cx);
                }
            }
            Err(e) => self.status = format!("{e:#}").into(),
        }
        self.update_title(window, cx);
        cx.notify();
    }

    /// Edit the text of shape `key` (double-click): nothing for shapes
    /// without text (lines, groups, form controls).
    pub(crate) fn open_shape_editor(
        &mut self,
        key: DrawingKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.shape_edit.as_ref().is_some_and(|e| e.key == key) {
            return;
        }
        self.commit_shape_text(false, window, cx);
        let (text, paras) = {
            let book = &self.table.read(cx).delegate().book;
            let Some(text) = book.shape_text(key) else {
                if book
                    .shape_xml(key)
                    .is_some_and(|x| sheets_engine::shape_format::text_has_fields(&x))
                {
                    self.status = t(
                        "This shape's text has a cell link, fields or equations: Kantor shows and keeps them but can't edit them",
                    )
                    .into();
                    cx.notify();
                }
                return;
            };
            let paras = book
                .shape_xml(key)
                .and_then(|x| slides_engine::sheet_shape::text_paras(&x));
            match paras {
                Some(p) => (rich::plain(&p), Some(p)),
                None => (text, None),
            }
        };
        let input = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx).auto_grow(3, 12);
            s.set_value(text.clone(), window, cx);
            s
        });
        let sub = cx.subscribe_in(&input, window, |this, _, e: &InputEvent, _, cx| {
            if let InputEvent::Change = e {
                this.shape_text_changed(cx);
            }
        });
        self.drawings.borrow_mut().selected = Some(key);
        self.shape_edit = Some(ShapeEditor {
            key,
            input: input.clone(),
            orig: paras.clone(),
            paras,
            _sub: sub,
        });
        // After a menu closes or a click ends it gives focus back: focus the
        // editor once that is done.
        window.defer(cx, move |window, cx| {
            input.update(cx, |s, cx| s.set_cursor_position(end_of(&text), window, cx));
        });
        cx.notify();
    }

    /// End the text edit, keeping its text (one undo step if it changed).
    /// `refocus`: give the grid the keyboard again.
    pub(crate) fn commit_shape_text(
        &mut self,
        refocus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(ed) = self.shape_edit.take() else {
            return;
        };
        self.drawings.borrow_mut().preview = None;
        let text = ed.input.read(cx).value().to_string();
        let res = self.table.update(cx, |table, cx| {
            let grid = table.delegate_mut();
            let before = grid.book.revision();
            let r = match &ed.paras {
                Some(p) if ed.orig.as_ref() == Some(p) => Ok(()),
                Some(p) => match grid
                    .book
                    .shape_xml(ed.key)
                    .and_then(|x| slides_engine::sheet_shape::with_paras(&x, p))
                {
                    Some(x) => grid.book.set_shape_xml(ed.key, &x),
                    None => grid.book.set_shape_text(ed.key, &text),
                },
                None => grid.book.set_shape_text(ed.key, &text),
            }
            .map(|()| grid.book.revision() != before);
            table.refresh(cx);
            r
        });
        match res {
            Ok(true) => self.status = t("Text changed").into(),
            Ok(false) => {}
            Err(e) => self.status = format!("{e:#}").into(),
        }
        self.update_title(window, cx);
        if refocus {
            self.focus_grid(window, cx);
        }
        cx.notify();
    }

    /// The box's text changed: the paragraphs follow, the shape redraws.
    fn shape_text_changed(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.shape_edit.as_mut() else {
            return;
        };
        let Some(paras) = ed.paras.as_mut() else {
            return;
        };
        let text = ed.input.read(cx).value().replace("\r\n", "\n");
        if rich::plain(paras) != text {
            slides_engine::text_ops::replace_changed(paras, &text);
        }
        self.preview_shape_text(cx);
    }

    /// Draw the shape with the paragraphs being edited.
    fn preview_shape_text(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.shape_edit.as_ref() else {
            return;
        };
        let Some(paras) = ed.paras.as_ref() else {
            return;
        };
        let xml = self
            .table
            .read(cx)
            .delegate()
            .book
            .shape_xml(ed.key)
            .and_then(|x| slides_engine::sheet_shape::with_paras(&x, paras));
        self.drawings.borrow_mut().preview = xml.map(|x| (ed.key, x.into()));
        cx.notify();
    }

    /// A toolbar command on the box's selection (all the text without one).
    fn shape_rich_op(&mut self, op: RichOp, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.shape_edit.as_mut() else {
            return;
        };
        let range = ed.input.read(cx).selected_range();
        let input = ed.input.clone();
        let Some(paras) = ed.paras.as_mut() else {
            return;
        };
        let sel = rich::selection(paras, range);
        rich::apply(paras, sel, op);
        self.preview_shape_text(cx);
        // Typing goes on in the box.
        input.update(cx, |s, cx| s.focus(window, cx));
    }

    /// The editor box below the shape being edited.
    pub(crate) fn render_shape_editor(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let ed = self.shape_edit.as_ref()?;
        let tok = kantor_ui::tokens(cx);
        let zoom = {
            let grid = self.table.read(cx).delegate();
            crate::look::zoom(&grid.book, grid.sheet)
        };
        let rect = self.drawings.borrow().screen_rect(ed.key);
        // Below the shape, so it shows the text as it's formatted.
        let (at, w) = match rect {
            Some(b) => (
                point(b.origin.x, b.origin.y + b.size.height + SPACE_2),
                b.size.width.max(px(340.)),
            ),
            None => (point(SPACE_7 * 4., SPACE_7 * 4.), px(340.)),
        };
        let bar = ed.paras.as_ref().map(|paras| {
            let sel = rich::selection(paras, ed.input.read(cx).selected_range());
            let state = RichState::at(paras, sel);
            let view = cx.entity().downgrade();
            rich::toolbar(
                "shape-rich",
                &state,
                true,
                std::rc::Rc::new(move |op, w, cx| {
                    let _ = view.update(cx, |this, cx| this.shape_rich_op(op, w, cx));
                }),
                cx,
            )
        });
        let panel = div()
            .id("sheets-shape-text")
            .key_context(SHAPE_TEXT_CONTEXT)
            .occlude()
            .w(w)
            .p(SPACE_2)
            .flex()
            .flex_col()
            .gap(SPACE_1)
            .bg(tok.elevated)
            .border_1()
            .border_color(tok.accent)
            .rounded(RADIUS_SM)
            .shadow(tok.shadow(Elevation::Popover))
            .text_size(FONT_SMALL)
            .text_color(tok.fg)
            .on_action(cx.listener(|this, _: &input::Escape, window, cx| {
                this.commit_shape_text(true, window, cx);
            }))
            .child(div().text_color(tok.fg_muted).child(t("Edit Text")))
            .children(bar)
            // The text types at the sheet's zoom, as the shape shows it.
            .child(
                Textarea::new(&ed.input)
                    .small()
                    .text_size(FONT_SMALL * zoom),
            );
        Some(
            deferred(
                anchored()
                    .position(at)
                    .snap_to_window_with_margin(SPACE_4)
                    .child(panel),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn every_preset_has_a_translated_name() {
        for p in std::iter::once(ShapePreset::TextBox).chain(ShapePreset::SHAPES) {
            assert!(
                kantor_ui::i18n::indonesian(label(p)).is_some(),
                "{}",
                label(p)
            );
        }
    }

    #[std::prelude::v1::test]
    fn caret_goes_to_the_end() {
        assert_eq!(end_of(""), Position::new(0, 0));
        assert_eq!(end_of("ab\ncde"), Position::new(1, 3));
    }
}
