//! Relayout after an edit ([`docs_pdf::LayoutCache::relayout`]) draws
//! exactly what a full layout of the edited document draws, whether it
//! took the quick path (one paragraph's layout replaced) or not. Both
//! layouts use the same fonts, so this holds whatever fonts are installed.

use docs_engine::{Document, ListKind, Position, Selection};
use docs_layout::Engine;
use docs_pdf::LayoutCache;
use docs_pdf::cache::{Outcome, difference};

/// A document of several pages: paragraphs of different lengths, a
/// numbered list and a table.
fn long_document() -> Document {
    let words = [
        "the",
        "contractor",
        "shall",
        "provide",
        "notice",
        "within",
        "thirty",
        "days",
        "of",
        "any",
        "material",
        "change",
        "to",
        "works",
    ];
    let paras: Vec<String> = (0..90)
        .map(|i| {
            let n = 4 + (i * 7) % 40;
            (0..n)
                .map(|k| words[(i + k * 3) % words.len()])
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let mut doc = Document::new();
    doc.insert_text(Selection::default(), &paras.join("\n"), None);
    doc.toggle_list(
        Selection {
            anchor: Position::new(10, 0),
            head: Position::new(14, 0),
        },
        ListKind::Numbered,
    );
    doc.insert_table(Selection::caret(Position::new(30, 0)), 3, 3);
    doc
}

struct Check {
    cache: LayoutCache,
    engine: Engine,
    full: Engine,
    laid: Option<docs_pdf::Layout>,
    quick: usize,
}

impl Check {
    fn new(doc: &Document) -> Self {
        let mut c = Self {
            cache: LayoutCache::default(),
            engine: Engine::new(),
            full: Engine::new(),
            laid: None,
            quick: 0,
        };
        c.relayout(doc, "the first layout");
        c
    }

    fn relayout(&mut self, doc: &Document, what: &str) {
        let laid = self.cache.relayout(doc, &mut self.engine, self.laid.take());
        if matches!(self.cache.outcome, Outcome::Quick { .. }) {
            self.quick += 1;
        }
        let full = docs_pdf::lay_out(doc, &mut self.full);
        assert_eq!(difference(&laid, &full), None, "after {what}");
        self.laid = Some(laid);
    }
}

#[test]
fn edits_relay_out_as_a_full_layout_does() {
    let mut doc = long_document();
    let mut c = Check::new(&doc);
    assert!(c.laid.as_ref().unwrap().paged.pages.len() >= 2);
    let cell = doc
        .paragraphs()
        .iter()
        .position(|p| p.cell.is_some())
        .expect("a table");
    let last = doc.len() - 1;
    for para in [0, 11, cell, 45, last] {
        let mut at = doc.clamp(Position::new(para, 3));
        // Typing: mostly the same lines (quick), now and then one more.
        for k in 0..30 {
            let ch = if k % 5 == 4 { " " } else { "m" };
            at = doc.insert_text(Selection::caret(at), ch, None).head;
            c.relayout(&doc, &format!("typing {k} in paragraph {para}"));
        }
        for k in 0..8 {
            at = doc.backspace(Selection::caret(at)).head;
            c.relayout(&doc, &format!("backspace {k} in paragraph {para}"));
        }
        doc.undo();
        c.relayout(&doc, &format!("undo in paragraph {para}"));
        doc.redo();
        c.relayout(&doc, &format!("redo in paragraph {para}"));
        // A new paragraph, and joining it again (full layouts).
        at = doc.insert_text(Selection::caret(at), "\n", None).head;
        c.relayout(&doc, &format!("Enter in paragraph {para}"));
        doc.backspace(Selection::caret(at));
        c.relayout(&doc, &format!("joining paragraph {para}"));
    }
    // Tracked changes: insertions and struck-through deletions.
    doc.set_track_changes(true, Selection::default());
    c.relayout(&doc, "track changes on");
    let mut at = doc.clamp(Position::new(5, 6));
    for k in 0..10 {
        at = doc.insert_text(Selection::caret(at), "q", None).head;
        c.relayout(&doc, &format!("tracked typing {k}"));
    }
    for k in 0..12 {
        at = doc.backspace(Selection::caret(at)).head;
        c.relayout(&doc, &format!("tracked backspace {k}"));
    }
    assert!(c.quick >= 100, "only {} quick updates", c.quick);
}

/// An edit the cache can't see as one paragraph's (another document in
/// the same view, a style change) lays everything out again.
#[test]
fn other_changes_lay_everything_out() {
    let mut doc = long_document();
    let mut c = Check::new(&doc);
    doc.set_style(Selection::caret(Position::new(3, 0)), Some("Heading1"));
    c.relayout(&doc, "a heading style");
    assert_eq!(c.cache.outcome, Outcome::Full);
    let other = long_document();
    c.relayout(&other, "another document");
    assert_eq!(c.cache.outcome, Outcome::Full);
}
