//! Text layout spike benchmark (plan.md §6 M0, docs/decisions/0001-text-engine.md).
//!
//! `cargo run --release -p docs_layout --example bench -- [min_pages]`
//!
//! Lays out ~20 A4 pages of mixed Indonesian / CJK / Arabic text with mixed
//! runs and reports pages, lines, timings and memory. No window is opened.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use docs_layout::{Engine, PageSetup, PagedDocument, rss_kib, sample};

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn main() {
    let (rss0, _) = rss_kib().unwrap_or_default();

    let t = Instant::now();
    let mut engine = Engine::new();
    let font_cx = t.elapsed();

    let setup = PageSetup::a4();
    let min_pages: usize = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    // Find the number of sections that fills at least `min_pages` pages.
    let mut sections = 3 * min_pages;
    let paras = loop {
        let paras = sample::document(sections);
        if engine.layout_document(&paras, setup).pages.len() >= min_pages {
            break paras;
        }
        sections += 10;
    };
    let chars: usize = paras.iter().map(|p| p.text.chars().count()).sum();

    // Cold-ish: first full layout after warming the shaping caches once above.
    let runs = 10;
    let mut full = Vec::new();
    let mut doc = None;
    for _ in 0..runs {
        let t = Instant::now();
        let d = engine.layout_document(&paras, setup);
        full.push(t.elapsed());
        doc = Some(d);
    }
    let doc = doc.expect("ran at least once");
    let pages = doc.pages.len();
    let lines = doc.line_count();

    // Edit one paragraph: reshape it, then repaginate everything.
    let mut edit = Vec::new();
    let mut doc = doc;
    let target = paras.len() / 2;
    for _ in 0..runs {
        let t = Instant::now();
        let PagedDocument {
            mut layouts, setup, ..
        } = doc;
        layouts[target] = engine.layout_paragraph(&paras[target], setup.content_width());
        doc = PagedDocument::paginate(&paras, layouts, setup);
        edit.push(t.elapsed());
    }

    // Repaginate only (e.g. margins of a later section changed height).
    let mut repag = Vec::new();
    for _ in 0..runs {
        let t = Instant::now();
        let PagedDocument { layouts, setup, .. } = doc;
        doc = PagedDocument::paginate(&paras, layouts, setup);
        repag.push(t.elapsed());
    }

    // Glyph extraction for painting one page, and caret hit-testing.
    let t = Instant::now();
    let mut glyphs = 0;
    let mut notdef = 0;
    let mut fonts = HashSet::new();
    for page in 0..pages {
        for run in doc.glyph_runs(page) {
            fonts.insert((run.font.data.id(), run.font.index));
            glyphs += run.glyphs.len();
            notdef += run.glyphs.iter().filter(|g| g.id == 0).count();
        }
    }
    let extract = t.elapsed();

    let t = Instant::now();
    let mut hits = 0;
    for page in 0..pages {
        for i in 0..100 {
            let y = setup.margin_top + setup.content_height() * (i as f32 / 100.0);
            if doc.hit_test(page, setup.width / 2.0, y).is_some() {
                hits += 1;
            }
        }
    }
    let hit = t.elapsed();

    let (rss1, hwm) = rss_kib().unwrap_or_default();

    println!("text engine spike: parley 0.12, A4 @ 96 dpi, 2.54 cm margins");
    println!(
        "  document     {} paragraphs, {} chars, {} lines, {} pages",
        paras.len(),
        chars,
        lines,
        pages
    );
    println!(
        "  font context {:>8.2?}  (system font scan, once per process)",
        font_cx
    );
    println!(
        "  full layout  {:>8.2?}  median of {runs}  ({:.2?} per 20 pages)",
        median(full.clone()),
        median(full).mul_f64(20.0 / pages as f64)
    );
    println!(
        "  edit 1 para  {:>8.2?}  reshape one paragraph + repaginate",
        median(edit)
    );
    println!("  repaginate   {:>8.2?}", median(repag));
    println!(
        "  glyph runs   {:>8.2?}  all pages: {glyphs} glyphs, {} fonts, {notdef} .notdef",
        extract,
        fonts.len()
    );
    println!(
        "  hit-test     {:>8.2?}  {hits} points ({:.2?} each)",
        hit,
        hit / hits.max(1)
    );
    println!(
        "  memory       RSS {:.1} MB -> {:.1} MB, peak {:.1} MB",
        rss0 as f64 / 1024.0,
        rss1 as f64 / 1024.0,
        hwm as f64 / 1024.0
    );
    for (i, page) in doc.pages.iter().take(3).enumerate() {
        let first = page.lines.first().map(|l| (l.para, l.line));
        let last = page.lines.last().map(|l| (l.para, l.line));
        println!(
            "  page {:>2}      {} lines, first (para, line) {:?}, last {:?}",
            i + 1,
            page.lines.len(),
            first,
            last
        );
    }
}
