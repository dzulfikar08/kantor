//! Kantor PDF: a PDF viewer pane.
//!
//! Pages are drawn by [`pdf_engine`] (hayro) on a background thread
//! ([`worker`]) and shown in one continuous column ([`layout`]); images are
//! cached up to a byte budget, so a 1000-page file costs no more memory
//! than a few screens of it. While a new zoom is being drawn the old image
//! shows stretched.
//!
//! Around the pages: a toolbar ([`toolbar`]: sidebar, page box, zoom,
//! rotate, find, print), a sidebar with thumbnails or bookmarks ([`side`]),
//! the shared Find bar ([`find`]). On the pages: drag selects text (double
//! click a word, triple click a line), Ctrl+C copies it, links go to their
//! place in the document or, after asking, open in the browser.
//!
//! Pages can be organized ([`organize`]): selected in the thumbnails or
//! the Organize Pages grid ([`grid`]), dragged to a new place (or to
//! another open PDF), rotated, deleted, duplicated, copied and pasted,
//! inserted from files; and the whole document stamped with page numbers
//! or a watermark, split, or combined with other files ([`dialog`]). The
//! document is a list of pages of sources ([`model`]): an edit is instant
//! and only pages that changed are drawn again. Save writes the file in
//! place, safely ([`save`]).
//! A protected file asks for its password; a damaged one says so.

extern crate gpui_kit as gpui;

mod dialog;
mod find;
mod grid;
mod layout;
mod model;
mod organize;
mod save;
mod side;
mod toolbar;
mod worker;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{DialogDescription, DialogFooter};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{Sizable as _, WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::find_bar::{FindNext, FindPrev, OpenFind, OpenReplace};
use kantor_ui::i18n::{self, tf};
use kantor_ui::tokens::Elevation;
use kantor_ui::{metrics::*, tokens};
use pdf_engine::{Dest, Link, LinkTarget, OpenError, OutlineItem, PageText, Turn};

use model::{Model, PageKey, PageRef};

pub use dialog::PdfEvent;
pub use layout::Zoom;
use layout::{Column, ImageCache, PX_PER_PT};
use worker::{Msg, PageJob, Worker};

/// Key context of the page area.
pub const KEY_CONTEXT: &str = "PdfViewer";

/// Bytes of page images kept (pages on screen are kept even past it).
const PAGE_BUDGET: usize = 96 << 20;
/// Bytes of thumbnails kept.
const THUMB_BUDGET: usize = 24 << 20;
/// A thumbnail's box, px.
pub(crate) const THUMB_W: f32 = 112.0;
pub(crate) const THUMB_H: f32 = 144.0;
/// Width of the sidebar, px.
pub(crate) const SIDEBAR_W: f32 = 200.0;
/// One arrow key's scroll, px.
const LINE: f32 = 40.0;

actions!(
    pdf,
    [
        ScrollUp,
        ScrollDown,
        ScrollLeft,
        ScrollRight,
        PageUp,
        PageDown,
        FirstPage,
        LastPage,
        ZoomIn,
        ZoomOut,
        ActualSize,
        FitPage,
        FitWidth,
        RotateCw,
        RotateCcw,
        ToggleSidebar,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Escape,
        Save,
        SaveAs,
        Undo,
        Redo,
        DeletePages,
        SelectAllPages,
        PrevPageSel,
        NextPageSel,
        PrevPageSelExtend,
        NextPageSelExtend,
        RowUpSel,
        RowDownSel,
        RotatePagesLeft,
        RotatePagesRight,
        ToggleOrganize,
        LeavePages,
    ]
);

/// Register the viewer's key bindings.
pub fn init(cx: &mut App) {
    kantor_ui::find_bar::bind_app_keys(KEY_CONTEXT, cx);
    let c = Some(KEY_CONTEXT);
    let mut keys = vec![
        KeyBinding::new("up", ScrollUp, c),
        KeyBinding::new("down", ScrollDown, c),
        KeyBinding::new("left", ScrollLeft, c),
        KeyBinding::new("right", ScrollRight, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("shift-space", PageUp, c),
        KeyBinding::new("space", PageDown, c),
        KeyBinding::new("home", FirstPage, c),
        KeyBinding::new("end", LastPage, c),
        KeyBinding::new("f4", ToggleSidebar, c),
        KeyBinding::new("escape", Escape, c),
    ];
    keys.push(KeyBinding::new(
        kantor_ui::keys::primary("ctrl-home"),
        FirstPage,
        c,
    ));
    keys.push(KeyBinding::new(
        kantor_ui::keys::primary("ctrl-end"),
        LastPage,
        c,
    ));
    macro_rules! primary {
        ($($key:literal => $action:expr),* $(,)?) => {
            $(keys.extend(kantor_ui::keys::primary_pair(concat!("ctrl-", $key), $action, c));)*
        };
    }
    // Zoom as in Acrobat (Ctrl+0 fit page, Ctrl+1 actual size, Ctrl+2 fit
    // width); rotate view as in browsers' PDF viewers (Ctrl+] / Ctrl+[).
    primary!(
        "=" => ZoomIn,
        "+" => ZoomIn,
        "shift-=" => ZoomIn,
        "-" => ZoomOut,
        "0" => FitPage,
        "1" => ActualSize,
        "2" => FitWidth,
        "]" => RotateCw,
        "[" => RotateCcw,
        "a" => SelectAll,
        "c" => Copy,
        "x" => Cut,
        "v" => Paste,
        "s" => Save,
        "shift-s" => SaveAs,
        "z" => Undo,
        "shift-z" => Redo,
        "y" => Redo,
        "shift-o" => ToggleOrganize,
    );
    // The thumbnails and the grid (as in Acrobat's page panel).
    let p = Some(grid::PAGES_CONTEXT);
    keys.extend([
        KeyBinding::new("delete", DeletePages, p),
        KeyBinding::new("backspace", DeletePages, p),
        KeyBinding::new("up", PrevPageSel, p),
        KeyBinding::new("down", NextPageSel, p),
        KeyBinding::new("left", PrevPageSel, p),
        KeyBinding::new("right", NextPageSel, p),
        KeyBinding::new("shift-up", PrevPageSelExtend, p),
        KeyBinding::new("shift-down", NextPageSelExtend, p),
        KeyBinding::new("shift-left", PrevPageSelExtend, p),
        KeyBinding::new("shift-right", NextPageSelExtend, p),
        KeyBinding::new("pageup", RowUpSel, p),
        KeyBinding::new("pagedown", RowDownSel, p),
        KeyBinding::new("escape", LeavePages, p),
    ]);
    keys.extend(kantor_ui::keys::primary_pair("ctrl-a", SelectAllPages, p));
    // Shift+[ types `{` on most layouts: every spelling.
    for k in ["ctrl-shift-[", "ctrl-{", "ctrl-shift-{"] {
        keys.extend(kantor_ui::keys::primary_pair(k, RotatePagesLeft, p));
    }
    for k in ["ctrl-shift-]", "ctrl-}", "ctrl-shift-}"] {
        keys.extend(kantor_ui::keys::primary_pair(k, RotatePagesRight, p));
    }
    cx.bind_keys(keys);
}

/// What the pane shows.
enum State {
    Loading,
    /// Waiting for the password (`wrong`: the last one didn't open it).
    Password {
        wrong: bool,
    },
    Failed(OpenError),
    Ready(Box<Doc>),
}

/// The open file, as the UI knows it.
struct Doc {
    model: Model,
    /// Each page's size as shown (its own rotation applied), points.
    sizes: Vec<(f32, f32)>,
}

/// A place in the text: page and byte offset in its [`PageText`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct TextPos {
    pub page: usize,
    pub offset: usize,
}

/// A mouse press on the pages, until it is released.
enum Press {
    /// On a link: followed if the mouse doesn't move away.
    Link(LinkTarget, Point<Pixels>),
    /// Selecting text.
    Select,
}

/// What is under the pointer (the cursor shape).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    None,
    Text,
    Link,
}

/// Which sidebar panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Panel {
    Thumbnails,
    Bookmarks,
}

pub struct PdfView {
    path: PathBuf,
    name: String,
    title: Option<String>,
    focus: FocusHandle,
    status: SharedString,
    state: State,
    worker: Option<Rc<Worker>>,
    _receiver: Option<Task<()>>,
    password: Entity<InputState>,
    page_box: Entity<InputState>,
    find: find::PdfFind,
    zoom: Zoom,
    turn: Turn,
    /// Scroll offset into the column, px.
    scroll: Point<f32>,
    /// Bounds of the page area (from the last paint).
    viewport: Rc<Cell<Bounds<Pixels>>>,
    column: Column,
    column_key: (u32, Turn, usize),
    panel: Option<Panel>,
    pages: ImageCache,
    thumbs: Rc<RefCell<ImageCache>>,
    /// Text and links of each page, by place (from [`Self::src_texts`]).
    texts: HashMap<usize, Arc<PageText>>,
    links: HashMap<usize, Vec<Link>>,
    /// Text and links read, by source page (links still in source pages).
    src_texts: HashMap<PageKey, (Arc<PageText>, Vec<Link>)>,
    asked_text: HashSet<PageKey>,
    /// Pages selected in the thumbnails or the grid, and the page Shift+click
    /// extends from.
    page_sel: BTreeSet<usize>,
    sel_anchor: Option<usize>,
    /// The thumbnails and the grid: Delete, Ctrl+A, Ctrl+C… act on pages.
    pages_focus: FocusHandle,
    /// The Organize Pages grid replaces the pages.
    organize: bool,
    /// Where dragged pages or files would go (a gap before this page).
    drop_at: Option<usize>,
    grid_scroll: UniformListScrollHandle,
    /// Bounds of the thumbnails list and the grid (last paint).
    thumbs_bounds: Rc<Cell<Bounds<Pixels>>>,
    grid_bounds: Rc<Cell<Bounds<Pixels>>>,
    dialog: Option<dialog::Dialog>,
    dialog_focus: FocusHandle,
    /// Work in the background (saving, inserting…): its message.
    busy: Option<SharedString>,
    disk_guard: Option<kantor_ui::disk_guard::DiskGuard>,
    sel: Option<(TextPos, TextPos)>,
    press: Option<Press>,
    hover: Hover,
    /// Expanded bookmarks, by their path of child indexes.
    expanded: HashSet<Vec<usize>>,
    /// The thumbnail list, kept on the current page.
    thumb_scroll: UniformListScrollHandle,
    thumb_followed: Option<usize>,
    /// Put the caret in the password field on the next frame.
    focus_password: bool,
    saving: bool,
    /// Opened from a recovery copy: these bytes, unsaved changes.
    recovered: Option<Arc<Vec<u8>>>,
    /// Mark the document as changed once it opens (a recovery copy).
    opened_dirty: Option<u64>,
    entity_id: EntityId,
    dialog_subs: Vec<Subscription>,
    _subs: Vec<Subscription>,
}

impl PdfView {
    pub fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let password = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(i18n::t("Password"))
        });
        let page_box = cx.new(|cx| InputState::new(window, cx));
        let subs = vec![
            cx.subscribe_in(&password, window, |this, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.submit_password(window, cx);
                }
            }),
            cx.subscribe_in(
                &page_box,
                window,
                |this, input, ev: &InputEvent, window, cx| {
                    match ev {
                        InputEvent::PressEnter { .. } => {
                            let typed = input.read(cx).value().trim().to_owned();
                            match typed.parse::<usize>() {
                                Ok(n) if n >= 1 && n <= this.page_count() => {
                                    this.go_to(
                                        &Dest {
                                            page: n - 1,
                                            x: None,
                                            y: None,
                                        },
                                        cx,
                                    );
                                }
                                _ => {
                                    this.status =
                                        tf("There is no page {} in this document", &[&typed])
                                            .into();
                                }
                            }
                            this.focus(window, cx);
                            cx.notify();
                        }
                        InputEvent::Blur => {
                            // Shows the current page again (render refills it).
                            input.update(cx, |i, cx| i.set_value("", window, cx));
                            cx.notify();
                        }
                        _ => {}
                    }
                },
            ),
        ];
        let find = find::PdfFind::new(window, cx);
        let path = path.unwrap_or_default();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "PDF".into());
        let mut this = Self {
            path,
            name,
            title: None,
            focus: cx.focus_handle(),
            status: SharedString::default(),
            state: State::Loading,
            worker: None,
            _receiver: None,
            password,
            page_box,
            find,
            zoom: Zoom::FitWidth,
            turn: Turn::default(),
            scroll: Point::default(),
            viewport: Rc::default(),
            column: Column::default(),
            column_key: (0, Turn(0), 0),
            panel: None,
            pages: ImageCache::new(PAGE_BUDGET),
            thumbs: Rc::new(RefCell::new(ImageCache::new(THUMB_BUDGET))),
            texts: HashMap::new(),
            links: HashMap::new(),
            src_texts: HashMap::new(),
            asked_text: HashSet::new(),
            page_sel: BTreeSet::new(),
            sel_anchor: None,
            pages_focus: cx.focus_handle(),
            organize: false,
            drop_at: None,
            grid_scroll: UniformListScrollHandle::new(),
            thumbs_bounds: Rc::default(),
            grid_bounds: Rc::default(),
            dialog: None,
            dialog_focus: cx.focus_handle(),
            busy: None,
            disk_guard: None,
            sel: None,
            press: None,
            hover: Hover::None,
            expanded: HashSet::new(),
            thumb_scroll: UniformListScrollHandle::new(),
            thumb_followed: None,
            focus_password: false,
            saving: false,
            recovered: None,
            opened_dirty: None,
            entity_id: cx.entity_id(),
            dialog_subs: Vec::new(),
            _subs: subs,
        };
        this.start(String::new(), cx);
        this
    }

    /// A view of a recovery copy (`bytes`) of the PDF at `original`: it
    /// opens with unsaved changes; Save writes the original file.
    pub fn new_recovered(
        bytes: Vec<u8>,
        original: Option<PathBuf>,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(original.clone(), window, cx);
        if original.is_none() {
            this.name = name;
        }
        this.recovered = Some(Arc::new(bytes));
        this.opened_dirty = Some(u64::MAX);
        this.start(String::new(), cx);
        this
    }

    /// (Re)open the file with `password`.
    fn start(&mut self, password: String, cx: &mut Context<Self>) {
        let (tx, rx) = async_channel::unbounded();
        let worker = match &self.recovered {
            Some(bytes) => Worker::start_bytes(self.name.clone(), bytes.clone(), password, tx),
            None => Worker::start(self.path.clone(), password, tx),
        };
        self.worker = Some(Rc::new(worker));
        self.state = State::Loading;
        self._receiver = Some(cx.spawn(async move |this, cx| {
            while let Ok(msg) = rx.recv().await {
                let mut batch = vec![msg];
                while let Ok(m) = rx.try_recv() {
                    batch.push(m);
                }
                if this
                    .update(cx, |this, cx| {
                        for m in batch {
                            this.on_msg(m, cx);
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn on_msg(&mut self, msg: Msg, cx: &mut Context<Self>) {
        match msg {
            Msg::Opened(source) => {
                let n = source.sizes.len();
                self.expanded = initially_open(&source.outline);
                if !source.outline.is_empty() && self.panel.is_none() {
                    self.panel = Some(Panel::Bookmarks);
                }
                self.title = source.title.clone();
                let model = Model::new(source);
                self.state = State::Ready(Box::new(Doc {
                    sizes: model.sizes(),
                    model,
                }));
                if let Some(rev) = self.opened_dirty.take()
                    && let State::Ready(d) = &mut self.state
                {
                    d.model.mark_saved(rev);
                }
                self.status = if n == 1 {
                    tf("Opened {} (1 page)", &[&self.name]).into()
                } else {
                    tf("Opened {} ({} pages)", &[&self.name, &n]).into()
                };
            }
            Msg::Failed(OpenError::Password { wrong }) => {
                self.state = State::Password { wrong };
                self.focus_password = true;
                self.status = i18n::t("This document is password-protected").into();
            }
            Msg::Failed(e) => {
                log::warn!("opening {}: {e}", self.path.display());
                self.status = tf("Kantor can't open {}", &[&self.name]).into();
                self.state = State::Failed(e);
            }
            Msg::Page {
                job,
                thumb,
                image,
                bytes,
            } => {
                if thumb {
                    self.thumbs.borrow_mut().insert(
                        job.key,
                        job.turn,
                        job.scale,
                        image,
                        bytes,
                        &[job.key],
                    );
                } else {
                    let keep: Vec<PageKey> = self
                        .visible_pages()
                        .filter_map(|i| self.page_ref(i).map(PageRef::key))
                        .collect();
                    self.pages
                        .insert(job.key, job.turn, job.scale, image, bytes, &keep);
                }
            }
            Msg::Text { key, text, links } => {
                self.src_texts.insert(key, (text.clone(), links));
                let places: Vec<usize> = self
                    .doc()
                    .map(|d| {
                        d.model
                            .pages()
                            .iter()
                            .enumerate()
                            .filter(|(_, p)| p.key() == key)
                            .map(|(i, _)| i)
                            .collect()
                    })
                    .unwrap_or_default();
                for page in places {
                    self.find.on_text(page, &text);
                    self.place_text(page);
                }
                self.find_after_text(cx);
            }
        }
    }

    fn submit_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pw = self.password.read(cx).value().to_string();
        if pw.is_empty() {
            return;
        }
        self.password
            .update(cx, |i, cx| i.set_value("", window, cx));
        self.start(pw, cx);
        cx.notify();
    }

    fn doc(&self) -> Option<&Doc> {
        match &self.state {
            State::Ready(d) => Some(d),
            _ => None,
        }
    }

    fn doc_mut(&mut self) -> Option<&mut Doc> {
        match &mut self.state {
            State::Ready(d) => Some(d),
            _ => None,
        }
    }

    pub fn page_count(&self) -> usize {
        self.doc().map_or(0, |d| d.sizes.len())
    }

    /// The page at place `i`.
    pub(crate) fn page_ref(&self, i: usize) -> Option<PageRef> {
        self.doc()?.model.page(i)
    }

    /// The view's rotation plus page `i`'s own (the turn its text, links
    /// and image are mapped with).
    pub(crate) fn page_turn(&self, i: usize) -> Turn {
        let rot = self.page_ref(i).map_or(0, |p| p.rot);
        Turn((self.turn.0 + rot) % 4)
    }

    /// The bookmarks of the document (its base file's).
    pub(crate) fn outline(&self) -> &[OutlineItem] {
        self.doc()
            .map_or(&[], |d| d.model.base().outline.as_slice())
    }

    /// Put the text and links read for page `i`'s source page at place `i`,
    /// links to other pages pointing where those pages are now (links to
    /// pages that are gone are left out).
    fn place_text(&mut self, i: usize) {
        let Some(p) = self.page_ref(i) else {
            return;
        };
        let Some((text, links)) = self.src_texts.get(&p.key()).cloned() else {
            self.texts.remove(&i);
            self.links.remove(&i);
            return;
        };
        let links = links
            .into_iter()
            .filter_map(|mut l| {
                l.target = self.map_target(p.src, l.target)?;
                Some(l)
            })
            .collect();
        self.texts.insert(i, text);
        self.links.insert(i, links);
    }

    /// A link target of source `src` as a target in the document now:
    /// `None` when its page was deleted.
    pub(crate) fn map_target(&self, src: model::SrcId, target: LinkTarget) -> Option<LinkTarget> {
        match target {
            LinkTarget::Dest(mut d) => {
                d.page = self.doc()?.model.index_of((src, d.page))?;
                Some(LinkTarget::Dest(d))
            }
            uri => Some(uri),
        }
    }

    // ── the shell's interface ──────────────────────────────────────────────

    /// What the inspector shows.
    pub fn info(&self) -> PdfInfo {
        let bytes = std::fs::metadata(&self.path).map_or(0, |m| m.len());
        let dirty = self.doc().is_some_and(|d| d.model.is_dirty());
        PdfInfo {
            name: self.name.clone(),
            path: self.path.clone(),
            title: self.title.clone(),
            pages: self.page_count(),
            current: self.current_page(),
            size: size_text(bytes),
            dirty,
        }
    }

    pub fn doc_name(&self, _: &App) -> String {
        self.name.clone()
    }

    pub fn path(&self, _: &App) -> Option<PathBuf> {
        (!self.path.as_os_str().is_empty()).then(|| self.path.clone())
    }

    pub fn title(&self, _: &App) -> String {
        match &self.title {
            Some(t) if t != &self.name => format!("{} ({t}) — Kantor PDF", self.name),
            _ => format!("{} — Kantor PDF", self.name),
        }
    }

    pub fn status(&self) -> SharedString {
        self.busy.clone().unwrap_or_else(|| self.status.clone())
    }

    /// Copy: the selected pages when the thumbnails or the grid have the
    /// focus, else the selected text.
    pub fn copy_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages_focused(window, cx) {
            self.copy_pages(cx);
        } else {
            self.copy_text(cx);
        }
    }

    /// Cut: pages (thumbnails or grid focused); text is only copied.
    pub fn cut_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pages_focused(window, cx) {
            self.cut_pages(cx);
        } else {
            self.copy_text(cx);
        }
    }

    /// Paste: copied pages after the selected ones (or the current page).
    pub fn paste_doc(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.paste_pages(cx);
    }

    fn pages_focused(&self, window: &Window, cx: &App) -> bool {
        self.organize || self.pages_focus.contains_focused(window, cx)
    }

    /// Organize Pages: the grid of pages instead of the pages themselves.
    pub fn toggle_organize(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.organize = !self.organize;
        if self.organize {
            let cur = self.current_page();
            if self.page_sel.is_empty() {
                self.page_sel.insert(cur);
                self.sel_anchor = Some(cur);
            }
            let cols = self.grid_columns();
            self.grid_scroll
                .scroll_to_item(cur / cols.max(1), ScrollStrategy::Center);
            window.focus(&self.pages_focus, cx);
        } else {
            if let Some(&p) = self.page_sel.iter().next() {
                self.go_to_page(p, cx);
            }
            self.focus(window, cx);
        }
        cx.notify();
    }

    pub fn is_organizing(&self) -> bool {
        self.organize
    }

    /// For the status bar: `Page 2 of 12`.
    pub fn location(&self) -> String {
        match self.page_count() {
            0 => String::new(),
            n => tf("Page {} of {}", &[&(self.current_page() + 1), &n]),
        }
    }

    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
    }

    pub fn editor_focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn zoom(&self) -> Zoom {
        self.zoom
    }

    pub fn panel(&self) -> Option<Panel> {
        self.panel
    }

    // ── geometry ───────────────────────────────────────────────────────────

    fn view_size(&self) -> (f32, f32) {
        let b = self.viewport.get();
        (f32::from(b.size.width), f32::from(b.size.height))
    }

    /// The zoom in effect (Fit Width / Fit Page depend on the view).
    pub fn zoom_value(&self) -> f32 {
        let Some(doc) = self.doc() else {
            return 1.0;
        };
        let (vw, vh) = self.view_size();
        match self.zoom {
            Zoom::Percent(z) => z,
            _ if vw <= 0.0 => 1.0,
            Zoom::FitWidth => layout::fit_width(&doc.sizes, self.turn, vw),
            Zoom::FitPage => layout::fit_page(&doc.sizes, self.current_page(), self.turn, (vw, vh)),
        }
    }

    /// Rebuild the column if the zoom, rotation or file changed.
    fn sync_column(&mut self) {
        let z = self.zoom_value();
        let key = (z.to_bits(), self.turn, self.page_count());
        if key != self.column_key {
            // The view's width changed the fit (sidebar, window size): keep
            // the same place of the same page at the top.
            let keep = (self.column.len() == key.2 && self.column_key.1 == key.1)
                .then(|| {
                    let page = self.column.page_at(self.scroll.y);
                    let (_, h) = self.column.sizes.get(page)?;
                    let top = self.column.tops.get(page)?;
                    Some((page, (self.scroll.y - top) / h.max(1.0)))
                })
                .flatten();
            self.column_key = key;
            self.column = match self.doc() {
                Some(d) => Column::new(&d.sizes, z, self.turn),
                None => Column::default(),
            };
            if let Some((page, rel)) = keep
                && let (Some(top), Some((_, h))) =
                    (self.column.tops.get(page), self.column.sizes.get(page))
            {
                self.scroll.y = top + rel * h;
            }
        }
    }

    fn clamp_scroll(&mut self) {
        let (vw, vh) = self.view_size();
        let max_x = (self.column.width - vw).max(0.0);
        let max_y = (self.column.height - vh).max(0.0);
        self.scroll.x = self.scroll.x.clamp(0.0, max_x);
        self.scroll.y = self.scroll.y.clamp(0.0, max_y);
    }

    fn visible_pages(&self) -> Range<usize> {
        let (_, vh) = self.view_size();
        self.column
            .visible(self.scroll.y, self.scroll.y + vh.max(1.0))
    }

    /// The page the user is on: the one a third of the way down the view.
    pub fn current_page(&self) -> usize {
        let (_, vh) = self.view_size();
        self.column.page_at(self.scroll.y + vh / 3.0)
    }

    /// A page's rectangle in the viewport, px.
    fn page_rect(&self, page: usize) -> Bounds<Pixels> {
        let (vw, _) = self.view_size();
        let (w, h) = self.column.sizes.get(page).copied().unwrap_or_default();
        let x = self.column.left(page, vw) - self.scroll.x;
        let y = self.column.tops.get(page).copied().unwrap_or_default() - self.scroll.y;
        Bounds::new(point(px(x), px(y)), size(px(w), px(h)))
    }

    /// A page-space rectangle on screen (viewport px).
    fn screen_rect(&self, page: usize, r: [f32; 4]) -> Bounds<Pixels> {
        let pr = self.page_rect(page);
        let (w, h) = self.page_points(page);
        let r = self.page_turn(page).map_rect(w, h, r);
        let k = self.zoom_value() * PX_PER_PT;
        Bounds::from_corners(
            point(pr.origin.x + px(r[0] * k), pr.origin.y + px(r[1] * k)),
            point(pr.origin.x + px(r[2] * k), pr.origin.y + px(r[3] * k)),
        )
    }

    /// Page `page`'s size in its own space (without its extra rotation).
    fn page_points(&self, page: usize) -> (f32, f32) {
        match (self.doc(), self.page_ref(page)) {
            (Some(d), Some(p)) => d.model.unturned_size(p),
            _ => (612.0, 792.0),
        }
    }

    /// The page under a window position and the point on it (page space);
    /// `near`: between pages, the closest page instead of nothing.
    fn hit(&self, pos: Point<Pixels>, near: bool) -> Option<(usize, f32, f32)> {
        if self.column.len() == 0 {
            return None;
        }
        let vp = self.viewport.get();
        let local = pos - vp.origin;
        let y = f32::from(local.y) + self.scroll.y;
        let mut page = self.column.page_at(y);
        let r = self.page_rect(page);
        let below = f32::from(local.y) > f32::from(r.origin.y + r.size.height);
        if below && page + 1 < self.column.len() {
            let next = self.page_rect(page + 1);
            let gap_mid = (f32::from(r.origin.y + r.size.height) + f32::from(next.origin.y)) / 2.0;
            if f32::from(local.y) > gap_mid {
                page += 1;
            }
        }
        let r = self.page_rect(page);
        let inside = r.contains(&local);
        if !inside && !near {
            return None;
        }
        let k = self.zoom_value() * PX_PER_PT;
        let tx = f32::from(local.x - r.origin.x).clamp(0.0, f32::from(r.size.width)) / k;
        let ty = f32::from(local.y - r.origin.y).clamp(0.0, f32::from(r.size.height)) / k;
        let (w, h) = self.page_points(page);
        let (x, y) = self.page_turn(page).unmap(w, h, tx, ty);
        Some((page, x, y))
    }

    fn link_at(&self, pos: Point<Pixels>) -> Option<&Link> {
        let (page, x, y) = self.hit(pos, false)?;
        self.links
            .get(&page)?
            .iter()
            .find(|l| x >= l.rect[0] && x <= l.rect[2] && y >= l.rect[1] && y <= l.rect[3])
    }

    // ── moving around ─────────────────────────────────────────────────────

    /// Change the zoom keeping the point under `anchor` (window position;
    /// default the top middle of the view) where it is.
    pub fn set_zoom(&mut self, zoom: Zoom, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let vp = self.viewport.get();
        let a = anchor.map_or(point(vp.size.width / 2.0, px(0.)), |p| p - vp.origin);
        let (ax, ay) = (f32::from(a.x), f32::from(a.y));
        self.sync_column();
        let page = self.column.page_at(self.scroll.y + ay);
        let before = self.page_rect(page);
        let rel_x = (ax - f32::from(before.origin.x)) / f32::from(before.size.width).max(1.0);
        let rel_y = (ay - f32::from(before.origin.y)) / f32::from(before.size.height).max(1.0);
        self.zoom = match zoom {
            Zoom::Percent(z) => Zoom::Percent(z.clamp(layout::ZOOM_MIN, layout::ZOOM_MAX)),
            z => z,
        };
        self.sync_column();
        let (vw, _) = self.view_size();
        let (w, h) = self.column.sizes.get(page).copied().unwrap_or_default();
        let left = self.column.left(page, vw);
        let top = self.column.tops.get(page).copied().unwrap_or_default();
        self.scroll.x = left + rel_x * w - ax;
        self.scroll.y = top + rel_y * h - ay;
        self.clamp_scroll();
        cx.notify();
    }

    pub fn zoom_step(&mut self, up: bool, anchor: Option<Point<Pixels>>, cx: &mut Context<Self>) {
        let z = layout::step(self.zoom_value(), up);
        self.set_zoom(Zoom::Percent(z), anchor, cx);
    }

    pub fn rotate(&mut self, cw: bool, cx: &mut Context<Self>) {
        let page = self.current_page();
        self.turn = if cw { self.turn.cw() } else { self.turn.ccw() };
        self.sync_column();
        self.scroll.y = self.column.tops.get(page).copied().unwrap_or_default() - layout::MARGIN;
        self.clamp_scroll();
        cx.notify();
    }

    /// Show `dest`: its page at the top of the view, or its point when the
    /// destination names one.
    pub fn go_to(&mut self, dest: &Dest, cx: &mut Context<Self>) {
        self.sync_column();
        let Some(top) = self.column.tops.get(dest.page).copied() else {
            return;
        };
        let (w, h) = self.page_points(dest.page);
        let k = self.zoom_value() * PX_PER_PT;
        let (vw, _) = self.view_size();
        match dest.y {
            Some(y) => {
                let (tx, ty) = self
                    .page_turn(dest.page)
                    .map(w, h, dest.x.unwrap_or(0.0), y);
                self.scroll.y = top + ty * k;
                if dest.x.is_some() {
                    self.scroll.x = self.column.left(dest.page, vw) + tx * k - layout::MARGIN;
                }
            }
            None => self.scroll.y = top - layout::MARGIN,
        }
        self.clamp_scroll();
        cx.notify();
    }

    pub fn go_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        self.go_to(
            &Dest {
                page,
                x: None,
                y: None,
            },
            cx,
        );
    }

    /// Bring a page-space rectangle into view (a search hit).
    fn reveal(&mut self, page: usize, r: [f32; 4], cx: &mut Context<Self>) {
        self.sync_column();
        let (vw, vh) = self.view_size();
        let b = self.screen_rect(page, r);
        let (y0, y1) = (f32::from(b.origin.y), f32::from(b.origin.y + b.size.height));
        if y0 < 0.0 || y1 > vh {
            self.scroll.y += y0 - vh / 3.0;
        }
        let (x0, x1) = (f32::from(b.origin.x), f32::from(b.origin.x + b.size.width));
        if x0 < 0.0 || x1 > vw {
            self.scroll.x += x0 - vw / 3.0;
        }
        self.clamp_scroll();
        cx.notify();
    }

    fn scroll_by(&mut self, dx: f32, dy: f32, cx: &mut Context<Self>) {
        self.scroll.x += dx;
        self.scroll.y += dy;
        self.clamp_scroll();
        cx.notify();
    }

    fn page_step(&mut self, down: bool, cx: &mut Context<Self>) {
        let (_, vh) = self.view_size();
        let d = (vh - LINE).max(LINE);
        self.scroll_by(0.0, if down { d } else { -d }, cx);
    }

    /// Left / Right: scroll sideways when the page is wider than the view,
    /// else the previous / next page.
    fn sideways(&mut self, right: bool, cx: &mut Context<Self>) {
        let (vw, _) = self.view_size();
        if self.column.width > vw + 1.0 {
            self.scroll_by(if right { LINE } else { -LINE }, 0.0, cx);
            return;
        }
        let p = self.current_page();
        let to = if right {
            (p + 1).min(self.page_count().saturating_sub(1))
        } else {
            p.saturating_sub(1)
        };
        self.go_to_page(to, cx);
    }

    pub fn toggle_panel(&mut self, panel: Panel, cx: &mut Context<Self>) {
        self.panel = if self.panel == Some(panel) {
            None
        } else {
            Some(panel)
        };
        cx.notify();
    }

    /// Put the caret in the page number box (Go to Page).
    pub fn focus_page_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.page_box.update(cx, |i, cx| {
            i.focus(window, cx);
            i.select_all(window, cx);
        });
    }

    fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.panel = match self.panel {
            Some(_) => None,
            None => Some(Panel::Thumbnails),
        };
        cx.notify();
    }

    // ── text selection ────────────────────────────────────────────────────

    fn text_pos(&self, pos: Point<Pixels>, near: bool) -> Option<TextPos> {
        let (page, x, y) = self.hit(pos, near)?;
        let text = self.texts.get(&page)?;
        let offset = text.offset_at(x, y)?;
        Some(TextPos { page, offset })
    }

    fn ordered_sel(&self) -> Option<(TextPos, TextPos)> {
        let (a, b) = self.sel?;
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        (a != b).then_some((a, b))
    }

    /// The selected byte range on `page`.
    fn sel_on(&self, page: usize) -> Option<Range<usize>> {
        let (a, b) = self.ordered_sel()?;
        if page < a.page || page > b.page {
            return None;
        }
        let len = self.texts.get(&page).map_or(0, |t| t.text.len());
        let start = if page == a.page { a.offset } else { 0 };
        let end = if page == b.page { b.offset } else { len };
        (start < end).then_some(start..end)
    }

    /// The selected text, pages joined by line breaks; `None` while a
    /// page in it hasn't been read yet.
    fn selected_text(&self) -> Option<String> {
        let (a, b) = self.ordered_sel()?;
        let mut out = String::new();
        for page in a.page..=b.page {
            let t = self.texts.get(&page)?;
            if let Some(r) = self.sel_on(page) {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(&t.text[r]);
            }
        }
        Some(out)
    }

    fn copy_text(&mut self, cx: &mut Context<Self>) {
        if self.ordered_sel().is_none() {
            return;
        }
        match self.selected_text() {
            Some(text) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                self.status = i18n::t("Copied").into();
            }
            None => {
                let (a, b) = self.ordered_sel().unwrap_or_default();
                self.ask_text(a.page..b.page + 1);
                self.status =
                    i18n::t("Still reading the text of those pages; copy again in a moment").into();
            }
        }
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        let n = self.page_count();
        if n == 0 {
            return;
        }
        self.ask_text(0..n);
        let end = self
            .texts
            .get(&(n - 1))
            .map_or(usize::MAX / 2, |t| t.text.len());
        self.sel = Some((
            TextPos { page: 0, offset: 0 },
            TextPos {
                page: n - 1,
                offset: end,
            },
        ));
        cx.notify();
    }

    fn ask_text(&mut self, pages: Range<usize>) {
        self.ask_text_in_order(pages);
    }

    // ── mouse ─────────────────────────────────────────────────────────────

    fn on_mouse_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        // Page commands act on the current page again.
        self.page_sel.clear();
        if let Some(link) = self.link_at(ev.position) {
            self.press = Some(Press::Link(link.target.clone(), ev.position));
            return;
        }
        let Some(pos) = self.text_pos(ev.position, false) else {
            self.sel = None;
            self.press = None;
            cx.notify();
            return;
        };
        let text = self.texts.get(&pos.page).cloned();
        match (ev.click_count, text) {
            (2, Some(t)) => {
                let r = t.word_at(pos.offset);
                self.sel = Some((
                    TextPos {
                        page: pos.page,
                        offset: r.start,
                    },
                    TextPos {
                        page: pos.page,
                        offset: r.end,
                    },
                ));
            }
            (n, Some(t)) if n >= 3 => {
                let r = t.line_at(pos.offset);
                self.sel = Some((
                    TextPos {
                        page: pos.page,
                        offset: r.start,
                    },
                    TextPos {
                        page: pos.page,
                        offset: r.end,
                    },
                ));
            }
            _ if ev.modifiers.shift && self.sel.is_some() => {
                if let Some(s) = &mut self.sel {
                    s.1 = pos;
                }
            }
            _ => self.sel = Some((pos, pos)),
        }
        self.press = Some(Press::Select);
        cx.notify();
    }

    fn on_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        match &self.press {
            Some(Press::Select) if ev.pressed_button == Some(MouseButton::Left) => {
                // Past the top or bottom edge: scroll along.
                let vp = self.viewport.get();
                let y = f32::from(ev.position.y - vp.origin.y);
                let h = f32::from(vp.size.height);
                if y < 0.0 {
                    self.scroll_by(0.0, y.max(-LINE), cx);
                } else if y > h {
                    self.scroll_by(0.0, (y - h).min(LINE), cx);
                }
                if let Some(pos) = self.text_pos(ev.position, true)
                    && let Some(s) = &mut self.sel
                    && s.1 != pos
                {
                    s.1 = pos;
                    cx.notify();
                }
                return;
            }
            Some(Press::Link(_, at)) if (ev.position - *at).magnitude() > 4.0 => {
                self.press = None;
            }
            _ => {}
        }
        let hover = if self.link_at(ev.position).is_some() {
            Hover::Link
        } else if self
            .hit(ev.position, false)
            .and_then(|(p, x, y)| Some(self.texts.get(&p)?.is_on_text(x, y)))
            .unwrap_or(false)
        {
            Hover::Text
        } else {
            Hover::None
        };
        if hover != self.hover {
            self.hover = hover;
            cx.notify();
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(Press::Link(target, _)) = self.press.take() {
            self.follow(target, window, cx);
        }
    }

    fn follow(&mut self, target: LinkTarget, window: &mut Window, cx: &mut Context<Self>) {
        match target {
            LinkTarget::Dest(d) => self.go_to(&d, cx),
            LinkTarget::Uri(uri) => confirm_open(uri, window, cx),
        }
    }

    fn on_scroll(&mut self, ev: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let delta = ev.delta.pixel_delta(window.line_height());
        if ev.modifiers.control || ev.modifiers.platform {
            let dy = f32::from(delta.y);
            if dy != 0.0 {
                let z = self.zoom_value() * if dy > 0.0 { 1.1 } else { 1.0 / 1.1 };
                self.set_zoom(Zoom::Percent(z), Some(ev.position), cx);
            }
            return;
        }
        let (mut dx, mut dy) = (f32::from(delta.x), f32::from(delta.y));
        if ev.modifiers.shift && dx == 0.0 {
            dx = dy;
            dy = 0.0;
        }
        self.scroll_by(-dx, -dy, cx);
    }

    // ── rendering ─────────────────────────────────────────────────────────

    /// Ask for what the view needs: page images at this zoom (on screen,
    /// then the next and previous page) and the text of pages on screen.
    fn request(&mut self, device: f32) {
        let Some(worker) = self.worker.clone() else {
            return;
        };
        let vis = self.visible_pages();
        if vis.is_empty() {
            return;
        }
        let mut jobs = Vec::new();
        let around = vis.clone().chain([vis.end, vis.start.wrapping_sub(1)]);
        for page in around {
            let Some(p) = self.page_ref(page) else {
                continue;
            };
            let turn = self.page_turn(page);
            let wanted = PageJob {
                key: p.key(),
                scale: device,
                turn,
            };
            match self.pages.get(p.key(), turn, device) {
                Some((_, true)) => {}
                _ => {
                    if !jobs.contains(&wanted) {
                        jobs.push(wanted);
                    }
                }
            }
        }
        worker.want_pages(jobs);
        for page in vis.clone() {
            let Some(p) = self.page_ref(page) else {
                continue;
            };
            if self.asked_text.insert(p.key()) {
                worker.text_first(p.key());
            }
        }
    }

    fn render_pages(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens(cx);
        self.sync_column();
        self.clamp_scroll();
        let zoom = self.zoom_value();
        let device = zoom * PX_PER_PT * window.scale_factor();
        self.request(device);
        for img in self.pages.take_evicted() {
            window.drop_image(img).ok();
        }
        for img in self.thumbs.borrow_mut().take_evicted() {
            window.drop_image(img).ok();
        }
        let current_hit = self.find.current_hit();
        let mut items = Vec::new();
        for page in self.visible_pages() {
            let rect = self.page_rect(page);
            let turn = self.page_turn(page);
            let image = self
                .page_ref(page)
                .and_then(|p| self.pages.get(p.key(), turn, device))
                .map(|(i, _)| i);
            let mut marks = Vec::new();
            for (hit, current) in self.find.hits_on(page, current_hit) {
                if let Some(text) = self.texts.get(&page) {
                    let color = if current {
                        t.find_match_current
                    } else {
                        t.find_match
                    };
                    for r in text.rects(hit) {
                        marks.push((self.screen_rect(page, r), color));
                    }
                }
            }
            if let (Some(r), Some(text)) = (self.sel_on(page), self.texts.get(&page)) {
                for r in text.rects(r) {
                    marks.push((self.screen_rect(page, r), t.paper_selection));
                }
            }
            items.push((rect, image, marks));
        }
        let viewport = self.viewport.clone();
        let entity = cx.entity().downgrade();
        let shadow = t.shadow(Elevation::Raised);
        let (desk, paper, border) = (t.desk, t.paper, t.border_strong);
        let pages = canvas(
            move |bounds, _, cx| {
                let old = viewport.replace(bounds);
                // A new size (first paint, sidebar, window): lay out again
                // next frame, so the fit zooms use the real size.
                if old.size != bounds.size {
                    cx.defer(move |cx| {
                        let _ = entity.update(cx, |_, cx| cx.notify());
                    });
                }
            },
            move |bounds, _, window, _| {
                window.paint_quad(fill(bounds, desk));
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    for (r, image, marks) in &items {
                        let r = Bounds::new(r.origin + bounds.origin, r.size);
                        window.paint_drop_shadows(r, Corners::default(), &shadow);
                        window.paint_quad(fill(r, paper));
                        if let Some(img) = image {
                            let _ =
                                window.paint_image(r, r, Corners::default(), img.clone(), 0, false);
                        }
                        window.paint_quad(outline(r, border.opacity(0.4), BorderStyle::Solid));
                        for (m, color) in marks {
                            let m = Bounds::new(m.origin + bounds.origin, m.size);
                            window.paint_quad(fill(m, *color));
                        }
                    }
                });
            },
        )
        .size_full();
        let cursor = match (&self.press, self.hover) {
            (Some(Press::Select), _) => CursorStyle::IBeam,
            (_, Hover::Link) => CursorStyle::PointingHand,
            (_, Hover::Text) => CursorStyle::IBeam,
            _ => CursorStyle::Arrow,
        };
        div()
            .id("pdf-pages")
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            .overflow_hidden()
            .cursor(cursor)
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.press = None),
            )
            .on_scroll(cx.listener(Self::on_scroll))
            .child(pages)
            .children(self.render_scrollbar(cx))
            .children(self.find.bar(cx))
            .into_any_element()
    }

    /// A thin scroll indicator on the right (the wheel and keys scroll).
    fn render_scrollbar(&self, cx: &App) -> Option<Div> {
        let (_, vh) = self.view_size();
        let total = self.column.height;
        if total <= vh || vh <= 0.0 {
            return None;
        }
        let t = tokens(cx);
        let h = (vh * vh / total).max(24.0);
        let y = (self.scroll.y / (total - vh)) * (vh - h);
        Some(
            div()
                .absolute()
                .right(px(3.))
                .top(px(y))
                .w(px(6.))
                .h(px(h))
                .rounded(px(3.))
                .bg(t.fg_dim.opacity(0.45)),
        )
    }

    fn render_message(&self, cx: &mut Context<Self>) -> AnyElement {
        let t = tokens(cx);
        let body = match &self.state {
            State::Loading => v_flex()
                .items_center()
                .gap(SPACE_4)
                .child(Spinner::new().large().color(t.fg_muted))
                .child(
                    div()
                        .text_size(FONT_EMPHASIS)
                        .text_color(t.fg)
                        .child(tf("Opening {}…", &[&self.name])),
                ),
            State::Password { wrong } => {
                let view = cx.entity().downgrade();
                v_flex()
                    .w(px(360.))
                    .gap(SPACE_5)
                    .p(SPACE_8)
                    .rounded(RADIUS_LG)
                    .bg(t.elevated)
                    .border_1()
                    .border_color(t.border)
                    .shadow(t.shadow(Elevation::Popover))
                    .child(
                        h_flex()
                            .gap(SPACE_4)
                            .child(gpui_kit::component::Icon::new(Lucide::Lock).with_size(ICON_LG))
                            .child(
                                div()
                                    .text_size(FONT_TITLE)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(i18n::t("Password")),
                            ),
                    )
                    .child(div().text_color(t.fg_muted).child(tf(
                        "“{}” is protected. Enter the password to open it.",
                        &[&self.name],
                    )))
                    .child(Input::new(&self.password).cleanable(false))
                    .when(*wrong, |d| {
                        d.child(
                            div()
                                .text_size(FONT_SMALL)
                                .text_color(t.danger)
                                .child(i18n::t("The password is incorrect. Try again.")),
                        )
                    })
                    .child(
                        h_flex().justify_end().child(
                            Button::new("pdf-password-ok")
                                .primary()
                                .small()
                                .label(i18n::t("Open"))
                                .on_click(move |_, window, cx| {
                                    let _ = view.update(cx, |v, cx| v.submit_password(window, cx));
                                }),
                        ),
                    )
            }
            State::Failed(e) => {
                let detail = match e {
                    OpenError::UnsupportedEncryption => i18n::t(
                        "The file is encrypted in a way Kantor can't read. Open it in another PDF viewer.",
                    ),
                    _ => i18n::t(
                        "The file is damaged and could not be repaired. If it came as an attachment or a download, get it again.",
                    ),
                };
                v_flex()
                    .w(px(420.))
                    .gap(SPACE_5)
                    .p(SPACE_8)
                    .rounded(RADIUS_LG)
                    .bg(t.elevated)
                    .border_1()
                    .border_color(t.border)
                    .shadow(t.shadow(Elevation::Popover))
                    .child(
                        h_flex()
                            .gap(SPACE_4)
                            .child(
                                gpui_kit::component::Icon::new(Lucide::TriangleAlert)
                                    .with_size(ICON_LG)
                                    .text_color(t.danger),
                            )
                            .child(
                                div()
                                    .text_size(FONT_TITLE)
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(tf("Kantor can't open “{}”", &[&self.name])),
                            ),
                    )
                    .child(div().text_color(t.fg_muted).child(detail))
            }
            State::Ready(_) => v_flex(),
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .size_full()
            .items_center()
            .justify_center()
            .bg(t.desk)
            .child(body)
            .into_any_element()
    }
}

/// The inspector's facts about the open PDF.
pub struct PdfInfo {
    pub name: String,
    pub path: PathBuf,
    pub title: Option<String>,
    pub pages: usize,
    pub current: usize,
    /// File size, e.g. `2.4 MB`.
    pub size: String,
    /// Changed since it was saved.
    pub dirty: bool,
}

pub(crate) fn size_text(n: u64) -> String {
    match n {
        0 => String::new(),
        n if n >= 1 << 20 => format!("{:.1} MB", n as f64 / (1 << 20) as f64),
        n if n >= 1 << 10 => format!("{} KB", n >> 10),
        n => format!("{n} B"),
    }
}

/// Bookmarks the file asks to show expanded.
fn initially_open(items: &[OutlineItem]) -> HashSet<Vec<usize>> {
    fn walk(items: &[OutlineItem], path: &mut Vec<usize>, out: &mut HashSet<Vec<usize>>) {
        for (i, it) in items.iter().enumerate() {
            path.push(i);
            if it.open && !it.children.is_empty() {
                out.insert(path.clone());
                walk(&it.children, path, out);
            }
            path.pop();
        }
    }
    let mut out = HashSet::new();
    walk(items, &mut Vec::new(), &mut out);
    out
}

/// Ask before opening an address from the document (as Acrobat does for
/// every link that leaves the document).
fn confirm_open(url: String, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    let title: SharedString = i18n::t("Open this link?").into();
    let detail: SharedString = tf(
        "The document is trying to open {}. Open only links from sources you trust.",
        &[&url],
    )
    .into();
    window.open_dialog(cx, move |dialog, _, _| {
        let url = url.clone();
        dialog
            .w(px(460.))
            .title(title.clone())
            .child(v_flex().child(DialogDescription::new().child(detail.clone())))
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("pdf-link-cancel")
                            .label(i18n::t("Cancel"))
                            .ghost()
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("pdf-link-open")
                            .label(i18n::t("Open"))
                            .primary()
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                cx.open_url(&url);
                            }),
                    ),
            )
    });
}

impl Render for PdfView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = tokens(cx);
        let ready = matches!(self.state, State::Ready(_));
        if !cx.has_active_drag() && self.drop_at.is_some() {
            self.drop_at = None;
        }
        let body = if ready {
            let pages = if self.organize {
                grid::render(self, window, cx)
            } else {
                self.render_pages(window, cx)
            };
            h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .children({
                    // The grid shows the pages itself: no thumbnails beside it.
                    let panel = self
                        .panel
                        .filter(|p| !(self.organize && *p == Panel::Thumbnails));
                    panel.map(|p| side::render(self, p, window, cx))
                })
                .child(pages)
                .into_any_element()
        } else {
            self.render_message(cx)
        };
        if ready {
            self.sync_page_box(window, cx);
        }
        if std::mem::take(&mut self.focus_password) && self.focus.contains_focused(window, cx) {
            self.password.update(cx, |i, cx| i.focus(window, cx));
        }
        macro_rules! on {
            ($root:expr, $($action:ident => $run:expr),* $(,)?) => {
                $root$(.on_action(cx.listener(|v, _: &$action, w, cx| {
                    let run: fn(&mut PdfView, &mut Window, &mut Context<PdfView>) = $run;
                    run(v, w, cx)
                })))*
            };
        }
        let root = v_flex()
            .id("pdf-view")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(t.bg);
        let root = on!(root,
            ScrollUp => |v, _, cx| v.scroll_by(0.0, -LINE, cx),
            ScrollDown => |v, _, cx| v.scroll_by(0.0, LINE, cx),
            ScrollLeft => |v, _, cx| v.sideways(false, cx),
            ScrollRight => |v, _, cx| v.sideways(true, cx),
            PageUp => |v, _, cx| v.page_step(false, cx),
            PageDown => |v, _, cx| v.page_step(true, cx),
            FirstPage => |v, _, cx| v.go_to_page(0, cx),
            LastPage => |v, _, cx| {
                let n = v.page_count();
                v.go_to_page(n.saturating_sub(1), cx)
            },
            ZoomIn => |v, _, cx| v.zoom_step(true, None, cx),
            ZoomOut => |v, _, cx| v.zoom_step(false, None, cx),
            ActualSize => |v, _, cx| v.set_zoom(Zoom::Percent(1.0), None, cx),
            FitPage => |v, _, cx| v.set_zoom(Zoom::FitPage, None, cx),
            FitWidth => |v, _, cx| v.set_zoom(Zoom::FitWidth, None, cx),
            RotateCw => |v, _, cx| v.rotate(true, cx),
            RotateCcw => |v, _, cx| v.rotate(false, cx),
            ToggleSidebar => |v, _, cx| v.toggle_sidebar(cx),
            SelectAll => |v, _, cx| v.select_all(cx),
            Copy => |v, w, cx| v.copy_doc(w, cx),
            Cut => |v, w, cx| v.cut_doc(w, cx),
            Paste => |v, w, cx| v.paste_doc(w, cx),
            Save => |v, w, cx| v.save_doc(w, cx),
            SaveAs => |v, w, cx| v.save_doc_as(w, cx),
            Undo => |v, w, cx| v.undo_doc(w, cx),
            Redo => |v, w, cx| v.redo_doc(w, cx),
            ToggleOrganize => |v, w, cx| v.toggle_organize(w, cx),
            DeletePages => |v, _, cx| v.delete_pages(cx),
            SelectAllPages => |v, _, cx| v.select_all_pages(cx),
            PrevPageSel => |v, _, cx| v.step_page_sel(-1, false, cx),
            NextPageSel => |v, _, cx| v.step_page_sel(1, false, cx),
            PrevPageSelExtend => |v, _, cx| v.step_page_sel(-1, true, cx),
            NextPageSelExtend => |v, _, cx| v.step_page_sel(1, true, cx),
            RowUpSel => |v, _, cx| {
                let step = if v.organize { v.grid_columns() as isize } else { 5 };
                v.step_page_sel(-step, false, cx)
            },
            RowDownSel => |v, _, cx| {
                let step = if v.organize { v.grid_columns() as isize } else { 5 };
                v.step_page_sel(step, false, cx)
            },
            RotatePagesLeft => |v, _, cx| v.rotate_pages(false, cx),
            RotatePagesRight => |v, _, cx| v.rotate_pages(true, cx),
            LeavePages => |v, w, cx| {
                if v.organize {
                    v.toggle_organize(w, cx);
                } else {
                    v.page_sel.clear();
                    v.focus(w, cx);
                }
            },
            Escape => |v, _, cx| {
                v.sel = None;
                cx.notify();
            },
            OpenFind => |v, w, cx| v.open_find(false, w, cx),
            OpenReplace => |v, w, cx| v.open_find(false, w, cx),
            FindNext => |v, w, cx| v.find_next_doc(true, w, cx),
            FindPrev => |v, w, cx| v.find_next_doc(false, w, cx),
        );
        let dialog = self.render_dialog(cx);
        root.relative()
            .when(ready, |d| d.child(toolbar::render(self, cx)))
            .child(body)
            .children(dialog)
    }
}

impl PdfView {
    /// The page box shows the current page unless the user is typing in it.
    fn sync_page_box(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.page_box.read(cx).focus_handle(cx).is_focused(window) {
            return;
        }
        let want = (self.current_page() + 1).to_string();
        if self.page_box.read(cx).value().as_ref() != want {
            self.page_box
                .update(cx, |i, cx| i.set_value(want, window, cx));
        }
    }
}
