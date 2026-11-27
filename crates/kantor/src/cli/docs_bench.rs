//! `kantor docs bench FILE.docx` (not in the usage text): times what the
//! Docs view does with a big document, without a window: open, the first
//! layout, typing (one character, then the view's relayout) on the first
//! page, the middle page and the last page, Find All and Save As (to a
//! copy next to the file, deleted afterwards). Layout goes through
//! [`docs_pdf::LayoutCache`] as in the view. Prints one line per step;
//! with `KANTOR_PERF=1` the layout phases too. `--verify` (slow) checks
//! every layout after an edit against a fresh full layout.

use std::path::Path;
use std::time::Instant;

use anyhow::{Context as _, Result};
use docs_engine::{Document, Position, Selection};

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn pct(mut v: Vec<f64>) -> String {
    v.sort_by(f64::total_cmp);
    let p = |q: usize| v[(v.len() * q / 100).min(v.len() - 1)];
    format!(
        "p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        p(50),
        p(95),
        v[v.len() - 1]
    )
}

/// Peak resident memory (VmHWM), in MB.
fn peak_mb() -> Option<f64> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = s.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: f64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb / 1024.0)
}

/// A paragraph with text on `page`, and an offset in it.
fn spot(doc: &Document, laid: &docs_pdf::Layout, page: usize) -> Option<Position> {
    let p = laid.paged.pages.get(page)?;
    p.lines
        .iter()
        .filter(|l| !l.repeat)
        .map(|l| l.para)
        .find(|&i| i < doc.len() && doc.paragraph(i).text.len() > 20)
        .map(|i| doc.clamp(Position::new(i, 10)))
}

pub fn run(file: &Path, keys: usize, verify: bool) -> Result<()> {
    let t = Instant::now();
    let mut doc = Document::open(file).with_context(|| format!("can't open {}", file.display()))?;
    println!("open (read): {:.1} ms, {} paragraphs", ms(t), doc.len());
    let mut engine = docs_layout::Engine::new();
    let mut cache = docs_pdf::LayoutCache::default();
    let t = Instant::now();
    let mut laid = cache.relayout(&doc, &mut engine, None);
    let pages = laid.paged.pages.len();
    println!("first layout: {:.1} ms, {pages} pages", ms(t));
    let t = Instant::now();
    laid = cache.relayout(&doc, &mut engine, Some(laid));
    println!("layout again (nothing changed): {:.1} ms", ms(t));
    // What the view does on opening a long document: its first pages,
    // and the whole of it on other threads.
    let t = Instant::now();
    let first = docs_pdf::LayoutCache::default().relayout_prefix(&doc, &mut engine, 400);
    println!(
        "first pages only: {:.1} ms, {} pages",
        ms(t),
        first.paged.pages.len()
    );
    engine.fonts.source_cache.make_shared();
    let fonts = engine.fonts.clone();
    let t = Instant::now();
    let bg = docs_pdf::cache::lay_out_parallel(&doc, &fonts);
    println!(
        "whole layout on several threads: {:.1} ms, {} pages",
        ms(t),
        bg.pages()
    );
    let mut fresh_engine = docs_layout::Engine::new();
    let mut mismatches = 0;

    for (name, page) in [
        ("page 1", 0),
        ("middle page", pages / 2),
        ("last page", pages.saturating_sub(1)),
    ] {
        // That page, else the nearest before it with text.
        let Some(mut at) = (0..=page).rev().find_map(|p| spot(&doc, &laid, p)) else {
            println!("typing on {name}: no text there");
            continue;
        };
        let mut times = Vec::with_capacity(keys);
        let mut quick = 0;
        for k in 0..keys {
            let t = Instant::now();
            // Mostly letters, a space now and then.
            let ch = if k % 6 == 5 { " " } else { "x" };
            let sel = doc.insert_text(Selection::caret(at), ch, None);
            at = sel.head;
            laid = cache.relayout(&doc, &mut engine, Some(laid));
            times.push(ms(t));
            if matches!(cache.outcome, docs_pdf::cache::Outcome::Quick { .. }) {
                quick += 1;
            }
            if verify {
                let full = docs_pdf::lay_out(&doc, &mut fresh_engine);
                if let Some(d) = docs_pdf::cache::difference(&laid, &full) {
                    mismatches += 1;
                    println!("  MISMATCH after key {k} on {name}: {d}");
                }
            }
        }
        println!(
            "typing on {name} (page {}, {keys} keys, edit + relayout, {quick} quick): {}",
            page + 1,
            pct(times)
        );
        // Enter and Backspace there: a paragraph more, then one less.
        let mut times = Vec::new();
        for _ in 0..3 {
            let t = Instant::now();
            let sel = doc.insert_text(Selection::caret(at), "\n", None);
            at = sel.head;
            laid = cache.relayout(&doc, &mut engine, Some(laid));
            times.push(ms(t));
            let t = Instant::now();
            let sel = doc.backspace(Selection::caret(at));
            at = sel.head;
            laid = cache.relayout(&doc, &mut engine, Some(laid));
            times.push(ms(t));
        }
        println!("Enter / Backspace on {name}: {}", pct(times));
    }

    let t = Instant::now();
    let opts = kantor_core::find::FindOptions::default();
    let n: usize = doc
        .paragraphs()
        .iter()
        .map(|p| kantor_core::find::find_all(&p.text, "the", opts).len())
        .sum();
    println!("find all \"the\": {:.1} ms, {n} matches", ms(t));

    let out = file.with_extension("bench-save.docx");
    let t = Instant::now();
    doc.save_as(&out)?;
    println!("save: {:.1} ms", ms(t));
    std::fs::remove_file(&out).ok();
    if verify {
        println!("verify: {mismatches} mismatches");
    }
    if let Some(mb) = peak_mb() {
        println!("peak memory: {mb:.0} MB");
    }
    Ok(())
}
