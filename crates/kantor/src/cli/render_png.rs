//! Slides as PNG files (`kantor convert deck.pptx -o DIR --to png`).
//!
//! The slides are written as a PDF by the PDF export (`slides_pdf`, the
//! scenes the canvas paints) and that PDF is rasterized with hayro (as the
//! PDF viewer and Sheets' print preview do), so a PNG shows exactly what
//! the PDF shows: pictures, gradients, tables, charts and text included.
//! No window is needed.
//!
//! A contact sheet puts every slide in one PNG, numbered, for a quick look
//! at a whole deck (agents that can only read pictures review decks so).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow, bail};
use slides_engine::Presentation;
use slides_scene::{Builder, Engine, Options};

/// Default width of one slide's PNG, px.
pub const DEFAULT_WIDTH: u32 = 1600;
/// Default width of a slide in a contact sheet, px.
pub const SHEET_WIDTH: u32 = 480;
/// Widths allowed (the rasterizer's limit is [`pdf_engine::MAX_SIDE`]).
pub const MIN_WIDTH: u32 = 16;
pub const MAX_WIDTH: u32 = 6000;

/// An opaque RGB picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

impl Image {
    fn new(width: u32, height: u32, color: [u8; 3]) -> Self {
        Self {
            width,
            height,
            rgb: color
                .iter()
                .copied()
                .cycle()
                .take(width as usize * height as usize * 3)
                .collect(),
        }
    }

    fn put(&mut self, x: u32, y: u32, c: [u8; 3]) {
        if x < self.width && y < self.height {
            let i = (y as usize * self.width as usize + x as usize) * 3;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }

    fn rect(&mut self, x: u32, y: u32, w: u32, h: u32, c: [u8; 3]) {
        for yy in y..y + h {
            for xx in x..x + w {
                self.put(xx, yy, c);
            }
        }
    }

    /// `src` with its top left at `x`, `y`.
    fn paste(&mut self, src: &Image, x: u32, y: u32) {
        for row in 0..src.height {
            let dy = y + row;
            if dy >= self.height {
                break;
            }
            let w = src.width.min(self.width.saturating_sub(x)) as usize;
            let s = row as usize * src.width as usize * 3;
            let d = (dy as usize * self.width as usize + x as usize) * 3;
            self.rgb[d..d + w * 3].copy_from_slice(&src.rgb[s..s + w * 3]);
        }
    }

    /// The picture as a PNG file (8-bit RGB).
    pub fn png(&self) -> Vec<u8> {
        use std::io::Write as _;
        let stride = self.width as usize * 3;
        let mut raw = Vec::with_capacity((stride + 1) * self.height as usize);
        for row in self.rgb.chunks_exact(stride.max(1)) {
            // Filter 1 (Sub): each byte minus the one a pixel to the left.
            raw.push(1);
            for (i, &b) in row.iter().enumerate() {
                let left = if i >= 3 { row[i - 3] } else { 0 };
                raw.push(b.wrapping_sub(left));
            }
        }
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = z.write_all(&raw);
        let data = z.finish().unwrap_or_default();
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut chunk = |kind: &[u8], body: &[u8]| {
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            let mut c = flate2::Crc::new();
            c.update(kind);
            c.update(body);
            out.extend_from_slice(kind);
            out.extend_from_slice(body);
            out.extend_from_slice(&c.sum().to_be_bytes());
        };
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&self.width.to_be_bytes());
        ihdr.extend_from_slice(&self.height.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(b"IHDR", &ihdr);
        chunk(b"IDAT", &data);
        chunk(b"IEND", &[]);
        out
    }
}

/// Slides `slides` (0-based) of `pres`, each `width` px wide.
pub fn render(
    pres: &Presentation,
    engine: &mut Engine,
    slides: &[usize],
    width: u32,
) -> Result<Vec<Image>> {
    if slides.is_empty() {
        return Ok(Vec::new());
    }
    let builder = Builder::new();
    let deck = slides_pdf::Deck {
        title: pres.title(),
        width: pres.size.cx as f32 / slides_engine::EMU_PER_PX,
        height: pres.size.cy as f32 / slides_engine::EMU_PER_PX,
        slides: slides
            .iter()
            .map(|&i| builder.slide(engine, pres, i, &Options::default()))
            .collect(),
    };
    let pdf = slides_pdf::write(&deck)?;
    let doc = pdf_engine::PdfDoc::open(Arc::new(pdf), "")
        .map_err(|e| anyhow!("reading the slides' PDF: {e}"))?;
    let renderer = pdf_engine::Renderer::new(&doc);
    // Page width in points (px × 0.75).
    let page_w = (deck.width * 0.75).max(1.0);
    let scale = width.clamp(MIN_WIDTH, MAX_WIDTH) as f32 / page_w;
    (0..slides.len())
        .map(|page| {
            let r = renderer
                .render(page, scale, pdf_engine::Turn(0))
                .ok_or_else(|| anyhow!("can't draw slide {}", slides[page] + 1))?;
            let rgb = r
                .rgba
                .chunks_exact(4)
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            Ok(Image {
                width: r.width,
                height: r.height,
                rgb,
            })
        })
        .collect()
}

/// Slide numbers `spec` names: `3`, `2-5`, `1,3,7-9`, `4-` (to the end);
/// 0-based, in order, each once. `n` is the slide count.
pub fn parse_slides(spec: &str, n: usize) -> Result<Vec<usize>, String> {
    let mut out = Vec::new();
    let num = |s: &str| -> Result<usize, String> {
        let s = s.trim();
        match s.parse::<usize>() {
            Ok(v) if (1..=n).contains(&v) => Ok(v),
            Ok(v) => Err(format!("there is no slide {v}; the deck has {n}")),
            Err(_) => Err(format!(
                "--slide {spec}: use a number, N-M or a list (1,3,5-7)"
            )),
        }
    };
    for part in spec.split(',').filter(|p| !p.trim().is_empty()) {
        let (a, b) = match part.split_once('-') {
            Some((a, b)) => (
                if a.trim().is_empty() { 1 } else { num(a)? },
                if b.trim().is_empty() { n } else { num(b)? },
            ),
            None => {
                let v = num(part)?;
                (v, v)
            }
        };
        if a > b {
            return Err(format!("--slide {spec}: {a}-{b} goes backwards"));
        }
        for v in a..=b {
            if !out.contains(&(v - 1)) {
                out.push(v - 1);
            }
        }
    }
    if out.is_empty() {
        return Err(format!("--slide {spec}: no slides"));
    }
    Ok(out)
}

/// 3 × 5 digits for slide numbers on contact sheets (rows of 3 bits).
const DIGITS: [[u8; 5]; 10] = [
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [7, 1, 7, 4, 7],
    [7, 1, 7, 1, 7],
    [5, 5, 7, 1, 1],
    [7, 4, 7, 1, 7],
    [7, 4, 7, 5, 7],
    [7, 1, 1, 1, 1],
    [7, 5, 7, 5, 7],
    [7, 5, 7, 1, 7],
];

fn draw_number(img: &mut Image, n: usize, x: u32, y: u32, k: u32, c: [u8; 3]) {
    for (i, ch) in n.to_string().bytes().enumerate() {
        let g = DIGITS[usize::from(ch - b'0')];
        let gx = x + i as u32 * 4 * k;
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (4 >> col) != 0 {
                    img.rect(gx + col * k, y + row as u32 * k, k, k, c);
                }
            }
        }
    }
}

/// Columns for `n` thumbnails `w` × `h` (label strip included): a sheet
/// about 16:10.
fn columns(n: usize, w: u32, h: u32) -> usize {
    let c = (1.6 * n as f32 * h as f32 / w.max(1) as f32).sqrt().ceil() as usize;
    c.clamp(1, 8).min(n.max(1))
}

/// One picture of `thumbs` (slide number from 1, picture) in a grid,
/// each numbered below.
pub fn contact_sheet(thumbs: &[(usize, Image)]) -> Image {
    const GAP: u32 = 24;
    const LABEL: u32 = 30;
    const BACK: [u8; 3] = [0xE6, 0xE6, 0xE6];
    const EDGE: [u8; 3] = [0x99, 0x99, 0x99];
    const INK: [u8; 3] = [0x33, 0x33, 0x33];
    let Some((_, first)) = thumbs.first() else {
        return Image::new(GAP * 2, GAP * 2, BACK);
    };
    let (tw, th) = (first.width, first.height);
    let cols = columns(thumbs.len(), tw, th + LABEL) as u32;
    let rows = (thumbs.len() as u32).div_ceil(cols);
    let width = GAP + cols * (tw + GAP);
    let height = GAP + rows * (th + LABEL + GAP);
    let mut sheet = Image::new(width, height, BACK);
    for (k, (n, img)) in thumbs.iter().enumerate() {
        let (c, r) = (k as u32 % cols, k as u32 / cols);
        let x = GAP + c * (tw + GAP);
        let y = GAP + r * (th + LABEL + GAP);
        sheet.rect(x - 1, y - 1, img.width + 2, img.height + 2, EDGE);
        sheet.paste(img, x, y);
        draw_number(&mut sheet, *n, x, y + th + 6, 4, INK);
    }
    sheet
}

/// What `kantor convert … --to png` was asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PngOptions {
    /// `--slide`: which slides (default: every slide shown in the show).
    pub slides: Option<String>,
    /// `--width`: px per slide.
    pub width: Option<u32>,
    /// `--contact-sheet`: one PNG of all the slides.
    pub sheet: bool,
}

impl PngOptions {
    pub fn given(&self) -> bool {
        *self != PngOptions::default()
    }
}

/// The files for `output` (`DIR/deck.png` or a name given): `deck-01.png`…
/// per slide, or the name itself for one slide named by the user or a
/// contact sheet (`deck-sheet.png` when the name came from `-o DIR`).
fn names(
    output: &Path,
    numbered: bool,
    slides: &[usize],
    total: usize,
    sheet: bool,
) -> Vec<PathBuf> {
    let stem = output
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "slides".to_owned());
    let dir = output.parent().unwrap_or(Path::new(""));
    if sheet {
        return vec![if numbered {
            dir.join(format!("{stem}-sheet.png"))
        } else {
            output.to_path_buf()
        }];
    }
    if !numbered && slides.len() == 1 {
        return vec![output.to_path_buf()];
    }
    let digits = total.to_string().len().max(2);
    slides
        .iter()
        .map(|i| dir.join(format!("{stem}-{:0digits$}.png", i + 1)))
        .collect()
}

/// Convert a presentation to PNG files; returns the summary line.
pub fn convert(
    pres: &Presentation,
    output: &Path,
    numbered: bool,
    opts: &PngOptions,
    force: bool,
) -> Result<String> {
    let n = pres.len();
    if n == 0 {
        bail!("the presentation has no slides");
    }
    let slides = match &opts.slides {
        Some(spec) => parse_slides(spec, n).map_err(|e| anyhow!(e))?,
        None => (0..n).filter(|&i| !pres.slide(i).hidden).collect(),
    };
    if slides.is_empty() {
        bail!("every slide is hidden (name them with --slide)");
    }
    let width = opts.width.unwrap_or(if opts.sheet {
        SHEET_WIDTH
    } else {
        DEFAULT_WIDTH
    });
    let files = names(output, numbered, &slides, n, opts.sheet);
    for f in &files {
        if f.exists() && !force {
            bail!("{} exists (use -f to overwrite)", f.display());
        }
    }
    let mut engine = Engine::new();
    let images = render(pres, &mut engine, &slides, width)?;
    let (w, h) = images.first().map_or((0, 0), |i| (i.width, i.height));
    if opts.sheet {
        let thumbs: Vec<(usize, Image)> = slides.iter().map(|i| i + 1).zip(images).collect();
        let sheet = contact_sheet(&thumbs);
        docs_pdf::write_file(&files[0], &sheet.png())
            .with_context(|| format!("can't write {}", files[0].display()))?;
        return Ok(format!(
            "contact sheet of {} slide{}, {} × {} px",
            thumbs.len(),
            if thumbs.len() == 1 { "" } else { "s" },
            sheet.width,
            sheet.height
        ));
    }
    for (img, f) in images.iter().zip(&files) {
        docs_pdf::write_file(f, &img.png())
            .with_context(|| format!("can't write {}", f.display()))?;
    }
    let shown = match files.as_slice() {
        [one] => one.display().to_string(),
        [a, .., b] => format!("{} … {}", a.display(), b.display()),
        [] => String::new(),
    };
    Ok(format!(
        "{} PNG{}, {w} × {h} px: {shown}",
        files.len(),
        if files.len() == 1 { "" } else { "s" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn deck() -> Presentation {
        let json = json!({"units": "in", "background": "#FFFFFF", "slides": [
            {"layout": "Blank", "elements": [
                {"type": "shape", "x": 0, "y": 0, "w": 6.6667, "h": 7.5, "fill": "#C00000", "line": "none"},
                {"type": "shape", "x": 7, "y": 1, "w": 5, "h": 5, "fill": {"gradient": {"type": "linear", "angle": 0, "stops": ["#0000FF", "#00FF00"]}}, "line": "none"},
                {"type": "text", "x": 1, "y": 1, "w": 5, "h": 1, "text": "Hello", "size": 40, "color": "#FFFFFF"},
            ]},
            {"layout": "Blank", "background": "#203040", "elements": []},
        ]});
        crate::agent::slides_author::build(&json, None).unwrap()
    }

    fn px(img: &Image, x: u32, y: u32) -> [u8; 3] {
        let i = (y as usize * img.width as usize + x as usize) * 3;
        [img.rgb[i], img.rgb[i + 1], img.rgb[i + 2]]
    }

    #[std::prelude::v1::test]
    fn slides_render_at_the_asked_width() {
        let p = deck();
        let mut engine = Engine::new();
        let imgs = render(&p, &mut engine, &[0, 1], 320).unwrap();
        assert_eq!(imgs.len(), 2);
        assert_eq!(imgs[0].width, 320);
        assert_eq!(imgs[0].height, 180);
        // The red half, the plain background of slide 2.
        let red = px(&imgs[0], 40, 150);
        assert!(red[0] > 180 && red[1] < 40 && red[2] < 40, "{red:?}");
        assert_eq!(px(&imgs[1], 160, 90), [0x20, 0x30, 0x40]);
        // Not blank: the gradient has more than one colour.
        let (a, b) = (px(&imgs[0], 200, 40), px(&imgs[0], 290, 140));
        assert_ne!(a, b);
        // The same deck, the same pixels.
        let again = render(&p, &mut engine, &[0, 1], 320).unwrap();
        assert_eq!(imgs, again);
        assert_eq!(imgs[0].png(), again[0].png());
        let png = imgs[0].png();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[16..24], &[0, 0, 1, 64, 0, 0, 0, 180]);
    }

    #[std::prelude::v1::test]
    fn contact_sheets_number_the_slides() {
        let p = deck();
        let imgs = render(&p, &mut Engine::new(), &[0, 1], 160).unwrap();
        let thumbs: Vec<_> = [1, 2].into_iter().zip(imgs).collect();
        let sheet = contact_sheet(&thumbs);
        assert_eq!(sheet.width, 24 + 2 * (160 + 24));
        assert_eq!(sheet.height, 24 + 90 + 30 + 24);
        assert_eq!(px(&sheet, 24 + 184 + 80, 24 + 45), [0x20, 0x30, 0x40]);
        assert_eq!(columns(10, 480, 300), 4);
        assert_eq!(columns(1, 480, 300), 1);
    }

    #[std::prelude::v1::test]
    fn slide_lists_and_file_names() {
        assert_eq!(parse_slides("3", 10).unwrap(), vec![2]);
        assert_eq!(parse_slides("1,3,7-9", 10).unwrap(), vec![0, 2, 6, 7, 8]);
        assert_eq!(parse_slides("9-", 10).unwrap(), vec![8, 9]);
        assert!(parse_slides("11", 10).unwrap_err().contains("no slide 11"));
        assert!(parse_slides("5-2", 10).unwrap_err().contains("backwards"));
        assert!(parse_slides("x", 10).is_err());
        let out = Path::new("out/deck.png");
        assert_eq!(
            names(out, true, &[0, 9], 10, false),
            vec![
                PathBuf::from("out/deck-01.png"),
                PathBuf::from("out/deck-10.png")
            ]
        );
        assert_eq!(
            names(out, true, &[4], 120, false),
            vec![PathBuf::from("out/deck-005.png")]
        );
        assert_eq!(
            names(out, true, &[0, 1], 2, true),
            vec![PathBuf::from("out/deck-sheet.png")]
        );
        assert_eq!(
            names(out, false, &[3], 9, false),
            vec![PathBuf::from("out/deck.png")]
        );
    }

    #[std::prelude::v1::test]
    fn convert_writes_numbered_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = deck();
        let out = dir.path().join("deck.png");
        let opts = PngOptions {
            width: Some(200),
            ..PngOptions::default()
        };
        let msg = convert(&p, &out, true, &opts, false).unwrap();
        assert!(msg.starts_with("2 PNGs, 200 × 11"), "{msg}");
        assert!(dir.path().join("deck-01.png").is_file());
        assert!(dir.path().join("deck-02.png").is_file());
        assert!(
            convert(&p, &out, true, &opts, false)
                .unwrap_err()
                .to_string()
                .contains("exists")
        );
        let sheet = PngOptions {
            sheet: true,
            width: Some(100),
            ..PngOptions::default()
        };
        convert(&p, &out, true, &sheet, false).unwrap();
        assert!(dir.path().join("deck-sheet.png").is_file());
    }
}
