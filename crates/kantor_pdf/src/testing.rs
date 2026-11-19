//! Small PDFs made on the fly for tests (here and in `kantor`'s CLI tests).
//! Page `k` is `600 + k` points wide and shows "Page k", so tests can tell
//! pages apart after reordering.

use lopdf::content::{Content, Operation};
use lopdf::{
    Dictionary, Document, EncryptionState, EncryptionVersion, Object, Permissions, Stream,
    StringFormat,
};

/// Width of sample page `k` (1-based).
pub fn sample_width(k: u32) -> f32 {
    600.0 + k as f32
}

fn build(pages: u32, inherit: bool) -> Document {
    let mut doc = Document::with_version("1.4");
    let pages_id = doc.new_object_id();
    let mut font = Dictionary::new();
    font.set("Type", "Font");
    font.set("Subtype", "Type1");
    font.set("BaseFont", "Helvetica");
    let font_id = doc.add_object(font);
    let mut fonts = Dictionary::new();
    fonts.set("F1", font_id);
    let mut resources = Dictionary::new();
    resources.set("Font", fonts);
    let resources_id = doc.add_object(resources);
    let mut kids = Vec::new();
    for k in 1..=pages {
        let ops = vec![
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec!["F1".into(), 24.into()]),
            Operation::new("Td", vec![72.into(), 700.into()]),
            Operation::new(
                "Tj",
                vec![Object::String(
                    format!("Page {k}").into_bytes(),
                    StringFormat::Literal,
                )],
            ),
            Operation::new("ET", vec![]),
        ];
        let content = Content { operations: ops }.encode().expect("encode");
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content));
        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Parent", pages_id);
        page.set("Contents", content_id);
        if !inherit {
            page.set("Resources", resources_id);
        }
        page.set(
            "MediaBox",
            vec![
                0.into(),
                0.into(),
                Object::Real(sample_width(k)),
                800.into(),
            ],
        );
        kids.push(Object::Reference(doc.add_object(page)));
    }
    let mut tree = Dictionary::new();
    tree.set("Type", "Pages");
    tree.set("Kids", kids);
    tree.set("Count", pages as i64);
    if inherit {
        // Resources and rotation inherited from the page tree.
        tree.set("Resources", resources_id);
        tree.set("Rotate", 90);
    }
    doc.objects.insert(pages_id, Object::Dictionary(tree));
    let mut catalog = Dictionary::new();
    catalog.set("Type", "Catalog");
    catalog.set("Pages", pages_id);
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", catalog_id);
    let mut info = Dictionary::new();
    info.set("Title", Object::string_literal("Sample"));
    info.set("Author", Object::string_literal("Kantor tests"));
    let info_id = doc.add_object(info);
    doc.trailer.set("Info", info_id);
    doc
}

fn save(mut doc: Document) -> Vec<u8> {
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("save");
    out
}

/// A PDF with `pages` pages (see the module docs).
pub fn sample_pdf(pages: u32) -> Vec<u8> {
    save(build(pages, false))
}

/// Like [`sample_pdf`], but Resources and `/Rotate 90` live on the page tree
/// root, so every page inherits them.
pub fn sample_pdf_inherited(pages: u32) -> Vec<u8> {
    save(build(pages, true))
}

/// A sample PDF encrypted with RC4 128-bit; `user` may be empty.
pub fn sample_pdf_encrypted(pages: u32, owner: &str, user: &str) -> Vec<u8> {
    let mut doc = build(pages, false);
    doc.trailer.set(
        "ID",
        vec![
            Object::String(b"kantor-test-id-1".to_vec(), StringFormat::Hexadecimal),
            Object::String(b"kantor-test-id-1".to_vec(), StringFormat::Hexadecimal),
        ],
    );
    let version = EncryptionVersion::V2 {
        document: &doc,
        owner_password: owner,
        user_password: user,
        key_length: 128,
        permissions: Permissions::all(),
    };
    let state = EncryptionState::try_from(version).expect("encryption state");
    doc.encrypt(&state).expect("encrypt");
    save(doc)
}

/// A small PNG (RGBA when `alpha`).
pub fn sample_png(width: u32, height: u32, alpha: bool) -> Vec<u8> {
    let img = if alpha {
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(width, height, |x, _| {
            image::Rgba([200, 30, 30, if x % 2 == 0 { 255 } else { 128 }])
        }))
    } else {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            width,
            height,
            image::Rgb([30, 120, 200]),
        ))
    };
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .expect("png");
    out.into_inner()
}

/// A small JPEG.
pub fn sample_jpeg(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbImage::from_pixel(width, height, image::Rgb([30, 160, 60]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .expect("jpeg");
    out.into_inner()
}

/// A sample PDF with a bookmark per page ("Page k", the last one with a
/// child bookmark to page 1), a link on page 1 to the last page, a named
/// destination "last" for it and a link on page 2 that goes there.
pub fn sample_pdf_linked(pages: u32) -> Vec<u8> {
    let mut doc = build(pages, false);
    let ids: Vec<lopdf::ObjectId> = doc.get_pages().into_values().collect();
    let last = *ids.last().expect("pages");
    let dest = |page| {
        Object::Array(vec![
            Object::Reference(page),
            "XYZ".into(),
            Object::Null,
            Object::Null,
            Object::Null,
        ])
    };
    let outlines = doc.new_object_id();
    let mut items = Vec::new();
    for (k, &page) in ids.iter().enumerate() {
        let mut item = Dictionary::new();
        item.set(
            "Title",
            Object::string_literal(format!("Page {}", k + 1).as_str()),
        );
        item.set("Parent", outlines);
        item.set("Dest", dest(page));
        items.push(doc.add_object(item));
    }
    // A child under the last bookmark.
    let mut child = Dictionary::new();
    child.set("Title", Object::string_literal("Back to start"));
    child.set("Parent", *items.last().expect("items"));
    child.set("Dest", dest(ids[0]));
    let child = doc.add_object(child);
    for (i, &id) in items.iter().enumerate() {
        let d = doc.get_dictionary_mut(id).expect("item");
        if i > 0 {
            d.set("Prev", items[i - 1]);
        }
        if let Some(&n) = items.get(i + 1) {
            d.set("Next", n);
        }
        if i + 1 == items.len() {
            d.set("First", child);
            d.set("Last", child);
            d.set("Count", 1);
        }
    }
    let mut root = Dictionary::new();
    root.set("Type", "Outlines");
    root.set("First", items[0]);
    root.set("Last", *items.last().expect("items"));
    root.set("Count", items.len() as i64 + 1);
    doc.objects.insert(outlines, Object::Dictionary(root));
    // Named destination "last" and the links.
    let mut dests = Dictionary::new();
    dests.set("last", dest(last));
    let link = |doc: &mut Document, target: Object, named: bool| {
        let mut a = Dictionary::new();
        a.set("Type", "Annot");
        a.set("Subtype", "Link");
        a.set("Rect", vec![72.into(), 600.into(), 200.into(), 620.into()]);
        if named {
            let mut act = Dictionary::new();
            act.set("S", "GoTo");
            act.set("D", target);
            a.set("A", act);
        } else {
            a.set("Dest", target);
        }
        doc.add_object(a)
    };
    let l1 = link(&mut doc, dest(last), false);
    let l2 = link(&mut doc, Object::Name(b"last".to_vec()), true);
    doc.get_dictionary_mut(ids[0])
        .expect("page")
        .set("Annots", vec![Object::Reference(l1)]);
    if ids.len() > 1 {
        doc.get_dictionary_mut(ids[1])
            .expect("page")
            .set("Annots", vec![Object::Reference(l2)]);
    }
    let cat = doc.catalog_mut().expect("catalog");
    cat.set("Outlines", outlines);
    cat.set("Dests", dests);
    cat.set("OpenAction", dest(last));
    save(doc)
}
