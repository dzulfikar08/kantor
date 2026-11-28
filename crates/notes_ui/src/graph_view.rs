//! The graph view: every note of the vault (or the current note's
//! neighbourhood) as a dot, every link as a line, laid out by
//! [`notes_engine::graph`] and painted with GPUI paths and quads. Drag the
//! background to pan, scroll to zoom, drag a dot to move it (the others
//! follow), click a dot to open its note.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex};
use gpui_kit::prelude::*;
use gpui_kit::*;
use kantor_ui::components::{IconButtonExt as _, icon_button};
use kantor_ui::i18n::{t, tf};
use kantor_ui::{metrics::*, tokens};
use notes_engine::graph::Graph;

use crate::NotesView;

/// A dot's radius (graph units) for a note with `degree` links.
fn radius(degree: u32) -> f32 {
    4. + (degree as f32).sqrt() * 1.8
}

struct Node {
    rel: String,
    label: SharedString,
    path: PathBuf,
}

enum Drag {
    Pan {
        from: Point<Pixels>,
        pan0: Point<Pixels>,
    },
    Node {
        i: usize,
        from: Point<Pixels>,
        moved: bool,
    },
}

pub(crate) struct GraphState {
    /// Only the current note and its neighbours.
    pub local: bool,
    pub depth: usize,
    nodes: Vec<Node>,
    sim: Graph,
    /// What the nodes were built from: vault revision, centre note, local, depth.
    built: Option<(u64, Option<String>, bool, usize)>,
    /// The current note's dot.
    center: Option<usize>,
    pan: Point<Pixels>,
    zoom: f32,
    /// Keep everything in view until the user pans or zooms.
    auto_fit: bool,
    drag: Option<Drag>,
    hover: Option<usize>,
    bounds: Rc<Cell<Bounds<Pixels>>>,
    ticking: bool,
    _ticker: Option<Task<()>>,
}

impl GraphState {
    pub(crate) fn new(local: bool) -> Self {
        Self {
            local,
            depth: 1,
            nodes: Vec::new(),
            sim: Graph::new(0, Vec::new()),
            built: None,
            center: None,
            pan: point(px(0.), px(0.)),
            zoom: 1.,
            auto_fit: true,
            drag: None,
            hover: None,
            bounds: Rc::default(),
            ticking: false,
            _ticker: None,
        }
    }

    fn to_world(&self, b: Bounds<Pixels>, s: Point<Pixels>) -> [f32; 2] {
        let c = b.center();
        [
            f32::from(s.x - c.x - self.pan.x) / self.zoom,
            f32::from(s.y - c.y - self.pan.y) / self.zoom,
        ]
    }

    /// Zoom and pan so every dot is in view.
    fn fit(&mut self) {
        let b = self.bounds.get();
        let Some(r) = self.sim.bounds() else {
            return;
        };
        if b.size.width <= px(0.) {
            return;
        }
        let margin = 60.;
        let w = (r[2] - r[0]).max(1.) + margin * 2.;
        let h = (r[3] - r[1]).max(1.) + margin * 2.;
        let z = (f32::from(b.size.width) / w)
            .min(f32::from(b.size.height) / h)
            .clamp(0.15, 2.);
        self.zoom = z;
        let mid = [(r[0] + r[2]) / 2., (r[1] + r[3]) / 2.];
        self.pan = point(px(-mid[0] * z), px(-mid[1] * z));
    }

    /// The dot under screen point `s`.
    fn node_at(&self, s: Point<Pixels>) -> Option<usize> {
        let b = self.bounds.get();
        let w = self.to_world(b, s);
        let i = self.sim.nearest(w, (radius(30) + 6.) / self.zoom.min(1.))?;
        let r = radius(self.sim.degree[i]) + 4. / self.zoom;
        let p = self.sim.pos[i];
        ((p[0] - w[0]).powi(2) + (p[1] - w[1]).powi(2) <= r * r).then_some(i)
    }
}

impl NotesView {
    /// Show or hide the graph (in place of the note).
    pub fn toggle_graph(&mut self, local: bool, cx: &mut Context<Self>) {
        if !self.ensure_vault(cx) {
            return;
        }
        match &mut self.graph {
            Some(g) if g.local == local => self.graph = None,
            Some(g) => {
                g.local = local;
                g.auto_fit = true;
            }
            None => self.graph = Some(GraphState::new(local)),
        }
        cx.notify();
    }

    pub fn graph_open(&self) -> bool {
        self.graph.is_some()
    }

    /// Rebuild the dots when the vault or the centre note changed (dots
    /// that stay keep their places).
    fn graph_sync(&mut self, cx: &mut Context<Self>) {
        let Some(v) = self.vault.clone() else {
            return;
        };
        let me = self.my_rel(cx);
        let Some(g) = &mut self.graph else {
            return;
        };
        let s = v.read(cx);
        let key = (s.revision, me.clone(), g.local, g.depth);
        if g.built.as_ref() == Some(&key) {
            return;
        }
        let idx = &s.index;
        let center = me.as_deref().and_then(|r| idx.find(r));
        let picked: Vec<usize> = if g.local {
            center
                .map(|c| idx.neighbourhood(c, g.depth))
                .unwrap_or_default()
        } else {
            (0..idx.len()).collect()
        };
        let slot: HashMap<usize, usize> = picked.iter().enumerate().map(|(k, &i)| (i, k)).collect();
        let edges: Vec<(usize, usize)> = idx
            .edges()
            .into_iter()
            .filter_map(|(a, b)| Some((*slot.get(&a)?, *slot.get(&b)?)))
            .collect();
        let old: HashMap<&str, [f32; 2]> = g
            .nodes
            .iter()
            .zip(&g.sim.pos)
            .map(|(n, p)| (n.rel.as_str(), *p))
            .collect();
        let mut sim = Graph::new(picked.len(), edges);
        let mut kept = 0;
        let nodes: Vec<Node> = picked
            .iter()
            .enumerate()
            .map(|(k, &i)| {
                let n = &idx.notes[i];
                if let Some(p) = old.get(n.rel.as_str()) {
                    sim.pos[k] = *p;
                    kept += 1;
                }
                Node {
                    rel: n.rel.clone(),
                    label: n.stem().to_owned().into(),
                    path: idx.path_of(i),
                }
            })
            .collect();
        if kept * 2 >= nodes.len().max(1) {
            // Mostly the same graph: a gentle move.
            sim.alpha = 0.3;
        } else {
            // New: start it moving before the first paint.
            sim.settle(60);
            g.auto_fit = true;
        }
        g.center = center.and_then(|c| slot.get(&c).copied());
        g.nodes = nodes;
        g.sim = sim;
        g.built = Some(key);
        g.hover = None;
        g.drag = None;
    }

    /// Step the layout every frame while it moves.
    fn graph_kick(&mut self, cx: &mut Context<Self>) {
        let Some(g) = &mut self.graph else {
            return;
        };
        if g.ticking || !g.sim.is_active() {
            return;
        }
        g.ticking = true;
        g._ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let go = this
                    .update(cx, |this, cx| {
                        let Some(g) = &mut this.graph else {
                            return false;
                        };
                        let moving = g.sim.tick();
                        if g.auto_fit {
                            g.fit();
                        }
                        if !moving {
                            g.ticking = false;
                        }
                        cx.notify();
                        moving
                    })
                    .unwrap_or(false);
                if !go {
                    break;
                }
            }
        }));
    }

    fn graph_mouse_down(&mut self, ev: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = &mut self.graph else {
            return;
        };
        g.drag = Some(match g.node_at(ev.position) {
            Some(i) => {
                g.sim.fixed[i] = true;
                Drag::Node {
                    i,
                    from: ev.position,
                    moved: false,
                }
            }
            None => {
                g.auto_fit = false;
                Drag::Pan {
                    from: ev.position,
                    pan0: g.pan,
                }
            }
        });
        cx.notify();
    }

    fn graph_mouse_move(&mut self, ev: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = &mut self.graph else {
            return;
        };
        let b = g.bounds.get();
        let pressed = ev.pressed_button == Some(MouseButton::Left);
        let mut kick = false;
        match &mut g.drag {
            Some(Drag::Pan { from, pan0 }) if pressed => {
                g.pan = point(
                    pan0.x + ev.position.x - from.x,
                    pan0.y + ev.position.y - from.y,
                );
            }
            Some(Drag::Node { i, from, moved }) if pressed => {
                let far = f32::from(ev.position.x - from.x).abs()
                    + f32::from(ev.position.y - from.y).abs()
                    > 3.;
                if far || *moved {
                    *moved = true;
                    let i = *i;
                    g.auto_fit = false;
                    g.sim.pos[i] = g.to_world(b, ev.position);
                    g.sim.alpha_target = 0.3;
                    g.sim.reheat(0.3);
                    kick = true;
                }
            }
            _ => {
                let hover = g.node_at(ev.position);
                if hover == g.hover {
                    return;
                }
                g.hover = hover;
            }
        }
        if kick {
            self.graph_kick(cx);
        }
        cx.notify();
    }

    fn graph_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = &mut self.graph else {
            return;
        };
        let open = match g.drag.take() {
            Some(Drag::Node { i, moved, .. }) => {
                g.sim.fixed[i] = false;
                g.sim.alpha_target = 0.;
                (!moved).then(|| g.nodes[i].path.clone())
            }
            _ => None,
        };
        if let Some(path) = open {
            self.open_target(path, None, cx);
        }
        cx.notify();
    }

    fn graph_scroll(&mut self, ev: &ScrollEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = &mut self.graph else {
            return;
        };
        let dy = f32::from(ev.delta.pixel_delta(window.line_height()).y);
        if dy == 0. {
            return;
        }
        let b = g.bounds.get();
        let before = g.to_world(b, ev.position);
        let z = (g.zoom * (1.0015f32).powf(dy)).clamp(0.05, 6.);
        g.zoom = z;
        // Keep the point under the mouse where it is.
        let c = b.center();
        g.pan = point(
            ev.position.x - c.x - px(before[0] * z),
            ev.position.y - c.y - px(before[1] * z),
        );
        g.auto_fit = false;
        cx.notify();
    }

    pub(crate) fn render_graph(&mut self, cx: &mut Context<Self>) -> AnyElement {
        self.graph_sync(cx);
        self.graph_kick(cx);
        let t = tokens(cx);
        let Some(g) = &self.graph else {
            return div().into_any_element();
        };
        // What the canvas paints (a copy: the view can change meanwhile).
        let hover = g.hover;
        let neighbours: HashSet<usize> = match hover {
            Some(h) => g
                .sim
                .edges
                .iter()
                .filter_map(|&(a, b)| {
                    if a == h {
                        Some(b)
                    } else if b == h {
                        Some(a)
                    } else {
                        None
                    }
                })
                .collect(),
            None => HashSet::new(),
        };
        let pos = g.sim.pos.clone();
        let edges = g.sim.edges.clone();
        let degree = g.sim.degree.clone();
        let labels: Vec<SharedString> = g.nodes.iter().map(|n| n.label.clone()).collect();
        let (pan, zoom, center) = (g.pan, g.zoom, g.center);
        let bounds_cell = g.bounds.clone();
        let count = g.nodes.len();
        let local = g.local;
        let colors = (t.fg_muted, t.accent, t.border_strong, t.fg, t.fg_dim, t.bg);
        let font_family = cx.theme().font_family.clone();
        let entity = cx.entity().downgrade();
        let canvas = canvas(
            move |bounds, _, cx| {
                let old = bounds_cell.replace(bounds);
                if old.size != bounds.size {
                    let _ = entity.update(cx, |this, cx| {
                        if let Some(g) = &mut this.graph
                            && g.auto_fit
                        {
                            g.fit();
                        }
                        cx.notify();
                    });
                }
            },
            move |bounds, _, window, cx| {
                let (dot, accent, line, fg, dim, bg) = colors;
                let c = bounds.center();
                let at = |p: [f32; 2]| {
                    point(c.x + pan.x + px(p[0] * zoom), c.y + pan.y + px(p[1] * zoom))
                };
                window.with_content_mask(Some(ContentMask { bounds }), |window| {
                    // Lines: one path per few thousand (index limits).
                    let width = px((1.2 * zoom).clamp(0.6, 2.0));
                    for (lit, color) in [(false, line), (true, accent)] {
                        for chunk in edges
                            .iter()
                            .filter(|&&(a, b)| {
                                let on = hover.is_some_and(|h| a == h || b == h);
                                on == lit
                            })
                            .collect::<Vec<_>>()
                            .chunks(3000)
                        {
                            let mut pb = PathBuilder::stroke(width);
                            for &&(a, b) in chunk {
                                pb.move_to(at(pos[a]));
                                pb.line_to(at(pos[b]));
                            }
                            if let Ok(path) = pb.build() {
                                let color = if hover.is_some() && !lit {
                                    color.opacity(0.45)
                                } else {
                                    color
                                };
                                window.paint_path(path, color);
                            }
                        }
                    }
                    // Dots, then names.
                    let show_all_labels = zoom >= 0.75 && pos.len() <= 600;
                    let size = px((11. * zoom.clamp(0.8, 1.4)).round());
                    for (i, p) in pos.iter().enumerate() {
                        let s = at(*p);
                        if s.x < bounds.left() - px(80.)
                            || s.x > bounds.right() + px(80.)
                            || s.y < bounds.top() - px(40.)
                            || s.y > bounds.bottom() + px(40.)
                        {
                            continue;
                        }
                        let r = px((radius(degree[i]) * zoom).max(2.));
                        let is_center = center == Some(i);
                        let lit = hover == Some(i) || neighbours.contains(&i);
                        let faded = hover.is_some() && !lit;
                        let color = if is_center || hover == Some(i) {
                            accent
                        } else if faded {
                            dot.opacity(0.35)
                        } else {
                            dot
                        };
                        window.paint_quad(quad(
                            Bounds::new(point(s.x - r, s.y - r), size(r * 2., r * 2.)),
                            r,
                            color,
                            px(if is_center { 2. } else { 0. }),
                            bg,
                            BorderStyle::default(),
                        ));
                        if show_all_labels || lit || is_center {
                            let label = labels[i].clone();
                            let run = TextRun {
                                len: label.len(),
                                font: font(font_family.clone()),
                                color: if faded {
                                    dim
                                } else if lit || is_center {
                                    fg
                                } else {
                                    dot
                                },
                                background_color: None,
                                underline: None,
                                strikethrough: None,
                            };
                            let shaped = window.text_system().shape_line(label, size, &[run], None);
                            let x = s.x - shaped.width() / 2.;
                            let _ = shaped.paint(
                                point(x, s.y + r + px(3.)),
                                size * 1.3,
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }
                    }
                });
            },
        )
        .size_full();
        let controls = h_flex()
            .id("notes-graph-controls")
            .absolute()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .top(SPACE_4)
            .left(SPACE_4)
            .gap(SPACE_2)
            .p(SPACE_2)
            .rounded(RADIUS_MD)
            .bg(t.elevated)
            .border_1()
            .border_color(t.border)
            .child(
                Button::new("notes-graph-all")
                    .label(t("Whole vault"))
                    .xsmall()
                    .when(!local, |b| b.primary())
                    .when(local, |b| b.ghost())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(g) = &mut this.graph {
                            g.local = false;
                            g.auto_fit = true;
                        }
                        cx.notify();
                    })),
            )
            .child(
                Button::new("notes-graph-local")
                    .label(t("This note"))
                    .xsmall()
                    .when(local, |b| b.primary())
                    .when(!local, |b| b.ghost())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(g) = &mut this.graph {
                            g.local = true;
                            g.auto_fit = true;
                        }
                        cx.notify();
                    })),
            )
            .when(local, |el| {
                let depth = self.graph.as_ref().map_or(1, |g| g.depth);
                el.child(
                    Button::new("notes-graph-depth")
                        .label(tf("Depth {}", &[&depth]))
                        .xsmall()
                        .ghost()
                        .tooltip(t("Links away from this note"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(g) = &mut this.graph {
                                g.depth = g.depth % 3 + 1;
                                g.auto_fit = true;
                            }
                            cx.notify();
                        })),
                )
            })
            .child(
                Button::new("notes-graph-fit")
                    .label(t("Fit"))
                    .xsmall()
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(g) = &mut this.graph {
                            g.auto_fit = true;
                            g.fit();
                        }
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .px(SPACE_2)
                    .text_size(FONT_CAPTION)
                    .text_color(t.fg_dim)
                    .child(tf("{} notes", &[&count])),
            );
        let close = div()
            .absolute()
            .top(SPACE_4)
            .right(SPACE_4)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                icon_button("notes-graph-close", Lucide::X, false, cx)
                    .tip(t("Back to the note"), Some("escape"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.graph = None;
                        cx.notify();
                    })),
            );
        div()
            .id("notes-graph")
            .flex_1()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .bg(t.bg)
            .cursor(if hover.is_some() {
                CursorStyle::PointingHand
            } else {
                CursorStyle::Arrow
            })
            .on_mouse_down(MouseButton::Left, cx.listener(Self::graph_mouse_down))
            .on_mouse_move(cx.listener(Self::graph_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::graph_mouse_up))
            .on_scroll(cx.listener(Self::graph_scroll))
            .child(canvas)
            .child(controls)
            .child(close)
            .into_any_element()
    }
}
