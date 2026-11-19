//! Timing on a real file: `cargo run --release -p pdf_engine --example bench -- FILE.pdf`.
//! Prints open time, per-page render time at 100 % and 200 % (96 dpi), and
//! text extraction + search over every page.

use std::sync::Arc;
use std::time::Instant;

use pdf_engine::{PdfDoc, Renderer, Turn, find_in};

fn main() {
    let path = std::env::args().nth(1).expect("usage: bench FILE.pdf [QUERY]");
    let query = std::env::args().nth(2).unwrap_or_else(|| "the".into());
    let data = Arc::new(std::fs::read(&path).expect("read"));
    let t = Instant::now();
    let doc = PdfDoc::open(data, "").expect("open");
    let n = doc.page_count();
    println!("open: {:.1} ms, {n} pages", ms(t));
    let t = Instant::now();
    let outline = doc.outline();
    println!("outline: {:.1} ms, {} top items", ms(t), outline.len());
    let r = Renderer::new(&doc);
    for (label, scale) in [("100%", 96.0 / 72.0), ("200%", 2.0 * 96.0 / 72.0)] {
        let pages: Vec<usize> = (0..n).step_by((n / 20).max(1)).collect();
        let mut times: Vec<f64> = pages
            .iter()
            .map(|&p| {
                let t = Instant::now();
                r.render(p, scale, Turn(0)).expect("render");
                ms(t)
            })
            .collect();
        times.sort_by(f64::total_cmp);
        println!(
            "render {label}: median {:.1} ms, max {:.1} ms ({} pages)",
            times[times.len() / 2],
            times[times.len() - 1],
            times.len()
        );
    }
    let t = Instant::now();
    let mut hits = 0;
    let mut chars = 0;
    for p in 0..n {
        let text = doc.text(p);
        chars += text.text.len();
        hits += find_in(p, &text, &query, false, false).len();
    }
    println!(
        "text + search \"{query}\": {:.0} ms for {n} pages ({chars} bytes, {hits} hits)",
        ms(t)
    );
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}
