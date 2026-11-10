//! Text stamped on top of pages: watermarks and page numbers.
//!
//! The text is set in Helvetica, one of the 14 standard PDF fonts, so nothing
//! is embedded; it covers Latin text (WinAnsi encoding). Positions are in the
//! page as it is displayed, so a page with /Rotate 90 gets its number at the
//! bottom of what the reader sees.

use std::str::FromStr;

use lopdf::content::{Content, Operation};
use lopdf::{Dictionary, Document, Object, ObjectId, Stream, StringFormat};

use crate::pages::{inherited, page_ids, rotation, visible_box};
use crate::range::PageRanges;
use crate::{Error, Pdf, Result};

/// Where text goes on the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Center,
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Position {
    pub const NAMES: &'static str =
        "center, top-left, top-center, top-right, bottom-left, bottom-center, bottom-right";
}

impl FromStr for Position {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let key: String = s
            .chars()
            .filter(|c| !matches!(c, '-' | '_' | ' '))
            .collect::<String>()
            .to_ascii_lowercase();
        Ok(match key.as_str() {
            "center" | "middle" => Position::Center,
            "topleft" => Position::TopLeft,
            "topcenter" | "top" => Position::TopCenter,
            "topright" => Position::TopRight,
            "bottomleft" => Position::BottomLeft,
            "bottomcenter" | "bottom" => Position::BottomCenter,
            "bottomright" => Position::BottomRight,
            _ => {
                return Err(Error::Invalid(format!(
                    "unknown position \"{s}\" (use {})",
                    Position::NAMES
                )));
            }
        })
    }
}

/// A diagonal text watermark.
#[derive(Debug, Clone)]
pub struct WatermarkOptions {
    pub text: String,
    /// Font size in points.
    pub font_size: f32,
    /// 0 (invisible) to 1 (opaque).
    pub opacity: f32,
    /// Degrees counter-clockwise; negative turns clockwise.
    pub rotation: f32,
    pub position: Position,
    /// RGB, each 0 to 1.
    pub color: [f32; 3],
    /// Distance from the page edge for the edge positions, in points.
    pub margin: f32,
}

impl Default for WatermarkOptions {
    fn default() -> Self {
        WatermarkOptions {
            text: String::new(),
            font_size: 48.0,
            opacity: 0.3,
            rotation: 45.0,
            position: Position::Center,
            color: [0.5, 0.5, 0.5],
            margin: 36.0,
        }
    }
}

/// Page numbers such as `3` or `3 of 10`.
#[derive(Debug, Clone)]
pub struct PageNumberOptions {
    /// `{n}` is the number, `{total}` the last number.
    pub format: String,
    pub position: Position,
    /// The number on the first numbered page.
    pub start: i64,
    pub font_size: f32,
    pub margin: f32,
    pub color: [f32; 3],
}

impl Default for PageNumberOptions {
    fn default() -> Self {
        PageNumberOptions {
            format: "{n}".into(),
            position: Position::BottomCenter,
            start: 1,
            font_size: 12.0,
            margin: 30.0,
            color: [0.0, 0.0, 0.0],
        }
    }
}

/// Helvetica advance widths (1/1000 em) for ASCII 32..=126, from its AFM.
const HELVETICA_ASCII: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // ' '../
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, // 0-9
    278, 278, 584, 584, 584, 556, 1015, // :;<=>?@
    667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667,
    611, 722, 667, 944, 667, 667, 611, // A-Z
    278, 278, 278, 469, 556, 333, // [\]^_`
    556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500,
    278, 556, 500, 722, 500, 500, 500, // a-z
    334, 260, 334, 584, // {|}~
];

/// Helvetica cap height in 1/1000 em.
const CAP_HEIGHT: f32 = 718.0;

/// A character in WinAnsiEncoding (the code) and its Helvetica width.
fn winansi(c: char) -> Option<(u8, u16)> {
    let ascii_width = |b: u8| HELVETICA_ASCII[(b - 32) as usize];
    let code = c as u32;
    if (32..=126).contains(&code) {
        return Some((code as u8, ascii_width(code as u8)));
    }
    let special = match c {
        '€' => (0x80, 556),
        '‚' => (0x82, 222),
        '„' => (0x84, 333),
        '…' => (0x85, 1000),
        '‘' => (0x91, 222),
        '’' => (0x92, 222),
        '“' => (0x93, 333),
        '”' => (0x94, 333),
        '•' => (0x95, 350),
        '–' => (0x96, 556),
        '—' => (0x97, 1000),
        '™' => (0x99, 1000),
        _ => (0, 0),
    };
    if special.0 != 0 {
        return Some(special);
    }
    if (0xA0..=0xFF).contains(&code) {
        // Latin-1 letters take the width of their base letter.
        let base = match c {
            'À'..='Å' => 'A',
            'Æ' => return Some((code as u8, 1000)),
            'Ç' => 'C',
            'È'..='Ë' => 'E',
            'Ì'..='Ï' => 'I',
            'Ð' => 'D',
            'Ñ' => 'N',
            'Ò'..='Ö' | 'Ø' => 'O',
            'Ù'..='Ü' => 'U',
            'Ý' => 'Y',
            'à'..='å' => 'a',
            'æ' => return Some((code as u8, 889)),
            'ç' => 'c',
            'è'..='ë' => 'e',
            'ì'..='ï' => 'i',
            'ñ' => 'n',
            'ò'..='ö' | 'ø' => 'o',
            'ù'..='ü' => 'u',
            'ý' | 'ÿ' => 'y',
            'ß' => return Some((code as u8, 611)),
            '\u{A0}' => ' ',
            _ => return Some((code as u8, 556)),
        };
        return Some((code as u8, ascii_width(base as u8)));
    }
    None
}

/// Encode `text` for the Helvetica font; returns the bytes and the width in
/// 1/1000 em.
fn encode(text: &str) -> Result<(Vec<u8>, f32)> {
    let mut bytes = Vec::with_capacity(text.len());
    let mut width = 0.0;
    for c in text.chars() {
        let (b, w) = winansi(c).ok_or_else(|| {
            Error::Invalid(format!(
                "the text can't contain '{c}': the built-in Helvetica font covers Latin \
                 characters only"
            ))
        })?;
        bytes.push(b);
        width += f32::from(w);
    }
    Ok((bytes, width))
}

/// Shared objects for one stamping run.
struct Stamper {
    font: ObjectId,
    gstate: Option<ObjectId>,
    /// `q` stream put before the page's own content, so its graphics state
    /// can't leak into the stamp.
    save: ObjectId,
}

impl Stamper {
    fn new(doc: &mut Document, opacity: f32) -> Stamper {
        let mut font = Dictionary::new();
        font.set("Type", "Font");
        font.set("Subtype", "Type1");
        font.set("BaseFont", "Helvetica");
        font.set("Encoding", "WinAnsiEncoding");
        let font = doc.add_object(font);
        let gstate = (opacity < 1.0).then(|| {
            let mut gs = Dictionary::new();
            gs.set("Type", "ExtGState");
            gs.set("ca", opacity);
            gs.set("CA", opacity);
            doc.add_object(gs)
        });
        let save = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
        Stamper { font, gstate, save }
    }

    /// Stamp `text` on `page` at `position`, turned `angle` degrees.
    #[allow(clippy::too_many_arguments)]
    fn stamp(
        &self,
        doc: &mut Document,
        page: ObjectId,
        text: &str,
        size: f32,
        angle: f32,
        position: Position,
        margin: f32,
        color: [f32; 3],
    ) -> Result<()> {
        let (bytes, width) = encode(text)?;
        let font_name = add_resource(doc, page, b"Font", "KtF", self.font)?;
        let gs_name = match self.gstate {
            Some(gs) => Some(add_resource(doc, page, b"ExtGState", "KtGS", gs)?),
            None => None,
        };

        // Display space: the page as shown, origin bottom-left.
        let [x0, y0, x1, y1] = visible_box(doc, page);
        let (w, h) = (x1 - x0, y1 - y0);
        let rot = rotation(doc, page);
        let (cm, disp_w, disp_h) = match rot {
            90 => ([0.0, 1.0, -1.0, 0.0, x0 + w, y0], h, w),
            180 => ([-1.0, 0.0, 0.0, -1.0, x0 + w, y0 + h], w, h),
            270 => ([0.0, -1.0, 1.0, 0.0, x0, y0 + h], h, w),
            _ => ([1.0, 0.0, 0.0, 1.0, x0, y0], w, h),
        };

        // The text's box from its baseline origin, turned.
        let tw = width * size / 1000.0;
        let th = CAP_HEIGHT * size / 1000.0;
        let (sin, cos) = angle.to_radians().sin_cos();
        let corners = [(0.0, 0.0), (tw, 0.0), (0.0, th), (tw, th)]
            .map(|(x, y)| (x * cos - y * sin, x * sin + y * cos));
        let min_x = corners.iter().map(|c| c.0).fold(f32::MAX, f32::min);
        let max_x = corners.iter().map(|c| c.0).fold(f32::MIN, f32::max);
        let min_y = corners.iter().map(|c| c.1).fold(f32::MAX, f32::min);
        let max_y = corners.iter().map(|c| c.1).fold(f32::MIN, f32::max);
        let (bw, bh) = (max_x - min_x, max_y - min_y);
        use Position::*;
        let left = match position {
            TopLeft | BottomLeft => margin,
            TopRight | BottomRight => disp_w - margin - bw,
            Center | TopCenter | BottomCenter => (disp_w - bw) / 2.0,
        };
        let bottom = match position {
            BottomLeft | BottomCenter | BottomRight => margin,
            TopLeft | TopCenter | TopRight => disp_h - margin - bh,
            Center => (disp_h - bh) / 2.0,
        };
        let (tx, ty) = (left - min_x, bottom - min_y);

        let real = Object::Real;
        let mut ops = vec![
            Operation::new("Q", vec![]),
            Operation::new("q", vec![]),
            Operation::new("cm", cm.iter().map(|&v| real(v)).collect()),
        ];
        if let Some(gs) = gs_name {
            ops.push(Operation::new("gs", vec![Object::Name(gs)]));
        }
        ops.extend([
            Operation::new("rg", color.iter().map(|&v| real(v)).collect()),
            Operation::new("BT", vec![]),
            Operation::new("Tf", vec![Object::Name(font_name), real(size)]),
            Operation::new(
                "Tm",
                vec![
                    real(cos),
                    real(sin),
                    real(-sin),
                    real(cos),
                    real(tx),
                    real(ty),
                ],
            ),
            Operation::new("Tj", vec![Object::String(bytes, StringFormat::Literal)]),
            Operation::new("ET", vec![]),
            Operation::new("Q", vec![]),
        ]);
        let mut data = b"\n".to_vec();
        data.extend(Content { operations: ops }.encode()?);
        let mut stream = Stream::new(Dictionary::new(), data);
        let _ = stream.compress();
        let stamp = doc.add_object(stream);
        wrap_contents(doc, page, self.save, stamp)
    }
}

/// Give `page` its own Resources dictionary that names `target` under
/// `category`, reusing an existing name for it. Returns the name.
fn add_resource(
    doc: &mut Document,
    page: ObjectId,
    category: &[u8],
    prefix: &str,
    target: ObjectId,
) -> Result<Vec<u8>> {
    let deref = |doc: &Document, o: Option<Object>| -> Dictionary {
        match o {
            Some(Object::Reference(id)) => doc.get_dictionary(id).cloned().unwrap_or_default(),
            Some(Object::Dictionary(d)) => d,
            _ => Dictionary::new(),
        }
    };
    let mut resources = deref(doc, inherited(doc, page, b"Resources"));
    let mut sub = deref(doc, resources.get(category).ok().cloned());
    let existing = sub
        .iter()
        .find(|(_, v)| matches!(v, Object::Reference(id) if *id == target))
        .map(|(k, _)| k.clone());
    let name = match existing {
        Some(name) => name,
        None => {
            let name = (1..)
                .map(|i| format!("{prefix}{i}").into_bytes())
                .find(|n| !sub.has(n))
                .expect("unbounded");
            sub.set(name.clone(), target);
            name
        }
    };
    resources.set(category.to_vec(), sub);
    doc.get_dictionary_mut(page)?.set("Resources", resources);
    Ok(name)
}

/// Contents become `[save, …existing…, stamp]`; `stamp` starts with `Q`.
fn wrap_contents(
    doc: &mut Document,
    page: ObjectId,
    save: ObjectId,
    stamp: ObjectId,
) -> Result<()> {
    let current = doc.get_dictionary(page)?.get(b"Contents").ok().cloned();
    let mut parts = vec![Object::Reference(save)];
    match current {
        Some(Object::Reference(id)) => match doc.get_object(id) {
            Ok(Object::Array(a)) => parts.extend(a.iter().cloned()),
            _ => parts.push(Object::Reference(id)),
        },
        Some(Object::Array(a)) => parts.extend(a),
        _ => {}
    }
    parts.push(Object::Reference(stamp));
    doc.get_dictionary_mut(page)?.set("Contents", parts);
    Ok(())
}

fn check_size(size: f32, what: &str) -> Result<()> {
    if !(size.is_finite() && size > 0.0 && size <= 1000.0) {
        return Err(Error::Invalid(format!(
            "{what} must be between 0 and 1000 points"
        )));
    }
    Ok(())
}

/// Stamp a watermark on the pages in `ranges`. Returns how many pages.
pub fn add_watermark(pdf: &mut Pdf, opts: &WatermarkOptions, ranges: &PageRanges) -> Result<u32> {
    if opts.text.trim().is_empty() {
        return Err(Error::Invalid("the watermark text is empty".into()));
    }
    check_size(opts.font_size, "the font size")?;
    if !(0.0..=1.0).contains(&opts.opacity) {
        return Err(Error::Invalid(
            "opacity must be between 0 and 1 (or 0% and 100%)".into(),
        ));
    }
    encode(&opts.text)?;
    let doc = &mut pdf.doc;
    let ids = page_ids(doc);
    let pages = ranges.resolve_set(ids.len() as u32)?;
    let stamper = Stamper::new(doc, opts.opacity);
    for &p in &pages {
        stamper.stamp(
            doc,
            ids[p as usize - 1],
            &opts.text,
            opts.font_size,
            opts.rotation,
            opts.position,
            opts.margin,
            opts.color,
        )?;
    }
    Ok(pages.len() as u32)
}

/// The text for page number `n` of `total`.
pub(crate) fn format_number(format: &str, n: i64, total: i64) -> String {
    format
        .replace("{n}", &n.to_string())
        .replace("{total}", &total.to_string())
}

/// Number the pages in `ranges`: the first gets `start`, the next `start + 1`,
/// and so on; `{total}` is the last number. Returns how many pages.
pub fn add_page_numbers(
    pdf: &mut Pdf,
    opts: &PageNumberOptions,
    ranges: &PageRanges,
) -> Result<u32> {
    if !opts.format.contains("{n}") && !opts.format.contains("{total}") {
        return Err(Error::Invalid(format!(
            "the format \"{}\" has no {{n}} (try \"{{n}}\" or \"{{n}} of {{total}}\")",
            opts.format
        )));
    }
    check_size(opts.font_size, "the font size")?;
    let doc = &mut pdf.doc;
    let ids = page_ids(doc);
    let pages = ranges.resolve_set(ids.len() as u32)?;
    let total = opts.start + pages.len() as i64 - 1;
    // Check the text once up front so a bad character fails before any change.
    encode(&format_number(&opts.format, total, total))?;
    let stamper = Stamper::new(doc, 1.0);
    for (i, &p) in pages.iter().enumerate() {
        let text = format_number(&opts.format, opts.start + i as i64, total);
        stamper.stamp(
            doc,
            ids[p as usize - 1],
            &text,
            opts.font_size,
            0.0,
            opts.position,
            opts.margin,
            opts.color,
        )?;
    }
    Ok(pages.len() as u32)
}

#[cfg(test)]
pub(crate) fn text_width(text: &str, size: f32) -> f32 {
    encode(text).unwrap().1 * size / 1000.0
}
