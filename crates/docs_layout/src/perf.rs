//! Opt-in timing of the Docs layout phases: with `KANTOR_PERF` set,
//! [`span`] prints each phase that took at least a millisecond to stderr
//! (`[perf] docs …`). Shared by docs_pdf, docs_ui and `kantor docs bench`.

use std::sync::OnceLock;
use std::time::Instant;

/// `KANTOR_PERF` is set.
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("KANTOR_PERF").is_some())
}

/// Times its scope; prints `name` and the time when dropped, if slow.
#[must_use]
pub struct Span {
    name: &'static str,
    start: Option<Instant>,
}

/// A timed scope (does nothing unless [`enabled`]).
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
                eprintln!("[perf] {:<28} {ms:8.2} ms", self.name);
            }
        }
    }
}
