//! The suite shell, laid out like Voltrus Tools:
//!
//! ```text
//! ┌ tab strip (38) ── [K] [budget.xlsx ×] [report.xlsx ×] [+] ──────── [Ctrl K] ┐
//! │ document pane (cached) or Home                                              │
//! └ status bar (26) ── ● Sheets · B7 · Saved ─────────────── vantablack · 0.0.1 ┘
//! ```
//!
//! One window holds any number of documents as tabs; Home shows when no tab is
//! active. The tab strip and status bar are cached views; the command palette
//! (Ctrl+K) is the main entry point, since Linux has no global menu bar.

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::component::input::{self, Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_core::fuzzy;
use kantor_ui::components::{dot, kbd, section_label};
use kantor_ui::metrics::*;
use kantor_ui::tokens::Elevation;
use kantor_ui::{CachedView, tokens};
use sheets_ui::SheetsView;

use crate::actions::*;

const LOGO_SVG: &[u8] = include_bytes!("../../../assets/icons/kantor.svg");

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
            "xlsx" | "xlsm" | "xls" | "ods" | "csv" => Some(Self::Sheets),
            "docx" | "doc" | "odt" | "rtf" => Some(Self::Docs),
            "pptx" | "ppt" | "odp" => Some(Self::Slides),
            _ => None,
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

/// An open document (one tab).
struct Doc {
    id: usize,
    view: Entity<SheetsView>,
    _changes: Subscription,
}

/// A palette command.
struct Command {
    title: &'static str,
    detail: &'static str,
    keys: &'static str,
    run: fn(&mut Workspace, &mut Window, &mut Context<Workspace>),
}

const COMMANDS: &[Command] = &[
    Command {
        title: "New Spreadsheet",
        detail: "Sheets",
        keys: "Ctrl N",
        run: |ws, window, cx| ws.new_spreadsheet(&NewSpreadsheet, window, cx),
    },
    Command {
        title: "Open File…",
        detail: ".xlsx",
        keys: "Ctrl O",
        run: |ws, window, cx| ws.open_file(&OpenFile, window, cx),
    },
    Command {
        title: "Save",
        detail: "Current document",
        keys: "Ctrl S",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.save_doc(w, cx)),
    },
    Command {
        title: "Save As…",
        detail: "Current document",
        keys: "Ctrl Shift S",
        run: |ws, window, cx| ws.with_active(window, cx, |v, w, cx| v.save_doc_as(w, cx)),
    },
    Command {
        title: "Close Tab",
        detail: "",
        keys: "Ctrl W",
        run: |ws, window, cx| ws.close_tab(&CloseTab, window, cx),
    },
    Command {
        title: "Next Tab",
        detail: "",
        keys: "Ctrl Tab",
        run: |ws, window, cx| ws.next_tab(&NextTab, window, cx),
    },
    Command {
        title: "Previous Tab",
        detail: "",
        keys: "Ctrl Shift Tab",
        run: |ws, window, cx| ws.prev_tab(&PrevTab, window, cx),
    },
    Command {
        title: "Go Home",
        detail: "Start screen",
        keys: "Ctrl Shift H",
        run: |ws, window, cx| ws.go_home(&GoHome, window, cx),
    },
    Command {
        title: "New Document",
        detail: "Docs · coming soon",
        keys: "",
        run: |ws, _, cx| ws.not_yet(AppKind::Docs, cx),
    },
    Command {
        title: "New Presentation",
        detail: "Slides · coming soon",
        keys: "",
        run: |ws, _, cx| ws.not_yet(AppKind::Slides, cx),
    },
    Command {
        title: "Quit Kantor",
        detail: "",
        keys: "Ctrl Q",
        run: |_, _, cx| cx.quit(),
    },
];

struct Palette {
    input: Entity<InputState>,
    /// Indices into [`COMMANDS`], best match first.
    matches: Vec<usize>,
    selected: usize,
    _subscription: Subscription,
}

struct Chrome {
    tabs: Entity<CachedView<Workspace>>,
    status: Entity<CachedView<Workspace>>,
}

pub struct Workspace {
    docs: Vec<Doc>,
    /// Index into `docs`; `None` shows Home.
    active: Option<usize>,
    next_id: usize,
    palette: Option<Palette>,
    message: SharedString,
    logo: Arc<Image>,
    focus: FocusHandle,
    chrome: Option<Chrome>,
}

impl Workspace {
    pub fn new(
        app: Option<AppKind>,
        file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            docs: Vec::new(),
            active: None,
            next_id: 0,
            palette: None,
            message: SharedString::default(),
            logo: Arc::new(Image::from_bytes(ImageFormat::Svg, LOGO_SVG.to_vec())),
            focus: cx.focus_handle(),
            chrome: None,
        };
        this.chrome = Some(Chrome {
            tabs: CachedView::new(cx, |ws, window, cx| {
                ws.render_tab_strip(window, cx).into_any_element()
            }),
            status: CachedView::new(cx, |ws, _, cx| ws.render_status_bar(cx).into_any_element()),
        });
        let app = app.or_else(|| file.as_deref().and_then(AppKind::for_path));
        match app {
            Some(app) => this.launch(app, file, window, cx),
            None => this.go_home(&GoHome, window, cx),
        }
        this
    }

    // ── documents ──────────────────────────────────────────────────────────

    fn launch(
        &mut self,
        app: AppKind,
        file: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match app {
            AppKind::Sheets => {
                // Reuse the tab if this file is already open.
                if let Some(path) = &file
                    && let Some(ix) = self
                        .docs
                        .iter()
                        .position(|d| d.view.read(cx).path(cx).as_deref() == Some(path.as_path()))
                {
                    self.activate(Some(ix), window, cx);
                    return;
                }
                let view = cx.new(|cx| SheetsView::new(file, window, cx));
                let changes = cx.observe(&view, |_, _, cx| cx.notify());
                self.docs.push(Doc {
                    id: self.next_id,
                    view,
                    _changes: changes,
                });
                self.next_id += 1;
                self.activate(Some(self.docs.len() - 1), window, cx);
            }
            AppKind::Docs | AppKind::Slides => self.not_yet(app, cx),
        }
    }

    fn not_yet(&mut self, app: AppKind, cx: &mut Context<Self>) {
        self.message = format!("{} is not built yet — it's on the roadmap.", app.name()).into();
        cx.notify();
    }

    fn activate(&mut self, ix: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.active = ix.filter(|&i| i < self.docs.len());
        match self.active {
            Some(i) => {
                let view = self.docs[i].view.clone();
                let title = view.read(cx).title(cx);
                window.set_window_title(&title);
                view.update(cx, |v, cx| v.focus(window, cx));
            }
            None => {
                window.set_window_title("Kantor");
                window.focus(&self.focus, cx);
            }
        }
        cx.notify();
    }

    fn with_active(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut SheetsView, &mut Window, &mut Context<SheetsView>),
    ) {
        if let Some(i) = self.active {
            let view = self.docs[i].view.clone();
            view.update(cx, |v, cx| f(v, window, cx));
        }
    }

    fn new_spreadsheet(&mut self, _: &NewSpreadsheet, window: &mut Window, cx: &mut Context<Self>) {
        self.launch(AppKind::Sheets, None, window, cx);
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
            let _ = this.update_in(cx, |this, window, cx| match AppKind::for_path(&path) {
                Some(app) => this.launch(app, Some(path), window, cx),
                None => {
                    this.message = format!("Kantor can't open {}", path.display()).into();
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn close_doc(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.docs.len() {
            return;
        }
        // TODO(M1): ask before discarding unsaved changes.
        self.docs.remove(ix);
        let next = match self.active {
            _ if self.docs.is_empty() => None,
            Some(a) if a > ix => Some(a - 1),
            Some(a) if a == ix => Some(ix.min(self.docs.len() - 1)),
            other => other,
        };
        self.activate(next, window, cx);
    }

    fn close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.active {
            self.close_doc(ix, window, cx);
        }
    }

    fn cycle(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        // Home counts as a position before the first tab.
        let n = self.docs.len() + 1;
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

    // ── command palette ────────────────────────────────────────────────────

    fn toggle_palette(&mut self, _: &TogglePalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.is_some() {
            self.close_palette(window, cx);
            return;
        }
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Type a command…"));
        let subscription = cx.subscribe_in(&input, window, Self::on_palette_input);
        input.update(cx, |i, cx| i.focus(window, cx));
        self.palette = Some(Palette {
            input,
            matches: (0..COMMANDS.len()).collect(),
            selected: 0,
            _subscription: subscription,
        });
        cx.notify();
    }

    fn on_palette_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let query = input.read(cx).value().to_string();
                if let Some(p) = self.palette.as_mut() {
                    p.matches = fuzzy::rank(&query, COMMANDS.iter().map(|c| c.title));
                    p.selected = 0;
                }
                cx.notify();
            }
            InputEvent::PressEnter { .. } => self.run_selected(window, cx),
            _ => {}
        }
    }

    fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette.take().is_some() {
            // Return focus to whatever the palette covered.
            self.activate(self.active, window, cx);
        }
    }

    fn dismiss(&mut self, _: &Dismiss, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(window, cx);
    }

    fn palette_escape(&mut self, _: &input::Escape, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(window, cx);
    }

    fn palette_move(&mut self, delta: isize, cx: &mut Context<Self>) {
        if let Some(p) = self.palette.as_mut() {
            let n = p.matches.len().min(PALETTE_MAX_ROWS) as isize;
            if n > 0 {
                p.selected = (p.selected as isize + delta).rem_euclid(n) as usize;
                cx.notify();
            }
        }
    }

    fn palette_up(&mut self, _: &PaletteUp, _: &mut Window, cx: &mut Context<Self>) {
        self.palette_move(-1, cx);
    }

    fn palette_down(&mut self, _: &PaletteDown, _: &mut Window, cx: &mut Context<Self>) {
        self.palette_move(1, cx);
    }

    fn run_command(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette(window, cx);
        if let Some(cmd) = COMMANDS.get(ix) {
            (cmd.run)(self, window, cx);
        }
    }

    fn run_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self
            .palette
            .as_ref()
            .and_then(|p| p.matches.get(p.selected).copied());
        match ix {
            Some(ix) => self.run_command(ix, window, cx),
            None => self.close_palette(window, cx),
        }
    }

    fn quit(&mut self, _: &Quit, _: &mut Window, cx: &mut Context<Self>) {
        cx.quit();
    }

    // ── rendering ──────────────────────────────────────────────────────────

    fn render_tab_strip(&mut self, _: &mut Window, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let home_selected = self.active.is_none();
        let entity = cx.entity().downgrade();

        let home = {
            let entity = entity.clone();
            div()
                .id("tab-home")
                .flex_none()
                .size(TAB_HEIGHT)
                .flex()
                .items_center()
                .justify_center()
                .rounded(RADIUS_MD)
                .cursor_pointer()
                .when_else(
                    home_selected,
                    |d| d.bg(t.bg).border_1().border_color(t.border_strong),
                    |d| d.hover(|s| s.bg(t.hover)),
                )
                .child(img(self.logo.clone()).size(ICON_LG + SPACE_1))
                .on_click(move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.go_home(&GoHome, window, cx));
                })
        };

        let mut tabs = Vec::with_capacity(self.docs.len());
        for (ix, doc) in self.docs.iter().enumerate() {
            let selected = self.active == Some(ix);
            let view = doc.view.read(cx);
            let name = SharedString::from(view.doc_name(cx));
            let dirty = view.is_dirty(cx);
            let group = SharedString::from(format!("tab-{}", doc.id));
            let (on_select, on_close) = (entity.clone(), entity.clone());
            tabs.push(
                div()
                    .id(("tab", doc.id))
                    .group(group.clone())
                    .flex_none()
                    .h(TAB_HEIGHT)
                    .pl(SPACE_5)
                    .pr(SPACE_2)
                    .flex()
                    .items_center()
                    .gap(SPACE_3)
                    .rounded(RADIUS_MD)
                    .text_size(FONT_SMALL)
                    .font_weight(FontWeight::MEDIUM)
                    .cursor_pointer()
                    .when_else(
                        selected,
                        |d| {
                            d.bg(t.bg)
                                .border_1()
                                .border_color(t.border_strong)
                                .text_color(t.fg)
                        },
                        |d| d.text_color(t.fg_muted).hover(|s| s.bg(t.hover)),
                    )
                    .child(dot(if dirty { t.warning } else { t.sheets }))
                    .child(div().w(TAB_TITLE_WIDTH).truncate().child(name))
                    .child(
                        div()
                            .id(("tab-close", doc.id))
                            .size(px(18.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(RADIUS_SM)
                            .text_color(t.fg_dim)
                            .hover(|s| s.bg(t.hover).text_color(t.fg))
                            .when(!selected, |d| {
                                d.invisible().group_hover(group, |s| s.visible())
                            })
                            .child("×")
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                let _ = on_close.update(cx, |ws, cx| ws.close_doc(ix, window, cx));
                            }),
                    )
                    .on_click(move |_, window, cx| {
                        let _ = on_select.update(cx, |ws, cx| ws.activate(Some(ix), window, cx));
                    }),
            );
        }

        let new_tab = {
            let entity = entity.clone();
            div()
                .id("tab-new")
                .flex_none()
                .size(TAB_HEIGHT)
                .flex()
                .items_center()
                .justify_center()
                .rounded(RADIUS_MD)
                .text_color(t.fg_muted)
                .cursor_pointer()
                .hover(|s| s.bg(t.hover).text_color(t.fg))
                .child("+")
                .on_click(move |_, window, cx| {
                    let _ =
                        entity.update(cx, |ws, cx| ws.new_spreadsheet(&NewSpreadsheet, window, cx));
                })
        };

        let palette_hint = div()
            .id("palette-hint")
            .flex_none()
            .h(TAB_HEIGHT)
            .px(SPACE_4)
            .flex()
            .items_center()
            .gap(SPACE_3)
            .rounded(RADIUS_MD)
            .text_size(FONT_SMALL)
            .text_color(t.fg_muted)
            .cursor_pointer()
            .hover(|s| s.bg(t.hover))
            .child("Commands")
            .child(kbd("Ctrl K", cx))
            .on_click(move |_, window, cx| {
                let _ = entity.update(cx, |ws, cx| ws.toggle_palette(&TogglePalette, window, cx));
            });

        h_flex()
            .size_full()
            .px(SPACE_4)
            .gap(SPACE_2)
            .bg(t.chrome)
            .border_b_1()
            .border_color(t.border)
            .child(home)
            .child(
                h_flex()
                    .id("tab-list")
                    .flex_1()
                    .min_w_0()
                    .gap(SPACE_2)
                    .overflow_x_scroll()
                    .children(tabs)
                    .child(new_tab),
            )
            .child(palette_hint)
    }

    fn render_status_bar(&mut self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let (app, color, detail, status) = match self.active {
            Some(i) => {
                let v = self.docs[i].view.read(cx);
                (AppKind::Sheets.name(), t.sheets, v.cell_ref(), v.status())
            }
            None => ("Home", t.accent, String::new(), self.message.clone()),
        };
        let theme = kantor_theme::theme_name().unwrap_or_else(|| "default".to_owned());
        h_flex()
            .size_full()
            .px(SPACE_6)
            .gap(SPACE_6)
            .bg(t.chrome)
            .border_t_1()
            .border_color(t.border)
            .text_size(FONT_CAPTION)
            .text_color(t.fg_muted)
            .child(
                h_flex()
                    .gap(SPACE_3)
                    .child(dot(color))
                    .child(div().text_color(t.fg).child(app)),
            )
            .when(!detail.is_empty(), |d| d.child(div().child(detail)))
            .child(div().flex_1().min_w_0().truncate().child(status))
            .child(div().child(theme))
            .child(
                div()
                    .text_color(t.fg_dim)
                    .child(concat!("v", env!("CARGO_PKG_VERSION"))),
            )
    }

    fn render_card(
        &self,
        id: &'static str,
        app: AppKind,
        subtitle: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let t = tokens(cx);
        let (accent, ready) = match app {
            AppKind::Docs => (t.docs, false),
            AppKind::Sheets => (t.sheets, true),
            AppKind::Slides => (t.slides, false),
        };
        let entity = cx.entity().downgrade();
        v_flex()
            .id(id)
            .w(px(208.))
            .p(SPACE_7)
            .gap(SPACE_4)
            .rounded(RADIUS_XL)
            .bg(t.surface)
            .border_1()
            .border_color(t.border)
            .when(ready, |d| {
                d.cursor_pointer()
                    .hover(|s| s.border_color(t.border_strong).bg(t.elevated))
            })
            .child(div().w(px(28.)).h(px(4.)).rounded(px(2.)).bg(accent))
            .child(
                div()
                    .text_size(FONT_TITLE)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.fg)
                    .child(app.name()),
            )
            .child(
                div()
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(subtitle),
            )
            .child(
                div()
                    .text_size(FONT_CAPTION)
                    .text_color(if ready { accent } else { t.fg_dim })
                    .child(if ready { "New  →" } else { "Coming soon" }),
            )
            .on_click(move |_, window, cx| {
                let _ = entity.update(cx, |ws, cx| ws.launch(app, None, window, cx));
            })
    }

    fn render_home(&mut self, cx: &mut Context<Self>) -> Div {
        let t = tokens(cx);
        let docs = self.render_card("home-docs", AppKind::Docs, "Write .docx documents", cx);
        let sheets = self.render_card(
            "home-sheets",
            AppKind::Sheets,
            "Edit .xlsx spreadsheets",
            cx,
        );
        let slides = self.render_card("home-slides", AppKind::Slides, "Present .pptx decks", cx);
        let hints: Vec<Div> = [
            ("Ctrl N", "New spreadsheet"),
            ("Ctrl O", "Open file"),
            ("Ctrl K", "All commands"),
        ]
        .into_iter()
        .map(|(keys, label)| h_flex().gap(SPACE_3).child(kbd(keys, cx)).child(label))
        .collect();
        let create = section_label("Create", cx);

        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap(SPACE_9)
            .bg(t.bg)
            .child(img(self.logo.clone()).size(px(88.)))
            .child(
                v_flex()
                    .items_center()
                    .gap(SPACE_2)
                    .child(
                        div()
                            .text_size(FONT_DISPLAY)
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.fg)
                            .child("Kantor"),
                    )
                    .child(
                        div()
                            .text_size(FONT_BODY)
                            .text_color(t.fg_muted)
                            .child("Docs, Sheets and Slides — native, fast, yours."),
                    ),
            )
            .child(
                v_flex().gap(SPACE_4).child(create).child(
                    h_flex()
                        .gap(SPACE_6)
                        .child(docs)
                        .child(sheets)
                        .child(slides),
                ),
            )
            .child(
                h_flex()
                    .gap(SPACE_8)
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .children(hints),
            )
            .child(
                div()
                    .h(px(18.))
                    .text_size(FONT_SMALL)
                    .text_color(t.fg_muted)
                    .child(self.message.clone()),
            )
    }

    fn render_palette(&mut self, cx: &mut Context<Self>) -> Option<Div> {
        let t = tokens(cx);
        let entity = cx.entity().downgrade();
        let palette = self.palette.as_ref()?;

        let mut rows = Vec::new();
        for (row, &ix) in palette.matches.iter().take(PALETTE_MAX_ROWS).enumerate() {
            let cmd = &COMMANDS[ix];
            let selected = row == palette.selected;
            let entity = entity.clone();
            rows.push(
                h_flex()
                    .id(("palette-row", ix))
                    .h(PALETTE_ROW_HEIGHT)
                    .px(SPACE_6)
                    .gap(SPACE_4)
                    .rounded(RADIUS_MD)
                    .cursor_pointer()
                    .when_else(
                        selected,
                        |d| d.bg(t.selected),
                        |d| d.hover(|s| s.bg(t.hover)),
                    )
                    .child(div().text_size(FONT_BODY).text_color(t.fg).child(cmd.title))
                    .child(
                        div()
                            .flex_1()
                            .text_size(FONT_SMALL)
                            .text_color(t.fg_dim)
                            .child(cmd.detail),
                    )
                    .when(!cmd.keys.is_empty(), |d| d.child(kbd(cmd.keys, cx)))
                    .on_click(move |_, window, cx| {
                        let _ = entity.update(cx, |ws, cx| ws.run_command(ix, window, cx));
                    }),
            );
        }

        let count = palette.matches.len();
        let hints: Vec<Div> = [("↑↓", "navigate"), ("↵", "run"), ("Esc", "close")]
            .into_iter()
            .map(|(keys, label)| h_flex().gap(SPACE_2).child(kbd(keys, cx)).child(label))
            .collect();
        let on_escape = cx.listener(Self::palette_escape);
        let panel = v_flex()
            .id("palette")
            .key_context(PALETTE)
            .on_action(on_escape)
            .w(PALETTE_WIDTH)
            .mt(PALETTE_TOP)
            .rounded(RADIUS_XL)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border_strong)
            .shadow(t.shadow(Elevation::Modal))
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .p(SPACE_4)
                    .border_b_1()
                    .border_color(t.border)
                    .child(Input::new(&palette.input).appearance(false)),
            )
            .child(
                v_flex()
                    .p(SPACE_3)
                    .gap(SPACE_1)
                    .children(rows)
                    .when(count == 0, |d| {
                        d.child(
                            div()
                                .h(PALETTE_ROW_HEIGHT)
                                .px(SPACE_6)
                                .flex()
                                .items_center()
                                .text_size(FONT_SMALL)
                                .text_color(t.fg_dim)
                                .child("No matching commands"),
                        )
                    }),
            )
            .child(
                h_flex()
                    .h(PALETTE_FOOTER_HEIGHT)
                    .px(SPACE_6)
                    .gap(SPACE_6)
                    .bg(t.chrome)
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(FONT_CAPTION)
                    .text_color(t.fg_muted)
                    .children(hints)
                    .child(div().flex_1())
                    .child(format!("{count} commands")),
            );

        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .justify_center()
                .items_start()
                .bg(t.scrim)
                .occlude()
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    let _ = entity.update(cx, |ws, cx| ws.close_palette(window, cx));
                })
                .child(panel),
        )
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        window.set_rem_size(REM);
        let t = tokens(cx);
        let content: AnyElement = match self.active {
            Some(i) => self.docs[i]
                .view
                .clone()
                .cached(div().size_full().style().clone())
                .into_any_element(),
            None => self.render_home(cx).into_any_element(),
        };
        let palette = self.render_palette(cx);
        let chrome = self
            .chrome
            .as_ref()
            .map(|c| (c.tabs.clone(), c.status.clone()));

        let mut root = v_flex()
            .id("workspace")
            .key_context(WORKSPACE)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::toggle_palette))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(Self::palette_up))
            .on_action(cx.listener(Self::palette_down))
            .on_action(cx.listener(Self::new_spreadsheet))
            .on_action(cx.listener(Self::open_file))
            .on_action(cx.listener(Self::close_tab))
            .on_action(cx.listener(Self::next_tab))
            .on_action(cx.listener(Self::prev_tab))
            .on_action(cx.listener(Self::go_home))
            .on_action(cx.listener(Self::quit))
            .relative()
            .size_full()
            .bg(t.bg)
            .text_color(t.fg)
            .text_size(FONT_BODY);
        if let Some((tabs, _)) = &chrome {
            let style = div()
                .flex_none()
                .w_full()
                .h(TITLE_BAR_HEIGHT)
                .style()
                .clone();
            root = root.child(tabs.clone().cached(style));
        }
        root = root.child(div().flex_1().min_h_0().child(content));
        if let Some((_, status)) = &chrome {
            let style = div()
                .flex_none()
                .w_full()
                .h(STATUS_BAR_HEIGHT)
                .style()
                .clone();
            root = root.child(status.clone().cached(style));
        }
        root.children(palette)
    }
}
