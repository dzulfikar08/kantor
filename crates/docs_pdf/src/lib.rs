//! Kantor Docs → PDF (no GPUI).
//!
//! [`build`] turns a [`docs_engine::Document`] into `docs_layout` paragraphs
//! and blocks; the Docs view paints that layout with GPUI and this crate
//! writes the same layout with [krilla], so the PDF matches the screen by
//! construction (ADR 0001).
//!
//! Export is two steps so the slow one can leave the UI thread:
//! 1. [`scene`]: collect each page's glyph runs, rules and pictures from a
//!    laid-out document (cheap; needs the `Document` and `PagedDocument`).
//! 2. [`write`]: encode a [`Scene`] as PDF, subsetting and embedding every
//!    font (with ToUnicode, so text can be searched and copied). `Scene` is
//!    `Send`.
//!
//! [`export`] does both for headless callers (`kantor convert`).
//!
//! What the PDF leaves out: Kantor's own placeholders for content it can't
//! draw (charts, shapes, unsupported picture formats: the space stays
//! empty), and headers/footers, which the model keeps only as XML.

pub mod build;

pub use docs_layout::Engine;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use docs_engine::{Document, ImageFormat, Object};
use docs_layout::parley::FontData;
use docs_layout::{ObjectKind, PagedDocument, PositionedGlyph, Rgba, RunSource};
use krilla::color::rgb;
use krilla::error::KrillaError;
use krilla::geom::{PathBuilder, Point, Rect, Size, Transform};
use krilla::image::Image;
use krilla::metadata::Metadata;
use krilla::num::NormalizedF32;
use krilla::page::PageSettings;
use krilla::paint::{Fill, Stroke};
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph};

/// CSS px (96 dpi, the layout's unit) to PDF points.
const TO_PT: f32 = 72.0 / 96.0;

/// One page's content in page coordinates (px), ready to encode.
#[derive(Clone, Debug, Default)]
pub struct PageScene {
    /// Filled rectangles (table borders), `[x, y, w, h]`.
    pub rules: Vec<([f32; 4], Rgba)>,
    pub pictures: Vec<Picture>,
    pub runs: Vec<TextRun>,
}

/// A picture placed on a page.
#[derive(Clone, Debug)]
pub struct Picture {
    pub rect: [f32; 4],
    pub format: ImageFormat,
    pub bytes: Arc<[u8]>,
}

/// A glyph run with the text its clusters index (for ToUnicode).
#[derive(Clone, Debug)]
pub struct TextRun {
    pub font: FontData,
    pub font_size: f32,
    pub color: Rgba,
    pub embolden: bool,
    pub skew_deg: Option<f32>,
    pub origin_x: f32,
    pub baseline: f32,
    pub advance: f32,
    pub underline: Option<(f32, f32)>,
    pub glyphs: Vec<PositionedGlyph>,
    pub text: Arc<str>,
}

/// A picture's format and file bytes.
type PictureData = (ImageFormat, Arc<[u8]>);

/// A whole document ready to encode: page size (px) and pages.
#[derive(Clone, Debug)]
pub struct Scene {
    pub width: f32,
    pub height: f32,
    pub title: String,
    pub pages: Vec<PageScene>,
}

/// Lay a document out on pages the way the Docs view does (without its
/// per-paragraph cache).
pub fn lay_out(doc: &Document, engine: &mut Engine) -> PagedDocument {
    let setup = build::page_setup(doc);
    let (paras, widths, blocks) = build::build(doc);
    let layouts = paras
        .iter()
        .zip(&widths)
        .map(|(p, &w)| Arc::new(engine.layout_paragraph(p, w)))
        .collect();
    PagedDocument::paginate_blocks(&paras, layouts, &blocks, setup)
}

/// Collect what every page draws. `paged` must be `doc` laid out with
/// [`build`] (as [`lay_out`] or the Docs view does).
pub fn scene(doc: &Document, paged: &PagedDocument) -> Scene {
    let texts: Vec<Arc<str>> = doc
        .paragraphs()
        .iter()
        .map(|p| Arc::from(p.text.as_str()))
        .collect();
    let mut markers: HashMap<usize, Arc<str>> = HashMap::new();
    // Picture bytes by relationship id; `None` when not PNG/JPEG.
    let mut pictures: HashMap<String, Option<PictureData>> = HashMap::new();
    let pages = (0..paged.pages.len())
        .map(|page| {
            let mut out = PageScene {
                rules: paged.pages[page]
                    .rules
                    .iter()
                    .map(|r| (r.rect, r.color))
                    .collect(),
                ..PageScene::default()
            };
            for o in paged.objects(page) {
                if o.kind != ObjectKind::Image {
                    continue;
                }
                let Some(Object::Image(img)) = doc.paragraph(o.para).object_at(o.index) else {
                    continue;
                };
                let found = pictures.entry(img.rel_id.clone()).or_insert_with(|| {
                    let bytes = doc.image_bytes(&img.rel_id)?;
                    match ImageFormat::sniff(&bytes) {
                        ImageFormat::Other => None,
                        format => Some((format, bytes)),
                    }
                });
                if let Some((format, bytes)) = found {
                    out.pictures.push(Picture {
                        rect: o.rect,
                        format: *format,
                        bytes: bytes.clone(),
                    });
                }
            }
            for run in paged.glyph_runs(page) {
                let text = match run.source {
                    RunSource::Text => texts.get(run.para).cloned(),
                    RunSource::Marker => Some(
                        markers
                            .entry(run.para)
                            .or_insert_with(|| Arc::from(paged.run_text(&run, "")))
                            .clone(),
                    ),
                    // Kantor's placeholder labels are not document content.
                    RunSource::Label(_) => None,
                };
                let Some(text) = text else { continue };
                if run.glyphs.is_empty() {
                    continue;
                }
                out.runs.push(TextRun {
                    font: run.font,
                    font_size: run.font_size,
                    color: run.color,
                    embolden: run.embolden,
                    skew_deg: run.skew_deg,
                    origin_x: run.origin_x,
                    baseline: run.baseline,
                    advance: run.advance,
                    underline: run.underline,
                    glyphs: run.glyphs,
                    text,
                });
            }
            out
        })
        .collect();
    Scene {
        width: paged.setup.width,
        height: paged.setup.height,
        title: doc.title(),
        pages,
    }
}

/// Lay out and encode `doc` as PDF.
pub fn export(doc: &Document, engine: &mut Engine) -> Result<Vec<u8>> {
    let paged = lay_out(doc, engine);
    write(&scene(doc, &paged))
}

/// Encode a scene as PDF. Pictures krilla can't decode are left out (with
/// a warning) rather than failing the export.
pub fn write(scene: &Scene) -> Result<Vec<u8>> {
    let mut skip: HashSet<*const u8> = HashSet::new();
    loop {
        let mut images: HashMap<*const u8, Option<Image>> = HashMap::new();
        match encode(scene, &skip, &mut images) {
            Ok(bytes) => return Ok(bytes),
            Err(KrillaError::Image(bad, _, why)) => {
                let key = images
                    .iter()
                    .find(|(_, i)| i.as_ref() == Some(&bad))
                    .map(|(k, _)| *k)
                    .ok_or_else(|| anyhow!("writing PDF: picture: {why}"))?;
                log::warn!("PDF export: leaving out a picture krilla can't decode: {why}");
                skip.insert(key);
            }
            Err(e) => return Err(anyhow!("writing PDF: {e}")),
        }
    }
}

/// Write PDF bytes to `path` atomically (temp file in the same folder, then
/// rename), so a failed export never leaves half a file.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("can't write in {}", dir.display()))?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .map_err(|e| anyhow!("{}: {}", path.display(), e.error))?;
    Ok(())
}

fn fill(c: Rgba) -> Fill {
    let [r, g, b, a] = c.0;
    Fill {
        paint: rgb::Color::new(r, g, b).into(),
        opacity: NormalizedF32::new(a as f32 / 255.0).unwrap_or(NormalizedF32::ONE),
        rule: Default::default(),
    }
}

fn fill_rect(surface: &mut Surface, [x, y, w, h]: [f32; 4], color: Rgba) {
    let Some(rect) = Rect::from_xywh(x * TO_PT, y * TO_PT, w * TO_PT, h * TO_PT) else {
        return;
    };
    let mut pb = PathBuilder::new();
    pb.push_rect(rect);
    if let Some(path) = pb.finish() {
        surface.set_fill(Some(fill(color)));
        surface.draw_path(&path);
    }
}

fn encode(
    scene: &Scene,
    skip: &HashSet<*const u8>,
    images: &mut HashMap<*const u8, Option<Image>>,
) -> Result<Vec<u8>, KrillaError> {
    let mut fonts: HashMap<(u64, u32), Option<Font>> = HashMap::new();
    let mut pdf = krilla::Document::new();
    pdf.set_metadata(
        Metadata::new()
            .title(scene.title.clone())
            .creator("Kantor".into())
            .producer(format!("Kantor {}", env!("CARGO_PKG_VERSION"))),
    );
    let settings = || {
        PageSettings::from_wh(
            (scene.width * TO_PT).max(1.0),
            (scene.height * TO_PT).max(1.0),
        )
        .expect("positive page size")
    };
    let blank = [PageScene::default()];
    let pages = if scene.pages.is_empty() {
        &blank[..]
    } else {
        &scene.pages[..]
    };
    for content in pages {
        let mut page = pdf.start_page_with(settings());
        let mut surface = page.surface();
        for (rect, color) in &content.rules {
            fill_rect(&mut surface, *rect, *color);
        }
        for pic in &content.pictures {
            let key = pic.bytes.as_ptr();
            if skip.contains(&key) {
                continue;
            }
            let image = images
                .entry(key)
                .or_insert_with(|| {
                    let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(pic.bytes.clone());
                    let made = match pic.format {
                        ImageFormat::Png => Image::from_png(data.into(), true),
                        ImageFormat::Jpeg => Image::from_jpeg(data.into(), true),
                        ImageFormat::Other => Err("not PNG or JPEG".into()),
                    };
                    made.map_err(|e| log::warn!("PDF export: picture left out: {e}"))
                        .ok()
                })
                .clone();
            let [x, y, w, h] = pic.rect;
            let (Some(image), Some(size)) = (image, Size::from_wh(w * TO_PT, h * TO_PT)) else {
                continue;
            };
            surface.push_transform(&Transform::from_translate(x * TO_PT, y * TO_PT));
            surface.draw_image(image, size);
            surface.pop();
        }
        for run in &content.runs {
            let font = fonts
                .entry((run.font.data.id(), run.font.index))
                .or_insert_with(|| {
                    let (data, _) = run.font.data.clone().into_raw_parts();
                    let font = Font::new(data.into(), run.font.index);
                    if font.is_none() {
                        log::warn!("PDF export: a font krilla can't read; its text is left out");
                    }
                    font
                })
                .clone();
            let Some(font) = font else { continue };
            draw_run(&mut surface, run, font);
        }
        surface.finish();
        page.finish();
    }
    pdf.finish()
}

fn draw_run(surface: &mut Surface, run: &TextRun, font: Font) {
    let size = run.font_size;
    if size <= 0.0 {
        return;
    }
    // krilla takes advances and offsets in font-size units, relative to a
    // pen that moves by each glyph's advance (justification included).
    let mut pen = run.origin_x;
    let glyphs: Vec<KrillaGlyph> = run
        .glyphs
        .iter()
        .map(|g| {
            let glyph = KrillaGlyph::new(
                GlyphId::new(g.id),
                g.advance / size,
                (g.x - pen) / size,
                (g.y - run.baseline) / size,
                0.0,
                g.cluster.clone(),
                None,
            );
            pen += g.advance;
            glyph
        })
        .collect();
    surface.set_fill(Some(fill(run.color)));
    // Synthetic bold: stroke the outlines too, as fontique/swash embolden.
    let [r, g, b, _] = run.color.0;
    surface.set_stroke(run.embolden.then(|| Stroke {
        paint: rgb::Color::new(r, g, b).into(),
        width: size * TO_PT / 30.0,
        ..Stroke::default()
    }));
    // Synthetic italic: shear around the run's baseline.
    let skew = run.skew_deg.filter(|d| *d != 0.0).map(|d| {
        let t = d.to_radians().tan();
        Transform::from_row(1.0, 0.0, -t, 1.0, t * run.baseline * TO_PT, 0.0)
    });
    if let Some(t) = &skew {
        surface.push_transform(t);
    }
    surface.draw_glyphs(
        Point::from_xy(run.origin_x * TO_PT, run.baseline * TO_PT),
        &glyphs,
        font,
        &run.text,
        size * TO_PT,
        false,
    );
    if skew.is_some() {
        surface.pop();
    }
    surface.set_stroke(None);
    if let Some((offset, thick)) = run.underline {
        let y = run.baseline + offset;
        fill_rect(
            surface,
            [run.origin_x, y, run.advance, thick.max(0.5)],
            run.color,
        );
    }
}
