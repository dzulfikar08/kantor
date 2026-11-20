//! Comments, markup, form filling and Fill & Sign on the pages.
//!
//! Like Acrobat, the toolbar has a *Comment* mode (highlight, underline,
//! strikethrough, sticky notes, text boxes, the pen, shapes, image stamps)
//! and a *Fill & Sign* mode (text, date, check and cross marks,
//! signatures, stamps). Everything is a standard PDF annotation written by
//! `kantor_pdf::annot`; form fields are filled with `kantor_pdf::form`.
//!
//! A page whose annotations or fields were changed is drawn with a
//! *layer*: the page without its own annotations, then a one-page PDF of
//! its annotations as they are now ([`crate::worker::Layer`]), so what the
//! pane shows is what other viewers will show.
//!
//! Coordinates of annotations are the page's PDF user space; the pane
//! converts with each source page's transform ([`Source::to_user`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::component::input::{InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_pdf::annot::{Annot, Art, Font, Kind};
use kantor_pdf::form::{FieldKind, FieldValue};
use kantor_pdf::sig::Signatures;
use kantor_ui::i18n::{self, tf, tk};

use crate::PdfView;
use crate::model::{Change, PageKey, PageRef, SrcId};
use crate::worker::Layer;

/// Which toolbar row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    View,
    Comment,
    FillSign,
}

/// Something to put on a page with a click: a picture or a signature.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Placed {
    /// A PNG or JPEG file and its size in pixels.
    Image {
        bytes: Arc<Vec<u8>>,
        size: (u32, u32),
    },
    /// Drawn strokes in a 0..1 box (y up), and the box's width / height.
    Ink {
        strokes: Vec<Vec<[f32; 2]>>,
        aspect: f32,
    },
    /// A typed signature as outlines in a 0..1 box, and its aspect.
    Typed {
        segs: Arc<Vec<kantor_pdf::annot::Seg>>,
        aspect: f32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Tool {
    Select,
    Highlight,
    Underline,
    StrikeOut,
    Note,
    TextBox,
    Pen,
    Rect,
    Ellipse,
    Line,
    Arrow,
    Polygon,
    FsText,
    FsDate,
    FsCheck,
    FsCross,
    /// Drag the box of a certificate signature (Sign with Certificate,
    /// Sign with Provider).
    CertBox,
    /// Click or drag the e-meterai's square (Add e-Meterai).
    MeteraiBox,
    /// Put this down where the user clicks (`signature`: from Fill & Sign).
    Place {
        what: Placed,
        signature: bool,
    },
}

impl Tool {
    /// Text markup: drags select text.
    pub fn marks_text(&self) -> bool {
        matches!(self, Tool::Highlight | Tool::Underline | Tool::StrikeOut)
    }
}

/// The selected annotation: the page (place) and its id.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Sel {
    pub place: usize,
    pub id: String,
}

/// Which handle of the selection box (0 = top left, clockwise; 8 = the
/// turn handle of a stamp).
type Handle = usize;

/// A mouse drag on the pages, in the page's user space.
#[derive(Clone, Debug)]
pub(crate) enum Drag {
    Create {
        place: usize,
        start: [f32; 2],
        now: [f32; 2],
    },
    Ink {
        place: usize,
        points: Vec<[f32; 2]>,
    },
    Move {
        place: usize,
        id: String,
        start: [f32; 2],
        now: [f32; 2],
    },
    Resize {
        place: usize,
        id: String,
        handle: Handle,
        orig: [f32; 4],
        now: [f32; 2],
        free: bool,
    },
}

/// A text being typed into a text box or a note.
pub(crate) struct TextEdit {
    pub place: usize,
    pub id: String,
    pub input: Entity<TextareaState>,
    _sub: Subscription,
}

/// A form text field being filled.
pub(crate) struct FieldEdit {
    pub place: usize,
    pub src: SrcId,
    pub name: String,
    /// The widget's box (user space).
    pub rect: [f32; 4],
    pub input: Entity<InputState>,
    _sub: Subscription,
}

/// A drop-down or list field's options shown.
pub(crate) struct ChoiceOpen {
    pub place: usize,
    pub src: SrcId,
    pub name: String,
    pub rect: [f32; 4],
    pub options: Vec<(String, String)>,
}

struct LayerEntry {
    marks: Option<Arc<Vec<Annot>>>,
    fields: Option<Arc<HashMap<String, FieldValue>>>,
    layer: Layer,
}

pub(crate) struct Markup {
    pub mode: Mode,
    pub tool: Tool,
    pub color: [f32; 3],
    pub width: f32,
    pub font_size: f32,
    pub font: Font,
    pub opacity: f32,
    pub sel: Option<Sel>,
    pub drag: Option<Drag>,
    /// Polygon points so far.
    pub poly: Option<(usize, Vec<[f32; 2]>)>,
    pub editor: Option<TextEdit>,
    pub field: Option<FieldEdit>,
    pub choice: Option<ChoiceOpen>,
    layers: HashMap<PageKey, LayerEntry>,
    next_layer: u64,
    /// Sources whose annotations are being read.
    loading: HashSet<SrcId>,
    /// The reply being written in the comments panel: (annotation id, page).
    pub reply: Option<(String, usize, Entity<InputState>, Subscription)>,
    /// Where an image file dragged over the pages would go.
    pub stamp_drop: Option<(usize, [f32; 2])>,
    pub author: String,
}

impl Markup {
    pub fn new() -> Self {
        Self {
            mode: Mode::View,
            tool: Tool::Select,
            color: YELLOW,
            width: 2.0,
            font_size: 12.0,
            font: Font::Helvetica,
            opacity: 1.0,
            sel: None,
            drag: None,
            poly: None,
            editor: None,
            field: None,
            choice: None,
            layers: HashMap::new(),
            next_layer: 1,
            loading: HashSet::new(),
            reply: None,
            stamp_drop: None,
            author: kantor_core::settings::Settings::load().author_name(),
        }
    }
}

/// Key context of a form field being filled (Tab goes to the next).
pub(crate) const FIELD_CONTEXT: &str = "PdfField";

pub(crate) const YELLOW: [f32; 3] = [1.0, 0.85, 0.1];
/// The colours offered (as Acrobat's comment colours).
pub(crate) const COLORS: &[([f32; 3], &str)] = &[
    (YELLOW, tk("Yellow")),
    ([0.47, 0.87, 0.36], tk("Green")),
    ([0.35, 0.75, 1.0], tk("Blue")),
    ([1.0, 0.45, 0.6], tk("Pink")),
    ([0.9, 0.1, 0.1], tk("Red")),
    ([1.0, 0.55, 0.0], tk("Orange")),
    ([0.0, 0.2, 0.75], tk("Dark Blue")),
    ([0.0, 0.0, 0.0], tk("Black")),
];

pub(crate) fn hsla(c: [f32; 3], a: f32) -> Hsla {
    Rgba {
        r: c[0],
        g: c[1],
        b: c[2],
        a,
    }
    .into()
}

/// What may be changed in a signed document, with the message when not.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Allow {
    Annotations,
    Forms,
}

/// Overlay shapes painted over the pages (viewport px).
pub(crate) enum Ov {
    Quad {
        b: Bounds<Pixels>,
        fill: Option<Hsla>,
        border: Option<Hsla>,
    },
    Poly {
        pts: Vec<Point<Pixels>>,
        closed: bool,
        color: Hsla,
        width: f32,
    },
}

impl PdfView {
    // ── modes and tools ────────────────────────────────────────────────────

    pub fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        let m = &mut self.markup;
        m.mode = if m.mode == mode { Mode::View } else { mode };
        m.tool = Tool::Select;
        m.poly = None;
        if m.mode != Mode::View {
            self.load_all_marks(cx);
        }
        cx.notify();
    }

    pub fn mode(&self) -> Mode {
        self.markup.mode
    }

    pub(crate) fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if self.markup.mode == Mode::View {
            self.markup.mode = if matches!(
                tool,
                Tool::FsText | Tool::FsDate | Tool::FsCheck | Tool::FsCross
            ) {
                Mode::FillSign
            } else {
                Mode::Comment
            };
        }
        self.markup.tool = if self.markup.tool == tool {
            Tool::Select
        } else {
            tool
        };
        self.markup.poly = None;
        if matches!(self.markup.tool, Tool::Highlight) {
            self.markup.color = YELLOW;
        }
        // Text selected first, then Highlight: mark it at once (Acrobat).
        if self.markup.tool.marks_text() && self.ordered_sel().is_some() {
            let tool = self.markup.tool.clone();
            self.markup_selection(&tool, cx);
        }
        self.load_all_marks(cx);
        cx.notify();
    }

    /// Start reading every source's annotations (in the background).
    pub(crate) fn load_all_marks(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<SrcId> = self
            .doc()
            .map(|d| {
                let mut v: Vec<SrcId> = d.model.pages().iter().map(|p| p.src).collect();
                v.sort_unstable();
                v.dedup();
                v
            })
            .unwrap_or_default();
        for id in ids {
            self.ensure_marks(id, cx);
        }
    }

    /// Read source `id`'s annotations in the background if not done yet.
    /// Returns whether they are ready.
    pub(crate) fn ensure_marks(&mut self, id: SrcId, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.doc().and_then(|d| d.model.source(id)).cloned() else {
            return false;
        };
        if s.marks_ready().is_some() {
            return true;
        }
        if s.marks.get().is_some() || !self.markup.loading.insert(id) {
            return false;
        }
        let task = cx.background_spawn(async move {
            s.marks();
        });
        cx.spawn(async move |this, cx| {
            task.await;
            let _ = this.update(cx, |this, cx| {
                this.markup.loading.remove(&id);
                cx.notify();
            });
        })
        .detach();
        false
    }

    /// The base file's signatures (once its annotations are read).
    pub(crate) fn signatures(&self) -> Option<Signatures> {
        self.doc()?.model.base().marks_ready()?.sigs
    }

    /// Whether the change is allowed in a signed document; says why not.
    pub(crate) fn allowed(&mut self, what: Allow, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.signatures() else {
            return true;
        };
        let ok = match what {
            Allow::Annotations => s.allows_annotations(),
            Allow::Forms => s.allows_form_fill(),
        };
        if !ok {
            self.status = match s.allowed() {
                kantor_pdf::sig::Allowed::Nothing => i18n::t(
                    "This document is certified with a signature that allows no changes",
                ),
                _ => i18n::t(
                    "This document is certified with a signature that allows only filling in forms and signing",
                ),
            }
            .into();
            cx.notify();
        }
        ok
    }

    // ── layers ─────────────────────────────────────────────────────────────

    /// The layer to draw source page `key` with, if its annotations or
    /// fields changed.
    pub(crate) fn layer_of(&self, key: PageKey) -> Option<Layer> {
        self.markup.layers.get(&key).map(|e| e.layer.clone())
    }

    pub(crate) fn img_key(&self, key: PageKey) -> crate::layout::ImgKey {
        (
            key.0,
            key.1,
            self.markup.layers.get(&key).map_or(0, |e| e.layer.id),
        )
    }

    /// Make the layers follow the model (after any change, undo or redo).
    pub(crate) fn sync_layers(&mut self) {
        let Some(doc) = self.doc() else {
            return;
        };
        let m = &doc.model;
        let mut wanted: HashSet<PageKey> = m.edited_keys().collect();
        for (src, values) in m.all_field_values() {
            let Some(sm) = m.source(src).and_then(|s| s.marks_ready()) else {
                continue;
            };
            if let Some(form) = &sm.form {
                for f in &form.fields {
                    if values.contains_key(&f.name) {
                        wanted.extend(f.widgets.iter().map(|w| (src, w.page as usize)));
                    }
                }
            }
        }
        let mut new_layers = Vec::new();
        for key in &wanted {
            let marks = m.marks(*key);
            let fields = m.field_values(key.0).cloned();
            let same =
                self.markup.layers.get(key).is_some_and(|e| {
                    opt_ptr_eq(&e.marks, &marks) && opt_ptr_eq(&e.fields, &fields)
                });
            if same {
                continue;
            }
            let Some(sm) = m.source(key.0).and_then(|s| s.marks_ready()) else {
                continue;
            };
            let list = match &marks {
                Some(l) => l.clone(),
                None => continue,
            };
            let empty = HashMap::new();
            let values = fields.as_deref().unwrap_or(&empty);
            match kantor_pdf::annot::layer(&sm.pdf, key.1 as u32, &list, values) {
                Ok(bytes) => new_layers.push((*key, marks, fields, Arc::new(bytes))),
                Err(e) => log::warn!("annotation layer: {e}"),
            }
        }
        self.markup.layers.retain(|k, _| wanted.contains(k));
        for (key, marks, fields, bytes) in new_layers {
            let id = self.markup.next_layer;
            self.markup.next_layer += 1;
            self.markup.layers.insert(
                key,
                LayerEntry {
                    marks,
                    fields,
                    layer: Layer { id, bytes },
                },
            );
        }
    }

    // ── geometry ───────────────────────────────────────────────────────────

    /// The page under `pos` and the point there in its user space.
    pub(crate) fn user_at(
        &self,
        pos: Point<Pixels>,
        near: bool,
    ) -> Option<(usize, PageRef, [f32; 2])> {
        let (place, x, y) = self.hit(pos, near)?;
        let p = self.page_ref(place)?;
        let s = self.doc()?.model.source(p.src)?;
        Some((place, p, s.to_user(p.page, x, y)))
    }

    /// A user-space rectangle of page `place` on screen (viewport px).
    pub(crate) fn user_rect_screen(&self, place: usize, r: [f32; 4]) -> Option<Bounds<Pixels>> {
        let p = self.page_ref(place)?;
        let s = self.doc()?.model.source(p.src)?;
        Some(self.screen_rect(place, s.rect_to_page(p.page, r)))
    }

    /// A user-space point of page `place` on screen.
    pub(crate) fn user_point_screen(&self, place: usize, pt: [f32; 2]) -> Option<Point<Pixels>> {
        let p = self.page_ref(place)?;
        let s = self.doc()?.model.source(p.src)?;
        let q = s.to_page(p.page, pt[0], pt[1]);
        let b = self.screen_rect(place, [q[0], q[1], q[0], q[1]]);
        Some(b.origin)
    }

    /// Points per screen pixel at this zoom.
    fn pt_per_px(&self) -> f32 {
        1.0 / (self.zoom_value() * crate::layout::PX_PER_PT)
    }

    /// The annotations of page `place` now (`None` until read).
    pub(crate) fn place_marks(&self, place: usize) -> Option<Arc<Vec<Annot>>> {
        let p = self.page_ref(place)?;
        self.doc()?.model.marks(p.key())
    }

    pub(crate) fn find_mark(&self, sel: &Sel) -> Option<Annot> {
        self.place_marks(sel.place)?
            .iter()
            .find(|a| a.id == sel.id)
            .cloned()
    }

    /// The editable annotation at `pt` on page `place` (topmost).
    fn mark_at(&self, place: usize, pt: [f32; 2]) -> Option<Annot> {
        let marks = self.place_marks(place)?;
        let tol = 3.0 * self.pt_per_px();
        marks
            .iter()
            .rev()
            .filter(|a| a.kind.editable() && !a.hidden && a.reply_to.is_none())
            .find(|a| {
                let b = a.bounds();
                pt[0] >= b[0] - tol
                    && pt[0] <= b[2] + tol
                    && pt[1] >= b[1] - tol
                    && pt[1] <= b[3] + tol
            })
            .cloned()
    }

    /// Change page `place`'s annotations in one undo step.
    pub(crate) fn edit_marks(
        &mut self,
        place: usize,
        change: &'static str,
        f: impl FnOnce(&mut Vec<Annot>),
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.allowed(Allow::Annotations, cx) {
            return false;
        }
        let Some(p) = self.page_ref(place) else {
            return false;
        };
        if !self.ensure_marks(p.src, cx) {
            self.status =
                i18n::t("Still reading the page's comments; try again in a moment").into();
            cx.notify();
            return false;
        }
        let Some(cur) = self.place_marks(place) else {
            return false;
        };
        let mut list = (*cur).clone();
        f(&mut list);
        if list == *cur {
            return false;
        }
        if let Some(doc) = self.doc_mut() {
            doc.model.set_marks(p.key(), list, Change::Markup(change));
        }
        self.markup_changed(cx);
        self.status = i18n::t(change).into();
        true
    }

    /// After an annotation or form change (and undo / redo).
    pub(crate) fn markup_changed(&mut self, cx: &mut Context<Self>) {
        self.sync_layers();
        if let Some(s) = &self.markup.sel
            && self.find_mark(s).is_none()
        {
            self.markup.sel = None;
        }
        self.warn_signed_pages();
        cx.notify();
    }

    /// A new annotation with the current colour, width and author.
    fn new_annot(&self, kind: Kind, rect: [f32; 4]) -> Annot {
        let mut a = Annot::new(kind, rect, self.markup.color);
        a.author = self.markup.author.clone();
        a.width = self.markup.width;
        a
    }

    fn add_mark(&mut self, place: usize, a: Annot, change: &'static str, cx: &mut Context<Self>) {
        let id = a.id.clone();
        if self.edit_marks(place, change, |l| l.push(a), cx) {
            self.markup.sel = Some(Sel { place, id });
        }
    }

    // ── mouse ──────────────────────────────────────────────────────────────

    /// A press on the pages; true when markup took it.
    pub(crate) fn markup_mouse_down(
        &mut self,
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.commit_text_edit(cx);
        self.commit_field(cx);
        self.markup.choice = None;
        let Some((place, p, pt)) = self.user_at(ev.position, false) else {
            if self.markup.sel.take().is_some() {
                cx.notify();
            }
            return false;
        };
        self.ensure_marks(p.src, cx);
        let tool = self.markup.tool.clone();
        // Form fields answer in every mode with the Select tool.
        if tool == Tool::Select && self.field_click(place, p, pt, window, cx) {
            return true;
        }
        match tool {
            Tool::Select => self.select_at(place, pt, ev, window, cx),
            Tool::Highlight | Tool::Underline | Tool::StrikeOut => false,
            Tool::Note => {
                let mut a = self.new_annot(Kind::Note, [pt[0], pt[1] - 20.0, pt[0] + 20.0, pt[1]]);
                a.color = YELLOW;
                let id = a.id.clone();
                self.add_mark(place, a, tk("Add Sticky Note"), cx);
                self.markup.tool = Tool::Select;
                self.open_text_edit(place, id, window, cx);
                true
            }
            Tool::FsText | Tool::FsDate => {
                let size = self.markup.font_size;
                let text = if tool == Tool::FsDate {
                    today()
                } else {
                    String::new()
                };
                let w = if text.is_empty() {
                    160.0
                } else {
                    self.markup.font.width(&text, size) + 8.0
                };
                let mut a = self.new_annot(
                    Kind::FreeText {
                        font: self.markup.font,
                        size,
                    },
                    [pt[0], pt[1] - size * 1.6, pt[0] + w, pt[1]],
                );
                a.color = [0.0, 0.0, 0.0];
                a.width = 0.0;
                a.contents = text.clone();
                a.subject = i18n::t("Fill & Sign").into();
                let id = a.id.clone();
                self.add_mark(place, a, tk("Add Text"), cx);
                if text.is_empty() {
                    self.open_text_edit(place, id, window, cx);
                }
                true
            }
            Tool::FsCheck | Tool::FsCross => {
                let s = 12.0;
                let (x, y) = (pt[0] - s / 2.0, pt[1] - s / 2.0);
                let strokes = if tool == Tool::FsCheck {
                    vec![vec![
                        [x + 1.0, y + s * 0.5],
                        [x + s * 0.4, y + 1.5],
                        [x + s - 1.0, y + s - 1.0],
                    ]]
                } else {
                    vec![
                        vec![[x + 1.5, y + 1.5], [x + s - 1.5, y + s - 1.5]],
                        vec![[x + 1.5, y + s - 1.5], [x + s - 1.5, y + 1.5]],
                    ]
                };
                let mut a = self.new_annot(Kind::Ink(strokes), [x, y, x + s, y + s]);
                a.color = [0.0, 0.0, 0.0];
                a.width = 1.5;
                a.subject = i18n::t("Fill & Sign").into();
                self.add_mark(place, a, tk("Add Mark"), cx);
                true
            }
            Tool::Place { what, signature } => {
                self.place_item(place, pt, &what, signature, cx);
                self.markup.tool = Tool::Select;
                true
            }
            Tool::Polygon => {
                let done = ev.click_count >= 2;
                let pts = match &mut self.markup.poly {
                    Some((pl, pts)) if *pl == place => {
                        pts.push(pt);
                        pts.clone()
                    }
                    _ => {
                        self.markup.poly = Some((place, vec![pt]));
                        vec![pt]
                    }
                };
                if done {
                    self.finish_polygon(cx);
                } else if pts.len() > 2 {
                    let first = pts[0];
                    let tol = 6.0 * self.pt_per_px();
                    if (first[0] - pt[0]).abs() < tol && (first[1] - pt[1]).abs() < tol {
                        if let Some((_, p)) = &mut self.markup.poly {
                            p.pop();
                        }
                        self.finish_polygon(cx);
                    }
                }
                cx.notify();
                true
            }
            Tool::Pen => {
                self.markup.drag = Some(Drag::Ink {
                    place,
                    points: vec![pt],
                });
                true
            }
            Tool::TextBox
            | Tool::Rect
            | Tool::Ellipse
            | Tool::Line
            | Tool::Arrow
            | Tool::CertBox
            | Tool::MeteraiBox => {
                self.markup.drag = Some(Drag::Create {
                    place,
                    start: pt,
                    now: pt,
                });
                true
            }
        }
    }

    /// The Select tool's press: on a handle, an annotation, or nothing.
    fn select_at(
        &mut self,
        place: usize,
        pt: [f32; 2],
        ev: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(sel) = self.markup.sel.clone()
            && sel.place == place
            && let Some(a) = self.find_mark(&sel)
            && let Some(h) = self.handle_at(place, &a, ev.position)
        {
            if h == 8 {
                // The turn handle turns a quarter clockwise.
                self.turn_stamp(-90.0, cx);
                return true;
            }
            self.markup.drag = Some(Drag::Resize {
                place,
                id: a.id.clone(),
                handle: h,
                orig: a.rect,
                now: pt,
                free: ev.modifiers.shift,
            });
            return true;
        }
        match self.mark_at(place, pt) {
            Some(a) => {
                self.markup.sel = Some(Sel {
                    place,
                    id: a.id.clone(),
                });
                if ev.click_count >= 2 && matches!(a.kind, Kind::FreeText { .. } | Kind::Note) {
                    self.open_text_edit(place, a.id, window, cx);
                } else {
                    self.markup.drag = Some(Drag::Move {
                        place,
                        id: a.id,
                        start: pt,
                        now: pt,
                    });
                }
                cx.notify();
                true
            }
            None => {
                if self.markup.sel.take().is_some() {
                    cx.notify();
                }
                false
            }
        }
    }

    /// The selection box's handles on screen (8 around, plus the turn
    /// handle of a stamp).
    fn handles(&self, place: usize, a: &Annot) -> Vec<Bounds<Pixels>> {
        let Some(b) = self.user_rect_screen(place, a.bounds()) else {
            return Vec::new();
        };
        let (x0, y0) = (b.origin.x, b.origin.y);
        let (x1, y1) = (x0 + b.size.width, y0 + b.size.height);
        let xm = (x0 + x1) / 2.0;
        let ym = (y0 + y1) / 2.0;
        let s = px(7.);
        let mut out: Vec<Bounds<Pixels>> = [
            (x0, y0),
            (xm, y0),
            (x1, y0),
            (x1, ym),
            (x1, y1),
            (xm, y1),
            (x0, y1),
            (x0, ym),
        ]
        .iter()
        .map(|&(x, y)| Bounds::new(point(x - s / 2.0, y - s / 2.0), size(s, s)))
        .collect();
        if matches!(a.kind, Kind::Stamp { .. }) {
            out.push(Bounds::new(point(xm - s / 2.0, y0 - s * 3.0), size(s, s)));
        }
        out
    }

    fn handle_at(&self, place: usize, a: &Annot, pos: Point<Pixels>) -> Option<Handle> {
        let local = pos - self.viewport.get().origin;
        if !a.kind.editable() || matches!(a.kind, Kind::Note) {
            return None;
        }
        self.handles(place, a).iter().position(|h| {
            let grow = px(3.);
            Bounds::new(
                h.origin - point(grow, grow),
                h.size + size(grow * 2.0, grow * 2.0),
            )
            .contains(&local)
        })
    }

    pub(crate) fn markup_mouse_move(
        &mut self,
        ev: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.markup.drag.is_none() {
            return false;
        }
        if ev.pressed_button != Some(MouseButton::Left) {
            return false;
        }
        let place = match &self.markup.drag {
            Some(Drag::Create { place, .. })
            | Some(Drag::Ink { place, .. })
            | Some(Drag::Move { place, .. })
            | Some(Drag::Resize { place, .. }) => *place,
            None => return false,
        };
        let Some(pt) = self.user_on(place, ev.position) else {
            return true;
        };
        let shift = ev.modifiers.shift;
        match &mut self.markup.drag {
            Some(Drag::Create { now, .. }) | Some(Drag::Move { now, .. }) => *now = pt,
            Some(Drag::Resize { now, free, .. }) => {
                *now = pt;
                *free = shift;
            }
            Some(Drag::Ink { points, .. }) => points.push(pt),
            None => {}
        }
        cx.notify();
        true
    }

    /// The user-space point of page `place` under `pos` (even outside it).
    fn user_on(&self, place: usize, pos: Point<Pixels>) -> Option<[f32; 2]> {
        let r = self.page_rect(place);
        let local = pos - self.viewport.get().origin;
        let k = self.zoom_value() * crate::layout::PX_PER_PT;
        let tx = f32::from(local.x - r.origin.x) / k;
        let ty = f32::from(local.y - r.origin.y) / k;
        let (w, h) = self.page_points(place);
        let (x, y) = self.page_turn(place).unmap(w, h, tx, ty);
        let p = self.page_ref(place)?;
        Some(self.doc()?.model.source(p.src)?.to_user(p.page, x, y))
    }

    pub(crate) fn markup_mouse_up(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if self.markup.tool.marks_text() && self.ordered_sel().is_some() {
            let tool = self.markup.tool.clone();
            self.markup_selection(&tool, cx);
            return false;
        }
        let Some(drag) = self.markup.drag.take() else {
            return false;
        };
        let k = self.pt_per_px();
        match drag {
            Drag::Move {
                place,
                id,
                start,
                now,
            } => {
                let (dx, dy) = (now[0] - start[0], now[1] - start[1]);
                if dx.abs() + dy.abs() > k {
                    self.edit_marks(
                        place,
                        tk("Move Comment"),
                        |l| {
                            if let Some(a) = l.iter_mut().find(|a| a.id == id) {
                                a.translate(dx, dy);
                                a.touch();
                            }
                        },
                        cx,
                    );
                }
            }
            Drag::Resize {
                place,
                id,
                handle,
                orig,
                now,
                free,
            } => {
                let Some(a) = self.find_mark(&Sel {
                    place,
                    id: id.clone(),
                }) else {
                    return true;
                };
                let lock = matches!(a.kind, Kind::Stamp { .. }) && !free;
                let r = resized(orig, handle, now, lock);
                self.edit_marks(
                    place,
                    tk("Resize Comment"),
                    |l| {
                        if let Some(a) = l.iter_mut().find(|a| a.id == id) {
                            a.resize(r);
                            a.touch();
                        }
                    },
                    cx,
                );
            }
            Drag::Ink { place, points } => {
                if points.len() >= 2 {
                    let kind = Kind::Ink(vec![simplify(&points, k * 0.75)]);
                    let r =
                        kantor_pdf::annot::fit_rect(&kind, self.markup.width).unwrap_or_default();
                    let mut a = self.new_annot(kind, r);
                    a.opacity = self.markup.opacity;
                    self.add_mark(place, a, tk("Draw"), cx);
                }
            }
            Drag::Create { place, start, now } => {
                let small =
                    (now[0] - start[0]).abs() < 4.0 * k && (now[1] - start[1]).abs() < 4.0 * k;
                let tool = self.markup.tool.clone();
                let rect = |w: f32, h: f32| {
                    if small {
                        [start[0], start[1] - h, start[0] + w, start[1]]
                    } else {
                        [start[0], start[1], now[0], now[1]]
                    }
                };
                match tool {
                    Tool::TextBox => {
                        let size = self.markup.font_size;
                        let mut a = self.new_annot(
                            Kind::FreeText {
                                font: self.markup.font,
                                size,
                            },
                            rect(180.0, size * 3.0),
                        );
                        a.color = if self.markup.color == YELLOW {
                            [0.0, 0.0, 0.0]
                        } else {
                            self.markup.color
                        };
                        a.width = 1.0;
                        let id = a.id.clone();
                        self.add_mark(place, a, tk("Add Text Box"), cx);
                        self.markup.tool = Tool::Select;
                        self.open_text_edit(place, id, window, cx);
                    }
                    Tool::CertBox => {
                        let r = rect(200.0, 60.0);
                        let r = [
                            r[0].min(r[2]),
                            r[1].min(r[3]),
                            r[0].max(r[2]),
                            r[1].max(r[3]),
                        ];
                        if matches!(self.esign, Some(crate::esign::Esign::Sign(_))) {
                            self.provider_box_drawn(place, r, window, cx);
                        } else {
                            self.cert_box_drawn(place, r, window, cx);
                        }
                    }
                    Tool::MeteraiBox => {
                        let r = crate::esign::meterai_rect(start, now, small);
                        self.meterai_box_drawn(place, r, cx);
                    }
                    Tool::Rect | Tool::Ellipse => {
                        let kind = if tool == Tool::Rect {
                            Kind::Square
                        } else {
                            Kind::Circle
                        };
                        let mut a = self.new_annot(kind, rect(72.0, 48.0));
                        a.opacity = self.markup.opacity;
                        self.add_mark(place, a, tk("Add Shape"), cx);
                    }
                    Tool::Line | Tool::Arrow => {
                        let end = if small {
                            [start[0] + 72.0, start[1]]
                        } else {
                            now
                        };
                        let kind = Kind::Line {
                            a: start,
                            b: end,
                            arrow: tool == Tool::Arrow,
                        };
                        let r = kantor_pdf::annot::fit_rect(&kind, self.markup.width)
                            .unwrap_or_default();
                        let mut a = self.new_annot(kind, r);
                        a.opacity = self.markup.opacity;
                        self.add_mark(place, a, tk("Add Shape"), cx);
                    }
                    _ => {}
                }
            }
        }
        cx.notify();
        true
    }

    fn finish_polygon(&mut self, cx: &mut Context<Self>) {
        let Some((place, pts)) = self.markup.poly.take() else {
            return;
        };
        let mut pts = pts;
        pts.dedup();
        if pts.len() < 3 {
            return;
        }
        let kind = Kind::Polygon(pts);
        let r = kantor_pdf::annot::fit_rect(&kind, self.markup.width).unwrap_or_default();
        let a = self.new_annot(kind, r);
        self.add_mark(place, a, tk("Add Shape"), cx);
    }

    /// Highlight / underline / strike out the selected text.
    pub(crate) fn markup_selection(&mut self, tool: &Tool, cx: &mut Context<Self>) {
        let Some((a, b)) = self.ordered_sel() else {
            return;
        };
        let mut done = false;
        for place in a.page..=b.page {
            let Some(range) = self.sel_on(place) else {
                continue;
            };
            let Some(text) = self.texts.get(&place).cloned() else {
                continue;
            };
            let Some(p) = self.page_ref(place) else {
                continue;
            };
            let Some(s) = self.doc().and_then(|d| d.model.source(p.src)).cloned() else {
                continue;
            };
            let mut quads = Vec::new();
            let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for r in text.rects(range.clone()) {
                // Page-space rect → user-space corners (Acrobat's order).
                let ul = s.to_user(p.page, r[0], r[1]);
                let ur = s.to_user(p.page, r[2], r[1]);
                let ll = s.to_user(p.page, r[0], r[3]);
                let lr = s.to_user(p.page, r[2], r[3]);
                for q in [ul, ur, ll, lr] {
                    bounds = [
                        bounds[0].min(q[0]),
                        bounds[1].min(q[1]),
                        bounds[2].max(q[0]),
                        bounds[3].max(q[1]),
                    ];
                }
                quads.push([ul[0], ul[1], ur[0], ur[1], ll[0], ll[1], lr[0], lr[1]]);
            }
            if quads.is_empty() {
                continue;
            }
            let content: String = text.text.get(range).unwrap_or_default().to_owned();
            let (kind, change) = match tool {
                Tool::Underline => (Kind::Underline(quads), tk("Underline")),
                Tool::StrikeOut => (Kind::StrikeOut(quads), tk("Strikethrough")),
                _ => (Kind::Highlight(quads), tk("Highlight")),
            };
            let mut ann = self.new_annot(kind, bounds);
            ann.contents = content;
            if matches!(tool, Tool::Underline | Tool::StrikeOut) && self.markup.color == YELLOW {
                ann.color = [0.9, 0.1, 0.1];
            }
            self.add_mark(place, ann, change, cx);
            done = true;
        }
        if done {
            self.sel = None;
        }
    }

    /// Put a picture or signature at `pt`, centred, at its natural size
    /// (images at 96 dpi, signatures 150 pt wide), at most half the page.
    pub(crate) fn place_item(
        &mut self,
        place: usize,
        pt: [f32; 2],
        what: &Placed,
        signature: bool,
        cx: &mut Context<Self>,
    ) {
        let (pw, ph) = self.page_points(place);
        let (w, h) = match what {
            Placed::Image { size, .. } => (size.0 as f32 * 0.75, size.1 as f32 * 0.75),
            Placed::Ink { aspect, .. } | Placed::Typed { aspect, .. } => {
                (150.0, 150.0 / aspect.max(0.05))
            }
        };
        let k = (pw * 0.5 / w).min(ph * 0.5 / h).min(1.0);
        let (w, h) = (w * k, h * k);
        let rect = [
            pt[0] - w / 2.0,
            pt[1] - h / 2.0,
            pt[0] + w / 2.0,
            pt[1] + h / 2.0,
        ];
        let mut a = match what {
            Placed::Image { bytes, .. } => self.new_annot(
                Kind::Stamp {
                    art: Art::Image(bytes.clone()),
                    rotate: 0.0,
                    signature,
                },
                rect,
            ),
            Placed::Typed { segs, .. } => {
                let mut a = self.new_annot(
                    Kind::Stamp {
                        art: Art::Paths(segs.clone()),
                        rotate: 0.0,
                        signature,
                    },
                    rect,
                );
                a.color = [0.05, 0.1, 0.35];
                a
            }
            Placed::Ink { strokes, .. } => {
                let strokes = strokes
                    .iter()
                    .map(|s| {
                        s.iter()
                            .map(|p| [rect[0] + p[0] * w, rect[1] + p[1] * h])
                            .collect()
                    })
                    .collect();
                let mut a = self.new_annot(Kind::Ink(strokes), rect);
                a.color = [0.05, 0.1, 0.35];
                a.width = 1.5;
                a
            }
        };
        if signature {
            a.subject = "Signature".into();
        }
        let change = if signature {
            tk("Add Signature")
        } else {
            tk("Add Stamp")
        };
        self.add_mark(place, a, change, cx);
    }

    // ── the selection ──────────────────────────────────────────────────────

    pub(crate) fn delete_mark(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(sel) = self.markup.sel.clone() else {
            return false;
        };
        self.edit_marks(
            sel.place,
            tk("Delete Comment"),
            |l| {
                // Its replies and review states go with it.
                l.retain(|a| a.id != sel.id && a.reply_to.as_deref() != Some(&sel.id));
            },
            cx,
        );
        self.markup.sel = None;
        cx.notify();
        true
    }

    /// Apply `f` to the selected annotation (one undo step).
    pub(crate) fn change_selected(
        &mut self,
        change: &'static str,
        f: impl FnOnce(&mut Annot),
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(sel) = self.markup.sel.clone() else {
            return false;
        };
        self.edit_marks(
            sel.place,
            change,
            |l| {
                if let Some(a) = l.iter_mut().find(|a| a.id == sel.id) {
                    f(a);
                    a.touch();
                }
            },
            cx,
        )
    }

    pub(crate) fn set_color(&mut self, c: [f32; 3], cx: &mut Context<Self>) {
        self.markup.color = c;
        self.change_selected(tk("Change Color"), |a| a.color = c, cx);
        cx.notify();
    }

    pub(crate) fn set_width(&mut self, w: f32, cx: &mut Context<Self>) {
        self.markup.width = w;
        self.change_selected(
            tk("Change Line Width"),
            |a| {
                if !matches!(a.kind, Kind::FreeText { .. }) || a.width > 0.0 {
                    a.width = w;
                    if let Some(r) = kantor_pdf::annot::fit_rect(&a.kind, w) {
                        a.rect = r;
                    }
                }
            },
            cx,
        );
        cx.notify();
    }

    pub(crate) fn set_font_size(&mut self, s: f32, cx: &mut Context<Self>) {
        self.markup.font_size = s;
        self.change_selected(
            tk("Change Font Size"),
            |a| {
                if let Kind::FreeText { size, .. } = &mut a.kind {
                    *size = s;
                }
            },
            cx,
        );
        cx.notify();
    }

    pub(crate) fn set_font(&mut self, f: Font, cx: &mut Context<Self>) {
        self.markup.font = f;
        self.change_selected(
            tk("Change Font"),
            |a| {
                if let Kind::FreeText { font, .. } = &mut a.kind {
                    *font = f;
                }
            },
            cx,
        );
        cx.notify();
    }

    pub(crate) fn set_opacity(&mut self, o: f32, cx: &mut Context<Self>) {
        self.markup.opacity = o;
        self.change_selected(tk("Change Opacity"), |a| a.opacity = o, cx);
        cx.notify();
    }

    /// Turn the selected stamp by `deg` (counterclockwise positive).
    pub(crate) fn turn_stamp(&mut self, deg: f32, cx: &mut Context<Self>) {
        self.change_selected(
            tk("Rotate Stamp"),
            |a| {
                if let Kind::Stamp { rotate, .. } = &mut a.kind {
                    *rotate = (*rotate + deg).rem_euclid(360.0);
                }
            },
            cx,
        );
    }

    /// The selected stamp, copied to the same place of `pages` (0-based
    /// places; its own page is skipped).
    pub(crate) fn stamp_on_pages(&mut self, pages: &[usize], cx: &mut Context<Self>) -> usize {
        let Some(sel) = self.markup.sel.clone() else {
            return 0;
        };
        let Some(a) = self.find_mark(&sel) else {
            return 0;
        };
        if !self.allowed(Allow::Annotations, cx) {
            return 0;
        }
        let Some(doc) = self.doc() else {
            return 0;
        };
        let mut list: Vec<(PageKey, Vec<Annot>)> = Vec::new();
        let mut missing = false;
        for &place in pages {
            if place == sel.place {
                continue;
            }
            let Some(p) = doc.model.page(place) else {
                continue;
            };
            if list.iter().any(|(k, _)| *k == p.key()) {
                continue;
            }
            let Some(cur) = doc.model.marks(p.key()) else {
                missing = true;
                continue;
            };
            let mut l = (*cur).clone();
            let mut copy = a.clone();
            copy.id = kantor_pdf::annot::new_id();
            copy.orig = None;
            copy.dirty = true;
            l.push(copy);
            list.push((p.key(), l));
        }
        let n = list.len();
        if missing {
            self.load_all_marks(cx);
        }
        if n > 0
            && let Some(doc) = self.doc_mut()
        {
            doc.model
                .set_many_marks(list, Change::Markup(tk("Place on Pages")));
        }
        self.markup_changed(cx);
        self.status = tf("Placed on {} pages", &[&n]).into();
        n
    }

    // ── typing text ────────────────────────────────────────────────────────

    pub(crate) fn open_text_edit(
        &mut self,
        place: usize,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = self
            .find_mark(&Sel {
                place,
                id: id.clone(),
            })
            .map(|a| a.contents)
            .unwrap_or_default();
        let input = cx.new(|cx| {
            let mut s = TextareaState::new(window, cx)
                .auto_grow(2, 12)
                .placeholder(i18n::t("Type here"));
            s.set_value(text, window, cx);
            s
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter {
                    secondary: true, ..
                } => {
                    this.commit_text_edit(cx);
                    this.focus(window, cx);
                }
                InputEvent::Blur => {
                    this.commit_text_edit(cx);
                }
                _ => {}
            },
        );
        input.update(cx, |s, cx| s.focus(window, cx));
        self.markup.editor = Some(TextEdit {
            place,
            id,
            input,
            _sub: sub,
        });
        cx.notify();
    }

    pub(crate) fn commit_text_edit(&mut self, cx: &mut Context<Self>) {
        let Some(e) = self.markup.editor.take() else {
            return;
        };
        let text = e.input.read(cx).value().trim_end().to_string();
        let sel = Sel {
            place: e.place,
            id: e.id.clone(),
        };
        let Some(a) = self.find_mark(&sel) else {
            return;
        };
        if a.contents == text {
            cx.notify();
            return;
        }
        let font_grow = matches!(a.kind, Kind::FreeText { .. });
        self.edit_marks(
            e.place,
            tk("Edit Text"),
            |l| {
                if let Some(a) = l.iter_mut().find(|a| a.id == e.id) {
                    a.contents = text;
                    if font_grow && let Kind::FreeText { font, size } = a.kind {
                        // Grow the box downwards to fit the text.
                        let w = (a.rect[2] - a.rect[0] - 2.0 * (2.0 + a.width)).max(1.0);
                        let lines = kantor_pdf::annot::wrap(&a.contents, font, size, w)
                            .len()
                            .max(1);
                        let need = lines as f32 * size * 1.17 + 2.0 * (2.0 + a.width) + size * 0.3;
                        if need > a.rect[3] - a.rect[1] {
                            a.rect[1] = a.rect[3] - need;
                        }
                    }
                    a.touch();
                }
            },
            cx,
        );
        cx.notify();
    }

    // ── forms ──────────────────────────────────────────────────────────────

    /// A press on a form field: fill it. True when there was one.
    fn field_click(
        &mut self,
        place: usize,
        p: PageRef,
        pt: [f32; 2],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(sm) = self
            .doc()
            .and_then(|d| d.model.source(p.src))
            .and_then(|s| s.marks_ready())
            .cloned()
        else {
            return false;
        };
        let Some(form) = &sm.form else {
            return false;
        };
        let hit = form.fields.iter().find_map(|f| {
            let w = f.widgets.iter().find(|w| {
                w.page as usize == p.page
                    && pt[0] >= w.rect[0]
                    && pt[0] <= w.rect[2]
                    && pt[1] >= w.rect[1]
                    && pt[1] <= w.rect[3]
            })?;
            Some((f.clone(), w.clone()))
        });
        let Some((f, w)) = hit else {
            return false;
        };
        if f.read_only || matches!(f.kind, FieldKind::Push | FieldKind::Signature) {
            return true;
        }
        if !self.allowed(Allow::Forms, cx) {
            return true;
        }
        let cur = self.doc().and_then(|d| d.model.field_value(p.src, &f.name));
        match &f.kind {
            FieldKind::Check | FieldKind::Radio => {
                let on = w.on_state.clone().unwrap_or_else(|| "Yes".into());
                let is_on = cur == Some(FieldValue::State(on.clone()));
                let v = if is_on && f.kind == FieldKind::Check {
                    FieldValue::State("Off".into())
                } else {
                    FieldValue::State(on)
                };
                self.set_field(p.src, f.name.clone(), v, cx);
            }
            FieldKind::Combo { .. } | FieldKind::List { .. } => {
                self.markup.choice = Some(ChoiceOpen {
                    place,
                    src: p.src,
                    name: f.name.clone(),
                    rect: w.rect,
                    options: f.options.clone(),
                });
                cx.notify();
            }
            FieldKind::Text { .. } => {
                let text = match cur {
                    Some(FieldValue::Text(t)) => t,
                    _ => String::new(),
                };
                self.open_field(place, p.src, f.name.clone(), w.rect, text, window, cx);
            }
            _ => {}
        }
        true
    }

    pub(crate) fn set_field(
        &mut self,
        src: SrcId,
        name: String,
        v: FieldValue,
        cx: &mut Context<Self>,
    ) {
        let same = self.doc().and_then(|d| d.model.field_value(src, &name)) == Some(v.clone());
        if same {
            return;
        }
        if let Some(doc) = self.doc_mut() {
            doc.model
                .set_fields(src, vec![(name, v)], Change::Markup(tk("Fill Form")));
        }
        self.markup_changed(cx);
    }

    #[allow(clippy::too_many_arguments)]
    fn open_field(
        &mut self,
        place: usize,
        src: SrcId,
        name: String,
        rect: [f32; 4],
        text: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |i, cx| {
            i.set_value(text, window, cx);
            i.focus(window, cx);
            i.select_all(window, cx);
        });
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, _, ev: &InputEvent, window, cx| match ev {
                InputEvent::PressEnter { .. } => {
                    this.commit_field(cx);
                    this.focus(window, cx);
                }
                InputEvent::Blur => this.commit_field(cx),
                _ => {}
            },
        );
        self.markup.field = Some(FieldEdit {
            place,
            src,
            name,
            rect,
            input,
            _sub: sub,
        });
        cx.notify();
    }

    pub(crate) fn commit_field(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.markup.field.take() else {
            return;
        };
        let text = f.input.read(cx).value().to_string();
        self.set_field(f.src, f.name, FieldValue::Text(text), cx);
        cx.notify();
    }

    /// Tab in a field: the next (or previous) text or choice field.
    pub(crate) fn tab_field(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cur) = self
            .markup
            .field
            .as_ref()
            .map(|f| (f.place, f.src, f.name.clone()))
        else {
            return;
        };
        self.commit_field(cx);
        let Some(doc) = self.doc() else {
            return;
        };
        // Every fillable widget in page order, then annotation order.
        let mut all: Vec<(
            usize,
            SrcId,
            kantor_pdf::form::Field,
            kantor_pdf::form::Widget,
        )> = Vec::new();
        for (place, p) in doc.model.pages().iter().enumerate() {
            let Some(sm) = doc.model.source(p.src).and_then(|s| s.marks_ready()) else {
                continue;
            };
            let Some(form) = &sm.form else { continue };
            let mut ws: Vec<_> = form
                .fields
                .iter()
                .filter(|f| !f.read_only && matches!(f.kind, FieldKind::Text { .. }))
                .flat_map(|f| {
                    f.widgets
                        .iter()
                        .filter(|w| w.page as usize == p.page)
                        .map(move |w| (f, w))
                })
                .collect();
            ws.sort_by_key(|(_, w)| w.annot_index);
            all.extend(
                ws.into_iter()
                    .map(|(f, w)| (place, p.src, f.clone(), w.clone())),
            );
        }
        if all.is_empty() {
            return;
        }
        let i = all
            .iter()
            .position(|(pl, s, f, _)| *pl == cur.0 && *s == cur.1 && f.name == cur.2)
            .unwrap_or(0);
        let n = all.len();
        let j = if back { (i + n - 1) % n } else { (i + 1) % n };
        let (place, src, f, w) = all[j].clone();
        let text = match doc.model.field_value(src, &f.name) {
            Some(FieldValue::Text(t)) => t,
            _ => String::new(),
        };
        let b = w.rect;
        if let Some(p) = self.page_ref(place)
            && let Some(s) = self.doc().and_then(|d| d.model.source(p.src)).cloned()
        {
            let r = s.rect_to_page(p.page, b);
            self.reveal(place, r, cx);
        }
        self.open_field(place, src, f.name, b, text, window, cx);
    }

    /// Reset Form: every field of every source back to its default.
    pub fn reset_form(&mut self, cx: &mut Context<Self>) {
        if !self.allowed(Allow::Forms, cx) {
            return;
        }
        let Some(doc) = self.doc() else {
            return;
        };
        let mut srcs: Vec<SrcId> = doc.model.pages().iter().map(|p| p.src).collect();
        srcs.sort_unstable();
        srcs.dedup();
        let mut any = false;
        let mut sets = Vec::new();
        for s in srcs {
            let Some(sm) = doc.model.source(s).and_then(|x| x.marks_ready()) else {
                continue;
            };
            let Some(form) = &sm.form else { continue };
            let d: Vec<(String, FieldValue)> = form.defaults().into_iter().collect();
            if !d.is_empty() {
                sets.push((s, d));
                any = true;
            }
        }
        if !any {
            self.status = i18n::t("This document has no form fields").into();
            cx.notify();
            return;
        }
        if let Some(doc) = self.doc_mut() {
            for (s, d) in sets {
                doc.model.set_fields(s, d, Change::Markup(tk("Reset Form")));
            }
        }
        self.markup_changed(cx);
        self.status = i18n::t("Reset Form").into();
    }

    /// The document's form, if any, and whether it is XFA only.
    pub(crate) fn form_state(&self) -> Option<bool> {
        let doc = self.doc()?;
        let sm = doc.model.base().marks_ready()?;
        let f = sm.form.as_ref()?;
        if f.fields.is_empty() && !f.xfa {
            return None;
        }
        Some(f.xfa_only())
    }

    // ── painting ───────────────────────────────────────────────────────────

    /// What markup paints over the pages this frame (viewport px).
    pub(crate) fn markup_overlays(&self, pages: std::ops::Range<usize>, cx: &App) -> Vec<Ov> {
        let t = kantor_ui::tokens(cx);
        let mut out = Vec::new();
        let accent = t.accent;
        // Form fields: a light blue tint like Acrobat's field highlight.
        for place in pages.clone() {
            let Some(p) = self.page_ref(place) else {
                continue;
            };
            let Some(sm) = self
                .doc()
                .and_then(|d| d.model.source(p.src))
                .and_then(|s| s.marks_ready())
            else {
                continue;
            };
            if let Some(form) = &sm.form {
                for f in &form.fields {
                    if f.read_only || matches!(f.kind, FieldKind::Push) {
                        continue;
                    }
                    for w in f.widgets.iter().filter(|w| w.page as usize == p.page) {
                        if let Some(b) = self.user_rect_screen(place, w.rect) {
                            out.push(Ov::Quad {
                                b,
                                fill: Some(hsla([0.8, 0.85, 1.0], 0.35)),
                                border: None,
                            });
                        }
                    }
                }
            }
        }
        // The selection.
        if let Some(sel) = &self.markup.sel
            && pages.contains(&sel.place)
            && let Some(a) = self.find_mark(sel)
            && let Some(b) = self.user_rect_screen(sel.place, a.bounds())
        {
            out.push(Ov::Quad {
                b,
                fill: None,
                border: Some(accent),
            });
            if !matches!(a.kind, Kind::Note) && self.markup.drag.is_none() {
                for h in self.handles(sel.place, &a) {
                    out.push(Ov::Quad {
                        b: h,
                        fill: Some(t.paper),
                        border: Some(accent),
                    });
                }
            }
        }
        // Drags in progress.
        let color = hsla(self.markup.color, 1.0);
        let w = (self.markup.width / self.pt_per_px()).max(1.0);
        match &self.markup.drag {
            Some(Drag::Ink { place, points }) => {
                let pts: Vec<_> = points
                    .iter()
                    .filter_map(|p| self.user_point_screen(*place, *p))
                    .collect();
                out.push(Ov::Poly {
                    pts,
                    closed: false,
                    color,
                    width: w,
                });
            }
            Some(Drag::Create { place, start, now }) => {
                let r = [
                    start[0].min(now[0]),
                    start[1].min(now[1]),
                    start[0].max(now[0]),
                    start[1].max(now[1]),
                ];
                match self.markup.tool {
                    Tool::Line | Tool::Arrow => {
                        let pts: Vec<_> = [*start, *now]
                            .iter()
                            .filter_map(|p| self.user_point_screen(*place, *p))
                            .collect();
                        out.push(Ov::Poly {
                            pts,
                            closed: false,
                            color,
                            width: w,
                        });
                    }
                    Tool::Ellipse => {
                        let (cx0, cy0) = ((r[0] + r[2]) / 2.0, (r[1] + r[3]) / 2.0);
                        let (rx, ry) = ((r[2] - r[0]) / 2.0, (r[3] - r[1]) / 2.0);
                        let pts: Vec<_> = (0..48)
                            .filter_map(|i| {
                                let a = i as f32 / 48.0 * std::f32::consts::TAU;
                                self.user_point_screen(
                                    *place,
                                    [cx0 + rx * a.cos(), cy0 + ry * a.sin()],
                                )
                            })
                            .collect();
                        out.push(Ov::Poly {
                            pts,
                            closed: true,
                            color,
                            width: w,
                        });
                    }
                    _ => {
                        if let Some(b) = self.user_rect_screen(*place, r) {
                            out.push(Ov::Quad {
                                b,
                                fill: None,
                                border: Some(color),
                            });
                        }
                    }
                }
            }
            Some(Drag::Move {
                place,
                id,
                start,
                now,
            }) => {
                if let Some(a) = self.find_mark(&Sel {
                    place: *place,
                    id: id.clone(),
                }) {
                    let mut b = a.bounds();
                    let (dx, dy) = (now[0] - start[0], now[1] - start[1]);
                    b = [b[0] + dx, b[1] + dy, b[2] + dx, b[3] + dy];
                    if let Some(b) = self.user_rect_screen(*place, b) {
                        out.push(Ov::Quad {
                            b,
                            fill: Some(accent.opacity(0.08)),
                            border: Some(accent),
                        });
                    }
                }
            }
            Some(Drag::Resize {
                place,
                id,
                handle,
                orig,
                now,
                free,
            }) => {
                let lock = self
                    .find_mark(&Sel {
                        place: *place,
                        id: id.clone(),
                    })
                    .is_some_and(|a| matches!(a.kind, Kind::Stamp { .. }))
                    && !free;
                let r = resized(*orig, *handle, *now, lock);
                if let Some(b) = self.user_rect_screen(*place, r) {
                    out.push(Ov::Quad {
                        b,
                        fill: Some(accent.opacity(0.08)),
                        border: Some(accent),
                    });
                }
            }
            None => {}
        }
        if let Some((place, pts)) = &self.markup.poly {
            let pts: Vec<_> = pts
                .iter()
                .filter_map(|p| self.user_point_screen(*place, *p))
                .collect();
            out.push(Ov::Poly {
                pts,
                closed: false,
                color,
                width: w,
            });
        }
        if let Some((place, pt)) = self.markup.stamp_drop
            && let Some(p) = self.user_point_screen(place, pt)
        {
            out.push(Ov::Quad {
                b: Bounds::new(p - point(px(40.), px(40.)), size(px(80.), px(80.))),
                fill: Some(accent.opacity(0.1)),
                border: Some(accent),
            });
        }
        out
    }

    /// Elements over the pages: the text editor, the field editor, a
    /// drop-down's options.
    pub(crate) fn markup_elements(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let t = kantor_ui::tokens(cx);
        let mut out = Vec::new();
        if let Some(e) = &self.markup.editor
            && let Some(a) = self.find_mark(&Sel {
                place: e.place,
                id: e.id.clone(),
            })
            && let Some(b) = self.user_rect_screen(e.place, a.bounds())
        {
            let note = matches!(a.kind, Kind::Note);
            let (x, y, w) = if note {
                (b.origin.x + b.size.width + px(6.), b.origin.y, px(240.))
            } else {
                (b.origin.x, b.origin.y, b.size.width.max(px(120.)))
            };
            out.push(
                div()
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(w)
                    .occlude()
                    .bg(t.surface)
                    .rounded(kantor_ui::metrics::RADIUS_SM)
                    .shadow_md()
                    .child(Textarea::new(&e.input).text_size(kantor_ui::metrics::FONT_BODY))
                    .into_any_element(),
            );
        }
        if let Some(f) = &self.markup.field
            && let Some(b) = self.user_rect_screen(f.place, f.rect)
        {
            let input = f.input.clone();
            let weak = cx.entity().downgrade();
            out.push(
                div()
                    .absolute()
                    .left(b.origin.x)
                    .top(b.origin.y)
                    .w(b.size.width.max(px(80.)))
                    .occlude()
                    .key_context(FIELD_CONTEXT)
                    .on_action({
                        let weak = weak.clone();
                        move |_: &crate::NextField, window, cx| {
                            let _ = weak.update(cx, |v, cx| v.tab_field(false, window, cx));
                        }
                    })
                    .on_action(move |_: &crate::PrevField, window, cx| {
                        let _ = weak.update(cx, |v, cx| v.tab_field(true, window, cx));
                    })
                    .child(gpui_kit::component::input::Input::new(&input).cleanable(false))
                    .into_any_element(),
            );
        }
        if let Some(c) = &self.markup.choice
            && let Some(b) = self.user_rect_screen(c.place, c.rect)
        {
            let mut list = gpui_kit::component::v_flex()
                .absolute()
                .left(b.origin.x)
                .top(b.origin.y + b.size.height)
                .min_w(b.size.width.max(px(120.)))
                .occlude()
                .bg(t.surface)
                .border_1()
                .border_color(t.border_strong)
                .rounded(kantor_ui::metrics::RADIUS_SM)
                .shadow_md()
                .py(px(4.));
            let cur = self.doc().and_then(|d| d.model.field_value(c.src, &c.name));
            for (i, (export, shown)) in c.options.iter().enumerate() {
                let chosen = matches!(&cur, Some(FieldValue::Choice(v)) if v.contains(export));
                let weak = cx.entity().downgrade();
                let (src, name, export) = (c.src, c.name.clone(), export.clone());
                list = list.child(
                    div()
                        .id(("pdf-choice", i))
                        .px(px(10.))
                        .py(px(4.))
                        .text_size(kantor_ui::metrics::FONT_BODY)
                        .when(chosen, |d| d.bg(t.selected))
                        .hover(|d| d.bg(t.hover))
                        .cursor_pointer()
                        .child(shown.clone())
                        .on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |v, cx| {
                                v.markup.choice = None;
                                v.set_field(
                                    src,
                                    name.clone(),
                                    FieldValue::Choice(vec![export.clone()]),
                                    cx,
                                );
                            });
                        }),
                );
            }
            out.push(list.into_any_element());
        }
        out
    }

    /// The cursor over the pages in markup modes.
    pub(crate) fn markup_cursor(&self) -> Option<CursorStyle> {
        match self.markup.tool {
            Tool::Select => self.markup.drag.as_ref().map(|_| CursorStyle::ClosedHand),
            Tool::Highlight | Tool::Underline | Tool::StrikeOut => Some(CursorStyle::IBeam),
            _ => Some(CursorStyle::Crosshair),
        }
    }
}

fn opt_ptr_eq<T>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// The box `orig` with handle `h` dragged to `to` (aspect kept when
/// `lock`). Handles: 0 top left, clockwise to 7 left middle (user space:
/// top is the larger y).
pub(crate) fn resized(orig: [f32; 4], h: Handle, to: [f32; 2], lock: bool) -> [f32; 4] {
    let [mut x0, mut y0, mut x1, mut y1] = orig;
    match h {
        0 => {
            x0 = to[0];
            y1 = to[1];
        }
        1 => y1 = to[1],
        2 => {
            x1 = to[0];
            y1 = to[1];
        }
        3 => x1 = to[0],
        4 => {
            x1 = to[0];
            y0 = to[1];
        }
        5 => y0 = to[1],
        6 => {
            x0 = to[0];
            y0 = to[1];
        }
        7 => x0 = to[0],
        _ => {}
    }
    let mut r = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];
    if r[2] - r[0] < 4.0 {
        r[2] = r[0] + 4.0;
    }
    if r[3] - r[1] < 4.0 {
        r[3] = r[1] + 4.0;
    }
    if lock {
        let aspect = (orig[2] - orig[0]) / (orig[3] - orig[1]).max(1e-3);
        let (w, h2) = (r[2] - r[0], r[3] - r[1]);
        // The dragged side leads; corners follow the wider change.
        let (nw, nh) = match h {
            1 | 5 => (h2 * aspect, h2),
            3 | 7 => (w, w / aspect),
            _ if w / aspect > h2 => (w, w / aspect),
            _ => (h2 * aspect, h2),
        };
        // Keep the corner opposite the handle where it was.
        let ax = if matches!(h, 0 | 6 | 7) {
            orig[2]
        } else {
            orig[0]
        };
        let ay = if matches!(h, 0..=2) { orig[1] } else { orig[3] };
        let (cx_mid, cy_mid) = ((orig[0] + orig[2]) / 2.0, (orig[1] + orig[3]) / 2.0);
        let (x0, x1) = match h {
            1 | 5 => (cx_mid - nw / 2.0, cx_mid + nw / 2.0),
            0 | 6 | 7 => (ax - nw, ax),
            _ => (ax, ax + nw),
        };
        let (y0, y1) = match h {
            3 | 7 => (cy_mid - nh / 2.0, cy_mid + nh / 2.0),
            0..=2 => (ay, ay + nh),
            _ => (ay - nh, ay),
        };
        r = [x0, y0, x1, y1];
    }
    r
}

/// Drop points closer than `tol` to the last one kept.
fn simplify(points: &[[f32; 2]], tol: f32) -> Vec<[f32; 2]> {
    let mut out: Vec<[f32; 2]> = Vec::with_capacity(points.len());
    for &p in points {
        match out.last() {
            Some(l) if (l[0] - p[0]).hypot(l[1] - p[1]) < tol => {}
            _ => out.push(p),
        }
    }
    if let (Some(&last), Some(&end)) = (out.last(), points.last())
        && last != end
    {
        out.push(end);
    }
    out
}

/// Today's date as the interface writes dates (`19/11/2026` in Indonesian,
/// `11/19/2026` in English).
pub(crate) fn today() -> String {
    let d = kantor_pdf::pdf_date_now();
    let s = d.trim_start_matches("D:");
    let (y, m, day) = (&s[0..4], &s[4..6], &s[6..8]);
    match i18n::lang() {
        i18n::Lang::Id => format!("{day}/{m}/{y}"),
        i18n::Lang::En => format!("{m}/{day}/{y}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn resize_keeps_aspect_for_stamps() {
        let r = resized([0.0, 0.0, 100.0, 50.0], 4, [200.0, -50.0], true);
        assert_eq!(r, [0.0, -50.0, 200.0, 50.0]);
        let r = resized([0.0, 0.0, 100.0, 50.0], 4, [200.0, 20.0], false);
        assert_eq!(r, [0.0, 20.0, 200.0, 50.0]);
        let r = resized([0.0, 0.0, 100.0, 50.0], 3, [50.0, 0.0], true);
        assert_eq!(r, [0.0, 12.5, 50.0, 37.5]);
    }

    #[std::prelude::v1::test]
    fn simplify_drops_close_points() {
        let pts = [[0.0, 0.0], [0.1, 0.0], [1.0, 0.0], [1.05, 0.0]];
        assert_eq!(
            simplify(&pts, 0.5),
            vec![[0.0, 0.0], [1.0, 0.0], [1.05, 0.0]]
        );
    }
}
