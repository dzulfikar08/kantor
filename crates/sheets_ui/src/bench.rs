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
    /// One benchmark step per frame (called from `render` while a workbook
    /// is shown).
    pub(crate) fn bench_step(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bench) = self.bench.as_mut() else {
            return;
        };
        let now = Instant::now();
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
