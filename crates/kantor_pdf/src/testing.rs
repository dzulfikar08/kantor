//! Small PDFs made on the fly for tests (here and in `kantor`'s CLI tests).
//! Page `k` is `600 + k` points wide and shows "Page k", so tests can tell
//! pages apart after reordering.

use lopdf::content::{Content, Operation};
use lopdf::{
    Dictionary, Document, EncryptionState, EncryptionVersion, Object, Stream, StringFormat,
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

pub use lopdf::Permissions;

/// The standard security handlers, for tests of password-protected files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protect {
    /// RC4 40-bit (V1, R2).
    Rc4_40,
    /// RC4 128-bit (V2, R3).
    Rc4_128,
    /// AES-128 (V4, R4, AESV2 crypt filter).
    Aes128,
    /// AES-256 (V5, R6, AESV3 crypt filter).
    Aes256,
}

impl Protect {
    pub const ALL: [Protect; 4] = [
        Protect::Rc4_40,
        Protect::Rc4_128,
        Protect::Aes128,
        Protect::Aes256,
    ];
}

/// A sample PDF protected with `kind`, the passwords and `permissions`
/// (for people with the user password).
pub fn sample_pdf_protected(
    pages: u32,
    kind: Protect,
    owner: &str,
    user: &str,
    permissions: Permissions,
) -> Vec<u8> {
    use lopdf::encryption::crypt_filters::{Aes128CryptFilter, Aes256CryptFilter, CryptFilter};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    let mut doc = build(pages, false);
    doc.trailer.set(
        "ID",
        vec![
            Object::String(b"kantor-test-id-2".to_vec(), StringFormat::Hexadecimal),
            Object::String(b"kantor-test-id-2".to_vec(), StringFormat::Hexadecimal),
        ],
    );
    let filters = |f: Arc<dyn CryptFilter>| BTreeMap::from([(b"StdCF".to_vec(), f)]);
    let key = [0x5Au8; 32];
    let version = match kind {
        Protect::Rc4_40 => EncryptionVersion::V1 {
            document: &doc,
            owner_password: owner,
            user_password: user,
            permissions,
        },
        Protect::Rc4_128 => EncryptionVersion::V2 {
            document: &doc,
            owner_password: owner,
            user_password: user,
            key_length: 128,
            permissions,
        },
        Protect::Aes128 => EncryptionVersion::V4 {
            document: &doc,
            encrypt_metadata: true,
            crypt_filters: filters(Arc::new(Aes128CryptFilter)),
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: owner,
            user_password: user,
            permissions,
        },
        Protect::Aes256 => EncryptionVersion::V5 {
            encrypt_metadata: true,
            crypt_filters: filters(Arc::new(Aes256CryptFilter)),
            file_encryption_key: &key,
            stream_filter: b"StdCF".to_vec(),
            string_filter: b"StdCF".to_vec(),
            owner_password: owner,
            user_password: user,
            permissions,
        },
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

/// A one-page sample PDF (612 × 800) with an AcroForm: text field `name`
/// (value "Ann"), multi-line `notes`, check box `agree` (on state `Yes`),
/// radio group `size` (`S`, `M`, `L`; `M` chosen), drop-down `city`
/// (Jakarta, Bandung) and list box `fruit` (apple, pear). No appearances:
/// filling must write them.
pub fn sample_form_pdf() -> Vec<u8> {
    let mut doc = build(1, false);
    let page = doc.get_pages()[&1];
    let mut annots = Vec::new();
    let mut fields = Vec::new();
    let s = |t: &str| Object::string_literal(t);
    let rect = |r: [i64; 4]| Object::Array(r.iter().map(|&v| v.into()).collect());
    let mut widget = |doc: &mut Document, mut d: Dictionary, r: [i64; 4]| {
        d.set("Type", "Annot");
        d.set("Subtype", "Widget");
        d.set("Rect", rect(r));
        d.set("P", page);
        d.set("F", 4);
        let mut mk = Dictionary::new();
        mk.set("BC", vec![0.into(), 0.into(), 0.into()]);
        d.set("MK", mk);
        let id = doc.add_object(d);
        annots.push(Object::Reference(id));
        id
    };
    let mut d = Dictionary::new();
    d.set("FT", "Tx");
    d.set("T", s("name"));
    d.set("V", s("Ann"));
    d.set("DA", s("/Helv 12 Tf 0 g"));
    fields.push(Object::Reference(widget(&mut doc, d, [72, 600, 300, 624])));
    let mut d = Dictionary::new();
    d.set("FT", "Tx");
    d.set("T", s("notes"));
    d.set("Ff", 1 << 12);
    d.set("DA", s("/Helv 10 Tf 0 g"));
    fields.push(Object::Reference(widget(&mut doc, d, [72, 500, 300, 580])));
    let mut d = Dictionary::new();
    d.set("FT", "Btn");
    d.set("T", s("agree"));
    d.set("V", Object::Name(b"Off".to_vec()));
    d.set("AS", Object::Name(b"Off".to_vec()));
    d.set("DA", s("/ZaDb 0 Tf 0 g"));
    fields.push(Object::Reference(widget(&mut doc, d, [72, 460, 90, 478])));
    // Radio group: a parent with three widget kids.
    let parent = doc.new_object_id();
    let mut kids = Vec::new();
    for (i, st) in ["S", "M", "L"].iter().enumerate() {
        let mut k = Dictionary::new();
        k.set("Parent", parent);
        k.set(
            "AS",
            Object::Name(if *st == "M" {
                b"M".to_vec()
            } else {
                b"Off".to_vec()
            }),
        );
        let form = |content: &[u8]| {
            let mut d = Dictionary::new();
            d.set("Type", "XObject");
            d.set("Subtype", "Form");
            d.set("BBox", vec![0.into(), 0.into(), 18.into(), 18.into()]);
            Stream::new(d, content.to_vec())
        };
        let mut n = Dictionary::new();
        n.set(
            *st,
            doc.add_object(form(b"0 g 5 5 m 13 5 l 13 13 l 5 13 l h f")),
        );
        n.set("Off", doc.add_object(form(b"")));
        let mut ap = Dictionary::new();
        ap.set("N", n);
        k.set("AP", ap);
        let x = 72 + 30 * i as i64;
        kids.push(Object::Reference(widget(
            &mut doc,
            k,
            [x, 420, x + 18, 438],
        )));
    }
    let mut d = Dictionary::new();
    d.set("FT", "Btn");
    d.set("Ff", 1 << 15);
    d.set("T", s("size"));
    d.set("V", Object::Name(b"M".to_vec()));
    d.set("Kids", kids);
    doc.objects.insert(parent, Object::Dictionary(d));
    fields.push(Object::Reference(parent));
    let mut d = Dictionary::new();
    d.set("FT", "Ch");
    d.set("T", s("city"));
    d.set("Ff", 1 << 17);
    d.set("Opt", vec![s("Jakarta"), s("Bandung")]);
    d.set("DA", s("/Helv 11 Tf 0 g"));
    fields.push(Object::Reference(widget(&mut doc, d, [72, 380, 250, 400])));
    let mut d = Dictionary::new();
    d.set("FT", "Ch");
    d.set("T", s("fruit"));
    d.set("Opt", vec![s("apple"), s("pear")]);
    d.set("DA", s("/Helv 11 Tf 0 g"));
    fields.push(Object::Reference(widget(&mut doc, d, [72, 300, 250, 360])));
    doc.get_dictionary_mut(page)
        .expect("page")
        .set("Annots", annots);
    let mut helv = Dictionary::new();
    helv.set("Type", "Font");
    helv.set("Subtype", "Type1");
    helv.set("BaseFont", "Helvetica");
    let mut fonts = Dictionary::new();
    fonts.set("Helv", doc.add_object(helv));
    let mut dr = Dictionary::new();
    dr.set("Font", fonts);
    let mut form = Dictionary::new();
    form.set("Fields", fields);
    form.set("DR", dr);
    form.set("DA", s("/Helv 0 Tf 0 g"));
    let form = doc.add_object(form);
    doc.catalog_mut().expect("catalog").set("AcroForm", form);
    save(doc)
}

/// A round red stamp, `size` px square: opaque ring and text-like bar,
/// half-transparent inside, fully transparent outside the circle.
pub fn sample_round_stamp(size: u32) -> Vec<u8> {
    let c = size as f32 / 2.0;
    let img = image::RgbaImage::from_fn(size, size, |x, y| {
        let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
        if d > c {
            image::Rgba([0, 0, 0, 0])
        } else if d > c * 0.8 || (y as f32 - c).abs() < c * 0.1 {
            image::Rgba([200, 20, 30, 255])
        } else {
            image::Rgba([200, 20, 30, 70])
        }
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("png");
    out.into_inner()
}

/// [`sample_form_pdf`] with Adobe Reader usage rights, as the IRS forms
/// have: a `/UR3` signature in the catalog's `/Perms` (no signature field).
pub fn sample_usage_rights_pdf() -> Vec<u8> {
    use lopdf::dictionary;
    let mut doc = Document::load_mem(&sample_form_pdf()).expect("sample form");
    let sig = doc.add_object(dictionary! {
        "Type" => "Sig",
        "Filter" => "Adobe.PPKLite",
        "SubFilter" => "adbe.pkcs7.detached",
        "ByteRange" => vec![0.into(), 0.into(), 0.into(), 0.into()],
        "Contents" => Object::String(vec![0; 8], StringFormat::Hexadecimal),
        "Reference" => vec![Object::Dictionary(dictionary! {
            "Type" => "SigRef",
            "TransformMethod" => "UR3",
            "TransformParams" => dictionary! { "Type" => "TransformParams", "V" => "2.2" },
        })],
    });
    doc.catalog_mut()
        .expect("catalog")
        .set("Perms", dictionary! { "UR3" => sig });
    let mut out = Vec::new();
    doc.save_to(&mut out).expect("save");
    out
}
