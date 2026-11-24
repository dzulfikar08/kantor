//! Slide show effects read from a slide (read only): its transition
//! (`p:transition`, including PowerPoint 2010+ transitions in
//! `mc:AlternateContent`) and its build steps (the main sequence of
//! `p:timing`). Saving never looks at this model: the XML is kept as read
//! (see [`crate::presentation`]).
//!
//! Directions ([`Dir`]) are always the direction things move on screen:
//! `Push(Dir::Up)` is PowerPoint's "Push From Bottom" (`<p:push dir="u"/>`),
//! a "Fly In From Left" entrance is `Fly(Dir::Right)`.
//!
//! What the show can't draw is mapped to something close
//! ([`TransitionKind`], [`Preset`]): unknown transitions fade, unknown
//! entrances fade, unknown exits disappear. Emphasis effects and motion
//! paths are listed but change nothing; animations started by a trigger
//! (interactive sequences), sounds and media commands are left out.

use roxmltree::Node;

use crate::Rgb;
use crate::presentation::Slide;
use crate::xml::{NS_P, attr_bool, child, children, is};

const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const NS_P14: &str = "http://schemas.microsoft.com/office/powerpoint/2010/main";
const NS_P15: &str = "http://schemas.microsoft.com/office/powerpoint/2012/main";

/// A direction of motion on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
    LeftUp,
    RightUp,
    LeftDown,
    RightDown,
}

impl Dir {
    /// The motion as a unit step in x and y (y grows downwards).
    pub fn vector(self) -> (f32, f32) {
        match self {
            Dir::Left => (-1.0, 0.0),
            Dir::Right => (1.0, 0.0),
            Dir::Up => (0.0, -1.0),
            Dir::Down => (0.0, 1.0),
            Dir::LeftUp => (-1.0, -1.0),
            Dir::RightUp => (1.0, -1.0),
            Dir::LeftDown => (-1.0, 1.0),
            Dir::RightDown => (1.0, 1.0),
        }
    }

    pub fn reverse(self) -> Dir {
        match self {
            Dir::Left => Dir::Right,
            Dir::Right => Dir::Left,
            Dir::Up => Dir::Down,
            Dir::Down => Dir::Up,
            Dir::LeftUp => Dir::RightDown,
            Dir::RightUp => Dir::LeftDown,
            Dir::LeftDown => Dir::RightUp,
            Dir::RightDown => Dir::LeftUp,
        }
    }

    /// `ST_TransitionEightDirectionType` / side direction (`l`, `u`, `rd`...).
    fn from_ooxml(v: &str) -> Option<Dir> {
        Some(match v {
            "l" => Dir::Left,
            "r" => Dir::Right,
            "u" => Dir::Up,
            "d" => Dir::Down,
            "lu" => Dir::LeftUp,
            "ru" => Dir::RightUp,
            "ld" => Dir::LeftDown,
            "rd" => Dir::RightDown,
            _ => return None,
        })
    }

    /// A side as PowerPoint's `presetSubtype` bits (1 top, 2 right,
    /// 4 bottom, 8 left): the direction pointing to that side.
    fn side_bits(bits: u32) -> Option<Dir> {
        Some(match bits {
            1 => Dir::Up,
            2 => Dir::Right,
            4 => Dir::Down,
            8 => Dir::Left,
            3 => Dir::RightUp,
            6 => Dir::RightDown,
            9 => Dir::LeftUp,
            12 => Dir::LeftDown,
            _ => return None,
        })
    }
}

/// How one slide replaces the previous one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TransitionKind {
    /// At once.
    Cut,
    /// The new slide fades in over the old one.
    Fade,
    /// Both slides move by one slide width / height.
    Push(Dir),
    /// The new slide is revealed by an edge moving in `Dir`.
    Wipe(Dir),
    /// A band through the middle opens out (`out`) or closes in.
    /// `vertical`: the band is upright and grows sideways.
    Split { vertical: bool, out: bool },
    /// The new slide slides in over the old one.
    Cover(Dir),
    /// The old slide slides away, showing the new one under it.
    Uncover(Dir),
    /// A box from the centre: the new slide grows out of it (`out`), or
    /// the old one shrinks into it.
    Box { out: bool },
}

/// A slide's transition (`p:transition`).
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub kind: TransitionKind,
    /// Through a colour: black for `thruBlk`, white for a flash.
    pub through: Option<Rgb>,
    pub duration_ms: u32,
    /// A click moves on (`advClick`, default true).
    pub advance_on_click: bool,
    /// Moves on by itself this long after the slide's animations (`advTm`).
    pub advance_after_ms: Option<u32>,
    /// The element read (`fade`, `p14:vortex`, `p15:prstTrans curtains`),
    /// to tell what was approximated.
    pub source: String,
}

impl Transition {
    /// No transition: a cut, advanced by click.
    pub fn none() -> Self {
        Transition {
            kind: TransitionKind::Cut,
            through: None,
            duration_ms: 0,
            advance_on_click: true,
            advance_after_ms: None,
            source: String::new(),
        }
    }
}

/// What a build effect does to its shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EffectClass {
    Entrance,
    Exit,
    Emphasis,
    /// Motion path (not drawn: the shape stays put).
    Path,
    Other,
}

/// How an effect animates (drawn the same way in and out).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    /// Appears / disappears at once.
    Appear,
    Fade,
    /// Flies in from / out to beyond the slide edge, moving in `Dir`.
    Fly(Dir),
    /// An edge moving in `Dir` reveals (entrance) or erases (exit) it.
    Wipe(Dir),
    Split {
        vertical: bool,
        out: bool,
    },
    /// Zoom, grow, box, circle...: a box from the centre and a fade.
    Zoom,
    /// Floats a little way in `Dir` while fading.
    Float(Dir),
    /// Emphasis, motion paths and the like: drawn as is.
    None,
}

/// What an effect animates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Target {
    /// A whole slide shape, by shape id (`p:cNvPr/@id`).
    Shape(u32),
    /// Only the shape's own drawing, not its text (`p:bg` in `p:spTgt`).
    Background(u32),
    /// One paragraph of the shape's text, from 0 (`p:txEl/p:pRg`).
    Para(u32, u32),
}

impl Target {
    pub fn shape_id(self) -> u32 {
        match self {
            Target::Shape(s) | Target::Background(s) | Target::Para(s, _) => s,
        }
    }
}

/// How an effect was started in PowerPoint's animation pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    OnClick,
    WithPrevious,
    AfterPrevious,
}

/// One effect of a build step.
#[derive(Clone, Debug, PartialEq)]
pub struct Effect {
    pub target: Target,
    pub class: EffectClass,
    pub preset: Preset,
    /// PowerPoint's `presetID` / `presetSubtype` (0 if none).
    pub preset_id: u32,
    pub preset_subtype: u32,
    pub trigger: Trigger,
    /// Start, from the start of its step.
    pub start_ms: u32,
    pub dur_ms: u32,
}

impl Effect {
    pub fn end_ms(&self) -> u32 {
        self.start_ms + self.dur_ms
    }
}

/// What one click plays.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BuildStep {
    /// Starts by itself (after the transition) instead of on a click: a
    /// slide whose first effect is "With / After Previous".
    pub auto: bool,
    /// By start time.
    pub effects: Vec<Effect>,
}

impl BuildStep {
    pub fn duration_ms(&self) -> u32 {
        self.effects.iter().map(Effect::end_ms).max().unwrap_or(0)
    }
}

/// A slide's transition and build steps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlideFx {
    pub transition: Option<Transition>,
    pub steps: Vec<BuildStep>,
}

impl SlideFx {
    /// Read from a slide's XML (`p:sld`). Bad XML reads as no effects.
    pub fn from_xml(xml: &str) -> SlideFx {
        let Ok(doc) = roxmltree::Document::parse(xml) else {
            return SlideFx::default();
        };
        let root = doc.root_element();
        SlideFx {
            transition: find_alternate(root, "transition").and_then(read_transition),
            steps: find_alternate(root, "timing").map_or_else(Vec::new, read_timing),
        }
    }

    /// Every effect, with its step.
    pub fn effects(&self) -> impl Iterator<Item = (usize, &Effect)> {
        self.steps
            .iter()
            .enumerate()
            .flat_map(|(i, s)| s.effects.iter().map(move |e| (i, e)))
    }

    /// Whether `target` is hidden when the slide comes up: its first
    /// entrance or exit is an entrance.
    pub fn starts_hidden(&self, target: Target) -> bool {
        self.effects()
            .map(|(_, e)| e)
            .find(|e| {
                e.target == target && matches!(e.class, EffectClass::Entrance | EffectClass::Exit)
            })
            .is_some_and(|e| e.class == EffectClass::Entrance)
    }

    /// Targets hidden when the slide comes up.
    pub fn hidden_at_start(&self) -> Vec<Target> {
        let mut out: Vec<Target> = Vec::new();
        for (_, e) in self.effects() {
            if !out.contains(&e.target) && self.starts_hidden(e.target) {
                out.push(e.target);
            }
        }
        out
    }

    /// Whether any effect animates shape `id` (or a part of it).
    pub fn animates(&self, id: u32) -> bool {
        self.effects().any(|(_, e)| e.target.shape_id() == id)
    }

    /// Whether the slide's text paragraphs of shape `id` animate apart.
    pub fn by_paragraph(&self, id: u32) -> bool {
        self.effects()
            .any(|(_, e)| matches!(e.target, Target::Para(s, _) if s == id))
    }

    /// Keep effects whose shape is in `ids`; drop steps left empty.
    pub fn retain_shapes(&mut self, ids: &[u32]) {
        for s in &mut self.steps {
            s.effects.retain(|e| ids.contains(&e.target.shape_id()));
        }
        self.steps.retain(|s| !s.effects.is_empty());
    }
}

impl Slide {
    /// The slide's transition and build steps, as the slide show plays
    /// them. Effects on shapes no longer on the slide (or inside a group)
    /// are left out.
    pub fn show_fx(&self) -> SlideFx {
        let mut fx = SlideFx::from_xml(&self.xml());
        let ids: Vec<u32> = self.shapes.iter().map(|s| s.id).collect();
        fx.retain_shapes(&ids);
        fx
    }
}

/// `p:<name>` under the root, or inside `mc:AlternateContent` there (the
/// first `mc:Choice` holding one, else the `mc:Fallback`).
fn find_alternate<'a, 'i>(root: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    if let Some(n) = child(root, NS_P, name) {
        return Some(n);
    }
    for alt in children(root, NS_MC, "AlternateContent") {
        let choice = children(alt, NS_MC, "Choice").find_map(|c| child(c, NS_P, name));
        let fallback = || child(alt, NS_MC, "Fallback").and_then(|f| child(f, NS_P, name));
        if let Some(n) = choice.or_else(fallback) {
            return Some(n);
        }
    }
    None
}

fn read_transition(t: Node) -> Option<Transition> {
    let speed_ms = match t.attribute("spd") {
        Some("slow") => 1000,
        Some("med") => 750,
        _ => 500,
    };
    let dur_attr = t
        .attribute((NS_P14, "dur"))
        .and_then(|d| d.trim().parse::<u32>().ok());
    let advance_on_click = attr_bool(t, "advClick").unwrap_or(true);
    let advance_after_ms = t
        .attribute("advTm")
        .and_then(|v| v.trim().parse::<u32>().ok());
    let black = Rgb([0, 0, 0]);
    let white = Rgb([255, 255, 255]);
    let mut through = None;
    let mut duration = dur_attr.unwrap_or(speed_ms);
    let el = t
        .children()
        .find(|c| c.is_element() && !matches!(c.tag_name().name(), "sndAc" | "extLst"));
    let (kind, source) = match el {
        None => {
            duration = 0;
            (TransitionKind::Cut, String::new())
        }
        Some(e) => {
            let ns = e.tag_name().namespace().unwrap_or("");
            let name = e.tag_name().name();
            let dir = |d: Dir| e.attribute("dir").and_then(Dir::from_ooxml).unwrap_or(d);
            let thru = attr_bool(e, "thruBlk").unwrap_or(false);
            let source = match ns {
                NS_P14 => format!("p14:{name}"),
                NS_P15 => match e.attribute("prst") {
                    Some(p) => format!("p15:{name} {p}"),
                    None => format!("p15:{name}"),
                },
                _ => name.to_owned(),
            };
            let kind = if ns == NS_P {
                match name {
                    "cut" => {
                        if thru {
                            through = Some(black);
                        } else if dur_attr.is_none() {
                            duration = 0;
                        }
                        TransitionKind::Cut
                    }
                    "fade" => {
                        if thru {
                            through = Some(black);
                        }
                        TransitionKind::Fade
                    }
                    "push" => TransitionKind::Push(dir(Dir::Left)),
                    "wipe" => TransitionKind::Wipe(dir(Dir::Left)),
                    "strips" => TransitionKind::Wipe(dir(Dir::LeftUp)),
                    "cover" => TransitionKind::Cover(dir(Dir::Left)),
                    "pull" => TransitionKind::Uncover(dir(Dir::Left)),
                    "split" => TransitionKind::Split {
                        vertical: e.attribute("orient") == Some("vert"),
                        out: e.attribute("dir") != Some("in"),
                    },
                    "zoom" => TransitionKind::Box {
                        out: e.attribute("dir") == Some("in"),
                    },
                    "circle" | "diamond" | "plus" | "newsflash" => {
                        TransitionKind::Box { out: true }
                    }
                    _ => TransitionKind::Fade,
                }
            } else if ns == NS_P14 {
                let side = || match e.attribute("dir") {
                    Some("r") => Dir::Right,
                    Some("u") => Dir::Up,
                    Some("d") => Dir::Down,
                    _ => Dir::Left,
                };
                match name {
                    "pan" | "conveyor" | "ferris" | "gallery" => TransitionKind::Push(side()),
                    "doors" | "window" => TransitionKind::Split {
                        vertical: e.attribute("dir") != Some("horz"),
                        out: true,
                    },
                    "flash" => {
                        through = Some(white);
                        TransitionKind::Fade
                    }
                    "reveal" => {
                        through = Some(black);
                        TransitionKind::Fade
                    }
                    "warp" | "flythrough" => TransitionKind::Box {
                        out: e.attribute("dir") != Some("out"),
                    },
                    _ => TransitionKind::Fade,
                }
            } else {
                TransitionKind::Fade
            };
            (kind, source)
        }
    };
    Some(Transition {
        kind,
        through,
        duration_ms: duration,
        advance_on_click,
        advance_after_ms,
        source,
    })
}

/// A `p:cond`/`p:cTn` time in ms (`indefinite` and junk are `None`).
fn ms(v: Option<&str>) -> Option<u32> {
    let v = v?.trim();
    v.parse::<i64>().ok().map(|n| n.max(0) as u32)
}

/// The time nodes (`p:par`, `p:seq`, `p:set`, `p:anim`...) in a
/// `p:childTnLst`, each with its `p:cTn`.
fn time_children<'a, 'i>(ctn: Node<'a, 'i>) -> Vec<(Node<'a, 'i>, Node<'a, 'i>)> {
    let Some(list) = child(ctn, NS_P, "childTnLst") else {
        return Vec::new();
    };
    list.children()
        .filter(|n| n.is_element())
        .filter_map(|n| {
            let ctn = child(n, NS_P, "cTn")
                .or_else(|| child(n, NS_P, "cBhvr").and_then(|b| child(b, NS_P, "cTn")))
                .or_else(|| child(n, NS_P, "cMediaNode").and_then(|m| child(m, NS_P, "cTn")))?;
            Some((n, ctn))
        })
        .collect()
}

/// The `p:cond` children of a `p:cTn`'s start conditions.
fn start_conds<'a, 'i>(ctn: Node<'a, 'i>) -> Vec<Node<'a, 'i>> {
    child(ctn, NS_P, "stCondLst")
        .map(|l| children(l, NS_P, "cond").collect())
        .unwrap_or_default()
}

/// The fixed start delay of a time node (its first numeric `p:cond`).
fn delay(ctn: Node) -> u32 {
    start_conds(ctn)
        .into_iter()
        .find_map(|c| ms(c.attribute("delay")))
        .unwrap_or(0)
}

fn read_timing(timing: Node) -> Vec<BuildStep> {
    let Some(root) = child(timing, NS_P, "tnLst")
        .and_then(|l| child(l, NS_P, "par"))
        .and_then(|p| child(p, NS_P, "cTn"))
    else {
        return Vec::new();
    };
    let Some(main) = time_children(root).into_iter().find_map(|(n, c)| {
        (is(n, NS_P, "seq") && c.attribute("nodeType") == Some("mainSeq")).then_some(c)
    }) else {
        return Vec::new();
    };
    let mut steps = Vec::new();
    for (i, (_, click)) in time_children(main).into_iter().enumerate() {
        let conds = start_conds(click);
        let on_begin = conds.iter().any(|c| c.attribute("evt") == Some("onBegin"));
        let waits = conds
            .iter()
            .any(|c| c.attribute("delay") == Some("indefinite"));
        let mut step = BuildStep {
            auto: i == 0 && (on_begin || !waits),
            effects: Vec::new(),
        };
        for (_, group) in time_children(click) {
            let at = delay(group);
            for (_, effect) in time_children(group) {
                read_effect(effect, at + delay(effect), &mut step.effects);
            }
        }
        step.effects.sort_by_key(|e| e.start_ms);
        if !step.effects.is_empty() {
            steps.push(step);
        }
    }
    steps
}

/// What the behaviours of one effect say.
#[derive(Default)]
struct Behaviours {
    end_ms: u32,
    targets: Vec<Target>,
    filter: Option<String>,
    moves: bool,
}

fn behaviours(ctn: Node, offset: u32, out: &mut Behaviours) {
    for (n, c) in time_children(ctn) {
        let start = offset + delay(c);
        let dur = ms(c.attribute("dur")).unwrap_or(0);
        out.end_ms = out.end_ms.max(start + dur);
        if is(n, NS_P, "animEffect")
            && out.filter.is_none()
            && let Some(f) = n.attribute("filter")
        {
            out.filter = Some(f.to_owned());
        }
        if is(n, NS_P, "anim")
            && n.descendants()
                .any(|a| is(a, NS_P, "attrName") && a.text().is_some_and(|t| t.starts_with("ppt_")))
        {
            out.moves = true;
        }
        if let Some(tgt) = child(n, NS_P, "cBhvr")
            .and_then(|b| child(b, NS_P, "tgtEl"))
            .and_then(|t| child(t, NS_P, "spTgt"))
        {
            for t in sp_targets(tgt) {
                if !out.targets.contains(&t) {
                    out.targets.push(t);
                }
            }
        }
        // Nested groups (`p:par` inside an effect).
        if matches!(n.tag_name().name(), "par" | "seq" | "excl") {
            behaviours(c, start, out);
        }
    }
}

fn sp_targets(tgt: Node) -> Vec<Target> {
    let Some(id) = tgt
        .attribute("spid")
        .and_then(|v| v.trim().parse::<u32>().ok())
    else {
        return Vec::new();
    };
    if child(tgt, NS_P, "bg").is_some() {
        return vec![Target::Background(id)];
    }
    if let Some(r) = child(tgt, NS_P, "txEl").and_then(|t| child(t, NS_P, "pRg")) {
        let st = ms(r.attribute("st")).unwrap_or(0);
        let end = ms(r.attribute("end")).unwrap_or(st).max(st);
        // A paragraph range is at most a slide of text; don't trust more.
        return (st..=end.min(st + 1000))
            .map(|p| Target::Para(id, p))
            .collect();
    }
    vec![Target::Shape(id)]
}

fn read_effect(ctn: Node, start: u32, out: &mut Vec<Effect>) {
    let class = match ctn.attribute("presetClass") {
        Some("entr") => EffectClass::Entrance,
        Some("exit") => EffectClass::Exit,
        Some("emph") => EffectClass::Emphasis,
        Some("path") => EffectClass::Path,
        _ => EffectClass::Other,
    };
    let trigger = match ctn.attribute("nodeType") {
        Some("withEffect") => Trigger::WithPrevious,
        Some("afterEffect") => Trigger::AfterPrevious,
        _ => Trigger::OnClick,
    };
    let preset_id = ms(ctn.attribute("presetID")).unwrap_or(0);
    let preset_subtype = ms(ctn.attribute("presetSubtype")).unwrap_or(0);
    let mut b = Behaviours::default();
    behaviours(ctn, 0, &mut b);
    // An effect's own duration (`p:cTn/@dur`) when its behaviours have none.
    let own = ms(ctn.attribute("dur")).unwrap_or(0);
    let dur = b.end_ms.max(own);
    let preset = preset(class, preset_id, preset_subtype, &b);
    for target in b.targets {
        out.push(Effect {
            target,
            class,
            preset,
            preset_id,
            preset_subtype,
            trigger,
            start_ms: start,
            dur_ms: dur,
        });
    }
}

/// A `wipe(up)` / `barn(inVertical)` / `fade` filter.
fn filter_preset(filter: &str) -> Option<Preset> {
    let (name, arg) = match filter.split_once('(') {
        Some((n, a)) => (n.trim(), a.trim_end_matches(')').trim()),
        None => (filter.trim(), ""),
    };
    Some(match name {
        "fade" => Preset::Fade,
        "wipe" | "slide" => Preset::Wipe(match arg {
            "up" | "fromBottom" => Dir::Up,
            "down" | "fromTop" => Dir::Down,
            "left" | "fromRight" => Dir::Left,
            _ => Dir::Right,
        }),
        "barn" => Preset::Split {
            vertical: arg.contains("Vertical"),
            out: arg.starts_with("out"),
        },
        "box" | "circle" | "diamond" | "plus" => Preset::Zoom,
        "image" => Preset::Appear,
        _ => return None,
    })
}

fn preset(class: EffectClass, id: u32, subtype: u32, b: &Behaviours) -> Preset {
    let entrance = class == EffectClass::Entrance;
    let filtered = b.filter.as_deref().and_then(filter_preset);
    if !matches!(class, EffectClass::Entrance | EffectClass::Exit) {
        return Preset::None;
    }
    let side = Dir::side_bits(subtype);
    match id {
        1 => Preset::Appear,
        10 => Preset::Fade,
        2 | 12 => {
            // Fly In from a side moves away from it; Fly Out moves to it.
            let side = side.unwrap_or(Dir::Down);
            Preset::Fly(if entrance { side.reverse() } else { side })
        }
        22 => filtered
            .filter(|p| matches!(p, Preset::Wipe(_)))
            .unwrap_or(Preset::Wipe(side.unwrap_or(Dir::Down).reverse())),
        16 => filtered
            .filter(|p| matches!(p, Preset::Split { .. }))
            .unwrap_or(Preset::Split {
                vertical: true,
                out: true,
            }),
        23 | 53 | 31 | 4 | 6 | 8 | 13 | 55 => Preset::Zoom,
        42 => Preset::Float(if entrance { Dir::Up } else { Dir::Down }),
        47 => Preset::Float(if entrance { Dir::Down } else { Dir::Up }),
        _ => match filtered {
            Some(p) => p,
            None if b.moves && entrance => Preset::Fly(Dir::Up),
            None if entrance => Preset::Fade,
            None => Preset::Appear,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = r#"<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main"><p:cSld><p:spTree/></p:cSld>"#;

    fn slide(tail: &str) -> SlideFx {
        SlideFx::from_xml(&format!("{HEAD}{tail}</p:sld>"))
    }

    #[test]
    fn transitions() {
        let t = slide(r#"<p:transition spd="slow" advClick="0" advTm="3000"><p:push dir="u"/></p:transition>"#)
            .transition
            .unwrap();
        assert_eq!(t.kind, TransitionKind::Push(Dir::Up));
        assert_eq!(t.duration_ms, 1000);
        assert!(!t.advance_on_click);
        assert_eq!(t.advance_after_ms, Some(3000));

        let t = slide(r#"<p:transition><p:fade thruBlk="1"/></p:transition>"#)
            .transition
            .unwrap();
        assert_eq!(
            (t.kind, t.through, t.duration_ms),
            (TransitionKind::Fade, Some(Rgb([0, 0, 0])), 500)
        );

        let t = slide(r#"<p:transition/>"#).transition.unwrap();
        assert_eq!((t.kind, t.duration_ms), (TransitionKind::Cut, 0));
        assert!(slide("").transition.is_none());

        let t = slide(r#"<p:transition><p:split orient="vert" dir="in"/></p:transition>"#)
            .transition
            .unwrap();
        assert_eq!(
            t.kind,
            TransitionKind::Split {
                vertical: true,
                out: false
            }
        );
    }

    #[test]
    fn alternate_content_prefers_the_choice() {
        let fx = slide(
            r#"<mc:AlternateContent><mc:Choice Requires="p14"><p:transition spd="slow" p14:dur="2000"><p14:pan dir="u"/></p:transition></mc:Choice><mc:Fallback><p:transition spd="slow"><p:fade/></p:transition></mc:Fallback></mc:AlternateContent>"#,
        );
        let t = fx.transition.unwrap();
        assert_eq!(t.kind, TransitionKind::Push(Dir::Up));
        assert_eq!(t.duration_ms, 2000);
        assert_eq!(t.source, "p14:pan");

        let t = slide(r#"<mc:AlternateContent><mc:Choice Requires="p14"><p:transition><p14:vortex dir="r"/></p:transition></mc:Choice></mc:AlternateContent>"#)
            .transition
            .unwrap();
        assert_eq!(t.kind, TransitionKind::Fade);
    }

    /// One effect node as PowerPoint writes it.
    #[allow(clippy::too_many_arguments)]
    fn effect(
        id: u32,
        class: &str,
        preset: u32,
        sub: u32,
        node: &str,
        delay: u32,
        target: &str,
        body: &str,
    ) -> String {
        format!(
            r#"<p:par><p:cTn id="{id}" presetID="{preset}" presetClass="{class}" presetSubtype="{sub}" fill="hold" nodeType="{node}"><p:stCondLst><p:cond delay="{delay}"/></p:stCondLst><p:childTnLst><p:set><p:cBhvr><p:cTn id="{}" dur="1" fill="hold"><p:stCondLst><p:cond delay="0"/></p:stCondLst></p:cTn><p:tgtEl><p:spTgt spid="{target}"/></p:tgtEl><p:attrNameLst><p:attrName>style.visibility</p:attrName></p:attrNameLst></p:cBhvr><p:to><p:strVal val="visible"/></p:to></p:set>{body}</p:childTnLst></p:cTn></p:par>"#,
            id + 100
        )
    }

    fn fade_body(target: &str, dur: u32) -> String {
        format!(
            r#"<p:animEffect transition="in" filter="fade"><p:cBhvr><p:cTn id="900" dur="{dur}"/><p:tgtEl><p:spTgt spid="{target}"/></p:tgtEl></p:cBhvr></p:animEffect>"#
        )
    }

    fn timing(clicks: &[String]) -> String {
        format!(
            r#"<p:timing><p:tnLst><p:par><p:cTn id="1" dur="indefinite" restart="never" nodeType="tmRoot"><p:childTnLst><p:seq concurrent="1" nextAc="seek"><p:cTn id="2" dur="indefinite" nodeType="mainSeq"><p:childTnLst>{}</p:childTnLst></p:cTn><p:prevCondLst/><p:nextCondLst/></p:seq></p:childTnLst></p:cTn></p:par></p:tnLst></p:timing>"#,
            clicks.concat()
        )
    }

    fn click(conds: &str, groups: &[(u32, String)]) -> String {
        let inner: String = groups
            .iter()
            .map(|(d, e)| {
                format!(
                    r#"<p:par><p:cTn id="50" fill="hold"><p:stCondLst><p:cond delay="{d}"/></p:stCondLst><p:childTnLst>{e}</p:childTnLst></p:cTn></p:par>"#
                )
            })
            .collect();
        format!(
            r#"<p:par><p:cTn id="40" fill="hold"><p:stCondLst>{conds}</p:stCondLst><p:childTnLst>{inner}</p:childTnLst></p:cTn></p:par>"#
        )
    }

    #[test]
    fn steps_with_and_after_previous() {
        let first = click(
            r#"<p:cond delay="indefinite"/><p:cond evt="onBegin" delay="0"><p:tn val="2"/></p:cond>"#,
            &[(
                0,
                effect(
                    3,
                    "entr",
                    10,
                    0,
                    "afterEffect",
                    0,
                    "4",
                    &fade_body("4", 500),
                ),
            )],
        );
        let second = click(
            r#"<p:cond delay="indefinite"/>"#,
            &[
                (
                    0,
                    effect(5, "entr", 2, 4, "clickEffect", 0, "5", "")
                        + &effect(6, "entr", 22, 8, "withEffect", 250, "6", ""),
                ),
                (
                    500,
                    effect(
                        7,
                        "exit",
                        10,
                        0,
                        "afterEffect",
                        0,
                        "4",
                        &fade_body("4", 750),
                    ),
                ),
            ],
        );
        let fx = slide(&timing(&[first, second]));
        assert_eq!(fx.steps.len(), 2);
        assert!(fx.steps[0].auto);
        assert!(!fx.steps[1].auto);
        let e = &fx.steps[1].effects;
        assert_eq!(e.len(), 3);
        assert_eq!(
            (e[0].target, e[0].preset, e[0].trigger),
            (Target::Shape(5), Preset::Fly(Dir::Up), Trigger::OnClick)
        );
        assert_eq!(
            (e[1].preset, e[1].start_ms, e[1].trigger),
            (Preset::Wipe(Dir::Right), 250, Trigger::WithPrevious)
        );
        assert_eq!(
            (e[2].class, e[2].start_ms, e[2].dur_ms),
            (EffectClass::Exit, 500, 750)
        );
        assert_eq!(fx.steps[1].duration_ms(), 1250);
        assert!(fx.starts_hidden(Target::Shape(4)));
        assert!(fx.starts_hidden(Target::Shape(5)));
        assert!(!fx.starts_hidden(Target::Shape(9)));
    }

    #[test]
    fn paragraph_targets_and_dropped_shapes() {
        let para = effect(3, "entr", 10, 0, "clickEffect", 0, "4", "").replace(
            r#"<p:spTgt spid="4"/>"#,
            r#"<p:spTgt spid="4"><p:txEl><p:pRg st="1" end="1"/></p:txEl></p:spTgt>"#,
        );
        let exit_only = effect(8, "exit", 10, 0, "clickEffect", 0, "9", "");
        let mut fx = slide(&timing(&[
            click(r#"<p:cond delay="indefinite"/>"#, &[(0, para)]),
            click(r#"<p:cond delay="indefinite"/>"#, &[(0, exit_only)]),
        ]));
        assert_eq!(fx.steps[0].effects[0].target, Target::Para(4, 1));
        assert!(fx.by_paragraph(4));
        assert!(fx.starts_hidden(Target::Para(4, 1)));
        assert!(!fx.starts_hidden(Target::Shape(9)));
        fx.retain_shapes(&[4]);
        assert_eq!(fx.steps.len(), 1);
    }
}
