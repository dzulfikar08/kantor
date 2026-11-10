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
