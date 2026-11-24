//! The slide show: F5 / Shift+F5 open a borderless fullscreen window with
//! only the slides; with two or more displays it goes on the one that
//! isn't the editor's, and a presenter view (current and next slide,
//! notes, timer, slide strip, pen tools) fills the editor's display. With
//! one display Alt+F5 starts in the presenter view, and Alt+F5 in the show
//! switches between the two.
//!
//! One [`SlideShow`] entity holds the show's state (position, ink, laser,
//! black / white screen, timer); every show window ([`window::ShowWindow`])
//! observes it, so a click or key in either window moves both.
//!
//! Every slide on screen goes through [`paint_show_slide`] and every step
//! through [`SlideShow::advance`] / [`SlideShow::retreat`]: transitions and
//! per-click builds plug in there.

pub mod ink;
mod presenter;
pub mod state;
mod window;

use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::*;
use slides_engine::SlideSize;

use crate::SlidesView;
use crate::draw::{Painted, Surface, paint_slide};
use ink::{Ink, InkKind, Laser, Rect, SlidePoint, Stroke, Tool};
use state::{Nav, NumberEntry, Pos, Screen, Stopwatch};
pub(crate) use window::ShowWindow;

/// Key context of the show windows.
pub const SHOW_CONTEXT: &str = "SlidesShow";

actions!(
    slides_show,
    [
        ShowNext,
        ShowPrev,
        ShowFirst,
        ShowLast,
        /// Enter: the typed slide number, else the next slide.
        ShowEnter,
        /// Esc: back to the arrow, else close the grid, else end the show.
        ShowEscape,
        ShowEnd,
        ToggleBlack,
        ToggleWhite,
        EraseAll,
        ArrowTool,
        LaserTool,
        PenTool,
        HighlighterTool,
        EraserTool,
        ToggleGrid,
        /// One display: switch between the slides and the presenter view.
        TogglePresenterView,
        NotesBigger,
        NotesSmaller,
    ]
);

/// The show windows' key bindings (PowerPoint's).
pub(crate) fn key_bindings() -> Vec<KeyBinding> {
    let c = Some(SHOW_CONTEXT);
    let mut keys = vec![
        KeyBinding::new("right", ShowNext, c),
        KeyBinding::new("down", ShowNext, c),
        KeyBinding::new("space", ShowNext, c),
        KeyBinding::new("pagedown", ShowNext, c),
        KeyBinding::new("n", ShowNext, c),
        KeyBinding::new("enter", ShowEnter, c),
        KeyBinding::new("left", ShowPrev, c),
        KeyBinding::new("up", ShowPrev, c),
        KeyBinding::new("pageup", ShowPrev, c),
        KeyBinding::new("backspace", ShowPrev, c),
        KeyBinding::new("p", ShowPrev, c),
        KeyBinding::new("home", ShowFirst, c),
        KeyBinding::new("end", ShowLast, c),
        KeyBinding::new("escape", ShowEscape, c),
        KeyBinding::new("b", ToggleBlack, c),
        KeyBinding::new(".", ToggleBlack, c),
        KeyBinding::new("w", ToggleWhite, c),
        KeyBinding::new(",", ToggleWhite, c),
        KeyBinding::new("e", EraseAll, c),
        KeyBinding::new("g", ToggleGrid, c),
        KeyBinding::new("alt-f5", TogglePresenterView, c),
    ];
    macro_rules! primary {
        ($($key:literal => $action:expr),* $(,)?) => {
            $(keys.extend(kantor_ui::keys::primary_pair(concat!("ctrl-", $key), $action, c));)*
        };
    }
    primary!(
        "a" => ArrowTool,
        "l" => LaserTool,
        "p" => PenTool,
        "i" => HighlighterTool,
        "e" => EraserTool,
        "=" => NotesBigger,
        "+" => NotesBigger,
        "-" => NotesSmaller,
    );
    keys
}

/// What covers the slides on the audience's screen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cover {
    #[default]
    None,
    Black,
    White,
}

/// Which window a [`ShowWindow`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    /// The slides, for the audience (a presenter window is on another display).
    Audience,
    /// The presenter view, on the presenter's display.
    Presenter,
    /// The only show window (one display): slides, or the presenter view
    /// while [`SlideShow::presenter_view`] is on.
    Single,
}

/// Speaker notes text size, px.
const NOTES_SIZES: [f32; 7] = [14., 16., 18., 22., 26., 32., 40.];

/// The state of a running show, shared by its windows.
pub struct SlideShow {
    view: WeakEntity<SlidesView>,
    editor_window: AnyWindowHandle,
    pub(crate) nav: Nav,
    pub(crate) cover: Cover,
    pub(crate) ink: Ink,
    pub(crate) laser: Laser,
    pub(crate) tool: Tool,
    pub(crate) pen_color: u32,
    pub(crate) highlighter_color: u32,
    pub(crate) number: NumberEntry,
    pub(crate) watch: Stopwatch,
    pub(crate) notes_size: usize,
    /// One display: the single window shows the presenter view.
    pub(crate) presenter_view: bool,
    /// Display for the slides and for the presenter view (`None`: one display).
    audience_display: Option<DisplayId>,
    presenter_display: Option<DisplayId>,
    windows: Vec<AnyWindowHandle>,
    ended: bool,
    deck: Option<Subscription>,
}

/// How a show starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartAt {
    /// F5: the first shown slide.
    First,
    /// Shift+F5: the editor's slide.
    Current,
}

impl SlidesView {
    /// Start the show (F5 / Shift+F5; `presenter` is Alt+F5: the presenter
    /// view even on one display). A running show is brought to the front.
    pub(crate) fn start_show(
        &mut self,
        at: StartAt,
        presenter: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(show) = &self.show {
            let windows = show.read(cx).windows.clone();
            cx.defer(move |cx| {
                for w in windows {
                    let _ = w.update(cx, |_, window, _| window.activate_window());
                }
            });
            return;
        }
        if self.pres.is_empty() {
            return;
        }
        self.editing = None;
        let hidden: Vec<bool> = self.pres.slides().iter().map(|s| s.hidden).collect();
        let start = match at {
            StartAt::First => (0..hidden.len()).find(|&i| !hidden[i]).unwrap_or(0),
            StartAt::Current => self.current,
        };
        let screens: Vec<Screen<DisplayId>> = cx
            .displays()
            .iter()
            .map(|d| {
                let o = d.bounds().origin;
                Screen {
                    id: d.id(),
                    origin: (f32::from(o.x), f32::from(o.y)),
                }
            })
            .collect();
        let primary = cx.primary_display().map(|d| d.id());
        let editor = window.display(cx).map(|d| d.id());
        let (audience, presenter_display) = state::choose_displays(&screens, primary, editor);
        let view = cx.entity();
        let editor_window = window.window_handle();
        let show = cx.new(|cx| {
            let deck = cx.observe(&view, |show: &mut SlideShow, view, cx| {
                let hidden = view
                    .read(cx)
                    .pres
                    .slides()
                    .iter()
                    .map(|s| s.hidden)
                    .collect();
                show.nav.update_slides(hidden);
                cx.notify();
            });
            SlideShow {
                view: view.downgrade(),
                editor_window,
                nav: Nav::new(hidden, start),
                cover: Cover::None,
                ink: Ink::default(),
                laser: Laser::default(),
                tool: Tool::Arrow,
                pen_color: ink::PEN_COLORS[0],
                highlighter_color: ink::HIGHLIGHTER_COLORS[0],
                number: NumberEntry::default(),
                watch: Stopwatch::new(Instant::now()),
                notes_size: 2,
                presenter_view: presenter && presenter_display.is_none(),
                audience_display: audience,
                presenter_display,
                windows: Vec::new(),
                ended: false,
                deck: Some(deck),
            }
        });
        self.show = Some(show.clone());
        cx.notify();
        cx.defer(move |cx| open_windows(&show, cx));
    }

    /// The show ended (its windows are closing): back to the slide it was on.
    fn show_ended(&mut self, slide: usize, cx: &mut Context<Self>) {
        self.show = None;
        self.go_to(slide, cx);
    }
}

/// Open the show's windows on their displays.
fn open_windows(show: &Entity<SlideShow>, cx: &mut App) {
    let (audience, presenter) = {
        let s = show.read(cx);
        (s.audience_display, s.presenter_display)
    };
    let mut opened = Vec::new();
    let mut open = |role: Role, display: Option<DisplayId>, cx: &mut App| {
        let bounds = display
            .and_then(|id| cx.find_display(id))
            .map(|d| d.bounds())
            .unwrap_or_else(|| Bounds::centered(None, size(px(1280.), px(720.)), cx));
        let options = WindowOptions {
            app_id: Some("kantor".into()),
            titlebar: None,
            window_bounds: Some(WindowBounds::Fullscreen(bounds)),
            display_id: display,
            focus: role != Role::Audience || presenter.is_none(),
            window_background: WindowBackgroundAppearance::Opaque,
            ..Default::default()
        };
        let entity = show.clone();
        match gpui_kit::open_window(options, cx, move |window, cx| {
            let w = cx.new(|cx| ShowWindow::new(entity, role, window, cx));
            let focus = w.read(cx).focus.clone();
            window.focus(&focus, cx);
            w
        }) {
            Ok((handle, _)) => opened.push(handle),
            Err(e) => log::error!("slide show: can't open a window: {e:#}"),
        }
    };
    match presenter {
        Some(p) => {
            open(Role::Audience, audience, cx);
            open(Role::Presenter, Some(p), cx);
        }
        None => open(Role::Single, audience, cx),
    }
    let failed = opened.is_empty();
    show.update(cx, |s, cx| {
        s.windows = opened;
        if failed {
            s.end(cx);
        }
    });
}

impl SlideShow {
    /// The deck the show is showing.
    pub(crate) fn view(&self) -> Option<Entity<SlidesView>> {
        self.view.upgrade()
    }

    pub(crate) fn two_displays(&self) -> bool {
        self.presenter_display.is_some()
    }

    /// The next click: a build step, the next slide, the end screen, or
    /// (on the end screen) the end of the show.
    pub(crate) fn advance(&mut self, cx: &mut Context<Self>) {
        self.ink.finish();
        self.number.clear();
        if self.cover != Cover::None {
            self.cover = Cover::None;
        } else if !self.nav.advance() {
            self.end(cx);
            return;
        }
        self.laser.hide();
        cx.notify();
    }

    /// Back one build step or slide.
    pub(crate) fn retreat(&mut self, cx: &mut Context<Self>) {
        self.ink.finish();
        self.number.clear();
        if self.cover != Cover::None {
            self.cover = Cover::None;
        } else {
            self.nav.retreat();
        }
        self.laser.hide();
        cx.notify();
    }

    pub(crate) fn go_to(&mut self, index: usize, cx: &mut Context<Self>) {
        self.ink.finish();
        self.number.clear();
        self.cover = Cover::None;
        self.nav.go_to(index);
        self.laser.hide();
        cx.notify();
    }

    pub(crate) fn first(&mut self, cx: &mut Context<Self>) {
        self.ink.finish();
        self.cover = Cover::None;
        self.nav.first();
        cx.notify();
    }

    pub(crate) fn last(&mut self, cx: &mut Context<Self>) {
        self.ink.finish();
        self.cover = Cover::None;
        self.nav.last();
        cx.notify();
    }

    /// Enter: the typed number's slide, else the next click.
    pub(crate) fn enter(&mut self, cx: &mut Context<Self>) {
        if self.number.is_empty() {
            self.advance(cx);
        } else if let Some(i) = self.number.take(self.nav.len()) {
            self.go_to(i, cx);
        } else {
            cx.notify();
        }
    }

    pub(crate) fn toggle_cover(&mut self, cover: Cover, cx: &mut Context<Self>) {
        self.cover = if self.cover == cover {
            Cover::None
        } else {
            cover
        };
        cx.notify();
    }

    pub(crate) fn set_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        self.ink.finish();
        if tool != Tool::Laser {
            self.laser.hide();
        }
        self.tool = tool;
        cx.notify();
    }

    pub(crate) fn erase_all(&mut self, cx: &mut Context<Self>) {
        if let Some(slide) = self.nav.slide() {
            self.ink.clear(slide);
            cx.notify();
        }
    }

    pub(crate) fn toggle_pause(&mut self, cx: &mut Context<Self>) {
        self.watch.toggle_pause(Instant::now());
        cx.notify();
    }

    pub(crate) fn reset_timer(&mut self, cx: &mut Context<Self>) {
        self.watch.reset(Instant::now());
        cx.notify();
    }

    pub(crate) fn notes_size_px(&self) -> f32 {
        NOTES_SIZES[self.notes_size.min(NOTES_SIZES.len() - 1)]
    }

    pub(crate) fn notes_bigger(&mut self, bigger: bool, cx: &mut Context<Self>) {
        self.notes_size = if bigger {
            (self.notes_size + 1).min(NOTES_SIZES.len() - 1)
        } else {
            self.notes_size.saturating_sub(1)
        };
        cx.notify();
    }

    pub(crate) fn toggle_presenter_view(&mut self, cx: &mut Context<Self>) {
        if !self.two_displays() {
            self.presenter_view = !self.presenter_view;
            cx.notify();
        }
    }

    /// Put the slides on the presenter's display and the other way round.
    pub(crate) fn swap_displays(&mut self, cx: &mut Context<Self>) {
        if !self.two_displays() {
            return;
        }
        std::mem::swap(&mut self.audience_display, &mut self.presenter_display);
        let old = std::mem::take(&mut self.windows);
        let show = cx.entity();
        cx.defer(move |cx| {
            for w in old {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            }
            open_windows(&show, cx);
        });
    }

    /// End the show: close its windows and go back to the editor on the
    /// slide the show was on. Ink is dropped.
    pub(crate) fn end(&mut self, cx: &mut Context<Self>) {
        if self.ended {
            return;
        }
        self.ended = true;
        self.deck = None;
        let windows = std::mem::take(&mut self.windows);
        let slide = self.nav.last_slide();
        let view = self.view.clone();
        let editor = self.editor_window;
        cx.defer(move |cx| {
            for w in windows {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            }
            let Some(view) = view.upgrade() else {
                return;
            };
            view.update(cx, |v, cx| v.show_ended(slide, cx));
            let _ = editor.update(cx, |_, window, cx| {
                window.activate_window();
                view.update(cx, |v, cx| v.focus(window, cx));
            });
        });
        cx.notify();
    }

    /// Start, extend or finish drawing at `p` on the slide on screen, as
    /// the tool does (`phase`: 0 down, 1 drag, 2 up).
    pub(crate) fn pointer(
        &mut self,
        tool: Tool,
        p: SlidePoint,
        phase: u8,
        aspect: f32,
        cx: &mut Context<Self>,
    ) {
        let Some(slide) = self.nav.slide() else {
            return;
        };
        match tool {
            Tool::Arrow => return,
            Tool::Laser => match phase {
                2 if self.tool != Tool::Laser => self.laser.hide(),
                _ => self.laser.move_to(slide, p, Instant::now()),
            },
            Tool::Pen | Tool::Highlighter => {
                let (kind, color) = if tool == Tool::Pen {
                    (InkKind::Pen, self.pen_color)
                } else {
                    (InkKind::Highlighter, self.highlighter_color)
                };
                match phase {
                    0 => self.ink.begin(slide, kind, color, p),
                    1 => self.ink.extend(p),
                    _ => {
                        self.ink.extend(p);
                        self.ink.finish();
                    }
                }
            }
            Tool::Eraser => {
                if !self.ink.erase_at(slide, p, aspect) {
                    return;
                }
            }
        }
        cx.notify();
    }

    /// What a window needs to paint the slide on screen (or `index`).
    pub(crate) fn frame(&self, index: usize, cx: &App) -> Option<SlideFrame> {
        let view = self.view.upgrade()?;
        let v = view.read(cx);
        if index >= v.pres.len() {
            return None;
        }
        let painted = v.painter.painted(&v.pres, index, Default::default(), cx);
        let now = Instant::now();
        let laser = self.laser.at.filter(|(s, _)| *s == index).map(|(_, p)| p);
        let trail = if laser.is_some() {
            self.laser.trail(now).filter(|(_, a)| *a > 0.0).collect()
        } else {
            Vec::new()
        };
        Some(SlideFrame {
            painted,
            bridge: v.painter.bridge.clone(),
            size: v.pres.size,
            strokes: self.ink.strokes(index).cloned().collect(),
            laser,
            trail,
        })
    }

    /// The slide on screen's frame (`None` on the end screen).
    pub(crate) fn current_frame(&self, cx: &App) -> Option<SlideFrame> {
        match self.nav.pos() {
            Pos::Slide { index, .. } => self.frame(index, cx),
            Pos::End => None,
        }
    }
}

/// One slide as a show window paints it: the slide, its ink, the laser.
pub(crate) struct SlideFrame {
    painted: Rc<Painted>,
    bridge: Rc<std::cell::RefCell<docs_ui::FontBridge>>,
    pub size: SlideSize,
    strokes: Vec<Stroke>,
    laser: Option<SlidePoint>,
    trail: Vec<(SlidePoint, f32)>,
}

impl SlideFrame {
    /// A frame without ink or laser (thumbnails, the next slide).
    pub(crate) fn plain(mut self) -> Self {
        self.strokes.clear();
        self.laser = None;
        self.trail.clear();
        self
    }
}

/// Height over width of a slide.
pub(crate) fn aspect(size: SlideSize) -> f32 {
    if size.cx <= 0 {
        0.5625
    } else {
        size.cy as f32 / size.cx as f32
    }
}

pub(crate) fn rect_of(b: Bounds<Pixels>) -> Rect {
    Rect {
        x: f32::from(b.origin.x),
        y: f32::from(b.origin.y),
        w: f32::from(b.size.width),
        h: f32::from(b.size.height),
    }
}

/// Paint `frame` fitted into `bounds` (letterboxed), recording where the
/// slide went in `placed` (for pointer tools). The one place a show slide
/// is painted: transitions wrap this.
pub(crate) fn paint_show_slide(
    frame: &SlideFrame,
    bounds: Bounds<Pixels>,
    placed: Option<&Cell<Bounds<Pixels>>>,
    other: Hsla,
    window: &mut Window,
) {
    let (v, b) = crate::fit_slide(bounds, frame.size);
    if let Some(placed) = placed {
        placed.set(b);
    }
    let surface = Surface {
        other,
        editor: false,
    };
    paint_slide(&frame.painted, b, v, surface, &frame.bridge, window);
    window.with_content_mask(Some(ContentMask { bounds: b }), |window| {
        paint_ink(&frame.strokes, b, window);
        paint_laser(frame, b, window);
    });
}

fn ink_color(s: &Stroke) -> Hsla {
    let mut c: Hsla = rgb(s.color).into();
    if s.kind == InkKind::Highlighter {
        c.a = 0.42;
    }
    c
}

fn circle(center: Point<Pixels>, r: f32, color: Hsla) -> PaintQuad {
    fill(
        Bounds::new(
            point(center.x - px(r), center.y - px(r)),
            size(px(r * 2.), px(r * 2.)),
        ),
        color,
    )
    .corner_radii(px(r))
}

fn paint_ink(strokes: &[Stroke], b: Bounds<Pixels>, window: &mut Window) {
    let rect = rect_of(b);
    for s in strokes {
        let w = (s.width * rect.w).max(1.5);
        let color = ink_color(s);
        let pts: Vec<Point<Pixels>> = s
            .points
            .iter()
            .map(|&p| {
                let (x, y) = rect.to_screen(p);
                point(px(x), px(y))
            })
            .collect();
        if let [one] = pts.as_slice() {
            window.paint_quad(circle(*one, w / 2., color));
            continue;
        }
        let opts = StrokeOptions::default()
            .with_line_width(w)
            .with_line_cap(lyon_tessellation::LineCap::Round)
            .with_line_join(lyon_tessellation::LineJoin::Round);
        let mut pb = PathBuilder::stroke(px(w)).with_style(PathStyle::Stroke(opts));
        pb.add_polygon(&pts, false);
        if let Ok(path) = pb.build() {
            window.paint_path(path, color);
        }
    }
}

/// The laser's red dot (with a soft halo) and its fading trail.
fn paint_laser(frame: &SlideFrame, b: Bounds<Pixels>, window: &mut Window) {
    let Some(at) = frame.laser else {
        return;
    };
    let rect = rect_of(b);
    let r = (rect.w * 0.006).clamp(4., 14.);
    let red = |a: f32| Hsla {
        h: 0.0,
        s: 0.95,
        l: 0.5,
        a,
    };
    let to = |p: SlidePoint| {
        let (x, y) = rect.to_screen(p);
        point(px(x), px(y))
    };
    for &(p, strength) in &frame.trail {
        window.paint_quad(circle(
            to(p),
            r * (0.4 + 0.5 * strength),
            red(0.55 * strength),
        ));
    }
    if !frame.trail.is_empty() {
        // Keep fading after the pointer stops.
        window.request_animation_frame();
    }
    window.paint_quad(circle(to(at), r * 1.9, red(0.25)));
    window.paint_quad(circle(to(at), r, red(1.0)));
}
