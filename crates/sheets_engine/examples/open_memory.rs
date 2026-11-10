//! Memory of a workbook through its life: opened, recalculated, closed.
//!
//! ```text
//! cargo run --release -p sheets_engine --example open_memory -- book.xlsx [--recalc] [--dhat]
//! ```
//!
//! After each step prints the process's resident memory (`VmRSS`) and its
//! peak (`VmHWM`) from `/proc/self/status` (Linux only; zeros elsewhere),
//! then the resident memory once the allocator has given free pages back
//! (`kantor_mem::release_free_memory`, as the app does): the difference is
//! memory freed but kept by the allocator.
//!
//! `--dhat` also counts live heap bytes with dhat (slow: minutes for a big
//! workbook) and writes `dhat-heap.json` in the current directory, viewable at
//! <https://nnethercote.github.io/dh_view/dh_view.html>. A release build has
//! no symbols, so its allocation sites are anonymous; their sizes and block
//! counts still tell rows, hash maps and strings apart.

use std::time::Instant;

use sheets_engine::Workbook;

#[global_allocator]
static ALLOC: dhat::Alloc = dhat::Alloc;

fn status_mb(key: &str) -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

fn report(step: &str, dhat: bool) {
    let rss = status_mb("VmRSS:");
    let peak = status_mb("VmHWM:");
    kantor_mem::release_free_memory();
    let released = status_mb("VmRSS:");
    let heap = if dhat {
        let stats = dhat::HeapStats::get();
        format!(
            "  heap live {:4.0} MB (peak {:.0} MB)",
            stats.curr_bytes as f64 / 1048576.0,
            stats.max_bytes as f64 / 1048576.0
        )
    } else {
        String::new()
    };
    println!("{step:<14} RSS {rss:4.0} MB, {released:4.0} MB released  peak {peak:4.0} MB{heap}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .expect("usage: open_memory <file.xlsx> [--recalc] [--dhat]");
    let dhat = flag("--dhat");
    let _profiler = dhat.then(dhat::Profiler::new_heap);

    report("start", dhat);
    let t = Instant::now();
    let mut book = Workbook::open(path).expect("open");
    println!(
        "opened in {:.2?}: {} formula cells",
        t.elapsed(),
        book.formula_count()
    );
    report("after open", dhat);
    if flag("--recalc") {
        let t = Instant::now();
        book.recalculate();
        println!("recalculated in {:.2?}", t.elapsed());
        report("after recalc", dhat);
    }
    drop(book);
    report("after close", dhat);
}
