//! Timings for big documents, with `KANTOR_PERF=1` (stderr, `[docs] …`):
//!
//! - opening: reading the file, the first layout, the first frame showing
//!   a page, and when the page count is final;
//! - every key press and wheel event: the time from the event to the end
//!   of the frame that shows its result (`[docs] key ctrl-end→paint 3.1 ms`);
//!   `tools/bigdoc/perf_stats.py` sums them up (p50/p95);
//! - with `KANTOR_PERF_DOCS_SCROLL=N` too, once the page count is final:
//!   N frames scrolling down 120 px per frame, N frames paging down one
//!   page per frame, then a jump to the last page and one to the middle,
//!   each frame's interval and work (render to end of paint) summed up.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::*;

use crate::DocsView;

pub(crate) struct Perf {
    /// When opening started.
    opened: Instant,
    /// The first frame with a page was painted.
    shown: Cell<bool>,
    /// The page count was reported final.
    final_pages: Cell<bool>,
    /// An input waiting for the frame that shows it: what, and when.
    pending: RefCell<Option<(String, Instant)>>,
    /// When the current frame's render started.
    render_started: Cell<Option<Instant>>,
    bench: RefCell<Option<Bench>>,
}

#[derive(Default)]
struct Bench {
    frames: usize,
    phase: usize,
    step: usize,
    last: Option<Instant>,
    intervals: Vec<f64>,
    work: Vec<f64>,
    jump: Option<Instant>,
}

const PHASES: [&str; 4] = ["wheel scroll", "page down", "jump to end", "jump to middle"];

fn stats(mut v: Vec<f64>) -> String {
    if v.is_empty() {
        return "no frames".into();
    }
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let p = |q: usize| v[(v.len() * q / 100).min(v.len() - 1)];
    format!(
        "mean {mean:.2} ms, p50 {:.2}, p95 {:.2}, max {:.2} ({} frames)",
        p(50),
        p(95),
        v[v.len() - 1],
        v.len()
    )
}

impl Perf {
    /// `None` unless `KANTOR_PERF` is set.
    pub(crate) fn new() -> Option<Rc<Self>> {
        if !docs_layout::perf::enabled() {
            return None;
        }
        let frames = std::env::var("KANTOR_PERF_DOCS_SCROLL")
            .ok()
            .and_then(|s| s.parse().ok());
        Some(Rc::new(Self {
            opened: Instant::now(),
            shown: Cell::new(false),
            final_pages: Cell::new(false),
            pending: RefCell::default(),
            render_started: Cell::new(None),
            bench: RefCell::new(frames.map(|frames| Bench {
                frames,
                ..Bench::default()
            })),
        }))
    }

    fn since_open(&self) -> f64 {
        self.opened.elapsed().as_secs_f64() * 1000.0
    }

    pub(crate) fn opened(&self, what: &str) {
        eprintln!("[docs] open {what}: {:.1} ms", self.since_open());
    }

    /// The page count is final (`pages` pages).
    pub(crate) fn pages_final(&self, pages: usize) {
        if !self.final_pages.replace(true) {
            eprintln!(
                "[docs] open pages final: {:.1} ms ({pages} pages)",
                self.since_open()
            );
        }
    }

    /// An input arrived (the first one since the last frame counts).
    pub(crate) fn input(&self, what: impl Into<String>) {
        let mut p = self.pending.borrow_mut();
        if p.is_none() {
            *p = Some((what.into(), Instant::now()));
        }
    }

    pub(crate) fn render_start(&self) {
        self.render_started.set(Some(Instant::now()));
    }

    /// A frame was painted (`pages` pages laid out so far).
    pub(crate) fn painted(&self, pages: usize) {
        let now = Instant::now();
        if pages > 0 && !self.shown.replace(true) {
            eprintln!("[docs] open first page shown: {:.1} ms", self.since_open());
        }
        if let Some((what, at)) = self.pending.borrow_mut().take() {
            eprintln!(
                "[docs] input {what}→paint {:.2} ms",
                (now - at).as_secs_f64() * 1000.0
            );
        }
        if let Some(b) = self.bench.borrow_mut().as_mut()
            && let Some(start) = self.render_started.get()
            && b.last.is_some()
        {
            b.work.push((now - start).as_secs_f64() * 1000.0);
        }
    }
}

impl DocsView {
    /// While `KANTOR_PERF_DOCS_SCROLL` runs: one step per frame (called
    /// from `render`).
    pub(crate) fn perf_bench_step(&mut self, window: &mut Window) {
        let Some(perf) = self.perf.clone() else {
            return;
        };
        perf.render_start();
        if !perf.final_pages.get() || !perf.shown.get() {
            return;
        }
        let mut slot = perf.bench.borrow_mut();
        let Some(b) = slot.as_mut() else {
            return;
        };
        let now = Instant::now();
        if let Some(last) = b.last.replace(now) {
            b.intervals.push((now - last).as_secs_f64() * 1000.0);
        }
        let g = self.geometry();
        let max_y = (g.content_h() - g.view_h()).max(0.0);
        let phase_done = match b.phase {
            0 | 1 => b.step >= b.frames,
            // A jump: the frame that showed it, and one more.
            _ => b.step >= 2,
        };
        if phase_done {
            // The first frames of a phase fill caches.
            let skip = if b.phase < 2 {
                5.min(b.intervals.len())
            } else {
                0
            };
            eprintln!(
                "[docs] bench {} interval: {}",
                PHASES[b.phase],
                stats(b.intervals[skip..].to_vec())
            );
            let skip = if b.phase < 2 { 5.min(b.work.len()) } else { 0 };
            eprintln!(
                "[docs] bench {} work:     {}",
                PHASES[b.phase],
                stats(b.work[skip..].to_vec())
            );
            if let Some(j) = b.jump.take() {
                eprintln!(
                    "[docs] bench {}: {:.2} ms to the frame after it",
                    PHASES[b.phase],
                    (now - j).as_secs_f64() * 1000.0
                );
            }
            b.phase += 1;
            b.step = 0;
            b.intervals.clear();
            b.work.clear();
            b.last = Some(now);
            if b.phase >= PHASES.len() {
                *slot = None;
                return;
            }
        }
        match b.phase {
            0 => self.scroll.y = (self.scroll.y + 120.0).min(max_y),
            1 => {
                let page = g.page_at(self.scroll.y + 1.0) + 1;
                self.scroll.y = g.page_top(page.min(self.laid.paged.pages.len() - 1)) - 10.0;
            }
            2 if b.step == 0 => {
                b.jump = Some(now);
                self.scroll.y = max_y;
            }
            3 if b.step == 0 => {
                b.jump = Some(now);
                let mid = self.laid.paged.pages.len() / 2;
                self.scroll.y = g.page_top(mid) - 10.0;
            }
            _ => {}
        }
        b.step += 1;
        self.clamp_scroll();
        window.request_animation_frame();
    }
}
