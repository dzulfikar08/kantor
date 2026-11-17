//! Pictures the `image` crate and krilla don't read: Windows metafiles (WMF,
//! EMF, EMF+), QuickDraw PICT and BMP/DIB bitmaps.
//!
//! Metafiles and PICTs are played into an SVG document (vector: paths,
//! text, embedded bitmaps) and rasterised with resvg, the renderer GPUI
//! already uses for SVG. On screen the apps turn such a picture into PNG
//! with [`to_png`] (off the UI thread); PDF export draws its [`usvg`] tree
//! ([`Picture::tree`]) as vectors. The file keeps its original bytes.
//!
//! Coverage follows what Office and LibreOffice write: shapes, paths,
//! arcs, text with its spacing and turn, hatches, pattern brushes,
//! bitmaps (with the common raster ops), clipping by rectangles, paths
//! and regions, world transforms and map modes, gradient fills; EMF+ only
//! files through their EMF+ records. Not drawn: GDI regions in WMF, ROP2
//! modes other than copy, EMF+ path gradients (drawn flat), glyph-index
//! text in EMF+ (`DrawDriverString` without character codes).

mod canvas;
mod dib;
mod emf;
mod emfplus;
mod gdi;
mod pict;
mod rd;
mod wmf;

use std::sync::{Arc, OnceLock};

/// The SVG tree library this crate reads its output with (for drawing a
/// [`Picture`] as vectors, e.g. into a PDF).
pub use resvg::usvg;

/// Formats this crate reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Wmf,
    Emf,
    Pict,
    /// A `.bmp` file (with its `BM` header).
    Bmp,
}

/// What a picture's bytes are, by their signature.
pub fn kind(bytes: &[u8]) -> Option<Kind> {
    if emf::is_emf(bytes) {
        Some(Kind::Emf)
    } else if wmf::is_wmf(bytes) {
        Some(Kind::Wmf)
    } else if bytes.starts_with(b"BM") && bytes.len() > 54 && dib::info_len(&bytes[14..]).is_some()
    {
        Some(Kind::Bmp)
    } else if pict::is_pict(bytes) {
        Some(Kind::Pict)
    } else {
        None
    }
}

/// A metafile or PICT played into SVG.
pub struct Picture {
    svg: String,
    width: f64,
    height: f64,
    drawn: usize,
    has_text: bool,
}

impl Picture {
    /// The picture as an SVG document.
    pub fn svg(&self) -> &str {
        &self.svg
    }

    /// Its natural size in pixels at 96 dpi.
    pub fn size(&self) -> (f64, f64) {
        (self.width, self.height)
    }

    /// Whether anything was drawn.
    pub fn is_empty(&self) -> bool {
        self.drawn == 0
    }

    /// The picture as a parsed SVG tree (text already turned into
    /// outlines), for drawing it as vectors. Loads the system fonts the
    /// first time a picture with text needs them (see [`preload_fonts`]).
    pub fn tree(&self) -> Option<usvg::Tree> {
        parse(&self.svg, self.has_text)
    }

    /// Whether the picture has text (drawing it needs the system fonts).
    pub fn has_text(&self) -> bool {
        self.has_text
    }

    /// Rasterised, its longer side at most `max_side` pixels (twice the
    /// natural size when that fits, for sharp zooming and printing).
    pub fn png(&self, max_side: u32) -> Option<Vec<u8>> {
        let (w, h) = (self.width, self.height);
        if !(w > 0.0 && h > 0.0) {
            return None;
        }
        let long = w.max(h);
        let max = max_side.max(16) as f64;
        let scale = (2.0f64).max(256.0 / long).min(max / long);
        let (pw, ph) = ((w * scale).round().max(1.0), (h * scale).round().max(1.0));
        rasterise(&self.svg, pw as u32, ph as u32, self.has_text)
    }
}

fn system_fonts() -> &'static OnceLock<Arc<resvg::usvg::fontdb::Database>> {
    static SYSTEM: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    &SYSTEM
}

fn fonts(text: bool) -> Arc<resvg::usvg::fontdb::Database> {
    static EMPTY: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    if !text {
        return EMPTY
            .get_or_init(|| Arc::new(resvg::usvg::fontdb::Database::new()))
            .clone();
    }
    system_fonts()
        .get_or_init(|| {
            let mut db = resvg::usvg::fontdb::Database::new();
            db.load_system_fonts();
            // Office's usual faces, then what Linux has instead.
            for (generic, names) in [
                (
                    "sans",
                    ["Arial", "Liberation Sans", "DejaVu Sans", "Helvetica"],
                ),
                (
                    "serif",
                    [
                        "Times New Roman",
                        "Liberation Serif",
                        "DejaVu Serif",
                        "Times",
                    ],
                ),
            ] {
                let found = names.iter().find(|n| {
                    db.faces().any(|f| {
                        f.families
                            .iter()
                            .any(|(fam, _)| fam.eq_ignore_ascii_case(n))
                    })
                });
                if let Some(n) = found {
                    if generic == "sans" {
                        db.set_sans_serif_family(*n);
                    } else {
                        db.set_serif_family(*n);
                    }
                }
            }
            Arc::new(db)
        })
        .clone()
}

/// Load the system fonts pictures with text are drawn with, on a
/// background thread, so that the first such picture doesn't wait for
/// them (loading takes up to a second). Does nothing after the first call.
pub fn preload_fonts() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let spawned = std::thread::Builder::new()
            .name("metafile-fonts".into())
            .spawn(|| {
                fonts(true);
            });
        if let Err(e) = spawned {
            log::warn!("metafile: fonts not preloaded: {e}");
        }
    });
}

/// Whether the system fonts are loaded (drawing a picture with text
/// won't wait for them).
pub fn fonts_ready() -> bool {
    system_fonts().get().is_some()
}

fn parse(svg: &str, text: bool) -> Option<usvg::Tree> {
    let opts = usvg::Options {
        fontdb: fonts(text),
        ..Default::default()
    };
    usvg::Tree::from_str(svg, &opts)
        .map_err(|e| log::warn!("metafile: SVG not read: {e}"))
        .ok()
}

fn rasterise(svg: &str, w: u32, h: u32, text: bool) -> Option<Vec<u8>> {
    let tree = parse(svg, text)?;
    let mut map = resvg::tiny_skia::Pixmap::new(w, h)?;
    let size = tree.size();
    let t = resvg::tiny_skia::Transform::from_scale(
        w as f32 / size.width().max(1e-3),
        h as f32 / size.height().max(1e-3),
    );
    resvg::render(&tree, t, &mut map.as_mut());
    map.encode_png().ok()
}

/// Play a WMF, EMF or PICT into SVG.
pub fn render(bytes: &[u8]) -> Option<Picture> {
    match kind(bytes)? {
        Kind::Emf => emf::play(bytes),
        Kind::Wmf => wmf::play(bytes),
        Kind::Pict => pict::play(bytes),
        Kind::Bmp => None,
    }
}

/// The natural size (pixels at 96 dpi) of a picture this crate reads.
pub fn natural_size(bytes: &[u8]) -> Option<(f64, f64)> {
    match kind(bytes)? {
        Kind::Bmp => {
            let img = dib::decode_packed(&bytes[14..], false)?;
            Some((img.w as f64, img.h as f64))
        }
        _ => render(bytes).map(|p| p.size()),
    }
}

/// Any picture this crate reads as PNG (see [`Picture::png`] for the size).
pub fn to_png(bytes: &[u8], max_side: u32) -> Option<Vec<u8>> {
    match kind(bytes)? {
        Kind::Bmp => bmp_to_png(bytes),
        _ => render(bytes)?.png(max_side),
    }
}

/// A `.bmp` file as PNG.
pub fn bmp_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    if !bytes.starts_with(b"BM") {
        return None;
    }
    dib_to_png(bytes.get(14..)?)
}

/// A packed DIB (BITMAPINFOHEADER, colour table, pixels: what `.doc`,
/// `.xls` and `.ppt` store as "DIB" pictures) as PNG.
pub fn dib_to_png(dib: &[u8]) -> Option<Vec<u8>> {
    if let Some(inner) = dib::embedded(dib) {
        return Some(inner.to_vec());
    }
    let alpha = rd::u16_at(dib, 14) == 32 && rd::u32_at(dib, 0) >= 40;
    let mut img = dib::decode_packed(dib, alpha)?;
    // 32-bit DIBs often leave the fourth byte zero: then it isn't alpha.
    if alpha && img.px.iter().all(|p| p[3] == 0) {
        for p in &mut img.px {
            p[3] = 255;
        }
    }
    img.png()
}

/// A packed DIB with a `BM` file header in front (a `.bmp` file).
pub fn dib_to_bmp(dib: &[u8]) -> Vec<u8> {
    let (info_len, _) = dib::info_len(dib).unwrap_or((40, 0));
    let mut out = Vec::with_capacity(dib.len() + 14);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((dib.len() + 14) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&((info_len + 14) as u32).to_le_bytes());
    out.extend_from_slice(dib);
    out
}

/// PNG, JPEG or GIF: the formats every Kantor picture path draws.
pub(crate) fn raster_mime(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF8") {
        Some("image/gif")
    } else {
        None
    }
}

/// Pixel size of a PNG, GIF or JPEG.
pub(crate) fn raster_size(b: &[u8]) -> Option<(f64, f64)> {
    if b.starts_with(b"\x89PNG") {
        return Some((rd::be32(b, 16) as f64, rd::be32(b, 20) as f64));
    }
    if b.starts_with(b"GIF8") {
        return Some((rd::u16_at(b, 6) as f64, rd::u16_at(b, 8) as f64));
    }
    if b.starts_with(&[0xFF, 0xD8]) {
        let mut i = 2;
        while i + 9 < b.len() {
            if b[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = b[i + 1];
            let len = rd::be16(b, i + 2) as usize;
            if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC
            {
                return Some((rd::be16(b, i + 7) as f64, rd::be16(b, i + 5) as f64));
            }
            i += 2 + len;
        }
    }
    None
}

/// A picture from an OfficeArt BLIP record (`.doc`, `.xls`, `.ppt`):
/// its usual file extension and bytes as a file of that type.
pub struct Blip {
    pub ext: &'static str,
    pub data: Vec<u8>,
}

/// Read the BLIP record of type `ty` (0xF01A…0xF02A) whose instance is
/// `inst` and body `body`: metafiles are inflated (and a WMF gets back the
/// placeable header Office strips), DIBs become `.bmp` files.
pub fn office_art_blip(ty: u16, inst: u16, body: &[u8]) -> Option<Blip> {
    use std::io::Read;
    let uids = if inst & 1 == 1 { 32 } else { 16 };
    let (ext, metafile) = match ty {
        0xF01A => ("emf", true),
        0xF01B => ("wmf", true),
        0xF01C => ("pict", true),
        0xF01D | 0xF02A => ("jpeg", false),
        0xF01E => ("png", false),
        0xF01F => ("bmp", false),
        0xF029 => ("tiff", false),
        _ => return None,
    };
    let rest = body.get(uids..)?;
    if !metafile {
        // A tag byte, then the file.
        let data = rest.get(1..)?;
        let data = if ext == "bmp" {
            dib_to_bmp(data)
        } else {
            data.to_vec()
        };
        return Some(Blip { ext, data });
    }
    // OfficeArtMetafileHeader: cbSize, rcBounds, ptSize (EMU), cbSave,
    // compression (0 deflate, 0xFE none), filter.
    let raw = rd::u32_at(rest, 0) as usize;
    let bounds = [
        rd::i32_at(rest, 4),
        rd::i32_at(rest, 8),
        rd::i32_at(rest, 12),
        rd::i32_at(rest, 16),
    ];
    let size = (rd::i32_at(rest, 20), rd::i32_at(rest, 24));
    let saved = rd::u32_at(rest, 28) as usize;
    let compression = rd::u8_at(rest, 32);
    let data = rest.get(34..)?;
    let data = &data[..saved.min(data.len())];
    let mut data = if compression == 0 {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(data)
            .take(raw.min(256 << 20) as u64)
            .read_to_end(&mut out)
            .ok()?;
        out
    } else {
        data.to_vec()
    };
    if ext == "wmf" {
        data = placeable_wmf(data, bounds, size);
    }
    Some(Blip { ext, data })
}

/// A WMF with a placeable header (frame `bounds` in its logical units,
/// `size_emu` its size); unchanged when it has one or the frame is empty.
pub fn placeable_wmf(wmf: Vec<u8>, bounds: [i32; 4], size_emu: (i32, i32)) -> Vec<u8> {
    let [l, t, r, b] = bounds;
    if rd::u32_at(&wmf, 0) == 0x9AC6_CDD7 || r <= l || b <= t {
        return wmf;
    }
    let fit = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    // Units per inch so that the frame is `size_emu` wide.
    let inch = if size_emu.0 > 0 {
        ((r - l) as f64 / (size_emu.0 as f64 / 914_400.0)).round()
    } else {
        1440.0
    };
    let inch = inch.clamp(1.0, 65_535.0) as u16;
    let mut out = Vec::with_capacity(wmf.len() + 22);
    out.extend_from_slice(&0x9AC6_CDD7u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    for v in [fit(l), fit(t), fit(r), fit(b)] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&inch.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    let mut sum = 0u16;
    for w in out.as_chunks::<2>().0 {
        sum ^= u16::from_le_bytes(*w);
    }
    out.extend_from_slice(&sum.to_le_bytes());
    out.extend_from_slice(&wmf);
    out
}
