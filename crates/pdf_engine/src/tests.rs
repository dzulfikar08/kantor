//! Engine tests on PDFs written here with lopdf (synthetic fixtures).

use std::sync::Arc;

use lopdf::{Document, Object, Stream, dictionary};

use crate::{Dest, LinkTarget, OpenError, PdfDoc, Renderer, Turn, find_in};

/// One page per entry of `pages`; each entry is the lines on that page,
/// drawn in Helvetica 12 pt from the top (72 pt margin, 20 pt apart). Page
/// 2 (when there is one) links to page 1 and to a web address; the outline
/// has "Chapter 1" (page 1, with a child "Section 1.1" → named destination)
/// and "Chapter 2" (page 2).
fn document(pages: &[&[&str]]) -> Document {
    let mut doc = Document::with_version("1.5");
    doc.trailer.set(
        "ID",
        Object::Array(vec![
            Object::string_literal(b"0123456789abcdef"),
            Object::string_literal(b"fedcba9876543210"),
        ]),
    );
    let pages_id = doc.new_object_id();
    let font_id = doc.add_object(dictionary! {
        "Type" => "Font",
        "Subtype" => "Type1",
        "BaseFont" => "Helvetica",
        "Encoding" => "WinAnsiEncoding",
    });
    let mut page_ids = Vec::new();
    for lines in pages {
        let mut content = String::from("BT\n/F1 12 Tf\n");
        for (i, line) in lines.iter().enumerate() {
            let y = 720 - 20 * i;
            content.push_str(&format!("1 0 0 1 72 {y} Tm\n({line}) Tj\n"));
        }
        content.push_str("ET\n");
        let contents_id = doc.add_object(Stream::new(dictionary! {}, content.into_bytes()));
        let id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
            "Resources" => dictionary! { "Font" => dictionary! { "F1" => font_id } },
            "Contents" => contents_id,
        });
        page_ids.push(id);
    }
    if page_ids.len() > 1 {
        let to_first = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "Rect" => vec![72.into(), 700.into(), 200.into(), 730.into()],
            "Dest" => vec![page_ids[0].into(), "XYZ".into(), 0.into(), 500.into(), Object::Null],
        });
        let web = doc.add_object(dictionary! {
            "Type" => "Annot",
            "Subtype" => "Link",
            "Rect" => vec![72.into(), 600.into(), 200.into(), 620.into()],
            "A" => dictionary! { "S" => "URI", "URI" => Object::string_literal("https://example.com/") },
        });
        if let Ok(Object::Dictionary(p)) = doc.get_object_mut(page_ids[1]) {
            p.set("Annots", vec![to_first.into(), web.into()]);
        }
    }
    let kids: Vec<Object> = page_ids.iter().map(|&id| id.into()).collect();
    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => kids,
            "Count" => page_ids.len() as i64,
        }),
    );

    // Outline.
    let outlines_id = doc.new_object_id();
    let ch1 = doc.new_object_id();
    let sec = doc.new_object_id();
    let last = *page_ids.last().unwrap();
    let ch2 = doc.add_object(dictionary! {
        "Title" => Object::string_literal("Chapter 2"),
        "Parent" => outlines_id,
        "Prev" => ch1,
        "Dest" => vec![last.into(), "Fit".into()],
    });
    doc.objects.insert(
        sec,
        Object::Dictionary(dictionary! {
            "Title" => Object::String(
                // UTF-16BE: "Section 1.1"
                {
                    let mut b = vec![0xFE, 0xFF];
                    for u in "Section 1.1".encode_utf16() {
                        b.extend(u.to_be_bytes());
                    }
                    b
                },
                lopdf::StringFormat::Hexadecimal,
            ),
            "Parent" => ch1,
            "Dest" => Object::string_literal("sec11"),
        }),
    );
    doc.objects.insert(
        ch1,
        Object::Dictionary(dictionary! {
            "Title" => Object::string_literal("Chapter 1"),
            "Parent" => outlines_id,
            "Next" => ch2,
            "First" => sec,
            "Last" => sec,
            "Count" => 1,
            "A" => dictionary! { "S" => "GoTo", "D" => vec![page_ids[0].into(), "Fit".into()] },
        }),
    );
    doc.objects.insert(
        outlines_id,
        Object::Dictionary(dictionary! {
            "Type" => "Outlines",
            "First" => ch1,
            "Last" => ch2,
            "Count" => 3,
        }),
    );
    let names = doc.add_object(dictionary! {
        "Names" => vec![
            Object::string_literal("sec11"),
            vec![page_ids[0].into(), "XYZ".into(), 72.into(), 700.into(), Object::Null].into(),
        ],
    });
    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => pages_id,
        "Outlines" => outlines_id,
        "Names" => dictionary! { "Dests" => names },
    });
    doc.trailer.set("Root", catalog_id);
    doc
}

fn bytes(mut doc: Document) -> Arc<Vec<u8>> {
    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    Arc::new(out)
}

fn sample() -> Arc<Vec<u8>> {
    bytes(document(&[
        &["Hello world", "Kantor reads PDF files"],
        &["Second page", "Visit the web site", "hello again"],
    ]))
}

#[test]
fn opens_and_counts_pages() {
    let doc = PdfDoc::open(sample(), "").unwrap();
    assert_eq!(doc.page_count(), 2);
    assert_eq!(doc.page_sizes(), vec![(612.0, 792.0), (612.0, 792.0)]);
}

#[test]
fn damaged_files_are_refused() {
    let err = PdfDoc::open(Arc::new(b"not a pdf at all".to_vec()), "").err();
    assert_eq!(err, Some(OpenError::Damaged));
    let err = PdfDoc::open(Arc::new(Vec::new()), "").err();
    assert_eq!(err, Some(OpenError::Damaged));
}

#[test]
fn text_has_lines_and_boxes() {
    let doc = PdfDoc::open(sample(), "").unwrap();
    let t = doc.text(0);
    assert_eq!(t.text, "Hello world\nKantor reads PDF files");
    // "H" sits at x = 72, on the first line (top 72 pt below the top edge).
    let h = t.boxes[0];
    assert!((h.rect[0] - 72.0).abs() < 0.5, "{h:?}");
    assert!(h.rect[1] > 60.0 && h.rect[3] < 80.0, "{h:?}");
    // Copy: a range of the text, and the rectangles that cover it.
    let r = t.text.find("reads").unwrap();
    assert_eq!(&t.text[r..r + 5], "reads");
    let rects = t.rects(r..r + 5);
    assert_eq!(rects.len(), 1);
    assert!(rects[0][1] > 80.0, "second line: {rects:?}");
    // Hit testing picks the character under the point.
    let (x, y) = ((h.rect[0] + h.rect[2]) / 2.0 - 1.0, (h.rect[1] + h.rect[3]) / 2.0);
    assert_eq!(t.offset_at(x, y), Some(0));
    assert_eq!(t.word_at(2), 0..5);
    assert_eq!(t.line_at(15), 12..t.text.len());
}

#[test]
fn search_finds_hits_on_every_page() {
    let doc = PdfDoc::open(sample(), "").unwrap();
    let hits: Vec<_> = (0..doc.page_count())
        .flat_map(|p| find_in(p, &doc.text(p), "hello", false, false))
        .collect();
    assert_eq!(hits.len(), 2);
    assert_eq!((hits[0].page, hits[1].page), (0, 1));
    assert!(find_in(0, &doc.text(0), "hello", true, false).is_empty());
    // A phrase across a line break is found.
    assert_eq!(find_in(0, &doc.text(0), "world kantor", false, false).len(), 1);
    // Whole words only.
    assert!(find_in(0, &doc.text(0), "hell", false, true).is_empty());
}

#[test]
fn outline_is_read() {
    let doc = PdfDoc::open(sample(), "").unwrap();
    let o = doc.outline();
    assert_eq!(o.len(), 2);
    assert_eq!(o[0].title, "Chapter 1");
    assert!(o[0].open);
    assert_eq!(
        o[0].target,
        Some(LinkTarget::Dest(Dest {
            page: 0,
            x: None,
            y: None
        }))
    );
    assert_eq!(o[0].children[0].title, "Section 1.1");
    // Named destination: [page /XYZ 72 700 null] → 92 pt from the top.
    assert_eq!(
        o[0].children[0].target,
        Some(LinkTarget::Dest(Dest {
            page: 0,
            x: Some(72.0),
            y: Some(92.0)
        }))
    );
    assert_eq!(o[1].title, "Chapter 2");
    assert!(matches!(&o[1].target, Some(LinkTarget::Dest(d)) if d.page == 1));
}

#[test]
fn links_are_read() {
    let doc = PdfDoc::open(sample(), "").unwrap();
    assert!(doc.links(0).is_empty());
    let links = doc.links(1);
    assert_eq!(links.len(), 2);
    assert_eq!(links[0].rect, [72.0, 62.0, 200.0, 92.0]);
    assert_eq!(
        links[0].target,
        LinkTarget::Dest(Dest {
            page: 0,
            x: Some(0.0),
            y: Some(292.0)
        })
    );
    assert_eq!(links[1].target, LinkTarget::Uri("https://example.com/".into()));
}

#[test]
fn password_protected_files_ask_for_the_password() {
    let mut doc = document(&[&["Secret plans"]]);
    let state = lopdf::EncryptionState::try_from(lopdf::EncryptionVersion::V2 {
        document: &doc,
        owner_password: "owner",
        user_password: "kantor",
        key_length: 128,
        permissions: lopdf::Permissions::all(),
    })
    .unwrap();
    doc.encrypt(&state).unwrap();
    let data = bytes(doc);
    assert_eq!(
        PdfDoc::open(data.clone(), "").err(),
        Some(OpenError::Password { wrong: false })
    );
    assert_eq!(
        PdfDoc::open(data.clone(), "nope").err(),
        Some(OpenError::Password { wrong: true })
    );
    let doc = PdfDoc::open(data, "kantor").unwrap();
    assert_eq!(doc.text(0).text, "Secret plans");
    assert_eq!(doc.outline()[0].title, "Chapter 1");
}

#[test]
fn renders_pages_turned() {
    let doc = PdfDoc::open(sample(), "").unwrap();
    let r = Renderer::new(&doc);
    let page = r.render(0, 0.5, Turn(0)).unwrap();
    assert_eq!((page.width, page.height), (306, 396));
    assert_eq!(page.rgba.len(), 306 * 396 * 4);
    // Some dark pixels (the text), the rest white.
    let dark = page.rgba.chunks(4).filter(|p| p[0] < 128).count();
    assert!(dark > 20, "{dark}");
    let turned = r.render(0, 0.5, Turn(1)).unwrap();
    assert_eq!((turned.width, turned.height), (396, 306));
    // Huge scales are capped.
    let big = r.render(0, 100.0, Turn(0)).unwrap();
    assert!(big.height <= crate::MAX_SIDE as u32);
}

#[test]
fn turns_map_and_unmap() {
    for k in 0..4 {
        let t = Turn(k);
        let (x, y) = t.map(100.0, 50.0, 10.0, 20.0);
        assert_eq!(t.unmap(100.0, 50.0, x, y), (10.0, 20.0));
        let (w, h) = t.size(100.0, 50.0);
        assert!(x >= 0.0 && x <= w && y >= 0.0 && y <= h);
    }
    assert_eq!(Turn(3).cw(), Turn(0));
    assert_eq!(Turn(0).ccw(), Turn(3));
}

#[test]
fn long_files_open_and_search_quickly() {
    let lines: Vec<String> = (1..=520)
        .map(|i| {
            if i % 100 == 0 {
                format!("Page {i} has the needle")
            } else {
                format!("Page {i}")
            }
        })
        .collect();
    let pages: Vec<Vec<&str>> = lines.iter().map(|l| vec![l.as_str()]).collect();
    let pages: Vec<&[&str]> = pages.iter().map(Vec::as_slice).collect();
    let doc = PdfDoc::open(bytes(document(&pages)), "").unwrap();
    assert_eq!(doc.page_count(), 520);
    let hits: Vec<usize> = (0..doc.page_count())
        .flat_map(|p| find_in(p, &doc.text(p), "needle", false, true))
        .map(|m| m.page)
        .collect();
    assert_eq!(hits, vec![99, 199, 299, 399, 499]);
    assert_eq!(doc.text(519).text, "Page 520");
}
