//! Slide show transitions and builds on screen.
//!
//! The model is read by [`slides_engine::anim`] (a slide's transition and
//! build steps) and the frames are worked out by [`slides_scene::show`]
//! (offsets, clips and opacities in slide px). Here:
//!
//! - [`BuiltSlide`]: a slide cut into layers prepared for GPUI, one per
//!   animated shape (or paragraph) and one per run of still shapes, so a
//!   build can move, cut or fade one shape in its place in the stacking
//!   order.
//! - [`slide_element`] / [`transition_element`]: elements filling their
//!   parent with the slide fitted and centred (letterbox is the parent's
//!   background), with a build state and a transition frame applied.
//! - [`ShowPlayer`]: the clock. The show window owns one and keeps its own
//!   slide index and build counter; see [`ShowPlayer`] for the calls.
//!
//! GPUI applies opacity per element only (`Styled::opacity`), so every
//! faded layer is its own element. It multiplies each primitive's alpha:
//! overlapping shapes of one faded slide show through each other a little
//! during a fade (PowerPoint fades the composed slide). Nothing is scaled
//! or turned (GPUI can't transform glyphs or pictures at paint time):
//! zooms open a box from the centre instead.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use docs_ui::FontBridge;
use gpui_kit::prelude::*;
use gpui_kit::*;
use slides_engine::anim::{self, SlideFx};
use slides_engine::{EMU_PER_PX, Presentation, Rgb, Slide};
use slides_scene::Options;
use slides_scene::show::{self as frames, BuildState, LayerFx, TransitionFrame};

use crate::draw::{Painted, Painter, View, hsla, paint_items};

/// A slide prepared for the show: its effects and its layers.
pub struct BuiltSlide {
    slide: Arc<Slide>,
    pub fx: Rc<SlideFx>,
    layers: Vec<(Option<anim::Target>, Rc<Painted>)>,
    boxes: HashMap<u32, [f32; 4]>,
    /// Slide px.
    pub width: f32,
    pub height: f32,
    pub background: Rgb,
    /// A picture is still loading: prepare again on a later frame.
    pub loading: bool,
}

impl BuiltSlide {
    pub fn new(painter: &Painter, pres: &Presentation, slide: usize, cx: &App) -> Self {
        let arc = pres.slides()[slide].clone();
        let fx = Rc::new(arc.show_fx());
        let (width, height) = (
            pres.size.cx as f32 / EMU_PER_PX,
            pres.size.cy as f32 / EMU_PER_PX,
        );
        let (layers, boxes) = if fx.steps.is_empty() {
            let painted = painter.painted(pres, slide, Options::default(), cx);
            (vec![(None, painted)], HashMap::new())
        } else {
            let scene = painter.scene(pres, slide, &Options::default());
            let layers = frames::split_scene(&scene, &arc, &fx)
                .into_iter()
                .map(|l| {
                    let painted = painter.prepare_items(&l.items, scene.background, cx);
                    (l.target, Rc::new(painted))
                })
                .collect();
            (layers, frames::shape_boxes(pres, slide))
        };
        let loading = layers.iter().any(|(_, p)| p.loading);
        BuiltSlide {
            slide: arc,
            fx,
            loading,
            background: layers
                .first()
                .map_or(Rgb([255, 255, 255]), |(_, p)| p.background),
            layers,
            boxes,
            width,
            height,
        }
    }

    /// Still the slide `pres` has at `slide`.
    pub fn is_current(&self, pres: &Presentation, slide: usize) -> bool {
        pres.slides()
            .get(slide)
            .is_some_and(|s| Arc::ptr_eq(s, &self.slide))
    }

    /// The build state after `step` steps, with step `step` `elapsed_ms`
    /// in (0 = not started).
    pub fn state(&self, step: usize, elapsed_ms: f32) -> BuildState {
        if self.fx.steps.is_empty() {
            return BuildState::default();
        }
        frames::build_state(
            &self.fx,
            step,
            elapsed_ms,
            &self.boxes,
            self.width,
            self.height,
        )
    }
}

/// The slide's top left on screen and screen px per slide px, for a
/// `w` × `h` px slide fitted and centred in `bounds`.
fn fit(bounds: Bounds<Pixels>, w: f32, h: f32) -> (Point<Pixels>, f32) {
    let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
    let k = (bw / w.max(1.0)).min(bh / h.max(1.0)).max(0.0);
    let origin = point(
        bounds.origin.x + px((bw - w * k) / 2.0),
        bounds.origin.y + px((bh - h * k) / 2.0),
    );
    (origin, k)
}

fn moved(origin: Point<Pixels>, k: f32, [dx, dy]: [f32; 2]) -> Point<Pixels> {
    point(origin.x + px(dx * k), origin.y + px(dy * k))
}

fn screen_rect(origin: Point<Pixels>, k: f32, [x, y, w, h]: [f32; 4]) -> Bounds<Pixels> {
    Bounds::new(
        moved(origin, k, [x, y]),
        size(px((w * k).max(0.0)), px((h * k).max(0.0))),
    )
}

/// A layer filling its (relative) parent.
fn layer_div(opacity: f32) -> Div {
    let d = div().absolute().top_0().left_0().size_full();
    if opacity < 1.0 { d.opacity(opacity) } else { d }
}

/// `built` with `state` applied, drawn as one slide moved, cut and faded
/// by `layer` (see [`TransitionFrame`]). Fills its parent, which must be
/// `relative()`; the slide is fitted and centred in it.
pub fn slide_element(
    built: &BuiltSlide,
    state: &BuildState,
    layer: LayerFx,
    bridge: &Rc<RefCell<FontBridge>>,
    other: Hsla,
) -> AnyElement {
    let (w, h) = (built.width, built.height);
    let mut root = layer_div(layer.opacity);
    for (target, painted) in &built.layers {
        let look = state.layer(*target);
        if !look.drawn() {
            continue;
        }
        let painted = painted.clone();
        let bridge = bridge.clone();
        let c = canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let (origin, k) = fit(bounds, w, h);
                let frame = screen_rect(origin, k, [0.0, 0.0, w, h]);
                let slide_at = moved(origin, k, layer.offset);
                let mut mask = frame.intersect(&screen_rect(slide_at, k, [0.0, 0.0, w, h]));
                if let Some(c) = layer.clip {
                    mask = mask.intersect(&screen_rect(origin, k, c));
                }
                let shape_at = moved(slide_at, k, look.fx.offset);
                if let Some(c) = look.fx.clip {
                    mask = mask.intersect(&screen_rect(shape_at, k, c));
                }
                if mask.size.width <= px(0.) || mask.size.height <= px(0.) {
                    return;
                }
                window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
                    paint_items(&painted, View::px(shape_at, k), other, &bridge, window)
                });
            },
        )
        .size_full();
        root = root.child(layer_div(look.fx.opacity).child(c));
    }
    root.into_any_element()
}

/// A colour over the slide's frame (fade through black, the black before
/// the first slide).
fn base_element(color: Rgb, w: f32, h: f32) -> AnyElement {
    layer_div(1.0)
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let (origin, k) = fit(bounds, w, h);
                    window.paint_quad(fill(screen_rect(origin, k, [0.0, 0.0, w, h]), hsla(color)));
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// One frame of a transition from `from` (`None`: from black, as the
/// first slide of a show) to `to`, each with its build state. Fills its
/// parent (relative), the slides fitted and centred.
pub fn transition_element(
    from: Option<(&BuiltSlide, &BuildState)>,
    to: (&BuiltSlide, &BuildState),
    frame: &TransitionFrame,
    bridge: &Rc<RefCell<FontBridge>>,
    other: Hsla,
) -> AnyElement {
    let (w, h) = (to.0.width, to.0.height);
    let mut root = div().relative().size_full();
    let base = frame
        .base
        .or_else(|| from.is_none().then_some(Rgb([0, 0, 0])));
    if let Some(c) = base {
        root = root.child(base_element(c, w, h));
    }
    let old = frame
        .from
        .zip(from)
        .map(|(l, (b, s))| slide_element(b, s, l, bridge, other));
    let new = frame
        .to
        .map(|l| slide_element(to.0, to.1, l, bridge, other));
    if frame.to_below {
        root = root.children(new).children(old);
    } else {
        root = root.children(old).children(new);
    }
    root.into_any_element()
}

/// What [`ShowPlayer::tick`] asks of the show.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShowTick {
    /// Nothing to do.
    Idle,
    /// Playing; frames are requested.
    Animating,
    /// The slide's "advance after" time is up: advance as on a click.
    Advance,
    /// Advance by itself in this long: wake the show then (a timer).
    Wait(Duration),
}

#[derive(Clone, Debug)]
enum Playing {
    Transition {
        from: Option<usize>,
        to: usize,
        transition: anim::Transition,
        start: Instant,
        /// Step 0 of `to` starts by itself when the transition ends.
        then_auto: bool,
    },
    Step {
        slide: usize,
        step: usize,
        start: Instant,
        dur_ms: u32,
    },
}

/// The show's clock for transitions and builds.
///
/// The show keeps `slide` and `done` (build steps of the slide shown so
/// far) and calls:
///
/// - advance (click, Space, →): if [`Self::is_animating`], [`Self::finish`]
///   and stop (a click ends an animation at once, as in PowerPoint). Else
///   if `done < player.steps(pres, slide)`: [`Self::start_step`]`(slide,
///   done)` and `done += 1`. Else go to the next slide: `done =
///   player.start_transition(pres, Some(slide), next)` (the build steps
///   that start by themselves, counted as done). A slide whose transition
///   says not to advance on click ([`Self::advances_on_click`]) ignores
///   clicks but not keys.
/// - retreat: [`Self::finish`], then `done -= 1` (shown without
///   animation), or the previous slide with all its steps done.
/// - jump to a slide: [`Self::finish`], `done = 0` (or call
///   `start_transition`, to play its transition).
/// - render: [`Self::tick`] (an `Advance` result is an advance; `Wait`
///   asks for a timer), then [`Self::element`] for the slide area.
pub struct ShowPlayer {
    playing: Option<Playing>,
    /// When the shown slide last stopped animating (auto-advance).
    still_since: Option<Instant>,
    built: HashMap<usize, Rc<BuiltSlide>>,
    fx: HashMap<usize, (Arc<Slide>, Rc<SlideFx>)>,
    /// Colour of objects Kantor can't draw.
    pub other: Hsla,
}

impl Default for ShowPlayer {
    fn default() -> Self {
        Self::new()
    }
}

fn elapsed_ms(start: Instant) -> f32 {
    start.elapsed().as_secs_f32() * 1000.0
}

impl ShowPlayer {
    pub fn new() -> Self {
        ShowPlayer {
            playing: None,
            still_since: Some(Instant::now()),
            built: HashMap::new(),
            fx: HashMap::new(),
            other: hsla(Rgb([128, 128, 128])),
        }
    }

    /// Slide `slide`'s transition and build steps (cached).
    pub fn fx(&mut self, pres: &Presentation, slide: usize) -> Rc<SlideFx> {
        let arc = &pres.slides()[slide];
        if let Some((a, fx)) = self.fx.get(&slide)
            && Arc::ptr_eq(a, arc)
        {
            return fx.clone();
        }
        let fx = Rc::new(arc.show_fx());
        self.fx.insert(slide, (arc.clone(), fx.clone()));
        fx
    }

    /// How many click steps slide `slide` has.
    pub fn steps(&mut self, pres: &Presentation, slide: usize) -> usize {
        self.fx(pres, slide).steps.len()
    }

    /// Whether slide `slide`'s first build step starts by itself (after
    /// its transition): it counts as done as soon as the slide is on.
    pub fn starts_by_itself(&mut self, pres: &Presentation, slide: usize) -> bool {
        self.fx(pres, slide).steps.first().is_some_and(|s| s.auto)
    }

    /// Whether a click on slide `slide` advances (`advClick`).
    pub fn advances_on_click(&mut self, pres: &Presentation, slide: usize) -> bool {
        self.fx(pres, slide)
            .transition
            .as_ref()
            .is_none_or(|t| t.advance_on_click)
    }

    /// Play `to`'s transition from `from` (`None`: from black). Returns
    /// how many of `to`'s build steps the show should count as done: 1
    /// when its first step starts by itself (it plays after the
    /// transition), else 0.
    pub fn start_transition(
        &mut self,
        pres: &Presentation,
        from: Option<usize>,
        to: usize,
    ) -> usize {
        let fx = self.fx(pres, to);
        let auto = fx.steps.first().is_some_and(|s| s.auto);
        let now = Instant::now();
        self.playing = match fx.transition.clone() {
            Some(transition) if transition.duration_ms > 0 => Some(Playing::Transition {
                from,
                to,
                transition,
                start: now,
                then_auto: auto,
            }),
            _ if auto => Some(Playing::Step {
                slide: to,
                step: 0,
                start: now,
                dur_ms: fx.steps[0].duration_ms(),
            }),
            _ => None,
        };
        self.still_since = self.playing.is_none().then_some(now);
        usize::from(auto)
    }

    /// Play build step `step` of slide `slide`.
    pub fn start_step(&mut self, pres: &Presentation, slide: usize, step: usize) {
        let fx = self.fx(pres, slide);
        let Some(s) = fx.steps.get(step) else {
            return;
        };
        self.playing = Some(Playing::Step {
            slide,
            step,
            start: Instant::now(),
            dur_ms: s.duration_ms(),
        });
        self.still_since = None;
    }

    pub fn is_animating(&self) -> bool {
        self.playing.is_some()
    }

    /// End what's playing at once (its end state shows).
    pub fn finish(&mut self) {
        self.playing = None;
        self.still_since = Some(Instant::now());
    }

    /// Move the clock on: ends finished animations, starts a step that
    /// follows a transition by itself, and tells when slide `slide`
    /// advances by itself. Call once per frame, before [`Self::element`].
    pub fn tick(&mut self, pres: &Presentation, slide: usize) -> ShowTick {
        let now = Instant::now();
        if let Some(Playing::Transition {
            to,
            transition,
            start,
            then_auto,
            ..
        }) = &self.playing
            && elapsed_ms(*start) >= transition.duration_ms as f32
        {
            let (to, then_auto) = (*to, *then_auto);
            self.playing = None;
            if then_auto {
                let dur_ms = self
                    .fx(pres, to)
                    .steps
                    .first()
                    .map_or(0, anim::BuildStep::duration_ms);
                self.playing = Some(Playing::Step {
                    slide: to,
                    step: 0,
                    start: now,
                    dur_ms,
                });
            }
        }
        if let Some(Playing::Step { start, dur_ms, .. }) = &self.playing
            && elapsed_ms(*start) >= *dur_ms as f32
        {
            self.playing = None;
        }
        if self.playing.is_some() {
            return ShowTick::Animating;
        }
        let since = *self.still_since.get_or_insert(now);
        let Some(after) = self
            .fx(pres, slide)
            .transition
            .as_ref()
            .and_then(|t| t.advance_after_ms)
        else {
            return ShowTick::Idle;
        };
        let after = Duration::from_millis(after as u64);
        let gone = now.duration_since(since);
        if gone >= after {
            self.still_since = None;
            ShowTick::Advance
        } else {
            ShowTick::Wait(after - gone)
        }
    }

    fn built(
        &mut self,
        painter: &Painter,
        pres: &Presentation,
        slide: usize,
        cx: &App,
    ) -> Rc<BuiltSlide> {
        if let Some(b) = self.built.get(&slide)
            && b.is_current(pres, slide)
        {
            return b.clone();
        }
        let b = Rc::new(BuiltSlide::new(painter, pres, slide, cx));
        if !b.loading {
            if self.built.len() > 16 {
                self.built.clear();
            }
            self.built.insert(slide, b.clone());
        }
        b
    }

    /// Drop prepared slides (the presentation changed).
    pub fn clear(&mut self) {
        self.built.clear();
        self.fx.clear();
    }

    /// Slide `slide` with `done` build steps shown, never animated (the
    /// presenter view's slides). Fills its parent like [`Self::element`].
    pub fn still_element(
        &mut self,
        painter: &Painter,
        pres: &Presentation,
        slide: usize,
        done: usize,
        cx: &App,
    ) -> AnyElement {
        let b = self.built(painter, pres, slide, cx);
        let state = b.state(done, 0.0);
        div()
            .relative()
            .size_full()
            .child(slide_element(
                &b,
                &state,
                LayerFx::FULL,
                &painter.bridge,
                self.other,
            ))
            .into_any_element()
    }

    /// The slide area: slide `slide` with `done` build steps shown, or
    /// what's playing. Fills its parent; requests the next frame while
    /// animating.
    pub fn element(
        &mut self,
        painter: &Painter,
        pres: &Presentation,
        slide: usize,
        done: usize,
        window: &mut Window,
        cx: &App,
    ) -> AnyElement {
        let bridge = painter.bridge.clone();
        let other = self.other;
        if self.playing.is_some() {
            window.request_animation_frame();
        }
        match self.playing.clone() {
            Some(Playing::Transition {
                from,
                to,
                transition,
                start,
                ..
            }) if to == slide => {
                let new = self.built(painter, pres, to, cx);
                let old = from
                    .filter(|&f| f < pres.len())
                    .map(|f| self.built(painter, pres, f, cx));
                let p = elapsed_ms(start) / transition.duration_ms.max(1) as f32;
                let frame = frames::transition_frame(&transition, p, new.width, new.height);
                let new_state = new.state(0, 0.0);
                let old_state = old.as_ref().map(|o| o.state(o.fx.steps.len(), 0.0));
                let old_pair = old.as_deref().zip(old_state.as_ref());
                div()
                    .relative()
                    .size_full()
                    .child(transition_element(
                        old_pair,
                        (&*new, &new_state),
                        &frame,
                        &bridge,
                        other,
                    ))
                    .into_any_element()
            }
            playing => {
                let b = self.built(painter, pres, slide, cx);
                let state = match playing {
                    Some(Playing::Step {
                        slide: s,
                        step,
                        start,
                        ..
                    }) if s == slide => b.state(step, elapsed_ms(start).max(f32::MIN_POSITIVE)),
                    _ => b.state(done, 0.0),
                };
                div()
                    .relative()
                    .size_full()
                    .child(slide_element(&b, &state, LayerFx::FULL, &bridge, other))
                    .into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn fit_centres_the_slide() {
        let b = Bounds::new(point(px(10.), px(0.)), size(px(200.), px(50.)));
        let (origin, k) = fit(b, 100.0, 50.0);
        assert_eq!(k, 1.0);
        assert_eq!(origin, point(px(60.), px(0.)));
    }
}
