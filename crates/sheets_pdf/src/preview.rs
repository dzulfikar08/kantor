//! Print preview: a page drawn exactly as the PDF export draws it. The page
//! is written as a one-page PDF by the export's own [`Encoder`] and that PDF
//! is rasterized (hayro), so the preview can't drift from the file.
//!
//! [`Rasterizer`] keeps the font database between pages (building it scans
//! the system's fonts); run it off the UI thread.

use std::sync::Arc;

use anyhow::{Result, anyhow};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};

use crate::{Encoder, PageContent, Shaper};

/// Largest side of a rendered page, pixels (the rasterizer's limit is
/// 65 535; this keeps a zoomed-in page's memory sane).
pub const MAX_SIDE: f32 = 6000.0;

/// A rendered page: `width` × `height` pixels, RGBA, opaque.
#[derive(Clone)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Renders pages one at a time.
#[derive(Default)]
pub struct Rasterizer {
    shaper: Option<Shaper>,
}

impl Rasterizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// `page` at `px_per_pt` pixels per point (1.0 is 72 dpi), capped at
    /// [`MAX_SIDE`].
    pub fn render(&mut self, page: &PageContent, px_per_pt: f32) -> Result<Raster> {
        let pdf = self.one_page_pdf(page)?;
        let side = page.width.max(page.height).max(1.0);
        let scale = px_per_pt.clamp(0.05, MAX_SIDE / side);
        rasterize(pdf, scale)
    }

    /// `page` as a PDF of its own, written as the export writes it.
    pub fn one_page_pdf(&mut self, page: &PageContent) -> Result<Vec<u8>> {
        let shaper = self.shaper.take().unwrap_or_default();
        let mut enc = Encoder::new("Preview", shaper);
        enc.page(page);
        let (pdf, shaper) = enc.finish_keeping_shaper();
        self.shaper = Some(shaper);
        match pdf {
            Ok(pdf) => Ok(pdf),
            // A picture krilla can't decode: the export leaves it out.
            Err(e) => match e.downcast_ref::<crate::BadPicture>() {
                Some(bad) => {
                    let skip = std::collections::HashSet::from([bad.0]);
                    let shaper = self.shaper.take().unwrap_or_default();
                    let mut enc = Encoder::new("Preview", shaper).skip_pictures(&skip);
                    enc.page(page);
                    let (pdf, shaper) = enc.finish_keeping_shaper();
                    self.shaper = Some(shaper);
                    pdf
                }
                None => Err(e),
            },
        }
    }
}

/// The first page of `pdf` at `scale` pixels per point, on white.
pub fn rasterize(pdf: Vec<u8>, scale: f32) -> Result<Raster> {
    let pdf = Pdf::new(Arc::new(pdf)).map_err(|e| anyhow!("reading the preview PDF: {e:?}"))?;
    let page = pdf
        .pages()
        .first()
        .ok_or_else(|| anyhow!("the preview PDF has no page"))?;
    let cache = RenderCache::new();
    let pixmap = hayro::render(
        page,
        &cache,
        &InterpreterSettings::default(),
        &RenderSettings::default(),
        &PixmapSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
        },
    );
    let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    // Opaque (drawn on white): premultiplied and straight alpha agree.
    let rgba = pixmap.data_as_u8_slice().to_vec();
    Ok(Raster {
        width,
        height,
        rgba,
    })
}
