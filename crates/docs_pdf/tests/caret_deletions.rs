//! Caret moves between wrapped lines, around tracked deletions (shown struck
//! through, still part of the paragraph). Checked as invariants over every
//! caret position, so the test doesn't depend on the fonts installed.

use docs_engine::{Document, Paragraph, Position, Selection};
use docs_layout::{Engine, PagedDocument};

/// Words of mixed lengths, so wrapped lines end at different x.
fn words() -> String {
    let w = [
        "Sales",
        "grew",
        "in",
        "seven",
        "of",
        "our",
        "eight",
        "cities",
        "this",
        "quarter.",
        "Together",
        "the",
        "outlets",
        "sold",
        "Rp",
        "1,581",
        "million,",
        "4",
        "percent",
        "more",
        "than",
        "in",
        "the",
        "second",
        "quarter,",
        "and",
        "the",
        "iced",
        "palm-sugar",
        "coffee",
        "is",
        "still",
        "a",
        "third",
        "of",
        "every",
        "order.",
    ];
    let mut out = Vec::new();
    for i in 0..4 {
        out.extend(w.iter().skip(i).map(|s| s.to_string()));
    }
    out.join(" ")
}

/// Two long paragraphs; in the first, the first words of each wrapped line
/// after the first are a tracked deletion.
fn doc() -> Document {
    let mut d = Document::from_paragraphs(vec![
        Paragraph::new(words()),
        Paragraph::new(words()),
        Paragraph::new("Short last paragraph."),
    ]);
    d.set_author("Ani Wijaya");
    d.set_track_changes(true, Selection::default());
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&d, &mut engine);
    let mut starts = Vec::new();
    let mut at = 0;
    while let Some((r, true)) = laid.paged.line_range(0, at) {
        starts.push(r.end);
        at = r.end;
    }
    assert!(starts.len() >= 2, "the paragraph wraps");
    // From the back, so earlier offsets stay put (deleted text stays).
    for s in starts.into_iter().rev() {
        let text = &d.paragraph(0).text;
        let end = text[s..].find(' ').map_or(text.len(), |i| s + i);
        d.delete_selection(Selection {
            anchor: Position::new(0, s),
            head: Position::new(0, end),
        });
    }
    assert!(!d.changes().is_empty(), "deletions are tracked");
    d
}

fn positions(d: &Document) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for para in 0..d.len() {
        let text = &d.paragraph(para).text;
        for (off, _) in text.char_indices() {
            out.push((para, off));
        }
        out.push((para, text.len()));
    }
    out
}

/// The line a caret at `pos` is drawn on.
fn line_of(paged: &PagedDocument, pos: (usize, usize)) -> (usize, usize) {
    paged.placed_line(pos.0, pos.1).expect("a placed line")
}

#[test]
fn up_and_down_always_reach_another_line() {
    let d = doc();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&d, &mut engine);
    let paged = &laid.paged;
    let mut checked = 0;
    for pos in positions(&d) {
        let c = paged.caret(pos.0, pos.1).unwrap();
        for down in [false, true] {
            let Some(n) = paged.vertical(pos.0, pos.1, c.x, down) else {
                continue;
            };
            let nc = paged.caret(n.0, n.1).unwrap();
            assert_ne!(
                line_of(paged, n),
                line_of(paged, pos),
                "{} from {pos:?} stays on its line ({n:?})",
                if down { "Down" } else { "Up" }
            );
            if nc.page == c.page {
                assert!(
                    if down { nc.top > c.top } else { nc.top < c.top },
                    "{pos:?} -> {n:?}: wrong way"
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 100);
}

#[test]
fn walking_up_and_down_visits_every_line() {
    let d = doc();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&d, &mut engine);
    let paged = &laid.paged;
    let lines: usize = paged.pages.iter().map(|p| p.lines.len()).sum();
    // A goal x right of every line's end: the hardest case.
    for x in [10_000.0f32, 0.0] {
        let end = d.end();
        let mut pos = (end.para, end.offset);
        let mut seen = vec![line_of(paged, pos)];
        while let Some(n) = paged.vertical(pos.0, pos.1, x, false) {
            assert!(seen.len() <= lines, "Up loops at {pos:?}");
            seen.push(line_of(paged, n));
            pos = n;
        }
        assert_eq!(seen.len(), lines, "Up at x {x} skips or repeats lines");
        assert_eq!(pos.0, 0);
        let mut down = vec![line_of(paged, pos)];
        while let Some(n) = paged.vertical(pos.0, pos.1, x, true) {
            assert!(down.len() <= lines, "Down loops at {pos:?}");
            down.push(line_of(paged, n));
            pos = n;
        }
        assert_eq!(down.len(), lines, "Down at x {x} skips or repeats lines");
    }
}

#[test]
fn clicks_past_a_wrapped_line_end_stay_on_it() {
    let d = doc();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&d, &mut engine);
    let paged = &laid.paged;
    for (page, p) in paged.pages.iter().enumerate() {
        for (i, l) in p.lines.iter().enumerate() {
            let y = l.top + l.height / 2.0;
            for x in [l.x - 50.0, l.x + l.width + 200.0] {
                let hit = paged.hit_test(page, x, y).unwrap();
                assert_eq!(line_of(paged, hit), (page, i), "click at {x},{y}");
            }
        }
    }
}
