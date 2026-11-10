//! Opt-in render timing for finding slow frames: set `KANTOR_PERF=1` and
//! renders slower than a millisecond are printed to stderr.

use std::sync::OnceLock;
use std::time::Instant;

pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KANTOR_PERF").is_some())
}

/// Times its scope; prints `name` and the time when dropped, if slow.
pub struct Span {
    name: &'static str,
    start: Option<Instant>,
}

pub fn span(name: &'static str) -> Span {
    Span {
        name,
        start: enabled().then(Instant::now),
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            if ms >= 1.0 {
                eprintln!("[perf] {:<24} {ms:8.2} ms", self.name);
            }
        }
    }
}

/// Per-frame totals of [`frame_timer`] scopes, by name.
#[derive(Default)]
struct FrameTotals {
    /// Time and scopes so far in the current frame; `None` until the first.
    current: std::collections::HashMap<&'static str, (f64, u32)>,
    /// Finished frames' times, kept for [`take_frame_totals`].
    done: std::collections::HashMap<&'static str, Vec<f64>>,
}

thread_local! {
    static FRAMES: std::cell::RefCell<FrameTotals> = Default::default();
}

/// Times a scope that runs many times per frame (e.g. once per grid cell)
/// and adds it to the frame's total for `name`, which is printed when the
/// next frame starts (if at least a millisecond) and kept for
/// [`take_frame_totals`]. Does nothing unless `KANTOR_PERF` is set.
pub fn frame_timer(name: &'static str, window: &gpui_kit::Window) -> FrameTimer {
    if !enabled() {
        return FrameTimer { name, start: None };
    }
    let first = FRAMES.with(|f| {
        let mut f = f.borrow_mut();
        let first = !f.current.contains_key(name);
        f.current.entry(name).or_insert((0.0, 0));
        first
    });
    if first {
        window.on_next_frame(move |_, _| {
            let total = FRAMES.with(|f| {
                let mut f = f.borrow_mut();
                let total = f.current.remove(name);
                if let Some((ms, _)) = total {
                    f.done.entry(name).or_default().push(ms);
                }
                total
            });
            if let Some((ms, n)) = total
                && ms >= 1.0
            {
                eprintln!("[perf] {name:<24} {ms:8.2} ms ({n} scopes)");
            }
        });
    }
    FrameTimer {
        name,
        start: Some(Instant::now()),
    }
}

/// The per-frame totals (ms) of `name` recorded so far, emptying the record.
pub fn take_frame_totals(name: &'static str) -> Vec<f64> {
    FRAMES.with(|f| f.borrow_mut().done.remove(name).unwrap_or_default())
}

pub struct FrameTimer {
    name: &'static str,
    start: Option<Instant>,
}

impl Drop for FrameTimer {
    fn drop(&mut self) {
        if let Some(start) = self.start {
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            FRAMES.with(|f| {
                if let Some(e) = f.borrow_mut().current.get_mut(self.name) {
                    e.0 += ms;
                    e.1 += 1;
                }
            });
        }
    }
}
