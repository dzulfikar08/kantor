//! Inline objects: images and XML Kantor keeps without modelling it.
//!
//! An object is one [`OBJECT`] character in a paragraph's text; the run
//! covering that character carries the [`Object`]. Editing treats it like
//! any character (select, delete, undo), layout gives it a box, and saving
//! writes its XML back.

use std::sync::Arc;

/// The character standing for an object in paragraph text (U+FFFC OBJECT
/// REPLACEMENT CHARACTER).
pub const OBJECT: char = '\u{FFFC}';

/// EMUs (English Metric Units) per CSS pixel at 96 dpi.
pub const EMU_PER_PX: f32 = 9525.0;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Object {
    Image(Arc<Image>),
    Opaque(Arc<Opaque>),
}

impl Object {
    /// Zero-size objects that only mark a position (bookmarks).
    pub fn is_marker(&self) -> bool {
        matches!(self, Self::Opaque(o) if o.level == Level::Marker)
    }

    pub fn is_block(&self) -> bool {
        matches!(self, Self::Opaque(o) if o.level == Level::Block)
    }

    /// Short description for placeholders.
    pub fn label(&self) -> &str {
        match self {
            Self::Image(i) if i.descr.is_empty() => "Image",
            Self::Image(i) => &i.descr,
            Self::Opaque(o) => &o.label,
        }
    }
}

/// An inline picture (`w:drawing/wp:inline` with an embedded blip).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Image {
    /// Relationship id of the picture part (`r:embed`).
    pub rel_id: String,
    /// Size in EMUs (`wp:extent`).
    pub cx: i64,
    pub cy: i64,
    /// Alt text (`wp:docPr/@descr`).
    pub descr: String,
    /// The original `<w:drawing>…</w:drawing>`, written back verbatim;
    /// `None` for an image inserted in Kantor (its XML is generated).
    pub xml: Option<Arc<str>>,
    /// `wp:docPr/@id` for generated XML.
    pub doc_pr: u32,
    /// Part name of the picture, for the placeholder label and format.
    pub part: String,
}

impl Image {
    /// Size in CSS px at 96 dpi.
    pub fn size_px(&self) -> (f32, f32) {
        (
            self.cx.max(0) as f32 / EMU_PER_PX,
            self.cy.max(0) as f32 / EMU_PER_PX,
        )
    }

    pub fn format(&self) -> ImageFormat {
        ImageFormat::from_name(&self.part)
    }
}

/// Picture formats Kantor draws; everything else gets a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Other,
}

impl ImageFormat {
    pub fn from_name(name: &str) -> Self {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "png" => Self::Png,
            "jpg" | "jpeg" | "jpe" => Self::Jpeg,
            _ => Self::Other,
        }
    }

    pub fn sniff(bytes: &[u8]) -> Self {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Self::Png
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Self::Jpeg
        } else {
            Self::Other
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Other => "bin",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Other => "application/octet-stream",
        }
    }
}

/// Pixel size of a PNG or JPEG from its header.
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    match ImageFormat::sniff(bytes) {
        ImageFormat::Png => {
            let b = bytes.get(16..24)?;
            let w = u32::from_be_bytes(b[0..4].try_into().ok()?);
            let h = u32::from_be_bytes(b[4..8].try_into().ok()?);
            (w > 0 && h > 0).then_some((w, h))
        }
        ImageFormat::Jpeg => {
            let mut i = 2;
            while i + 4 <= bytes.len() {
                if bytes[i] != 0xFF {
                    i += 1;
                    continue;
                }
                let marker = bytes[i + 1];
                if marker == 0xFF {
                    i += 1;
                    continue;
                }
                if matches!(marker, 0xD8 | 0x01 | 0xD0..=0xD7) {
                    i += 2;
                    continue;
                }
                let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
                let sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
                if sof {
                    let b = bytes.get(i + 5..i + 9)?;
                    let h = u16::from_be_bytes([b[0], b[1]]) as u32;
                    let w = u16::from_be_bytes([b[2], b[3]]) as u32;
                    return (w > 0 && h > 0).then_some((w, h));
                }
                i += 2 + len;
            }
            None
        }
        ImageFormat::Other => None,
    }
}

/// Where an opaque element sits, which decides how it's written back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    /// Zero-size paragraph-level marker (`w:bookmarkStart`, `w:permEnd`…).
    Marker,
    /// Run content (`w:drawing` anchors, `w:sym`, footnote references…):
    /// written inside a `<w:r>` with the run's formatting.
    Run,
    /// Paragraph content (`w:sdt`, fields, `w:oMath`…): written between runs.
    Para,
    /// Body or cell content (`w:sdt`, nested tables…): written between
    /// paragraphs.
    Block,
}

/// An element Kantor keeps byte for byte without modelling it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Opaque {
    pub level: Level,
    /// The original XML of the element (or of a whole field's runs).
    pub xml: String,
    /// Placeholder text, e.g. `Floating image` or `Field: PAGE`.
    pub label: String,
}
