//! Resident memory of a big workbook once opened, and its peak while saving
//! (plan.md, "Big workbooks").
//!
//! ```text
//! uv run --with xlsxwriter scripts/corpus/gen_big_xlsx.py ~/.cache/big90k.xlsx
//! KANTOR_MEM_XLSX=~/.cache/big90k.xlsx \
//!     cargo test --release -p sheets_engine --test memory -- --ignored --nocapture
//! ```
//!
//! Linux only (reads `/proc/self/status`), and only meaningful in a release
//! build. Its own test binary, so nothing else has run in the process.

use std::time::Instant;

use sheets_engine::Workbook;

/// The generated 90k-row workbook (16 MB, 810k formulas) took 92 MB here on
/// 2026-11-10 (149 MB before IronCalc's rows were packed and the file's bytes
/// no longer kept); IronCalc's cells are most of it. Some room for allocator
/// and platform differences.
const STEADY_LIMIT_MB: f64 = 115.0;

/// Peak while saving it (IronCalc writes the package in memory, then
/// `preserve` merges the original's extras): 310 MB on 2026-11-10, 1849 MB
/// before the merge stopped parsing whole sheets and IronCalc's writer
/// stopped copying the cells three times.
const SAVE_PEAK_LIMIT_MB: f64 = 420.0;

fn status_mb(key: &str) -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

#[test]
#[ignore]
fn big_workbook_steady_memory() {
    let Some(path) = std::env::var_os("KANTOR_MEM_XLSX") else {
        eprintln!("KANTOR_MEM_XLSX is not set: nothing to measure");
        return;
    };
    let t = Instant::now();
    let book = Workbook::open(&path).unwrap();
    let opened = t.elapsed();
    let peak = status_mb("VmHWM:");
    kantor_mem::release_free_memory();
    let steady = status_mb("VmRSS:");
    eprintln!(
        "opened in {opened:.2?}: {} formula cells; peak {peak:.0} MB, steady {steady:.0} MB \
         (limit {STEADY_LIMIT_MB:.0} MB)",
        book.formula_count()
    );
    if cfg!(target_os = "linux") && !cfg!(debug_assertions) {
        assert!(
            steady <= STEADY_LIMIT_MB,
            "steady RSS {steady:.0} MB over {STEADY_LIMIT_MB:.0} MB"
        );
    }
    let mut book = book;
    book.set_input(0, 0, 0, "edited").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let t = Instant::now();
    book.save_as(dir.path().join("saved.xlsx")).unwrap();
    let peak = status_mb("VmHWM:");
    eprintln!(
        "saved in {:.2?}: peak {peak:.0} MB (limit {SAVE_PEAK_LIMIT_MB:.0} MB), notes {:?}",
        t.elapsed(),
        book.save_notes()
    );
    if cfg!(target_os = "linux") && !cfg!(debug_assertions) {
        assert!(
            peak <= SAVE_PEAK_LIMIT_MB,
            "save peak {peak:.0} MB over {SAVE_PEAK_LIMIT_MB:.0} MB"
        );
    }
    drop(book);
    kantor_mem::release_free_memory();
    eprintln!("closed: {:.0} MB", status_mb("VmRSS:"));
}
