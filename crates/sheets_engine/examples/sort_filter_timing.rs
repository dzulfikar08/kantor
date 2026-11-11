//! Time sorting and filtering a big table, e.g. the workbook from
//! `scripts/corpus/gen_big_xlsx.py --table` (Orders is a 90 000-row table).
//!
//!     cargo run --release -p sheets_engine --example sort_filter_timing -- big.xlsx

use std::time::Instant;

use sheets_engine::{Condition, FilterKind, FilterOp, FilterScope, Workbook};

fn main() {
    let path = std::env::args().nth(1).expect("usage: sort_filter_timing <file.xlsx>");
    let t = Instant::now();
    let mut wb = Workbook::open(&path).expect("open");
    println!("open: {:.2?}", t.elapsed());
    let table = wb.tables(0).into_iter().next().expect("a table on the first sheet");
    let rows = table.data_rows().1 - table.data_rows().0 + 1;
    println!("table {} with {rows} data rows, calc mode {:?}", table.name, wb.calc_mode());
    let amount = table.area.left + 6;
    let region = table.area.left + 8;
    let scope = FilterScope::Table(table.name.clone());

    let t = Instant::now();
    let n = wb.sort_table(0, &table.name, amount, true).expect("sort");
    println!("sort {n} rows by Amount, descending: {:.2?}", t.elapsed());
    let t = Instant::now();
    wb.undo().unwrap();
    println!("undo the sort: {:.2?}", t.elapsed());

    let t = Instant::now();
    let values = wb.filter_values(0, &scope, region);
    println!("filter menu values of Region ({}): {:.2?}", values.len(), t.elapsed());
    let t = Instant::now();
    let (shown, total) = wb
        .set_column_filter(
            0,
            &scope,
            region,
            Some(FilterKind::Values {
                values: vec![values[0].text.clone()],
                blanks: false,
                dates: vec![],
            }),
        )
        .expect("filter");
    println!("filter Region = {:?}: {shown} of {total} shown, {:.2?}", values[0].text, t.elapsed());
    let t = Instant::now();
    let (shown, _) = wb
        .set_column_filter(
            0,
            &scope,
            amount,
            Some(FilterKind::Custom {
                and: false,
                conditions: vec![Condition {
                    op: FilterOp::GreaterThan,
                    value: "100000000".into(),
                }],
            }),
        )
        .expect("filter");
    println!("and Amount > 100,000,000: {shown} shown, {:.2?}", t.elapsed());
    let t = Instant::now();
    wb.clear_filter(0, &scope).unwrap();
    println!("clear the filter: {:.2?}", t.elapsed());
    let t = Instant::now();
    wb.undo().unwrap();
    println!("undo the clear: {:.2?}", t.elapsed());
}
