//! Track Changes over edits beyond typing: Replace All, table rows,
//! floating objects and text boxes (Word's behaviour), saved as Word
//! writes them and read back.

use std::io::{Cursor, Read as _};
use std::path::PathBuf;

use docs_engine::{Document, Object, Paragraph, Position, RevKind, Selection, TableEdit};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/docx")
        .join(name)
}

fn document_xml(bytes: &[u8]) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    zip.by_name("word/document.xml")
        .unwrap()
        .read_to_string(&mut s)
        .unwrap();
    s
}

fn tracked(paras: &[&str]) -> Document {
    let mut d = Document::from_paragraphs(paras.iter().map(|t| Paragraph::new(*t)).collect());
    d.set_author("Ani Wijaya");
    d.set_track_changes(true, Selection::default());
    d
}

fn caret(p: usize, o: usize) -> Selection {
    Selection::caret(Position::new(p, o))
}

fn kinds(d: &Document) -> Vec<(RevKind, String)> {
    d.changes().into_iter().map(|c| (c.kind, c.text)).collect()
}

fn reopen(d: &mut Document) -> Document {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.docx");
    d.save_as(&path).unwrap();
    Document::open(&path).unwrap()
}

#[test]
fn replace_all_is_deletions_and_insertions() {
    let mut d = tracked(&["cat and cat", "a cat"]);
    let after = d.replace_ranges(&[(0, 0..3), (0, 8..11), (1, 2..5)], "dog", caret(0, 0));
    assert_eq!(d.paragraph(0).text, "catdog and catdog");
    assert_eq!(d.paragraph(1).text, "a catdog");
    assert_eq!(after.head, Position::new(1, 8));
    assert_eq!(
        kinds(&d),
        [
            (RevKind::Delete, "cat".into()),
            (RevKind::Insert, "dog".into()),
            (RevKind::Delete, "cat".into()),
            (RevKind::Insert, "dog".into()),
            (RevKind::Delete, "cat".into()),
            (RevKind::Insert, "dog".into()),
        ]
    );
    // One undo step.
    d.undo();
    assert_eq!(d.paragraph(0).text, "cat and cat");
    assert!(!d.has_changes());
    d.redo();
    let mut accepted = d.clone();
    accepted.resolve_all(true, Selection::default());
    assert_eq!(accepted.paragraph(0).text, "dog and dog");
    d.resolve_all(false, Selection::default());
    assert_eq!(d.paragraph(0).text, "cat and cat");
    // Each replacement has its own ids when saved.
    let mut d = tracked(&["cat and cat"]);
    d.replace_ranges(&[(0, 0..3), (0, 8..11)], "dog", caret(0, 0));
    let back = reopen(&mut d);
    assert_eq!(back.changes().len(), 4);
}

fn table_doc() -> Document {
    let mut d = Document::from_paragraphs(vec![Paragraph::new("before")]);
    d.set_author("Ani Wijaya");
    let c = d.insert_table(caret(0, 6), 2, 2).unwrap();
    let c = d.insert_text(c, "a1", None);
    let c = d.next_cell(c, true).unwrap();
    let c = d.insert_text(c, "b1", None);
    let c = d.next_cell(c, true).unwrap();
    let c = d.insert_text(c, "a2", None);
    let c = d.next_cell(c, true).unwrap();
    d.insert_text(c, "b2", None);
    d.set_track_changes(true, Selection::default());
    d
}

#[test]
fn deleted_rows_stay_struck_until_accepted() {
    let mut d = table_doc();
    // Delete the first row: kept, marked deleted, its text struck.
    d.edit_table(caret(1, 0), TableEdit::DeleteRow).unwrap();
    assert_eq!(d.table(1).unwrap().rows.len(), 2);
    let ch = d.changes();
    assert!(
        ch.iter().any(|c| c.row && c.kind == RevKind::Delete),
        "{ch:?}"
    );
    assert!(
        ch.iter()
            .any(|c| !c.row && c.kind == RevKind::Delete && c.text == "a1")
    );
    // Saved as Word writes it, and read back.
    let mut copy = d.clone();
    let xml = document_xml(&copy.to_docx_bytes().unwrap());
    assert!(xml.contains("<w:trPr><w:del w:id="), "{xml}");
    let back = reopen(&mut copy);
    assert!(
        !back.has_unsupported_content(),
        "{:?}",
        back.unsupported_content()
    );
    assert!(back.changes().iter().any(|c| c.row));
    // Reject at the caret: the row is back, unmarked.
    let mut rej = d.clone();
    rej.reject_change(caret(1, 1)).unwrap();
    assert!(!rej.has_changes(), "{:?}", rej.changes());
    assert_eq!(rej.table(1).unwrap().rows.len(), 2);
    assert_eq!(rej.paragraph(1).text, "a1");
    // Accept: the row goes.
    d.accept_change(caret(1, 1)).unwrap();
    assert!(!d.has_changes(), "{:?}", d.changes());
    assert_eq!(d.table(1).unwrap().rows.len(), 1);
    assert_eq!(d.paragraph(1).text, "a2");
    d.undo();
    assert_eq!(d.table(1).unwrap().rows.len(), 2);
}

#[test]
fn inserted_rows_are_marked_and_rejected() {
    let mut d = table_doc();
    d.edit_table(caret(3, 0), TableEdit::RowBelow).unwrap();
    assert_eq!(d.table(1).unwrap().rows.len(), 3);
    let ch = d.changes();
    assert_eq!(ch.len(), 1, "{ch:?}");
    assert!(ch[0].row && ch[0].kind == RevKind::Insert);
    let mut acc = d.clone();
    acc.resolve_all(true, Selection::default());
    assert_eq!(acc.table(1).unwrap().rows.len(), 3);
    assert!(!acc.has_changes());
    d.resolve_all(false, Selection::default());
    assert_eq!(d.table(1).unwrap().rows.len(), 2);
    assert!(!d.has_changes());
    // Deleting your own inserted row removes it at once.
    let mut d = table_doc();
    d.edit_table(caret(3, 0), TableEdit::RowBelow).unwrap();
    d.edit_table(caret(5, 0), TableEdit::DeleteRow).unwrap();
    assert_eq!(d.table(1).unwrap().rows.len(), 2);
    assert!(!d.has_changes());
    // Delete Table marks every row.
    let mut d = table_doc();
    d.edit_table(caret(1, 0), TableEdit::DeleteTable).unwrap();
    assert_eq!(d.changes().iter().filter(|c| c.row).count(), 2);
    d.resolve_all(true, Selection::default());
    assert!(d.table(1).is_none());
    assert_eq!(d.paragraph(0).text, "before");
}

fn floats(doc: &Document) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for (i, p) in doc.paragraphs().iter().enumerate() {
        for (at, o) in p.objects() {
            if let Object::Float(f) = o {
                out.push((i, at, f.name.clone()));
            }
        }
    }
    out
}

#[test]
fn deleting_a_floating_object_is_tracked() {
    let mut doc = Document::open(&corpus("floats.docx")).unwrap();
    doc.set_author("Ani Wijaya");
    doc.set_track_changes(true, Selection::default());
    let n = floats(&doc).len();
    let (p, i, _) = floats(&doc)
        .into_iter()
        .find(|f| f.2 == "Ellipse behind")
        .unwrap();
    assert!(doc.delete_float(p, i));
    assert_eq!(floats(&doc).len(), n, "kept until accepted");
    assert_eq!(
        doc.changes()
            .iter()
            .filter(|c| c.kind == RevKind::Delete)
            .count(),
        1
    );
    let xml = document_xml(&doc.to_docx_bytes().unwrap());
    let at = xml.find("Ellipse behind").unwrap();
    let del = xml[..at].rfind("<w:del ").unwrap();
    assert!(!xml[del..at].contains("</w:del>"), "anchor inside w:del");
    let mut rej = doc.clone();
    rej.resolve_all(false, Selection::default());
    assert_eq!(floats(&rej).len(), n);
    assert!(!rej.has_changes());
    doc.resolve_all(true, Selection::default());
    assert_eq!(floats(&doc).len(), n - 1);
}

#[test]
fn text_box_edits_are_tracked() {
    let mut doc = Document::open(&corpus("floats.docx")).unwrap();
    doc.set_author("Ani Wijaya");
    doc.set_track_changes(true, Selection::default());
    let (para, index, _) = floats(&doc)
        .into_iter()
        .find(|f| f.2 == "Text Box 1")
        .unwrap();
    assert!(doc.set_text_box_text(para, index, "Text box\nwith three paragraphs"));
    let f = doc.float_at(para, index).unwrap();
    let tb = &f.text.as_ref().unwrap().doc;
    let ch: Vec<(RevKind, String)> = tb.changes().into_iter().map(|c| (c.kind, c.text)).collect();
    assert_eq!(
        ch,
        [
            (RevKind::Delete, "two".into()),
            (RevKind::Insert, "three".into())
        ]
    );
    assert_eq!(tb.changes()[0].author, "Ani Wijaya");
    let xml = document_xml(&doc.to_docx_bytes().unwrap());
    assert!(
        xml.contains("<w:delText xml:space=\"preserve\">two</w:delText>"),
        "{xml}"
    );
}

/// Files with every kind of tracked change Kantor makes, for the Office
/// validator: `KANTOR_TRACK_OUT=dir cargo test -p docs_engine --test
/// track_edits write_tracked_samples -- --ignored`.
#[test]
#[ignore]
fn write_tracked_samples() {
    use docs_engine::{Align, FieldContext, HfKind, HfType, NoteKind, PageLookup, Toggle};
    let out = PathBuf::from(std::env::var("KANTOR_TRACK_OUT").expect("KANTOR_TRACK_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let start = |name: &str| {
        let mut d = Document::open(&corpus(name)).unwrap();
        d.set_author("Ani Wijaya");
        d.set_track_changes(true, Selection::default());
        d
    };
    // Formatting, Replace All, tables.
    let mut d = table_doc();
    d.insert_text(caret(0, 6), " text and more text", None);
    d.toggle(
        Selection {
            anchor: Position::new(0, 0),
            head: Position::new(0, 6),
        },
        Toggle::Bold,
    );
    d.set_align(caret(0, 0), Align::Center);
    d.edit_table(caret(1, 0), TableEdit::DeleteRow).unwrap();
    d.edit_table(caret(3, 0), TableEdit::RowBelow).unwrap();
    d.replace_ranges(&[(0, 0..6)], "After", caret(0, 0));
    d.save_as(&out.join("tables-format.docx")).unwrap();
    // Fields updated with tracking.
    struct NoPages;
    impl PageLookup for NoPages {
        fn page_of(&self, _: Position) -> Option<usize> {
            Some(0)
        }
        fn page_number(&self, _: usize) -> u32 {
            7
        }
        fn page_label(&self, _: usize) -> String {
            "7".into()
        }
    }
    let cx = FieldContext {
        now: chrono::NaiveDate::from_ymd_opt(2026, 11, 15)
            .unwrap()
            .and_hms_opt(9, 0, 0)
            .unwrap(),
        offset: chrono::FixedOffset::east_opt(7 * 3600).unwrap(),
    };
    let mut d = start("fields.docx");
    let all = Selection {
        anchor: Position::default(),
        head: d.end(),
    };
    d.update_fields(all, &cx, &mut |_| Box::new(NoPages));
    d.update_tables_of_contents(caret(1, 0), &cx, &mut |_| Box::new(NoPages));
    d.save_as(&out.join("fields-tracked.docx")).unwrap();
    // Headers and notes.
    let mut d = start("notes.docx");
    let id = d
        .notes()
        .iter()
        .find(|n| n.kind == NoteKind::Footnote && !n.separator)
        .unwrap()
        .id
        .clone();
    let (e, mut n) = d.edit_note(NoteKind::Footnote, &id).unwrap();
    let end = n.end();
    n.insert_text(Selection::caret(end), " Checked.", None);
    n.delete_selection(Selection {
        anchor: Position::new(0, 1),
        head: Position::new(0, 4),
    });
    d.finish_part(e, n, Selection::default());
    let (e, mut h) = d.edit_hf(0, HfKind::Header, HfType::Default).unwrap();
    h.insert_text(Selection::default(), "Draft", None);
    d.finish_part(e, h, Selection::default());
    d.save_as(&out.join("notes-tracked.docx")).unwrap();
    // Floats and text boxes.
    let mut d = start("floats.docx");
    let (p, i, _) = floats(&d)
        .into_iter()
        .find(|f| f.2 == "Ellipse behind")
        .unwrap();
    d.delete_float(p, i);
    let (p, i, _) = floats(&d)
        .into_iter()
        .find(|f| f.2 == "Text Box 1")
        .unwrap();
    d.set_text_box_text(p, i, "Text box\nwith three paragraphs");
    d.save_as(&out.join("floats-tracked.docx")).unwrap();
    // Word's own tracked changes, edited further.
    let mut d = start("minutes.docx");
    d.toggle(
        Selection {
            anchor: Position::new(1, 0),
            head: Position::new(1, 9),
        },
        Toggle::Italic,
    );
    d.save_as(&out.join("minutes-tracked.docx")).unwrap();
}

#[test]
fn word_tracked_formatting_rows_and_parts_are_read_and_resolved() {
    let path = corpus("tracked.docx");
    let doc = Document::open(&path).unwrap();
    assert!(
        !doc.has_unsupported_content(),
        "{:?}",
        doc.unsupported_content()
    );
    let ch = doc.changes();
    let fmt: Vec<_> = ch.iter().filter(|c| c.kind == RevKind::Format).collect();
    assert_eq!(fmt.len(), 2, "{ch:?}");
    assert_eq!(fmt[0].format, [docs_engine::FormatItem::Bold(true)]);
    assert_eq!(fmt[0].author, "Citra");
    assert_eq!(
        fmt[1].format,
        [docs_engine::FormatItem::Align(docs_engine::Align::Center)]
    );
    let rows: Vec<_> = ch.iter().filter(|c| c.row).map(|c| c.kind).collect();
    assert_eq!(rows, [RevKind::Delete, RevKind::Insert]);
    assert_eq!(doc.part_changes(), 2, "header insertion, footnote deletion");
    let table = doc
        .paragraphs()
        .iter()
        .find_map(|p| p.cell)
        .map(|c| c.table)
        .unwrap();
    // Untouched, it saves the tracked rows back as they were.
    let mut copy = doc.clone();
    let xml = document_xml(&copy.to_docx_bytes().unwrap());
    assert!(xml.contains("<w:del w:id=\"21\" w:author=\"Citra\""), "{xml}");
    // Accept All: bold and centred stay, the deleted row goes, the
    // inserted one stays, the header keeps "Draft ", the note loses its
    // deleted words.
    let mut acc = doc.clone();
    acc.resolve_all(true, Selection::default());
    assert!(!acc.has_changes() && acc.part_changes() == 0);
    assert_eq!(acc.table(table).unwrap().rows.len(), 2);
    assert_eq!(acc.paragraph(0).runs[0].props.bold, Some(true));
    assert_eq!(acc.paragraph(1).props.align, Some(docs_engine::Align::Center));
    let header: String = acc.stories()[0].doc.paragraphs()[0].text.clone();
    assert_eq!(header, "Draft Budget memo");
    let back = reopen(&mut acc);
    assert!(!back.has_changes());
    // Reject All: the opposite.
    let mut rej = doc.clone();
    rej.resolve_all(false, Selection::default());
    assert!(!rej.has_changes() && rej.part_changes() == 0);
    assert_eq!(rej.table(table).unwrap().rows.len(), 2);
    assert_ne!(rej.paragraph(0).runs[0].props.bold, Some(true));
    assert_eq!(rej.paragraph(1).props.align, None);
    assert_eq!(
        rej.stories()[0].doc.paragraphs()[0].text,
        "Budget memo"
    );
    let row_texts: Vec<String> = rej
        .paragraphs()
        .iter()
        .filter(|p| p.cell.is_some())
        .map(|p| p.text.clone())
        .collect();
    assert_eq!(row_texts, ["Item", "Cost", "Venue", "Rp 5.000.000"]);
}
