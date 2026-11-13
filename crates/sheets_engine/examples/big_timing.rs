//! Times what makes a big workbook feel slow: opening it, sorting it,
//! filling formulas down and editing a cell that formulas read. Made for
//! the workbook of `scripts/corpus/gen_big_xlsx.py` (90 000 orders on the
//! first sheet, nine formula columns G:O; `--table` makes them a table).
//!
//!     uv run --with xlsxwriter scripts/corpus/gen_big_xlsx.py big.xlsx --table
//!     cargo run --release -p sheets_engine --example big_timing -- big.xlsx [runs] [fill rows]
//!
//! Each step after the open runs `runs` times (default 3) and is undone in
//! between; the best and the worst time are printed, and on Linux the least
//! CPU time a step took (steadier than the clock on a busy machine). The
//! fill goes down 1000 rows unless told otherwise.

use std::time::{Duration, Instant};

use sheets_engine::{CalcMode, CellRange, Fill, FillDirection, SortKey, Workbook};

/// Time this thread has spent on a CPU (Linux), which waiting for one on a
/// busy machine doesn't add to; `None` elsewhere.
fn cpu_time() -> Option<Duration> {
    let stat = std::fs::read_to_string("/proc/thread-self/schedstat").ok()?;
    let ns = stat.split_whitespace().next()?.parse().ok()?;
    Some(Duration::from_nanos(ns))
}

fn timed(name: &str, runs: usize, mut step: impl FnMut(), mut undo: impl FnMut()) {
    let mut times: Vec<Duration> = Vec::new();
    let mut cpu: Vec<Duration> = Vec::new();
    let mut undos: Vec<Duration> = Vec::new();
    for _ in 0..runs {
        let c = cpu_time();
        let t = Instant::now();
        step();
        times.push(t.elapsed());
        if let (Some(before), Some(after)) = (c, cpu_time()) {
            cpu.push(after - before);
        }
        let t = Instant::now();
        undo();
        undos.push(t.elapsed());
    }
    let span = |v: &[Duration]| {
        let min = v.iter().min().copied().unwrap_or_default();
        let max = v.iter().max().copied().unwrap_or_default();
        format!("{min:>9.2?} (worst {max:.2?})")
    };
    let cpu = match cpu.iter().min() {
        Some(min) => format!("   cpu {min:.2?}"),
        None => String::new(),
    };
    println!(
        "{name:<44} {}   undo {}{cpu}",
        span(&times),
        span(&undos)
    );
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: big_timing <file.xlsx> [runs]");
    let runs: usize = args.next().and_then(|n| n.parse().ok()).unwrap_or(3);
    let fill_rows: u32 = args.next().and_then(|n| n.parse().ok()).unwrap_or(1000);

    let t = Instant::now();
    let wb = Workbook::open(&path).expect("open");
    let (rows, cols) = wb.used_size(0);
    println!(
        "open: {:.2?} ({rows} rows x {cols} columns on the first sheet, {} formulas, {:?})",
        t.elapsed(),
        wb.formula_count(),
        wb.calc_mode()
    );
    let last = rows - 1;
    let amount = 6;

    // Sort the order list by Amount, descending (a table if there is one).
    let table = wb.tables(0).into_iter().next();
    let wb = std::cell::RefCell::new(wb);
    timed(
        "sort 90k rows by Amount",
        runs,
        || {
            let mut wb = wb.borrow_mut();
            match &table {
                Some(t) => wb.sort_table(0, &t.name, amount, true).map(|_| ()),
                None => wb
                    .sort_range(
                        0,
                        CellRange::from_corners((1, 0), (last, cols - 1)),
                        &[SortKey {
                            col: amount,
                            descending: true,
                        }],
                        false,
                    )
                    .map(|_| ()),
            }
            .expect("sort")
        },
        || {
            wb.borrow_mut().undo().expect("undo");
        },
    );

    // The last order's formulas (G:O) filled down `fill_rows` rows.
    let fill = Fill {
        source: CellRange::from_corners((last, 6), (last, 14)),
        direction: FillDirection::Down,
        to: last + fill_rows,
    };
    // Manual mode: the sort above left work for the next recalculation,
    // which the first fill would do. Start from up-to-date values.
    let t = Instant::now();
    wb.borrow_mut().recalculate();
    println!("recalculate everything (F9): {:.2?}", t.elapsed());
    for mode in [CalcMode::Manual, CalcMode::Automatic] {
        wb.borrow_mut().set_calc_mode(mode);
        timed(
            &format!("fill 9 formulas down {fill_rows} rows ({mode:?})"),
            runs,
            || {
                wb.borrow_mut().fill(0, &fill).expect("fill");
            },
            || {
                wb.borrow_mut().undo().expect("undo");
            },
        );
    }

    // A quantity in the middle: its row's formulas and the summary read it.
    wb.borrow_mut().set_calc_mode(CalcMode::Automatic);
    timed(
        "edit one Qty (Automatic)",
        runs,
        || {
            wb.borrow_mut()
                .set_input(0, rows / 2, 4, "7")
                .expect("edit")
        },
        || {
            wb.borrow_mut().undo().expect("undo");
        },
    );
}
