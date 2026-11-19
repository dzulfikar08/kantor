//! Rasterizing pages.

use hayro::hayro_interpret::{InterpreterSettings, TransformExt as _};
use hayro::kurbo::Affine;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::{self, Pixmap, RenderContext};
use hayro::{RenderCache, RenderSettings, render_into};

use crate::{PdfDoc, Turn};

/// Largest side of a rendered page, pixels: keeps a zoomed-in page's
/// memory sane (a 6000 px square is 144 MB of RGBA).
pub const MAX_SIDE: f32 = 6000.0;

/// A rendered page: `width` × `height` pixels, RGBA, opaque (on white).
#[derive(Clone)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Raster {
    pub fn bytes(&self) -> usize {
        self.rgba.len()
    }
}

/// Renders the pages of one [`PdfDoc`], keeping fonts and decoded objects
/// between pages. Not `Send`: use it on the thread that opened the file.
pub struct Renderer<'a> {
    doc: &'a PdfDoc,
    cache: RenderCache<'a>,
    settings: InterpreterSettings,
}

impl<'a> Renderer<'a> {
    pub fn new(doc: &'a PdfDoc) -> Self {
        Self {
            doc,
            cache: RenderCache::new(),
            settings: InterpreterSettings::default(),
        }
    }

    /// Page `page` at `scale` pixels per point, turned by `turn`; the
    /// scale is lowered so no side passes [`MAX_SIDE`].
    pub fn render(&self, page: usize, scale: f32, turn: Turn) -> Option<Raster> {
        let p = self.doc.pdf().pages().get(page)?;
        let (w, h) = p.render_dimensions();
        let (w, h) = (w.max(1.0), h.max(1.0));
        let scale = scale.clamp(0.01, MAX_SIDE / w.max(h));
        let (tw, th) = turn.size(w, h);
        let px_w = (tw * scale).round().clamp(1.0, MAX_SIDE) as u16;
        let px_h = (th * scale).round().clamp(1.0, MAX_SIDE) as u16;
        let transform = Affine::scale(f64::from(scale))
            * Affine::new(turn.coeffs(f64::from(w), f64::from(h)))
            * p.initial_transform(true).to_kurbo();
        let mut ctx = RenderContext::new(px_w, px_h);
        render_into(
            p,
            &self.cache,
            &self.settings,
            &RenderSettings::default(),
            &mut ctx,
            transform,
        );
        ctx.flush();
        let mut pixmap = Pixmap::new(ctx.width(), ctx.height());
        ctx.render_with(
            &mut pixmap,
            &mut vello_cpu::Resources::default(),
            vello_cpu::RasterizerSettings {
                target_init: vello_cpu::TargetInit::Clear(WHITE),
                ..Default::default()
            },
        );
        Some(Raster {
            width: u32::from(pixmap.width()),
            height: u32::from(pixmap.height()),
            // Opaque (drawn on white): premultiplied and straight agree.
            rgba: pixmap.data_as_u8_slice().to_vec(),
        })
    }
}
