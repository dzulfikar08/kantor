//! Merge Cells and Split Cells…, untracked and with Track Changes on, and
//! Word's tracked merges (`w:cellMerge`) read, accepted and rejected.

use std::io::{Cursor, Read as _};
use std::path::PathBuf;

use docs_engine::{Document, Paragraph, Position, RevKind, Selection, VMerge};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/docx")
        .join(name)
}

fn document_xml(path: &std::path::Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    zip.by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut s)
        .unwrap();
    s
}

fn caret(p: usize, o: usize) -> Selection {
    Selection::caret(Position::new(p, o))
}

/// "before", then a 3 × 3 table with "r{row}c{col}" in each cell.
fn grid_doc() -> Document {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("before")]);
    d.set_author("Ani Wijaya");
    let mut c = d.insert_table(caret(0, 6), 3, 3).unwrap();
    for r in 0..3 {
        for k in 0..3 {
            c = d.insert_text(c, &format!("r{r}c{k}"), None);
            if (r, k) != (2, 2) {
                c = d.next_cell(c, true).unwrap();
            }
        }
    }
    d
}

/// The paragraph of the cell whose text is `text`.
fn para(d: &Document, text: &str) -> usize {
    d.paragraphs().iter().position(|p| p.text == text).unwrap()
}

fn select(d: &Document, from: &str, to: &str) -> Selection {
    Selection {
        anchor: Position::new(para(d, from), 0),
        head: Position::new(para(d, to), 0),
    }
}

/// Each row's cells as (span, merge).
fn shape(d: &Document) -> Vec<Vec<(u32, VMerge)>> {
    d.table(1)
        .unwrap()
        .rows
        .iter()
        .map(|r| r.cells.iter().map(|c| (c.span, c.vmerge)).collect())
        .collect()
}

fn texts(d: &Document) -> Vec<String> {
    d.paragraphs().iter().map(|p| p.text.clone()).collect()
}

fn reopen(d: &mut Document, dir: &tempfile::TempDir, name: &str) -> Document {
    let path = dir.path().join(name);
    d.save_as(&path).unwrap();
    Document::open(&path).unwrap()
}

use VMerge::{Continue as C, None as N, Restart as R};

#[test]
fn merging_a_block_joins_its_text_and_saves_spans_and_vmerge() {
    let mut d = grid_doc();
    let sel = select(&d, "r0c0", "r1c1");
    assert!(d.can_merge_cells(sel));
    let (after, untracked) = d.merge_cells(sel).unwrap();
    assert!(!untracked);
    assert_eq!(
        shape(&d),
        [vec![(2, R), (1, N)], vec![(2, C), (1, N)], vec![(1, N); 3]]
    );
    // The four cells' text, in reading order, in the merged cell.
    assert_eq!(
        texts(&d)[1..7],
        ["r0c0", "r0c1", "r1c0", "r1c1", "r0c2", ""]
    );
    assert_eq!(after.head, Position::new(1, 0));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("m.docx");
    d.save_as(&path).unwrap();
    let xml = document_xml(&path);
    assert!(
        xml.contains("<w:gridSpan w:val=\"2\"/><w:vMerge w:val=\"restart\"/>"),
        "{xml}"
    );
    assert!(
        xml.contains("<w:gridSpan w:val=\"2\"/><w:vMerge/>"),
        "{xml}"
    );
    let back = Document::open(&path).unwrap();
    assert_eq!(shape(&back), shape(&d));
    // One undo step.
    d.undo();
    assert_eq!(shape(&d), vec![vec![(1, N); 3]; 3]);
    assert_eq!(texts(&d)[1], "r0c0");
}

#[test]
fn a_selection_through_a_merged_cell_takes_all_of_it() {
    let mut d = grid_doc();
    d.merge_cells(select(&d, "r0c1", "r1c1")).unwrap();
    // Column 0, rows 1-2.
    let (_, _) = d.merge_cells(select(&d, "r1c0", "r2c0")).unwrap();
    assert_eq!(
        shape(&d),
        [
            vec![(1, N), (1, R), (1, N)],
            vec![(1, R), (1, C), (1, N)],
            vec![(1, C), (1, N), (1, N)]
        ]
    );
    // Selecting r0c0..r0c1 crosses column 1's merge, which goes down to
    // row 1, and then column 0's, down to row 2.
    let mut e = d.clone();
    e.merge_cells(select(&e, "r0c0", "r0c1")).unwrap();
    assert_eq!(
        shape(&e),
        [
            vec![(2, R), (1, N)],
            vec![(2, C), (1, N)],
            vec![(2, C), (1, N)]
        ]
    );
    // A single cell can't be merged.
    assert!(!d.can_merge_cells(caret(para(&d, "r0c2"), 0)));
    assert!(d.merge_cells(caret(para(&d, "r0c2"), 0)).is_err());
}

#[test]
fn split_into_columns_and_rows() {
    let mut d = grid_doc();
    let at = caret(para(&d, "r1c1"), 0);
    d.split_cells(at, 2, 2).unwrap();
    // Column 1 became two grid columns; row 1 gained a row below.
    let t = d.table(1).unwrap();
    assert_eq!(t.grid.len(), 4);
    assert_eq!(
        shape(&d),
        [
            vec![(1, N), (2, N), (1, N)],
            vec![(1, R), (1, N), (1, N), (1, R)],
            vec![(1, C), (1, N), (1, N), (1, C)],
            vec![(1, N), (2, N), (1, N)],
        ]
    );
    // The total width is kept.
    let before = grid_doc();
    let w0: u32 = before.table(1).unwrap().grid.iter().sum();
    let w1: u32 = t.grid.iter().sum();
    assert!(w0.abs_diff(w1) <= 2, "{w0} {w1}");
    let dir = tempfile::tempdir().unwrap();
    let back = reopen(&mut d, &dir, "s.docx");
    assert_eq!(shape(&back), shape(&d));
    d.undo();
    assert_eq!(shape(&d), vec![vec![(1, N); 3]; 3]);
}

#[test]
fn split_a_merged_cell_back_and_spread_its_paragraphs() {
    let mut d = grid_doc();
    d.merge_cells(select(&d, "r0c0", "r0c2")).unwrap();
    assert_eq!(shape(&d)[0], [(3, N)]);
    d.split_cells(caret(para(&d, "r0c0"), 0), 1, 3).unwrap();
    assert_eq!(shape(&d), vec![vec![(1, N); 3]; 3]);
    assert_eq!(d.table(1).unwrap().grid.len(), 3);
    assert_eq!(texts(&d)[1..4], ["r0c0", "r0c1", "r0c2"]);
    // A cell merged down 2 rows splits into 2 rows, not 3.
    let mut e = grid_doc();
    e.merge_cells(select(&e, "r0c0", "r1c0")).unwrap();
    let at = caret(para(&e, "r0c0"), 0);
    assert!(e.split_cells(at, 3, 1).is_err());
    e.split_cells(at, 2, 1).unwrap();
    assert_eq!(shape(&e), vec![vec![(1, N); 3]; 3]);
}

#[test]
fn tracked_merge_down_a_column_is_cell_merge_and_resolves() {
    let mut d = grid_doc();
    d.set_track_changes(true, Selection::default());
    let (_, untracked) = d.merge_cells(select(&d, "r0c0", "r1c0")).unwrap();
    assert!(!untracked);
    assert_eq!(shape(&d)[0][0], (1, R));
    assert_eq!(shape(&d)[1][0], (1, C));
    let ch = d.changes();
    assert!(
        ch.iter()
            .any(|c| c.format.contains(&docs_engine::FormatItem::MergedCells)),
        "{ch:?}"
    );
    // The text moved up: inserted in the top cell, struck below.
    assert!(
        ch.iter()
            .any(|c| c.kind == RevKind::Insert && c.text.contains("r1c0"))
    );
    assert!(
        ch.iter()
            .any(|c| c.kind == RevKind::Delete && c.text.contains("r1c0"))
    );
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.docx");
    d.save_as(&path).unwrap();
    let xml = document_xml(&path);
    assert!(xml.contains("w:vMerge=\"rest\""), "{xml}");
    assert!(xml.contains("w:vMerge=\"cont\""), "{xml}");
    // Read back (it used to need Save As) and rejected: as before.
    let mut back = Document::open(&path).unwrap();
    assert!(
        !back.has_unsupported_content(),
        "files with w:cellMerge save in place"
    );
    assert_eq!(shape(&back), shape(&d));
    let all = Selection::default();
    back.resolve_all(false, all);
    assert_eq!(shape(&back), vec![vec![(1, N); 3]; 3]);
    assert_eq!(texts(&back), texts(&grid_doc()));
    // Accepted: merged, the text in the top cell.
    let mut acc = Document::open(&path).unwrap();
    acc.resolve_all(true, all);
    assert_eq!(shape(&acc)[1][0], (1, C));
    let i = para(&acc, "r0c0");
    assert_eq!(acc.paragraph(i + 1).text, "r1c0");
    assert!(acc.changes().is_empty());
    let saved = dir.path().join("acc.docx");
    acc.save_as(&saved).unwrap();
    let xml = document_xml(&saved);
    assert!(!xml.contains("cellMerge"));
    assert!(xml.contains("<w:vMerge w:val=\"restart\"/>"));
}

#[test]
fn tracked_merge_across_columns_goes_in_untracked() {
    let mut d = grid_doc();
    d.set_track_changes(true, Selection::default());
    let (_, untracked) = d.merge_cells(select(&d, "r0c0", "r0c1")).unwrap();
    assert!(untracked);
    assert_eq!(shape(&d)[0], [(2, N), (1, N)]);
    let (_, untracked) = d.split_cells(caret(1, 0), 1, 2).unwrap();
    assert!(untracked);
}

#[test]
fn word_cell_merge_records_open_and_resolve() {
    let path = corpus("tracked_merge.docx");
    let d = Document::open(&path).unwrap();
    assert!(
        !d.has_unsupported_content(),
        "{:?}",
        d.unsupported_content()
    );
    // Row 1's first cell continues row 0's (tracked).
    assert_eq!(shape(&d)[0][0].1, R);
    assert_eq!(shape(&d)[1][0].1, C);
    let ch = d.changes();
    assert_eq!(ch.len(), 1, "{ch:?}");
    assert_eq!(ch[0].format, [docs_engine::FormatItem::MergedCells]);
    let mut rej = d.clone();
    rej.reject_change(caret(ch[0].start.para, 0)).unwrap();
    assert_eq!(shape(&rej)[0][0].1, N);
    assert_eq!(shape(&rej)[1][0].1, N);
    let mut acc = d.clone();
    acc.accept_change(caret(ch[0].start.para, 0)).unwrap();
    assert_eq!(shape(&acc)[1][0].1, C);
    assert!(acc.changes().is_empty());
}

/// Writes merged, split and tracked-merge tables for the Open XML
/// validator: `KANTOR_MERGE_OUT=dir cargo test -p docs_engine --test
/// table_merge write_merge_samples -- --ignored`.
#[test]
#[ignore]
fn write_merge_samples() {
    let out = PathBuf::from(std::env::var("KANTOR_MERGE_OUT").expect("KANTOR_MERGE_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let mut d = grid_doc();
    d.merge_cells(select(&d, "r0c0", "r1c1")).unwrap();
    d.split_cells(caret(para(&d, "r2c2"), 0), 2, 3).unwrap();
    d.set_table_props(
        caret(para(&d, "r2c0"), 0),
        docs_engine::TableProps {
            align: docs_engine::TableAlign::Center,
            indent: 0,
        },
    )
    .unwrap();
    d.save_as(&out.join("merge-split.docx")).unwrap();
    let mut d = grid_doc();
    d.set_track_changes(true, Selection::default());
    d.merge_cells(select(&d, "r0c0", "r2c0")).unwrap();
    d.set_table_props(
        caret(1, 0),
        docs_engine::TableProps {
            align: docs_engine::TableAlign::Right,
            indent: 0,
        },
    )
    .unwrap();
    d.save_as(&out.join("merge-tracked.docx")).unwrap();
    let mut d = Document::open(&corpus("tracked_merge.docx")).unwrap();
    d.insert_text(caret(0, 0), "New ", None);
    d.save_as(&out.join("merge-word.docx")).unwrap();
}
