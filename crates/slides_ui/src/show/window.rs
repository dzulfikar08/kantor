//! A show window: the slides for the audience (letterboxed on black, the
//! end screen, black / white screens, the corner toolbar and slide grid),
//! or the presenter view ([`super::presenter`]).

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{Icon, h_flex, tooltip::Tooltip, v_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::i18n::{self, tf};
use kantor_ui::{metrics::*, tokens};

use super::ink::{self, Tool};
use super::state::Pos;
use super::*;

/// The corner toolbar and the cursor hide after this long without moving.
const IDLE: Duration = Duration::from_secs(2);

pub(crate) struct ShowWindow {
    pub(super) show: Entity<SlideShow>,
    pub(super) role: Role,
    pub(crate) focus: FocusHandle,
    /// Where the drawable slide was painted last (window px).
    pub(super) placed: Rc<Cell<Bounds<Pixels>>>,
    /// The tool of the drag in progress (Ctrl+drag: the laser).
    drag: Option<Tool>,
    last_move: Option<Instant>,
    /// The slide grid over the show (G).
    pub(super) grid: bool,
    pen_menu: bool,
    pub(super) notes_scroll: ScrollHandle,
    pub(super) strip_scroll: ScrollHandle,
    /// The slide the strip was last scrolled to.
    pub(super) strip_at: Option<usize>,
    _observe: Subscription,
    idle: Option<Task<()>>,
    _ticker: Option<Task<()>>,
}

impl ShowWindow {
    pub(super) fn new(
        show: Entity<SlideShow>,
        role: Role,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let observe = cx.observe(&show, |_, _, cx| cx.notify());
        let weak = show.downgrade();
        // Closing a show window (the window manager, Alt+F4) ends the show.
        window.on_window_should_close(cx, move |_, cx| {
            if let Some(show) = weak.upgrade() {
                show.update(cx, |s, cx| s.end(cx));
            }
            true
        });
        // The presenter's clocks.
        let ticker = (role != Role::Audience).then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    let alive = this.update(cx, |this, cx| {
                        if this.shows_presenter(cx) {
                            cx.notify();
                        }
                    });
                    if alive.is_err() {
                        break;
                    }
                }
            })
        });
        Self {
            show,
            role,
            focus: cx.focus_handle(),
            placed: Rc::default(),
            drag: None,
            last_move: None,
            grid: false,
            pen_menu: false,
            notes_scroll: ScrollHandle::new(),
            strip_scroll: ScrollHandle::new(),
            strip_at: None,
            _observe: observe,
            idle: None,
            _ticker: ticker,
        }
    }

    /// This window shows the presenter view now.
    pub(super) fn shows_presenter(&self, cx: &App) -> bool {
        match self.role {
            Role::Presenter => true,
            Role::Audience => false,
            Role::Single => self.show.read(cx).presenter_view,
        }
    }

    pub(super) fn update_show(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut SlideShow, &mut Context<SlideShow>),
    ) {
        self.show.update(cx, f);
    }

    fn escape(&mut self, cx: &mut Context<Self>) {
        if self.grid || self.pen_menu {
            self.grid = false;
            self.pen_menu = false;
            cx.notify();
            return;
        }
        self.update_show(cx, |s, cx| {
            if s.tool != Tool::Arrow {
                s.set_tool(Tool::Arrow, cx);
            } else if !s.number.is_empty() {
                s.number.clear();
                cx.notify();
            } else {
                s.end(cx);
            }
        });
    }

    fn key_down(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        if k.modifiers.modified() {
            return;
        }
        let digit = self.show.update(cx, |s, cx| {
            let d = s.number.push(&k.key);
            if d {
                cx.notify();
            }
            d
        });
        if digit {
            cx.stop_propagation();
        }
    }

    // ── pointer ─────────────────────────────────────────────────────────

    fn slide_point(&self, p: Point<Pixels>) -> Option<ink::SlidePoint> {
        rect_of(self.placed.get()).to_slide(f32::from(p.x), f32::from(p.y))
    }

    fn mouse_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if self.pen_menu {
            self.pen_menu = false;
            cx.notify();
        }
        let s = self.show.read(cx);
        let tool = s.tool;
        let on_slide = s.cover == Cover::None && s.nav.slide().is_some();
        let aspect = s.view().map_or(0.5625, |v| aspect(v.read(cx).pres.size));
        let p = self.slide_point(e.position).filter(|_| on_slide);
        let ctrl = e.modifiers.control || e.modifiers.platform;
        let drawing = if tool.draws() {
            Some(tool)
        } else if ctrl {
            // Hold Ctrl and drag: the laser for as long as the button is down.
            Some(Tool::Laser)
        } else {
            None
        };
        match (drawing, p) {
            (Some(t), Some(p)) => {
                self.drag = Some(t);
                self.update_show(cx, |s, cx| s.pointer(t, p, 0, aspect, cx));
            }
            // A drawing tool outside the slide does nothing.
            (Some(_), None) if on_slide => {}
            _ => self.update_show(cx, |s, cx| s.advance(cx)),
        }
    }

    fn mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let was_idle = self.is_idle();
        self.last_move = Some(Instant::now());
        self.idle = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(IDLE).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        }));
        if was_idle {
            cx.notify();
        }
        let s = self.show.read(cx);
        let aspect = s.view().map_or(0.5625, |v| aspect(v.read(cx).pres.size));
        let tool = s.tool;
        let on_slide = s.cover == Cover::None && s.nav.slide().is_some();
        if let Some(t) = self.drag {
            if e.pressed_button == Some(MouseButton::Left) {
                let p = rect_of(self.placed.get())
                    .to_slide_clamped(f32::from(e.position.x), f32::from(e.position.y));
                self.update_show(cx, |s, cx| s.pointer(t, p, 1, aspect, cx));
            } else {
                self.drag = None;
                self.update_show(cx, |s, cx| s.pointer(t, (0., 0.), 2, aspect, cx));
            }
        } else if tool == Tool::Laser {
            match self.slide_point(e.position).filter(|_| on_slide) {
                Some(p) => self.update_show(cx, |s, cx| s.pointer(Tool::Laser, p, 1, aspect, cx)),
                None => self.update_show(cx, |s, cx| {
                    if s.laser.at.is_some() {
                        s.laser.hide();
                        cx.notify();
                    }
                }),
            }
        }
    }

    fn mouse_up(&mut self, e: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(t) = self.drag.take() else {
            return;
        };
        let aspect = {
            let s = self.show.read(cx);
            s.view().map_or(0.5625, |v| aspect(v.read(cx).pres.size))
        };
        let p = rect_of(self.placed.get())
            .to_slide_clamped(f32::from(e.position.x), f32::from(e.position.y));
        self.update_show(cx, |s, cx| s.pointer(t, p, 2, aspect, cx));
    }

    fn is_idle(&self) -> bool {
        self.last_move.is_none_or(|t| t.elapsed() >= IDLE)
    }

    /// The drawable slide on screen: `frame` painted letterboxed (black or
    /// white instead under `cover`), with the pointer tools and click to
    /// advance.
    pub(super) fn slide_area(
        &self,
        id: &'static str,
        frame: Option<SlideFrame>,
        cover: Cover,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let placed = self.placed.clone();
        let other = tokens(cx).fg_muted;
        let tool = self.show.read(cx).tool;
        let paint = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| match (cover, &frame) {
                (Cover::Black, _) => window.paint_quad(fill(bounds, black())),
                (Cover::White, _) => window.paint_quad(fill(bounds, white())),
                (Cover::None, Some(f)) => paint_show_slide(f, bounds, Some(&placed), other, window),
                (Cover::None, None) => {}
            },
        )
        .size_full();
        div()
            .id(id)
            .size_full()
            .relative()
            .child(paint)
            .when(tool.draws(), |d| d.cursor(CursorStyle::Crosshair))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e, window, cx| this.mouse_down(e, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, _, cx| {
                    this.update_show(cx, |s, cx| s.retreat(cx))
                }),
            )
            .on_mouse_move(cx.listener(|this, e, _, cx| this.mouse_move(e, cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, e, _, cx| this.mouse_up(e, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, e, _, cx| this.mouse_up(e, cx)),
            )
    }

    // ── audience view ───────────────────────────────────────────────────

    fn render_audience(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let s = self.show.read(cx);
        let pos = s.nav.pos();
        let cover = s.cover;
        let frame = s.current_frame(cx);
        let number = s.number.text().to_owned();
        let presenter_toggle = !s.two_displays();
        let chrome = !self.is_idle() || self.pen_menu;
        let area = match pos {
            Pos::End if cover == Cover::None => self
                .slide_area("show-end", None, cover, cx)
                .child(end_screen()),
            _ => self.slide_area("show-slide", frame, cover, cx),
        };
        div()
            .relative()
            .size_full()
            .bg(black())
            .child(area)
            .when(!number.is_empty(), |d| d.child(number_badge(&number)))
            .when(chrome && !self.grid, |d| {
                d.child(self.corner_toolbar(presenter_toggle, cx))
            })
            .when(self.grid, |d| d.child(self.render_grid(cx)))
            .into_any_element()
    }

    /// PowerPoint's toolbar at the bottom left: previous, next, pens,
    /// all slides, presenter view (one display), end.
    fn corner_toolbar(&self, presenter_toggle: bool, cx: &mut Context<Self>) -> AnyElement {
        let s = self.show.read(cx);
        let tool = s.tool;
        let pen_color = s.pen_color;
        let hl_color = s.highlighter_color;
        let bar = h_flex()
            .gap(SPACE_2)
            .p(SPACE_2)
            .rounded_full()
            .bg(hsla(0., 0., 0.08, 0.62))
            .child(round_button(
                "show-prev",
                Lucide::ChevronLeft,
                false,
                tip(i18n::t("Previous"), None),
                cx.listener(|this, _, _, cx| this.update_show(cx, |s, cx| s.retreat(cx))),
            ))
            .child(round_button(
                "show-next",
                Lucide::ChevronRight,
                false,
                tip(i18n::t("Next"), None),
                cx.listener(|this, _, _, cx| this.update_show(cx, |s, cx| s.advance(cx))),
            ))
            .child(round_button(
                "show-pens",
                Lucide::PenLine,
                tool.draws() || self.pen_menu,
                tip(i18n::t("Pointer and pen tools"), None),
                cx.listener(|this, _, _, cx| {
                    this.pen_menu = !this.pen_menu;
                    cx.notify();
                }),
            ))
            .child(round_button(
                "show-grid",
                Lucide::LayoutGrid,
                false,
                tip(i18n::t("See all slides"), Some("G")),
                cx.listener(|this, _, _, cx| {
                    this.grid = true;
                    cx.notify();
                }),
            ))
            .when(presenter_toggle, |d| {
                d.child(round_button(
                    "show-presenter",
                    Lucide::Presentation,
                    false,
                    tip(i18n::t("Presenter view"), Some("Alt+F5")),
                    cx.listener(|this, _, _, cx| {
                        this.update_show(cx, |s, cx| s.toggle_presenter_view(cx))
                    }),
                ))
            })
            .child(round_button(
                "show-end",
                Lucide::X,
                false,
                tip(i18n::t("End show"), Some("Esc")),
                cx.listener(|this, _, _, cx| this.update_show(cx, |s, cx| s.end(cx))),
            ));
        let menu = self.pen_menu.then(|| {
            v_flex()
                .gap(SPACE_2)
                .p(SPACE_3)
                .rounded(RADIUS_XL)
                .bg(hsla(0., 0., 0.08, 0.78))
                .child(tool_row(tool, cx, true))
                .child(color_row(pen_color, hl_color, tool, cx, true))
        });
        v_flex()
            .absolute()
            .left(SPACE_7)
            .bottom(SPACE_7)
            .gap(SPACE_3)
            .items_start()
            .occlude()
            .children(menu)
            .child(bar)
            .into_any_element()
    }

    /// All slides as thumbnails (G or the toolbar): click one to go there.
    pub(super) fn render_grid(&self, cx: &mut Context<Self>) -> AnyElement {
        let s = self.show.read(cx);
        let current = s.nav.slide();
        let n = s.nav.len();
        let thumbs: Vec<_> = (0..n)
            .map(|i| {
                let frame = s.frame(i, cx).map(SlideFrame::plain);
                let hidden = s.nav.is_hidden(i);
                (i, frame, hidden)
            })
            .collect();
        let t = tokens(cx);
        let accent = t.accent;
        let cells = thumbs.into_iter().map(|(i, frame, hidden)| {
            let paint = canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    if let Some(f) = &frame {
                        paint_show_slide(f, bounds, None, white().opacity(0.3), window);
                    }
                },
            )
            .w(px(240.))
            .h(px(135.));
            v_flex()
                .id(("show-grid-slide", i))
                .gap(SPACE_2)
                .p(SPACE_2)
                .rounded(RADIUS_LG)
                .border_2()
                .border_color(if current == Some(i) {
                    accent
                } else {
                    transparent_black()
                })
                .cursor_pointer()
                .hover(|d| d.bg(white().opacity(0.08)))
                .when(hidden, |d| d.opacity(0.5))
                .child(paint)
                .child(
                    div()
                        .text_size(FONT_BODY)
                        .text_color(white().opacity(0.8))
                        .child(if hidden {
                            tf("{} (hidden)", &[&(i + 1)])
                        } else {
                            (i + 1).to_string()
                        }),
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.grid = false;
                    this.update_show(cx, |s, cx| s.go_to(i, cx));
                }))
        });
        div()
            .id("show-grid")
            .absolute()
            .inset_0()
            .occlude()
            .bg(hsla(0., 0., 0.06, 0.94))
            .overflow_y_scroll()
            .p(SPACE_10)
            .child(div().flex().flex_wrap().gap(SPACE_7).children(cells))
            .into_any_element()
    }
}

impl Render for ShowWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.show.read(cx).view().is_none() {
            // The deck was closed during the show.
            let show = self.show.clone();
            window.defer(cx, move |_, cx| show.update(cx, |s, cx| s.end(cx)));
        }
        let body = if self.shows_presenter(cx) {
            self.render_presenter(window, cx)
        } else {
            self.render_audience(cx)
        };
        macro_rules! on {
            ($div:expr, $($action:ty => |$s:ident, $cx:ident| $body:expr),* $(,)?) => {
                $div$(.on_action(cx.listener(|this, _: &$action, _, cx| {
                    this.update_show(cx, |$s, $cx| $body)
                })))*
            };
        }
        let root = div()
            .id("slide-show")
            .key_context(SHOW_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .bg(black())
            .on_key_down(cx.listener(|this, e, _, cx| this.key_down(e, cx)))
            .on_action(cx.listener(|this, _: &ShowEscape, _, cx| this.escape(cx)))
            .on_action(cx.listener(|this, _: &ToggleGrid, _, cx| {
                this.grid = !this.grid;
                cx.notify();
            }));
        on!(root,
            ShowNext => |s, cx| s.advance(cx),
            ShowPrev => |s, cx| s.retreat(cx),
            ShowFirst => |s, cx| s.first(cx),
            ShowLast => |s, cx| s.last(cx),
            ShowEnter => |s, cx| s.enter(cx),
            ShowEnd => |s, cx| s.end(cx),
            ToggleBlack => |s, cx| s.toggle_cover(Cover::Black, cx),
            ToggleWhite => |s, cx| s.toggle_cover(Cover::White, cx),
            EraseAll => |s, cx| s.erase_all(cx),
            ArrowTool => |s, cx| s.set_tool(Tool::Arrow, cx),
            LaserTool => |s, cx| s.set_tool(Tool::Laser, cx),
            PenTool => |s, cx| s.set_tool(Tool::Pen, cx),
            HighlighterTool => |s, cx| s.set_tool(Tool::Highlighter, cx),
            EraserTool => |s, cx| s.set_tool(Tool::Eraser, cx),
            TogglePresenterView => |s, cx| s.toggle_presenter_view(cx),
            NotesBigger => |s, cx| s.notes_bigger(true, cx),
            NotesSmaller => |s, cx| s.notes_bigger(false, cx),
        )
        .child(body)
    }
}

/// PowerPoint's end screen: black, a line of grey text at the top.
fn end_screen() -> Div {
    div()
        .absolute()
        .top(SPACE_10)
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .text_size(FONT_DISPLAY)
        .text_color(white().opacity(0.75))
        .child(i18n::t("End of slide show, click to exit."))
}

/// A tooltip: the label, then its shortcut as shown on this platform.
pub(super) fn tip(label: &str, shortcut: Option<&str>) -> SharedString {
    kantor_ui::components::tip_text(label, shortcut).into()
}

/// The slide number being typed (Enter goes there).
pub(super) fn number_badge(number: &str) -> Div {
    div()
        .absolute()
        .left(SPACE_7)
        .top(SPACE_7)
        .px(SPACE_6)
        .py(SPACE_4)
        .rounded(RADIUS_LG)
        .bg(hsla(0., 0., 0.08, 0.8))
        .text_size(FONT_DISPLAY)
        .text_color(white())
        .child(tf("Go to slide {}", &[&number]))
}

type Click = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A round icon button for the show's dark toolbars.
fn round_button(
    id: &'static str,
    icon: Lucide,
    active: bool,
    tip: SharedString,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let on_click: Click = Box::new(on_click);
    div()
        .id(id)
        .flex_none()
        .size(px(34.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .cursor_pointer()
        .bg(if active {
            white().opacity(0.22)
        } else {
            transparent_black()
        })
        .hover(|d| d.bg(white().opacity(0.16)))
        .child(
            Icon::new(icon)
                .with_size(ICON_LG)
                .text_color(white().opacity(0.92)),
        )
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(on_click)
}

/// Arrow, laser, pen, highlighter, eraser, erase all. `dark`: on the
/// slides' dark toolbar (else the presenter view's theme chrome).
pub(super) fn tool_row(tool: Tool, cx: &mut Context<ShowWindow>, dark: bool) -> Div {
    let tools = [
        (
            "show-arrow",
            Lucide::MousePointer2,
            Tool::Arrow,
            tip(i18n::t("Arrow"), Some("Ctrl+A")),
        ),
        (
            "show-laser",
            Lucide::CircleDot,
            Tool::Laser,
            tip(i18n::t("Laser pointer"), Some("Ctrl+L")),
        ),
        (
            "show-pen",
            Lucide::PenLine,
            Tool::Pen,
            tip(i18n::t("Pen"), Some("Ctrl+P")),
        ),
        (
            "show-highlighter",
            Lucide::Highlighter,
            Tool::Highlighter,
            tip(i18n::t("Highlighter"), Some("Ctrl+I")),
        ),
        (
            "show-eraser",
            Lucide::Eraser,
            Tool::Eraser,
            tip(i18n::t("Eraser"), Some("Ctrl+E")),
        ),
    ];
    let mut row = h_flex().gap(SPACE_1);
    for (id, icon, t, tip) in tools {
        let click = cx.listener(move |this: &mut ShowWindow, _: &ClickEvent, _, cx| {
            this.update_show(cx, |s, cx| s.set_tool(t, cx))
        });
        row = row.child(if dark {
            round_button(id, icon, tool == t, tip, click).into_any_element()
        } else {
            theme_button(id, icon, tool == t, tip, click, cx).into_any_element()
        });
    }
    let clear = cx.listener(|this: &mut ShowWindow, _: &ClickEvent, _, cx| {
        this.update_show(cx, |s, cx| s.erase_all(cx))
    });
    let tip = tip(i18n::t("Erase all ink on this slide"), Some("E"));
    row.child(if dark {
        round_button("show-erase-all", Lucide::Trash, false, tip.clone(), clear).into_any_element()
    } else {
        theme_button("show-erase-all", Lucide::Trash, false, tip, clear, cx).into_any_element()
    })
}

/// Pen colours (or the highlighter's while it's the tool).
pub(super) fn color_row(
    pen: u32,
    highlighter: u32,
    tool: Tool,
    cx: &mut Context<ShowWindow>,
    dark: bool,
) -> Div {
    let hl = tool == Tool::Highlighter;
    let (colors, chosen): (&[u32], u32) = if hl {
        (&ink::HIGHLIGHTER_COLORS, highlighter)
    } else {
        (&ink::PEN_COLORS, pen)
    };
    let ring = if dark { white() } else { tokens(cx).fg };
    let mut row = h_flex().gap(SPACE_3).px(SPACE_2);
    for (n, &c) in colors.iter().enumerate() {
        let swatch = div()
            .id(("show-ink-color", n))
            .size(px(20.))
            .rounded_full()
            .cursor_pointer()
            .bg(rgb(c))
            .border_2()
            .border_color(if c == chosen {
                ring
            } else {
                ring.opacity(0.25)
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.update_show(cx, |s, cx| {
                    if hl {
                        s.highlighter_color = c;
                    } else {
                        s.pen_color = c;
                        if s.tool != Tool::Pen {
                            s.set_tool(Tool::Pen, cx);
                        }
                    }
                    cx.notify();
                })
            }));
        row = row.child(swatch);
    }
    row
}

/// A square icon button in the presenter view's theme chrome.
pub(super) fn theme_button(
    id: &'static str,
    icon: Lucide,
    active: bool,
    tip: SharedString,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> Stateful<Div> {
    let t = tokens(cx);
    div()
        .id(id)
        .flex_none()
        .size(CONTROL_LG)
        .flex()
        .items_center()
        .justify_center()
        .rounded(RADIUS_MD)
        .cursor_pointer()
        .bg(if active {
            t.selected
        } else {
            transparent_black()
        })
        .hover(move |d| d.bg(if active { t.selected } else { t.hover }))
        .child(Icon::new(icon).with_size(ICON_LG).text_color(if active {
            t.fg
        } else {
            t.fg_muted
        }))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_click(on_click)
}
