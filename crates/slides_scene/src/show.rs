//! Slide show frames (no GPUI): where the old and the new slide are drawn
//! during a transition ([`transition_frame`]) and how each animated shape
//! looks at a moment of a build step ([`build_state`]), as offsets, clip
//! rectangles and opacities in slide px. [`split_scene`] cuts a slide's
//! scene into layers so that animated shapes (or paragraphs) can be drawn
//! apart, in their places in the stacking order.
//!
//! Nothing here scales or turns: GPUI can't transform glyphs or pictures
//! at paint time, so zooms are drawn as a box opening from the centre.

use std::collections::HashMap;
use std::ops::Range;

use slides_engine::anim::{EffectClass, Preset, SlideFx, Target, Transition, TransitionKind};
use slides_engine::{Layer, Presentation, Rgb, Slide};

use crate::build::px;
use crate::{Item, Prim, SlideScene, Text};

/// How a whole slide (or one shape) is drawn: moved by `offset`, cut to
/// `clip` (`[x, y, w, h]`, slide px), faded to `opacity`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerFx {
    pub offset: [f32; 2],
    pub clip: Option<[f32; 4]>,
    pub opacity: f32,
}

impl LayerFx {
    pub const FULL: LayerFx = LayerFx {
        offset: [0.0, 0.0],
        clip: None,
        opacity: 1.0,
    };

    fn opacity(opacity: f32) -> LayerFx {
        LayerFx {
            opacity: opacity.clamp(0.0, 1.0),
            ..LayerFx::FULL
        }
    }

    fn offset(x: f32, y: f32) -> LayerFx {
        LayerFx {
            offset: [x, y],
            ..LayerFx::FULL
        }
    }

    fn clip(r: [f32; 4]) -> LayerFx {
        LayerFx {
            clip: Some(r),
            ..LayerFx::FULL
        }
    }
}

/// One frame of a transition. Paint `base` (if any) over the whole slide,
/// then the two slides, `from` first unless `to_below`. A `None` slide
/// isn't drawn. Clips of slides are fixed to the slide's frame on screen
/// (they don't move with the offset).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransitionFrame {
    pub base: Option<Rgb>,
    pub from: Option<LayerFx>,
    pub to: Option<LayerFx>,
    /// The new slide is drawn under the old one (uncover, closing boxes).
    pub to_below: bool,
}

fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ease_out(t: f32) -> f32 {
    let t = 1.0 - t.clamp(0.0, 1.0);
    1.0 - t * t * t
}

fn ease_in(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * t
}

/// A centred `[x, y, w, h]` box `k` times the size of `r`.
fn centred(r: [f32; 4], kx: f32, ky: f32) -> [f32; 4] {
    let [x, y, w, h] = r;
    let (nw, nh) = (w * kx, h * ky);
    [x + (w - nw) / 2.0, y + (h - nh) / 2.0, nw, nh]
}

/// The part of `r` an edge moving in `(dx, dy)` has swept over after
/// `t` (0..1) of the way: the edge starts on the side it moves away from.
fn swept(r: [f32; 4], (dx, dy): (f32, f32), t: f32) -> [f32; 4] {
    let [mut x, mut y, mut w, mut h] = r;
    if dx != 0.0 {
        let nw = w * t;
        if dx < 0.0 {
            x += w - nw;
        }
        w = nw;
    }
    if dy != 0.0 {
        let nh = h * t;
        if dy < 0.0 {
            y += h - nh;
        }
        h = nh;
    }
    [x, y, w, h]
}

/// Where the slides are `progress` (0..1) of the way through `t`, on a
/// `w` × `h` px slide.
pub fn transition_frame(t: &Transition, progress: f32, w: f32, h: f32) -> TransitionFrame {
    let p = progress.clamp(0.0, 1.0);
    let full = [0.0, 0.0, w, h];
    let frame = |from: Option<LayerFx>, to: Option<LayerFx>, to_below: bool| TransitionFrame {
        base: None,
        from,
        to,
        to_below,
    };
    if p >= 1.0 {
        return frame(None, Some(LayerFx::FULL), false);
    }
    if let Some(c) = t.through
        && matches!(t.kind, TransitionKind::Fade | TransitionKind::Cut)
    {
        // Out to the colour in the first half, in from it in the second.
        let cut = t.kind == TransitionKind::Cut;
        let (from, to) = if p < 0.5 {
            let o = if cut { 0.0 } else { 1.0 - 2.0 * p };
            (Some(LayerFx::opacity(o)), None)
        } else {
            let o = if cut { 1.0 } else { 2.0 * p - 1.0 };
            (None, Some(LayerFx::opacity(o)))
        };
        return TransitionFrame {
            base: Some(c),
            from,
            to,
            to_below: false,
        };
    }
    let s = ease_in_out(p);
    match t.kind {
        TransitionKind::Cut => frame(None, Some(LayerFx::FULL), false),
        TransitionKind::Fade => frame(Some(LayerFx::FULL), Some(LayerFx::opacity(p)), false),
        TransitionKind::Push(d) => {
            let (dx, dy) = d.vector();
            frame(
                Some(LayerFx::offset(dx * w * s, dy * h * s)),
                Some(LayerFx::offset(-dx * w * (1.0 - s), -dy * h * (1.0 - s))),
                false,
            )
        }
        TransitionKind::Cover(d) => {
            let (dx, dy) = d.vector();
            frame(
                Some(LayerFx::FULL),
                Some(LayerFx::offset(-dx * w * (1.0 - s), -dy * h * (1.0 - s))),
                false,
            )
        }
        TransitionKind::Uncover(d) => {
            let (dx, dy) = d.vector();
            frame(
                Some(LayerFx::offset(dx * w * s, dy * h * s)),
                Some(LayerFx::FULL),
                true,
            )
        }
        TransitionKind::Wipe(d) => frame(
            Some(LayerFx::FULL),
            Some(LayerFx::clip(swept(full, d.vector(), p))),
            false,
        ),
        TransitionKind::Split { vertical, out } => {
            let k = if out { p } else { 1.0 - p };
            let band = if vertical {
                centred(full, k, 1.0)
            } else {
                centred(full, 1.0, k)
            };
            if out {
                frame(Some(LayerFx::FULL), Some(LayerFx::clip(band)), false)
            } else {
                frame(Some(LayerFx::clip(band)), Some(LayerFx::FULL), true)
            }
        }
        TransitionKind::Box { out } => {
            if out {
                frame(
                    Some(LayerFx::FULL),
                    Some(LayerFx::clip(centred(full, s, s))),
                    false,
                )
            } else {
                let k = 1.0 - s;
                frame(
                    Some(LayerFx::clip(centred(full, k, k))),
                    Some(LayerFx::FULL),
                    true,
                )
            }
        }
    }
}

/// How one animated target looks: drawn or not, and if drawn with which
/// offset (slide px), clip (`[x, y, w, h]` in slide px, moving with the
/// offset) and opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeFx {
    pub visible: bool,
    pub fx: LayerFx,
}

impl ShapeFx {
    pub const SHOWN: ShapeFx = ShapeFx {
        visible: true,
        fx: LayerFx::FULL,
    };
    pub const HIDDEN: ShapeFx = ShapeFx {
        visible: false,
        fx: LayerFx::FULL,
    };

    /// Both at once (a paragraph inside an animated shape).
    pub fn and(self, o: ShapeFx) -> ShapeFx {
        let clip = match (self.fx.clip, o.fx.clip) {
            (Some(a), Some(b)) => Some(intersect(a, b)),
            (a, b) => a.or(b),
        };
        ShapeFx {
            visible: self.visible && o.visible,
            fx: LayerFx {
                offset: [
                    self.fx.offset[0] + o.fx.offset[0],
                    self.fx.offset[1] + o.fx.offset[1],
                ],
                clip,
                opacity: self.fx.opacity * o.fx.opacity,
            },
        }
    }

    /// Drawn at all (visible and not fully faded or clipped away).
    pub fn drawn(&self) -> bool {
        self.visible
            && self.fx.opacity > 0.0
            && self.fx.clip.is_none_or(|[_, _, w, h]| w > 0.0 && h > 0.0)
    }
}

/// `[x, y, w, h]` ∩ `[x, y, w, h]` (empty = zero size).
pub fn intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let x0 = a[0].max(b[0]);
    let y0 = a[1].max(b[1]);
    let x1 = (a[0] + a[2]).min(b[0] + b[2]);
    let y1 = (a[1] + a[3]).min(b[1] + b[3]);
    [x0, y0, (x1 - x0).max(0.0), (y1 - y0).max(0.0)]
}

/// The look of every animated target at one moment of a slide's builds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildState {
    pub targets: HashMap<Target, ShapeFx>,
}

impl BuildState {
    /// A target's look (shown if it isn't animated).
    pub fn get(&self, t: Target) -> ShapeFx {
        self.targets.get(&t).copied().unwrap_or(ShapeFx::SHOWN)
    }

    /// The look of a layer from [`split_scene`]: a part of a shape also
    /// takes the whole shape's look.
    pub fn layer(&self, t: Option<Target>) -> ShapeFx {
        match t {
            None => ShapeFx::SHOWN,
            Some(Target::Shape(id)) => self.get(Target::Shape(id)),
            Some(part) => self.get(Target::Shape(part.shape_id())).and(self.get(part)),
        }
    }
}

/// Each slide shape's box (`[x, y, w, h]` slide px, around a turned
/// shape) by shape id.
pub fn shape_boxes(pres: &Presentation, slide: usize) -> HashMap<u32, [f32; 4]> {
    let shapes = &pres.slide(slide).shapes;
    pres.render_list(slide, false)
        .iter()
        .filter_map(|r| {
            let Layer::Slide(i) = r.layer else {
                return None;
            };
            let b = r.xfrm.bounding_box();
            Some((shapes.get(i)?.id, [px(b.x), px(b.y), px(b.cx), px(b.cy)]))
        })
        .collect()
}

/// An effect `t` (0..1) of the way through, on a shape in `b` on a
/// `w` × `h` slide.
fn effect_fx(class: EffectClass, preset: Preset, t: f32, b: [f32; 4], w: f32, h: f32) -> ShapeFx {
    let entrance = class == EffectClass::Entrance;
    // How far in: 0 = not there, 1 = all there.
    let k = if entrance { t } else { 1.0 - t };
    let shown = |fx: LayerFx| ShapeFx { visible: true, fx };
    match preset {
        Preset::None => ShapeFx::SHOWN,
        Preset::Appear => {
            if entrance == (t > 0.0) {
                ShapeFx::SHOWN
            } else {
                ShapeFx::HIDDEN
            }
        }
        Preset::Fade => shown(LayerFx::opacity(k)),
        Preset::Fly(d) => {
            let (dx, dy) = d.vector();
            let [x, y, bw, bh] = b;
            // Just past the slide's edge: behind the motion for an
            // entrance, ahead of it for an exit.
            let ahead = |v: f32, lo: f32, size: f32, extent: f32| {
                if v > 0.0 {
                    extent - lo
                } else if v < 0.0 {
                    -(lo + size)
                } else {
                    0.0
                }
            };
            let (ox, oy) = if entrance {
                let far = 1.0 - ease_out(t);
                (ahead(-dx, x, bw, w) * far, ahead(-dy, y, bh, h) * far)
            } else {
                let far = ease_in(t);
                (ahead(dx, x, bw, w) * far, ahead(dy, y, bh, h) * far)
            };
            shown(LayerFx::offset(ox, oy))
        }
        Preset::Wipe(d) => {
            let r = if entrance {
                swept(b, d.vector(), t)
            } else {
                // Erased from the side the edge leaves: what's left is on
                // the side it moves to.
                let (dx, dy) = d.vector();
                swept(b, (-dx, -dy), 1.0 - t)
            };
            shown(LayerFx::clip(r))
        }
        Preset::Split { vertical, .. } => {
            let band = if vertical {
                centred(b, k, 1.0)
            } else {
                centred(b, 1.0, k)
            };
            shown(LayerFx::clip(band))
        }
        Preset::Zoom => {
            let s = ease_out(k);
            shown(LayerFx {
                clip: Some(centred(b, s, s)),
                opacity: k,
                ..LayerFx::FULL
            })
        }
        Preset::Float(d) => {
            let (dx, dy) = d.vector();
            let dist = h * 0.1 * (1.0 - ease_out(k));
            let sign = if entrance { -1.0 } else { 1.0 };
            shown(LayerFx {
                offset: [sign * dx * dist, sign * dy * dist],
                opacity: k,
                ..LayerFx::FULL
            })
        }
    }
}

/// The look of every animated target after the first `step` build steps
/// have played and step `step` (if any) has run for `elapsed_ms`.
/// `boxes` are the shapes' boxes ([`shape_boxes`]), `w` × `h` the slide.
pub fn build_state(
    fx: &SlideFx,
    step: usize,
    elapsed_ms: f32,
    boxes: &HashMap<u32, [f32; 4]>,
    w: f32,
    h: f32,
) -> BuildState {
    let mut targets: HashMap<Target, ShapeFx> = HashMap::new();
    for t in fx.hidden_at_start() {
        targets.insert(t, ShapeFx::HIDDEN);
    }
    for (i, s) in fx.steps.iter().enumerate().take(step + 1) {
        for e in &s.effects {
            if !matches!(e.class, EffectClass::Entrance | EffectClass::Exit) {
                continue;
            }
            let t = if i < step {
                1.0
            } else {
                let into = elapsed_ms - e.start_ms as f32;
                if elapsed_ms <= 0.0 || into < 0.0 {
                    continue;
                }
                if e.dur_ms == 0 {
                    1.0
                } else {
                    (into / e.dur_ms as f32).min(1.0)
                }
            };
            let look = if t >= 1.0 {
                if e.class == EffectClass::Entrance {
                    ShapeFx::SHOWN
                } else {
                    ShapeFx::HIDDEN
                }
            } else {
                let b = boxes
                    .get(&e.target.shape_id())
                    .copied()
                    .unwrap_or([0.0, 0.0, w, h]);
                effect_fx(e.class, e.preset, t, b, w, h)
            };
            targets.insert(e.target, look);
        }
    }
    BuildState { targets }
}

/// A run of scene items drawn together: still ones (`target` `None`), or
/// one animated shape, shape background or paragraph.
#[derive(Clone, Debug)]
pub struct SceneLayer {
    pub target: Option<Target>,
    pub items: Vec<Item>,
}

/// The paragraphs a text item's runs cover, in order.
fn paras(t: &Text) -> Vec<usize> {
    let mut out: Vec<usize> = Vec::new();
    for r in &t.runs {
        if !out.contains(&r.run.para) {
            out.push(r.run.para);
        }
    }
    out
}

/// Cut a slide's scene into layers, bottom to top: shapes `fx` animates
/// get their own layers (their text by paragraph if they're built by
/// paragraph, the rest of the shape as [`Target::Background`]); everything
/// else is merged into still layers. With nothing animated the scene is
/// one layer.
pub fn split_scene(scene: &SlideScene, slide: &Slide, fx: &SlideFx) -> Vec<SceneLayer> {
    let mut out: Vec<SceneLayer> = Vec::new();
    let mut push = |target: Option<Target>, item: Item| match out.last_mut() {
        Some(l) if l.target == target => l.items.push(item),
        _ => out.push(SceneLayer {
            target,
            items: vec![item],
        }),
    };
    for item in &scene.items {
        let id = match item.layer {
            Some(Layer::Slide(i)) => slide.shapes.get(i).map(|s| s.id),
            _ => None,
        };
        let Some(id) = id.filter(|&id| fx.animates(id)) else {
            push(None, item.clone());
            continue;
        };
        if !fx.by_paragraph(id) {
            push(Some(Target::Shape(id)), item.clone());
            continue;
        }
        match &item.prim {
            Prim::Text(t) => {
                for p in paras(t) {
                    let runs = t.runs.iter().filter(|r| r.run.para == p).cloned().collect();
                    push(
                        Some(Target::Para(id, p as u32)),
                        Item {
                            layer: item.layer,
                            transform: item.transform,
                            prim: Prim::Text(Text {
                                origin: t.origin,
                                runs,
                            }),
                        },
                    );
                }
            }
            _ => push(Some(Target::Background(id)), item.clone()),
        }
    }
    out
}

/// Steps `range` of `fx` played back to back: how long they take (ms).
pub fn steps_duration(fx: &SlideFx, range: Range<usize>) -> u32 {
    fx.steps
        .get(range)
        .map_or(0, |s| s.iter().map(|s| s.duration_ms()).sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use slides_engine::anim::{BuildStep, Dir, Effect, Trigger};

    fn tr(kind: TransitionKind) -> Transition {
        Transition {
            kind,
            ..Transition::none()
        }
    }

    #[test]
    fn push_moves_both_slides() {
        let f = transition_frame(&tr(TransitionKind::Push(Dir::Left)), 0.5, 100.0, 50.0);
        assert_eq!(f.from.unwrap().offset, [-50.0, 0.0]);
        assert_eq!(f.to.unwrap().offset, [50.0, 0.0]);
        let f = transition_frame(&tr(TransitionKind::Push(Dir::Up)), 0.0, 100.0, 50.0);
        assert_eq!(f.from.unwrap().offset, [0.0, 0.0]);
        assert_eq!(f.to.unwrap().offset, [0.0, 50.0]);
        let end = transition_frame(&tr(TransitionKind::Push(Dir::Up)), 1.0, 100.0, 50.0);
        assert_eq!(end.from, None);
        assert_eq!(end.to, Some(LayerFx::FULL));
    }

    #[test]
    fn wipes_splits_and_boxes_clip() {
        let f = transition_frame(&tr(TransitionKind::Wipe(Dir::Left)), 0.25, 100.0, 40.0);
        assert_eq!(f.to.unwrap().clip, Some([75.0, 0.0, 25.0, 40.0]));
        let f = transition_frame(&tr(TransitionKind::Wipe(Dir::Down)), 0.25, 100.0, 40.0);
        assert_eq!(f.to.unwrap().clip, Some([0.0, 0.0, 100.0, 10.0]));
        let split = TransitionKind::Split {
            vertical: true,
            out: true,
        };
        let f = transition_frame(&tr(split), 0.5, 100.0, 40.0);
        assert_eq!(f.to.unwrap().clip, Some([25.0, 0.0, 50.0, 40.0]));
        let f = transition_frame(&tr(TransitionKind::Box { out: false }), 0.5, 100.0, 40.0);
        assert!(f.to_below);
        assert_eq!(f.from.unwrap().clip, Some([25.0, 10.0, 50.0, 20.0]));
    }

    #[test]
    fn fade_through_black() {
        let t = Transition {
            through: Some(Rgb([0, 0, 0])),
            ..tr(TransitionKind::Fade)
        };
        let f = transition_frame(&t, 0.25, 10.0, 10.0);
        assert_eq!(f.base, Some(Rgb([0, 0, 0])));
        assert_eq!(f.from.unwrap().opacity, 0.5);
        assert_eq!(f.to, None);
        let f = transition_frame(&t, 0.75, 10.0, 10.0);
        assert_eq!((f.from, f.to.unwrap().opacity), (None, 0.5));
        let f = transition_frame(&tr(TransitionKind::Fade), 0.25, 10.0, 10.0);
        assert_eq!(f.to.unwrap().opacity, 0.25);
        assert_eq!(f.from, Some(LayerFx::FULL));
    }

    fn effect(target: Target, class: EffectClass, preset: Preset, start: u32, dur: u32) -> Effect {
        Effect {
            target,
            class,
            preset,
            preset_id: 0,
            preset_subtype: 0,
            trigger: Trigger::OnClick,
            start_ms: start,
            dur_ms: dur,
        }
    }

    #[test]
    fn builds_hide_show_and_move() {
        let a = Target::Shape(2);
        let b = Target::Shape(3);
        let fx = SlideFx {
            transition: None,
            steps: vec![
                BuildStep {
                    auto: false,
                    effects: vec![
                        effect(a, EffectClass::Entrance, Preset::Fade, 0, 1000),
                        effect(b, EffectClass::Entrance, Preset::Fly(Dir::Up), 1000, 500),
                    ],
                },
                BuildStep {
                    auto: false,
                    effects: vec![effect(a, EffectClass::Exit, Preset::Appear, 0, 0)],
                },
            ],
        };
        let boxes = HashMap::from([(2, [10.0, 10.0, 20.0, 20.0]), (3, [10.0, 60.0, 20.0, 20.0])]);
        let at = |step, ms| build_state(&fx, step, ms, &boxes, 100.0, 100.0);
        // Before the first click both are hidden.
        let s = at(0, 0.0);
        assert!(!s.get(a).visible && !s.get(b).visible);
        // Half way through the fade.
        let s = at(0, 500.0);
        assert_eq!(s.get(a).fx.opacity, 0.5);
        assert!(!s.get(b).visible);
        // The fly starts below the slide: offset = 100 - 60.
        let s = at(0, 1000.0);
        assert_eq!(s.get(a), ShapeFx::SHOWN);
        assert_eq!(s.get(b).fx.offset, [0.0, 40.0]);
        // After step 1, before step 2: both shown; after step 2, a is gone.
        let s = at(1, 0.0);
        assert!(s.get(a).visible && s.get(b).visible);
        let s = at(2, 0.0);
        assert!(!s.get(a).visible && s.get(b).visible);
        assert_eq!(steps_duration(&fx, 0..2), 1500);
    }

    #[test]
    fn fly_out_and_wipe_out() {
        let b = [10.0, 10.0, 20.0, 20.0];
        let f = effect_fx(
            EffectClass::Exit,
            Preset::Fly(Dir::Right),
            1.0 - 1e-6,
            b,
            100.0,
            100.0,
        );
        assert!((f.fx.offset[0] - 90.0).abs() < 0.01, "{:?}", f.fx.offset);
        let f = effect_fx(
            EffectClass::Entrance,
            Preset::Fly(Dir::Right),
            0.0,
            b,
            100.0,
            100.0,
        );
        assert_eq!(f.fx.offset, [-30.0, 0.0]);
        // Wipe out moving up: the top part is what's left.
        let f = effect_fx(
            EffectClass::Exit,
            Preset::Wipe(Dir::Up),
            0.25,
            b,
            100.0,
            100.0,
        );
        assert_eq!(f.fx.clip, Some([10.0, 10.0, 20.0, 15.0]));
        let f = effect_fx(
            EffectClass::Entrance,
            Preset::Wipe(Dir::Up),
            0.25,
            b,
            100.0,
            100.0,
        );
        assert_eq!(f.fx.clip, Some([10.0, 25.0, 20.0, 5.0]));
    }

    #[test]
    fn parts_take_the_whole_shapes_look() {
        let mut s = BuildState::default();
        s.targets.insert(
            Target::Shape(4),
            ShapeFx {
                visible: true,
                fx: LayerFx::opacity(0.5),
            },
        );
        s.targets.insert(Target::Para(4, 1), ShapeFx::HIDDEN);
        assert_eq!(s.layer(Some(Target::Para(4, 0))).fx.opacity, 0.5);
        assert!(!s.layer(Some(Target::Para(4, 1))).visible);
        assert_eq!(s.layer(None), ShapeFx::SHOWN);
    }
}
