//! Conditional formatting timing: what the grid pays per frame.
//!
//!     uv run --with xlsxwriter scripts/corpus/gen_big_xlsx.py ~/.cache/big-cf.xlsx --cf
//!     cargo run --release -p sheets_engine --example cf_timing -- ~/.cache/big-cf.xlsx
//!
//! Opens the workbook, then asks for the conditional formats of a screenful
//! of cells (45 rows x 19 columns) the way the grid does: first frame (rules
//! compiled, range aggregates read), the same frame again (cached), a
//! screenful further down, and the first frame after an edit (everything
//! worked out again for the new state). Ends with a save.

use std::time::Instant;

use sheets_engine::Workbook;

fn frame(book: &mut Workbook, top: u32) -> (f64, usize) {
    let started = Instant::now();
    let mut hits = 0;
    for row in top..top + 45 {
        for col in 0..19 {
            hits += usize::from(book.conditional(0, row, col).is_some());
        }
    }
    (started.elapsed().as_secs_f64() * 1000.0, hits)
}

fn main() -> anyhow::Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("usage: cf_timing <file.xlsx>");
    let started = Instant::now();
    let mut book = Workbook::open(&path)?;
    println!("open: {:.2?}", started.elapsed());
    let report = |what: &str, (ms, hits): (f64, usize)| {
        println!("{what}: {ms:.2} ms ({hits} of 855 cells formatted)");
    };
    report("first frame (rules + aggregates)", frame(&mut book, 1));
    report("same frame again", frame(&mut book, 1));
    report("frame at row 45 000", frame(&mut book, 45_000));
    book.set_input(0, 1, 4, "39")?;
    report("first frame after an edit", frame(&mut book, 1));
    report("next frame", frame(&mut book, 2));
    // Next to the input (/tmp may be a small tmpfs).
    let out = std::path::Path::new(&path).with_extension("cf_timing_saved.xlsx");
    let started = Instant::now();
    book.save_as(&out)?;
    println!(
        "save: {:.2?} (notes: {:?})",
        started.elapsed(),
        book.save_notes()
    );
    std::fs::remove_file(out).ok();
    Ok(())
}
