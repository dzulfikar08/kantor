//! PDF export of real `.docx` files: built with docs_engine, saved, opened
//! again and exported. Checks the PDF's structure (pages, embedded fonts
//! with ToUnicode, pictures, colours) by reading it back, so nothing here
//! depends on which system fonts lay the text out (CI fonts differ).

mod fixture;

use std::io::Read as _;

use docs_engine::{Document, Selection};
use docs_layout::Engine;
use fixture::report;

/// Every stream of the PDF, inflated when it's Flate-compressed.
fn streams(pdf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = find(&pdf[at..], b"stream") {
        let start = at + i + b"stream".len();
        // Not "endstream".
        if at + i >= 3 && &pdf[at + i - 3..at + i] == b"end" {
            at = start;
            continue;
        }
        let body = start
            + match pdf.get(start..start + 2) {
                Some(b"\r\n") => 2,
                _ => 1,
            };
        let Some(end) = find(&pdf[body..], b"endstream") else {
            break;
        };
        let raw = &pdf[body..body + end];
        let mut inflated = Vec::new();
        let ok = flate2::read::ZlibDecoder::new(raw)
            .read_to_end(&mut inflated)
            .is_ok();
        out.push(if ok && !inflated.is_empty() {
            inflated
        } else {
            raw.to_vec()
        });
        at = body + end;
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn count(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

/// Page objects (`/Type /Page`, not `/Pages`).
fn page_count(pdf: &[u8]) -> usize {
    count(pdf, b"/Type/Page") - count(pdf, b"/Type/Pages")
}

/// Code points the PDF's ToUnicode CMaps map glyphs to.
fn mapped_chars(pdf: &[u8]) -> String {
    let mut chars = String::new();
    for s in streams(pdf) {
        let text = String::from_utf8_lossy(&s);
        if !text.contains("begincmap") {
            continue;
        }
        for block in text.split("beginbfchar").skip(1) {
            let block = block.split("endbfchar").next().unwrap_or_default();
            for line in block.lines() {
                // `<cid> <utf16be hex>`
                let Some(hex) = line.split('<').nth(2) else {
                    continue;
                };
                let hex = hex.trim_end_matches(|c: char| c == '>' || c.is_whitespace());
                let units: Vec<u16> = (0..hex.len() / 4)
                    .filter_map(|k| u16::from_str_radix(&hex[k * 4..k * 4 + 4], 16).ok())
                    .collect();
                chars.extend(char::decode_utf16(units).filter_map(Result::ok));
            }
        }
    }
    chars
}

#[test]
fn report_exports_with_text_fonts_rules_and_picture() {
    let dir = tempfile::tempdir().unwrap();
    let doc = report(dir.path(), true, 0);
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let paged = &laid.paged;
    let scene = docs_pdf::scene(&doc, &laid);
    let pdf = docs_pdf::write(&scene).unwrap();

    assert!(pdf.starts_with(b"%PDF-"), "PDF header");
    assert!(pdf.trim_ascii_end().ends_with(b"%%EOF"));
    assert_eq!(page_count(&pdf), paged.pages.len());
    // Fonts are embedded as subsets with ToUnicode, so text extracts.
    assert!(count(&pdf, b"/FontFile2") + count(&pdf, b"/FontFile3") > 0);
    assert!(count(&pdf, b"/ToUnicode") > 0);
    let chars = mapped_chars(&pdf);
    for c in "LaporanPemeliharaKtsdiknu.12".chars() {
        assert!(chars.contains(c), "{c:?} has no ToUnicode entry: {chars:?}");
    }
    assert!(
        !chars.contains('\u{FFFC}'),
        "the object character isn't text"
    );
    // The picture, the table borders and the red run.
    assert_eq!(scene.pages[0].pictures.len(), 1);
    assert!(count(&pdf, b"/Subtype/Image") >= 1, "picture embedded");
    assert!(!scene.pages[0].rules.is_empty(), "table borders");
    let content: Vec<u8> = streams(&pdf).concat();
    assert!(find(&content, b"0.7529412 0 0 rg").is_some(), "red text");
    let rules: usize = scene.pages.iter().map(|p| p.rules.len()).sum();
    assert!(
        count(&content, b"\nh\nf") >= rules,
        "one filled path per rule"
    );
    // A4 from the document's section properties: 595.3 × 841.9 pt.
    assert!(find(&pdf, b"/MediaBox[0 0 595.").is_some());

    // pdftotext (poppler), when installed, reads the same text back.
    let file = dir.path().join("report.pdf");
    docs_pdf::write_file(&file, &pdf).unwrap();
    if let Ok(out) = std::process::Command::new("pdftotext")
        .arg(&file)
        .arg("-")
        .output()
    {
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("Laporan Pemeliharaan"), "{text}");
        assert!(text.contains("naik pada kuartal"), "{text}");
    }
}

#[test]
fn page_count_matches_layout_and_page_size_follows_document() {
    let dir = tempfile::tempdir().unwrap();
    let mut doc = report(dir.path(), false, 120);
    doc.set_page(
        docs_engine::PageGeometry::default()
            .with_paper(docs_engine::PaperSize::Letter)
            .with_orientation(docs_engine::Orientation::Landscape),
        Selection::default(),
    );
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let paged = &laid.paged;
    assert!(paged.pages.len() > 2, "{} pages", paged.pages.len());
    let pdf = docs_pdf::export(&doc, &mut engine).unwrap();
    assert_eq!(page_count(&pdf), paged.pages.len());
    assert_eq!(count(&pdf, b"/Subtype/Image"), 0);
    // US Letter landscape: 792 × 612 pt.
    assert!(find(&pdf, b"/MediaBox[0 0 792 612]").is_some());
}

#[test]
fn empty_document_is_one_blank_page() {
    let doc = Document::new();
    let pdf = docs_pdf::export(&doc, &mut Engine::new()).unwrap();
    assert_eq!(page_count(&pdf), 1);
}

#[test]
fn a_picture_that_does_not_decode_is_left_out() {
    // Only a PNG header: the size reads, the pixels don't.
    let mut fake = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    fake.extend(200u32.to_be_bytes());
    fake.extend(100u32.to_be_bytes());
    fake.extend([8, 6, 0, 0, 0]);
    let mut doc = Document::new();
    let c = doc.insert_text(Selection::default(), "Lihat ", None);
    doc.insert_image(c, fake, "rusak.png").unwrap();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let scene = docs_pdf::scene(&doc, &laid);
    assert_eq!(scene.pages[0].pictures.len(), 1);
    let pdf = docs_pdf::write(&scene).unwrap();
    assert_eq!(page_count(&pdf), 1);
    assert_eq!(count(&pdf, b"/Subtype/Image"), 0);
}

fn corpus(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/corpus/docx")
        .join(name)
}

/// pdftotext's text of a PDF, when poppler is installed.
fn pdftotext(pdf: &[u8], dir: &std::path::Path) -> Option<String> {
    let file = dir.join("out.pdf");
    std::fs::write(&file, pdf).unwrap();
    let out = std::process::Command::new("pdftotext")
        .arg("-layout")
        .arg(&file)
        .arg("-")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[test]
fn headers_footers_and_sections_are_in_the_pdf() {
    let doc = Document::open(&corpus("report.docx")).unwrap();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    let n = laid.paged.pages.len();
    assert!(n >= 4, "{n} pages");
    // The landscape section's page is landscape.
    let sizes: Vec<(f32, f32)> = (0..n)
        .map(|i| {
            let s = laid.paged.page_setup(i);
            (s.width, s.height)
        })
        .collect();
    assert!(sizes.iter().any(|(w, h)| w > h), "{sizes:?}");
    assert!(sizes[0].0 < sizes[0].1);
    // First page: the first-page header, no footer (none defined).
    let kinds = |i: usize| laid.furniture[i].iter().map(|p| p.kind).collect::<Vec<_>>();
    assert_eq!(kinds(0), [docs_engine::HfKind::Header]);
    assert_eq!(laid.furniture[0][0].layout.text(0), "CONFIDENTIAL");
    // Page 2: header and footer with its number.
    assert_eq!(kinds(1).len(), 2);
    let footer = &laid.furniture[1][1].layout;
    assert_eq!(footer.text(0), format!("Page 2 of {n}"));
    // Body text starts below the header.
    let top = laid.paged.pages[1].lines[0].top;
    let header = &laid.furniture[1][0];
    assert!(top >= header.y + header.layout.height - 0.5);

    let scene = docs_pdf::scene(&doc, &laid);
    let pdf = docs_pdf::write(&scene).unwrap();
    assert_eq!(page_count(&pdf), n);
    // Mixed page sizes: A4 portrait and landscape.
    assert!(find(&pdf, b"/MediaBox[0 0 595.").is_some());
    assert!(find(&pdf, b"/MediaBox[0 0 841.").is_some());
    let dir = tempfile::tempdir().unwrap();
    if let Some(text) = pdftotext(&pdf, dir.path()) {
        assert!(text.contains("CONFIDENTIAL"), "{text}");
        assert!(text.contains("Quarterly Operations Report"), "{text}");
        assert!(text.contains(&format!("Page 2 of {n}")), "{text}");
        assert!(text.contains(&format!("Page 3 of {n}")), "{text}");
    }
}

#[test]
fn odd_and_even_footers_alternate() {
    let doc = Document::open(&corpus("letter.docx")).unwrap();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    assert!(laid.paged.pages.len() >= 2);
    let footer = |i: usize| {
        laid.furniture[i]
            .iter()
            .find(|p| p.kind == docs_engine::HfKind::Footer)
            .map(|p| p.layout.text(0).to_owned())
    };
    assert_eq!(footer(0).as_deref(), Some("odd footer"));
    assert_eq!(footer(1).as_deref(), Some("even footer"));
    // No even header: page 2 has none.
    assert!(
        !laid.furniture[1]
            .iter()
            .any(|p| p.kind == docs_engine::HfKind::Header)
    );
}

#[test]
fn new_footer_with_page_numbers_round_trips_into_the_pdf() {
    let mut doc = Document::new();
    let mut c = Selection::default();
    for i in 0..60 {
        c = doc.insert_text(
            c,
            &format!("Paragraph number {i} of the long document.\n"),
            None,
        );
    }
    assert!(doc.set_hf_text(
        0,
        docs_engine::HfKind::Footer,
        docs_engine::HfType::Default,
        "Halaman {PAGE} dari {PAGES}",
        Some(docs_engine::Align::Right),
        Selection::default(),
    ));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("n.docx");
    doc.save_as(&path).unwrap();
    let doc = Document::open(&path).unwrap();
    let pdf = docs_pdf::export(&doc, &mut Engine::new()).unwrap();
    let n = page_count(&pdf);
    assert!(n >= 2);
    if let Some(text) = pdftotext(&pdf, dir.path()) {
        assert!(text.contains(&format!("Halaman 1 dari {n}")), "{text}");
        assert!(text.contains(&format!("Halaman {n} dari {n}")), "{text}");
    }
}

#[test]
fn tracked_changes_print_with_markup() {
    let doc = Document::open(&corpus("minutes.docx")).unwrap();
    let mut engine = Engine::new();
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    // " or Monday" is struck through: one rule in the author's colour.
    let strikes = laid.strike_rules(0);
    assert_eq!(strikes.len(), 1, "{strikes:?}");
    let colour = docs_pdf::build::author_color("Citra");
    assert_eq!(strikes[0].color, colour);
    let scene = docs_pdf::scene(&doc, &laid);
    assert!(scene.pages[0].rules.iter().any(|(_, c)| *c == colour));
    // The inserted and deleted text are both in the PDF, in that colour.
    let pdf = docs_pdf::write(&scene).unwrap();
    let dir = tempfile::tempdir().unwrap();
    if let Some(text) = pdftotext(&pdf, dir.path()) {
        assert!(text.contains("signed draft by Friday or Monday."), "{text}");
    }
    // After Accept All the PDF has no markup left.
    let mut doc = doc;
    doc.resolve_all(true, Selection::default());
    let laid = docs_pdf::lay_out(&doc, &mut engine);
    assert!(laid.strike_rules(0).is_empty());
}
