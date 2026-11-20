//! Kantor PDF: reading PDF files for the viewer (no GPUI).
//!
//! Everything goes through hayro (pure Rust; see
//! `docs/decisions/0003-pdf-renderer.md`): [`PdfDoc::open`] reads the file
//! (asking for a password when it is protected), [`Renderer`] rasterizes
//! pages, [`PdfDoc::text`] extracts a page's text with the place of every
//! character (search, selection, copy), [`PdfDoc::outline`] and
//! [`PdfDoc::links`] read bookmarks and link annotations.
//!
//! Positions are in *page space*: points, origin at the top left of the
//! page as it is shown (the page's own `/Rotate` and crop box applied, y
//! down), the space hayro renders at scale 1. A view rotation on top
//! ([`Turn`]) maps page space to the turned page with [`Turn::map`].
//!
//! hayro's types borrow from the open file and aren't `Send`: a viewer
//! opens the file once per worker thread (the bytes are shared, see
//! [`PdfDoc::open`]) and sends plain data ([`Raster`], [`PageText`]…) back.

mod nav;
mod render;
pub mod signatures;
pub mod signing;
mod text;

use std::sync::Arc;

use hayro::hayro_syntax::object::ObjectIdentifier;
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};

pub use nav::{Dest, Link, LinkTarget, OutlineItem};
pub use render::{MAX_SIDE, Raster, Renderer};
pub use text::{Match, PageText, find_in};

/// Why a file couldn't be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// The file is password-protected: `wrong` when a password was given
    /// and didn't open it.
    Password { wrong: bool },
    /// Encrypted with a method Kantor can't read.
    UnsupportedEncryption,
    /// Not a PDF, or damaged beyond repair.
    Damaged,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Password { wrong: false } => "the PDF is password-protected",
            Self::Password { wrong: true } => "the password is incorrect",
            Self::UnsupportedEncryption => "the PDF is encrypted in a way Kantor can't read",
            Self::Damaged => "the file is damaged and could not be repaired",
        })
    }
}

impl std::error::Error for OpenError {}

/// A view rotation in quarter turns clockwise (0–3), on top of the page's
/// own rotation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Turn(pub u8);

impl Turn {
    pub fn cw(self) -> Self {
        Self((self.0 + 1) % 4)
    }

    pub fn ccw(self) -> Self {
        Self((self.0 + 3) % 4)
    }

    /// Size of a `w` × `h` page turned.
    pub fn size(self, w: f32, h: f32) -> (f32, f32) {
        if self.0 % 2 == 1 { (h, w) } else { (w, h) }
    }

    /// A point of a `w` × `h` page (page space) on the turned page.
    pub fn map(self, w: f32, h: f32, x: f32, y: f32) -> (f32, f32) {
        match self.0 % 4 {
            1 => (h - y, x),
            2 => (w - x, h - y),
            3 => (y, w - x),
            _ => (x, y),
        }
    }

    /// The inverse of [`Self::map`]: a point of the turned page in page space.
    pub fn unmap(self, w: f32, h: f32, x: f32, y: f32) -> (f32, f32) {
        match self.0 % 4 {
            1 => (y, h - x),
            2 => (w - x, h - y),
            3 => (w - y, x),
            _ => (x, y),
        }
    }

    /// A page-space rectangle `[x0, y0, x1, y1]` on the turned page.
    pub fn map_rect(self, w: f32, h: f32, r: [f32; 4]) -> [f32; 4] {
        let (ax, ay) = self.map(w, h, r[0], r[1]);
        let (bx, by) = self.map(w, h, r[2], r[3]);
        [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]
    }

    /// The affine map of [`Self::map`] as `[a, b, c, d, e, f]` (kurbo order).
    fn coeffs(self, w: f64, h: f64) -> [f64; 6] {
        match self.0 % 4 {
            1 => [0.0, 1.0, -1.0, 0.0, h, 0.0],
            2 => [-1.0, 0.0, 0.0, -1.0, w, h],
            3 => [0.0, -1.0, 1.0, 0.0, 0.0, w],
            _ => [1.0, 0.0, 0.0, 1.0, 0.0, 0.0],
        }
    }
}

/// An open PDF file.
pub struct PdfDoc {
    pdf: Pdf,
    /// Each page's object, for destinations that name a page by reference.
    page_ids: Vec<Option<ObjectIdentifier>>,
}

impl PdfDoc {
    /// Read `data` (shared: every worker opens the same bytes), with
    /// `password` for a protected file (`""` when there is none yet).
    pub fn open(data: Arc<Vec<u8>>, password: &str) -> Result<Self, OpenError> {
        let pdf = Pdf::new_with_password(data, password).map_err(|e| match e {
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) => OpenError::Password {
                wrong: !password.is_empty(),
            },
            LoadPdfError::Decryption(DecryptionError::UnsupportedAlgorithm) => {
                OpenError::UnsupportedEncryption
            }
            LoadPdfError::Decryption(_) | LoadPdfError::Invalid => OpenError::Damaged,
        })?;
        if pdf.pages().is_empty() {
            return Err(OpenError::Damaged);
        }
        let page_ids = pdf.pages().iter().map(|p| p.raw().obj_id()).collect();
        Ok(Self { pdf, page_ids })
    }

    pub fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    /// Every page's size as shown (points, the page's own rotation applied).
    pub fn page_sizes(&self) -> Vec<(f32, f32)> {
        self.pdf
            .pages()
            .iter()
            .map(|p| {
                let (w, h) = p.render_dimensions();
                (w.max(1.0), h.max(1.0))
            })
            .collect()
    }

    /// The map from page `page`'s PDF user space (points, y up) to page
    /// space (y down, the page's crop box and rotation applied), as
    /// `[a, b, c, d, e, f]`.
    pub fn user_to_page(&self, page: usize) -> [f64; 6] {
        use hayro::hayro_interpret::TransformExt as _;
        self.pdf
            .pages()
            .get(page)
            .map_or([1.0, 0.0, 0.0, -1.0, 0.0, 792.0], |p| {
                p.initial_transform(true).to_kurbo().as_coeffs()
            })
    }

    /// The document's title from its information dictionary, if set.
    pub fn title(&self) -> Option<String> {
        let raw = self.pdf.metadata().title.as_deref()?;
        let title = nav::decode_text(raw);
        let title = title.trim();
        (!title.is_empty()).then(|| title.to_owned())
    }

    pub(crate) fn pdf(&self) -> &Pdf {
        &self.pdf
    }

    /// The index of the page with object `id`.
    pub(crate) fn page_index(&self, id: ObjectIdentifier) -> Option<usize> {
        self.page_ids.iter().position(|p| *p == Some(id))
    }
}

#[cfg(test)]
mod tests;
