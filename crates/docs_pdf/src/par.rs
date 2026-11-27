//! Work spread over threads for long documents (layout paragraphs built,
//! keyed and shaped on several threads; short ones stay on one).

/// Below this many items, one thread.
pub(crate) const MIN_ITEMS: usize = 2048;

/// Threads to use: the machine's, one left for the UI, at most 8.
pub(crate) fn threads() -> usize {
    // Asked once: it reads the cgroup limits (slow next to a small layout).
    static THREADS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *THREADS.get_or_init(|| {
        std::thread::available_parallelism()
            .map_or(4, |n| n.get())
            .saturating_sub(1)
            .clamp(1, 8)
    })
}

/// `f(0)`, `f(1)`, … `f(n - 1)` in order, worked out on several threads
/// when `n` is large.
pub(crate) fn map<T: Send>(n: usize, f: &(dyn Fn(usize) -> T + Sync)) -> Vec<T> {
    if n < MIN_ITEMS || threads() < 2 {
        return (0..n).map(f).collect();
    }
    let threads = threads();
    let chunk = n.div_ceil(threads);
    std::thread::scope(|s| {
        let parts: Vec<_> = (0..n)
            .step_by(chunk)
            .map(|start| {
                s.spawn(move || (start..(start + chunk).min(n)).map(f).collect::<Vec<T>>())
            })
            .collect();
        let mut out = Vec::with_capacity(n);
        for (k, p) in parts.into_iter().enumerate() {
            match p.join() {
                Ok(v) => out.extend(v),
                // A panic in a worker: do its part here (and panic the
                // same way if it does).
                Err(_) => out.extend((k * chunk..((k + 1) * chunk).min(n)).map(f)),
            }
        }
        out
    })
}
