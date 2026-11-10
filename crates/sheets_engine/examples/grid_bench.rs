//! Times what the Sheets grid asks the engine for each visible cell, per frame.
//!
//!     cargo run --release -p sheets_engine --example grid_bench -- BOOK.xlsx [SHEET]
//!
//! Scrolls a 45 x 16 window down the sheet in steps of 3 rows (like a fast
//! wheel scroll) and prints the mean and p95 time of one window.

use std::time::Instant;

use sheets_engine::Workbook;

const ROWS: u32 = 45;
const COLS: u32 = 16;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: grid_bench BOOK.xlsx [SHEET]");
    let sheet: u32 = args.next().map_or(0, |s| s.parse().unwrap());
    let t = Instant::now();
    let book = Workbook::open(std::path::Path::new(&path))?;
    eprintln!("opened in {:.1} s", t.elapsed().as_secs_f64());
    let (used_rows, _) = book.used_size(sheet);
    run("style + align", &book, sheet, used_rows, frame_cell);
    run("cell look (grid)", &book, sheet, used_rows, frame_look);
    Ok(())
}

fn run(
    name: &str,
    book: &Workbook,
    sheet: u32,
    used_rows: u32,
    cell: fn(&Workbook, u32, u32, u32) -> usize,
) {
    let mut times = Vec::new();
    let mut sink = 0usize;
    let mut top = 0;
    while top + ROWS < used_rows && times.len() < 3000 {
        let t = Instant::now();
        for row in top..top + ROWS {
            for col in 0..COLS {
                sink += cell(book, sheet, row, col);
            }
        }
        times.push(t.elapsed().as_secs_f64() * 1000.0);
        top += 3 + (times.len() as u32 % 7) * 40;
    }
    times.sort_by(f64::total_cmp);
    let mean = times.iter().sum::<f64>() / times.len() as f64;
    let p95 = times[times.len() * 95 / 100];
    println!(
        "{name:<18} {} windows of {ROWS}x{COLS}: mean {mean:.3} ms, p95 {p95:.3} ms (sink {sink})",
        times.len()
    );
}

/// The per-cell engine calls of `SheetGrid::render_td`.
fn frame_cell(book: &Workbook, sheet: u32, row: u32, col: u32) -> usize {
    let text = book.display_value(sheet, row, col);
    let pending = text.is_empty() && book.is_pending(sheet, row, col);
    let style = book.cell_style(sheet, row, col);
    let align = book.display_align(sheet, row, col);
    text.len() + pending as usize + style.bold as usize + align as usize
}

/// What `render_td` asks for now: the look (style, merge, shared borders,
/// gridlines), then the value and alignment of the cell (or merge anchor).
fn frame_look(book: &Workbook, sheet: u32, row: u32, col: u32) -> usize {
    let look = book.cell_look(sheet, row, col);
    let (r, c) = look.merge.map_or((row, col), |m| (m.top, m.left));
    let text = book.display_value(sheet, r, c);
    let pending = text.is_empty() && book.is_pending(sheet, r, c);
    let align = book.display_align_of(&look.style, sheet, r, c);
    text.len() + pending as usize + look.grid_right as usize + align as usize
}
