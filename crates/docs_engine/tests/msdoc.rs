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

    let e = format!("{:#}", import(b"{\\rtf1 hello}").err().unwrap());
    assert!(e.contains("Can't open x.doc"), "{e}");
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
