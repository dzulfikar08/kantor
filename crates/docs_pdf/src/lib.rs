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
//! Pages follow their sections' sizes and carry their headers and footers
//! with the page numbers filled in ([`pages`]). What the PDF leaves out:
//! Kantor's own placeholders for content it can't draw (charts, shapes,
//! unsupported picture formats: the space stays empty).

pub mod blur;
pub mod build;
pub mod chart;
pub mod floats;
pub mod pages;
pub mod smartart;
pub mod vector;

pub use floats::FloatItem;
pub use pages::{Layout, Placed, StoryLayout, lay_out_with};

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

/// The model's object character as text.
const OBJECT_STR: &str = "\u{FFFC}";

/// CSS px (96 dpi, the layout's unit) to PDF points.
const TO_PT: f32 = 72.0 / 96.0;

/// One page's content in page coordinates (px), ready to encode.
#[derive(Clone, Debug, Default)]
pub struct PageScene {
    /// Page size, px.
    pub width: f32,
    pub height: f32,
    /// Filled rectangles (table borders), `[x, y, w, h]`.
    pub rules: Vec<([f32; 4], Rgba)>,
    pub pictures: Vec<Picture>,
    /// Inline pictures' outlines, drawn over them.
    pub outlines: Vec<floats::DrawPath>,
    pub runs: Vec<TextRun>,
    /// Floating objects behind the text and in front of it, back to
    /// front.
    pub behind: Vec<FloatScene>,
    pub front: Vec<FloatScene>,
}

/// A floating object: its outline paths and picture, then its text box's
/// content.
#[derive(Clone, Debug, Default)]
pub struct FloatScene {
    pub paths: Vec<floats::DrawPath>,
    pub pictures: Vec<Picture>,
    /// Drawing order: `(true, i)` is `pictures[i]`, `(false, i)` `paths[i]`.
    pub order: Vec<(bool, usize)>,
    pub content: PageScene,
    /// The text box's content is turned by this many degrees about this
    /// point (a rotated text box).
    pub content_turn: Option<(f32, [f32; 2])>,
}

/// A picture placed on a page.
#[derive(Clone, Debug)]
pub struct Picture {
    pub rect: [f32; 4],
    /// Drawn only inside this box (cropped pictures).
    pub clip: Option<[f32; 4]>,
    /// Cut to this shape (in the clip box before it's turned and flipped).
    pub shape: Option<Vec<floats::Seg>>,
    /// Turned clockwise (degrees) about the clip box's centre, flipped
    /// first (horizontally, vertically).
    pub turn: f32,
    pub flip: (bool, bool),
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

impl TextRun {
    /// A laid-out glyph run with the text its clusters index.
    pub fn from_glyph_run(run: &docs_layout::GlyphRun, text: Arc<str>) -> Self {
        Self {
            font: run.font.clone(),
            font_size: run.font_size,
            color: run.color,
            embolden: run.embolden,
            skew_deg: run.skew_deg,
            origin_x: run.origin_x,
            baseline: run.baseline,
            advance: run.advance,
            underline: run.underline,
            glyphs: run.glyphs.clone(),
            text,
        }
    }
}

/// A picture's format and file bytes.
type PictureData = (ImageFormat, Arc<[u8]>);

/// A picture's bytes as the PDF writer takes them: PNG and JPEG as they
/// are, WMF, EMF, PICT and BMP too (drawn by [`Images::picture`] when the
/// PDF is written, off the UI thread); `None` for what Kantor can't draw.
fn pdf_picture_data(bytes: Arc<[u8]>) -> Option<PictureData> {
    let format = ImageFormat::sniff(&bytes);
    (format != ImageFormat::Other || kantor_metafile::kind(&bytes).is_some())
        .then_some((format, bytes))
}

/// A whole document ready to encode: page size (px) and pages.
#[derive(Clone, Debug)]
pub struct Scene {
    pub width: f32,
    pub height: f32,
    pub title: String,
    pub pages: Vec<PageScene>,
}

/// Lay a document out on pages the way the Docs view does (without its
/// per-paragraph cache): sections, headers and footers.
pub fn lay_out(doc: &Document, engine: &mut Engine) -> Layout {
    lay_out_with(doc, &mut |p, w| Arc::new(engine.layout_paragraph(p, w)))
}

/// Where a laid-out document's text landed, for updating page references
/// in fields ([`docs_engine::Document::update_fields`]).
pub struct FieldPages {
    paged: PagedDocument,
    /// Per page: its number and that number in its section's format.
    numbers: Vec<(u32, String)>,
}

impl FieldPages {
    pub fn new(doc: &Document, laid: Layout) -> Self {
        let sections = doc.sections();
        let labels = pages::page_labels(doc, &sections, &laid.paged);
        let numbers = laid
            .paged
            .pages
            .iter()
            .zip(labels)
            .map(|(p, l)| (p.number, l))
            .collect();
        Self {
            paged: laid.paged,
            numbers,
        }
    }
}

impl docs_engine::PageLookup for FieldPages {
    fn page_of(&self, pos: docs_engine::Position) -> Option<usize> {
        self.paged
            .placed_line(pos.para, pos.offset)
            .map(|(page, _)| page)
    }

    fn page_number(&self, page: usize) -> u32 {
        self.numbers.get(page).map_or(1, |n| n.0)
    }

    fn page_label(&self, page: usize) -> String {
        self.numbers
            .get(page)
            .map_or_else(|| "1".into(), |n| n.1.clone())
    }
}

/// [`FieldPages`] for `doc`, laid out with `engine`.
pub fn field_pages(doc: &Document, engine: &mut Engine) -> Box<dyn docs_engine::PageLookup> {
    Box::new(FieldPages::new(doc, lay_out(doc, engine)))
}

/// Add what `paged`'s page `page` draws to `out`, moved by (`dx`, `dy`).
/// `paras` are the paragraphs `paged` was laid out from; pictures come
/// from `doc` (the document or the header's own part).
#[allow(clippy::too_many_arguments)]
fn add_page(
    out: &mut PageScene,
    doc: &Document,
    paras: &[docs_engine::Paragraph],
    paged: &PagedDocument,
    page: usize,
    (dx, dy): (f32, f32),
    pictures: &mut HashMap<(usize, String), Option<PictureData>>,
    doc_key: usize,
    painter: &floats::Painter,
) {
    let texts: Vec<Arc<str>> = paras.iter().map(|p| Arc::from(p.text.as_str())).collect();
    out.rules.extend(
        paged.pages[page]
            .rules
            .iter()
            .cloned()
            .chain(paged.tab_rules(page))
            .map(|r| {
                (
                    [r.rect[0] + dx, r.rect[1] + dy, r.rect[2], r.rect[3]],
                    r.color,
                )
            }),
    );
    for o in paged.objects(page) {
        if o.kind != ObjectKind::Image {
            continue;
        }
        let Some(Object::Image(img)) = paras.get(o.para).and_then(|p| p.object_at(o.index)) else {
            continue;
        };
        let found = pictures
            .entry((doc_key, img.rel_id.clone()))
            .or_insert_with(|| {
                let bytes = doc.image_bytes(&img.rel_id)?;
                pdf_picture_data(bytes)
            });
        if let Some((format, bytes)) = found {
            let [x, y, w, h] = o.whole;
            let [cx, cy, cw, ch] = o.rect;
            let whole = [x + dx, y + dy, w, h];
            // Cut to its shape, turned, outlined.
            let look = img
                .xml
                .as_deref()
                .and_then(|x| painter.inline_look(x, whole))
                .unwrap_or_default();
            out.pictures.push(Picture {
                rect: whole,
                // Cut at an exact line's top; a turned picture turns
                // about its box's centre.
                clip: if look.turned() {
                    Some(whole)
                } else {
                    (o.whole != o.rect).then_some([cx + dx, cy + dy, cw, ch])
                },
                shape: look.shape,
                turn: look.rot,
                flip: look.flip,
                format: *format,
                bytes: bytes.clone(),
            });
            out.outlines.extend(look.lines);
        }
    }
    let mut markers: HashMap<usize, Arc<str>> = HashMap::new();
    for run in paged.glyph_runs(page) {
        let text = match run.source {
            RunSource::Text => texts.get(run.para).cloned(),
            RunSource::Marker => Some(
                markers
                    .entry(run.para)
                    .or_insert_with(|| Arc::from(paged.run_text(&run, "")))
                    .clone(),
            ),
            // A note's number.
            RunSource::Mark(_) => Some(Arc::from(paged.run_text(&run, ""))),
            // Kantor's placeholder labels are not document content.
            RunSource::Label(_) => None,
        };
        let Some(text) = text else { continue };
        // The object character is laid out as a zero-width glyph holding
        // the picture's place; it isn't text to extract.
        let mut run = run;
        run.glyphs
            .retain(|g| text.get(g.cluster.clone()) != Some(OBJECT_STR));
        if run.glyphs.is_empty() {
            continue;
        }
        for g in &mut run.glyphs {
            g.x += dx;
            g.y += dy;
        }
        run.origin_x += dx;
        run.baseline += dy;
        out.runs.push(TextRun::from_glyph_run(&run, text));
    }
}

/// Collect what every page draws. `laid` must be `doc` laid out (as
/// [`lay_out`] or the Docs view does).
pub fn scene(doc: &Document, laid: &Layout) -> Scene {
    scene_of(
        doc,
        &laid.paged,
        &laid.furniture,
        &laid.floats,
        &laid.strikes,
    )
}

/// What a floating object draws in the PDF (placeholders are left out).
fn float_scene(
    doc: &Document,
    it: &FloatItem,
    painter: &floats::Painter,
    pictures: &mut HashMap<(usize, String), Option<PictureData>>,
) -> FloatScene {
    let mut out = FloatScene::default();
    let owner = it.doc(doc);
    let Some(f) = it.float(doc) else {
        return out;
    };
    let prims = match &it.chart {
        Some(c) => c.prims.clone(),
        None => painter.prims(f, it.rect),
    };
    for prim in prims {
        match prim {
            floats::FloatPrim::Path(p) => {
                out.order.push((false, out.paths.len()));
                out.paths.push(p);
            }
            floats::FloatPrim::Picture {
                rel_id,
                image,
                clip,
                rot,
                flip,
                shape,
            } => {
                let key = (it.story.map_or(0, |s| s + 1), rel_id.clone());
                let found = pictures.entry(key).or_insert_with(|| {
                    let bytes = owner.image_bytes(&rel_id)?;
                    pdf_picture_data(bytes)
                });
                if let Some((format, bytes)) = found {
                    out.order.push((true, out.pictures.len()));
                    out.pictures.push(Picture {
                        rect: image,
                        clip: Some(clip),
                        shape,
                        turn: rot,
                        flip,
                        format: *format,
                        bytes: bytes.clone(),
                    });
                }
            }
            floats::FloatPrim::Blurred(b) => {
                out.order.push((true, out.pictures.len()));
                out.pictures.push(Picture {
                    rect: b.rect,
                    clip: None,
                    shape: None,
                    turn: 0.0,
                    flip: (false, false),
                    format: ImageFormat::Png,
                    bytes: b.png.clone(),
                });
            }
            floats::FloatPrim::Placeholder(..) => {}
        }
    }
    if let Some((l, x, y)) = &it.text {
        let turn = floats::text_turn(f);
        if turn != 0.0 {
            let [rx, ry, rw, rh] = it.rect;
            out.content_turn = Some((turn, [rx + rw / 2.0, ry + rh / 2.0]));
        }
        add_page(
            &mut out.content,
            l.doc(doc),
            &l.paras,
            &l.paged,
            0,
            (*x, *y),
            pictures,
            l.key() + 1,
            painter,
        );
        out.content
            .rules
            .extend(l.strike_rules().into_iter().map(|r| {
                let [rx, ry, w, h] = r.rect;
                ([rx + x, ry + y, w, h], r.color)
            }));
    }
    out
}

/// [`scene`] from a layout's parts.
pub fn scene_of(
    doc: &Document,
    paged: &PagedDocument,
    furniture: &[Vec<Placed>],
    float_items: &[Vec<FloatItem>],
    strikes: &[docs_layout::Paragraph],
) -> Scene {
    let mut pictures = HashMap::new();
    let painter = &floats::Painter::new(doc);
    let pages = (0..paged.pages.len())
        .map(|page| {
            let setup = paged.page_setup(page);
            let mut out = PageScene {
                width: setup.width,
                height: setup.height,
                ..PageScene::default()
            };
            for f in furniture.get(page).into_iter().flatten() {
                add_page(
                    &mut out,
                    f.layout.doc(doc),
                    &f.layout.paras,
                    &f.layout.paged,
                    0,
                    (f.x, f.y),
                    &mut pictures,
                    f.layout.key() + 1,
                    painter,
                );
                out.rules
                    .extend(f.layout.strike_rules().into_iter().map(|r| {
                        let [x, y, w, h] = r.rect;
                        ([x + f.x, y + f.y, w, h], r.color)
                    }));
            }
            add_page(
                &mut out,
                doc,
                doc.paragraphs(),
                paged,
                page,
                (0.0, 0.0),
                &mut pictures,
                0,
                painter,
            );
            if !strikes.is_empty() {
                out.rules.extend(
                    paged
                        .strike_rules(page, strikes)
                        .into_iter()
                        .map(|r| (r.rect, r.color)),
                );
            }
            for it in float_items.get(page).into_iter().flatten() {
                let mut scenes = vec![float_scene(doc, it, painter, &mut pictures)];
                // A chart's labels, each turned on its own.
                for l in it.chart.iter().flat_map(|c| &c.labels) {
                    let mut fs = FloatScene::default();
                    add_page(
                        &mut fs.content,
                        l.layout.doc(doc),
                        &l.layout.paras,
                        &l.layout.paged,
                        0,
                        (l.x, l.y),
                        &mut pictures,
                        l.layout.key() + 1,
                        painter,
                    );
                    if l.rot != 0.0 {
                        let [x, y, w, h] = l.rect;
                        fs.content_turn = Some((l.rot, [x + w / 2.0, y + h / 2.0]));
                    }
                    scenes.push(fs);
                }
                // The text of a group's shapes, each turned with its shape.
                for b in &it.boxes {
                    let mut fs = FloatScene::default();
                    add_page(
                        &mut fs.content,
                        b.layout.doc(doc),
                        &b.layout.paras,
                        &b.layout.paged,
                        0,
                        (b.x, b.y),
                        &mut pictures,
                        b.layout.key() + 1,
                        painter,
                    );
                    fs.content
                        .rules
                        .extend(b.layout.strike_rules().into_iter().map(|r| {
                            let [rx, ry, w, h] = r.rect;
                            ([rx + b.x, ry + b.y, w, h], r.color)
                        }));
                    if b.turn != 0.0 {
                        let [x, y, w, h] = b.rect;
                        fs.content_turn = Some((b.turn, [x + w / 2.0, y + h / 2.0]));
                    }
                    scenes.push(fs);
                }
                if it.behind {
                    out.behind.extend(scenes);
                } else {
                    out.front.extend(scenes);
                }
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
    let laid = lay_out(doc, engine);
    write(&scene(doc, &laid))
}

/// Encode a scene as PDF. Pictures krilla can't decode are left out (with
/// a warning) rather than failing the export.
pub fn write(scene: &Scene) -> Result<Vec<u8>> {
    encode_retrying(|images| encode(scene, images))
}

/// Pictures embedded so far (by the address of their bytes), and those
/// krilla failed to decode on an earlier attempt.
#[derive(Default)]
pub struct Images {
    skip: HashSet<*const u8>,
    made: HashMap<*const u8, Option<Image>>,
    vectors: HashMap<*const u8, Option<Arc<kantor_metafile::usvg::Tree>>>,
}

/// A picture ready to draw into a PDF.
#[derive(Clone)]
pub enum PdfPicture {
    Raster(Image),
    /// A metafile (WMF, EMF, PICT) drawn as vector graphics.
    Vector(Arc<kantor_metafile::usvg::Tree>),
}

impl PdfPicture {
    /// Draw it stretched to `size` (pt) at the current origin.
    pub fn draw(self, surface: &mut Surface, size: Size) {
        match self {
            Self::Raster(image) => surface.draw_image(image, size),
            Self::Vector(tree) => vector::draw_tree(surface, &tree, size),
        }
    }
}

impl Images {
    /// No pictures yet; those at the addresses in `skip` are left out (they
    /// failed on an earlier attempt).
    pub fn skipping(skip: HashSet<*const u8>) -> Self {
        Self {
            skip,
            made: HashMap::new(),
            vectors: HashMap::new(),
        }
    }

    /// The picture in `data`: WMF, EMF and PICT as vector graphics (as a
    /// bitmap when they can't be read that way), the rest as [`Images::get`].
    pub fn picture(&mut self, data: Arc<dyn AsRef<[u8]> + Send + Sync>) -> Option<PdfPicture> {
        let bytes: &[u8] = (*data).as_ref();
        let key = bytes.as_ptr();
        if self.skip.contains(&key) {
            return None;
        }
        if matches!(
            kantor_metafile::kind(bytes),
            Some(
                kantor_metafile::Kind::Wmf
                    | kantor_metafile::Kind::Emf
                    | kantor_metafile::Kind::Pict
            )
        ) {
            let tree = self
                .vectors
                .entry(key)
                .or_insert_with(|| {
                    let pic = kantor_metafile::render(bytes).filter(|p| !p.is_empty())?;
                    pic.tree().map(Arc::new)
                })
                .clone();
            if let Some(tree) = tree {
                return Some(PdfPicture::Vector(tree));
            }
        }
        self.get(data).map(PdfPicture::Raster)
    }

    /// The address of the bytes `image` was made from (to skip it next
    /// time, see [`Images::skipping`]).
    pub fn key_of(&self, image: &Image) -> Option<*const u8> {
        self.made
            .iter()
            .find(|(_, i)| i.as_ref() == Some(image))
            .map(|(k, _)| *k)
    }

    /// The picture in `data` (PNG, JPEG, GIF or WebP, by signature), once
    /// per distinct buffer; `None` for other formats and pictures that
    /// failed before.
    pub fn get(&mut self, data: Arc<dyn AsRef<[u8]> + Send + Sync>) -> Option<Image> {
        let bytes: &[u8] = (*data).as_ref();
        let key = bytes.as_ptr();
        if self.skip.contains(&key) {
            return None;
        }
        let kind = picture_kind(bytes);
        self.made
            .entry(key)
            .or_insert_with(|| {
                let made = match kind {
                    Some("png") => Image::from_png(data.into(), true),
                    Some("jpeg") => Image::from_jpeg(data.into(), true),
                    Some("gif") => Image::from_gif(data.into(), true),
                    Some("webp") => Image::from_webp(data.into(), true),
                    // WMF, EMF, PICT and BMP are drawn as PNG.
                    _ => match kantor_metafile::to_png((*data).as_ref(), 3000) {
                        Some(png) => Image::from_png(png.into(), true),
                        None => Err("not PNG, JPEG, GIF, WebP, WMF, EMF, PICT or BMP".into()),
                    },
                };
                made.map_err(|e| log::warn!("PDF export: picture left out: {e}"))
                    .ok()
            })
            .clone()
    }
}

/// `png`, `jpeg`, `gif` or `webp` from a file's signature.
pub fn picture_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG") {
        Some("png")
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        Some("jpeg")
    } else if bytes.starts_with(b"GIF8") {
        Some("gif")
    } else if bytes.len() > 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// Run `encode` until krilla stops failing on a picture: a picture it
/// can't decode is reported, skipped, and the document encoded again.
pub fn encode_retrying(
    mut encode: impl FnMut(&mut Images) -> Result<Vec<u8>, KrillaError>,
) -> Result<Vec<u8>> {
    let mut skip: HashSet<*const u8> = HashSet::new();
    loop {
        let mut images = Images::skipping(skip.clone());
        match encode(&mut images) {
            Ok(bytes) => return Ok(bytes),
            Err(KrillaError::Image(bad, _, why)) => {
                let key = images
                    .key_of(&bad)
                    .ok_or_else(|| anyhow!("writing PDF: picture: {why}"))?;
                log::warn!("PDF export: leaving out a picture krilla can't decode: {why}");
                skip.insert(key);
            }
            Err(e) => return Err(anyhow!("writing PDF: {e}")),
        }
    }
}

/// Fonts embedded so far, by font data and index.
#[derive(Default)]
pub struct Fonts(HashMap<(u64, u32), Option<Font>>);

impl Fonts {
    /// The krilla font for `font`; `None` (with a warning, once) when
    /// krilla can't read it.
    pub fn get(&mut self, font: &FontData) -> Option<Font> {
        self.0
            .entry((font.data.id(), font.index))
            .or_insert_with(|| {
                let (data, _) = font.data.clone().into_raw_parts();
                let made = Font::new(data.into(), font.index);
                if made.is_none() {
                    log::warn!("PDF export: a font krilla can't read; its text is left out");
                }
                made
            })
            .clone()
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

/// A solid fill of `c` (with its opacity).
pub fn fill(c: Rgba) -> Fill {
    let [r, g, b, a] = c.0;
    Fill {
        paint: rgb::Color::new(r, g, b).into(),
        opacity: NormalizedF32::new(a as f32 / 255.0).unwrap_or(NormalizedF32::ONE),
        rule: Default::default(),
    }
}

/// A gradient fill (page px) as a PDF shading.
fn gradient_fill(g: &slides_scene::Paint) -> Fill {
    use krilla::paint::{LinearGradient, RadialGradient, SpreadMethod, Stop};
    let stops = |s: &[slides_scene::Stop]| -> Vec<Stop> {
        s.iter()
            .map(|s| {
                let [r, g, b, a] = s.color.0;
                Stop {
                    offset: NormalizedF32::new(s.offset.clamp(0.0, 1.0))
                        .unwrap_or(NormalizedF32::ZERO),
                    color: rgb::Color::new(r, g, b).into(),
                    opacity: NormalizedF32::new(a as f32 / 255.0).unwrap_or(NormalizedF32::ONE),
                }
            })
            .collect()
    };
    match g {
        slides_scene::Paint::Solid(c) => fill(*c),
        // Drawn by `fill_pattern`; the mixed colour where it can't be.
        p @ slides_scene::Paint::Pattern { .. } => fill(p.main_color()),
        slides_scene::Paint::Linear { from, to, stops: s } => Fill {
            paint: LinearGradient {
                x1: from.x * TO_PT,
                y1: from.y * TO_PT,
                x2: to.x * TO_PT,
                y2: to.y * TO_PT,
                transform: Transform::identity(),
                spread_method: SpreadMethod::Pad,
                stops: stops(s),
                anti_alias: true,
            }
            .into(),
            opacity: NormalizedF32::ONE,
            rule: Default::default(),
        },
        slides_scene::Paint::Radial {
            center,
            rx,
            ry,
            stops: s,
        } => {
            let (cx, cy, rx) = (center.x * TO_PT, center.y * TO_PT, (rx * TO_PT).max(1e-3));
            // A circle of radius rx, squeezed to ry vertically.
            let k = ry * TO_PT / rx;
            Fill {
                paint: RadialGradient {
                    fx: cx,
                    fy: cy,
                    fr: 0.0,
                    cx,
                    cy,
                    cr: rx,
                    transform: Transform::from_row(1.0, 0.0, 0.0, k, 0.0, cy * (1.0 - k)),
                    spread_method: SpreadMethod::Pad,
                    stops: stops(s),
                    anti_alias: true,
                }
                .into(),
                opacity: NormalizedF32::ONE,
                rule: Default::default(),
            }
        }
    }
}

/// Fill `path` with a DrawingML pattern: its background colour, then the
/// foreground bits as a PDF tiling pattern. `origin` and `cell` are in
/// the surface's current units (the pattern turns and scales with them).
#[allow(clippy::too_many_arguments)]
pub fn fill_pattern(
    surface: &mut Surface,
    path: &krilla::geom::Path,
    rule: krilla::paint::FillRule,
    fg: Rgba,
    bg: Rgba,
    bits: &[u8; 8],
    origin: (f32, f32),
    cell: f32,
) {
    let cell = cell.max(0.01);
    let colour = |c: Rgba| {
        let [r, g, b, a] = c.0;
        (
            rgb::Color::new(r, g, b),
            NormalizedF32::new(a as f32 / 255.0).unwrap_or(NormalizedF32::ONE),
        )
    };
    surface.set_stroke(None);
    if bg.0[3] > 0 {
        let (c, opacity) = colour(bg);
        surface.set_fill(Some(Fill {
            paint: c.into(),
            opacity,
            rule,
        }));
        surface.draw_path(path);
    }
    if fg.0[3] > 0 && bits.iter().any(|b| *b != 0) {
        let (c, opacity) = colour(fg);
        let stream = {
            let mut sb = surface.stream_builder();
            let mut tile = sb.surface();
            let mut pb = PathBuilder::new();
            for (y, row) in bits.iter().enumerate() {
                let mut x = 0;
                while x < 8 {
                    if row >> x & 1 == 0 {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < 8 && row >> x & 1 == 1 {
                        x += 1;
                    }
                    if let Some(r) = Rect::from_xywh(
                        start as f32 * cell,
                        y as f32 * cell,
                        (x - start) as f32 * cell,
                        cell,
                    ) {
                        pb.push_rect(r);
                    }
                }
            }
            if let Some(tp) = pb.finish() {
                tile.set_fill(Some(Fill {
                    paint: c.into(),
                    opacity: NormalizedF32::ONE,
                    rule: Default::default(),
                }));
                tile.draw_path(&tp);
            }
            tile.finish();
            sb.finish()
        };
        surface.set_fill(Some(Fill {
            paint: krilla::paint::Pattern {
                stream,
                transform: Transform::from_translate(origin.0, origin.1),
                width: 8.0 * cell,
                height: 8.0 * cell,
            }
            .into(),
            opacity,
            rule,
        }));
        surface.draw_path(path);
    }
    surface.set_fill(None);
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

fn encode(scene: &Scene, images: &mut Images) -> Result<Vec<u8>, KrillaError> {
    let mut fonts = Fonts::default();
    let mut pdf = krilla::Document::new();
    pdf.set_metadata(metadata(&scene.title));
    let settings = |c: &PageScene| {
        // Pages carry their own size (sections); 0 = the scene's.
        let (w, h) = if c.width > 0.0 && c.height > 0.0 {
            (c.width, c.height)
        } else {
            (scene.width, scene.height)
        };
        PageSettings::from_wh((w * TO_PT).max(1.0), (h * TO_PT).max(1.0))
            .expect("positive page size")
    };
    let blank = [PageScene::default()];
    let pages = if scene.pages.is_empty() {
        &blank[..]
    } else {
        &scene.pages[..]
    };
    for content in pages {
        let mut page = pdf.start_page_with(settings(content));
        let mut surface = page.surface();
        for f in &content.behind {
            draw_float(&mut surface, f, images, &mut fonts);
        }
        draw_content(&mut surface, content, images, &mut fonts);
        for f in &content.front {
            draw_float(&mut surface, f, images, &mut fonts);
        }
        surface.finish();
        page.finish();
    }
    pdf.finish()
}

/// A page's (or text box's) rules, pictures and text.
fn draw_content(
    surface: &mut Surface,
    content: &PageScene,
    images: &mut Images,
    fonts: &mut Fonts,
) {
    for (rect, color) in &content.rules {
        fill_rect(surface, *rect, *color);
    }
    for pic in &content.pictures {
        draw_picture(surface, pic, images);
    }
    for p in &content.outlines {
        draw_path(surface, p);
    }
    for run in &content.runs {
        let Some(font) = fonts.get(&run.font) else {
            continue;
        };
        draw_run(surface, run, font);
    }
}

/// Scale by `(sx, sy)` then turn `deg` clockwise, both about `m` (pt).
fn turn_about(deg: f32, (sx, sy): (f32, f32), (mx, my): (f32, f32)) -> Transform {
    let (sin, cos) = deg.to_radians().sin_cos();
    let (a, c) = (cos * sx, -sin * sy);
    let (b, d) = (sin * sx, cos * sy);
    Transform::from_row(a, b, c, d, mx - (a * mx + c * my), my - (b * mx + d * my))
}

fn draw_picture(surface: &mut Surface, pic: &Picture, images: &mut Images) {
    let data: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(pic.bytes.clone());
    let image = images.picture(data);
    let [x, y, w, h] = pic.rect;
    let (Some(image), Some(size)) = (image, Size::from_wh(w * TO_PT, h * TO_PT)) else {
        return;
    };
    let shape = pic.shape.as_deref().and_then(krilla_path);
    let clip = pic.clip.and_then(|[cx, cy, cw, ch]| {
        let rect = Rect::from_xywh(cx * TO_PT, cy * TO_PT, cw * TO_PT, ch * TO_PT)?;
        let mut pb = PathBuilder::new();
        pb.push_rect(rect);
        pb.finish()
    });
    // Turned and flipped about the box's centre.
    let turned = pic.turn != 0.0 || pic.flip.0 || pic.flip.1;
    if turned {
        let [cx, cy, cw, ch] = pic.clip.unwrap_or(pic.rect);
        let (mx, my) = ((cx + cw / 2.0) * TO_PT, (cy + ch / 2.0) * TO_PT);
        let (sx, sy) = (
            if pic.flip.0 { -1.0 } else { 1.0 },
            if pic.flip.1 { -1.0 } else { 1.0 },
        );
        surface.push_transform(&turn_about(pic.turn, (sx, sy), (mx, my)));
    }
    // Cut to its box (crop, an exact line's top) and its shape.
    let clips: Vec<_> = clip.iter().chain(shape.iter()).collect();
    for c in &clips {
        surface.push_clip_path(c, &krilla::paint::FillRule::NonZero);
    }
    surface.push_transform(&Transform::from_translate(x * TO_PT, y * TO_PT));
    image.draw(surface, size);
    surface.pop();
    for _ in &clips {
        surface.pop();
    }
    if turned {
        surface.pop();
    }
}

/// A filled and/or stroked outline.
fn draw_path(surface: &mut Surface, p: &floats::DrawPath) {
    // Nothing to paint (krilla fills a path drawn with neither in black).
    if p.fill.is_none() && p.gradient.is_none() && p.stroke.is_none() {
        return;
    }
    let Some(path) = krilla_path(&p.segs) else {
        return;
    };

    if let Some(slides_scene::Paint::Pattern {
        fg,
        bg,
        bits,
        origin,
        cell,
    }) = &p.gradient
    {
        fill_pattern(
            surface,
            &path,
            Default::default(),
            *fg,
            *bg,
            bits,
            (origin.x * TO_PT, origin.y * TO_PT),
            cell * TO_PT,
        );
    }
    let patterned = matches!(p.gradient, Some(slides_scene::Paint::Pattern { .. }));
    surface.set_fill(match &p.gradient {
        _ if patterned => None,
        Some(g) => Some(gradient_fill(g)),
        None => p.fill.map(fill),
    });
    surface.set_stroke(p.stroke.as_ref().map(|(c, w, dash)| {
        let [r, g, b, a] = c.0;
        Stroke {
            paint: rgb::Color::new(r, g, b).into(),
            width: w * TO_PT,
            opacity: NormalizedF32::new(a as f32 / 255.0).unwrap_or(NormalizedF32::ONE),
            dash: dash.as_ref().map(|d| krilla::paint::StrokeDash {
                array: d.iter().map(|v| v * TO_PT).collect(),
                offset: 0.0,
            }),
            ..Stroke::default()
        }
    }));
    surface.draw_path(&path);
    surface.set_fill(None);
    surface.set_stroke(None);
}

/// A floating object: its outline's fill and stroke, its picture, its
/// text box's content.
fn draw_float(surface: &mut Surface, f: &FloatScene, images: &mut Images, fonts: &mut Fonts) {
    for &(is_pic, i) in &f.order {
        if is_pic {
            if let Some(pic) = f.pictures.get(i) {
                draw_picture(surface, pic, images);
            }
            continue;
        }
        if let Some(p) = f.paths.get(i) {
            draw_path(surface, p);
        }
    }
    if let Some((deg, [cx, cy])) = f.content_turn {
        let (mx, my) = (cx * TO_PT, cy * TO_PT);
        surface.push_transform(&turn_about(deg, (1.0, 1.0), (mx, my)));
        draw_content(surface, &f.content, images, fonts);
        surface.pop();
    } else {
        draw_content(surface, &f.content, images, fonts);
    }
}

fn krilla_path(segs: &[floats::Seg]) -> Option<krilla::geom::Path> {
    let mut pb = PathBuilder::new();
    let p = |q: [f32; 2]| (q[0] * TO_PT, q[1] * TO_PT);
    for s in segs {
        match *s {
            floats::Seg::Move(a) => {
                let (x, y) = p(a);
                pb.move_to(x, y);
            }
            floats::Seg::Line(a) => {
                let (x, y) = p(a);
                pb.line_to(x, y);
            }
            floats::Seg::Quad(c, a) => {
                let ((cx, cy), (x, y)) = (p(c), p(a));
                pb.quad_to(cx, cy, x, y);
            }
            floats::Seg::Cubic(c1, c2, a) => {
                let ((ax, ay), (bx, by), (x, y)) = (p(c1), p(c2), p(a));
                pb.cubic_to(ax, ay, bx, by, x, y);
            }
            floats::Seg::Close => pb.close(),
        }
    }
    pb.finish()
}

/// Document metadata: title, and Kantor as creator and producer.
pub fn metadata(title: &str) -> Metadata {
    Metadata::new()
        .title(title.to_owned())
        .creator("Kantor".into())
        .producer(format!("Kantor {}", env!("CARGO_PKG_VERSION")))
}

/// Draw a glyph run given in px (96 dpi) on a surface in points: krilla
/// writes the glyphs with their text, so the PDF's text can be copied.
pub fn draw_run(surface: &mut Surface, run: &TextRun, font: Font) {
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
