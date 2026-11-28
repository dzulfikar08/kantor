//! A force-directed layout for the graph view: notes are points that push
//! each other apart, links are springs, and a weak pull to the middle
//! keeps unlinked notes near the rest (the model of d3-force, which
//! Obsidian's graph uses too).
//!
//! The push between every pair of points is approximated with a
//! Barnes–Hut quadtree, so a step is O(n log n) and a vault of thousands
//! of notes still moves smoothly. The simulation cools down ("alpha") and
//! stops; dragging a point warms it up again.

/// How the graph moves.
#[derive(Clone, Copy, Debug)]
pub struct Forces {
    /// Spring rest length.
    pub link_distance: f32,
    /// Push between points (negative: apart).
    pub charge: f32,
    /// Pushes from farther than this are left out.
    pub charge_max_distance: f32,
    /// Pull toward the middle.
    pub gravity: f32,
    /// Share of the speed lost each step.
    pub velocity_decay: f32,
    /// Barnes–Hut accuracy (0: exact).
    pub theta: f32,
}

impl Default for Forces {
    fn default() -> Self {
        Self {
            link_distance: 60.,
            charge: -140.,
            charge_max_distance: 900.,
            gravity: 0.045,
            velocity_decay: 0.4,
            theta: 0.9,
        }
    }
}

/// Below this the simulation is at rest.
pub const ALPHA_MIN: f32 = 0.001;
/// Cooling per step (d3's: about 300 steps from 1 to rest).
const ALPHA_DECAY: f32 = 0.0228;

#[derive(Clone, Debug)]
pub struct Graph {
    pub pos: Vec<[f32; 2]>,
    vel: Vec<[f32; 2]>,
    pub edges: Vec<(usize, usize)>,
    /// Links per point.
    pub degree: Vec<u32>,
    /// Held in place (being dragged).
    pub fixed: Vec<bool>,
    pub alpha: f32,
    /// Where alpha settles (above [`ALPHA_MIN`] while dragging).
    pub alpha_target: f32,
    pub forces: Forces,
}

impl Graph {
    /// `n` points on a spiral (deterministic), linked by `edges`.
    pub fn new(n: usize, edges: Vec<(usize, usize)>) -> Self {
        let mut pos = Vec::with_capacity(n);
        // d3's phyllotaxis start: even spacing, no randomness.
        let angle = std::f32::consts::PI * (3. - 5f32.sqrt());
        for i in 0..n {
            let r = 10. * (0.5 + i as f32).sqrt();
            let a = i as f32 * angle;
            pos.push([r * a.cos(), r * a.sin()]);
        }
        let edges: Vec<(usize, usize)> = edges
            .into_iter()
            .filter(|&(a, b)| a < n && b < n && a != b)
            .collect();
        let mut degree = vec![0u32; n];
        for &(a, b) in &edges {
            degree[a] += 1;
            degree[b] += 1;
        }
        Self {
            pos,
            vel: vec![[0., 0.]; n],
            edges,
            degree,
            fixed: vec![false; n],
            alpha: 1.,
            alpha_target: 0.,
            forces: Forces::default(),
        }
    }

    pub fn len(&self) -> usize {
        self.pos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pos.is_empty()
    }

    /// Still moving.
    pub fn is_active(&self) -> bool {
        self.alpha >= ALPHA_MIN || self.alpha_target >= ALPHA_MIN
    }

    /// Warm up to at least `alpha` (after a drag or a change).
    pub fn reheat(&mut self, alpha: f32) {
        self.alpha = self.alpha.max(alpha);
    }

    /// One step; returns whether anything still moves.
    pub fn tick(&mut self) -> bool {
        if !self.is_active() {
            return false;
        }
        self.alpha += (self.alpha_target - self.alpha) * ALPHA_DECAY;
        let alpha = self.alpha;
        self.links(alpha);
        self.charge(alpha);
        let g = self.forces.gravity * alpha;
        let keep = 1. - self.forces.velocity_decay;
        for ((p, v), &fixed) in self.pos.iter_mut().zip(&mut self.vel).zip(&self.fixed) {
            if fixed {
                *v = [0., 0.];
                continue;
            }
            v[0] = (v[0] - p[0] * g) * keep;
            v[1] = (v[1] - p[1] * g) * keep;
            p[0] += v[0];
            p[1] += v[1];
        }
        true
    }

    /// Run until at rest (or `max` steps), e.g. before the first paint.
    pub fn settle(&mut self, max: usize) {
        for _ in 0..max {
            if !self.tick() {
                break;
            }
        }
    }

    fn links(&mut self, alpha: f32) {
        let d0 = self.forces.link_distance;
        let edges = std::mem::take(&mut self.edges);
        for (k, &(s, t)) in edges.iter().enumerate() {
            let (ds, dt) = (self.degree[s].max(1) as f32, self.degree[t].max(1) as f32);
            let strength = 1. / ds.min(dt);
            let bias = ds / (ds + dt);
            let mut dx = self.pos[t][0] + self.vel[t][0] - self.pos[s][0] - self.vel[s][0];
            let mut dy = self.pos[t][1] + self.vel[t][1] - self.pos[s][1] - self.vel[s][1];
            if dx == 0. && dy == 0. {
                dx = jiggle(k);
                dy = jiggle(k + 1);
            }
            let l = (dx * dx + dy * dy).sqrt();
            let f = (l - d0) / l * alpha * strength;
            let (fx, fy) = (dx * f, dy * f);
            self.vel[t][0] -= fx * bias;
            self.vel[t][1] -= fy * bias;
            self.vel[s][0] += fx * (1. - bias);
            self.vel[s][1] += fy * (1. - bias);
        }
        self.edges = edges;
    }

    fn charge(&mut self, alpha: f32) {
        let deltas = charge_deltas(
            &self.pos,
            self.forces.charge * alpha,
            self.forces.theta,
            self.forces.charge_max_distance,
        );
        for (v, d) in self.vel.iter_mut().zip(deltas) {
            v[0] += d[0];
            v[1] += d[1];
        }
    }

    /// The point within `radius` of `p` nearest to it.
    pub fn nearest(&self, p: [f32; 2], radius: f32) -> Option<usize> {
        let mut best = None;
        let mut best_d = radius * radius;
        for (i, q) in self.pos.iter().enumerate() {
            let d = (q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2);
            if d <= best_d {
                best_d = d;
                best = Some(i);
            }
        }
        best
    }

    /// `[min x, min y, max x, max y]` of the points.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let mut it = self.pos.iter();
        let f = it.next()?;
        let mut b = [f[0], f[1], f[0], f[1]];
        for p in it {
            b[0] = b[0].min(p[0]);
            b[1] = b[1].min(p[1]);
            b[2] = b[2].max(p[0]);
            b[3] = b[3].max(p[1]);
        }
        Some(b)
    }
}

/// A tiny, deterministic nudge for points on top of each other.
fn jiggle(k: usize) -> f32 {
    ((k as f32 * 12.9898).sin() * 43_758.547).fract() * 1e-3 + 1e-4
}

struct Quad {
    /// Centre of mass and how many points.
    cx: f32,
    cy: f32,
    mass: f32,
    /// Side length.
    size: f32,
    kids: [u32; 4],
    /// Leaf points (empty for an inner node).
    pts: Vec<u32>,
}

const NONE: u32 = u32::MAX;
const MAX_DEPTH: u32 = 24;

fn build(
    arena: &mut Vec<Quad>,
    pos: &[[f32; 2]],
    idx: Vec<u32>,
    origin: [f32; 2],
    size: f32,
    depth: u32,
) -> u32 {
    let n = idx.len() as f32;
    let (sx, sy) = idx.iter().fold((0., 0.), |(x, y), &i| {
        (x + pos[i as usize][0], y + pos[i as usize][1])
    });
    let me = arena.len() as u32;
    arena.push(Quad {
        cx: sx / n,
        cy: sy / n,
        mass: n,
        size,
        kids: [NONE; 4],
        pts: Vec::new(),
    });
    if idx.len() <= 1 || depth >= MAX_DEPTH {
        arena[me as usize].pts = idx;
        return me;
    }
    let half = size / 2.;
    let mid = [origin[0] + half, origin[1] + half];
    let mut parts: [Vec<u32>; 4] = Default::default();
    for i in idx {
        let p = pos[i as usize];
        let q = usize::from(p[0] >= mid[0]) + 2 * usize::from(p[1] >= mid[1]);
        parts[q].push(i);
    }
    for (q, part) in parts.into_iter().enumerate() {
        if part.is_empty() {
            continue;
        }
        let o = [
            origin[0] + if q & 1 == 1 { half } else { 0. },
            origin[1] + if q & 2 == 2 { half } else { 0. },
        ];
        let k = build(arena, pos, part, o, half, depth + 1);
        arena[me as usize].kids[q] = k;
    }
    me
}

/// The velocity change charge gives each point (`strength` already scaled
/// by alpha), Barnes–Hut with accuracy `theta`.
pub fn charge_deltas(pos: &[[f32; 2]], strength: f32, theta: f32, max_dist: f32) -> Vec<[f32; 2]> {
    let n = pos.len();
    let mut out = vec![[0f32; 2]; n];
    if n < 2 {
        return out;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in pos {
        x0 = x0.min(p[0]);
        y0 = y0.min(p[1]);
        x1 = x1.max(p[0]);
        y1 = y1.max(p[1]);
    }
    let size = (x1 - x0).max(y1 - y0).max(1.) * 1.0001;
    let mut arena = Vec::with_capacity(n * 2);
    let root = build(&mut arena, pos, (0..n as u32).collect(), [x0, y0], size, 0);
    let theta2 = theta * theta;
    let max2 = max_dist * max_dist;
    let mut stack = Vec::with_capacity(64);
    for (i, p) in pos.iter().enumerate() {
        let mut v = [0f32; 2];
        stack.clear();
        stack.push(root);
        while let Some(k) = stack.pop() {
            let q = &arena[k as usize];
            let leaf = q.kids.iter().all(|&c| c == NONE);
            if leaf {
                for &j in &q.pts {
                    if j as usize == i {
                        continue;
                    }
                    let mut dx = pos[j as usize][0] - p[0];
                    let mut dy = pos[j as usize][1] - p[1];
                    if dx == 0. && dy == 0. {
                        dx = jiggle(i);
                        dy = jiggle(j as usize);
                    }
                    let mut d2 = dx * dx + dy * dy;
                    if d2 > max2 {
                        continue;
                    }
                    // Very close points: d3 keeps the force finite this way.
                    if d2 < 1. {
                        d2 = d2.sqrt();
                    }
                    v[0] += dx * strength / d2;
                    v[1] += dy * strength / d2;
                }
                continue;
            }
            let dx = q.cx - p[0];
            let dy = q.cy - p[1];
            let d2 = dx * dx + dy * dy;
            if q.size * q.size < theta2 * d2 {
                // Far enough: the whole cell as one point.
                if d2 < max2 {
                    let d2 = d2.max(1.);
                    v[0] += dx * strength * q.mass / d2;
                    v[1] += dy * strength * q.mass / d2;
                }
                continue;
            }
            stack.extend(q.kids.iter().copied().filter(|&c| c != NONE));
        }
        out[i] = v;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(g: &Graph, a: usize, b: usize) -> f32 {
        let (p, q) = (g.pos[a], g.pos[b]);
        ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
    }

    #[test]
    fn linked_notes_end_up_closer() {
        // Two triangles, joined by one link; one lone point.
        let edges = vec![(0, 1), (1, 2), (2, 0), (3, 4), (4, 5), (5, 3), (2, 3)];
        let mut g = Graph::new(7, edges);
        g.settle(1000);
        assert!(!g.is_active(), "comes to rest");
        let inside = dist(&g, 0, 1);
        let across = dist(&g, 0, 5);
        assert!(inside < across, "{inside} < {across}");
        assert!(
            (20. ..130.).contains(&inside),
            "springs near their length: {inside}"
        );
        // The lone point stays in view (gravity), not far away.
        let b = g.bounds().unwrap();
        assert!(b[2] - b[0] < 1000. && b[3] - b[1] < 1000.);
    }

    #[test]
    fn barnes_hut_is_close_to_exact() {
        let g = Graph::new(200, vec![]);
        let exact = charge_deltas(&g.pos, -100., 0., 1e9);
        let approx = charge_deltas(&g.pos, -100., 0.9, 1e9);
        let mean = exact
            .iter()
            .map(|e| (e[0].powi(2) + e[1].powi(2)).sqrt())
            .sum::<f32>()
            / exact.len() as f32;
        let mut worst = 0f32;
        for (e, a) in exact.iter().zip(&approx) {
            let d = ((e[0] - a[0]).powi(2) + (e[1] - a[1]).powi(2)).sqrt();
            worst = worst.max(d / mean);
        }
        assert!(worst < 0.3, "worst error {worst} of the mean push");
    }

    #[test]
    fn fixed_points_stay_put_and_are_found() {
        let mut g = Graph::new(5, vec![(0, 1), (1, 2)]);
        g.fixed[0] = true;
        g.pos[0] = [500., 500.];
        for _ in 0..50 {
            g.tick();
        }
        assert_eq!(g.pos[0], [500., 500.]);
        assert_eq!(g.nearest([505., 498.], 20.), Some(0));
        assert_eq!(g.nearest([5000., 0.], 20.), None);
    }

    #[test]
    fn deterministic_and_coincident_points_separate() {
        let mut a = Graph::new(30, vec![(0, 1), (2, 3)]);
        let mut b = a.clone();
        a.settle(100);
        b.settle(100);
        assert_eq!(a.pos, b.pos);
        let mut c = Graph::new(3, vec![]);
        c.pos = vec![[0., 0.]; 3];
        c.settle(300);
        assert!(dist(&c, 0, 1) > 1.);
    }
}
