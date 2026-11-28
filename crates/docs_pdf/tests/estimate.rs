//! The page count a long document is given while its first pages are laid
//! out and the rest is still being laid out
//! ([`docs_pdf::pages::estimate_pages`]): a contents list of short lines
//! at the start mustn't make it look much longer than it is.

use docs_engine::{Document, Selection};
use docs_layout::Engine;
use docs_pdf::LayoutCache;
use docs_pdf::pages::{EstPara, estimate_from_heights, height_of};

fn para(chars: usize, extra: f32) -> EstPara {
    EstPara {
        chars,
        cell: None,
        size: 11.0,
        line: 14.0,
        extra,
    }
}

/// After 8 pages of one-line entries, 3,000 paragraphs of 630 characters
/// (7 lines of 14 pt at 90 a line, 10 pt spacing: 108 pt each) at 720 pt a
/// page are 450 more pages, however short the first pages' lines.
#[test]
fn short_lines_first_dont_inflate_the_estimate() {
    let mut paras = vec![para(20, 4.0); 320];
    paras.extend(std::iter::repeat_n(para(630, 10.0), 3000));
    assert_eq!(estimate_from_heights(&paras[320..], 8, 720.0, 90.0), 458);
    // By share of the text (as before), the same start said 845 pages.
    let chars = |ps: &[EstPara]| ps.iter().map(|p| p.chars + 40).sum::<usize>();
    let by_text = (8 * chars(&paras)).div_ceil(chars(&paras[..320]));
    assert!(by_text > 800, "{by_text}");
}

/// A table row is as tall as its tallest cell, not all its cells.
#[test]
fn table_rows_count_once() {
    let mut rows = Vec::new();
    for row in 0..200u32 {
        for cell in 0..4u32 {
            rows.push(EstPara {
                cell: Some((0, row, cell)),
                // The last cell has two paragraphs.
                ..para(10, 0.0)
            });
            if cell == 3 {
                rows.push(EstPara {
                    cell: Some((0, row, cell)),
                    ..para(10, 0.0)
                });
            }
        }
    }
    assert_eq!(height_of(&rows, 90.0), 200.0 * 28.0);
    assert_eq!(estimate_from_heights(&rows, 5, 560.0, 90.0), 15);
}

/// Whatever fonts are installed: laid out up to the end of a long list of
/// short entries, the estimate is near the page count the whole layout
/// finds (both use the same fonts).
#[test]
fn prefix_estimate_is_near_the_full_count() {
    let words = [
        "the",
        "contractor",
        "shall",
        "provide",
        "notice",
        "within",
        "thirty",
        "days",
    ];
    let long = |i: usize| {
        let n = 40 + (i * 7) % 60;
        (0..n)
            .map(|k| words[(i + k * 3) % 8])
            .collect::<Vec<_>>()
            .join(" ")
    };
    // Entries, with a few long paragraphs among them (a line's length is
    // measured on those).
    let mut text: Vec<String> = (0..300)
        .map(|i| {
            if i % 10 == 9 {
                long(i)
            } else {
                format!("{} {}", words[i % 8], i)
            }
        })
        .collect();
    text.extend((0..1500).map(long));
    let mut doc = Document::new();
    doc.insert_text(Selection::default(), &text.join("\n"), None);
    let mut cache = LayoutCache::default();
    let first = cache.relayout_prefix(&doc, &mut Engine::new(), 300);
    let est = cache.estimate.expect("an estimate");
    let all = docs_pdf::lay_out(&doc, &mut Engine::new())
        .paged
        .pages
        .len();
    assert!(first.paged.pages.len() < all / 4);
    let off = (est as f32 - all as f32).abs() / all as f32;
    assert!(off < 0.1, "estimate {est}, laid out {all}");
}
