//! Export a workbook to PDF and report the time each step takes and the
//! peak memory (Linux):
//!
//! ```text
//! cargo run --release -p sheets_pdf --example export -- book.xlsx out.pdf
//! ```

use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output] = &args[..] else {
        anyhow::bail!("usage: export INPUT.xlsx OUTPUT.pdf");
    };
    let t = Instant::now();
    let mut book = sheets_engine::Workbook::open(input)?;
    let opened = t.elapsed();
    let rss_open = docs_layout::rss_kib();
    let opts = sheets_pdf::Options::default()
        .now()
        .with_path(Some(std::path::Path::new(input)));
    let t = Instant::now();
    let mut shaper = sheets_pdf::Shaper::new();
    let plan = sheets_pdf::Plan::new(&book, &opts, &mut shaper);
    let planned = t.elapsed();
    let t = Instant::now();
    let mut enc = sheets_pdf::Encoder::new(&plan.title, shaper);
    let mut extract = std::time::Duration::ZERO;
    for i in 0..plan.pages.len() {
        let e = Instant::now();
        let page = sheets_pdf::extract(&mut book, &plan, i);
        extract += e.elapsed();
        enc.page(&page);
    }
    let pages = enc.pages();
    let bytes = enc.finish()?;
    let written = t.elapsed();
    sheets_pdf::write_file(std::path::Path::new(output), &bytes)?;
    println!(
        "{pages} pages, {:.1} MB; open {:.2?}, plan {:.2?}, pages {:.2?} (reading cells {:.2?})",
        bytes.len() as f64 / 1e6,
        opened,
        planned,
        written,
        extract
    );
    if let (Some((rss, _)), Some((_, peak))) = (rss_open, docs_layout::rss_kib()) {
        println!("RSS after open {} MB, peak {} MB", rss / 1024, peak / 1024);
    }
    Ok(())
}
