//! Times opening, editing one cell and saving an `.xlsx`, with memory.
//!
//! ```text
//! cargo run --release -p sheets_engine --example save_timing -- book.xlsx [OUT.xlsx]
//! ```
//!
//! Prints the time of each step, the resident memory after it and the peak so
//! far (`VmRSS` / `VmHWM` from `/proc/self/status`, Linux only), and the notes
//! the save left. Saves to a temp file unless `OUT.xlsx` is given. Files over
//! 4 MB take the "read the original again at save" path.

use std::time::Instant;

use sheets_engine::Workbook;

fn status_mb(key: &str) -> f64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

fn report(step: &str, t: Instant) {
    eprintln!(
        "{step:<8} {:>7.2} s   rss {:>6.0} MB   peak {:>6.0} MB",
        t.elapsed().as_secs_f64(),
        status_mb("VmRSS:"),
        status_mb("VmHWM:")
    );
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: save_timing BOOK.xlsx [OUT.xlsx]");
    let tmp = tempfile::tempdir()?;
    let out = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| tmp.path().join("saved.xlsx"));

    let t = Instant::now();
    let mut book = Workbook::open(&path)?;
    report("open", t);
    kantor_mem::release_free_memory();

    let t = Instant::now();
    book.set_input(0, 0, 0, "edited")?;
    report("edit", t);

    let t = Instant::now();
    book.save_as(&out)?;
    report("save", t);
    for note in book.save_notes() {
        eprintln!("  note: {note}");
    }

    let t = Instant::now();
    book.save()?;
    report("save 2", t);
    eprintln!(
        "saved {} bytes to {}",
        std::fs::metadata(&out)?.len(),
        out.display()
    );
    Ok(())
}
