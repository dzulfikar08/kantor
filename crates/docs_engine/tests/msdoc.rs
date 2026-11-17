//! Importing Word 97-2003 `.doc` files: the corpus in tests/corpus/doc
//! (made by scripts/corpus/gen_doc.py: python-docx, then LibreOffice's
//! "MS Word 97" filter), saving the import as `.docx`, and damaged files.

use std::path::{Path, PathBuf};

use docs_engine::{Align, Document, HfKind, Object, VMerge, page_field};

fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/doc")
        .join(name)
}

fn open(name: &str) -> Document {
    Document::open(&corpus(name)).unwrap_or_else(|e| panic!("{name}: {e:#}"))
}

fn texts(doc: &Document) -> Vec<&str> {
    doc.paragraphs().iter().map(|p| p.text.as_str()).collect()
}

#[test]
fn text_formatting_and_headings() {
    let doc = open("text.doc");
    assert!(doc.is_imported());
    assert!(doc.path().is_none(), "Save must not write over the .doc");
    assert!(doc.needs_save_as());
    assert_eq!(doc.title(), "text.doc");
    assert_eq!(doc.suggested_file_name(), "text.docx");
    assert_eq!(doc.source_path(), Some(corpus("text.doc").as_path()));

    let t = texts(&doc);
    assert_eq!(&t[..3], ["Laporan Kantor", "Ringkasan", "Detail"]);
    for (i, h) in ["Heading1", "Heading2", "Heading3"].iter().enumerate() {
        assert_eq!(doc.paragraphs()[i].props.style.as_deref(), Some(*h));
    }
    let p = &doc.paragraphs()[3];
    let run = |s: &str| {
        let at = p
            .text
            .find(s)
            .unwrap_or_else(|| panic!("{s} in {:?}", p.text));
        p.props_at(at + 1)
    };
    assert_eq!(run("bold").bold, Some(true));
    assert_eq!(run("italic").italic, Some(true));
    assert_eq!(run("underlined").underline, Some(true));
    assert!(format!("{:?}", run("struck").extras).contains("w:strike"));
    assert_eq!(run("big red").size, Some(36));
    assert_eq!(
        run("big red").color.map(|c| c.hex()).as_deref(),
        Some("C00000")
    );
    assert_eq!(
        run("Courier New").fonts.ascii.as_deref(),
        Some("Courier New")
    );
    assert!(format!("{:?}", run("highlighted").extras).contains("w:highlight"));
    assert_eq!(run("Plain").bold, None);

    assert_eq!(doc.paragraphs()[4].props.align, Some(Align::Center));
    assert_eq!(doc.paragraphs()[5].props.align, Some(Align::Right));
    let j = &doc.paragraphs()[6].props;
    assert_eq!(j.align, Some(Align::Justify));
    assert_eq!((j.before, j.after), (Some(240), Some(120)));
    assert_eq!(j.line.map(|l| l.line), Some(360));
    assert_eq!(j.indent.left, Some(1134));

    assert_eq!(
        t[7],
        format!("Name:\tKantor{}Second line", docs_engine::LINE_BREAK)
    );
    // Unicode from UTF-16 pieces: Indonesian, CJK, emoji (surrogate pairs).
    assert!(t.contains(&"Bahasa Indonesia: Selamat pagi, apa kabar? Rp 1.250.000,00"));
    assert!(t.contains(&"CJK: 你好世界 こんにちは 안녕하세요"));
    assert!(t.contains(&"Emoji: 😀 👍🏽 ✓"));
    // The HYPERLINK field shows its result, not its instruction.
    assert!(t.contains(&"Visit the Kantor site today."));
    assert!(t.iter().all(|s| !s.contains("HYPERLINK")));
    assert_eq!(t.last(), Some(&"Last paragraph."));
}

#[test]
fn tables_with_merged_cells() {
    let doc = open("tables.doc");
    let t = texts(&doc);
    assert_eq!(t[0], "Quarterly figures");
    assert_eq!(t.last(), Some(&"Text after the table."));
    let cell = doc.paragraphs()[1].cell.expect("a table follows the title");
    let table = doc.table(cell.table).unwrap();
    assert_eq!(table.rows.len(), 4);
    assert_eq!(table.grid.len(), 3);
    assert!(
        table.rows[0]
            .cells
            .iter()
            .all(|c| c.borders.top.is_some_and(|b| b.on))
    );
    // "Supplies" spans two rows; "313" two columns.
    assert_eq!(table.rows[1].cells[0].vmerge, VMerge::Restart);
    assert_eq!(table.rows[2].cells[0].vmerge, VMerge::Continue);
    assert_eq!(table.rows[3].cells.len(), 2);
    assert_eq!(table.rows[3].cells[1].span, 2);
    let in_cells: Vec<&str> = doc
        .paragraphs()
        .iter()
        .filter(|p| p.cell.is_some())
        .map(|p| p.text.as_str())
        .collect();
    assert_eq!(
        in_cells,
        [
            "Item", "Q1", "Q2", "Supplies", "120", "135", "", "40", "38", "Total", "313"
        ]
    );
    let header = &doc.paragraphs()[1];
    assert_eq!(header.props_at(1).bold, Some(true));
}

#[test]
fn bullets_and_numbering() {
    let doc = open("lists.doc");
    let m = doc.list_markers();
    let m: Vec<Option<&str>> = m.iter().map(Option::as_deref).collect();
    assert_eq!(
        m,
        [
            None,
            Some("•"),
            Some("•"),
            Some("•"),
            None,
            Some("1."),
            Some("2."),
            Some("1."),
            Some("3."),
            None
        ]
    );
    assert_eq!(
        doc.paragraphs()[7].props.style.as_deref(),
        Some("ListNumber2")
    );
}

#[test]
fn page_setup_sections_headers_and_footers() {
    let doc = open("page.doc");
    let s = doc.sections();
    assert_eq!(s.len(), 2);
    let p = s[0].props.page;
    // US Letter landscape, 3 / 2 / 2.5 / 1.5 cm margins.
    assert_eq!((p.width, p.height), (15840, 12240));
    assert_eq!(
        (p.margin_left, p.margin_right, p.margin_top, p.margin_bottom),
        (1701, 1134, 1417, 850)
    );
    let a4 = s[1].props.page;
    assert_eq!((a4.width, a4.height), (11906, 16838));
    let header = doc
        .stories()
        .iter()
        .find(|st| st.kind == HfKind::Header)
        .expect("a header");
    assert_eq!(header.doc.paragraphs()[0].text, "Kantor header");
    let footer = doc
        .stories()
        .iter()
        .find(|st| st.kind == HfKind::Footer)
        .expect("a footer");
    let fp = &footer.doc.paragraphs()[0];
    assert!(fp.text.starts_with("Page "));
    assert!(
        fp.objects().any(|(_, o)| page_field(o).is_some()),
        "PAGE field"
    );
    assert_eq!(footer.doc.paragraphs().len(), 1, "no guard paragraph");
}

#[test]
fn inline_picture() {
    let doc = open("picture.doc");
    assert_eq!(texts(&doc)[0], "Above the picture.");
    let p = &doc.paragraphs()[1];
    let img = p
        .objects()
        .find_map(|(_, o)| match o {
            Object::Image(i) => Some(i.clone()),
            _ => None,
        })
        .expect("an inline picture");
    // 6 cm wide, 2:1.
    assert!((img.cx - 2_160_000).abs() < 5_000, "{}", img.cx);
    assert!((img.cy * 2 - img.cx).abs() < 5_000);
    let bytes = doc.image_bytes(&img.rel_id).expect("picture bytes");
    assert!(bytes.starts_with(b"\x89PNG"));
}

#[test]
fn template_and_save_as_docx() {
    let dir = tempfile::tempdir().unwrap();
    // A .dot is the same format.
    let dot = dir.path().join("Memo.dot");
    std::fs::copy(corpus("text.doc"), &dot).unwrap();
    let mut doc = Document::open(&dot).unwrap();
    assert_eq!(doc.title(), "Memo.dot");
    assert_eq!(doc.suggested_file_name(), "Memo.docx");
    assert!(doc.save().is_err(), "Save needs a new .docx name");
    let err = doc.save_as(&dot).unwrap_err().to_string();
    assert!(err.contains("Not overwriting Memo.dot"), "{err}");
    assert_eq!(
        std::fs::read(&dot).unwrap(),
        std::fs::read(corpus("text.doc")).unwrap()
    );

    let out = dir.path().join("Memo.docx");
    doc.save_as(&out).unwrap();
    assert!(!doc.is_imported());
    assert_eq!(doc.path(), Some(out.as_path()));
    assert_eq!(doc.title(), "Memo.docx");
    let again = Document::open(&out).unwrap();
    assert!(!again.is_imported());
    assert_eq!(texts(&again), texts(&doc));
    assert!(
        !again.has_unsupported_content(),
        "{:?}",
        again.unsupported_content()
    );
}

#[test]
fn recovery_copy_of_an_import_keeps_save_as() {
    let doc = open("tables.doc");
    let bytes = doc.to_docx_bytes().unwrap();
    let back = Document::open_recovered(bytes, Some(corpus("tables.doc")), Vec::new()).unwrap();
    assert!(back.is_imported());
    assert!(back.path().is_none());
    assert!(back.is_dirty());
    assert_eq!(back.title(), "tables.doc");
}

fn import(bytes: &[u8]) -> anyhow::Result<Document> {
    Document::import_doc(bytes, Path::new("x.doc"))
}

/// The WordDocument stream's first sector: where the FIB starts.
fn fib_offset(bytes: &[u8]) -> usize {
    (512..bytes.len())
        .step_by(512)
        .find(|&o| bytes[o..].starts_with(&[0xEC, 0xA5]))
        .expect("FIB")
}

#[test]
fn refuses_encrypted_old_and_foreign_files() {
    let good = std::fs::read(corpus("text.doc")).unwrap();
    let fib = fib_offset(&good);

    let mut enc = good.clone();
    enc[fib + 0x0B] |= 0x01; // fEncrypted
    let e = format!("{:#}", import(&enc).err().unwrap());
    assert!(e.contains("password-protected"), "{e}");

    let mut old = good.clone();
    old[fib + 2..fib + 4].copy_from_slice(&104u16.to_le_bytes()); // Word 95
    let e = format!("{:#}", import(&old).err().unwrap());
    assert!(e.contains("Word 6 or Word 95"), "{e}");
    // Word 6/95 files also have their own wIdent (0xA5DC): same message,
    // not "not a Word document" (found by the real-world sweep).
    let mut ident = good.clone();
    ident[fib..fib + 2].copy_from_slice(&0xA5DCu16.to_le_bytes());
    ident[fib + 2..fib + 4].copy_from_slice(&101u16.to_le_bytes());
    let e = format!("{:#}", import(&ident).err().unwrap());
    assert!(e.contains("Word 6 or Word 95"), "{e}");

    let e = format!("{:#}", import(b"{\\rtf1 hello}").err().unwrap());
    assert!(e.contains("not a Word 97-2003 document"), "{e}");
}

#[test]
fn truncated_file_errors_cleanly() {
    let good = std::fs::read(corpus("text.doc")).unwrap();
    for len in [0, 8, 511, 512, 1024, good.len() / 2, good.len() - 600] {
        assert!(import(&good[..len]).is_err(), "cut at {len}");
    }
}

/// Damaged files never panic: bytes overwritten all over every fixture
/// (`MSDOC_FUZZ=N` runs N damaged copies of each instead of 150).
#[test]
fn corrupt_bytes_never_panic() {
    let rounds: usize = std::env::var("MSDOC_FUZZ")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(150);
    let mut seed = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for name in [
        "text.doc",
        "tables.doc",
        "lists.doc",
        "page.doc",
        "picture.doc",
    ] {
        let good = std::fs::read(corpus(name)).unwrap();
        for _ in 0..rounds {
            let mut b = good.clone();
            for _ in 0..1 + next() % 8 {
                let at = (next() as usize) % b.len();
                b[at] = match next() % 3 {
                    0 => 0xFF,
                    1 => 0,
                    _ => next() as u8,
                };
            }
            let _ = import(&b);
        }
        // Truncation at every 1/40th.
        for k in 1..40 {
            let _ = import(&good[..good.len() * k / 40]);
        }
    }
}

/// Every floating object of the document.
fn floats(doc: &Document) -> Vec<docs_engine::Float> {
    doc.paragraphs()
        .iter()
        .flat_map(|p| p.objects())
        .filter_map(|(_, o)| match o {
            Object::Float(f) => Some((**f).clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn floating_pictures_and_text_boxes() {
    use docs_engine::float::{Place, RelH, RelV};
    use docs_engine::{FloatKind, Wrap};
    // floats.docx as LibreOffice saves it in Word 97 format: two pictures,
    // a text box, and shapes as freeforms drawn with guide formulas.
    let doc = open("floats.doc");
    // LibreOffice's arcs (the rounded rectangle, the ellipse) are followed.
    assert!(doc.import_notes().is_empty(), "{:?}", doc.import_notes());
    let f = floats(&doc);
    assert_eq!(f.len(), 6, "{f:?}");
    assert!(
        f.iter()
            .filter(|x| x.xml.contains("custGeom"))
            .any(|x| x.xml.contains("cubicBezTo")),
        "arcs as curves"
    );
    // The freeforms' paths, their guides worked out: inside their boxes.
    for x in f.iter().filter(|x| x.xml.contains("custGeom")) {
        let xml = &*x.xml;
        assert!(xml.contains("<a:path w=\""), "{xml}");
        let pts: Vec<i64> = xml
            .split("<a:pt x=\"")
            .skip(1)
            .filter_map(|s| s.split('"').next()?.parse().ok())
            .collect();
        assert!(!pts.is_empty() && pts.iter().all(|&v| v >= 0), "{pts:?}");
    }
    let pictures: Vec<_> = f
        .iter()
        .filter(|x| matches!(x.kind, FloatKind::Picture { .. }))
        .collect();
    assert_eq!(pictures.len(), 2);
    let square = pictures[0];
    assert_eq!(square.at.wrap, Wrap::Square);
    assert_eq!(square.at.h.0, RelH::Column);
    assert_eq!(square.at.v.0, RelV::Paragraph);
    // 160 x 120 px.
    let (w, h) = square.at.size_px();
    assert!(
        (w - 160.0).abs() < 1.0 && (h - 120.0).abs() < 1.0,
        "{w} x {h}"
    );
    let FloatKind::Picture { crop, rel_id, .. } = &pictures[1].kind else {
        unreachable!()
    };
    assert_eq!(crop[0], 25000);
    assert!(doc.image_bytes(rel_id).unwrap().starts_with(b"\x89PNG"));
    assert_eq!(pictures[1].at.wrap, Wrap::TopAndBottom);
    assert!(matches!(pictures[1].at.v.1, Place::Offset(_)));
    let text_box = f
        .iter()
        .find(|x| x.kind == FloatKind::Shape && float_texts(x).len() == 2)
        .expect("the text box");
    assert_eq!(float_texts(text_box), ["Text box", "with two paragraphs"]);
    save_and_reopen(doc, 6);
}

fn float_texts(f: &docs_engine::Float) -> Vec<String> {
    f.text
        .as_ref()
        .map(|t| t.doc.paragraphs().iter().map(|p| p.text.clone()).collect())
        .unwrap_or_default()
}

/// Saved as .docx and opened again: the same objects in the same places.
fn save_and_reopen(mut doc: Document, n: usize) {
    let before = floats(&doc);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("out.docx");
    doc.save_as(&out).unwrap();
    let again = Document::open(&out).unwrap();
    let after = floats(&again);
    assert_eq!(after.len(), n);
    assert_eq!(
        after.iter().map(|x| x.at).collect::<Vec<_>>(),
        before.iter().map(|x| x.at).collect::<Vec<_>>()
    );
    assert_eq!(
        after.iter().map(float_texts).collect::<Vec<_>>(),
        before.iter().map(float_texts).collect::<Vec<_>>()
    );
}

#[test]
fn floating_shapes() {
    use docs_engine::float::RelH;
    use docs_engine::{FloatKind, LayoutOption, Wrap};
    // LibreOffice's own shapes, saved as Word's preset types.
    let doc = open("shapes.doc");
    assert!(doc.import_notes().is_empty(), "{:?}", doc.import_notes());
    let f = floats(&doc);
    assert_eq!(f.len(), 5, "{f:?}");
    assert!(f.iter().all(|x| x.kind == FloatKind::Shape));
    let geom = |x: &docs_engine::Float| x.geometry_xml().unwrap_or_default().to_owned();
    let by_geom = |g: &str| {
        f.iter()
            .find(|x| geom(x).contains(&format!("prst=\"{g}\"")))
            .unwrap_or_else(|| panic!("{g}"))
    };
    let rect = by_geom("rect");
    assert_eq!(float_texts(rect), ["Revenue"]);
    assert!(rect.shape_xml().0.unwrap().contains("4472C4"));
    assert_eq!(rect.at.wrap, Wrap::Square);
    let ellipse = by_geom("ellipse");
    assert_eq!(ellipse.at.layout_option(), LayoutOption::Behind);
    assert!(ellipse.shape_xml().0.unwrap().contains("DDEBFF"));
    let round = by_geom("roundRect");
    assert_eq!(float_texts(round), ["Rounded"]);
    let line = by_geom("line");
    assert!(
        line.shape_xml()
            .0
            .unwrap()
            .contains("<a:tailEnd type=\"triangle\"/>")
    );
    assert_eq!(line.at.layout_option(), LayoutOption::InFront);
    let frame = f
        .iter()
        .find(|x| float_texts(x).len() == 2)
        .expect("the text frame");
    assert_eq!(float_texts(frame), ["A text frame", "with two lines"]);
    assert_eq!(frame.at.h.0, RelH::Column);
    save_and_reopen(doc, 5);
}

#[test]
fn comments_with_their_ranges() {
    let doc = open("comments.doc");
    assert!(doc.import_notes().is_empty(), "{:?}", doc.import_notes());
    let c = doc.comments();
    assert_eq!(c.len(), 3, "{c:?}");
    assert_eq!(
        (
            c[0].author.as_str(),
            c[0].initials.as_str(),
            c[0].text.as_str()
        ),
        ("Ani Wijaya", "AW", "Check the totals.")
    );
    assert_eq!(c[1].author, "Budi Santoso");
    assert_eq!(c[1].text, "Two runs.\nSecond line.");
    assert_eq!(c[2].text, "On a whole paragraph.");
    // Their dates (AtrdExtra), to the minute.
    assert!(
        c.iter()
            .all(|c| c.date.len() == 20 && c.date.ends_with(":00Z")),
        "{:?}",
        c.iter().map(|c| &c.date).collect::<Vec<_>>()
    );
    let anchors = doc.comment_anchors();
    assert_eq!(anchors.len(), 3, "{anchors:?}");
    // The first covers "budget"; the second crosses two runs.
    let covered = |i: usize| {
        let a = anchors[i].1;
        assert_eq!(a.start.para, a.end.para);
        doc.paragraphs()[a.start.para].text[a.start.offset..a.end.offset].replace('\u{FFFC}', "")
    };
    assert_eq!(covered(0), "budget");
    assert_eq!(covered(1), "Bold start and plain end.");

    // Saved as .docx: the same comments.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("comments.docx");
    let mut doc = doc;
    doc.save_as(&out).unwrap();
    let again = Document::open(&out).unwrap();
    assert_eq!(again.comments().len(), 3);
    assert_eq!(again.comment_anchors().len(), 3);
}

/// revisions.doc: tracked changes come in as revisions, with their
/// authors and dates, not accepted.
#[test]
fn tracked_changes_come_in_as_revisions() {
    let doc = open("revisions.doc");
    let changes = doc.changes();
    let summary: Vec<(String, String, String)> = changes
        .iter()
        .map(|c| (format!("{:?}", c.kind), c.author.clone(), c.text.clone()))
        .collect();
    assert!(
        summary
            .iter()
            .any(|(k, a, t)| k.contains("Ins") && a == "Citra Dewi" && t == "is renewed "),
        "{summary:?}"
    );
    assert!(
        summary
            .iter()
            .any(|(k, a, t)| k.contains("Del") && a == "Budi Santoso" && t == " or two"),
        "{summary:?}"
    );
    let ins = changes.iter().find(|c| c.author == "Citra Dewi").unwrap();
    assert_eq!(ins.date, "2026-11-10T09:15:00Z");
    // Accept all: the text as the authors meant it.
    let mut doc = doc;
    doc.resolve_all(true, docs_engine::Selection::default());
    assert_eq!(
        doc.paragraphs()[0].text.replace('\u{FFFC}', ""),
        "The contract is renewed for one year."
    );
    assert!(
        doc.paragraphs()
            .iter()
            .any(|p| p.text == "This paragraph's end was deleted,so it joins this one."),
        "{:?}",
        texts(&doc)
    );
}

/// nested.doc: a table inside a cell stays a table inside the cell (as
/// Kantor keeps nested tables of .docx files), saved nested.
#[test]
fn nested_tables_are_kept_nested() {
    let doc = open("nested.doc");
    assert!(
        !doc.import_notes().iter().any(|n| n.contains("nested")),
        "{:?}",
        doc.import_notes()
    );
    let t = texts(&doc);
    assert!(t.iter().any(|p| p.contains("Outer A1")), "{t:?}");
    assert!(
        t.iter().any(|p| p.contains("After the inner table.")),
        "{t:?}"
    );
    assert!(
        t.iter().any(|p| p.contains("After the outer table.")),
        "{t:?}"
    );
    // The inner table isn't flattened into the cell's paragraphs.
    assert!(
        !t.iter()
            .any(|p| p.trim_start_matches('\u{FFFC}') == "In A1"),
        "{t:?}"
    );
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("nested.docx");
    let mut doc = doc;
    doc.save_as(&out).unwrap();
    let bytes = std::fs::read(&out).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    std::io::Read::read_to_string(&mut zip.by_name("word/document.xml").unwrap(), &mut xml)
        .unwrap();
    // A w:tbl inside a w:tc.
    let outer = xml.find("<w:tbl>").unwrap();
    let inner = xml[outer + 7..]
        .find("<w:tbl>")
        .map(|i| i + outer + 7)
        .unwrap();
    assert!(xml[outer..inner].contains("<w:tc>"), "nested");
    assert!(xml[inner..].contains(">In B2<"));
}

/// hdrfloat.doc: a floating picture in the header comes in, in the
/// header, its picture in the header's own relationships.
#[test]
fn header_floating_objects_come_in() {
    let doc = open("hdrfloat.doc");
    assert!(
        !doc.import_notes().iter().any(|n| n.contains("header")),
        "{:?}",
        doc.import_notes()
    );
    let st = doc
        .stories()
        .iter()
        .find(|s| {
            s.kind == HfKind::Header
                && s.doc
                    .paragraphs()
                    .iter()
                    .any(|p| p.text.contains("Header text"))
        })
        .expect("the header");
    let floats: Vec<_> = st
        .doc
        .paragraphs()
        .iter()
        .flat_map(|p| p.objects())
        .filter_map(|(_, o)| match o {
            Object::Float(f) => Some(f.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(floats.len(), 1, "the logo");
    let docs_engine::FloatKind::Picture { rel_id, .. } = &floats[0].kind else {
        panic!("{:?}", floats[0].kind);
    };
    assert!(st.doc.image_bytes(rel_id).is_some(), "its picture");
    let mut doc = doc;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("hdrfloat.docx");
    doc.save_as(&out).unwrap();
    let again = Document::open(&out).unwrap();
    assert!(again.stories().iter().any(|s| {
        s.doc
            .paragraphs()
            .iter()
            .any(|p| p.objects().any(|(_, o)| matches!(o, Object::Float(_))))
    }));
}

/// EMF and WMF pictures (metafiles.doc, from scripts/corpus/gen_metafiles.py)
/// come in as they are and are drawn as PNG.
#[test]
fn metafile_pictures() {
    let doc = open("metafiles.doc");
    let imgs: Vec<_> = doc
        .paragraphs()
        .iter()
        .flat_map(|p| p.objects().map(|(_, o)| o.clone()).collect::<Vec<_>>())
        .filter_map(|o| match o {
            Object::Image(i) => Some(i),
            _ => None,
        })
        .collect();
    assert_eq!(imgs.len(), 2, "{:?}", doc.import_notes());
    for img in &imgs {
        let bytes = doc.image_bytes(&img.rel_id).expect("picture bytes");
        assert!(kantor_metafile::kind(&bytes).is_some(), "{}", img.rel_id);
        let (format, png) = docs_engine::drawable_picture(bytes).expect("drawn");
        assert_eq!(format, docs_engine::ImageFormat::Png);
        assert!(png.starts_with(b"\x89PNG"));
    }
    assert!(
        !doc.import_notes().iter().any(|n| n.contains("picture")),
        "{:?}",
        doc.import_notes()
    );
}

/// A .docx keeps its EMF and WMF parts and draws them.
#[test]
fn docx_metafiles_are_drawn() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/docx/metafiles.docx");
    let doc = Document::open(&path).unwrap();
    let mut n = 0;
    for p in doc.paragraphs() {
        for (_, o) in p.objects() {
            if let Object::Image(i) = o {
                let bytes = doc.image_bytes(&i.rel_id).expect("kept");
                assert!(docs_engine::drawable_picture(bytes).is_some());
                n += 1;
            }
        }
    }
    assert_eq!(n, 2);
}

/// Grouped shapes (drawings.doc: drawings.docx as LibreOffice saves it in
/// Word 97 format) come in as groups with their shapes.
#[test]
fn grouped_shapes() {
    use docs_engine::FloatKind;
    let doc = open("drawings.doc");
    assert!(doc.import_notes().is_empty(), "{:?}", doc.import_notes());
    let f = floats(&doc);
    let groups: Vec<_> = f
        .iter()
        .filter(|x| matches!(x.kind, FloatKind::Group { .. }))
        .collect();
    assert_eq!(groups.len(), 2, "{f:?}");
    for g in &groups {
        let xml = &*g.xml;
        assert!(
            xml.contains("<wpg:wgp>") && xml.contains("<a:chExt "),
            "{xml}"
        );
        assert!(xml.matches("<wps:wsp>").count() >= 2, "{xml}");
    }
    // The chart LibreOffice saved as an EMF picture.
    assert!(
        f.iter()
            .any(|x| matches!(&x.kind, FloatKind::Picture { part, .. } if part.ends_with(".emf")))
    );
    // Saved as .docx and opened again: still groups.
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("drawings.docx");
    let mut doc = doc;
    doc.save_as(&out).unwrap();
    let again = Document::open(&out).unwrap();
    assert_eq!(
        floats(&again)
            .iter()
            .filter(|x| matches!(x.kind, FloatKind::Group { .. }))
            .count(),
        2
    );
}
