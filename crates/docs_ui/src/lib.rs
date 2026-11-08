//! Kantor Docs UI: a paged `.docx` editor view.
//!
//! The model is [`docs_engine::Document`]; layout is [`docs_layout`] (parley,
//! no GPUI); this crate paints the laid-out glyphs with
//! `Window::paint_glyph` through the [`font_bridge`] and handles input
//! (ADR 0001). Pages sit on a "desk" with a gap and shadow, scroll
//! vertically (and horizontally when zoomed wider than the pane), and only
//! the visible pages are painted.
//!
//! Layouts are cached per paragraph, keyed by a hash of everything that
//! affects shaping, so an edit reshapes only the paragraphs it touched and
//! then re-paginates (~50 µs per paragraph edit in the spike's numbers).
//! Changing the pane size doesn't re-lay out at all: pages have a fixed
//! width and are just re-centred.
//!
//! Text input, including IME composition (fcitx5), goes through GPUI's
//! [`EntityInputHandler`] (see [`input`]); everything else is an action in
//! the [`KEY_CONTEXT`] key context.

extern crate gpui_kit as gpui;

mod ai;
mod blocks;
mod find;
pub mod font_bridge;
mod input;
mod page_menu;
mod ruler;
mod toolbar;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use docs_engine::{
    Align, Document, ListKind, OBJECT, Object, Position, RunProps, Selection, TableEdit, Toggle,
};
use docs_layout::{
    Engine, ObjectKind, PageSetup, PagedDocument, ParaLayout, PlacedObject, RunSource,
};
use gpui_kit::component::v_flex;
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::tokens::Elevation;
use kantor_ui::{Tokens, metrics::*, tokens};

pub use docs_engine::{MarginPreset, Orientation, PageGeometry, PaperSize};
pub use font_bridge::FontBridge;
pub use ruler::{ruler_visible, set_ruler_visible, toggle_ruler};

pub use docs_engine::Toggle as DocsToggle;

/// Key context of the document view.
pub const KEY_CONTEXT: &str = "DocsEditor";

actions!(
    docs,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        MoveLineStart,
        MoveLineEnd,
        SelectLineStart,
        SelectLineEnd,
        MoveDocStart,
        MoveDocEnd,
        SelectDocStart,
        SelectDocEnd,
        PageUp,
        PageDown,
        SelectPageUp,
        SelectPageDown,
        SelectAll,
        Collapse,
        Backspace,
        Delete,
        DeleteWordBack,
        DeleteWordForward,
        Newline,
        LineBreak,
        Tab,
        BackTab,
        ToggleBullets,
        ToggleNumbered,
        IndentList,
        OutdentList,
        InsertTable,
        InsertImage,
        InsertRowAbove,
        InsertRowBelow,
        InsertColumnLeft,
        InsertColumnRight,
        DeleteRow,
        DeleteColumn,
        DeleteTable,
        Copy,
        Cut,
        Paste,
        ToggleBold,
        ToggleItalic,
        ToggleUnderline,
        StyleNormal,
        StyleHeading1,
        StyleHeading2,
        StyleHeading3,
        AlignLeft,
        AlignCenter,
        AlignRight,
        AlignJustify,
        Undo,
        Redo,
        Save,
        SaveAs,
        ZoomIn,
        ZoomOut,
        ZoomReset,
    ]
);

/// Register Docs key bindings. Both Ctrl and Super work (Mac-style Cmd = Super).
pub fn init(cx: &mut App) {
    ruler::init(cx);
    kantor_ui::find_bar::bind_app_keys(KEY_CONTEXT, cx);
    let c = Some(KEY_CONTEXT);
    let mut keys = vec![
        KeyBinding::new("left", MoveLeft, c),
        KeyBinding::new("right", MoveRight, c),
        KeyBinding::new("up", MoveUp, c),
        KeyBinding::new("down", MoveDown, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-left"), MoveWordLeft, c),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-right"), MoveWordRight, c),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-left"),
            SelectWordLeft,
            c,
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-right"),
            SelectWordRight,
            c,
        ),
        KeyBinding::new("home", MoveLineStart, c),
        KeyBinding::new("end", MoveLineEnd, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-home"), MoveDocStart, c),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-end"), MoveDocEnd, c),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-shift-home"),
            SelectDocStart,
            c,
        ),
        KeyBinding::new(kantor_ui::keys::primary("ctrl-shift-end"), SelectDocEnd, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("shift-pageup", SelectPageUp, c),
        KeyBinding::new("shift-pagedown", SelectPageDown, c),
        KeyBinding::new("escape", Collapse, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-backspace"),
            DeleteWordBack,
            c,
        ),
        KeyBinding::new(
            kantor_ui::keys::primary("ctrl-delete"),
            DeleteWordForward,
            c,
        ),
        KeyBinding::new("enter", Newline, c),
        KeyBinding::new("shift-enter", LineBreak, c),
        KeyBinding::new("tab", Tab, c),
        KeyBinding::new("shift-tab", BackTab, c),
    ];
    // Shortcuts with the primary modifier: Ctrl, and Super as Mac-style Cmd.
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
        "alt-0" => StyleNormal,
        "alt-1" => StyleHeading1,
        "alt-2" => StyleHeading2,
        "alt-3" => StyleHeading3,
        "l" => AlignLeft,
        "e" => AlignCenter,
        "r" => AlignRight,
        "j" => AlignJustify,
        "z" => Undo,
        "shift-z" => Redo,
        "y" => Redo,
        "s" => Save,
        "shift-s" => SaveAs,
        "=" => ZoomIn,
        "+" => ZoomIn,
        "shift-=" => ZoomIn,
        "-" => ZoomOut,
        "0" => ZoomReset,
        // Word's list shortcuts are Ctrl+Shift+L-ish per locale; Kantor uses
        // Ctrl+Shift+7 / 8 (Google Docs). Both spellings of the shifted key.
        "shift-7" => ToggleNumbered,
        "&" => ToggleNumbered,
        "shift-8" => ToggleBullets,
        "*" => ToggleBullets,
    );
    cx.bind_keys(keys);
}

/// The parley engine and the font bridge, shared by every Docs view.
/// Created the first time a Docs view opens, so Sheets-only sessions don't
/// pay for the system font scan (ADR 0001, consequences).
struct Shared {
    engine: Rc<RefCell<Engine>>,
    bridge: Rc<RefCell<FontBridge>>,
}

impl Global for Shared {}

fn shared(cx: &mut App) -> (Rc<RefCell<Engine>>, Rc<RefCell<FontBridge>>) {
    if !cx.has_global::<Shared>() {
        cx.set_global(Shared {
            engine: Rc::new(RefCell::new(Engine::new())),
            bridge: Rc::new(RefCell::new(FontBridge::default())),
        });
    }
    let s = cx.global::<Shared>();
    (s.engine.clone(), s.bridge.clone())
}

/// The parley engine and font bridge shared by every Docs view, for other
/// views that lay out text the same way (Slides text boxes).
pub fn shared_text(cx: &mut App) -> (Rc<RefCell<Engine>>, Rc<RefCell<FontBridge>>) {
    shared(cx)
}

/// Document facts, for the inspector.
pub struct DocInfo {
    pub name: String,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    pub pages: usize,
    pub words: usize,
    pub chars: usize,
    pub paragraphs: usize,
    /// Style of the paragraph at the caret (display name).
    pub style: String,
    /// Font and size at the caret, e.g. `Calibri 11 pt`.
    pub font: String,
    pub unsupported: Vec<String>,
    /// Content kept on save but only shown as placeholders.
    pub preserved: Vec<String>,
}

/// The laid-out document plus the source text of each paragraph (font
/// bridge samples).
struct Laid {
    paged: PagedDocument,
    texts: Vec<String>,
}

/// Where pages sit in the view. Document space is unzoomed page px; page
/// `i` starts at `PAGE_GAP + i * (height + PAGE_GAP)`.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    origin: Point<Pixels>,
    size: Size<Pixels>,
    zoom: f32,
    scroll: Point<f32>,
    page_w: f32,
    page_h: f32,
}

impl Geometry {
    fn view_w(&self) -> f32 {
        f32::from(self.size.width) / self.zoom
    }

    fn view_h(&self) -> f32 {
        f32::from(self.size.height) / self.zoom
    }

    /// Left edge of the pages in document space (centred when they fit).
    fn page_x(&self) -> f32 {
        ((self.view_w() - self.page_w) / 2.0).max(PAGE_GAP)
    }

    fn page_top(&self, page: usize) -> f32 {
        PAGE_GAP + page as f32 * (self.page_h + PAGE_GAP)
    }

    fn content_w(&self) -> f32 {
        self.page_w + 2.0 * PAGE_GAP
    }

    fn content_h(&self, pages: usize) -> f32 {
        PAGE_GAP + pages as f32 * (self.page_h + PAGE_GAP)
    }

    fn page_to_screen(self, page: usize, x: f32, y: f32) -> Point<Pixels> {
        point(
            self.origin.x + px((self.page_x() + x - self.scroll.x) * self.zoom),
            self.origin.y + px((self.page_top(page) + y - self.scroll.y) * self.zoom),
        )
    }

    /// A window point to `(page, x, y)` in page coordinates; points in a gap
    /// go to the nearest page.
    fn screen_to_page(self, p: Point<Pixels>, pages: usize) -> (usize, f32, f32) {
        let x = f32::from(p.x - self.origin.x) / self.zoom + self.scroll.x - self.page_x();
        let y = f32::from(p.y - self.origin.y) / self.zoom + self.scroll.y;
        let pitch = self.page_h + PAGE_GAP;
        let page = (((y - PAGE_GAP + PAGE_GAP / 2.0) / pitch).floor().max(0.0) as usize)
            .min(pages.saturating_sub(1));
        let py = (y - self.page_top(page)).clamp(0.0, self.page_h);
        (page, x, py)
    }

    /// Pages that intersect the view.
    fn visible_pages(&self, pages: usize) -> Range<usize> {
        let pitch = self.page_h + PAGE_GAP;
        let first = ((self.scroll.y - PAGE_GAP) / pitch).floor().max(0.0) as usize;
        let last = ((self.scroll.y + self.view_h()) / pitch).ceil().max(0.0) as usize + 1;
        first.min(pages)..last.min(pages)
    }
}

/// A Docs document view.
pub struct DocsView {
    doc: Document,
    /// Name shown while the document has no file (`Untitled 2.docx`), given
    /// by the shell so new documents are numbered across the session.
    untitled_name: Option<String>,
    sel: Selection,
    /// Remembered x (page coordinates) for repeated Up/Down.
    goal_x: Option<f32>,
    /// Formatting for the next typed text after Ctrl+B etc. with no selection.
    typing_props: Option<RunProps>,
    /// IME composition: byte range in the caret's paragraph.
    marked: Option<Range<usize>>,
    laid: Rc<Laid>,
    cache: HashMap<u64, Arc<ParaLayout>>,
    /// Decoded-on-demand pictures by relationship id.
    images: HashMap<String, Arc<gpui_kit::Image>>,
    /// The insert-table grid: where it opened (window coordinates) and the
    /// size under the pointer.
    table_picker: Option<(Point<Pixels>, (usize, usize))>,
    /// The Page popover, and when it was last closed by a click outside
    /// (so that click on the toolbar control doesn't reopen it).
    page_menu: Option<page_menu::PageMenu>,
    page_menu_closed: Option<std::time::Instant>,
    /// A margin or indent being dragged on the ruler.
    ruler_drag: Option<ruler::Drag>,
    engine: Rc<RefCell<Engine>>,
    bridge: Rc<RefCell<FontBridge>>,
    zoom: f32,
    /// Scroll offset in document space.
    scroll: Point<f32>,
    /// Canvas bounds from the last paint.
    viewport: Rc<Cell<Bounds<Pixels>>>,
    /// Scroll the caret into view on the next paint (set by keyboard edits;
    /// needs the viewport size, known after layout).
    reveal_caret: Rc<Cell<bool>>,
    dragging: bool,
    focus: FocusHandle,
    status: SharedString,
    /// Find & Replace: the bar, its matches and the current one.
    find: Entity<kantor_ui::find_bar::FindBar>,
    found: find::Found,
    found_current: Option<usize>,
    _find_sub: Subscription,
}

/// Cache key: everything that affects shaping and line breaking.
fn layout_key(p: &docs_layout::Paragraph, width: f32) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    p.text.hash(&mut h);
    p.family.hash(&mut h);
    p.size_pt.to_bits().hash(&mut h);
    format!("{:?}", p.line_height).hash(&mut h);
    (p.align as u8).hash(&mut h);
    width.to_bits().hash(&mut h);
    for v in [p.indent_left_pt, p.indent_right_pt, p.first_line_pt] {
        v.to_bits().hash(&mut h);
    }
    format!("{:?}{:?}", p.marker, p.objects).hash(&mut h);
    for s in &p.spans {
        s.range.hash(&mut h);
        (s.bold, s.italic, s.underline).hash(&mut h);
        s.size_pt.map(f32::to_bits).hash(&mut h);
        s.color.hash(&mut h);
        s.family.hash(&mut h);
    }
    h.finish()
}

impl DocsView {
    pub fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (doc, status) = match path.as_deref().map(Document::open) {
            Some(Ok(doc)) => {
                let s = opened_status(&doc);
                (doc, s)
            }
            Some(Err(err)) => {
                log::error!("{err:#}");
                (Document::new(), format!("{err:#}"))
            }
            None => (Document::new(), String::new()),
        };
        let (engine, bridge) = shared(cx);
        let focus = cx.focus_handle();
        let (find, find_sub) = find::new_bar(window, cx);
        let mut this = Self {
            doc,
            untitled_name: None,
            sel: Selection::default(),
            goal_x: None,
            typing_props: None,
            marked: None,
            laid: Rc::new(Laid {
                paged: PagedDocument::paginate(&[], Vec::new(), PageSetup::a4()),
                texts: Vec::new(),
            }),
            cache: HashMap::new(),
            images: HashMap::new(),
            table_picker: None,
            page_menu: None,
            page_menu_closed: None,
            ruler_drag: None,
            engine,
            bridge,
            zoom: 1.0,
            scroll: point(0., 0.),
            viewport: Rc::default(),
            reveal_caret: Rc::default(),
            dragging: false,
            focus,
            status: status.into(),
            find,
            found: Rc::default(),
            found_current: None,
            _find_sub: find_sub,
        };
        this.relayout();
        this
    }

    // ── layout ─────────────────────────────────────────────────────────────

    /// Lay out the document again, reusing cached paragraph layouts.
    fn relayout(&mut self) {
        let setup = blocks::page_setup(&self.doc);
        let (paras, widths, blocks) = blocks::build(&self.doc);
        let mut engine = self.engine.borrow_mut();
        let mut fresh = HashMap::with_capacity(paras.len());
        let layouts = paras
            .iter()
            .zip(&widths)
            .map(|(p, &width)| {
                let key = layout_key(p, width);
                let layout = match self.cache.get(&key) {
                    Some(l) => l.clone(),
                    None => Arc::new(engine.layout_paragraph(p, width)),
                };
                fresh.insert(key, layout.clone());
                layout
            })
            .collect();
        self.cache = fresh;
        let paged = PagedDocument::paginate_blocks(&paras, layouts, &blocks, setup);
        let texts = self
            .doc
            .paragraphs()
            .iter()
            .map(|p| p.text.clone())
            .collect();
        self.laid = Rc::new(Laid { paged, texts });
    }

    fn geometry(&self) -> Geometry {
        let b = self.viewport.get();
        let s = self.laid.paged.setup;
        Geometry {
            origin: b.origin,
            size: b.size,
            zoom: self.zoom,
            scroll: self.scroll,
            page_w: s.width,
            page_h: s.height,
        }
    }

    fn clamp_scroll(&mut self) {
        let g = self.geometry();
        let max_y = (g.content_h(self.laid.paged.pages.len()) - g.view_h()).max(0.0);
        let max_x = (g.content_w() - g.view_w()).max(0.0);
        self.scroll.y = self.scroll.y.clamp(0.0, max_y);
        self.scroll.x = self.scroll.x.clamp(0.0, max_x);
    }

    /// Scroll so the caret is visible (with a little margin).
    fn scroll_to_caret(&mut self) {
        let g = self.geometry();
        if g.size.height <= px(0.) {
            return;
        }
        let Some(c) = self
            .laid
            .paged
            .caret(self.sel.head.para, self.sel.head.offset)
        else {
            return;
        };
        let top = g.page_top(c.page) + c.top - PAGE_GAP;
        let bottom = g.page_top(c.page) + c.top + c.height + PAGE_GAP;
        if top < self.scroll.y {
            self.scroll.y = top;
        } else if bottom > self.scroll.y + g.view_h() {
            self.scroll.y = bottom - g.view_h();
        }
        let x = g.page_x() + c.x;
        if x < self.scroll.x + PAGE_GAP {
            self.scroll.x = x - PAGE_GAP;
        } else if x > self.scroll.x + g.view_w() - PAGE_GAP {
            self.scroll.x = x - g.view_w() + PAGE_GAP;
        }
        self.clamp_scroll();
    }

    // ── state changes ──────────────────────────────────────────────────────

    /// After an edit: new selection, re-layout, reveal the caret.
    fn edited(&mut self, sel: Selection, window: &mut Window, cx: &mut Context<Self>) {
        let was_dirty_title = self.title(cx);
        self.sel = self.doc.clamp_selection(sel);
        self.goal_x = None;
        self.relayout();
        self.refresh_find(cx);
        self.reveal_caret.set(true);
        self.status = SharedString::default();
        if self.title(cx) != was_dirty_title {
            self.update_title(window, cx);
        }
        cx.notify();
    }

    /// Move the caret (or the selection head when extending).
    fn move_to(&mut self, pos: Position, extend: bool, cx: &mut Context<Self>) {
        let pos = self.doc.clamp(pos);
        if extend {
            self.sel.head = pos;
        } else {
            self.sel = Selection::caret(pos);
        }
        self.typing_props = None;
        self.marked = None;
        self.reveal_caret.set(true);
        cx.notify();
    }

    fn horizontal(&mut self, forward: bool, word: bool, extend: bool, cx: &mut Context<Self>) {
        self.goal_x = None;
        if !extend && !self.sel.is_empty() && !word {
            let (start, end) = self.sel.ordered();
            self.move_to(if forward { end } else { start }, false, cx);
            return;
        }
        let head = self.sel.head;
        let pos = match (forward, word) {
            (true, false) => self.doc.next_position(head),
            (false, false) => self.doc.prev_position(head),
            (true, true) => self.doc.next_word_position(head),
            (false, true) => self.doc.prev_word_position(head),
        };
        self.move_to(pos, extend, cx);
    }

    fn vertical(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        let head = self.sel.head;
        let paged = &self.laid.paged;
        let x = match self.goal_x {
            Some(x) => x,
            None => match paged.caret(head.para, head.offset) {
                Some(c) => c.x,
                None => return,
            },
        };
        let pos = match paged.vertical(head.para, head.offset, x, down) {
            Some((para, offset)) => Position::new(para, offset),
            None if down => self.doc.end(),
            None => Position::default(),
        };
        self.move_to(pos, extend, cx);
        self.goal_x = Some(x);
    }

    fn line_edge(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        self.goal_x = None;
        let head = self.sel.head;
        let Some((range, soft)) = self.laid.paged.line_range(head.para, head.offset) else {
            return;
        };
        let text = &self.doc.paragraph(head.para).text;
        let offset = if !end {
            range.start
        } else {
            let mut e = range.end.min(text.len());
            // A wrapped line ends after the space it broke at; a line ended
            // by a line break ends before it.
            if soft {
                e = text[..e].trim_end_matches(' ').len().max(range.start);
            } else if text[..e].ends_with(docs_engine::LINE_BREAK) {
                e -= docs_engine::LINE_BREAK.len_utf8();
            }
            e
        };
        self.move_to(Position::new(head.para, offset), extend, cx);
    }

    fn page_move(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        let g = self.geometry();
        let head = self.sel.head;
        let paged = &self.laid.paged;
        let Some(c) = paged.caret(head.para, head.offset) else {
            return;
        };
        let dy = g.view_h() * 0.9 * if down { 1.0 } else { -1.0 };
        let y = g.page_top(c.page) + c.top + c.height / 2.0 + dy;
        let screen = point(g.origin.x, g.origin.y + px((y - g.scroll.y) * g.zoom));
        let (page, _, py) = g.screen_to_page(screen, paged.pages.len());
        if let Some((para, offset)) = paged.hit_test(page, c.x, py) {
            self.scroll.y += dy;
            self.clamp_scroll();
            self.move_to(Position::new(para, offset), extend, cx);
        }
    }

    // ── editing ────────────────────────────────────────────────────────────

    fn insert(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let props = self.typing_props.take();
        let sel = self.doc.insert_text(self.sel, text, props);
        self.marked = None;
        self.edited(sel, window, cx);
    }

    fn toggle(&mut self, toggle: Toggle, window: &mut Window, cx: &mut Context<Self>) {
        if self.sel.is_empty() {
            // Applies to the next typed text, like Word.
            let mut props = self
                .typing_props
                .take()
                .unwrap_or_else(|| self.doc.run_props_at(self.sel.head));
            let on = !self.is_on(toggle);
            toggle.set(&mut props, on);
            self.typing_props = Some(props);
            cx.notify();
            return;
        }
        let sel = self.sel;
        self.doc.toggle(sel, toggle);
        self.edited(sel, window, cx);
    }

    /// Whether `toggle` is on at the caret / for the whole selection
    /// (including a pending typing format).
    pub fn is_on(&self, toggle: Toggle) -> bool {
        if self.sel.is_empty()
            && let Some(props) = &self.typing_props
        {
            let p = self.doc.paragraph(self.sel.head.para);
            let r = self.doc.resolve_run(p, props);
            return match toggle {
                Toggle::Bold => r.bold,
                Toggle::Italic => r.italic,
                Toggle::Underline => r.underline,
            };
        }
        self.doc.is_on(self.sel, toggle)
    }

    /// Style id of the paragraph at the caret (`None` = default).
    pub fn current_style(&self) -> Option<String> {
        self.doc
            .paragraph(self.sel.head.para)
            .props
            .style
            .clone()
            .filter(|s| s != "Normal")
    }

    pub fn current_align(&self) -> Align {
        let p = self.doc.paragraph(self.sel.head.para);
        self.doc.resolve_para(p).align
    }

    /// The list kind of the paragraph at the caret.
    pub fn current_list(&self) -> Option<ListKind> {
        self.doc.list_kind(self.sel.head.para)
    }

    /// Whether the caret is in a table.
    pub fn in_table(&self) -> bool {
        self.doc.cell_at(self.sel.head).is_some()
    }

    /// Bulleted or numbered list on/off for the selected paragraphs.
    pub fn toggle_list_doc(&mut self, kind: ListKind, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.sel;
        self.doc.toggle_list(sel, kind);
        self.edited(sel, window, cx);
    }

    /// Indent (`+1`) or outdent (`-1`) the selected list items.
    pub fn list_level_doc(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.sel;
        if self.doc.change_list_level(sel, delta) {
            self.edited(sel, window, cx);
        }
    }

    /// Insert a `rows` × `cols` table at the caret.
    pub fn insert_table_doc(
        &mut self,
        rows: usize,
        cols: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.table_picker = None;
        match self.doc.insert_table(self.sel, rows, cols) {
            Some(sel) => self.edited(sel, window, cx),
            None => {
                self.status = "Tables inside tables aren't supported yet".into();
                cx.notify();
            }
        }
    }

    /// Change the structure of the table at the caret.
    pub fn table_edit_doc(&mut self, edit: TableEdit, window: &mut Window, cx: &mut Context<Self>) {
        match self.doc.edit_table(self.sel, edit) {
            Ok(sel) => self.edited(sel, window, cx),
            Err(msg) => {
                self.status = msg.into();
                cx.notify();
            }
        }
    }

    /// Ask for a PNG or JPEG and insert it at the caret.
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
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let result = std::fs::read(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|bytes| this.doc.insert_image(this.sel, bytes, &name));
                match result {
                    Ok(sel) => this.edited(sel, window, cx),
                    Err(err) => {
                        this.status = format!("Can't insert {name}: {err:#}").into();
                        cx.notify();
                    }
                }
            });
        })
        .detach();
    }

    /// Tab: list level, next table cell, or a tab character.
    fn tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.sel;
        let head = sel.head;
        let at_item_start = self.doc.list_kind(head.para).is_some()
            && (!sel.is_empty() || {
                let p = self.doc.paragraph(head.para);
                p.text[..head.offset].chars().all(|c| c == OBJECT)
            });
        if at_item_start {
            self.list_level_doc(if back { -1 } else { 1 }, window, cx);
        } else if self.in_table() {
            if let Some(s) = self.doc.next_cell(sel, !back) {
                self.edited(s, window, cx);
            }
        } else if !back {
            self.insert("\t", window, cx);
        }
    }

    pub fn toggle_doc(&mut self, toggle: Toggle, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle(toggle, window, cx);
    }

    /// Apply a paragraph style (`None` = Normal) to the selected paragraphs.
    pub fn set_style_doc(
        &mut self,
        style: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sel = self.sel;
        let style = style.filter(|s| *s != "Normal" || self.doc.styles.get("Normal").is_none());
        self.doc.set_style(sel, style);
        self.edited(sel, window, cx);
    }

    pub fn set_align_doc(&mut self, align: Align, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.sel;
        self.doc.set_align(sel, align);
        self.edited(sel, window, cx);
    }

    pub fn undo_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.doc.undo() {
            Some(sel) => self.edited(sel, window, cx),
            None => {
                self.status = "Nothing to undo".into();
                cx.notify();
            }
        }
    }

    pub fn redo_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.doc.redo() {
            Some(sel) => self.edited(sel, window, cx),
            None => {
                self.status = "Nothing to redo".into();
                cx.notify();
            }
        }
    }

    fn selected_text(&self) -> Option<String> {
        let (start, end) = self.sel.ordered();
        (start != end).then(|| self.doc.text_in(start, end))
    }

    pub fn copy_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub fn cut_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            let sel = self.doc.delete_selection(self.sel);
            self.edited(sel, window, cx);
        }
    }

    /// Paste plain text (new lines start new paragraphs).
    pub fn paste_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
            self.insert(&text, window, cx);
        }
    }

    pub fn zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        self.set_zoom(self.zoom * factor, cx);
    }

    pub fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let zoom = zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        // Keep the middle of the view where it is.
        let g = self.geometry();
        let mid = self.scroll.y + g.view_h() / 2.0;
        self.zoom = zoom;
        let g = self.geometry();
        self.scroll.y = mid - g.view_h() / 2.0;
        self.clamp_scroll();
        cx.notify();
    }

    pub fn zoom(&self) -> f32 {
        self.zoom
    }

    // ── shell API (tabs, status bar, inspector, close prompt) ──────────────

    pub fn doc_name(&self, _: &App) -> String {
        self.name()
    }

    /// Set the name shown while the document has no file.
    pub fn set_untitled_name(&mut self, name: String, cx: &mut Context<Self>) {
        self.untitled_name = Some(name);
        cx.notify();
    }

    /// The file name, or the shell-given name while there is no file.
    fn name(&self) -> String {
        match &self.untitled_name {
            Some(name) if self.doc.path().is_none() => name.clone(),
            _ => self.doc.title(),
        }
    }

    pub fn path(&self, _: &App) -> Option<PathBuf> {
        self.doc.path().map(|p| p.to_path_buf())
    }

    pub fn is_dirty(&self, _: &App) -> bool {
        self.doc.is_dirty()
    }

    pub fn title(&self, _: &App) -> String {
        let dirty = if self.doc.is_dirty() { "• " } else { "" };
        format!("{dirty}{} — Kantor Docs", self.name())
    }

    fn update_title(&self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.title(cx);
        window.set_window_title(&title);
    }

    /// Give keyboard focus to the page.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    /// Status line for the app's status bar.
    pub fn status(&self) -> SharedString {
        self.status.clone()
    }

    /// Caret location for the status bar, e.g. `Page 2 of 5 · 1,234 words`.
    pub fn location(&self) -> String {
        let pages = self.laid.paged.pages.len();
        let page = self
            .laid
            .paged
            .caret(self.sel.head.para, self.sel.head.offset)
            .map_or(1, |c| c.page + 1);
        let total = (self.doc.word_count(), self.doc.char_count());
        let (start, end) = self.sel.ordered();
        let selected = (start != end).then(|| self.doc.stats_in(start, end));
        format!("Page {page} of {pages} · {}", count_text(total, selected))
    }

    pub fn info(&self) -> DocInfo {
        let p = self.doc.paragraph(self.sel.head.para);
        let style = p
            .props
            .style
            .as_deref()
            .map(|s| self.doc.styles.display_name(s))
            .unwrap_or_else(|| "Normal".to_owned());
        let r = self.doc.resolved_at(self.sel.head);
        DocInfo {
            name: self.name(),
            path: self.doc.path().map(|p| p.to_path_buf()),
            dirty: self.doc.is_dirty(),
            pages: self.laid.paged.pages.len(),
            words: self.doc.word_count(),
            chars: self.doc.char_count(),
            paragraphs: self.doc.len(),
            style,
            font: format!("{} {} pt", r.font, r.size_pt),
            unsupported: self.doc.unsupported_content().to_vec(),
            preserved: self.doc.preserved_content().to_vec(),
        }
    }

    // ── saving ─────────────────────────────────────────────────────────────

    pub fn save_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save(&Save, window, cx);
    }

    pub fn save_doc_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as(&SaveAs, window, cx);
    }

    fn save(&mut self, _: &Save, window: &mut Window, cx: &mut Context<Self>) {
        if self.doc.needs_save_as() {
            self.save_as_task(window, cx).detach();
            return;
        }
        let result = self.doc.save();
        self.after_save(result, window, cx);
    }

    fn save_as(&mut self, _: &SaveAs, window: &mut Window, cx: &mut Context<Self>) {
        self.save_as_task(window, cx).detach();
    }

    /// Save before the document closes. Resolves to `true` once saved;
    /// `false` if Save As was cancelled or saving failed.
    pub fn save_for_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if self.doc.needs_save_as() {
            return self.save_as_task(window, cx);
        }
        self.save(&Save, window, cx);
        Task::ready(!self.doc.is_dirty())
    }

    /// Ask for a path and save there; resolves to whether the document is saved.
    fn save_as_task(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Task<bool> {
        if self.doc.has_unsupported_content() {
            self.status = format!(
                "{} has content Kantor can't keep ({}), so it's saved as a new file; the original stays as it is",
                self.doc.title(),
                self.doc.unsupported_content().join(", ")
            )
            .into();
            cx.notify();
        }
        let dir = self
            .doc
            .path()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("."));
        let name = if self.doc.path().is_none() {
            self.name()
        } else {
            self.doc.suggested_file_name()
        };
        let path = cx.prompt_for_new_path(&dir, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(mut path))) = path.await else {
                return false;
            };
            if path.extension().is_none() {
                path.set_extension("docx");
            }
            this.update_in(cx, |this, window, cx| {
                let result = this.doc.save_as(&path);
                this.after_save(result, window, cx);
                !this.doc.is_dirty()
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
            Ok(()) => format!("Saved {}", self.doc.title()).into(),
            Err(err) => format!("Save failed: {err:#}").into(),
        };
        self.update_title(window, cx);
        cx.notify();
    }

    // ── mouse ──────────────────────────────────────────────────────────────

    fn hit(&self, p: Point<Pixels>) -> Option<Position> {
        let g = self.geometry();
        let paged = &self.laid.paged;
        let (page, x, y) = g.screen_to_page(p, paged.pages.len());
        let (para, offset) = paged.hit_test(page, x, y)?;
        Some(self.doc.clamp(Position::new(para, offset)))
    }

    /// The object box under a window point.
    fn object_at(&self, p: Point<Pixels>) -> Option<PlacedObject> {
        let g = self.geometry();
        let paged = &self.laid.paged;
        let (page, x, y) = g.screen_to_page(p, paged.pages.len());
        paged.objects(page).into_iter().find(|o| {
            let [ox, oy, w, h] = o.rect;
            x >= ox && x <= ox + w && y >= oy && y <= oy + h
        })
    }

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.table_picker = None;
        // A click on a picture or placeholder selects it as one character.
        if !ev.modifiers.shift
            && let Some(o) = self.object_at(ev.position)
        {
            self.sel = Selection {
                anchor: Position::new(o.para, o.index),
                head: Position::new(o.para, o.index + OBJECT.len_utf8()),
            };
            self.goal_x = None;
            self.typing_props = None;
            self.marked = None;
            self.dragging = false;
            cx.notify();
            return;
        }
        let Some(pos) = self.hit(ev.position) else {
            return;
        };
        self.goal_x = None;
        self.typing_props = None;
        self.marked = None;
        let text = &self.doc.paragraph(pos.para).text;
        match ev.click_count {
            2 => {
                let w = docs_engine::model::word_at(text, pos.offset);
                self.sel = Selection {
                    anchor: Position::new(pos.para, w.start),
                    head: Position::new(pos.para, w.end),
                };
            }
            n if n >= 3 => {
                self.sel = Selection {
                    anchor: Position::new(pos.para, 0),
                    head: Position::new(pos.para, text.len()),
                };
            }
            _ if ev.modifiers.shift => self.sel.head = pos,
            _ => self.sel = Selection::caret(pos),
        }
        self.dragging = ev.click_count <= 1;
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.dragging {
            return;
        }
        if ev.pressed_button != Some(MouseButton::Left) {
            self.dragging = false;
            return;
        }
        if let Some(pos) = self.hit(ev.position)
            && pos != self.sel.head
        {
            self.sel.head = pos;
            self.reveal_caret.set(true);
            cx.notify();
        }
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(window.line_height());
        if ev.modifiers.control || ev.modifiers.platform {
            let dy = f32::from(delta.y);
            if dy != 0.0 {
                self.zoom_by(if dy > 0.0 { ZOOM_STEP } else { 1.0 / ZOOM_STEP }, cx);
            }
            return;
        }
        let (mut dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        if ev.modifiers.shift && dx == 0.0 {
            dx = dy;
            self.scroll.x -= dx / self.zoom;
        } else {
            self.scroll.x -= dx / self.zoom;
            self.scroll.y -= dy / self.zoom;
        }
        self.clamp_scroll();
        cx.notify();
    }

    /// Decode (through GPUI's asset cache) the pictures on visible pages.
    fn visible_images(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> HashMap<(usize, usize), Arc<RenderImage>> {
        let mut out = HashMap::new();
        let laid = self.laid.clone();
        let paged = &laid.paged;
        let g = self.geometry();
        for page in g.visible_pages(paged.pages.len()) {
            for o in paged.objects(page) {
                if o.kind != ObjectKind::Image {
                    continue;
                }
                let Some(Object::Image(img)) = self.doc.paragraph(o.para).object_at(o.index) else {
                    continue;
                };
                let image = match self.images.get(&img.rel_id) {
                    Some(i) => i.clone(),
                    None => {
                        let Some(bytes) = self.doc.image_bytes(&img.rel_id) else {
                            continue;
                        };
                        let format = match docs_engine::ImageFormat::sniff(&bytes) {
                            docs_engine::ImageFormat::Png => ImageFormat::Png,
                            docs_engine::ImageFormat::Jpeg => ImageFormat::Jpeg,
                            docs_engine::ImageFormat::Other => continue,
                        };
                        let i = Arc::new(gpui_kit::Image::from_bytes(format, bytes.to_vec()));
                        self.images.insert(img.rel_id.clone(), i.clone());
                        i
                    }
                };
                if let Some(data) = image.use_render_image(window, cx) {
                    out.insert((o.para, o.index), data);
                }
            }
        }
        out
    }

    fn render_table_picker(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        const N: usize = 8;
        let (at, (hr, hc)) = self.table_picker?;
        let t = tokens(cx);
        let view = cx.entity().downgrade();
        let mut grid = v_flex().gap(SPACE_1);
        for r in 1..=N {
            let mut row = gpui_kit::component::h_flex().gap(SPACE_1);
            for c in 1..=N {
                let on = r <= hr && c <= hc;
                let v = view.clone();
                let v2 = view.clone();
                row = row.child(
                    div()
                        .id(("docs-table-cell", r * 16 + c))
                        .size(SPACE_7)
                        .rounded(RADIUS_SM)
                        .border_1()
                        .border_color(if on { t.accent } else { t.border })
                        .bg(if on { t.accent_soft } else { t.surface })
                        .on_hover(move |hovered, _, cx| {
                            if *hovered {
                                let _ = v.update(cx, |this, cx| {
                                    if let Some(p) = &mut this.table_picker {
                                        p.1 = (r, c);
                                    }
                                    cx.notify();
                                });
                            }
                        })
                        .on_click(move |_, window, cx| {
                            let _ = v2.update(cx, |this, cx| {
                                this.insert_table_doc(r, c, window, cx);
                                this.focus(window, cx);
                            });
                        }),
                );
            }
            grid = grid.child(row);
        }
        let label = if hr > 0 && hc > 0 {
            format!("{hr} × {hc} table")
        } else {
            "Insert table".to_owned()
        };
        Some(deferred(
            anchored().position(at).child(
                v_flex()
                    .id("docs-table-picker")
                    .occlude()
                    .gap(SPACE_3)
                    .p(SPACE_4)
                    .bg(t.elevated)
                    .border_1()
                    .border_color(t.border)
                    .rounded(RADIUS_LG)
                    .shadow(t.shadow(Elevation::Popover))
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.table_picker = None;
                        cx.notify();
                    }))
                    .child(grid)
                    .child(label),
            ),
        ))
    }

    /// Open (or close) the insert-table grid at a window position.
    pub fn toggle_table_picker(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        self.table_picker = match self.table_picker {
            Some(_) => None,
            None => Some((at, (0, 0))),
        };
        cx.notify();
    }

    fn render_banner(&self, t: &Tokens) -> Option<Div> {
        let what = self.doc.unsupported_content();
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
                    "This file has content Kantor can't show or keep yet: {}. Save writes a new copy; the original is never overwritten.",
                    what.join(", ")
                ))),
        )
    }
}

fn opened_status(doc: &Document) -> String {
    let mut s = format!("Opened {}", doc.title());
    if doc.has_unsupported_content() {
        s.push_str(" (has unsupported content; Save creates a copy)");
    } else if !doc.preserved_content().is_empty() {
        s.push_str(&format!(
            " ({} shown as placeholders, kept as they are when saving)",
            doc.preserved_content().join(", ")
        ));
    }
    s
}

/// `1,234 words · 6,789 characters`, or for a selection
/// `12 of 1,234 words · 60 characters`.
fn count_text(total: (usize, usize), selected: Option<(usize, usize)>) -> String {
    let plural = |n: usize, word: &str| {
        format!(
            "{} {word}{}",
            group_thousands(n),
            if n == 1 { "" } else { "s" }
        )
    };
    match selected {
        Some((words, chars)) => format!(
            "{} of {} · {}",
            group_thousands(words),
            plural(total.0, "word"),
            plural(chars, "character")
        ),
        None => format!(
            "{} · {}",
            plural(total.0, "word"),
            plural(total.1, "character")
        ),
    }
}

fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

fn layout_color(c: docs_layout::Rgba) -> Hsla {
    let [r, g, b, a] = c.0;
    gpui_kit::Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a: a as f32 / 255.,
    }
    .into()
}

macro_rules! listeners {
    ($el:expr, $cx:expr, [$($action:ident => |$this:ident, $window:ident, $c:ident| $body:expr),* $(,)?]) => {
        $el$(.on_action($cx.listener(|$this: &mut DocsView, _: &$action, $window: &mut Window, $c: &mut Context<DocsView>| { $body; })))*
    };
}

/// What the canvas needs to paint one frame.
struct Frame {
    laid: Rc<Laid>,
    /// Decoded pictures on the visible pages, by `(paragraph, byte index)`.
    images: HashMap<(usize, usize), Arc<RenderImage>>,
    sel: Selection,
    marked: Option<Range<usize>>,
    zoom: f32,
    scroll: Point<f32>,
    focused: bool,
    /// A ruler drag's guide line (page x).
    guide: Option<f32>,
    /// Find matches to highlight and the current one.
    found: find::Found,
    found_current: Option<usize>,
    t: Tokens,
    bridge: Rc<RefCell<FontBridge>>,
    focus: FocusHandle,
    view: Entity<DocsView>,
}

fn paint_frame(f: &Frame, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
    let paged = &f.laid.paged;
    let g = Geometry {
        origin: bounds.origin,
        size: bounds.size,
        zoom: f.zoom,
        scroll: f.scroll,
        page_w: paged.setup.width,
        page_h: paged.setup.height,
    };
    window.paint_quad(fill(bounds, f.t.desk));
    let (start, end) = f.sel.ordered();
    let selection = if start != end {
        paged.selection_rects((start.para, start.offset), (end.para, end.offset))
    } else {
        Vec::new()
    };
    // Find matches: all of them light, the current one stronger.
    let visible = g.visible_pages(paged.pages.len());
    let found: Vec<(usize, [f32; 4], bool)> = f
        .found
        .iter()
        .enumerate()
        .flat_map(|(i, (p, r))| {
            paged
                .selection_rects((*p, r.start), (*p, r.end))
                .into_iter()
                .map(move |(page, rect)| (page, rect, Some(i) == f.found_current))
        })
        .filter(|(page, _, _)| visible.contains(page))
        .collect();
    let marked = f.marked.as_ref().map(|m| {
        let p = f.sel.head.para;
        paged.selection_rects((p, m.start), (p, m.end))
    });
    let shadow = f.t.shadow(Elevation::Popover);
    let ts = window.text_system().clone();
    let z = f.zoom;
    let rect = |page: usize, r: [f32; 4]| {
        Bounds::from_corners(
            g.page_to_screen(page, r[0], r[1]),
            g.page_to_screen(page, r[0] + r[2], r[1] + r[3]),
        )
    };
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for page in g.visible_pages(paged.pages.len()) {
            let pb = rect(page, [0., 0., g.page_w, g.page_h]);
            window.paint_drop_shadows(pb, Corners::default(), &shadow);
            window.paint_quad(fill(pb, f.t.paper));
            for (_, r, current) in found.iter().filter(|(p, _, _)| *p == page) {
                let color = if *current {
                    f.t.find_match_current
                } else {
                    f.t.find_match
                };
                window.paint_quad(fill(rect(page, *r), color));
            }
            for (_, r) in selection.iter().filter(|(p, _)| *p == page) {
                window.paint_quad(fill(rect(page, *r), f.t.paper_selection));
            }
            // Table borders (document content: the file's colours).
            for rule in &paged.pages[page].rules {
                let mut r = rule.rect;
                // At least one device pixel at any zoom.
                r[2] = r[2].max(1.0 / z);
                r[3] = r[3].max(1.0 / z);
                window.paint_quad(fill(rect(page, r), layout_color(rule.color)));
            }
            // Pictures and placeholders.
            for o in paged.objects(page) {
                let b = rect(page, o.rect);
                let picture = (o.kind == ObjectKind::Image)
                    .then(|| f.images.get(&(o.para, o.index)))
                    .flatten();
                match picture {
                    Some(data) => {
                        let _ =
                            window.paint_image(b, b, Corners::default(), data.clone(), 0, false);
                    }
                    None => {
                        window.paint_quad(fill(b, f.t.paper_selection.opacity(0.35)));
                        window.paint_quad(outline(
                            b,
                            f.t.paper_ink.opacity(0.35),
                            BorderStyle::Dashed,
                        ));
                    }
                }
                let selected = start != end
                    && (start.para, start.offset) <= (o.para, o.index)
                    && (end.para, end.offset) >= (o.para, o.index + OBJECT.len_utf8());
                if selected {
                    window.paint_quad(fill(b, f.t.paper_selection.opacity(0.6)));
                    window.paint_quad(outline(b, f.t.accent, BorderStyle::Solid));
                }
            }
            let mut bridge = f.bridge.borrow_mut();
            for run in paged.glyph_runs(page) {
                let para_text = f.laid.texts.get(run.para).map_or("", String::as_str);
                let sample = paged
                    .run_text(&run, para_text)
                    .get(run.text_range.clone())
                    .unwrap_or_default();
                let Some(font_id) = bridge.font_id(&run.font, sample, &ts) else {
                    continue;
                };
                let color: Hsla = match run.source {
                    // Placeholder labels are Kantor's, not the document's.
                    RunSource::Label(_) => f.t.paper_ink.opacity(0.6),
                    _ => layout_color(run.color),
                };
                let size = px(run.font_size * z);
                for glyph in &run.glyphs {
                    let origin = g.page_to_screen(page, glyph.x, glyph.y);
                    let _ = window.paint_glyph(origin, font_id, GlyphId(glyph.id), size, color);
                }
                if let Some((offset, thick)) = run.underline {
                    let y = run.baseline + offset;
                    window.paint_quad(fill(
                        rect(page, [run.origin_x, y, run.advance, thick.max(1.0 / z)]),
                        color,
                    ));
                }
            }
            drop(bridge);
            if let Some(x) = f.guide {
                let line = [x - 0.5 / z, 0., 1. / z, g.page_h];
                window.paint_quad(fill(rect(page, line), f.t.accent));
            }
            // IME composition: underline the marked text.
            for (_, r) in marked.iter().flatten().filter(|(p, _)| *p == page) {
                let ink = f.t.paper_ink;
                window.paint_quad(fill(rect(page, [r[0], r[1] + r[3] - 1.5, r[2], 1.5]), ink));
            }
        }
        if f.focused
            && start == end
            && let Some(c) = paged.caret(f.sel.head.para, f.sel.head.offset)
        {
            let top = g.page_to_screen(c.page, c.x, c.top);
            let caret = Bounds::new(top, size(CARET_WIDTH, px(c.height * z)));
            window.paint_quad(fill(caret, f.t.paper_ink));
        }
    });
    window.handle_input(
        &f.focus,
        ElementInputHandler::new(bounds, f.view.clone()),
        cx,
    );
}

impl Render for DocsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        if self.reveal_caret.get() && self.viewport.get().size.height > px(0.) {
            self.reveal_caret.set(false);
            self.scroll_to_caret();
        }
        let images = self.visible_images(window, cx);
        let frame = Frame {
            laid: self.laid.clone(),
            images,
            sel: self.sel,
            marked: self.marked.clone(),
            zoom: self.zoom,
            scroll: self.scroll,
            focused: self.focus.is_focused(window),
            guide: self.ruler_drag.map(|d| d.x),
            found: self.found.clone(),
            found_current: self.found_current,
            t,
            bridge: self.bridge.clone(),
            focus: self.focus.clone(),
            view: cx.entity(),
        };
        let viewport = self.viewport.clone();
        let reveal = self.reveal_caret.clone();
        let entity = cx.entity().downgrade();
        let page = canvas(
            move |bounds, _, cx| {
                let old = viewport.replace(bounds);
                // First paint (or a resize): re-render once the size is known
                // so scrolling to the caret and clamping use real numbers.
                if old.size != bounds.size && (reveal.get() || old.size.height <= px(0.)) {
                    let _ = entity.update(cx, |_, cx| cx.notify());
                }
            },
            move |bounds, _, window, cx| paint_frame(&frame, bounds, window, cx),
        )
        .size_full();

        let area = div()
            .id("docs-page")
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.dragging = false),
            )
            .on_scroll(cx.listener(Self::on_scroll))
            .child(page)
            .when(self.find.read(cx).is_open(), |el| {
                el.child(self.find.clone())
            });

        let root = v_flex()
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(t.bg)
            .text_color(t.fg)
            .text_size(FONT_BODY);
        let root = listeners!(root, cx, [
            MoveLeft => |this, _w, cx| this.horizontal(false, false, false, cx),
            MoveRight => |this, _w, cx| this.horizontal(true, false, false, cx),
            SelectLeft => |this, _w, cx| this.horizontal(false, false, true, cx),
            SelectRight => |this, _w, cx| this.horizontal(true, false, true, cx),
            MoveWordLeft => |this, _w, cx| this.horizontal(false, true, false, cx),
            MoveWordRight => |this, _w, cx| this.horizontal(true, true, false, cx),
            SelectWordLeft => |this, _w, cx| this.horizontal(false, true, true, cx),
            SelectWordRight => |this, _w, cx| this.horizontal(true, true, true, cx),
            MoveUp => |this, _w, cx| this.vertical(false, false, cx),
            MoveDown => |this, _w, cx| this.vertical(true, false, cx),
            SelectUp => |this, _w, cx| this.vertical(false, true, cx),
            SelectDown => |this, _w, cx| this.vertical(true, true, cx),
            MoveLineStart => |this, _w, cx| this.line_edge(false, false, cx),
            MoveLineEnd => |this, _w, cx| this.line_edge(true, false, cx),
            SelectLineStart => |this, _w, cx| this.line_edge(false, true, cx),
            SelectLineEnd => |this, _w, cx| this.line_edge(true, true, cx),
            MoveDocStart => |this, _w, cx| this.move_to(Position::default(), false, cx),
            MoveDocEnd => |this, _w, cx| { let e = this.doc.end(); this.move_to(e, false, cx) },
            SelectDocStart => |this, _w, cx| this.move_to(Position::default(), true, cx),
            SelectDocEnd => |this, _w, cx| { let e = this.doc.end(); this.move_to(e, true, cx) },
            PageUp => |this, _w, cx| this.page_move(false, false, cx),
            PageDown => |this, _w, cx| this.page_move(true, false, cx),
            SelectPageUp => |this, _w, cx| this.page_move(false, true, cx),
            SelectPageDown => |this, _w, cx| this.page_move(true, true, cx),
            SelectAll => |this, _w, cx| {
                this.sel = Selection { anchor: Position::default(), head: this.doc.end() };
                cx.notify()
            },
            Collapse => |this, _w, cx| {
                this.table_picker = None;
                this.page_menu = None;
                this.ruler_drag = None;
                let h = this.sel.head;
                this.move_to(h, false, cx)
            },
            Backspace => |this, w, cx| { let s = this.doc.backspace(this.sel); this.edited(s, w, cx) },
            Delete => |this, w, cx| { let s = this.doc.delete_forward(this.sel); this.edited(s, w, cx) },
            DeleteWordBack => |this, w, cx| {
                if this.sel.is_empty() { this.sel.head = this.doc.prev_word_position(this.sel.head); }
                let s = this.doc.delete_selection(this.sel);
                this.edited(s, w, cx)
            },
            DeleteWordForward => |this, w, cx| {
                if this.sel.is_empty() { this.sel.head = this.doc.next_word_position(this.sel.head); }
                let s = this.doc.delete_selection(this.sel);
                this.edited(s, w, cx)
            },
            Newline => |this, w, cx| {
                let props = this.typing_props.take();
                let s = this.doc.newline(this.sel, props);
                this.marked = None;
                this.edited(s, w, cx)
            },
            LineBreak => |this, w, cx| this.insert(&docs_engine::LINE_BREAK.to_string(), w, cx),
            Tab => |this, w, cx| this.tab(false, w, cx),
            BackTab => |this, w, cx| this.tab(true, w, cx),
            ToggleBullets => |this, w, cx| this.toggle_list_doc(ListKind::Bullet, w, cx),
            ToggleNumbered => |this, w, cx| this.toggle_list_doc(ListKind::Numbered, w, cx),
            IndentList => |this, w, cx| this.list_level_doc(1, w, cx),
            OutdentList => |this, w, cx| this.list_level_doc(-1, w, cx),
            InsertTable => |this, w, cx| this.insert_table_doc(3, 3, w, cx),
            InsertImage => |this, w, cx| this.insert_image_doc(w, cx),
            InsertRowAbove => |this, w, cx| this.table_edit_doc(TableEdit::RowAbove, w, cx),
            InsertRowBelow => |this, w, cx| this.table_edit_doc(TableEdit::RowBelow, w, cx),
            InsertColumnLeft => |this, w, cx| this.table_edit_doc(TableEdit::ColumnLeft, w, cx),
            InsertColumnRight => |this, w, cx| this.table_edit_doc(TableEdit::ColumnRight, w, cx),
            DeleteRow => |this, w, cx| this.table_edit_doc(TableEdit::DeleteRow, w, cx),
            DeleteColumn => |this, w, cx| this.table_edit_doc(TableEdit::DeleteColumn, w, cx),
            DeleteTable => |this, w, cx| this.table_edit_doc(TableEdit::DeleteTable, w, cx),
            Copy => |this, w, cx| this.copy_doc(w, cx),
            Cut => |this, w, cx| this.cut_doc(w, cx),
            Paste => |this, w, cx| this.paste_doc(w, cx),
            ToggleBold => |this, w, cx| this.toggle(Toggle::Bold, w, cx),
            ToggleItalic => |this, w, cx| this.toggle(Toggle::Italic, w, cx),
            ToggleUnderline => |this, w, cx| this.toggle(Toggle::Underline, w, cx),
            StyleNormal => |this, w, cx| this.set_style_doc(None, w, cx),
            StyleHeading1 => |this, w, cx| this.set_style_doc(Some("Heading1"), w, cx),
            StyleHeading2 => |this, w, cx| this.set_style_doc(Some("Heading2"), w, cx),
            StyleHeading3 => |this, w, cx| this.set_style_doc(Some("Heading3"), w, cx),
            AlignLeft => |this, w, cx| this.set_align_doc(Align::Left, w, cx),
            AlignCenter => |this, w, cx| this.set_align_doc(Align::Center, w, cx),
            AlignRight => |this, w, cx| this.set_align_doc(Align::Right, w, cx),
            AlignJustify => |this, w, cx| this.set_align_doc(Align::Justify, w, cx),
            Undo => |this, w, cx| this.undo_doc(w, cx),
            Redo => |this, w, cx| this.redo_doc(w, cx),
            ZoomIn => |this, _w, cx| this.zoom_by(ZOOM_STEP, cx),
            ZoomOut => |this, _w, cx| this.zoom_by(1.0 / ZOOM_STEP, cx),
            ZoomReset => |this, _w, cx| this.set_zoom(1.0, cx),
        ]);
        use kantor_ui::find_bar as fb;
        root.on_action(cx.listener(Self::save))
            .on_action(cx.listener(Self::save_as))
            .on_action(cx.listener(|this, _: &fb::OpenFind, w, cx| this.open_find(false, w, cx)))
            .on_action(cx.listener(|this, _: &fb::OpenReplace, w, cx| this.open_find(true, w, cx)))
            .on_action(cx.listener(|this, _: &fb::FindNext, w, cx| this.find_next_doc(true, w, cx)))
            .on_action(
                cx.listener(|this, _: &fb::FindPrev, w, cx| this.find_next_doc(false, w, cx)),
            )
            .child(toolbar::render(self, cx))
            .children(self.render_banner(&t))
            .children(self.render_ruler(cx))
            .child(area)
            .children(self.render_table_picker(cx))
            .children(self.render_page_menu(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use docs_engine::Paragraph;

    fn laid(doc: &Document) -> (Vec<docs_layout::Paragraph>, PagedDocument) {
        let mut engine = Engine::new();
        let setup = blocks::page_setup(doc);
        let (paras, widths, blocks) = blocks::build(doc);
        let layouts = paras
            .iter()
            .zip(&widths)
            .map(|(p, w)| Arc::new(engine.layout_paragraph(p, *w)))
            .collect();
        let paged = PagedDocument::paginate_blocks(&paras, layouts, &blocks, setup);
        (paras, paged)
    }

    #[std::prelude::v1::test]
    fn new_document_lays_out_one_a4_page_with_a_caret() {
        let doc = Document::new();
        let (paras, paged) = laid(&doc);
        assert_eq!(paged.pages.len(), 1);
        assert!((paged.setup.width - 793.7).abs() < 0.1, "A4 from twips");
        assert_eq!(paras[0].size_pt, 11.0);
        let c = paged.caret(0, 0).expect("caret in an empty document");
        assert!(c.height > 10.0 && c.height < 25.0, "{c:?}");
    }

    #[std::prelude::v1::test]
    fn runs_resolve_to_spans_and_keys_track_content() {
        let mut doc = Document::from_paragraphs(vec![
            Paragraph::new("Judul").with_style("Heading1"),
            Paragraph::new("satu\tdua"),
        ]);
        let end = doc.end();
        doc.insert_text(
            Selection::caret(end),
            " tebal",
            Some(RunProps {
                bold: Some(true),
                ..RunProps::default()
            }),
        );
        let (paras, paged) = laid(&doc);
        assert_eq!(paras[0].spans[0].size_pt, Some(16.0));
        assert_eq!(
            paras[1].text, "satu\tdua tebal",
            "tabs are the layout's job"
        );
        assert!(paras[1].spans.iter().any(|s| s.bold));
        assert!(!paged.glyph_runs(0).is_empty());
        let w = paged.setup.content_width();
        assert_eq!(layout_key(&paras[1], w), layout_key(&paras[1].clone(), w));
        assert_ne!(layout_key(&paras[0], w), layout_key(&paras[1], w));
        assert_ne!(layout_key(&paras[1], w), layout_key(&paras[1], w + 1.0));
    }

    #[std::prelude::v1::test]
    fn pictures_tables_and_lists_reach_the_page() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend(200u32.to_be_bytes());
        png.extend(100u32.to_be_bytes());
        png.extend([8, 6, 0, 0, 0]);
        let mut doc = Document::new();
        let c = doc.insert_text(Selection::default(), "Lihat ", None);
        doc.insert_image(c, png, "grafik.png").unwrap();
        doc.toggle_list(Selection::caret(Position::new(0, 0)), ListKind::Bullet);
        doc.insert_table(Selection::caret(doc.end()), 2, 2);
        let (_, paged) = laid(&doc);
        let objects = paged.objects(0);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind, ObjectKind::Image);
        assert!((objects[0].rect[2] - 200.0).abs() < 0.5, "{:?}", objects[0]);
        assert!(!paged.pages[0].rules.is_empty(), "table borders");
        assert!(
            paged
                .glyph_runs(0)
                .iter()
                .any(|r| r.source == RunSource::Marker),
            "bullet drawn"
        );
    }

    #[std::prelude::v1::test]
    fn counts_read_like_a_status_bar() {
        assert_eq!(
            count_text((1234, 6789), None),
            "1,234 words · 6,789 characters"
        );
        assert_eq!(count_text((1, 1), None), "1 word · 1 character");
        assert_eq!(
            count_text((1234, 6789), Some((12, 60))),
            "12 of 1,234 words · 60 characters"
        );
    }

    #[std::prelude::v1::test]
    fn geometry_round_trips_and_culls_pages() {
        let g = Geometry {
            origin: point(px(100.), px(50.)),
            size: size(px(1000.), px(800.)),
            zoom: 1.5,
            scroll: point(0., 1500.),
            page_w: 793.7,
            page_h: 1122.5,
        };
        let p = g.page_to_screen(1, 120.0, 300.0);
        let (page, x, y) = g.screen_to_page(p, 10);
        assert_eq!(page, 1);
        assert!((x - 120.0).abs() < 0.01 && (y - 300.0).abs() < 0.01);
        let vis = g.visible_pages(10);
        assert!(vis.contains(&1) && !vis.contains(&5), "{vis:?}");
    }
}
