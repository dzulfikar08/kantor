//! A sample report built with docs_engine and saved as `.docx`, shared by
//! the export tests and `examples/sample.rs`.

#![allow(dead_code)]

use std::io::Write as _;
use std::path::Path;

use docs_engine::{Document, ListKind, Position, RunProps, Selection};

/// A valid RGB PNG, `w` × `h`, one colour (stored deflate via flate2).
pub fn png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut c = flate2::Crc::new();
        c.update(kind);
        c.update(data);
        out.extend(kind);
        out.extend(data);
        out.extend(c.sum().to_be_bytes());
    }
    // Each row: filter byte 0, then the pixels.
    let row: Vec<u8> = std::iter::once(0)
        .chain(rgb.into_iter().cycle().take(3 * w as usize))
        .collect();
    let raw = row.repeat(h as usize);
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
    z.write_all(&raw).unwrap();
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(w.to_be_bytes());
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z.finish().unwrap());
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A document like a short report: heading, coloured and bold runs, a
/// numbered list, a 3 × 2 table and a picture; saved as .docx and opened.
pub fn report(dir: &Path, picture: bool, extra_paragraphs: usize) -> Document {
    let mut doc = Document::new();
    let mut c = doc.insert_text(Selection::default(), "Laporan Pemeliharaan\n", None);
    doc.set_style(
        Selection::caret(Position::new(0, 0)),
        Some(docs_engine::HEADING_STYLES[0]),
    );
    c = doc.insert_text(c, "Ketersediaan mesin ", None);
    c = doc.insert_text(
        c,
        "naik",
        Some(RunProps {
            bold: Some(true),
            color: Some(docs_engine::Rgb([0xC0, 0x00, 0x00])),
            ..RunProps::default()
        }),
    );
    // Typing continues in the caret's formatting: switch back to plain.
    let plain = || {
        Some(RunProps {
            bold: Some(false),
            color: Some(docs_engine::Rgb([0, 0, 0])),
            ..RunProps::default()
        })
    };
    c = doc.insert_text(c, " pada kuartal ketiga.\nPompa\nKompresor", plain());
    doc.toggle_list(
        Selection {
            anchor: Position::new(2, 0),
            head: c.head,
        },
        ListKind::Numbered,
    );
    c = doc.insert_text(c, "\n", None);
    doc.toggle_list(c, ListKind::Numbered);
    if picture {
        c = doc
            .insert_image(c, png(120, 60, [0x20, 0x80, 0xE0]), "grafik.png")
            .unwrap();
    }
    for k in 0..extra_paragraphs {
        c = doc.insert_text(
            c,
            &format!(
                "\nParagraf {k}: perawatan berkala mengurangi waktu henti dan \
                 menjaga mesin tetap andal sepanjang tahun produksi."
            ),
            plain(),
        );
    }
    doc.insert_table(Selection::caret(doc.end()), 3, 2);
    let path = dir.join("report.docx");
    doc.save_as(&path).unwrap();
    Document::open(&path).unwrap()
}
