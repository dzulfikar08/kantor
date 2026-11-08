//! Times loading and evaluating an `.xlsx` with IronCalc, and compares the
//! values IronCalc computes with the values Excel cached in the file.
//!
//! ```text
//! cargo run --release -p sheets_engine --example eval_timing -- book.xlsx [max_mismatches_to_list]
//! ```
//!
//! The path can also come from `KANTOR_EVAL_XLSX`. Mismatches are listed by
//! address and formula only; values are printed with `--values`. `--dump=PATH`
//! writes every formula cell's computed value to PATH.

use std::collections::BTreeMap;
use std::time::Instant;

use ironcalc::base::cell::CellValue;
use ironcalc::import::load_from_xlsx;

fn peak_rss_mb() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    status
        .lines()
        .find(|l| l.starts_with("VmHWM:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|kb| kb.parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

fn same(a: &CellValue, b: &CellValue) -> bool {
    match (a, b) {
        (CellValue::Number(x), CellValue::Number(y)) => {
            x == y || (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0)
        }
        (CellValue::None, CellValue::String(s)) | (CellValue::String(s), CellValue::None) => {
            s.is_empty()
        }
        _ => a == b,
    }
}

fn column_name(mut column: i32) -> String {
    let mut name = Vec::new();
    while column > 0 {
        let rem = (column - 1) % 26;
        name.push(b'A' + rem as u8);
        column = (column - 1) / 26;
    }
    name.reverse();
    String::from_utf8(name).unwrap_or_default()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let show_values = args.iter().any(|a| a == "--values");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let path = positional
        .first()
        .map(|s| s.to_string())
        .or_else(|| std::env::var("KANTOR_EVAL_XLSX").ok())
        .expect("usage: eval_timing <file.xlsx> [max_listed]");
    let max_listed: usize = positional.get(1).and_then(|s| s.parse().ok()).unwrap_or(20);

    let start = Instant::now();
    let mut model = load_from_xlsx(&path, "en", "UTC", "en").expect("load");
    println!(
        "load: {:.2}s  (peak RSS {:.0} MB)",
        start.elapsed().as_secs_f64(),
        peak_rss_mb()
    );

    // Excel's cached values of every formula cell, before IronCalc touches them.
    let mut cached = Vec::new();
    for (sheet, worksheet) in model.workbook.worksheets.iter().enumerate() {
        for (row, column, cell) in worksheet.sheet_data.cells() {
            if cell.has_formula() {
                cached.push((sheet as u32, row, column));
            }
        }
    }
    let before: Vec<CellValue> = cached
        .iter()
        .map(|&(s, r, c)| model.get_cell_value_by_index(s, r, c).unwrap())
        .collect();
    println!("formula cells: {}", cached.len());

    let start = Instant::now();
    model.evaluate();
    println!(
        "evaluate: {:.2}s  (peak RSS {:.0} MB)",
        start.elapsed().as_secs_f64(),
        peak_rss_mb()
    );

    // `--dump=PATH`: every formula cell's value after evaluation, one per line,
    // to compare two builds of the engine (`diff`).
    if let Some(dump) = args.iter().find_map(|a| a.strip_prefix("--dump=")) {
        let mut out = String::new();
        for &(s, r, c) in &cached {
            let value = model.get_cell_value_by_index(s, r, c).unwrap();
            out.push_str(&format!("{s}\t{r}\t{c}\t{value:?}\n"));
        }
        std::fs::write(dump, out).expect("write dump");
    }

    let names: Vec<String> = model
        .workbook
        .worksheets
        .iter()
        .map(|w| w.name.clone())
        .collect();
    // `--sheet=NAME`: list mismatches of that sheet only.
    let only_sheet = args.iter().find_map(|a| a.strip_prefix("--sheet="));
    let mut by_function: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut by_column: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    let mut listed = 0;
    for (&(s, r, c), old) in cached.iter().zip(&before) {
        let new = model.get_cell_value_by_index(s, r, c).unwrap();
        let formula = model
            .get_cell_formula(s, r, c)
            .ok()
            .flatten()
            .unwrap_or_default();
        let function: String = formula
            .trim_start_matches('=')
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '.' || *ch == '_')
            .collect();
        let function = if formula
            .trim_start_matches('=')
            .starts_with(&format!("{function}("))
        {
            function
        } else {
            "(expression)".to_string()
        };
        let entry = by_function.entry(function).or_default();
        entry.0 += 1;
        let column_entry = by_column
            .entry(format!("{}!{}", names[s as usize], column_name(c)))
            .or_default();
        column_entry.0 += 1;
        if !same(old, &new) {
            entry.1 += 1;
            column_entry.1 += 1;
            if listed < max_listed && only_sheet.is_none_or(|name| name == names[s as usize]) {
                listed += 1;
                let address = format!("{}!{}{}", names[s as usize], column_name(c), r);
                if show_values {
                    println!("  mismatch {address}: {formula}  excel={old:?} ironcalc={new:?}");
                } else {
                    println!("  mismatch {address}: {formula}");
                }
            }
        }
    }
    let total: usize = by_function.values().map(|v| v.1).sum();
    println!("mismatches: {total} of {}", cached.len());
    let mut by_function: Vec<_> = by_function.into_iter().filter(|(_, v)| v.1 > 0).collect();
    by_function.sort_by_key(|(_, (_, bad))| std::cmp::Reverse(*bad));
    for (function, (count, bad)) in by_function.iter().take(25) {
        println!("  {function:<16} {bad} of {count}");
    }
    let mut by_column: Vec<_> = by_column.into_iter().filter(|(_, v)| v.1 > 0).collect();
    by_column.sort_by_key(|(_, (_, bad))| std::cmp::Reverse(*bad));
    println!("by column:");
    for (column, (count, bad)) in by_column.iter().take(25) {
        println!("  {column:<16} {bad} of {count}");
    }
}
