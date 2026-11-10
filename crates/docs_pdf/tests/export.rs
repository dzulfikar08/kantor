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
    let paged = docs_pdf::lay_out(&doc, &mut engine);
    let scene = docs_pdf::scene(&doc, &paged);
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
    let paged = docs_pdf::lay_out(&doc, &mut engine);
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
    let paged = docs_pdf::lay_out(&doc, &mut engine);
    let scene = docs_pdf::scene(&doc, &paged);
    assert_eq!(scene.pages[0].pictures.len(), 1);
    let pdf = docs_pdf::write(&scene).unwrap();
    assert_eq!(page_count(&pdf), 1);
    assert_eq!(count(&pdf, b"/Subtype/Image"), 0);
}
