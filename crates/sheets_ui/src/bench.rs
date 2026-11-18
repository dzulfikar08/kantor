//! Scroll benchmark for the grid: with `KANTOR_PERF=1 KANTOR_PERF_SCROLL=N`,
//! once a workbook is shown the grid scrolls down 3 rows every frame for `N`
//! frames, then prints the frame intervals and the time spent building cell
//! elements per frame (`render_td`, see `kantor_ui::perf::frame_timer`),
//! jumps to the last used row (as Ctrl+End does) and prints how long that
//! frame took, and quits. It also prints the time from creating the view to
//! the first frame showing the workbook (opening the file). The window must
//! be visible (a hidden GPUI window doesn't paint).

use std::time::Instant;

use gpui_kit::*;

use crate::SheetsView;

pub(crate) struct ScrollBench {
    frames: usize,
    row: usize,
    last: Option<Instant>,
    intervals: Vec<f64>,
    created: Instant,
    /// When the jump to the end was asked for, and the frames since.
    jumped: Option<(Instant, u32)>,
    /// When this frame's render started, and each frame's time from there
    /// to the end of painting ([`SheetsView::bench_probe`]).
    started: std::rc::Rc<std::cell::Cell<Option<Instant>>>,
    work: std::rc::Rc<std::cell::RefCell<Vec<f64>>>,
    /// The same to the end of prepaint (layout and prepaint).
    pre: std::rc::Rc<std::cell::RefCell<Vec<f64>>>,
}

impl ScrollBench {
    pub(crate) fn from_env() -> Option<Self> {
        let frames = std::env::var("KANTOR_PERF_SCROLL").ok()?.parse().ok()?;
        Some(Self {
            frames,
            row: 0,
            last: None,
            intervals: Vec::new(),
            created: Instant::now(),
            jumped: None,
            started: Default::default(),
            work: Default::default(),
            pre: Default::default(),
        })
    }
}

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

impl SheetsView {
    /// While benchmarking: an empty element painted last, which records the
    /// time from the start of the frame's render to the end of its paint.
    pub(crate) fn bench_probe(&self) -> Option<impl IntoElement> {
        let bench = self.bench.as_ref()?;
        let (started, work) = (bench.started.clone(), bench.work.clone());
        let (started2, pre) = (bench.started.clone(), bench.pre.clone());
        Some(
            canvas(
                move |_, _, _| {
                    if let Some(t) = started2.get() {
                        pre.borrow_mut().push(t.elapsed().as_secs_f64() * 1000.0);
                    }
                },
                move |_, _, _, _| {
                    if let Some(t) = started.take() {
                        work.borrow_mut().push(t.elapsed().as_secs_f64() * 1000.0);
                    }
                },
            )
            .absolute()
            .size_0(),
        )
    }

    /// One benchmark step per frame (called from `render` while a workbook
    /// is shown).
    pub(crate) fn bench_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.as_mut() else {
            return;
        };
        let now = Instant::now();
        bench.started.set(Some(now));
        if bench.last.is_none() {
            eprintln!(
                "[bench] open: {:.0} ms to the first frame of the workbook",
                (now - bench.created).as_secs_f64() * 1000.0
            );
        }
        if let Some((jumped, frames)) = bench.jumped.as_mut() {
            // Frame 1 builds the jumped-to rows; frame 2 starts once it's done.
            *frames += 1;
            if *frames < 2 {
                window.request_animation_frame();
                return;
            }
            eprintln!(
                "[bench] jump to the last row: {:.2} ms until the frame after it",
                (now - *jumped).as_secs_f64() * 1000.0
            );
            self.bench = None;
            cx.quit();
            return;
        }
        if let Some(last) = bench.last.replace(now) {
            bench.intervals.push((now - last).as_secs_f64() * 1000.0);
        }
        if bench.intervals.len() >= bench.frames {
            // The first frames load fonts and fill caches.
            let skip = 10.min(bench.intervals.len());
            let cells = kantor_ui::perf::take_frame_totals("sheets cells");
            let cells = cells[10.min(cells.len())..].to_vec();
            eprintln!(
                "[bench] frame interval: {}",
                stats(bench.intervals[skip..].to_vec())
            );
            eprintln!("[bench] cell elements:  {}", stats(cells));
            let work = bench.work.borrow();
            eprintln!(
                "[bench] frame work:     {}",
                stats(work[10.min(work.len())..].to_vec())
            );
            drop(work);
            let pre = bench.pre.borrow();
            eprintln!(
                "[bench] to prepaint:    {}",
                stats(pre[10.min(pre.len())..].to_vec())
            );
            drop(pre);
            bench.jumped = Some((Instant::now(), 0));
            let last = {
                let grid = self.table.read(cx).delegate();
                grid.book.used_size(grid.sheet).0.saturating_sub(1)
            };
            self.select_cell(last, 0, cx);
            window.request_animation_frame();
            return;
        }
        bench.row += 3;
        let row = bench.row;
        self.table.update(cx, |table, cx| {
            table
                .vertical_scroll_handle
                .scroll_to_item(row, ScrollStrategy::Top);
            cx.notify();
        });
        window.request_animation_frame();
    }
}
