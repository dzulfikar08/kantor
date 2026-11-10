//! Images to PDF: one page per JPEG or PNG.
//!
//! JPEGs are embedded as they are (no re-encoding, no quality loss). PNGs are
//! decoded and stored losslessly with Flate, with transparency as a soft
//! mask. EXIF orientation is honoured, so phone photos come out upright.

use std::io::Cursor;
use std::str::FromStr;

use image::{DynamicImage, ImageDecoder, ImageFormat};
use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, Stream};

use crate::{Error, Pdf, Result};

/// The page size for images to PDF.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageSize {
    /// The page takes the image's size (1 pixel = 1 point), plus margins.
    Fit,
    A4,
    Letter,
}

impl PageSize {
    fn points(self) -> Option<(f32, f32)> {
        match self {
            PageSize::Fit => None,
            PageSize::A4 => Some((595.276, 841.89)),
            PageSize::Letter => Some((612.0, 792.0)),
        }
    }
}

impl FromStr for PageSize {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "fit" | "image" => Ok(PageSize::Fit),
            "a4" => Ok(PageSize::A4),
            "letter" => Ok(PageSize::Letter),
            _ => Err(Error::Invalid(format!(
                "unknown page size \"{s}\" (use fit, a4 or letter)"
            ))),
        }
    }
}

/// Page orientation for A4/Letter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    /// Landscape for wide images, portrait otherwise.
    Auto,
    Portrait,
    Landscape,
}

impl FromStr for Orientation {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Orientation::Auto),
            "portrait" => Ok(Orientation::Portrait),
            "landscape" => Ok(Orientation::Landscape),
            _ => Err(Error::Invalid(format!(
                "unknown orientation \"{s}\" (use auto, portrait or landscape)"
            ))),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ImageOptions {
    pub page_size: PageSize,
    pub orientation: Orientation,
    /// Space around the image, in points.
    pub margin: f32,
}

impl Default for ImageOptions {
    fn default() -> Self {
        ImageOptions {
            page_size: PageSize::Fit,
            orientation: Orientation::Auto,
            margin: 0.0,
        }
    }
}

/// An image ready to place: its XObject and pixel size as stored.
struct Embedded {
    xobject: Stream,
    smask: Option<Stream>,
    width: u32,
    height: u32,
    /// EXIF orientation, 1 to 8.
    exif: u8,
}

/// Width, height and component count from a JPEG's SOF marker, and whether
/// an Adobe APP14 marker is present (Adobe CMYK JPEGs store inverted values).
fn jpeg_header(data: &[u8]) -> Option<(u32, u32, u8, bool)> {
    let mut i = 2;
    let mut adobe = false;
    while i + 4 <= data.len() {
        if data[i] != 0xFF {
            return None;
        }
        let marker = data[i + 1];
        if marker == 0xFF {
            i += 1;
            continue;
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        let body = data.get(i + 4..i + 2 + len)?;
        if marker == 0xEE && body.starts_with(b"Adobe") {
            adobe = true;
        }
        // SOF0..SOF15 except DHT (C4), JPG (C8) and DAC (CC).
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            let h = u16::from_be_bytes([*body.get(1)?, *body.get(2)?]) as u32;
            let w = u16::from_be_bytes([*body.get(3)?, *body.get(4)?]) as u32;
            let n = *body.get(5)?;
            return Some((w, h, n, adobe));
        }
        i += 2 + len;
    }
    None
}

fn embed(data: &[u8], name: &str) -> Result<Embedded> {
    let bad = |e: String| Error::Invalid(format!("{name}: {e}"));
    let format = image::guess_format(data).map_err(|_| bad("not a JPEG or PNG image".into()))?;
    match format {
        ImageFormat::Jpeg => {
            let (width, height, comps, adobe) =
                jpeg_header(data).ok_or_else(|| bad("unreadable JPEG header".into()))?;
            let exif = image::codecs::jpeg::JpegDecoder::new(Cursor::new(data))
                .ok()
                .and_then(|mut d| d.orientation().ok())
                .map_or(1, |o| o.to_exif());
            let space = match comps {
                1 => "DeviceGray",
                3 => "DeviceRGB",
                4 => "DeviceCMYK",
                n => {
                    return Err(bad(format!(
                        "JPEG with {n} colour components is not supported"
                    )));
                }
            };
            let mut dict = Dictionary::new();
            dict.set("Type", "XObject");
            dict.set("Subtype", "Image");
            dict.set("Width", width as i64);
            dict.set("Height", height as i64);
            dict.set("ColorSpace", space);
            dict.set("BitsPerComponent", 8);
            dict.set("Filter", "DCTDecode");
            if comps == 4 && adobe {
                dict.set(
                    "Decode",
                    vec![
                        1.into(),
                        0.into(),
                        1.into(),
                        0.into(),
                        1.into(),
                        0.into(),
                        1.into(),
                        0.into(),
                    ],
                );
            }
            let mut xobject = Stream::new(dict, data.to_vec());
            xobject.allows_compression = false;
            Ok(Embedded {
                xobject,
                smask: None,
                width,
                height,
                exif,
            })
        }
        ImageFormat::Png => {
            let mut decoder = image::codecs::png::PngDecoder::new(Cursor::new(data))
                .map_err(|e| bad(e.to_string()))?;
            let exif = decoder.orientation().map_or(1, |o| o.to_exif());
            let img = DynamicImage::from_decoder(decoder).map_err(|e| bad(e.to_string()))?;
            let (width, height) = (img.width(), img.height());
            let has_alpha = img.color().has_alpha();
            let gray = !img.color().has_color();
            let (pixels, space, alpha) = match (gray, has_alpha) {
                (true, false) => (img.into_luma8().into_raw(), "DeviceGray", None),
                (false, false) => (img.into_rgb8().into_raw(), "DeviceRGB", None),
                (true, true) => {
                    let la = img.into_luma_alpha8().into_raw();
                    let (l, a) = la.chunks(2).map(|p| (p[0], p[1])).unzip();
                    (l, "DeviceGray", Some(a))
                }
                (false, true) => {
                    let rgba = img.into_rgba8().into_raw();
                    let rgb = rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
                    let a = rgba.chunks(4).map(|p| p[3]).collect();
                    (rgb, "DeviceRGB", Some(a))
                }
            };
            let image_dict = |space: &str| {
                let mut d = Dictionary::new();
                d.set("Type", "XObject");
                d.set("Subtype", "Image");
                d.set("Width", width as i64);
                d.set("Height", height as i64);
                d.set("ColorSpace", space);
                d.set("BitsPerComponent", 8);
                d
            };
            let flate = |dict: Dictionary, data: Vec<u8>| -> Result<Stream> {
                let mut s = Stream::new(dict, data);
                s.compress()?;
                Ok(s)
            };
            // Drop the mask when every pixel is opaque.
            let alpha = alpha.filter(|a: &Vec<u8>| a.iter().any(|&v| v != 255));
            let smask = alpha
                .map(|a| flate(image_dict("DeviceGray"), a))
                .transpose()?;
            Ok(Embedded {
                xobject: flate(image_dict(space), pixels)?,
                smask,
                width,
                height,
                exif,
            })
        }
        other => Err(bad(format!(
            "{} images are not supported (use JPEG or PNG)",
            other.extensions_str().first().unwrap_or(&"this kind of")
        ))),
    }
}

/// The matrix that draws the unit-square image upright into a `w` x `h` box
/// at (x, y), for an EXIF orientation.
fn placement(exif: u8, x: f32, y: f32, w: f32, h: f32) -> [f32; 6] {
    let [a, b, c, d, e, f] = match exif {
        2 => [-w, 0.0, 0.0, h, w, 0.0],
        3 => [-w, 0.0, 0.0, -h, w, h],
        4 => [w, 0.0, 0.0, -h, 0.0, h],
        5 => [0.0, -h, -w, 0.0, w, h],
        6 => [0.0, -h, w, 0.0, 0.0, h],
        7 => [0.0, h, w, 0.0, 0.0, 0.0],
        8 => [0.0, h, -w, 0.0, w, 0.0],
        _ => [w, 0.0, 0.0, h, 0.0, 0.0],
    };
    [a, b, c, d, e + x, f + y]
}

/// Make a PDF with one page per image. `images` pairs a display name (for
/// errors) with the file bytes.
pub fn images_to_pdf(images: &[(String, Vec<u8>)], opts: &ImageOptions) -> Result<Pdf> {
    if images.is_empty() {
        return Err(Error::Invalid("no images given".into()));
    }
    if !(opts.margin.is_finite() && opts.margin >= 0.0) {
        return Err(Error::Invalid("the margin can't be negative".into()));
    }
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for (name, data) in images {
        let img = embed(data, name)?;
        // Upright size in pixels.
        let (iw, ih) = if (5..=8).contains(&img.exif) {
            (img.height as f32, img.width as f32)
        } else {
            (img.width as f32, img.height as f32)
        };
        let m = opts.margin;
        let (pw, ph) = match opts.page_size.points() {
            None => (iw + 2.0 * m, ih + 2.0 * m),
            Some((short, long)) => {
                let landscape = match opts.orientation {
                    Orientation::Auto => iw > ih,
                    Orientation::Portrait => false,
                    Orientation::Landscape => true,
                };
                if landscape {
                    (long, short)
                } else {
                    (short, long)
                }
            }
        };
        let (aw, ah) = (pw - 2.0 * m, ph - 2.0 * m);
        if aw <= 1.0 || ah <= 1.0 {
            return Err(Error::Invalid(format!(
                "the margin ({m} pt) leaves no room on the page"
            )));
        }
        let scale = (aw / iw).min(ah / ih);
        let (dw, dh) = (iw * scale, ih * scale);
        let (x, y) = (m + (aw - dw) / 2.0, m + (ah - dh) / 2.0);

        let mut xobject = img.xobject;
        if let Some(mask) = img.smask {
            let mask_id = doc.add_object(mask);
            xobject.dict.set("SMask", mask_id);
        }
        let image_id = doc.add_object(xobject);
        let ops = vec![
            Operation::new("q", vec![]),
            Operation::new(
                "cm",
                placement(img.exif, x, y, dw, dh)
                    .iter()
                    .map(|&v| Object::Real(v))
                    .collect(),
            ),
            Operation::new("Do", vec![Object::Name(b"Im1".to_vec())]),
            Operation::new("Q", vec![]),
        ];
        let content = doc.add_object(Stream::new(
            Dictionary::new(),
            Content { operations: ops }.encode()?,
        ));
        let mut xobjects = Dictionary::new();
        xobjects.set("Im1", image_id);
        let mut resources = Dictionary::new();
        resources.set("XObject", xobjects);
        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Parent", pages_id);
        page.set(
            "MediaBox",
            vec![0.into(), 0.into(), Object::Real(pw), Object::Real(ph)],
        );
        page.set("Resources", resources);
        page.set("Contents", content);
        kids.push(Object::Reference(doc.add_object(page)));
    }
    let count = kids.len() as i64;
    let mut pages = Dictionary::new();
    pages.set("Type", "Pages");
    pages.set("Kids", kids);
    pages.set("Count", count);
    doc.objects.insert(pages_id, Object::Dictionary(pages));
    let mut catalog = Dictionary::new();
    catalog.set("Type", "Catalog");
    catalog.set("Pages", pages_id);
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", catalog_id);
    Ok(Pdf {
        doc,
        was_encrypted: false,
    })
}

#[cfg(test)]
pub(crate) fn exif_matrix(exif: u8) -> [f32; 6] {
    placement(exif, 0.0, 0.0, 1.0, 1.0)
}
