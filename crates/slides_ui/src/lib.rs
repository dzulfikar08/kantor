//! Kantor Slides UI: a `.pptx` editor view.
//!
//! The model is [`slides_engine::Presentation`]; text boxes are laid out
//! with [`docs_layout`] (parley) and painted through Docs' font bridge
//! (ADR 0001), so Slides and Docs share one text engine.
//!
//! Layout of the view: a toolbar, a virtualised thumbnail strip on the
//! left (click to go to a slide, drag to reorder), and the current slide
//! scaled to fit the canvas, centred with an even margin (Ctrl+= / Ctrl+-
//! or Ctrl+wheel zoom, Ctrl+0 fits again). On the canvas: click selects a shape, drag
//! moves it, the handles resize it, a second click (or typing, or Enter)
//! edits its text, the round handle above it rotates it (Shift snaps to
//! 15°). Under the canvas, a resizable speaker-notes pane ([`notes`]).
//! F5 presents in the pane, with the notes under the slide (S hides them).
//!
//! Text input, including IME composition, goes through GPUI's
//! [`EntityInputHandler`] (see [`input`]).

extern crate gpui_kit as gpui;

mod draw;
mod find;
mod fit;
mod input;
mod notes;
mod paint_menu;
mod toolbar;

use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use docs_engine::Toggle;
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::find_bar::{FindNext, FindPrev, OpenFind, OpenReplace};
use kantor_ui::tokens::Elevation;
use kantor_ui::{Tokens, metrics::*, tokens};
use slides_engine::{
    Focus, Geom, Layer, Position, Presentation, RunProps, Selection, ShapeKind, SlideSize, Xfrm,
    text_ops,
};

use draw::{
    Item, Painter, Surface, Turn, View, fill_polygon, paint_slide, stroke_polygon, text_origin,
    turned, turned_rect,
};
use fit::{
    canvas_margin, place_slide, pointer_degrees, resize_box, scroll_keeping, turned_handle_cursor,
};

/// Key context of the editor.
pub const KEY_CONTEXT: &str = "SlidesEditor";
/// Key context while presenting.
pub const PRESENTER_CONTEXT: &str = "SlidesPresenter";

actions!(
    slides,
    [
        Left,
        Right,
        Up,
        Down,
        ShiftLeft,
        ShiftRight,
        ShiftUp,
        ShiftDown,
        LineStart,
        LineEnd,
        SelectLineStart,
        SelectLineEnd,
        Backspace,
        Delete,
        Newline,
        LineBreak,
        Escape,
        NextShape,
        PrevShape,
        SelectAll,
        Copy,
        Cut,
        Paste,
        ToggleBold,
        ToggleItalic,
        ToggleUnderline,
        NewSlide,
        Duplicate,
        DuplicateSlide,
        DeleteSlide,
        MoveSlideUp,
        MoveSlideDown,
        InsertTextBox,
        InsertRectangle,
        InsertEllipse,
        InsertImage,
        ZoomIn,
        ZoomOut,
        ZoomFit,
        Undo,
        Redo,
        Save,
        SaveAs,
        Present,
        PresentFromCurrent,
        PresentNext,
        PresentPrev,
        PresentFirst,
        PresentLast,
        PresentExit,
        PresentToggleNotes,
        ToggleNotes,
    ]
);

/// Register Slides key bindings (Ctrl, and Super as Mac-style Cmd).
pub fn init(cx: &mut App) {
    kantor_ui::find_bar::bind_app_keys(KEY_CONTEXT, cx);
    let c = Some(KEY_CONTEXT);
    let p = Some(PRESENTER_CONTEXT);
    let mut keys = vec![
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("up", Up, c),
        KeyBinding::new("down", Down, c),
        KeyBinding::new("shift-left", ShiftLeft, c),
        KeyBinding::new("shift-right", ShiftRight, c),
        KeyBinding::new("shift-up", ShiftUp, c),
        KeyBinding::new("shift-down", ShiftDown, c),
        KeyBinding::new("home", LineStart, c),
        KeyBinding::new("end", LineEnd, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("enter", Newline, c),
        KeyBinding::new("shift-enter", LineBreak, c),
        KeyBinding::new("escape", Escape, c),
        KeyBinding::new("tab", NextShape, c),
        KeyBinding::new("shift-tab", PrevShape, c),
        KeyBinding::new("f5", Present, c),
        KeyBinding::new("shift-f5", PresentFromCurrent, c),
        // Presenting.
        KeyBinding::new("right", PresentNext, p),
        KeyBinding::new("down", PresentNext, p),
        KeyBinding::new("space", PresentNext, p),
        KeyBinding::new("enter", PresentNext, p),
        KeyBinding::new("pagedown", PresentNext, p),
        KeyBinding::new("n", PresentNext, p),
        KeyBinding::new("left", PresentPrev, p),
        KeyBinding::new("up", PresentPrev, p),
        KeyBinding::new("pageup", PresentPrev, p),
        KeyBinding::new("backspace", PresentPrev, p),
        KeyBinding::new("p", PresentPrev, p),
        KeyBinding::new("home", PresentFirst, p),
        KeyBinding::new("end", PresentLast, p),
        KeyBinding::new("escape", PresentExit, p),
        KeyBinding::new("f5", PresentExit, p),
        KeyBinding::new("s", PresentToggleNotes, p),
    ];
    macro_rules! primary {
        ($($key:literal => $action:expr),* $(,)?) => {
            keys.extend([$(
                KeyBinding::new(kantor_ui::keys::primary(concat!("ctrl-", $key)), $action, c),
                KeyBinding::new(concat!("super-", $key), $action, c),
            )*]);
        };
    }
    primary!(
        "a" => SelectAll,
        "c" => Copy,
        "x" => Cut,
        "v" => Paste,
        "b" => ToggleBold,
        "i" => ToggleItalic,
        "u" => ToggleUnderline,
        "m" => NewSlide,
        "d" => Duplicate,
        "up" => MoveSlideUp,
        "down" => MoveSlideDown,
        "z" => Undo,
        "shift-z" => Redo,
        "y" => Redo,
        "s" => Save,
        "shift-s" => SaveAs,
        "=" => ZoomIn,
        "+" => ZoomIn,
        "shift-=" => ZoomIn,
        "-" => ZoomOut,
        "0" => ZoomFit,
    );
    cx.bind_keys(keys);
}

/// Shapes copied in any Slides view (so they paste into another
/// presentation), and the clipboard text written with them: a paste uses
/// the shapes while the system clipboard still holds that text.
struct ShapeClipboard {
    clip: slides_engine::ShapeClip,
    written: String,
}

impl Global for ShapeClipboard {}

/// Facts for the inspector.
pub struct SlidesInfo {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub slides: usize,
    pub current: usize,
    pub layout: String,
    /// Selected shape: name, kind and position/size in EMU.
    pub shape: Option<(String, String, Xfrm)>,
    pub unsupported: Vec<String>,
}

/// Text being edited in a shape of the current slide.
#[derive(Clone, Debug)]
struct Editing {
    shape: usize,
    sel: Selection,
    /// IME composition range in the caret's paragraph.
    marked: Option<Range<usize>>,
    /// Formatting for the next typed text (Ctrl+B with no selection).
    typing_props: Option<RunProps>,
    goal_x: Option<f32>,
}

/// Which edges a resize handle moves: -1 left/top, 1 right/bottom, 0 none.
type Handle = (i8, i8);

const HANDLES: [Handle; 8] = [
    (-1, -1),
    (0, -1),
    (1, -1),
    (1, 0),
    (1, 1),
    (0, 1),
    (-1, 1),
    (-1, 0),
];

enum Drag {
    Move {
        shape: usize,
        start: Point<Pixels>,
        orig: Xfrm,
        moved: bool,
        was_selected: bool,
    },
    Resize {
        shape: usize,
        handle: Handle,
        start: Point<Pixels>,
        orig: Xfrm,
    },
    Rotate {
        shape: usize,
        orig: Xfrm,
    },
    Text,
}

/// Clipboard metadata marking text Kantor wrote for copied shapes.
const SHAPES_METADATA: &str = "kantor-slides-shapes";

/// Distance of the rotate handle above the selected shape's top edge.
const ROTATE_GAP: Pixels = SPACE_8;

/// A Slides document view.
pub struct SlidesView {
    pres: Presentation,
    /// Name shown while the presentation has no file (`Untitled 2.pptx`), given
    /// by the shell so new documents are numbered across the session.
    untitled_name: Option<String>,
    current: usize,
    selected: Option<usize>,
    editing: Option<Editing>,
    painter: Rc<Painter>,
    /// Canvas bounds from the last paint.
    viewport: Rc<Cell<Bounds<Pixels>>>,
    drag: Option<Drag>,
    /// Live position of a shape being dragged (committed on mouse up).
    preview: Option<(usize, Xfrm)>,
    /// Thumbnail being dragged to a new place, and where it would land.
    strip_drag: Option<usize>,
    strip_drop: Option<usize>,
    strip_scroll: UniformListScrollHandle,
    /// The strip was clicked last (Delete then removes the slide).
    strip_focused: bool,
    presenting: Option<usize>,
    /// Canvas zoom relative to fit (1.0 = the slide fits the pane).
    zoom: f32,
    /// Canvas scroll in screen px, used only while zoomed past the pane.
    scroll: Point<f32>,
    /// Pointer shape for what the mouse is over (handle, shape, text).
    hover_cursor: CursorStyle,
    focus: FocusHandle,
    status: SharedString,
    /// Find & Replace (Ctrl+F / Ctrl+H).
    find: find::SlidesFind,
    notes: notes::NotesPane,
    paint_menu: Option<paint_menu::PaintMenu>,
    paint_menu_closed: Option<std::time::Instant>,
    /// Speaker notes under the slide while presenting.
    present_notes: bool,
}

fn cm(emu: i64) -> String {
    format!("{:.2} cm", emu as f64 / 360_000.0)
}

impl SlidesView {
    pub fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (pres, status) = match path.as_deref().map(Presentation::open) {
            Some(Ok(p)) => {
                let mut s = format!("Opened {}", p.title());
                if p.has_unsupported_content() {
                    s.push_str(" (has unsupported content; Save creates a copy)");
                }
                (p, s)
            }
            Some(Err(err)) => {
                log::error!("{err:#}");
                (Presentation::new(), format!("{err:#}"))
            }
            None => (Presentation::new(), String::new()),
        };
        let (engine, bridge) = docs_ui::shared_text(cx);
        let find = find::SlidesFind::new(window, cx);
        let notes = notes::NotesPane::new(window, cx);
        Self {
            pres,
            untitled_name: None,
            current: 0,
            selected: None,
            editing: None,
            painter: Rc::new(Painter::new(engine, bridge)),
            viewport: Rc::default(),
            drag: None,
            preview: None,
            strip_drag: None,
            strip_drop: None,
            strip_scroll: UniformListScrollHandle::new(),
            strip_focused: false,
            presenting: None,
            zoom: 1.0,
            scroll: point(0., 0.),
            hover_cursor: CursorStyle::Arrow,
            focus: cx.focus_handle(),
            status: status.into(),
            find,
            notes,
            paint_menu: None,
            paint_menu_closed: None,
            present_notes: true,
        }
    }

    // ── geometry ───────────────────────────────────────────────────────────

    /// The slide's placement on the canvas (mapping, screen rect, scroll).
    fn placement(&self) -> (View, Bounds<Pixels>, Point<f32>) {
        let b = self.viewport.get();
        place_slide(
            b,
            self.pres.size,
            canvas_margin(b.size),
            self.zoom,
            self.scroll,
        )
    }

    fn view(&self) -> View {
        self.placement().0
    }

    /// Zoom to `zoom` × fit, keeping the slide point under `at` (the
    /// pointer; the pane's centre when `None`) where it is.
    fn set_zoom(&mut self, zoom: f32, at: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        if (zoom - self.zoom).abs() < 1e-4 {
            return;
        }
        let b = self.viewport.get();
        let at = at.unwrap_or_else(|| b.center());
        let v = self.view();
        let emu = (
            f32::from(at.x - v.origin.x) / v.scale,
            f32::from(at.y - v.origin.y) / v.scale,
        );
        // The scale is fit × zoom, so it changes by the zoom ratio.
        let scale = v.scale * zoom / self.zoom;
        self.zoom = zoom;
        self.scroll = scroll_keeping(b, canvas_margin(b.size), scale, emu, at);
        self.scroll = self.placement().2;
        cx.notify();
    }

    /// Back to fit (Ctrl+0).
    pub fn zoom_to_fit(&mut self, cx: &mut Context<Self>) {
        self.zoom = 1.0;
        self.scroll = point(0., 0.);
        cx.notify();
    }

    pub fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom * factor, None, cx);
    }

    /// Canvas zoom relative to fit.
    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(window.line_height());
        if ev.modifiers.control || ev.modifiers.platform {
            let dy = f32::from(delta.y);
            if dy != 0.0 {
                let f = if dy > 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP };
                self.set_zoom(self.zoom * f, Some(ev.position), cx);
            }
            return;
        }
        let (mut dx, mut dy) = (f32::from(delta.x), f32::from(delta.y));
        if ev.modifiers.shift && dx == 0.0 {
            (dx, dy) = (dy, 0.0);
        }
        let before = self.scroll;
        self.scroll = point(self.scroll.x - dx, self.scroll.y - dy);
        self.scroll = self.placement().2;
        if self.scroll != before {
            cx.notify();
        }
    }

    fn shape_xfrm(&self, i: usize) -> Option<Xfrm> {
        if let Some((j, x)) = self.preview
            && i == j
        {
            return Some(x);
        }
        let shape = self.pres.slide(self.current).shapes.get(i)?;
        self.pres.effective_xfrm(self.current, shape)
    }

    /// The laid-out text of shape `i` (no prompt), with its box.
    fn laid(&self, i: usize) -> Option<(Xfrm, draw::LaidText)> {
        let xfrm = self.shape_xfrm(i)?;
        let list = self.pres.render_list(self.current, false);
        let r = list.iter().find(|r| r.layer == Layer::Slide(i))?;
        let t = r.text.as_ref()?;
        Some((xfrm, self.painter.place(t, &xfrm)))
    }

    /// Text position under a screen point, in shape `i`.
    fn text_hit(&self, i: usize, p: Point<Pixels>) -> Option<Position> {
        let (xfrm, t) = self.laid(i)?;
        let v = self.view();
        // Rotated text: measure in the shape's own frame.
        let p = Turn::of(&xfrm, v).map_or(p, |t| t.unapply(p));
        let o = text_origin(&t, &xfrm, v);
        let s = v.px_scale();
        let x = f32::from(p.x - o.x) / s;
        let y = f32::from(p.y - o.y) / s;
        let (para, offset) = t.layout.paged.hit_test(0, x, y)?;
        let paras = self.pres.text_paras((self.current, i))?;
        Some(text_ops::clamp(paras, Position::new(para, offset)))
    }

    /// Topmost slide shape under a screen point.
    fn shape_at(&self, p: Point<Pixels>) -> Option<usize> {
        let (x, y) = self.view().to_emu(p);
        let slide = self.pres.slide(self.current);
        (0..slide.shapes.len())
            .rev()
            .find(|&i| self.shape_xfrm(i).is_some_and(|b| b.contains(x, y)))
    }

    fn handle_bounds(&self, xfrm: &Xfrm, h: Handle) -> Bounds<Pixels> {
        let v = self.view();
        Bounds::centered_at(handle_point(v, xfrm, h), size(SLIDE_HANDLE, SLIDE_HANDLE))
    }

    /// The selected shape if the pointer is on its rotate handle.
    fn rotate_handle_at(&self, p: Point<Pixels>) -> Option<usize> {
        let i = self.selected?;
        let shape = self.pres.slide(self.current).shapes.get(i)?;
        if self.editing.is_some() || !shape.movable() {
            return None;
        }
        let xfrm = self.shape_xfrm(i)?;
        let c = rotate_handle_point(self.view(), &xfrm);
        Bounds::centered_at(c, size(SLIDE_HANDLE, SLIDE_HANDLE))
            .dilate(px(4.))
            .contains(&p)
            .then_some(i)
    }

    fn handle_at(&self, p: Point<Pixels>) -> Option<(usize, Handle)> {
        let i = self.selected?;
        if self.editing.is_some() || !self.pres.slide(self.current).shapes.get(i)?.movable() {
            return None;
        }
        let xfrm = self.shape_xfrm(i)?;
        HANDLES
            .iter()
            .find(|h| self.handle_bounds(&xfrm, **h).dilate(px(3.)).contains(&p))
            .map(|h| (i, *h))
    }

    // ── state changes ──────────────────────────────────────────────────────

    fn set_status(&mut self, s: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = s.into();
        cx.notify();
    }

    fn report<T>(&mut self, r: anyhow::Result<T>, cx: &mut Context<Self>) -> Option<T> {
        match r {
            Ok(v) => Some(v),
            Err(e) => {
                self.set_status(format!("{e:#}"), cx);
                None
            }
        }
    }

    fn after_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.status = SharedString::default();
        self.update_title(window, cx);
        cx.notify();
    }

    fn go_to(&mut self, slide: usize, cx: &mut Context<Self>) {
        let n = self.pres.len();
        if n == 0 {
            return;
        }
        let slide = slide.min(n - 1);
        if slide != self.current {
            self.pres.seal_history();
        }
        self.current = slide;
        self.selected = None;
        self.editing = None;
        self.preview = None;
        self.drag = None;
        self.strip_scroll
            .scroll_to_item(slide, ScrollStrategy::Center);
        cx.notify();
    }

    /// Show where an undo/redo happened.
    fn apply_focus(&mut self, f: Focus, cx: &mut Context<Self>) {
        let n = self.pres.len();
        self.current = f.slide.min(n.saturating_sub(1));
        let shapes = self.pres.slide(self.current).shapes.len();
        self.selected = f.shape.filter(|&s| s < shapes);
        self.editing = match (self.selected, f.sel) {
            (Some(shape), Some(sel)) => {
                let paras = self.pres.text_paras((self.current, shape));
                paras.map(|p| Editing {
                    shape,
                    sel: Selection {
                        anchor: text_ops::clamp(p, sel.anchor),
                        head: text_ops::clamp(p, sel.head),
                    },
                    marked: None,
                    typing_props: None,
                    goal_x: None,
                })
            }
            _ => None,
        };
        self.strip_scroll
            .scroll_to_item(self.current, ScrollStrategy::Center);
        cx.notify();
    }

    fn select(&mut self, shape: Option<usize>, cx: &mut Context<Self>) {
        if self.editing.as_ref().map(|e| e.shape) != shape {
            self.editing = None;
            self.pres.seal_history();
        }
        self.selected = shape;
        self.strip_focused = false;
        cx.notify();
    }

    /// Start editing shape `i`'s text with the caret at `pos` (end if `None`).
    fn begin_editing(&mut self, i: usize, pos: Option<Position>, cx: &mut Context<Self>) -> bool {
        let r = (self.current, i);
        let slide = self.pres.slide(self.current);
        let Some(shape) = slide.shapes.get(i) else {
            return false;
        };
        if let Some(why) = &slide.locked {
            self.set_status(format!("This slide can't be edited: {why}"), cx);
            return false;
        }
        if let Some(why) = shape.text.as_ref().and_then(|t| t.locked.clone()) {
            self.set_status(format!("This text can't be edited yet: it has {why}"), cx);
            return false;
        }
        if !shape.text_editable() {
            return false;
        }
        let paras = self.pres.text_paras(r).unwrap_or_default();
        let pos = pos.unwrap_or_else(|| text_ops::end(paras));
        self.selected = Some(i);
        self.editing = Some(Editing {
            shape: i,
            sel: Selection::caret(pos),
            marked: None,
            typing_props: None,
            goal_x: None,
        });
        self.pres.seal_history();
        cx.notify();
        true
    }

    // ── editing text ───────────────────────────────────────────────────────

    fn text_edit(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Presentation, (usize, usize), &mut Editing) -> anyhow::Result<Selection>,
    ) {
        let Some(mut e) = self.editing.clone() else {
            return;
        };
        let r = (self.current, e.shape);
        let result = f(&mut self.pres, r, &mut e);
        if let Some(sel) = self.report(result, cx) {
            e.sel = sel;
            e.goal_x = None;
            self.editing = Some(e);
            self.after_edit(window, cx);
        }
    }

    fn insert(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_none() {
            let Some(i) = self.selected else {
                return;
            };
            if !self.begin_editing(i, None, cx) {
                return;
            }
        }
        let text = text.to_owned();
        self.text_edit(window, cx, |p, r, e| {
            let props = e.typing_props.take();
            e.marked = None;
            p.insert_text(r, e.sel, &text, props)
        });
    }

    fn toggle(&mut self, toggle: Toggle, window: &mut Window, cx: &mut Context<Self>) {
        let Some(e) = self.editing.clone() else {
            self.set_status("Select text to format it", cx);
            return;
        };
        let r = (self.current, e.shape);
        if e.sel.is_empty() {
            let mut props = e
                .typing_props
                .clone()
                .unwrap_or_else(|| self.pres.run_props_at(r, e.sel.head));
            let on = !self.is_on(toggle);
            slides_engine::set_toggle(&mut props, toggle, on);
            if let Some(ed) = &mut self.editing {
                ed.typing_props = Some(props);
            }
            cx.notify();
            return;
        }
        let res = self.pres.toggle(r, e.sel, toggle);
        if self.report(res, cx).is_some() {
            self.after_edit(window, cx);
        }
    }

    /// Whether `toggle` is on at the caret / for the selected text.
    pub fn is_on(&self, toggle: Toggle) -> bool {
        let Some(e) = &self.editing else {
            return false;
        };
        let r = (self.current, e.shape);
        if e.sel.is_empty()
            && let Some(props) = &e.typing_props
        {
            let shape = &self.pres.slide(self.current).shapes[e.shape];
            let run = self
                .pres
                .resolve_run(self.current, shape, e.sel.head.para, props);
            return match toggle {
                Toggle::Bold => run.bold,
                Toggle::Italic => run.italic,
                Toggle::Underline => run.underline,
            };
        }
        self.pres.is_on(r, e.sel, toggle)
    }

    fn move_caret(&mut self, pos: Position, extend: bool, cx: &mut Context<Self>) {
        if let Some(e) = &mut self.editing {
            if extend {
                e.sel.head = pos;
            } else {
                e.sel = Selection::caret(pos);
            }
            e.typing_props = None;
            e.marked = None;
        }
        self.pres.seal_history();
        cx.notify();
    }

    fn caret_horizontal(&mut self, forward: bool, extend: bool, cx: &mut Context<Self>) {
        let Some(e) = self.editing.clone() else {
            return;
        };
        let Some(paras) = self.pres.text_paras((self.current, e.shape)) else {
            return;
        };
        if !extend && !e.sel.is_empty() {
            let (a, b) = e.sel.ordered();
            self.move_caret(if forward { b } else { a }, false, cx);
            return;
        }
        let h = text_ops::clamp(paras, e.sel.head);
        let text = &paras[h.para].text;
        let pos = if forward {
            if h.offset < text.len() {
                Position::new(h.para, docs_engine::model::next_grapheme(text, h.offset))
            } else if h.para + 1 < paras.len() {
                Position::new(h.para + 1, 0)
            } else {
                h
            }
        } else if h.offset > 0 {
            Position::new(h.para, docs_engine::model::prev_grapheme(text, h.offset))
        } else if h.para > 0 {
            Position::new(h.para - 1, paras[h.para - 1].text.len())
        } else {
            h
        };
        self.move_caret(pos, extend, cx);
    }

    fn caret_vertical(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        let Some(e) = self.editing.clone() else {
            return;
        };
        let Some((_, t)) = self.laid(e.shape) else {
            return;
        };
        let paged = &t.layout.paged;
        let h = e.sel.head;
        let x = match e.goal_x {
            Some(x) => x,
            None => match paged.caret(h.para, h.offset) {
                Some(c) => c.x,
                None => return,
            },
        };
        let paras = self
            .pres
            .text_paras((self.current, e.shape))
            .unwrap_or_default();
        let pos = match paged.vertical(h.para, h.offset, x, down) {
            Some((para, offset)) => Position::new(para, offset),
            None if down => text_ops::end(paras),
            None => Position::default(),
        };
        self.move_caret(pos, extend, cx);
        if let Some(e) = &mut self.editing {
            e.goal_x = Some(x);
        }
    }

    fn caret_line_edge(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        let Some(e) = self.editing.clone() else {
            return;
        };
        let Some((_, t)) = self.laid(e.shape) else {
            return;
        };
        let h = e.sel.head;
        let Some((range, soft)) = t.layout.paged.line_range(h.para, h.offset) else {
            return;
        };
        let Some(paras) = self.pres.text_paras((self.current, e.shape)) else {
            return;
        };
        let text = &paras[h.para].text;
        let offset = if !end {
            range.start
        } else {
            let mut e = range.end.min(text.len());
            if soft {
                e = text[..e].trim_end_matches(' ').len().max(range.start);
            } else if text[..e].ends_with(slides_engine::LINE_BREAK) {
                e -= slides_engine::LINE_BREAK.len_utf8();
            }
            e
        };
        self.move_caret(Position::new(h.para, offset), extend, cx);
    }

    // ── keyboard commands (one key, several meanings) ──────────────────────

    fn arrow(&mut self, dx: i8, dy: i8, shift: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            match (dx, dy) {
                (1, _) => self.caret_horizontal(true, shift, cx),
                (-1, _) => self.caret_horizontal(false, shift, cx),
                (_, 1) => self.caret_vertical(true, shift, cx),
                _ => self.caret_vertical(false, shift, cx),
            }
            return;
        }
        if let Some(i) = self.selected {
            let Some(x) = self.shape_xfrm(i) else {
                return;
            };
            let step = SLIDE_NUDGE_EMU * if shift { 10 } else { 1 };
            let moved = Xfrm {
                x: x.x + dx as i64 * step,
                y: x.y + dy as i64 * step,
                ..x
            };
            let res = self.pres.set_xfrm((self.current, i), moved, true);
            if self.report(res, cx).is_some() {
                self.after_edit(window, cx);
            }
            return;
        }
        let next = if dx + dy > 0 {
            self.current + 1
        } else {
            self.current.saturating_sub(1)
        };
        self.go_to(next, cx);
    }

    fn escape(&mut self, cx: &mut Context<Self>) {
        if self.editing.take().is_some() {
            self.pres.seal_history();
        } else {
            self.selected = None;
        }
        cx.notify();
    }

    fn delete_key(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.text_edit(window, cx, |p, r, e| {
                e.marked = None;
                if forward {
                    p.delete_forward(r, e.sel)
                } else {
                    p.backspace(r, e.sel)
                }
            });
            return;
        }
        if let Some(i) = self.selected {
            let res = self.pres.delete_shape((self.current, i));
            if self.report(res, cx).is_some() {
                self.selected = None;
                self.after_edit(window, cx);
            }
            return;
        }
        if self.strip_focused {
            self.delete_slide_doc(window, cx);
        }
    }

    fn enter(&mut self, line_break: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            let text = if line_break {
                slides_engine::LINE_BREAK.to_string()
            } else {
                "\n".into()
            };
            self.insert(&text, window, cx);
        } else if let Some(i) = self.selected {
            self.begin_editing(i, None, cx);
        }
    }

    /// Tab: in text, a level deeper (at the start of a paragraph, in a
    /// list or over several paragraphs; else a tab character); otherwise
    /// the next shape. Shift+Tab: a level up, or the previous shape.
    fn tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.notes.focused {
            return;
        }
        if let Some(e) = self.editing.clone() {
            let r = (self.current, e.shape);
            let (a, b) = e.sel.ordered();
            let in_list = self.pres.is_list(r, e.sel, false) || self.pres.is_list(r, e.sel, true);
            if !back && a.para == b.para && a.offset > 0 && !in_list {
                self.insert("\t", window, cx);
                return;
            }
            let res = self.pres.change_level(r, e.sel, if back { -1 } else { 1 });
            if self.report(res, cx).is_some() {
                self.after_edit(window, cx);
            }
            return;
        }
        let n = self.pres.slide(self.current).shapes.len();
        if n == 0 {
            return;
        }
        let next = match (self.selected, back) {
            (None, false) => 0,
            (None, true) => n - 1,
            (Some(i), false) => (i + 1) % n,
            (Some(i), true) => (i + n - 1) % n,
        };
        self.select(Some(next), cx);
    }

    /// Bulleted (`numbered` false) or numbered list on the paragraphs being
    /// edited, or all paragraphs of the selected shape.
    pub fn toggle_list_doc(&mut self, numbered: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some((r, sel)) = self.list_target() else {
            self.set_status("Select a text box or placeholder to make a list", cx);
            return;
        };
        let res = self.pres.toggle_list(r, sel, numbered);
        if self.report(res, cx).is_some() {
            self.after_edit(window, cx);
        }
    }

    /// The text a list toggle applies to: the edited selection, or the
    /// whole text of the selected shape.
    fn list_target(&self) -> Option<((usize, usize), Selection)> {
        if let Some(e) = &self.editing {
            return Some(((self.current, e.shape), e.sel));
        }
        let i = self.selected?;
        let r = (self.current, i);
        let shape = self.pres.slide(self.current).shapes.get(i)?;
        if !shape.text_editable() {
            return None;
        }
        let paras = self.pres.text_paras(r)?;
        Some((
            r,
            Selection {
                anchor: Position::default(),
                head: text_ops::end(paras),
            },
        ))
    }

    /// Whether the list toggle for `numbered` shows as on.
    pub fn is_list(&self, numbered: bool) -> bool {
        self.list_target()
            .is_some_and(|(r, sel)| self.pres.is_list(r, sel, numbered))
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        if let Some(e) = &mut self.editing
            && let Some(paras) = self.pres.text_paras((self.current, e.shape))
        {
            e.sel = Selection {
                anchor: Position::default(),
                head: text_ops::end(paras),
            };
            cx.notify();
        }
    }

    fn selected_text(&self) -> Option<String> {
        let e = self.editing.as_ref()?;
        let (a, b) = e.sel.ordered();
        let paras = self.pres.text_paras((self.current, e.shape))?;
        (a != b).then(|| text_ops::text_in(paras, a, b))
    }

    // ── shell API (tabs, status bar, inspector, palette, close prompt) ─────

    pub fn copy_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            if let Some(t) = self.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(t));
            }
            return;
        }
        self.copy_shape(cx);
    }

    /// Copy the selected shape (as shapes for Kantor, as text elsewhere).
    fn copy_shape(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(i) = self.selected else {
            return false;
        };
        let res = self.pres.copy_shapes(self.current, &[i]);
        let Some(clip) = self.report(res, cx) else {
            return false;
        };
        let written = if clip.text.trim().is_empty() {
            format!(
                "[{} shape{}]",
                clip.len(),
                if clip.len() == 1 { "" } else { "s" }
            )
        } else {
            clip.text.clone()
        };
        cx.write_to_clipboard(ClipboardItem::new_string_with_metadata(
            written.clone(),
            SHAPES_METADATA.into(),
        ));
        cx.set_global(ShapeClipboard { clip, written });
        true
    }

    pub fn cut_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            if let Some(t) = self.selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(t));
                self.delete_key(false, window, cx);
            }
            return;
        }
        if self.copy_shape(cx) {
            self.delete_key(false, window, cx);
        }
    }

    pub fn paste_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let text = item.text();
        // Copied shapes, if the clipboard still holds what Kantor put there.
        let shapes = (self.editing.is_none() && !self.pres.is_empty())
            .then(|| cx.try_global::<ShapeClipboard>())
            .flatten()
            .filter(|g| {
                item.metadata().is_some_and(|m| m == SHAPES_METADATA)
                    && text.as_deref() == Some(g.written.as_str())
            })
            .map(|g| g.clip.clone());
        if let Some(clip) = shapes {
            let res = self.pres.paste_shapes(self.current, &clip);
            if let Some(added) = self.report(res, cx) {
                self.select(added.last().copied(), cx);
                self.after_edit(window, cx);
            }
            return;
        }
        if let Some(text) = text {
            self.insert(&text, window, cx);
        }
    }

    /// Ctrl+D: duplicate the selected shape, else the slide.
    pub fn duplicate_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.notes.focused {
            return;
        }
        match (self.selected, &self.editing) {
            (Some(i), None) => {
                let res = self.pres.duplicate_shape((self.current, i));
                if let Some(j) = self.report(res, cx) {
                    self.select(Some(j), cx);
                    self.after_edit(window, cx);
                }
            }
            _ => self.duplicate_slide_doc(window, cx),
        }
    }

    pub fn toggle_doc(&mut self, toggle: Toggle, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle(toggle, window, cx);
    }

    pub fn undo_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        match self.pres.undo() {
            Some(f) => self.apply_focus(f, cx),
            None => self.set_status("Nothing to undo", cx),
        }
    }

    pub fn redo_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        match self.pres.redo() {
            Some(f) => self.apply_focus(f, cx),
            None => self.set_status("Nothing to redo", cx),
        }
    }

    pub fn new_slide_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let after = (!self.pres.is_empty()).then_some(self.current);
        let res = self.pres.new_slide(after);
        if let Some(i) = self.report(res, cx) {
            self.go_to(i, cx);
            self.after_edit(window, cx);
        }
    }

    pub fn duplicate_slide_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let res = self.pres.duplicate_slide(self.current);
        if let Some(i) = self.report(res, cx) {
            self.go_to(i, cx);
            self.after_edit(window, cx);
        }
    }

    pub fn delete_slide_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pres.is_empty() {
            return;
        }
        let res = self.pres.delete_slide(self.current);
        if self.report(res, cx).is_some() {
            // The slide index stays; it now shows the next slide.
            let next = self.current.min(self.pres.len().saturating_sub(1));
            self.selected = None;
            self.editing = None;
            self.current = next;
            self.strip_scroll
                .scroll_to_item(next, ScrollStrategy::Center);
            self.after_edit(window, cx);
        }
    }

    pub fn move_slide_doc(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        let to = if up {
            match self.current.checked_sub(1) {
                Some(t) => t,
                None => return,
            }
        } else {
            self.current + 1
        };
        if to >= self.pres.len() {
            return;
        }
        let res = self.pres.move_slide(self.current, to);
        if self.report(res, cx).is_some() {
            self.go_to(to, cx);
            self.after_edit(window, cx);
        }
    }

    fn inserted(
        &mut self,
        res: anyhow::Result<usize>,
        edit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(i) = self.report(res, cx) {
            self.select(Some(i), cx);
            if edit {
                self.begin_editing(i, None, cx);
            }
            self.after_edit(window, cx);
        }
    }

    pub fn insert_text_box_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let res = self.pres.add_text_box(self.current);
        self.inserted(res, true, window, cx);
    }

    pub fn insert_shape_doc(&mut self, geom: Geom, window: &mut Window, cx: &mut Context<Self>) {
        let res = self.pres.add_geometry(self.current, geom);
        self.inserted(res, false, window, cx);
    }

    pub fn insert_image_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Insert picture (PNG or JPEG)".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| {
                let res = std::fs::read(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|bytes| this.pres.add_picture(this.current, bytes));
                this.inserted(res, false, window, cx);
            });
        })
        .detach();
    }

    pub fn present_doc(&mut self, from_start: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.pres.is_empty() {
            return;
        }
        self.editing = None;
        self.presenting = Some(if from_start { 0 } else { self.current });
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn present_step(&mut self, delta: i64, cx: &mut Context<Self>) {
        let Some(cur) = self.presenting else {
            return;
        };
        let next = cur as i64 + delta;
        if next < 0 {
            return;
        }
        if next as usize >= self.pres.len() {
            self.stop_presenting(cx);
            return;
        }
        self.presenting = Some(next as usize);
        cx.notify();
    }

    fn stop_presenting(&mut self, cx: &mut Context<Self>) {
        if let Some(p) = self.presenting.take() {
            self.go_to(p, cx);
        }
        cx.notify();
    }

    pub fn is_presenting(&self) -> bool {
        self.presenting.is_some()
    }

    pub fn doc_name(&self, _: &App) -> String {
        self.name()
    }

    /// Set the name shown while the presentation has no file.
    pub fn set_untitled_name(&mut self, name: String, cx: &mut Context<Self>) {
        self.untitled_name = Some(name);
        cx.notify();
    }

    /// The file name, or the shell-given name while there is no file.
    fn name(&self) -> String {
        match &self.untitled_name {
            Some(name) if self.pres.path().is_none() => name.clone(),
            _ => self.pres.title(),
        }
    }

    pub fn path(&self, _: &App) -> Option<PathBuf> {
        self.pres.path().map(|p| p.to_path_buf())
    }

    pub fn is_dirty(&self, _: &App) -> bool {
        self.pres.is_dirty()
    }

    pub fn title(&self, _: &App) -> String {
        let dirty = if self.pres.is_dirty() { "• " } else { "" };
        format!("{dirty}{} — Kantor Slides", self.name())
    }

    fn update_title(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        window.set_window_title(&title);
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    pub fn status(&self) -> SharedString {
        self.status.clone()
    }

    /// For the status bar: `Slide 2 of 12`.
    pub fn location(&self) -> String {
        format!("Slide {} of {}", self.current + 1, self.pres.len())
    }

    pub fn info(&self) -> SlidesInfo {
        let shape = self.selected.and_then(|i| {
            let s = self.pres.slide(self.current).shapes.get(i)?;
            let kind = match &s.kind {
                ShapeKind::Shape { text_box: true, .. } => "Text box".to_owned(),
                ShapeKind::Shape { .. } if s.ph.is_some() => "Placeholder".to_owned(),
                ShapeKind::Shape { geom, .. } => match geom {
                    Geom::Ellipse => "Ellipse".into(),
                    Geom::Rect => "Rectangle".into(),
                    Geom::RoundRect => "Rounded rectangle".into(),
                    Geom::Line => "Line".into(),
                    Geom::Other(o) => format!("Shape ({o})"),
                },
                ShapeKind::Picture { .. } => "Picture".into(),
                ShapeKind::Other { what } => what.clone(),
            };
            Some((s.name.clone(), kind, self.shape_xfrm(i)?))
        });
        SlidesInfo {
            name: self.name(),
            path: self.pres.path().map(|p| p.to_path_buf()),
            dirty: self.pres.is_dirty(),
            slides: self.pres.len(),
            current: self.current,
            layout: if self.pres.is_empty() {
                String::new()
            } else {
                self.pres.layout_of(self.current).name.clone()
            },
            shape,
            unsupported: self.pres.unsupported_content(),
        }
    }

    /// Position and size as text (for the inspector).
    pub fn format_length(emu: i64) -> String {
        cm(emu)
    }

    // ── saving ─────────────────────────────────────────────────────────────

    pub fn save_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pres.needs_save_as() {
            self.save_as_task(window, cx).detach();
            return;
        }
        let result = self.pres.save();
        self.after_save(result, window, cx);
    }

    pub fn save_doc_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as_task(window, cx).detach();
    }

    /// Save before closing; resolves to whether the presentation is saved.
    pub fn save_for_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if self.pres.needs_save_as() {
            return self.save_as_task(window, cx);
        }
        self.save_doc(window, cx);
        Task::ready(!self.pres.is_dirty())
    }

    fn save_as_task(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if self.pres.has_unsupported_content() {
            self.status = format!(
                "{} has content Kantor can't keep ({}), so it's saved as a new file; the original stays as it is",
                self.pres.title(),
                self.pres.unsupported_content().join(", ")
            )
            .into();
            cx.notify();
        }
        let dir = self
            .pres
            .path()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let name = if self.pres.path().is_none() {
            self.name()
        } else {
            self.pres.suggested_file_name()
        };
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = path.await else {
                return false;
            };
            if path.extension().is_none() {
                path.set_extension("pptx");
            }
            this.update_in(cx, |this, window, cx| {
                let result = this.pres.save_as(&path);
                this.after_save(result, window, cx);
                !this.pres.is_dirty()
            })
            .unwrap_or(false)
        })
    }

    fn after_save(
        &mut self,
        result: anyhow::Result<()>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.status = match result {
            Ok(()) => format!("Saved {}", self.pres.title()).into(),
            Err(err) => format!("Save failed: {err:#}").into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }

    // ── mouse ──────────────────────────────────────────────────────────────

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.strip_focused = false;
        if self.pres.is_empty() {
            return;
        }
        let p = ev.position;
        if let Some(shape) = self.rotate_handle_at(p)
            && let Some(orig) = self.shape_xfrm(shape)
        {
            self.drag = Some(Drag::Rotate { shape, orig });
            return;
        }
        if let Some((shape, handle)) = self.handle_at(p) {
            let Some(orig) = self.shape_xfrm(shape) else {
                return;
            };
            self.drag = Some(Drag::Resize {
                shape,
                handle,
                start: p,
                orig,
            });
            return;
        }
        if let Some(e) = self.editing.clone()
            && self.shape_xfrm(e.shape).is_some_and(|x| {
                let (ex, ey) = self.view().to_emu(p);
                x.contains(ex, ey)
            })
        {
            if let Some(pos) = self.text_hit(e.shape, p) {
                let paras = self
                    .pres
                    .text_paras((self.current, e.shape))
                    .unwrap_or_default();
                let sel = match ev.click_count {
                    2 => {
                        let w = docs_engine::model::word_at(&paras[pos.para].text, pos.offset);
                        Selection {
                            anchor: Position::new(pos.para, w.start),
                            head: Position::new(pos.para, w.end),
                        }
                    }
                    n if n >= 3 => Selection {
                        anchor: Position::new(pos.para, 0),
                        head: Position::new(pos.para, paras[pos.para].text.len()),
                    },
                    _ if ev.modifiers.shift => Selection {
                        anchor: e.sel.anchor,
                        head: pos,
                    },
                    _ => Selection::caret(pos),
                };
                if let Some(ed) = &mut self.editing {
                    ed.sel = sel;
                    ed.marked = None;
                    ed.typing_props = None;
                    ed.goal_x = None;
                }
                self.pres.seal_history();
                self.drag = (ev.click_count <= 1).then_some(Drag::Text);
                cx.notify();
            }
            return;
        }
        match self.shape_at(p) {
            Some(i) => {
                let was_selected = self.selected == Some(i);
                self.select(Some(i), cx);
                if ev.click_count >= 2 {
                    let pos = self.text_hit(i, p);
                    self.begin_editing(i, pos, cx);
                    return;
                }
                if let Some(orig) = self.shape_xfrm(i) {
                    self.drag = Some(Drag::Move {
                        shape: i,
                        start: p,
                        orig,
                        moved: false,
                        was_selected,
                    });
                }
            }
            None => self.select(None, cx),
        }
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let view = self.view();
        let scale = view.scale;
        if self.drag.is_some() && ev.pressed_button != Some(MouseButton::Left) {
            self.drag = None;
            self.preview = None;
            cx.notify();
            return;
        }
        let Some(drag) = &mut self.drag else {
            let c = self.cursor_at(ev.position);
            if c != self.hover_cursor {
                self.hover_cursor = c;
                cx.notify();
            }
            return;
        };
        match drag {
            Drag::Move {
                shape,
                start,
                orig,
                moved,
                ..
            } => {
                let d = ev.position - *start;
                if !*moved && f32::from(d.x).abs() + f32::from(d.y).abs() < 3.0 {
                    return;
                }
                *moved = true;
                let x = Xfrm {
                    x: orig.x + (f32::from(d.x) / scale) as i64,
                    y: orig.y + (f32::from(d.y) / scale) as i64,
                    ..*orig
                };
                if self.pres.slide(self.current).shapes[*shape].movable() {
                    self.preview = Some((*shape, x));
                }
            }
            Drag::Resize {
                shape,
                handle,
                start,
                orig,
            } => {
                let d = ev.position - *start;
                let (dx, dy) = (f32::from(d.x) / scale, f32::from(d.y) / scale);
                let min = SLIDE_NUDGE_EMU * 4;
                let x = resize_box(orig, *handle, dx as f64, dy as f64, min);
                self.preview = Some((*shape, x));
            }
            Drag::Rotate { shape, orig } => {
                let c = view.rect(orig).center();
                let d = ev.position - c;
                let deg = pointer_degrees(f32::from(d.x), f32::from(d.y), ev.modifiers.shift);
                self.preview = Some((*shape, orig.with_degrees(deg)));
            }
            Drag::Text => {
                if let Some(e) = self.editing.clone()
                    && let Some(pos) = self.text_hit(e.shape, ev.position)
                    && let Some(ed) = &mut self.editing
                {
                    ed.sel.head = pos;
                }
            }
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let drag = self.drag.take();
        self.hover_cursor = self.cursor_at(ev.position);
        if let Some((shape, x)) = self.preview.take() {
            let res = self.pres.set_xfrm((self.current, shape), x, false);
            if self.report(res, cx).is_some() {
                self.after_edit(window, cx);
            }
            return;
        }
        if let Some(Drag::Move {
            shape,
            moved: false,
            was_selected: true,
            ..
        }) = drag
        {
            let pos = self.text_hit(shape, ev.position);
            self.begin_editing(shape, pos, cx);
        }
        cx.notify();
    }

    /// Pointer over the canvas: resize arrows on handles, an I-beam over
    /// the text being edited, an open hand over shapes that can be dragged.
    fn cursor_at(&self, p: Point<Pixels>) -> CursorStyle {
        if self.pres.is_empty() {
            return CursorStyle::Arrow;
        }
        if self.rotate_handle_at(p).is_some() {
            return CursorStyle::PointingHand;
        }
        if let Some((i, h)) = self.handle_at(p) {
            let rot = self.shape_xfrm(i).map_or(0, |x| x.rot);
            return turned_handle_cursor(h, rot);
        }
        let (x, y) = self.view().to_emu(p);
        if let Some(e) = &self.editing
            && self.shape_xfrm(e.shape).is_some_and(|b| b.contains(x, y))
        {
            return CursorStyle::IBeam;
        }
        match self.shape_at(p) {
            Some(i) if self.pres.slide(self.current).shapes[i].movable() => CursorStyle::OpenHand,
            _ => CursorStyle::Arrow,
        }
    }

    /// The canvas cursor: what a drag is doing, else what's under the pointer.
    fn canvas_cursor(&self) -> CursorStyle {
        match &self.drag {
            Some(Drag::Move { .. }) if self.preview.is_some() => CursorStyle::ClosedHand,
            Some(Drag::Resize { handle, orig, .. }) => turned_handle_cursor(*handle, orig.rot),
            Some(Drag::Rotate { .. }) => CursorStyle::ClosedHand,
            Some(Drag::Text) => CursorStyle::IBeam,
            _ => self.hover_cursor,
        }
    }

    // ── thumbnail strip ────────────────────────────────────────────────────

    fn thumb_size(&self) -> Size<Pixels> {
        let s = self.pres.size;
        let max_w = f32::from(SLIDE_STRIP_WIDTH) - 2.0 * f32::from(SPACE_7) - f32::from(SPACE_4);
        let max_h = f32::from(SLIDE_THUMB_ROW) - 2.0 * f32::from(SPACE_4);
        let k = (max_w / s.cx as f32).min(max_h / s.cy as f32);
        size_px(s.cx as f32 * k, s.cy as f32 * k)
    }

    fn render_strip(&self, t: &Tokens, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let n = self.pres.len();
        let thumb = self.thumb_size();
        let t = *t;
        uniform_list("slides-strip", n, move |range, _window, cx| {
            let Some(this) = view.upgrade() else {
                return Vec::new();
            };
            range
                .map(|i| {
                    let v = this.read(cx);
                    let items = v.painter.items(&v.pres, i, false, None, cx);
                    let bg = v.pres.background(i);
                    let bridge = v.painter.bridge.clone();
                    let current = v.current == i;
                    let drop_here = v.strip_drag.is_some() && v.strip_drop == Some(i);
                    let w = view.clone();
                    let w2 = view.clone();
                    let w3 = view.clone();
                    let other = t.fg_muted;
                    let slide_size = v.pres.size;
                    let thumb_canvas = canvas(
                        |_, _, _| {},
                        move |bounds, _, window, _| {
                            let (view, b) = fit_slide(bounds, slide_size);
                            let surface = Surface {
                                other,
                                editor: false,
                            };
                            paint_slide(&items, bg, b, view, surface, &bridge, window);
                        },
                    )
                    .size_full();
                    // The thumbnail is a small page: shadow and hairline
                    // border; the current one gets an accent ring and row.
                    h_flex()
                        .id(("slide-thumb", i))
                        .h(SLIDE_THUMB_ROW)
                        .w_full()
                        .px(SPACE_4)
                        .gap(SPACE_3)
                        .items_center()
                        .when(current, |d| d.bg(t.selected))
                        .when(!current, |d| d.hover(|d| d.bg(t.hover)))
                        .when(drop_here, |d| d.border_t_2().border_color(t.accent))
                        .child(
                            div()
                                .w(SPACE_7)
                                .flex_none()
                                .text_size(FONT_CAPTION)
                                .text_color(if current { t.accent } else { t.fg_dim })
                                .when(current, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .child(format!("{}", i + 1)),
                        )
                        .child(
                            div()
                                .flex_none()
                                .w(thumb.width)
                                .h(thumb.height)
                                .rounded(RADIUS_SM)
                                .border_2()
                                .border_color(if current {
                                    t.accent
                                } else {
                                    transparent_black()
                                })
                                .child(
                                    div()
                                        .size_full()
                                        .border_1()
                                        .border_color(t.border_strong)
                                        .shadow(t.shadow(Elevation::Raised))
                                        .overflow_hidden()
                                        .child(thumb_canvas),
                                ),
                        )
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            let _ = w.update(cx, |v, cx| {
                                v.go_to(i, cx);
                                v.strip_drag = Some(i);
                                v.strip_drop = None;
                                v.strip_focused = true;
                                window.focus(&v.focus, cx);
                            });
                        })
                        .on_mouse_move(move |ev, _, cx| {
                            let _ = w2.update(cx, |v, cx| {
                                if v.strip_drag.is_some()
                                    && ev.pressed_button == Some(MouseButton::Left)
                                {
                                    if v.strip_drop != Some(i) {
                                        v.strip_drop = Some(i);
                                        cx.notify();
                                    }
                                } else if v.strip_drag.take().is_some() {
                                    v.strip_drop = None;
                                    cx.notify();
                                }
                            });
                        })
                        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
                            let _ = w3.update(cx, |v, cx| {
                                if let (Some(from), Some(to)) =
                                    (v.strip_drag.take(), v.strip_drop.take())
                                    && from != to
                                {
                                    let res = v.pres.move_slide(from, to);
                                    if v.report(res, cx).is_some() {
                                        v.go_to(to, cx);
                                        v.after_edit(window, cx);
                                    }
                                }
                                v.strip_focused = true;
                                cx.notify();
                            });
                        })
                })
                .collect()
        })
        .track_scroll(&self.strip_scroll)
        .h_full()
    }

    fn render_banner(&self, t: &Tokens) -> Option<Div> {
        let what = self.pres.unsupported_content();
        if what.is_empty() {
            return None;
        }
        Some(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(SPACE_4)
                .px(SPACE_6)
                .py(SPACE_3)
                .bg(t.chrome)
                .border_b_1()
                .border_color(t.border)
                .text_size(FONT_SMALL)
                .text_color(t.fg_muted)
                .child(kantor_ui::components::dot(t.warning))
                .child(div().flex_1().min_w_0().child(format!(
                    "This file has content Kantor can't keep yet: {}. Save writes a new copy; the original is never overwritten.",
                    what.join(", ")
                ))),
        )
    }

    fn render_presenter(&mut self, slide: usize, cx: &mut Context<Self>) -> Stateful<Div> {
        let notes = self.render_presenter_notes(slide, cx);
        let items = self.painter.items(&self.pres, slide, false, None, cx);
        let bg = self.pres.background(slide);
        let bridge = self.painter.bridge.clone();
        let size = self.pres.size;
        let other = tokens(cx).fg_muted;
        let c = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let (v, b) = fit_slide(bounds, size);
                let surface = Surface {
                    other,
                    editor: false,
                };
                paint_slide(&items, bg, b, v, surface, &bridge, window);
            },
        )
        .size_full();
        div()
            .id("slides-presenter")
            .key_context(PRESENTER_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            // Letterbox around the slide, as in every slide show.
            .bg(black())
            .on_action(cx.listener(|v, _: &PresentNext, _, cx| v.present_step(1, cx)))
            .on_action(cx.listener(|v, _: &PresentPrev, _, cx| v.present_step(-1, cx)))
            .on_action(cx.listener(|v, _: &PresentFirst, _, cx| {
                v.presenting = Some(0);
                cx.notify()
            }))
            .on_action(cx.listener(|v, _: &PresentLast, _, cx| {
                v.presenting = Some(v.pres.len().saturating_sub(1));
                cx.notify()
            }))
            .on_action(cx.listener(|v, _: &PresentExit, _, cx| v.stop_presenting(cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|v, _: &MouseDownEvent, _, cx| v.present_step(1, cx)),
            )
            .on_action(cx.listener(|v, _: &PresentToggleNotes, _, cx| {
                v.present_notes = !v.present_notes;
                cx.notify()
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|v, _: &MouseDownEvent, _, cx| v.present_step(-1, cx)),
            )
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().child(c))
            .children(notes)
    }
}

fn size_px(w: f32, h: f32) -> Size<Pixels> {
    size(px(w), px(h))
}

/// A slide of `slide` size scaled to fill `bounds` and centred
/// (thumbnails, presenter mode).
fn fit_slide(bounds: Bounds<Pixels>, slide: SlideSize) -> (View, Bounds<Pixels>) {
    let (v, b, _) = place_slide(bounds, slide, 0.0, 1.0, point(0., 0.));
    (v, b)
}

/// What the canvas needs to paint one frame.
struct Frame {
    items: Vec<Item>,
    background: slides_engine::Rgb,
    painter: Rc<Painter>,
    /// Selected shape's box (screen space is computed at paint time).
    selected: Option<(Xfrm, bool)>,
    /// Editing: text layout placement, selection, composition, caret visible.
    editing: Option<(Xfrm, draw::LaidText, Selection, Option<Range<usize>>)>,
    t: Tokens,
    focused: bool,
    focus: FocusHandle,
    view: Entity<SlidesView>,
    slide_size: SlideSize,
    zoom: f32,
    scroll: Point<f32>,
}

impl Render for SlidesView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        if let Some(slide) = self.presenting {
            if slide >= self.pres.len() {
                self.presenting = None;
            } else {
                return self.render_presenter(slide, cx).into_any_element();
            }
        }
        if self.current >= self.pres.len() {
            self.current = self.pres.len().saturating_sub(1);
        }
        self.sync_notes(window, cx);
        let editing_shape = self.editing.as_ref().map(|e| e.shape);
        let mut items = if self.pres.is_empty() {
            Vec::new()
        } else {
            self.painter
                .items(&self.pres, self.current, true, self.preview, cx)
        };
        // The edited placeholder shows its (possibly empty) text, not the prompt.
        if let Some(i) = editing_shape
            && let Some((xfrm, laid)) = self.laid(i)
            && let Some(item) = items.iter_mut().find(|it| it.layer == Layer::Slide(i))
        {
            item.xfrm = xfrm;
            item.text = Some(laid);
        }
        let selected = self.selected.and_then(|i| {
            let movable = self.pres.slide(self.current).shapes.get(i)?.movable();
            Some((self.shape_xfrm(i)?, movable && self.editing.is_none()))
        });
        let editing = self.editing.as_ref().and_then(|e| {
            let (x, laid) = self.laid(e.shape)?;
            Some((x, laid, e.sel, e.marked.clone()))
        });
        let viewport = self.viewport.clone();
        let entity = cx.entity().downgrade();
        let frame = Frame {
            items,
            background: if self.pres.is_empty() {
                slides_engine::Rgb([255, 255, 255])
            } else {
                self.pres.background(self.current)
            },
            painter: self.painter.clone(),
            selected,
            editing,
            t,
            focused: self.focus.is_focused(window),
            focus: self.focus.clone(),
            view: cx.entity(),
            slide_size: self.pres.size,
            zoom: self.zoom,
            scroll: self.scroll,
        };
        let has_slides = !self.pres.is_empty();
        let canvas_el = canvas(
            move |bounds, _, cx| {
                let old = viewport.replace(bounds);
                if old.size != bounds.size {
                    let _ = entity.update(cx, |_, cx| cx.notify());
                }
            },
            move |bounds, _, window, cx| paint_frame(&frame, bounds, has_slides, window, cx),
        )
        .size_full();

        let cursor = self.canvas_cursor();
        let area = div()
            .id("slides-canvas")
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .overflow_hidden()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll(cx.listener(Self::on_scroll))
            .child(canvas_el)
            .children(self.find.bar(cx));

        let strip = div()
            .flex_none()
            .w(SLIDE_STRIP_WIDTH)
            .h_full()
            .py(SPACE_4)
            .bg(t.chrome)
            .border_r_1()
            .border_color(t.border)
            .child(self.render_strip(&t, cx));

        let root = v_flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(t.bg)
            .text_color(t.fg)
            .text_size(FONT_BODY);
        macro_rules! on {
            ($root:expr, [$($action:ident => |$this:ident, $w:ident, $c:ident| $body:expr),* $(,)?]) => {
                $root$(.on_action(cx.listener(|$this: &mut SlidesView, _: &$action, $w: &mut Window, $c: &mut Context<SlidesView>| { $body; })))*
            };
        }
        let root = on!(root, [
            Left => |v, w, cx| v.arrow(-1, 0, false, w, cx),
            Right => |v, w, cx| v.arrow(1, 0, false, w, cx),
            Up => |v, w, cx| v.arrow(0, -1, false, w, cx),
            Down => |v, w, cx| v.arrow(0, 1, false, w, cx),
            ShiftLeft => |v, w, cx| v.arrow(-1, 0, true, w, cx),
            ShiftRight => |v, w, cx| v.arrow(1, 0, true, w, cx),
            ShiftUp => |v, w, cx| v.arrow(0, -1, true, w, cx),
            ShiftDown => |v, w, cx| v.arrow(0, 1, true, w, cx),
            LineStart => |v, _w, cx| {
                if v.editing.is_some() { v.caret_line_edge(false, false, cx) } else { v.go_to(0, cx) }
            },
            LineEnd => |v, _w, cx| {
                if v.editing.is_some() { v.caret_line_edge(true, false, cx) } else { let n = v.pres.len(); v.go_to(n.saturating_sub(1), cx) }
            },
            SelectLineStart => |v, _w, cx| v.caret_line_edge(false, true, cx),
            SelectLineEnd => |v, _w, cx| v.caret_line_edge(true, true, cx),
            Backspace => |v, w, cx| v.delete_key(false, w, cx),
            Delete => |v, w, cx| v.delete_key(true, w, cx),
            Newline => |v, w, cx| v.enter(false, w, cx),
            LineBreak => |v, w, cx| v.enter(true, w, cx),
            Escape => |v, _w, cx| v.escape(cx),
            NextShape => |v, w, cx| v.tab(false, w, cx),
            PrevShape => |v, w, cx| v.tab(true, w, cx),
            SelectAll => |v, _w, cx| v.select_all(cx),
            Copy => |v, w, cx| v.copy_doc(w, cx),
            Cut => |v, w, cx| v.cut_doc(w, cx),
            Paste => |v, w, cx| v.paste_doc(w, cx),
            ToggleBold => |v, w, cx| v.toggle(Toggle::Bold, w, cx),
            ToggleItalic => |v, w, cx| v.toggle(Toggle::Italic, w, cx),
            ToggleUnderline => |v, w, cx| v.toggle(Toggle::Underline, w, cx),
            NewSlide => |v, w, cx| v.new_slide_doc(w, cx),
            Duplicate => |v, w, cx| v.duplicate_doc(w, cx),
            DuplicateSlide => |v, w, cx| v.duplicate_slide_doc(w, cx),
            DeleteSlide => |v, w, cx| v.delete_slide_doc(w, cx),
            MoveSlideUp => |v, w, cx| v.move_slide_doc(true, w, cx),
            MoveSlideDown => |v, w, cx| v.move_slide_doc(false, w, cx),
            InsertTextBox => |v, w, cx| v.insert_text_box_doc(w, cx),
            InsertRectangle => |v, w, cx| v.insert_shape_doc(Geom::Rect, w, cx),
            InsertEllipse => |v, w, cx| v.insert_shape_doc(Geom::Ellipse, w, cx),
            InsertImage => |v, w, cx| v.insert_image_doc(w, cx),
            Undo => |v, w, cx| v.undo_doc(w, cx),
            Redo => |v, w, cx| v.redo_doc(w, cx),
            Save => |v, w, cx| v.save_doc(w, cx),
            SaveAs => |v, w, cx| v.save_doc_as(w, cx),
            Present => |v, w, cx| v.present_doc(true, w, cx),
            PresentFromCurrent => |v, w, cx| v.present_doc(false, w, cx),
            ZoomIn => |v, _w, cx| v.zoom_by(ZOOM_STEP, cx),
            ZoomOut => |v, _w, cx| v.zoom_by(1.0 / ZOOM_STEP, cx),
            ZoomFit => |v, _w, cx| v.zoom_to_fit(cx),
            OpenFind => |v, w, cx| v.open_find(false, w, cx),
            OpenReplace => |v, w, cx| v.open_find(true, w, cx),
            FindNext => |v, w, cx| v.find_next_doc(true, w, cx),
            FindPrev => |v, w, cx| v.find_next_doc(false, w, cx),
            ToggleNotes => |v, w, cx| v.toggle_notes_doc(w, cx),
        ]);
        let notes_pane = self.render_notes(cx);
        let root = root
            .on_mouse_move(cx.listener(|v, ev: &MouseMoveEvent, _, cx| v.on_notes_drag(ev, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|v, _: &MouseUpEvent, _, cx| {
                    if v.notes.drag.take().is_some() {
                        cx.notify();
                    }
                }),
            );
        root.child(toolbar::render(self, cx))
            .children(self.render_banner(&t))
            .children(self.render_paint_menu(cx))
            .child(
                h_flex().flex_1().min_h_0().child(strip).child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(area)
                        .children(notes_pane),
                ),
            )
            .into_any_element()
    }
}

fn paint_frame(
    f: &Frame,
    bounds: Bounds<Pixels>,
    has_slides: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let t = &f.t;
    window.paint_quad(fill(bounds, t.desk));
    if !has_slides {
        return;
    }
    let (view, slide_bounds, _) = place_slide(
        bounds,
        f.slide_size,
        canvas_margin(bounds.size),
        f.zoom,
        f.scroll,
    );
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        paint_canvas(f, view, slide_bounds, window)
    });
    f.painter.drop_evicted(window);
    window.handle_input(
        &f.focus,
        ElementInputHandler::new(bounds, f.view.clone()),
        cx,
    );
}

/// The slide as a page on the desk (shadow and hairline border, as Docs'
/// pages), its content, then selection, handles, text selection and caret.
fn paint_canvas(f: &Frame, view: View, slide_bounds: Bounds<Pixels>, window: &mut Window) {
    let t = &f.t;
    window.paint_drop_shadows(
        slide_bounds,
        Corners::default(),
        &t.shadow(Elevation::Popover),
    );
    let surface = Surface {
        other: t.fg_muted,
        editor: true,
    };
    paint_slide(
        &f.items,
        f.background,
        slide_bounds,
        view,
        surface,
        &f.painter.bridge,
        window,
    );
    window.paint_quad(quad(
        slide_bounds.dilate(px(1.)),
        Corners::default(),
        transparent_black(),
        Edges::all(px(1.)),
        t.border_strong,
        BorderStyle::Solid,
    ));

    if let Some((x, handles)) = f.selected {
        let b = view.rect(&x);
        let turn = Turn::of(&x, view);
        if turn.is_some() {
            stroke_polygon(window, &turned_rect(b, turn), t.accent, SLIDE_OUTLINE);
        } else {
            window.paint_quad(quad(
                b,
                Corners::default(),
                transparent_black(),
                Edges::all(SLIDE_OUTLINE),
                t.accent,
                if handles {
                    BorderStyle::Solid
                } else {
                    BorderStyle::Dashed
                },
            ));
        }
        if handles {
            // Rotate handle: a round knob on a stem above the top edge.
            let top = handle_point(view, &x, (0, -1));
            let knob = rotate_handle_point(view, &x);
            let mut stem = PathBuilder::stroke(SLIDE_OUTLINE);
            stem.move_to(top);
            stem.line_to(knob);
            if let Ok(path) = stem.build() {
                window.paint_path(path, t.accent);
            }
            let kb = Bounds::centered_at(knob, size(SLIDE_HANDLE, SLIDE_HANDLE));
            window.paint_drop_shadows(kb, Corners::all(SLIDE_HANDLE), &t.shadow(Elevation::Raised));
            window.paint_quad(quad(
                kb,
                Corners::all(SLIDE_HANDLE),
                t.paper,
                Edges::all(SLIDE_OUTLINE),
                t.accent,
                BorderStyle::Solid,
            ));
            for h in HANDLES {
                let hb = Bounds::centered_at(
                    handle_point(view, &x, h),
                    size(SLIDE_HANDLE, SLIDE_HANDLE),
                );
                window.paint_drop_shadows(hb, Corners::all(px(2.)), &t.shadow(Elevation::Raised));
                window.paint_quad(quad(
                    hb,
                    Corners::all(px(2.)),
                    t.paper,
                    Edges::all(SLIDE_OUTLINE),
                    t.accent,
                    BorderStyle::Solid,
                ));
            }
        }
    }

    if let Some((x, laid, sel, marked)) = &f.editing {
        let o = text_origin(laid, x, view);
        let turn = Turn::of(x, view);
        let paint_rect = |window: &mut Window, b: Bounds<Pixels>, color: Hsla| {
            if turn.is_some() {
                fill_polygon(window, &turned_rect(b, turn), color);
            } else {
                window.paint_quad(fill(b, color));
            }
        };
        let s = view.px_scale();
        let paged = &laid.layout.paged;
        let rect = |r: [f32; 4]| {
            Bounds::new(
                point(o.x + px(r[0] * s), o.y + px(r[1] * s)),
                size_px(r[2] * s, r[3] * s),
            )
        };
        let (a, b) = sel.ordered();
        if a != b {
            for (_, r) in paged.selection_rects((a.para, a.offset), (b.para, b.offset)) {
                paint_rect(window, rect(r), t.accent.opacity(0.3));
            }
        }
        if let Some(m) = marked {
            let p = sel.head.para;
            for (_, r) in paged.selection_rects((p, m.start), (p, m.end)) {
                paint_rect(
                    window,
                    rect([r[0], r[1] + r[3] - 1.5 / s, r[2], 1.5 / s]),
                    t.paper_ink,
                );
            }
        }
        if f.focused
            && a == b
            && let Some(c) = paged.caret(sel.head.para, sel.head.offset)
        {
            let top = point(o.x + px(c.x * s), o.y + px(c.top * s));
            let caret = Bounds::new(top, size(CARET_WIDTH, px(c.height * s)));
            // The caret contrasts with the slide, not the chrome.
            let ink = if luminance(f.background) > 0.5 {
                black()
            } else {
                white()
            };
            paint_rect(window, caret, ink);
        }
    }
}

/// Screen position of a resize handle of a (possibly rotated) box.
fn handle_point(view: View, x: &Xfrm, h: Handle) -> Point<Pixels> {
    let b = view.rect(x);
    let px_ = match h.0 {
        -1 => b.left(),
        0 => b.center().x,
        _ => b.right(),
    };
    let py = match h.1 {
        -1 => b.top(),
        0 => b.center().y,
        _ => b.bottom(),
    };
    turned(Turn::of(x, view), point(px_, py))
}

/// Screen position of the rotate handle: above the top edge's middle.
fn rotate_handle_point(view: View, x: &Xfrm) -> Point<Pixels> {
    let b = view.rect(x);
    turned(Turn::of(x, view), point(b.center().x, b.top() - ROTATE_GAP))
}

fn luminance(c: slides_engine::Rgb) -> f32 {
    let [r, g, b] = c.0;
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use slides_engine::{Anchor, EMU_PER_PX};

    /// Placeholder prompts lay out with the layout's formatting and sit
    /// where the body anchors them (the Title Slide title is bottom-anchored).
    #[std::prelude::v1::test]
    fn prompt_text_is_laid_out_and_anchored() {
        let painter = Painter::new(
            Rc::new(RefCell::new(docs_layout::Engine::new())),
            Rc::new(RefCell::new(docs_ui::FontBridge::default())),
        );
        let pres = Presentation::new();
        let list = pres.render_list(0, true);
        let title = list
            .iter()
            .find(|r| r.shape.ph.as_ref().is_some_and(|p| p.is_title()))
            .unwrap();
        let t = title.text.as_ref().unwrap();
        assert_eq!(t.anchor, Anchor::Bottom);
        let laid = painter.place(t, &title.xfrm);
        let inner = (title.xfrm.cy - t.insets[1] - t.insets[3]) as f32 / EMU_PER_PX;
        assert!(
            laid.layout.height > 40.0,
            "60 pt line: {}",
            laid.layout.height
        );
        let bottom = laid.dy + laid.layout.height;
        let expected = t.insets[1] as f32 / EMU_PER_PX + inner;
        assert!((bottom - expected).abs() < 0.5, "{bottom} vs {expected}");
        // Cached: same layout object for the same text and width.
        let again = painter.place(t, &title.xfrm);
        assert!(Rc::ptr_eq(&laid.layout, &again.layout));
    }
}
