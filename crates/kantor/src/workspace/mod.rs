//! The suite shell, laid out like Voltrus Tools:
//!
//! ```text
//! ┌ sidebar ┬ tab strip (38): [K] [budget.xlsx +1 ×] [+]   [⎸⎸] [▭] [ⓘ] [Ctrl K] ┐
//! │ (248)   │ split panes (each a document, cached)          │ inspector (300) │
//! └ status bar (26): ● Sheets · B7 · Saved ──────────────── vantablack · v0.0.1 ┘
//! ```
//!
//! A window holds tabs; a tab holds a split tree of panes; a pane shows one
//! document. Home shows when no tab is active. Chrome regions are cached views.
//! Open tabs, layouts and panel sizes persist across runs (`kantor_core::session`).

mod agent;
mod ai;
mod chrome;
mod closing;
mod doc;
mod home;
mod naming;
mod palette;
mod panels;
mod recovery;
mod splits;
mod update;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::layout::{Axis, Layout, PaneId, Path as LayoutPath};
use kantor_core::session::{PaneState, Session, TabState, UiState};
use kantor_ui::metrics::*;
use kantor_ui::{CachedView, tokens};
use sheets_ui::SheetsView;

use doc::DocView;

use crate::actions::*;
use closing::Closing;
use palette::Palette;

const LOGO_SVG: &[u8] = include_bytes!("../../../../assets/icons/kantor.svg");

/// Which app a file or flag asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppKind {
    Docs,
    Sheets,
    Slides,
}

impl AppKind {
    /// Guess the app from a file extension.
    pub fn for_path(path: &std::path::Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        match ext.as_str() {
            "xlsx" | "xlsm" | "xls" | "xlsb" | "ods" | "csv" | "tsv" => Some(Self::Sheets),
            // Docs reads .docx (.odt isn't supported yet).
            "docx" => Some(Self::Docs),
            // Read-only imports: Save writes a .docx copy.
            "rtf" | "doc" | "dot" => Some(Self::Docs),
            // Slides reads .pptx only (.ppt / .odp aren't supported).
            "pptx" => Some(Self::Slides),
            _ => None,
        }
    }

    /// The app's colour (Docs blue, Sheets green, Slides orange).
    fn color(self, t: &kantor_ui::Tokens) -> Hsla {
        match self {
            Self::Docs => t.docs,
            Self::Sheets => t.sheets,
            Self::Slides => t.slides,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Docs => "Docs",
            Self::Sheets => "Sheets",
            Self::Slides => "Slides",
        }
    }
}

/// An open document; shown by exactly one pane.
struct Doc {
    view: DocView,
    _changes: Vec<Subscription>,
}

struct Tab {
    id: u64,
    layout: Layout,
    focused: PaneId,
}

/// An in-progress mouse drag.
enum Drag {
    Sidebar { origin_x: Pixels, origin_w: f32 },
    Inspector { origin_x: Pixels, origin_w: f32 },
    Split { path: LayoutPath, axis: Axis },
}

struct Chrome {
    tabs: Entity<CachedView<Workspace>>,
    status: Entity<CachedView<Workspace>>,
    sidebar: Entity<CachedView<Workspace>>,
    inspector: Entity<CachedView<Workspace>>,
    /// The command palette, redrawn on its own while typing (see
    /// `Workspace::notify_palette`) so a letter doesn't redraw the chrome.
    palette: Entity<CachedView<Workspace>>,
}

pub struct Workspace {
    docs: HashMap<PaneId, Doc>,
    tabs: Vec<Tab>,
    /// Index into `tabs`; `None` shows Home.
    active: Option<usize>,
    next_id: u64,
    palette: Option<Palette>,
    /// An open AI panel (settings, formula assistant, rewrite).
    ai: Option<ai::AiOverlay>,
    message: SharedString,
    /// A short-lived status-bar message shown over any document's (agent
    /// edits, "copied"); see `Workspace::notice`.
    notice: Option<SharedString>,
    notice_id: u64,
    /// Counts "Saving…" waits before a close; Cancel moves it on so the
    /// running wait doesn't close (see `closing`).
    save_wait: u64,
    logo: Arc<Image>,
    focus: FocusHandle,
    chrome: Option<Chrome>,
    ui: UiState,
    recent: Vec<PathBuf>,
    drag: Option<Drag>,
    /// Bounds of each split container in the active tab, recorded at paint time
    /// so divider drags can turn pixels into ratios.
    split_bounds: Rc<RefCell<HashMap<LayoutPath, Bounds<Pixels>>>>,
    /// In-app updates (see `update`).
    updates: update::Updates,
    /// Recovery copies and autosave (see `recovery`).
    recovery: recovery::Recovery,
}

impl Workspace {
    pub fn new(
        app: Option<AppKind>,
        file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let session = Session::load();
        let mut this = Self {
            docs: HashMap::new(),
            tabs: Vec::new(),
            active: None,
            next_id: 1,
            palette: None,
            ai: None,
            message: SharedString::default(),
            notice: None,
            notice_id: 0,
            save_wait: 0,
            logo: Arc::new(Image::from_bytes(ImageFormat::Svg, LOGO_SVG.to_vec())),
            focus: cx.focus_handle(),
            chrome: None,
            ui: session.ui.clone(),
            recent: session.recent.clone(),
            drag: None,
            split_bounds: Rc::default(),
            updates: update::Updates::default(),
            recovery: recovery::Recovery::default(),
        };
        let ws = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            ws.update(cx, |ws, cx| ws.should_close_window(window, cx))
                .unwrap_or(true)
        });
        this.chrome = Some(Chrome {
            tabs: CachedView::new(cx, |ws, window, cx| {
                ws.render_tab_strip(window, cx).into_any_element()
            }),
            status: CachedView::new(cx, |ws, _, cx| ws.render_status_bar(cx).into_any_element()),
            sidebar: CachedView::new(cx, |ws, _, cx| ws.render_sidebar(cx).into_any_element()),
            inspector: CachedView::new(cx, |ws, _, cx| ws.render_inspector(cx).into_any_element()),
            palette: CachedView::manual(cx, |ws, _, cx| {
                ws.render_palette(cx)
                    .map_or_else(|| div().into_any_element(), IntoElement::into_any_element)
            }),
        });

        let app = app.or_else(|| file.as_deref().and_then(AppKind::for_path));
        if app.is_none() {
            this.restore_tabs(&session, window, cx);
        }
        match app {
            Some(app) => this.launch(app, file, window, cx),
            None => {
                let active = session.active.filter(|&i| i < this.tabs.len());
                this.activate(active, window, cx);
            }
        }
        if std::env::var_os("KANTOR_BENCH_PALETTE").is_some() {
            this.bench_palette(window, cx);
        }
        this.init_updates(cx);
        this.init_recovery(window, cx);
        this
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    // ── documents, tabs, panes ─────────────────────────────────────────────

    /// Create a document view of app `kind` (empty, or loading `file`) and
    /// return its pane id; `None` for apps that don't exist yet.
    fn new_doc(
        &mut self,
        kind: AppKind,
        file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<PaneId> {
        let view = DocView::new(kind, file, window, cx)?;
        Some(self.insert_doc(view, window, cx))
    }

    /// Add an open document; returns its pane id.
    fn insert_doc(&mut self, view: DocView, window: &mut Window, cx: &mut Context<Self>) -> PaneId {
        let kind = view.kind();
        let id = self.alloc_id();
        if let Some(path) = view.path(cx) {
            Session::touch_recent(&mut self.recent, &path);
        } else if !view.is_dirty(cx) {
            // (A restored copy is named after the document it came from.)
            let open: Vec<String> = self
                .docs
                .values()
                .filter(|d| d.view.kind() == kind && d.view.path(cx).is_none())
                .map(|d| d.view.doc_name(cx))
                .collect();
            let name = naming::next_untitled_name(kind, open.iter().map(String::as_str));
            view.set_untitled_name(name, cx);
        }
        let changes = match &view {
            DocView::Sheets(e) => vec![
                cx.observe(e, |_, _, cx| cx.notify()),
                // Opening cancelled: the pane goes away.
                cx.subscribe_in(e, window, move |ws, _, event, window, cx| match event {
                    sheets_ui::SheetsEvent::Close => ws.remove_doc(id, window, cx),
                    sheets_ui::SheetsEvent::Notice(text) => ws.notice(text.clone(), cx),
                }),
            ],
            DocView::Docs(e) => vec![cx.observe(e, |_, _, cx| cx.notify())],
            DocView::Slides(e) => vec![cx.observe(e, |_, _, cx| cx.notify())],
        };
        self.docs.insert(
            id,
            Doc {
                view,
                _changes: changes,
            },
        );
        id
    }

    /// Find the tab and pane showing `path`.
    fn find_path(&self, path: &std::path::Path, cx: &App) -> Option<(usize, PaneId)> {
        self.tabs.iter().enumerate().find_map(|(ti, tab)| {
            tab.layout.panes().into_iter().find_map(|pane| {
                let doc = self.docs.get(&pane)?;
                (doc.view.path(cx).as_deref() == Some(path)).then_some((ti, pane))
            })
        })
    }

    fn launch(
        &mut self,
        app: AppKind,
        file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(path) = &file
            && let Some((ti, pane)) = self.find_path(path, cx)
        {
            self.tabs[ti].focused = pane;
            self.activate(Some(ti), window, cx);
            return;
        }
        let Some(pane) = self.new_doc(app, file, window, cx) else {
            self.not_yet(app, cx);
            return;
        };
        let id = self.alloc_id();
        self.tabs.push(Tab {
            id,
            layout: Layout::Leaf(pane),
            focused: pane,
        });
        self.activate(Some(self.tabs.len() - 1), window, cx);
    }

    fn not_yet(&mut self, app: AppKind, cx: &mut Context<Self>) {
        self.message = format!("{} is not built yet — it's on the roadmap.", app.name()).into();
        cx.notify();
    }

    fn focused_view(&self) -> Option<DocView> {
        let tab = self.tabs.get(self.active?)?;
        Some(self.docs.get(&tab.focused)?.view.clone())
    }

    /// Show tab `ix` (or Home) and focus its focused pane.
    fn activate(&mut self, ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.active = ix.filter(|&i| i < self.tabs.len());
        match self.focused_view() {
            Some(view) => {
                let title = view.title(cx);
                window.set_window_title(&title);
                view.focus(window, cx);
            }
            None => {
                window.set_window_title("Kantor");
                window.focus(&self.focus, cx);
            }
        }
        self.persist(cx);
        cx.notify();
    }

    /// Make `pane` the focused pane of the active tab (clicked into).
    fn focus_pane(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.active.and_then(|i| self.tabs.get_mut(i)) else {
            return;
        };
        if tab.focused == pane || !tab.layout.contains(pane) {
            return;
        }
        tab.focused = pane;
        if let Some(view) = self.focused_view() {
            let title = view.title(cx);
            window.set_window_title(&title);
        }
        cx.notify();
    }

    /// Run `f` on the focused document, whatever its app.
    fn with_active(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&DocView, &mut Window, &mut App),
    ) {
        if let Some(view) = self.focused_view() {
            f(&view, window, cx);
        }
    }

    /// Run `f` if the focused document is a spreadsheet.
    fn with_sheets(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut SheetsView, &mut Window, &mut Context<SheetsView>),
    ) {
        if let Some(DocView::Sheets(view)) = self.focused_view() {
            view.update(cx, |v, cx| f(v, window, cx));
        }
    }

    /// Run `f` if the focused document is a Docs document.
    fn with_docs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut docs_ui::DocsView, &mut Window, &mut Context<docs_ui::DocsView>),
    ) {
        if let Some(DocView::Docs(view)) = self.focused_view() {
            view.update(cx, |v, cx| f(v, window, cx));
        }
    }

    /// Run `f` if the focused document is a presentation.
    fn with_slides(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut slides_ui::SlidesView, &mut Window, &mut Context<slides_ui::SlidesView>),
    ) {
        if let Some(DocView::Slides(view)) = self.focused_view() {
            view.update(cx, |v, cx| f(v, window, cx));
        }
    }

    fn new_spreadsheet(&mut self, _: &NewSpreadsheet, window: &mut Window, cx: &mut Context<Self>) {
        self.launch(AppKind::Sheets, None, window, cx);
    }

    fn new_document(&mut self, _: &NewDocument, window: &mut Window, cx: &mut Context<Self>) {
        self.launch(AppKind::Docs, None, window, cx);
    }

    fn new_presentation(
        &mut self,
        _: &NewPresentation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.launch(AppKind::Slides, None, window, cx);
    }

    fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match AppKind::for_path(&path) {
            Some(app) => self.launch(app, Some(path), window, cx),
            None => {
                self.message = format!("Kantor can't open {}", path.display()).into();
                cx.notify();
            }
        }
    }

    /// Print the focused document: a PDF opened in the system's viewer
    /// (Slides: the layout chosen last, slides by default).
    pub(crate) fn print(&mut self, _: &Print, window: &mut Window, cx: &mut Context<Self>) {
        self.with_docs(window, cx, |v, w, cx| v.print_doc(w, cx));
        self.with_slides(window, cx, |v, w, cx| v.print_doc(w, cx));
        self.with_sheets(window, cx, |v, w, cx| v.print_doc(w, cx));
    }

    fn open_file(&mut self, _: &OpenFile, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Open document".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update_in(cx, |this, window, cx| this.open_path(path, window, cx));
        })
        .detach();
    }

    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        let kind = self.focused_view().map_or(AppKind::Sheets, |v| v.kind());
        let Some(ti) = self.active else {
            self.launch(kind, None, window, cx);
            return;
        };
        let Some(pane) = self.new_doc(kind, None, window, cx) else {
            return;
        };
        let tab = &mut self.tabs[ti];
        if let Some(layout) = tab.layout.split(tab.focused, pane, axis) {
            tab.layout = layout;
            tab.focused = pane;
        }
        self.activate(Some(ti), window, cx);
    }

    fn split_right(&mut self, _: &SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Horizontal, window, cx);
    }

    fn split_down(&mut self, _: &SplitDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Vertical, window, cx);
    }

    /// Remove one pane, discarding its document; a tab's last pane removes
    /// the tab. Use [`Self::request_close`] to ask about unsaved changes first.
    fn remove_pane(
        &mut self,
        ti: usize,
        pane: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tabs.get_mut(ti) else {
            return;
        };
        match tab.layout.without(pane) {
            Some(layout) => {
                if tab.focused == pane {
                    tab.focused = layout.panes()[0];
                }
                tab.layout = layout;
                self.docs.remove(&pane);
                self.forget_recovery(pane, cx);
                self.activate(Some(ti), window, cx);
            }
            None => self.remove_tab(ti, window, cx),
        }
    }

    /// Remove the pane showing document `pane`, without asking.
    fn remove_doc(&mut self, pane: PaneId, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ti) = self
            .tabs
            .iter()
            .position(|t| t.layout.panes().contains(&pane))
        {
            self.remove_pane(ti, pane, window, cx);
        }
    }

    fn close_pane(&mut self, _: &ClosePane, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.active.and_then(|ti| self.tabs.get(ti)) {
            let closing = Closing::Pane {
                tab: tab.id,
                pane: tab.focused,
            };
            self.request_close(closing, window, cx);
        }
    }

    /// Close tab `ix`, asking first if it has unsaved changes.
    fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab) = self.tabs.get(ix) {
            self.request_close(Closing::Tab(tab.id), window, cx);
        }
    }

    /// Remove tab `ix` and discard its documents.
    fn remove_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(ix);
        for pane in tab.layout.panes() {
            self.docs.remove(&pane);
            self.forget_recovery(pane, cx);
        }
        let next = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(a) if a > ix => Some(a - 1),
            Some(a) if a == ix => Some(ix.min(self.tabs.len() - 1)),
            other => other,
        };
        self.activate(next, window, cx);
    }

    fn close_active_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.active {
            self.close_tab(ix, window, cx);
        }
    }

    fn cycle(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Home counts as a position before the first tab.
        let n = self.tabs.len() + 1;
        let pos = self.active.map_or(0, |i| i + 1);
        let pos = if forward {
            (pos + 1) % n
        } else {
            (pos + n - 1) % n
        };
        self.activate(pos.checked_sub(1), window, cx);
    }

    fn next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(true, window, cx);
    }

    fn prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(false, window, cx);
    }

    fn go_home(&mut self, _: &GoHome, window: &mut Window, cx: &mut Context<Self>) {
        self.activate(None, window, cx);
    }

    fn toggle_sidebar(&mut self, _: &ToggleSidebar, _: &mut Window, cx: &mut Context<Self>) {
        self.ui.sidebar_open = !self.ui.sidebar_open;
        self.persist(cx);
        cx.notify();
    }

    fn toggle_inspector(&mut self, _: &ToggleInspector, _: &mut Window, cx: &mut Context<Self>) {
        self.ui.inspector_open = !self.ui.inspector_open;
        self.persist(cx);
        cx.notify();
    }

    fn quit(&mut self, _: &Quit, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(Closing::Quit, window, cx);
    }

    /// Quit from outside the workspace's focus (e.g. while a dialog has it).
    pub fn request_quit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.request_close(Closing::Quit, window, cx);
    }

    // ── dragging (panel edges, split dividers) ─────────────────────────────

    fn start_drag(&mut self, drag: Drag, cx: &mut Context<Self>) {
        self.drag = Some(drag);
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.end_drag(cx);
            return;
        }
        let Some(drag) = &self.drag else {
            return;
        };
        let x = event.position.x;
        match drag {
            Drag::Sidebar { origin_x, origin_w } => {
                self.ui.sidebar_width = UiState::clamp(origin_w + (x - *origin_x) / px(1.));
            }
            Drag::Inspector { origin_x, origin_w } => {
                self.ui.inspector_width = UiState::clamp(origin_w - (x - *origin_x) / px(1.));
            }
            Drag::Split { path, axis } => {
                let Some(bounds) = self.split_bounds.borrow().get(path).copied() else {
                    return;
                };
                let ratio = match axis {
                    Axis::Horizontal => (x - bounds.origin.x) / bounds.size.width,
                    Axis::Vertical => (event.position.y - bounds.origin.y) / bounds.size.height,
                };
                let path = path.clone();
                if let Some(tab) = self.active.and_then(|i| self.tabs.get_mut(i)) {
                    tab.layout.set_ratio(&path, ratio);
                }
            }
        }
        cx.notify();
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            self.persist(cx);
            cx.notify();
        }
    }

    // ── session ────────────────────────────────────────────────────────────

    fn restore_tabs(&mut self, session: &Session, window: &mut Window, cx: &mut Context<Self>) {
        for saved in &session.tabs {
            let mut ids = HashMap::new();
            for p in &saved.panes {
                if p.path.is_file()
                    && let Some(kind) = AppKind::for_path(&p.path)
                    && let Some(id) = self.new_doc(kind, Some(p.path.clone()), window, cx)
                {
                    ids.insert(p.id, id);
                }
            }
            let Some(layout) = saved.layout.map_panes(&mut |old| ids.get(&old).copied()) else {
                continue;
            };
            let focused = ids
                .get(&saved.focused)
                .copied()
                .unwrap_or_else(|| layout.panes()[0]);
            let id = self.alloc_id();
            self.tabs.push(Tab {
                id,
                layout,
                focused,
            });
        }
    }

    /// Save open tabs (saved files only), panel sizes and recent files.
    fn persist(&self, cx: &App) {
        let mut tabs = Vec::new();
        let mut active = None;
        for (ti, tab) in self.tabs.iter().enumerate() {
            let mut panes = Vec::new();
            let layout = tab.layout.map_panes(&mut |pane| {
                let path = self.docs.get(&pane)?.view.path(cx)?;
                panes.push(PaneState { id: pane, path });
                Some(pane)
            });
            let Some(layout) = layout else {
                continue;
            };
            let focused = if layout.contains(tab.focused) {
                tab.focused
            } else {
                layout.panes()[0]
            };
            if self.active == Some(ti) {
                active = Some(tabs.len());
            }
            tabs.push(TabState {
                layout,
                focused,
                panes,
            });
        }
        let session = Session {
            tabs,
            active,
            ui: self.ui.clone(),
            recent: self.recent.clone(),
        };
        if let Err(err) = session.save() {
            log::warn!("saving session: {err:#}");
        }
    }

    // ── rendering ──────────────────────────────────────────────────────────

    fn render_content(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.active {
            Some(ti) => self.render_layout_root(ti, cx),
            None => self.render_home(cx).into_any_element(),
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _span = kantor_ui::perf::span("workspace");
        window.set_rem_size(REM);
        let t = tokens(cx);
        let content = self.render_content(cx);
        let palette = self
            .palette
            .as_ref()
            .and(self.chrome.as_ref())
            .map(|c| c.palette.clone());
        let ai_panel = self.render_ai(cx);
        let chrome = self.chrome.as_ref().map(|c| {
            (
                c.tabs.clone(),
                c.status.clone(),
                c.sidebar.clone(),
                c.inspector.clone(),
            )
        });
        let drag_cursor = match &self.drag {
            None => None,
            Some(Drag::Split {
                axis: Axis::Vertical,
                ..
            }) => Some(CursorStyle::ResizeUpDown),
            Some(_) => Some(CursorStyle::ResizeLeftRight),
        };

        let mut root = v_flex()
            .id("workspace")
            .key_context(WORKSPACE)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::toggle_palette))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::palette_up))
            .on_action(cx.listener(Self::palette_down))
            .on_action(cx.listener(Self::new_spreadsheet))
            .on_action(cx.listener(Self::new_document))
            .on_action(cx.listener(Self::new_presentation))
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::print))
            .on_action(cx.listener(Self::close_pane))
            .on_action(cx.listener(Self::close_active_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::go_home))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_inspector))
            .on_action(cx.listener(Self::split_right))
            .on_action(cx.listener(Self::split_down))
            .on_action(cx.listener(Self::quit))
            .on_action(cx.listener(Self::open_ai_settings))
            .on_action(cx.listener(Self::ask_for_formula))
            .on_action(cx.listener(Self::explain_formula))
            .on_action(cx.listener(Self::rewrite_selection))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|ws, _: &MouseUpEvent, _, cx| ws.end_drag(cx)),
            )
            .relative()
            .size_full()
            .bg(t.bg)
            .text_color(t.fg)
            .text_size(FONT_BODY)
            .font_family(cx.theme().font_family.clone());
        if let Some(cursor) = drag_cursor {
            root = root.cursor(cursor);
        }

        let Some((tabs, status, sidebar, inspector)) = chrome else {
            return root.child(content);
        };
        let column = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                tabs.cached(
                    div()
                        .flex_none()
                        .w_full()
                        .h(TITLE_BAR_HEIGHT)
                        .style()
                        .clone(),
                ),
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(div().flex_1().min_w_0().h_full().child(content))
                    .when(self.ui.inspector_open, |d| {
                        d.child(
                            inspector.cached(
                                div()
                                    .flex_none()
                                    .h_full()
                                    .w(px(self.ui.inspector_width))
                                    .style()
                                    .clone(),
                            ),
                        )
                    }),
            );
        root.child(
            h_flex()
                .flex_1()
                .min_h_0()
                .w_full()
                .when(self.ui.sidebar_open, |d| {
                    d.child(
                        sidebar.cached(
                            div()
                                .flex_none()
                                .h_full()
                                .w(px(self.ui.sidebar_width))
                                .style()
                                .clone(),
                        ),
                    )
                })
                .child(column),
        )
        .child(
            status.cached(
                div()
                    .flex_none()
                    .w_full()
                    .h(STATUS_BAR_HEIGHT)
                    .style()
                    .clone(),
            ),
        )
        .children(palette)
        .children(ai_panel)
    }
}
