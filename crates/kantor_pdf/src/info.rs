//! Document facts: pages, page sizes, metadata, encryption.

use lopdf::{Document, LoadOptions};

use crate::pages::{page_ids, rotation, visible_box};
use crate::{Error, Result};

/// One page's size as displayed (rotation applied), in points.
#[derive(Debug, Clone, PartialEq)]
pub struct PageInfo {
    pub width: f32,
    pub height: f32,
    /// /Rotate: 0, 90, 180 or 270.
    pub rotation: i64,
}

impl PageInfo {
    /// A paper name when the size matches one (within 2 pt), like `A4` or
    /// `Letter landscape`.
    pub fn paper_name(&self) -> Option<String> {
        const PAPERS: [(&str, f32, f32); 7] = [
            ("A3", 841.89, 1190.55),
            ("A4", 595.28, 841.89),
            ("A5", 419.53, 595.28),
            ("B5", 498.9, 708.66),
            ("Letter", 612.0, 792.0),
            ("Legal", 612.0, 1008.0),
            ("Tabloid", 792.0, 1224.0),
        ];
        let close = |a: f32, b: f32| (a - b).abs() <= 2.0;
        PAPERS.iter().find_map(|&(name, w, h)| {
            if close(self.width, w) && close(self.height, h) {
                Some(name.to_string())
            } else if close(self.width, h) && close(self.height, w) {
                Some(format!("{name} landscape"))
            } else {
                None
            }
        })
    }
}

/// What `kantor pdf info` shows.
#[derive(Debug, Clone, Default)]
pub struct Info {
    pub version: String,
    pub encrypted: bool,
    /// False when the PDF is encrypted and could not be opened, so the page
    /// and metadata fields are empty.
    pub readable: bool,
    pub pages: Vec<PageInfo>,
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub creator: Option<String>,
    pub producer: Option<String>,
    pub file_size: usize,
}

fn info_string(doc: &Document, key: &[u8]) -> Option<String> {
    let info = doc.trailer.get(b"Info").ok()?;
    let dict = doc.dereference(info).ok()?.1.as_dict().ok()?;
    let value = doc.dereference(dict.get(key).ok()?).ok()?.1;
    let s = lopdf::decode_text_string(value).ok()?;
    let s = s.trim_matches(char::from(0)).trim().to_string();
    (!s.is_empty()).then_some(s)
}

/// Read the facts of a PDF. Unlike [`crate::Pdf::from_bytes`], an encrypted
/// PDF without a password is not an error: it's reported as encrypted, with
/// whatever lopdf could read.
pub fn info(bytes: &[u8], password: Option<&str>) -> Result<Info> {
    let options = match password {
        Some(p) => LoadOptions::with_password(p),
        None => LoadOptions::default(),
    };
    let doc = match Document::load_mem_with_options(bytes, options) {
        Ok(d) => d,
        Err(lopdf::Error::InvalidPassword) => return Err(Error::WrongPassword),
        Err(e) => {
            if password.is_none() && crate::sniff_encrypt(bytes) {
                return Ok(Info {
                    encrypted: true,
                    file_size: bytes.len(),
                    ..Info::default()
                });
            }
            return Err(Error::NotPdf(e.to_string()));
        }
    };
    let encrypted = crate::encrypted(&doc);
    // lopdf keeps /Encrypt in the trailer only when it could not decrypt.
    let readable = !doc.trailer.has(b"Encrypt");
    let mut info = Info {
        version: doc.version.clone(),
        encrypted,
        readable,
        file_size: bytes.len(),
        ..Info::default()
    };
    if !readable {
        return Ok(info);
    }
    info.pages = page_ids(&doc)
        .into_iter()
        .map(|id| {
            let [x0, y0, x1, y1] = visible_box(&doc, id);
            let r = rotation(&doc, id);
            let (w, h) = (x1 - x0, y1 - y0);
            let (width, height) = if r % 180 == 90 { (h, w) } else { (w, h) };
            PageInfo {
                width,
                height,
                rotation: r,
            }
        })
        .collect();
    info.title = info_string(&doc, b"Title");
    info.author = info_string(&doc, b"Author");
    info.subject = info_string(&doc, b"Subject");
    info.creator = info_string(&doc, b"Creator");
    info.producer = info_string(&doc, b"Producer");
    Ok(info)
}
