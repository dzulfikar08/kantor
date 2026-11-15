//! Tab stops: inheritance from styles, editing (Tabs dialog / ruler),
//! undo, the default interval, and saving as `w:tabs`.

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _};
use std::path::PathBuf;

use docs_engine::tabs::TabEdit;
use docs_engine::{Document, Paragraph, Position, Selection, TabKind, TabLeader, TabStop};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/docx")
        .join(name)
}

fn parts(bytes: &[u8]) -> BTreeMap<String, String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let name = f.name().unwrap().into_owned();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        out.insert(name, String::from_utf8_lossy(&v).into_owned());
    }
    out
}

fn saved(doc: &mut Document) -> (Vec<u8>, Document) {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("tabs.docx");
    doc.save_as(&out).unwrap();
    let bytes = std::fs::read(&out).unwrap();
    let again = Document::open(&out).unwrap();
    (bytes, again)
}

fn at(para: usize) -> Selection {
    Selection::caret(Position::new(para, 0))
}

fn stop(pos: i32, kind: TabKind, leader: TabLeader) -> TabStop {
    TabStop::new(pos, kind, leader)
}

#[test]
fn style_stops_are_inherited_and_cleared() {
    let mut doc = Document::new();
    let heading = doc
        .styles
        .styles
        .iter_mut()
        .find(|s| s.id == "Heading1")
        .expect("built-in Heading1");
    heading.para.extras.push(
        r#"<w:tabs><w:tab w:val="left" w:pos="1440"/><w:tab w:val="right" w:leader="dot" w:pos="9026"/></w:tabs>"#,
    );
    let mut p = Paragraph::new("Chapter\t1");
    p.props.style = Some("Heading1".into());
    assert_eq!(
        doc.tab_stops(&p),
        [
            stop(1440, TabKind::Left, TabLeader::None),
            stop(9026, TabKind::Right, TabLeader::Dot)
        ]
    );
    // The paragraph's own w:tabs clears one and adds a bar tab.
    p.props.extras.push(
        r#"<w:tabs><w:tab w:val="clear" w:pos="1440"/><w:tab w:val="bar" w:pos="300"/></w:tabs>"#,
    );
    assert_eq!(
        doc.tab_stops(&p),
        [
            stop(300, TabKind::Bar, TabLeader::None),
            stop(9026, TabKind::Right, TabLeader::Dot)
        ]
    );
    // Clearing an inherited stop in Kantor writes a clear element.
    let mut doc = Document::from_paragraphs(vec![p.clone()]);
    doc.styles = {
        let mut d = Document::new();
        let h = d
            .styles
            .styles
            .iter_mut()
            .find(|s| s.id == "Heading1")
            .unwrap();
        h.para
            .extras
            .push(r#"<w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9026"/></w:tabs>"#);
        d.styles
    };
    doc.edit_tab_stops(
        at(0),
        &TabEdit {
            clear_all: true,
            ..TabEdit::default()
        },
    );
    assert!(doc.tab_stops(doc.paragraph(0)).is_empty());
    let own: Vec<&str> = doc.paragraph(0).props.extras.iter().collect();
    assert_eq!(
        own,
        [r#"<w:tabs><w:tab w:val="clear" w:pos="9026"/></w:tabs>"#]
    );
}

#[test]
fn set_clear_undo_redo_and_save() {
    let mut doc = Document::from_paragraphs(vec![
        Paragraph::new("Contents"),
        Paragraph::new("Introduction\t1"),
        Paragraph::new("Results\t12"),
        Paragraph::new("Amount\t1234.50"),
    ]);
    let toc = Selection {
        anchor: Position::new(1, 0),
        head: Position::new(2, 3),
    };
    let dots = stop(9026, TabKind::Right, TabLeader::Dot);
    doc.edit_tab_stops(
        toc,
        &TabEdit {
            set: vec![dots],
            ..TabEdit::default()
        },
    );
    doc.edit_tab_stops(
        at(3),
        &TabEdit {
            set: vec![
                stop(4536, TabKind::Decimal, TabLeader::None),
                stop(567, TabKind::Center, TabLeader::Hyphen),
            ],
            ..TabEdit::default()
        },
    );
    assert_eq!(doc.tab_stops(doc.paragraph(1)), [dots]);
    assert_eq!(doc.tab_stops(doc.paragraph(2)), [dots]);
    assert!(doc.tab_stops(doc.paragraph(0)).is_empty());
    assert_eq!(doc.common_tab_stops(toc), [dots]);
    assert_eq!(
        doc.common_tab_stops(Selection {
            anchor: Position::new(0, 0),
            head: Position::new(3, 0)
        }),
        []
    );
    // Undo the decimal stops, redo them.
    doc.undo();
    assert!(doc.tab_stops(doc.paragraph(3)).is_empty());
    doc.redo();
    assert_eq!(doc.tab_stops(doc.paragraph(3)).len(), 2);
    // Moving a stop (as a ruler drag) = clear + set, one step.
    doc.edit_tab_stops(
        at(3),
        &TabEdit {
            clear: vec![567],
            set: vec![stop(1134, TabKind::Center, TabLeader::Hyphen)],
            ..TabEdit::default()
        },
    );
    assert_eq!(doc.tab_stops(doc.paragraph(3))[0].pos, 1134);
    doc.undo();
    assert_eq!(doc.tab_stops(doc.paragraph(3))[0].pos, 567);
    doc.redo();
    // The default interval: one undo step, saved in settings.xml.
    doc.set_default_tab(567, at(0));
    assert_eq!(doc.default_tab(), 567);
    doc.undo();
    assert_eq!(doc.default_tab(), 720);
    doc.redo();

    let (bytes, again) = saved(&mut doc);
    let p = parts(&bytes);
    let body = &p["word/document.xml"];
    assert!(
        body.contains(r#"<w:tabs><w:tab w:val="right" w:leader="dot" w:pos="9026"/></w:tabs>"#),
        "{body}"
    );
    assert!(body.contains(
        r#"<w:tabs><w:tab w:val="center" w:leader="hyphen" w:pos="1134"/><w:tab w:val="decimal" w:pos="4536"/></w:tabs>"#
    ));
    assert!(p["word/settings.xml"].contains(r#"<w:defaultTabStop w:val="567"/>"#));
    assert_eq!(again.default_tab(), 567);
    assert_eq!(again.tab_stops(again.paragraph(1)), [dots]);
    assert_eq!(
        again.tab_stops(again.paragraph(3)),
        [
            stop(1134, TabKind::Center, TabLeader::Hyphen),
            stop(4536, TabKind::Decimal, TabLeader::None)
        ]
    );
}

#[test]
fn editing_one_paragraph_leaves_the_others_byte_identical() {
    let mut doc = Document::open(&corpus("report.docx")).unwrap();
    let before = parts(&saved(&mut doc).0);
    let mut doc = Document::open(&corpus("report.docx")).unwrap();
    let k = (0..doc.paragraphs().len())
        .find(|&i| doc.paragraph(i).cell.is_none() && doc.paragraph(i).text.len() > 3)
        .unwrap();
    doc.edit_tab_stops(
        at(k),
        &TabEdit {
            set: vec![stop(2268, TabKind::Left, TabLeader::Underscore)],
            ..TabEdit::default()
        },
    );
    let after = parts(&saved(&mut doc).0);
    for (name, text) in &before {
        if name != "word/document.xml" && name != "docProps/core.xml" {
            assert_eq!(text, &after[name], "{name} changed");
        }
    }
    // The body differs only inside one paragraph.
    let (a, b) = (&before["word/document.xml"], &after["word/document.xml"]);
    let pre = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    let suf = a
        .bytes()
        .rev()
        .zip(b.bytes().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let changed = &b[pre..b.len() - suf];
    assert!(changed.contains("w:tabs"), "{changed}");
    assert!(!changed.contains("</w:p>"), "{changed}");
    // Setting the same stops again changes nothing (no undo step).
    let mut doc = Document::open(&corpus("report.docx")).unwrap();
    doc.edit_tab_stops(
        at(k),
        &TabEdit {
            clear: vec![12345],
            ..TabEdit::default()
        },
    );
    assert!(!doc.can_undo());
}

#[test]
fn table_of_contents_lines_and_style_stops_from_word() {
    let mut doc = Document::open(&corpus("toc.docx")).unwrap();
    // The first line holds the TOC field's start: its text is kept.
    assert_eq!(doc.paragraph(1).text, "1. Introduction\t1");
    let dots = stop(9026, TabKind::Right, TabLeader::Dot);
    for i in 1..=8 {
        assert_eq!(doc.tab_stops(doc.paragraph(i)), [dots], "{i}");
    }
    // Saved under a new name (the field spans paragraphs): lines, styles
    // and stops survive.
    let (_, again) = saved(&mut doc);
    assert_eq!(again.paragraph(1).text, "1. Introduction\t1");
    assert_eq!(again.tab_stops(again.paragraph(1)), [dots]);
    assert_eq!(again.tab_stops(again.paragraph(11)).len(), 3);
}
