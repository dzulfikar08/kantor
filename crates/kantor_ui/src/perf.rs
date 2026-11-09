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
