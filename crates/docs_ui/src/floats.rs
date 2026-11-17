//! Floating pictures, shapes and text boxes in the Docs view, as in Word:
//! a click selects one (eight handles and the Layout Options button); a
//! drag moves it (its new place is stored from the column and the anchor
//! paragraph, as Word does when an object moves with text) or resizes it
//! from a handle (corners keep a picture's proportions); Delete removes
//! it; Layout Options picks In Line with Text (pictures), Square, Tight,
//! Through, Top and Bottom, Behind Text or In Front of Text. A double
//! click on a text box edits its text in a popover (plain text, as notes
//! are). Objects in headers and footers are shown but not selected.
//! Every change is one undo step.

use std::collections::HashMap;
use std::sync::Arc;

use docs_engine::float::{Place, RelH, RelV};
use docs_engine::object::EMU_PER_PX;
use docs_engine::{LayoutOption, OBJECT, Object, Position, Selection};
use docs_pdf::floats::{FloatPrim, Seg};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::{
    Sizable as _,
    button::Button,
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    v_flex,
};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, t, t_dyn, tk};
use kantor_ui::tokens::Elevation;
use kantor_ui::{components::section_label, metrics::*, tokens};

use crate::{DocsView, Frame, Geometry, layout_color};

/// Handle size on screen, px.
const HANDLE: f32 = 8.0;
/// The smallest size a resize leaves, px.
const MIN_SIZE: f32 = 8.0;

/// The selected floating object (in the body).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FloatSel {
    pub para: usize,
    pub index: usize,
}

/// What a drag on a floating object does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Grab {
    Move,
    /// Handle 0–7, clockwise from the top left corner.
    Handle(usize),
}

/// A floating object being moved or resized.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FloatDrag {
    pub page: usize,
    pub grab: Grab,
    /// Where the drag started, page coordinates.
    pub start: (f32, f32),
    pub orig: [f32; 4],
    /// The box now (drawn as an outline until the drag ends).
    pub cur: [f32; 4],
    pub moved: bool,
    /// A picture: corners keep its proportions.
    pub keep_ratio: bool,
}

/// The text box being edited.
pub(crate) struct TextBoxEdit {
    sel: FloatSel,
    /// A group's shape's text ([`docs_engine::Float::boxes`]), or the
    /// object's own text box.
    which: Option<usize>,
    at: Point<Pixels>,
    input: Entity<TextareaState>,
    locked: Option<String>,
    /// What's edited (the popover's title).
    owner: TextOwner,
    /// Focus the text when the double click's button goes up (the page
    /// takes the focus back on mouse down).
    focus_on_up: bool,
    _sub: Subscription,
}

const LOCKED: &str = tk(
    "This text box has pictures, tables or other objects; Kantor shows it and keeps it as it is, but can't edit it yet",
);

/// What the text popover edits: Word's names for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextOwner {
    /// A text box (`wps:cNvSpPr/@txBox`, or VML's `v:textbox` on a
    /// text box shape).
    TextBox,
    /// Text in a shape (a shape with text, or a group's shape).
    Shape,
    SmartArt,
}

impl TextOwner {
    fn of(f: &docs_engine::Float, which: Option<usize>) -> Self {
        match (&f.kind, which) {
            (docs_engine::FloatKind::SmartArt { .. }, _) => Self::SmartArt,
            (_, Some(_)) => Self::Shape,
            _ if f.xml.contains("txBox=\"1\"") || f.xml.contains("o:spt=\"202\"") => Self::TextBox,
            (docs_engine::FloatKind::Shape, None) => Self::Shape,
            _ => Self::TextBox,
        }
    }

    fn title(self) -> &'static str {
        match self {
            Self::TextBox => t("Text box"),
            Self::Shape => t("Shape text"),
            Self::SmartArt => t("SmartArt text"),
        }
    }

    fn placeholder(self) -> &'static str {
        match self {
            Self::TextBox => t("Write the text box's text"),
            Self::Shape => t("Write the shape's text"),
            Self::SmartArt => t("Write the SmartArt's text, one line per item"),
        }
    }

    fn updated(self) -> &'static str {
        match self {
            Self::TextBox => t("Text box updated"),
            Self::Shape => t("Shape text updated"),
            Self::SmartArt => t("SmartArt text updated"),
        }
    }
}

/// SmartArt edit messages from `docs_pdf::smartart` (translated with
/// `t_dyn`).
const SMARTART_MESSAGES: [&str; 12] = [
    tk("This SmartArt shape shows several items: keep one line per item"),
    tk("Kantor can't read this SmartArt's drawing"),
    tk("This SmartArt shape doesn't show an item"),
    tk("A SmartArt keeps at least one item"),
    tk(
        "Kantor can't lay out this SmartArt layout yet: choose another layout with Change Layout to add or remove items",
    ),
    tk(
        "A Labeled Hierarchy's labels take no shapes above or below them: add labels before or after them",
    ),
    tk("The top of a Labeled Hierarchy has no shapes beside it: add shapes below or above it"),
    tk("The top of this Labeled Hierarchy can't be deleted while it has several shapes under it"),
    tk("The centre of a Radial Cycle has no shapes beside or above it: add shapes below it"),
    tk("A Radial Cycle has two levels: add shapes before or after the ones round the centre"),
    tk("The centre of this Radial Cycle can't be deleted while it has shapes round it"),
    tk("This SmartArt has no layout part to change"),
];

/// A change to a SmartArt's items from its text popover.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ItemEdit {
    AddBefore,
    AddAfter,
    Delete,
}

/// Word's Layout Options names in the interface language.
fn option_label(o: LayoutOption) -> &'static str {
    match o {
        LayoutOption::Inline => t("In Line with Text"),
        LayoutOption::Square => t("Square"),
        LayoutOption::Tight => t("Tight"),
        LayoutOption::Through => t("Through"),
        LayoutOption::TopAndBottom => t("Top and Bottom"),
        LayoutOption::Behind => t("Behind Text"),
        LayoutOption::InFront => t("In Front of Text"),
    }
}

/// The eight handle centres of a box: corners and edge middles,
/// clockwise from the top left.
fn handles([x, y, w, h]: [f32; 4]) -> [(f32, f32); 8] {
    [
        (x, y),
        (x + w / 2.0, y),
        (x + w, y),
        (x + w, y + h / 2.0),
        (x + w, y + h),
        (x + w / 2.0, y + h),
        (x, y + h),
        (x, y + h / 2.0),
    ]
}

impl FloatDrag {
    /// The box after the pointer moved to `(x, y)` (page coordinates).
    fn update(&mut self, (x, y): (f32, f32), page: (f32, f32)) {
        let (dx, dy) = (x - self.start.0, y - self.start.1);
        if dx.abs() + dy.abs() > 2.0 {
            self.moved = true;
        }
        let [ox, oy, ow, oh] = self.orig;
        self.cur = match self.grab {
            Grab::Move => {
                // Kept on its page.
                let nx = (ox + dx).clamp(-ow + 8.0, page.0 - 8.0);
                let ny = (oy + dy).clamp(-oh + 8.0, page.1 - 8.0);
                [nx, ny, ow, oh]
            }
            Grab::Handle(h) => {
                let (mut x0, mut y0, mut x1, mut y1) = (ox, oy, ox + ow, oy + oh);
                let left = matches!(h, 0 | 6 | 7);
                let right = matches!(h, 2..=4);
                let top = matches!(h, 0..=2);
                let bottom = matches!(h, 4..=6);
                if left {
                    x0 = (ox + dx).min(x1 - MIN_SIZE);
                }
                if right {
                    x1 = (ox + ow + dx).max(x0 + MIN_SIZE);
                }
                if top {
                    y0 = (oy + dy).min(y1 - MIN_SIZE);
                }
                if bottom {
                    y1 = (oy + oh + dy).max(y0 + MIN_SIZE);
                }
                let corner = matches!(h, 0 | 2 | 4 | 6);
                if corner && self.keep_ratio && ow > 0.0 && oh > 0.0 {
                    // The larger change wins; the opposite corner stays.
                    let s = ((x1 - x0) / ow).max((y1 - y0) / oh);
                    let (w, hh) = (ow * s, oh * s);
                    if left {
                        x0 = x1 - w;
                    } else {
                        x1 = x0 + w;
                    }
                    if top {
                        y0 = y1 - hh;
                    } else {
                        y1 = y0 + hh;
                    }
                }
                [x0, y0, x1 - x0, y1 - y0]
            }
        };
    }
}

impl DocsView {
    /// Whose floating objects can be selected: the body's (`Some(None)`),
    /// or the header's or footer's being edited (`Some(Some(story))`);
    /// `None` while a note is edited.
    fn float_story(&self) -> Option<Option<usize>> {
        match self.part.as_ref().map(|s| s.edit.part) {
            None => Some(None),
            Some(docs_engine::PartRef::Story(i)) => Some(Some(i)),
            Some(docs_engine::PartRef::Note(_) | docs_engine::PartRef::TextBox(_)) => None,
        }
    }

    /// Whether a header or footer is being edited.
    pub(crate) fn editing_hf(&self) -> bool {
        self.float_story().flatten().is_some()
    }

    /// Whether `it` is one of the objects that can be selected now.
    fn selectable(&self, it: &docs_pdf::floats::FloatItem) -> bool {
        self.float_story() == Some(it.story)
    }

    /// The selected floating object's box: page and `[x, y, w, h]` (a
    /// header's on the page it's edited on).
    pub(crate) fn float_sel_rect(&self) -> Option<(usize, [f32; 4])> {
        let s = self.float_sel?;
        let on = |page: usize| {
            self.laid.floats.get(page).and_then(|items| {
                items
                    .iter()
                    .find(|it| self.selectable(it) && it.para == s.para && it.index == s.index)
                    .map(|it| (page, it.rect))
            })
        };
        if let Some(found) = self.part.as_ref().and_then(|p| on(p.page)) {
            return Some(found);
        }
        (0..self.laid.floats.len()).find_map(on)
    }

    /// The body floating object under a window point (topmost first; one
    /// behind the text only where there's no text).
    fn float_hit(&self, p: Point<Pixels>) -> Option<(usize, FloatSel, [f32; 4])> {
        let g = self.geometry();
        let paged = &self.laid.paged;
        let (page, x, y) = g.screen_to_page(p, paged.pages.len());
        let items = self.laid.floats.get(page)?;
        let inside = |r: [f32; 4]| {
            // Lines (zero height) get a little room.
            let pad = if r[2] < 6.0 || r[3] < 6.0 { 4.0 } else { 0.0 };
            x >= r[0] - pad && x <= r[0] + r[2] + pad && y >= r[1] - pad && y <= r[1] + r[3] + pad
        };
        let over_text = paged.pages[page]
            .lines
            .iter()
            .any(|l| y >= l.top && y <= l.top + l.height && x >= l.x && x <= l.x + l.width);
        items
            .iter()
            .rev()
            .filter(|it| self.selectable(it) && inside(it.rect))
            .find(|it| !it.behind || !over_text)
            .map(|it| {
                (
                    page,
                    FloatSel {
                        para: it.para,
                        index: it.index,
                    },
                    it.rect,
                )
            })
    }

    /// The handle of the selected object under a window point.
    fn handle_hit(&self, p: Point<Pixels>) -> Option<(usize, usize, [f32; 4])> {
        let (page, rect) = self.float_sel_rect()?;
        let g = self.geometry();
        let (pg, x, y) = g.screen_to_page(p, self.laid.paged.pages.len());
        if pg != page {
            return None;
        }
        let r = HANDLE / g.zoom;
        handles(rect)
            .iter()
            .position(|&(hx, hy)| (x - hx).abs() <= r && (y - hy).abs() <= r)
            .map(|h| (page, h, rect))
    }

    /// Mouse down on the page: select, start moving or resizing a floating
    /// object, or edit a text box (double click). `true` when handled.
    pub(crate) fn float_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some((page, h, rect)) = self.handle_hit(ev.position) {
            self.start_float_drag(page, Grab::Handle(h), rect, ev.position);
            cx.notify();
            return true;
        }
        let Some((page, sel, rect)) = self.float_hit(ev.position) else {
            if self.float_sel.take().is_some() {
                cx.notify();
            }
            return false;
        };
        let was_selected = self.float_sel == Some(sel);
        self.float_sel = Some(sel);
        self.text_box_edit = None;
        if let Some(s) = &mut self.part {
            s.page = page;
        }
        // The caret goes to the anchor, as in Word (Delete, Layout Options
        // and undo work on the object).
        self.sel = Selection::caret(Position::new(sel.para, sel.index));
        self.dragging = false;
        // A double click, or a click on the text of a selected box, puts
        // the caret in the text (its edge still moves the box).
        let into_text = ev.click_count == 2 || (was_selected && self.on_text(rect, ev.position));
        let target = into_text
            .then(|| self.text_target(page, sel, ev.position))
            .flatten();
        if let Some(which) = target
            && self.text_in_place(page, sel, which, ev.position, window, cx)
        {
            self.dragging = ev.click_count <= 1;
            cx.notify();
            return true;
        }
        let target = target.filter(|_| ev.click_count == 2);
        if let Some(which) = target {
            let at = point(ev.position.x + SPACE_3, ev.position.y + SPACE_3);
            self.open_text_box(sel, which, at, window, cx);
        } else {
            self.start_float_drag(page, Grab::Move, rect, ev.position);
        }
        cx.notify();
        true
    }

    /// Whether a window point is inside the object box `rect`, away from
    /// its edge (where a press moves the box).
    fn on_text(&self, [x, y, w, h]: [f32; 4], p: Point<Pixels>) -> bool {
        let g = self.geometry();
        let (_, px, py) = g.screen_to_page(p, self.laid.paged.pages.len());
        let edge = 6.0 / g.zoom;
        px > x + edge && px < x + w - edge && py > y + edge && py < y + h - edge
    }

    /// Edit the text `which` of object `sel` where it's drawn, caret at
    /// the window point `p` (Word's text box editing). `false` when it
    /// can't be (SmartArt, turned text): the text popover edits it
    /// instead.
    fn text_in_place(
        &mut self,
        page: usize,
        sel: FloatSel,
        which: Option<usize>,
        p: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(story) = self.float_story() else {
            return false;
        };
        let Some(f) = self.doc.float_at(sel.para, sel.index) else {
            return false;
        };
        if matches!(f.kind, docs_engine::FloatKind::SmartArt { .. }) {
            return false;
        }
        let turned = match which {
            None => docs_pdf::floats::text_turn(f) != 0.0,
            Some(k) => self.laid.floats.get(page).is_some_and(|items| {
                items
                    .iter()
                    .filter(|it| {
                        self.selectable(it) && it.para == sel.para && it.index == sel.index
                    })
                    .flat_map(|it| &it.boxes)
                    .any(|b| b.which == k && b.turn != 0.0)
            }),
        };
        if turned {
            return false;
        }
        let b = docs_engine::BoxRef {
            story,
            para: sel.para,
            index: sel.index,
            which,
        };
        let (_, x, y) = self
            .geometry()
            .screen_to_page(p, self.laid.paged.pages.len());
        self.float_sel = None;
        self.float_drag = None;
        self.edit_text_box_in_place(b, page, Some((x, y)), window, cx)
    }

    /// The text a double click at `p` on object `sel` edits: its own text
    /// box (`Some(None)`), the text of the group's shape under the
    /// pointer (or nearest to it), or nothing.
    fn text_target(&self, page: usize, sel: FloatSel, p: Point<Pixels>) -> Option<Option<usize>> {
        let f = self.doc.float_at(sel.para, sel.index)?;
        if f.text.is_some() {
            return Some(None);
        }
        let smartart = matches!(f.kind, docs_engine::FloatKind::SmartArt { .. });
        if f.boxes.is_empty() && !smartart {
            return None;
        }
        let (_, x, y) = self
            .geometry()
            .screen_to_page(p, self.laid.paged.pages.len());
        let it = self
            .laid
            .floats
            .get(page)?
            .iter()
            .find(|it| self.selectable(it) && it.para == sel.para && it.index == sel.index)?;
        let dist = |r: [f32; 4]| {
            let dx = (r[0] - x).max(x - r[0] - r[2]).max(0.0);
            let dy = (r[1] - y).max(y - r[1] - r[3]).max(0.0);
            dx * dx + dy * dy
        };
        it.boxes
            .iter()
            .min_by(|a, b| dist(a.rect).total_cmp(&dist(b.rect)))
            .map(|b| Some(b.which))
    }

    fn start_float_drag(&mut self, page: usize, grab: Grab, rect: [f32; 4], p: Point<Pixels>) {
        let g = self.geometry();
        let (_, x, y) = g.screen_to_page(p, self.laid.paged.pages.len());
        let keep_ratio = self.float_sel.is_some_and(|s| {
            self.doc
                .float_at(s.para, s.index)
                .is_some_and(|f| matches!(f.kind, docs_engine::FloatKind::Picture { .. }))
        });
        self.float_drag = Some(FloatDrag {
            page,
            grab,
            start: (x, y),
            orig: rect,
            cur: rect,
            moved: false,
            keep_ratio,
        });
    }

    /// Pointer moves during a float drag. `true` when one is going on.
    pub(crate) fn float_mouse_move(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) -> bool {
        let Some(mut d) = self.float_drag else {
            return false;
        };
        if ev.pressed_button != Some(MouseButton::Left) {
            self.float_drag = None;
            cx.notify();
            return true;
        }
        let g = self.geometry();
        let doc_x = f32::from(ev.position.x - g.origin.x) / g.zoom + g.scroll.x;
        let doc_y = f32::from(ev.position.y - g.origin.y) / g.zoom + g.scroll.y;
        // Page coordinates of the drag's own page, even off its edges.
        let (x, y) = (doc_x - g.page_x(d.page), doc_y - g.page_top(d.page));
        d.update((x, y), g.page_size(d.page));
        self.float_drag = Some(d);
        cx.notify();
        true
    }

    /// The drag ends: store the new place and size (one undo step).
    pub(crate) fn float_mouse_up(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if let Some(edit) = self.text_box_edit.as_mut()
            && std::mem::take(&mut edit.focus_on_up)
            && edit.locked.is_none()
        {
            let input = edit.input.clone();
            input.update(cx, |s, cx| s.focus(window, cx));
        }
        let Some(d) = self.float_drag.take() else {
            return false;
        };
        let Some(s) = self.float_sel else {
            return true;
        };
        if d.moved && d.cur != d.orig {
            self.place_float(s, d.page, d.cur, window, cx);
        }
        cx.notify();
        true
    }

    /// Put the object at `rect` on `page`: positions from the column and
    /// the anchor paragraph, as Word stores a dragged object.
    fn place_float(
        &mut self,
        s: FloatSel,
        page: usize,
        rect: [f32; 4],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.doc.float_at(s.para, s.index) else {
            return;
        };
        let paged = &self.laid.paged;
        let setup = paged.page_setup(page);
        let para_top = paged
            .placed_line(s.para, 0)
            .map(|(p, i)| paged.pages[p].lines[i].top)
            .unwrap_or(setup.margin_top);
        let emu = |v: f32| (v * EMU_PER_PX).round() as i64;
        let mut at = f.at;
        if self.part.is_some() {
            // A header's or footer's object: from the page's corner (its
            // paragraph isn't in the body's layout).
            at.h = (RelH::Page, Place::Offset(emu(rect[0])));
            at.v = (RelV::Page, Place::Offset(emu(rect[1])));
        } else {
            at.h = (
                RelH::Column,
                Place::Offset(emu(rect[0] - setup.margin_left)),
            );
            at.v = (RelV::Paragraph, Place::Offset(emu(rect[1] - para_top)));
        }
        at.cx = emu(rect[2]);
        at.cy = emu(rect[3]);
        // A SmartArt Kantor lays out is laid out again for its new size.
        let resized = (at.cx, at.cy) != (f.at.cx, f.at.cy);
        let relaid = resized
            .then(|| {
                let mut sized = f.clone();
                sized.at = at;
                docs_pdf::smartart::relaid(&self.doc, &sized, (at.cx, at.cy))
            })
            .flatten();
        let done = match relaid {
            Some(parts) => self
                .doc
                .set_float_placement_parts(s.para, s.index, at, parts),
            None => self.doc.set_float_placement(s.para, s.index, at),
        };
        if done {
            self.float_edited(window, cx);
        }
    }

    /// After a change to a floating object: lay out again, keep the
    /// selection.
    fn float_edited(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.sel;
        self.edited(sel, window, cx);
        self.reveal_caret.set(false);
    }

    /// Delete the selected floating object (Delete / Backspace).
    pub(crate) fn delete_selected_float(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(s) = self.float_sel.take() else {
            return false;
        };
        if self.doc.delete_float(s.para, s.index) {
            let sel = Selection::caret(Position::new(s.para, s.index));
            self.edited(sel, window, cx);
        }
        true
    }

    /// The object Layout Options applies to: the selected floating object,
    /// or an inline picture selected on its own.
    pub(crate) fn layout_target(&self) -> Option<FloatSel> {
        if let Some(s) = self.float_sel {
            return Some(s);
        }
        let (a, b) = self.sel.ordered();
        (a.para == b.para && b.offset == a.offset + OBJECT.len_utf8())
            .then_some(())
            .and_then(|()| match self.doc.paragraph(a.para).object_at(a.offset) {
                Some(Object::Image(_)) => Some(FloatSel {
                    para: a.para,
                    index: a.offset,
                }),
                _ => None,
            })
    }

    /// Word's Layout Options for the selected object.
    pub(crate) fn set_layout_option_doc(
        &mut self,
        o: LayoutOption,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(s) = self.layout_target() else {
            return;
        };
        if self.doc.set_layout_option(s.para, s.index, o) {
            // An inline picture made floating stays selected as an object,
            // a floating one put in line as a character.
            if o == LayoutOption::Inline {
                self.float_sel = None;
                self.sel = Selection {
                    anchor: Position::new(s.para, s.index),
                    head: Position::new(s.para, s.index + OBJECT.len_utf8()),
                };
            } else {
                self.float_sel = Some(s);
                self.sel = Selection::caret(Position::new(s.para, s.index));
            }
            self.float_edited(window, cx);
        }
    }

    /// Word's Change Layout on the selected SmartArt: its items in layout
    /// `to` (one undo step).
    pub(crate) fn smartart_layout_doc(
        &mut self,
        to: slides_engine::SmartArtFamily,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(s) = self.layout_target() else {
            return;
        };
        let Some(f) = self.doc.float_at(s.para, s.index) else {
            return;
        };
        match docs_pdf::smartart::change_layout(&self.doc, f, to) {
            Ok(parts) => {
                if self.doc.set_float_parts(s.para, s.index, parts) {
                    self.float_edited(window, cx);
                    self.status = t("SmartArt layout changed").into();
                }
            }
            Err(why) => {
                debug_assert!(SMARTART_MESSAGES.contains(&why), "{why}");
                self.status = t_dyn(why).into();
            }
        }
        self.focus(window, cx);
        cx.notify();
    }

    /// The Change Layout button of a selected SmartArt: the layouts Kantor
    /// lays out, by category (the names are Slides' catalog keys).
    fn smartart_layout_button(&self, s: FloatSel, cx: &mut Context<Self>) -> Option<Button> {
        let f = self.doc.float_at(s.para, s.index)?;
        if !matches!(f.kind, docs_engine::FloatKind::SmartArt { .. }) {
            return None;
        }
        let current = docs_pdf::smartart::family(&self.doc, f);
        let view = cx.entity().downgrade();
        Some(
            Button::new("docs-smartart-layout")
                .outline()
                .xsmall()
                .icon(Lucide::Shapes)
                .tooltip(t("Change Layout"))
                .dropdown_menu(move |mut menu, _, _| {
                    let cats = [
                        ("list", tk("List")),
                        ("process", tk("Process")),
                        ("cycle", tk("Cycle")),
                        ("hierarchy", tk("Hierarchy")),
                        ("relationship", tk("Relationship")),
                        ("matrix", tk("Matrix")),
                        ("pyramid", tk("Pyramid")),
                    ];
                    for (cat, heading) in cats {
                        menu = menu.item(PopupMenuItem::label(t_dyn(heading)));
                        for (family, _, _, name) in slides_engine::smartart_layout::LAYOUTS
                            .iter()
                            .filter(|l| l.2 == cat)
                        {
                            let v = view.clone();
                            let to = *family;
                            menu = menu.item(
                                PopupMenuItem::new(t_dyn(name))
                                    .checked(current == Some(to))
                                    .on_click(move |_, window, cx| {
                                        let _ = v.update(cx, |this, cx| {
                                            this.smartart_layout_doc(to, window, cx);
                                        });
                                    }),
                            );
                        }
                    }
                    menu
                }),
        )
    }

    /// The Layout Options button beside the selected object.
    pub(crate) fn render_layout_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.layout_target()?;
        let g = self.geometry();
        let rect = match self.float_sel_rect() {
            Some((page, r)) => (page, r),
            None => {
                let o = self.laid.paged.caret(s.para, s.index)?;
                let obj = self
                    .laid
                    .paged
                    .objects(o.page)
                    .into_iter()
                    .find(|x| x.para == s.para && x.index == s.index)?;
                (o.page, obj.rect)
            }
        };
        if self.float_drag.is_some_and(|d| d.moved) {
            return None;
        }
        let (page, [x, y, w, _]) = rect;
        let at = g.page_to_screen(page, x + w, y);
        let origin = self.viewport.get().origin;
        let current = self.doc.layout_option(s.para, s.index);
        let inline_ok = self.doc.can_be_inline(s.para, s.index);
        let view = cx.entity().downgrade();
        let button = Button::new("docs-layout-options")
            .outline()
            .xsmall()
            .icon(Lucide::TextWrap)
            .tooltip(t("Layout Options"))
            .dropdown_menu(move |menu, _, _| {
                let item = |o: LayoutOption| {
                    let v = view.clone();
                    PopupMenuItem::new(option_label(o))
                        .checked(current == Some(o))
                        .disabled(o == LayoutOption::Inline && !inline_ok)
                        .on_click(move |_, window, cx| {
                            let _ = v.update(cx, |this, cx| {
                                this.set_layout_option_doc(o, window, cx);
                                this.focus(window, cx);
                            });
                        })
                };
                let mut menu = menu
                    .item(PopupMenuItem::label(t("Layout Options")))
                    .item(item(LayoutOption::Inline))
                    .separator()
                    .item(PopupMenuItem::label(t("With Text Wrapping")));
                for o in &LayoutOption::ALL[1..] {
                    menu = menu.item(item(*o));
                }
                menu
            });
        let smartart = self.smartart_layout_button(s, cx);
        Some(
            div()
                .absolute()
                .left(at.x - origin.x + SPACE_3)
                .top(at.y - origin.y)
                .flex()
                .flex_col()
                .gap(SPACE_2)
                .child(button)
                .children(smartart)
                .into_any_element(),
        )
    }

    /// Open the text box editor on `sel` at `at` (window coordinates).
    fn open_text_box(
        &mut self,
        sel: FloatSel,
        which: Option<usize>,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.doc.float_at(sel.para, sel.index) else {
            return;
        };
        let owner = TextOwner::of(f, which);
        let (text, locked) = match (&f.kind, which) {
            (docs_engine::FloatKind::SmartArt { .. }, Some(k)) => {
                let Some((text, locked)) = docs_pdf::smartart::shape_text(&self.doc, f, k) else {
                    return;
                };
                (text, locked.map(|l| t_dyn(l).to_owned()))
            }
            _ => (
                f.text_box_text_in(which).unwrap_or_default(),
                f.text_locked_in(which).map(str::to_owned),
            ),
        };
        let input = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder(owner.placeholder());
            s.set_value(text, window, cx);
            s
        });
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = ev
            {
                this.apply_text_box(window, cx);
                this.focus(window, cx);
            }
        });
        if locked.is_none() {
            input.update(cx, |s, cx| s.focus(window, cx));
        }
        self.text_box_edit = Some(TextBoxEdit {
            sel,
            which,
            at,
            input,
            locked,
            owner,
            focus_on_up: true,
            _sub: sub,
        });
    }

    /// Write the edited text box's text (one undo step) and close.
    pub(crate) fn apply_text_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.text_box_edit.take() else {
            return;
        };
        if edit.locked.is_none() {
            let value = edit.input.read(cx).value().to_string();
            let text = value.trim_end_matches(['\n', '\r']);
            let (para, index) = (edit.sel.para, edit.sel.index);
            let smartart = self
                .doc
                .float_at(para, index)
                .filter(|f| matches!(f.kind, docs_engine::FloatKind::SmartArt { .. }));
            match (smartart, edit.which) {
                (Some(f), Some(k)) => {
                    match docs_pdf::smartart::set_shape_text(&self.doc, f, k, text) {
                        Ok(Some(parts)) => {
                            if self.doc.set_float_parts(para, index, parts) {
                                self.float_edited(window, cx);
                                self.status = edit.owner.updated().into();
                            }
                        }
                        Ok(None) => {}
                        Err(why) => {
                            debug_assert!(
                                SMARTART_MESSAGES.contains(&why)
                                    || why.starts_with("This SmartArt text")
                            );
                            self.status = t_dyn(why).into();
                        }
                    }
                }
                _ => {
                    if self.doc.set_text_box_text_in(para, index, edit.which, text) {
                        self.float_edited(window, cx);
                        self.status = edit.owner.updated().into();
                    }
                }
            }
        }
        cx.notify();
    }

    /// Add an item before or after the one the SmartArt shape in the
    /// text popover shows, or delete it (one undo step); the popover
    /// closes without applying its text.
    pub(crate) fn smartart_item_doc(
        &mut self,
        op: ItemEdit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(edit) = self.text_box_edit.take() else {
            return;
        };
        let (para, index) = (edit.sel.para, edit.sel.index);
        let (Some(f), Some(k)) = (self.doc.float_at(para, index), edit.which) else {
            return;
        };
        let r = match op {
            ItemEdit::AddBefore => docs_pdf::smartart::add_item(&self.doc, f, k, false),
            ItemEdit::AddAfter => docs_pdf::smartart::add_item(&self.doc, f, k, true),
            ItemEdit::Delete => docs_pdf::smartart::remove_item(&self.doc, f, k),
        };
        match r {
            Ok(parts) => {
                if self.doc.set_float_parts(para, index, parts) {
                    self.float_edited(window, cx);
                    self.status = match op {
                        ItemEdit::Delete => t("SmartArt shape deleted"),
                        _ => t("SmartArt shape added"),
                    }
                    .into();
                }
            }
            Err(why) => {
                debug_assert!(SMARTART_MESSAGES.contains(&why), "{why}");
                self.status = t_dyn(why).into();
            }
        }
        self.focus(window, cx);
        cx.notify();
    }

    pub(crate) fn render_text_box_menu(
        &self,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let edit = self.text_box_edit.as_ref()?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let body = match &edit.locked {
            Some(why) => div()
                .p(SPACE_3)
                .rounded(RADIUS_MD)
                .bg(t.surface)
                .text_color(t.fg_muted)
                .child(if why == LOCKED {
                    t_dyn(why).to_owned()
                } else {
                    why.clone()
                })
                .into_any_element(),
            None => Textarea::new(&edit.input).small().into_any_element(),
        };
        let done = div()
            .id("docs-text-box-apply")
            .flex_none()
            .h(CONTROL_SM)
            .px(SPACE_5)
            .flex()
            .items_center()
            .rounded(RADIUS_MD)
            .cursor_pointer()
            .bg(t.accent)
            .text_color(t.accent_fg)
            .child(i18n::t("Done"))
            .on_click(move |_, window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.apply_text_box(window, cx);
                    this.focus(window, cx);
                });
            });
        let buttons = self.item_buttons(edit, cx);
        let panel = v_flex()
            .id("docs-text-box-menu")
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
                this.apply_text_box(window, cx);
            }))
            .child(section_label(edit.owner.title(), cx))
            .child(body)
            .when(!buttons.is_empty(), |p| {
                p.child(h_flex().flex_wrap().gap(SPACE_2).children(buttons))
            })
            .child(h_flex().child(div().flex_1()).child(done));
        Some(deferred(
            anchored()
                .position(edit.at)
                .anchor(Anchor::TopLeft)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel),
        ))
    }

    /// The SmartArt item buttons of the text popover (Word's Add Shape
    /// Before / After and Delete), for layouts Kantor lays out.
    fn item_buttons(&self, edit: &TextBoxEdit, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let laid_out = edit.which.is_some()
            && edit.owner == TextOwner::SmartArt
            && self
                .doc
                .float_at(edit.sel.para, edit.sel.index)
                .is_some_and(|f| docs_pdf::smartart::can_relayout(&self.doc, f));
        if !laid_out {
            return Vec::new();
        }
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        [
            (
                "docs-smartart-add-before",
                tk("Add Shape Before"),
                ItemEdit::AddBefore,
            ),
            (
                "docs-smartart-add-after",
                tk("Add Shape After"),
                ItemEdit::AddAfter,
            ),
            ("docs-smartart-delete", tk("Delete Shape"), ItemEdit::Delete),
        ]
        .into_iter()
        .map(|(id, label, op)| {
            let v = view.clone();
            div()
                .id(id)
                .flex_none()
                .h(CONTROL_SM)
                .px(SPACE_2)
                .flex()
                .items_center()
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .border_1()
                .border_color(t.border)
                .hover(|s| s.bg(t.hover))
                .child(t_dyn(label).to_owned())
                .on_click(move |_, window, cx| {
                    let _ = v.update(cx, |this, cx| this.smartart_item_doc(op, window, cx));
                })
                .into_any_element()
        })
        .collect()
    }

    /// Decoded pictures of the floating objects about to be painted, by
    /// (header or footer story + 1, or 0; [`picture_key`]): turned and
    /// flipped ones turned on the CPU (GPUI paints pictures upright).
    pub(crate) fn visible_float_images(
        &mut self,
        paint: &[FloatPaint],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> HashMap<(usize, String), Arc<RenderImage>> {
        let mut out = HashMap::new();
        for fp in paint {
            let owner = match fp.story {
                Some(s) => self.doc.stories().get(s).map_or(&self.doc, |st| &st.doc),
                None => &self.doc,
            };
            for prim in &fp.prims {
                if let FloatPrim::Blurred(b) = prim {
                    // A blurred shadow: its own picture.
                    let key = format!("blur:{}", b.key);
                    // Moving a shape makes new ones: keep only a few.
                    if !self.images.contains_key(&key)
                        && self
                            .images
                            .keys()
                            .filter(|k| k.starts_with("blur:"))
                            .count()
                            >= 32
                    {
                        self.images.retain(|k, _| !k.starts_with("blur:"));
                    }
                    let img = self
                        .images
                        .entry(key.clone())
                        .or_insert_with(|| {
                            Arc::new(gpui_kit::Image::from_bytes(
                                ImageFormat::Png,
                                b.png.to_vec(),
                            ))
                        })
                        .clone();
                    if let Some(data) = img.use_render_image(window, cx) {
                        out.insert((fp.story.map_or(0, |s| s + 1), key), data);
                    }
                    continue;
                }
                let FloatPrim::Picture {
                    rel_id,
                    image,
                    clip,
                    rot,
                    flip,
                } = prim
                else {
                    continue;
                };
                let base_key = format!("float{}:{}", fp.story.map_or(0, |s| s + 1), rel_id);
                let img = match self.images.get(&base_key) {
                    Some(i) => i.clone(),
                    None => {
                        let Some(bytes) = owner.image_bytes(rel_id) else {
                            continue;
                        };
                        let format = match docs_engine::ImageFormat::sniff(&bytes) {
                            docs_engine::ImageFormat::Png => ImageFormat::Png,
                            docs_engine::ImageFormat::Jpeg => ImageFormat::Jpeg,
                            docs_engine::ImageFormat::Other => continue,
                        };
                        let i = Arc::new(gpui_kit::Image::from_bytes(format, bytes.to_vec()));
                        self.images.insert(base_key, i.clone());
                        i
                    }
                };
                let Some(data) = img.use_render_image(window, cx) else {
                    continue;
                };
                let key = picture_key(rel_id, *rot, *flip, *image, *clip);
                let story = fp.story.map_or(0, |s| s + 1);
                if key == *rel_id {
                    out.insert((story, key), data);
                    continue;
                }
                let turned = match self.turned_images.get(&key) {
                    Some(t) => t.clone(),
                    None => {
                        let Some(t) = turn_image(&data, *rot, *flip, *image, *clip) else {
                            continue;
                        };
                        if self.turned_images.len() >= 24 {
                            for (_, old) in self.turned_images.drain() {
                                window.drop_image(old).ok();
                            }
                        }
                        self.turned_images.insert(key.clone(), t.clone());
                        t
                    }
                };
                out.insert((story, key), turned);
            }
        }
        out
    }
}

/// How a floating picture is looked up: its relationship id, plus its
/// turn, flips, crop and shape when it's drawn turned.
pub(crate) fn picture_key(
    rel_id: &str,
    rot: f32,
    flip: (bool, bool),
    image: [f32; 4],
    clip: [f32; 4],
) -> String {
    let deg = (rot.round() as i32).rem_euclid(360);
    if deg == 0 && !flip.0 && !flip.1 {
        return rel_id.to_owned();
    }
    let crop = crop_of(image, clip).map(|c| (c * 10_000.0) as i32);
    let aspect = (clip[2] / clip[3].max(1e-3) * 1000.0) as i64;
    format!(
        "{rel_id}@{deg}:{}{}:{crop:?}:{aspect}",
        flip.0 as u8, flip.1 as u8
    )
}

/// The crop (left, top, right, bottom fractions of the picture) that shows
/// `clip` of a picture drawn into `image`.
fn crop_of(image: [f32; 4], clip: [f32; 4]) -> [f32; 4] {
    let (iw, ih) = (image[2].max(1e-3), image[3].max(1e-3));
    [
        (clip[0] - image[0]) / iw,
        (clip[1] - image[1]) / ih,
        (image[0] + iw - clip[0] - clip[2]) / iw,
        (image[1] + ih - clip[1] - clip[3]) / ih,
    ]
}

/// The bounding box of `clip` turned `rot` degrees about its centre.
pub(crate) fn turned_box(clip: [f32; 4], rot: f32) -> [f32; 4] {
    let (s, c) = rot.to_radians().sin_cos();
    let [x, y, w, h] = clip;
    let bw = w * c.abs() + h * s.abs();
    let bh = w * s.abs() + h * c.abs();
    [x + w / 2.0 - bw / 2.0, y + h / 2.0 - bh / 2.0, bw, bh]
}

/// Most pixels on the longer side of a turned picture.
const TURNED_MAX: f32 = 2048.0;

/// A picture's shown part (`clip` of it drawn into `image`) flipped, then
/// turned `rot` degrees clockwise, into the box around the turned one
/// (nearest pixel; the corners stay transparent).
fn turn_image(
    img: &RenderImage,
    rot: f32,
    (flip_h, flip_v): (bool, bool),
    image: [f32; 4],
    clip: [f32; 4],
) -> Option<Arc<RenderImage>> {
    let size = img.size(0);
    let (sw, sh) = (size.width.0.max(1) as u32, size.height.0.max(1) as u32);
    let src = img
        .as_bytes(0)
        .filter(|b| b.len() >= (sw * sh * 4) as usize)?;
    let [cl, ct, cr, cb] = crop_of(image, clip);
    let (kw, kh) = (1.0 - cl - cr, 1.0 - ct - cb);
    let (w0, h0) = (clip[2].max(1e-3), clip[3].max(1e-3));
    // As many pixels as the picture has, at most TURNED_MAX on a side.
    let k = (TURNED_MAX.min((sw as f32 * kw).max(sh as f32 * kh)) / w0.max(h0)).max(1e-3);
    let (w, h) = ((w0 * k).max(1.0), (h0 * k).max(1.0));
    let (sin, cos) = rot.to_radians().sin_cos();
    let bw = (w * cos.abs() + h * sin.abs() - 1e-3).ceil().max(1.0) as u32;
    let bh = (w * sin.abs() + h * cos.abs() - 1e-3).ceil().max(1.0) as u32;
    let mut out = vec![0u8; (bw * bh * 4) as usize];
    for j in 0..bh {
        for i in 0..bw {
            let u = i as f32 + 0.5 - bw as f32 / 2.0;
            let v = j as f32 + 0.5 - bh as f32 / 2.0;
            let mut lx = u * cos + v * sin + w / 2.0;
            let mut ly = -u * sin + v * cos + h / 2.0;
            if lx < 0.0 || ly < 0.0 || lx >= w || ly >= h {
                continue;
            }
            if flip_h {
                lx = w - lx;
            }
            if flip_v {
                ly = h - ly;
            }
            let fx = cl + lx / w * kw;
            let fy = ct + ly / h * kh;
            if !(0.0..1.0).contains(&fx) || !(0.0..1.0).contains(&fy) {
                continue;
            }
            let sx = ((fx * sw as f32) as u32).min(sw - 1);
            let sy = ((fy * sh as f32) as u32).min(sh - 1);
            let s = ((sy * sw + sx) * 4) as usize;
            let d = ((j * bw + i) * 4) as usize;
            out[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    let buf = image::RgbaImage::from_raw(bw, bh, out)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

/// What the visible pages' floating objects draw, for the paint pass.
pub(crate) struct FloatPaint {
    pub page: usize,
    pub behind: bool,
    pub prims: Vec<FloatPrim>,
    pub story: Option<usize>,
    pub text: Option<(Arc<docs_pdf::StoryLayout>, f32, f32)>,
    /// A chart's upright labels: layout and where its (0, 0) goes.
    pub labels: Vec<(Arc<docs_pdf::StoryLayout>, f32, f32)>,
    /// Turned text (a turned text box's, a chart's turned labels), drawn
    /// as rasters.
    pub turned: Vec<std::rc::Rc<crate::turned_text::TurnedText>>,
}

/// Turned text boxes' text, kept while their layout stays the same.
pub(crate) type TurnedCache =
    HashMap<(usize, usize, u32, u32, u32), std::rc::Rc<crate::turned_text::TurnedText>>;

/// A turned text box's text as a raster source: its layout's (0, 0) at
/// `(x, y)`, turned `deg` about the centre of `rect` (page px).
fn turned_text(
    l: &docs_pdf::StoryLayout,
    (x, y): (f32, f32),
    rect: [f32; 4],
    deg: f32,
) -> Option<crate::turned_text::TurnedText> {
    use slides_scene::{Affine, Text, TextRun, p};
    // The text runs (the text the clusters index, for the font bridge).
    let runs: Vec<TextRun> = l
        .paged
        .glyph_runs(0)
        .into_iter()
        .filter(|r| !r.glyphs.is_empty())
        .map(|run| {
            let text: Arc<str> = match run.source {
                docs_layout::RunSource::Text => {
                    Arc::from(l.paras.get(run.para).map_or("", |p| p.text.as_str()))
                }
                _ => Arc::from(l.paged.run_text(&run, "")),
            };
            TextRun { run, text }
        })
        .collect();
    let (cx, cy) = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
    let t = Affine::translate(-cx, -cy)
        .then(Affine::rotate(deg.to_radians()))
        .then(Affine::translate(cx, cy));
    crate::turned_text::TurnedText::new(
        &Text {
            origin: p(x, y),
            runs,
        },
        &t,
    )
}

/// The floating objects of the visible pages, back to front per page.
pub(crate) fn float_paint(
    doc: &docs_engine::Document,
    laid: &crate::Laid,
    painter: &docs_pdf::floats::Painter,
    pages: std::ops::Range<usize>,
    cache: &mut TurnedCache,
) -> Vec<FloatPaint> {
    let mut out = Vec::new();
    let mut used = Vec::new();
    for page in pages {
        for it in laid.floats.get(page).into_iter().flatten() {
            let Some(f) = it.float(doc) else { continue };
            let deg = docs_pdf::floats::text_turn(f);
            let mut turned = Vec::new();
            let mut get =
                |l: &Arc<docs_pdf::StoryLayout>, at: (f32, f32), rect: [f32; 4], deg: f32| {
                    let key = (
                        Arc::as_ptr(l) as usize,
                        page,
                        deg.to_bits(),
                        rect[0].to_bits(),
                        rect[1].to_bits(),
                    );
                    used.push(key);
                    let t = match cache.get(&key) {
                        Some(t) => Some(t.clone()),
                        None => turned_text(l, at, rect, deg).map(|t| {
                            let t = std::rc::Rc::new(t);
                            cache.insert(key, t.clone());
                            t
                        }),
                    };
                    turned.extend(t);
                };
            if let Some((l, x, y)) = &it.text
                && deg != 0.0
            {
                get(l, (*x, *y), it.rect, deg);
            }
            let mut labels = Vec::new();
            for b in &it.boxes {
                if b.turn != 0.0 {
                    get(&b.layout, (b.x, b.y), b.rect, b.turn);
                } else {
                    labels.push((b.layout.clone(), b.x, b.y));
                }
            }
            for l in it.chart.iter().flat_map(|c| &c.labels) {
                if l.rot != 0.0 {
                    get(&l.layout, (l.x, l.y), l.rect, l.rot);
                } else {
                    labels.push((l.layout.clone(), l.x, l.y));
                }
            }
            out.push(FloatPaint {
                page,
                behind: it.behind,
                prims: match &it.chart {
                    Some(c) => c.prims.clone(),
                    None => painter.prims(f, it.rect),
                },
                story: it.story,
                text: it.text.clone().filter(|_| deg == 0.0),
                labels,
                turned,
            });
        }
    }
    cache.retain(|k, _| used.contains(k));
    out
}

/// Screen point of a page point.
/// A GPUI two-stop linear gradient for a gradient band on the page (box
/// `[x, y, w, h]`, colour along unit vector `u`: `from.1` where the
/// projection is `from.0`, `to.1` at `to.0`). GPUI measures a gradient
/// against the painted path's box in its own way (`gradient_color` in its
/// shaders): this picks the angle and stop percentages that land the stops
/// where asked (as Slides' bands). Zoom and scroll scale the box and the
/// projections alike, so page coordinates give the same percentages.
fn band(
    [x, y, w, h]: [f32; 4],
    u: [f32; 2],
    (s0, c0): (f32, docs_layout::Rgba),
    (s1, c1): (f32, docs_layout::Rgba),
) -> Background {
    let (w, h) = (w.max(1e-3), h.max(1e-3));
    let (kx, ky) = if w > h { (1.0, h / w) } else { (w / h, 1.0) };
    let r = (u[1] / ky).atan2(u[0] / kx);
    let dir = (r.cos() * kx, r.sin() * ky);
    let size = if dir.0.abs() > dir.1.abs() { w } else { h };
    let sc = (x + w / 2.0) * u[0] + (y + h / 2.0) * u[1];
    let pct = |s: f32| (s - sc + size / 2.0) / size;
    linear_gradient(
        r.to_degrees() + 90.0,
        linear_color_stop(layout_color(c0), pct(s0)),
        linear_color_stop(layout_color(c1), pct(s1)),
    )
}

fn sp(g: &Geometry, page: usize, q: [f32; 2]) -> Point<Pixels> {
    g.page_to_screen(page, q[0], q[1])
}

fn build_segs(pb: &mut PathBuilder, g: &Geometry, page: usize, segs: &[Seg]) {
    for s in segs {
        match *s {
            Seg::Move(a) => pb.move_to(sp(g, page, a)),
            Seg::Line(a) => pb.line_to(sp(g, page, a)),
            Seg::Quad(c, a) => pb.curve_to(sp(g, page, a), sp(g, page, c)),
            Seg::Cubic(c1, c2, a) => {
                pb.cubic_bezier_to(sp(g, page, a), sp(g, page, c1), sp(g, page, c2))
            }
            Seg::Close => pb.close(),
        }
    }
}

/// Paints a text box's laid-out text with its layout's (0, 0) at a point.
pub(crate) type PaintText<'a> = dyn FnMut(&mut Window, &docs_pdf::StoryLayout, (f32, f32)) + 'a;

/// Paint the floating objects of `page` that are behind the text
/// (`behind`) or in front of it.
pub(crate) fn paint_floats(
    f: &Frame,
    g: &Geometry,
    page: usize,
    behind: bool,
    window: &mut Window,
    paint_text: &mut PaintText,
) {
    let z = g.zoom;
    for fp in f
        .floats
        .iter()
        .filter(|fp| fp.page == page && fp.behind == behind)
    {
        for prim in &fp.prims {
            match prim {
                FloatPrim::Path(p) => {
                    if let Some(gr) = &p.gradient {
                        for (segs, piece) in docs_pdf::floats::gradient_pieces(&p.segs, gr) {
                            let mut pb = PathBuilder::fill();
                            build_segs(&mut pb, g, page, &segs);
                            let Ok(path) = pb.build() else { continue };
                            match piece {
                                docs_pdf::floats::GradientPiece::Solid(c) => {
                                    window.paint_path(path, layout_color(c))
                                }
                                docs_pdf::floats::GradientPiece::Band {
                                    bbox,
                                    dir,
                                    from,
                                    to,
                                } => window.paint_path(path, band(bbox, dir, from, to)),
                            }
                        }
                    } else if let Some(c) = p.fill {
                        let mut pb = PathBuilder::fill();
                        build_segs(&mut pb, g, page, &p.segs);
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, layout_color(c));
                        }
                    }
                    if let Some((c, w, dash)) = &p.stroke {
                        let mut pb = PathBuilder::stroke(px((w * z).max(1.0)));
                        if let Some(d) = dash {
                            let d: Vec<Pixels> = d.iter().map(|v| px((v * z).max(0.5))).collect();
                            pb = pb.dash_array(&d);
                        }
                        build_segs(&mut pb, g, page, &p.segs);
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, layout_color(*c));
                        }
                    }
                }
                FloatPrim::Picture {
                    rel_id,
                    image,
                    clip,
                    rot,
                    flip,
                } => {
                    let pk = picture_key(rel_id, *rot, *flip, *image, *clip);
                    let turned = pk != *rel_id;
                    let key = (fp.story.map_or(0, |s| s + 1), pk);
                    let rect = |r: [f32; 4]| {
                        Bounds::from_corners(
                            g.page_to_screen(page, r[0], r[1]),
                            g.page_to_screen(page, r[0] + r[2], r[1] + r[3]),
                        )
                    };
                    // A turned picture is its own image of the turned box.
                    let (into, whole) = if turned {
                        let b = turned_box(*clip, *rot);
                        (b, b)
                    } else {
                        (*clip, *image)
                    };
                    match f.float_images.get(&key) {
                        Some(data) => {
                            let _ = window.paint_image(
                                rect(into),
                                rect(whole),
                                Corners::default(),
                                data.clone(),
                                0,
                                false,
                            );
                        }
                        None => {
                            window.paint_quad(fill(rect(*clip), f.t.paper_selection.opacity(0.35)))
                        }
                    }
                }
                FloatPrim::Blurred(b) => {
                    let key = (fp.story.map_or(0, |s| s + 1), format!("blur:{}", b.key));
                    if let Some(data) = f.float_images.get(&key) {
                        let r = Bounds::from_corners(
                            g.page_to_screen(page, b.rect[0], b.rect[1]),
                            g.page_to_screen(page, b.rect[0] + b.rect[2], b.rect[1] + b.rect[3]),
                        );
                        let _ =
                            window.paint_image(r, r, Corners::default(), data.clone(), 0, false);
                    }
                }
                FloatPrim::Placeholder(r, _) => {
                    let b = Bounds::from_corners(
                        g.page_to_screen(page, r[0], r[1]),
                        g.page_to_screen(page, r[0] + r[2], r[1] + r[3]),
                    );
                    window.paint_quad(fill(b, f.t.paper_selection.opacity(0.35)));
                    window.paint_quad(outline(b, f.t.paper_ink.opacity(0.35), BorderStyle::Dashed));
                }
            }
        }
        // Pictures in text boxes (decoded with the headers' ones).
        let pictures = |window: &mut Window, l: &docs_pdf::StoryLayout, (x, y): (f32, f32)| {
            for o in l.paged.objects(0) {
                if let Some(data) = f.hf_images.get(&(l.key(), o.para, o.index)) {
                    let [ox, oy, w, h] = o.rect;
                    let b = Bounds::from_corners(
                        g.page_to_screen(page, x + ox, y + oy),
                        g.page_to_screen(page, x + ox + w, y + oy + h),
                    );
                    let _ = window.paint_image(b, b, Corners::default(), data.clone(), 0, false);
                }
            }
        };
        if let Some((l, x, y)) = &fp.text {
            paint_text(window, l, (*x, *y));
            pictures(window, l, (*x, *y));
        }
        for (l, x, y) in &fp.labels {
            paint_text(window, l, (*x, *y));
            pictures(window, l, (*x, *y));
        }
        for tt in &fp.turned {
            let Some(img) = tt.image(z * window.scale_factor()) else {
                continue;
            };
            let [x, y, w, h] = tt.bounds;
            let b = Bounds::from_corners(
                g.page_to_screen(page, x, y),
                g.page_to_screen(page, x + w, y + h),
            );
            let _ = window.paint_image(b, b, Corners::default(), img, 0, false);
        }
    }
}

/// The selected object's outline and handles, or the box being dragged.
pub(crate) fn paint_float_selection(f: &Frame, g: &Geometry, page: usize, window: &mut Window) {
    let Some((sel_page, rect)) = f.float_sel else {
        return;
    };
    if sel_page != page {
        return;
    }
    let screen = |r: [f32; 4]| {
        Bounds::from_corners(
            g.page_to_screen(page, r[0], r[1]),
            g.page_to_screen(page, r[0] + r[2], r[1] + r[3]),
        )
    };
    if let Some(d) = f.float_drag.filter(|d| d.moved && d.page == page) {
        window.paint_quad(outline(screen(d.cur), f.t.accent, BorderStyle::Dashed));
    }
    window.paint_quad(outline(screen(rect), f.t.accent, BorderStyle::Solid));
    let half = HANDLE / 2.0;
    for (hx, hy) in handles(rect) {
        let c = g.page_to_screen(page, hx, hy);
        let b = Bounds::new(
            point(c.x - px(half), c.y - px(half)),
            size(px(HANDLE), px(HANDLE)),
        );
        window.paint_quad(
            fill(b, f.t.paper)
                .border_widths(px(1.))
                .border_color(f.t.accent),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text popover is titled after what it edits.
    #[std::prelude::v1::test]
    fn text_popovers_name_what_they_edit() {
        let corpus = |n: &str| {
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/corpus/docx")
                .join(n)
        };
        let owners = |n: &str| {
            let d = docs_engine::Document::open(&corpus(n)).unwrap();
            let mut out = Vec::new();
            for p in d.paragraphs() {
                for (_, o) in p.objects() {
                    if let Some(f) = o.float() {
                        if f.text.is_some() {
                            out.push(TextOwner::of(f, None));
                        }
                        if !f.boxes.is_empty()
                            || matches!(f.kind, docs_engine::FloatKind::SmartArt { .. })
                        {
                            out.push(TextOwner::of(f, Some(0)));
                        }
                    }
                }
            }
            out
        };
        let d = owners("diagrams.docx");
        assert!(!d.is_empty());
        assert!(d.contains(&TextOwner::SmartArt), "{d:?}");
        assert!(d.contains(&TextOwner::Shape), "{d:?}");
        // A rounded rectangle with text is a shape; Word's text boxes say so.
        let d = docs_engine::Document::open(&corpus("floats.docx")).unwrap();
        let f = d
            .paragraphs()
            .iter()
            .flat_map(|p| p.objects())
            .filter_map(|(_, o)| o.float())
            .find(|f| f.text.is_some())
            .unwrap();
        assert_eq!(TextOwner::of(f, None), TextOwner::Shape);
        let mut b = f.clone();
        b.xml = f
            .xml
            .replacen("<wps:cNvSpPr/>", "<wps:cNvSpPr txBox=\"1\"/>", 1)
            .into();
        assert_eq!(TextOwner::of(&b, None), TextOwner::TextBox);
    }

    fn drag(grab: Grab, keep_ratio: bool) -> FloatDrag {
        FloatDrag {
            page: 0,
            grab,
            start: (100.0, 100.0),
            orig: [50.0, 60.0, 200.0, 100.0],
            cur: [50.0, 60.0, 200.0, 100.0],
            moved: false,
            keep_ratio,
        }
    }

    #[std::prelude::v1::test]
    fn moves_and_resizes() {
        let page = (794.0, 1123.0);
        let mut d = drag(Grab::Move, false);
        d.update((130.0, 90.0), page);
        assert!(d.moved);
        assert_eq!(d.cur, [80.0, 50.0, 200.0, 100.0]);
        // The right edge: wider only.
        let mut d = drag(Grab::Handle(3), false);
        d.update((150.0, 300.0), page);
        assert_eq!(d.cur, [50.0, 60.0, 250.0, 100.0]);
        // The top left corner of a picture keeps its proportions, the
        // bottom right corner stays.
        let mut d = drag(Grab::Handle(0), true);
        d.update((0.0, 100.0), page);
        assert_eq!(d.cur, [-50.0, 10.0, 300.0, 150.0]);
        // Never smaller than the minimum.
        let mut d = drag(Grab::Handle(4), false);
        d.update((-500.0, -500.0), page);
        assert_eq!((d.cur[2], d.cur[3]), (MIN_SIZE, MIN_SIZE));
    }
}
