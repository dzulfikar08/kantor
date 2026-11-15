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
use kantor_ui::{metrics::*, tokens};

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
    at: Point<Pixels>,
    input: Entity<TextareaState>,
    locked: Option<String>,
    /// Focus the text when the double click's button goes up (the page
    /// takes the focus back on mouse down).
    focus_on_up: bool,
    _sub: Subscription,
}

const LOCKED: &str = tk(
    "This text box has pictures, tables or other objects; Kantor shows it and keeps it as it is, but can't edit it yet",
);

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
    /// The selected floating object's box: page and `[x, y, w, h]`.
    pub(crate) fn float_sel_rect(&self) -> Option<(usize, [f32; 4])> {
        let s = self.float_sel?;
        self.laid
            .floats
            .iter()
            .enumerate()
            .find_map(|(page, items)| {
                items
                    .iter()
                    .find(|it| it.story.is_none() && it.para == s.para && it.index == s.index)
                    .map(|it| (page, it.rect))
            })
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
            .filter(|it| it.story.is_none() && inside(it.rect))
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
        self.float_sel = Some(sel);
        self.text_box_edit = None;
        // The caret goes to the anchor, as in Word (Delete, Layout Options
        // and undo work on the object).
        self.sel = Selection::caret(Position::new(sel.para, sel.index));
        self.dragging = false;
        if ev.click_count == 2
            && self
                .doc
                .float_at(sel.para, sel.index)
                .is_some_and(|f| f.text.is_some())
        {
            let at = point(ev.position.x + SPACE_3, ev.position.y + SPACE_3);
            self.open_text_box(sel, at, window, cx);
        } else {
            self.start_float_drag(page, Grab::Move, rect, ev.position);
        }
        cx.notify();
        true
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
        at.h = (
            RelH::Column,
            Place::Offset(emu(rect[0] - setup.margin_left)),
        );
        at.v = (RelV::Paragraph, Place::Offset(emu(rect[1] - para_top)));
        at.cx = emu(rect[2]);
        at.cy = emu(rect[3]);
        if self.doc.set_float_placement(s.para, s.index, at) {
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
        Some(
            div()
                .absolute()
                .left(at.x - origin.x + SPACE_3)
                .top(at.y - origin.y)
                .child(button)
                .into_any_element(),
        )
    }

    /// Open the text box editor on `sel` at `at` (window coordinates).
    fn open_text_box(
        &mut self,
        sel: FloatSel,
        at: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(f) = self.doc.float_at(sel.para, sel.index) else {
            return;
        };
        let text = f.text_box_text().unwrap_or_default();
        let locked = f.text_locked().map(str::to_owned);
        let input = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .auto_grow(2, 8)
                .placeholder(t("Write the text box's text"));
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
            at,
            input,
            locked,
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
            if self
                .doc
                .set_text_box_text(edit.sel.para, edit.sel.index, text)
            {
                self.float_edited(window, cx);
                self.status = t("Text box updated").into();
            }
        }
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
            .child(div().text_color(t.fg_muted).child(i18n::t("Text box")))
            .child(body)
            .child(h_flex().child(div().flex_1()).child(done));
        Some(deferred(
            anchored()
                .position(edit.at)
                .anchor(Anchor::TopLeft)
                .snap_to_window_with_margin(SPACE_4)
                .child(panel),
        ))
    }

    /// Decoded pictures of the floating objects on the visible pages, by
    /// (header or footer story + 1, or 0; relationship id).
    pub(crate) fn visible_float_images(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> HashMap<(usize, String), Arc<RenderImage>> {
        let mut out = HashMap::new();
        let laid = self.laid.clone();
        let g = self.geometry();
        for page in g.visible_pages(laid.paged.pages.len()) {
            for it in laid.floats.get(page).into_iter().flatten() {
                let Some(f) = it.float(&self.doc) else {
                    continue;
                };
                let docs_engine::FloatKind::Picture { rel_id, .. } = &f.kind else {
                    continue;
                };
                let key = (it.story.map_or(0, |s| s + 1), rel_id.clone());
                let cache_key = format!("float{}:{}", key.0, key.1);
                let image = match self.images.get(&cache_key) {
                    Some(i) => i.clone(),
                    None => {
                        let Some(bytes) = it.doc(&self.doc).image_bytes(rel_id) else {
                            continue;
                        };
                        let format = match docs_engine::ImageFormat::sniff(&bytes) {
                            docs_engine::ImageFormat::Png => ImageFormat::Png,
                            docs_engine::ImageFormat::Jpeg => ImageFormat::Jpeg,
                            docs_engine::ImageFormat::Other => continue,
                        };
                        let i = Arc::new(gpui_kit::Image::from_bytes(format, bytes.to_vec()));
                        self.images.insert(cache_key, i.clone());
                        i
                    }
                };
                if let Some(data) = image.use_render_image(window, cx) {
                    out.insert(key, data);
                }
            }
        }
        out
    }
}

/// What the visible pages' floating objects draw, for the paint pass.
pub(crate) struct FloatPaint {
    pub page: usize,
    pub behind: bool,
    pub prims: Vec<FloatPrim>,
    pub story: Option<usize>,
    pub text: Option<(Arc<docs_pdf::StoryLayout>, f32, f32)>,
}

/// The floating objects of the visible pages, back to front per page.
pub(crate) fn float_paint(
    doc: &docs_engine::Document,
    laid: &crate::Laid,
    painter: &docs_pdf::floats::Painter,
    pages: std::ops::Range<usize>,
) -> Vec<FloatPaint> {
    let mut out = Vec::new();
    for page in pages {
        for it in laid.floats.get(page).into_iter().flatten() {
            let Some(f) = it.float(doc) else { continue };
            out.push(FloatPaint {
                page,
                behind: it.behind,
                prims: painter.prims(f, it.rect),
                story: it.story,
                text: it.text.clone(),
            });
        }
    }
    out
}

/// Screen point of a page point.
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
                    if let Some(c) = p.fill {
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
                } => {
                    let key = (fp.story.map_or(0, |s| s + 1), rel_id.clone());
                    let rect = |r: [f32; 4]| {
                        Bounds::from_corners(
                            g.page_to_screen(page, r[0], r[1]),
                            g.page_to_screen(page, r[0] + r[2], r[1] + r[3]),
                        )
                    };
                    match f.float_images.get(&key) {
                        Some(data) => {
                            let _ = window.paint_image(
                                rect(*clip),
                                rect(*image),
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
        if let Some((l, x, y)) = &fp.text {
            paint_text(window, l, (*x, *y));
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
