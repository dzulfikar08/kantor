//! `notes.docx` (scripts/corpus/gen_docx.py): a footnote too long for its
//! page goes on at the bottom of the next one, under the continuation
//! separator and with the continuation notice; endnotes sit at the end of
//! each section (`w:pos sectEnd`).

use std::path::PathBuf;

use docs_engine::{Document, NoteKind};
use docs_layout::Engine;

fn open() -> Document {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/notes.docx");
    Document::open(&path).unwrap()
}

#[test]
fn long_footnote_continues_and_endnotes_end_their_sections() {
    let doc = open();
    assert!(doc.note_props(NoteKind::Endnote, 0).section_end);
    assert!(doc.continuation_notice(NoteKind::Footnote).is_some());
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let pages = &laid.paged.pages;
    // The long note (id 1) starts on one page and continues.
    let parts: Vec<(usize, &docs_layout::NotePart)> = pages
        .iter()
        .enumerate()
        .flat_map(|(i, p)| p.note_parts.iter().map(move |n| (i, n)))
        .collect();
    let first = parts
        .iter()
        .find(|(_, p)| p.more)
        .expect("a note that continues");
    let next = parts
        .iter()
        .find(|(i, p)| *i == first.0 + 1 && p.from == first.1.to)
        .expect("its continuation on the next page");
    assert!(pages[next.0].continued);
    // On screen and in the PDF: the part shown, the notice under it, and
    // the continuation separator spanning the text column.
    let placed: Vec<_> = laid.furniture[first.0]
        .iter()
        .filter(|p| p.layout.note.is_some())
        .collect();
    assert!(placed.iter().any(|p| p.note.is_none()), "the notice");
    let s = pages[next.0].setup;
    assert!(
        pages[next.0]
            .rules
            .iter()
            .any(|r| (r.rect[2] - s.content_width()).abs() < 0.01)
    );
    // Endnotes: the first section's before the second section's first
    // page, the second section's on its last page.
    let section_of = |pi: usize| pages[pi].section;
    let endnote_pages: Vec<(usize, usize)> = pages
        .iter()
        .enumerate()
        .flat_map(|(i, p)| {
            p.gaps
                .iter()
                .filter(|g| g.id != usize::MAX)
                .map(move |_| (i, 0))
        })
        .collect();
    assert_eq!(endnote_pages.len(), 2, "{endnote_pages:?}");
    assert_eq!(section_of(endnote_pages[0].0), 0);
    assert_eq!(section_of(endnote_pages[1].0), 1);
    let pdf = docs_pdf::write(&docs_pdf::scene(&doc, &laid)).unwrap();
    assert!(pdf.starts_with(b"%PDF"));
}
