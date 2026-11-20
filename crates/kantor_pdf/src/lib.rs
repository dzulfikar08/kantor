//! PDF page tools for Kantor, without GPUI: merge, split, extract, delete,
//! reorder, rotate, info, images to PDF, text watermark, page numbers and
//! lossless compression, and batch actions on many files ([`batch`]).
//! `kantor pdf <tool>` is the command-line front end.
//!
//! Built on [`lopdf`]'s object model. Every operation works on a loaded
//! [`Pdf`] in memory; [`Pdf::to_bytes`] or [`Pdf::save`] writes the result.
//! Pages are numbered from 1 everywhere (see [`range`]).

pub mod annot;
#[cfg(test)]
mod annot_tests;
pub mod batch;
#[cfg(test)]
mod batch_tests;
mod compress;
mod edit;
pub mod form;
mod images;
mod info;
mod overlay;
mod pages;
mod pdfstr;
pub mod range;
pub mod sig;
pub mod sign;
pub mod table;
#[doc(hidden)]
pub mod testing;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::{Path, PathBuf};

use lopdf::{Document, LoadOptions};

pub use compress::{CompressReport, compress};
pub use edit::{Pick, assemble, blank_page, page_size, write_atomic};
pub use images::{ImageOptions, Orientation, PageSize, images_to_pdf};
pub use info::{Info, PageInfo, info};
pub use overlay::{PageNumberOptions, Position, WatermarkOptions, add_page_numbers, add_watermark};
pub use pages::{SplitPart, delete, every, extract, merge, reorder, rotate, split};
pub use pdfstr::{pdf_date, pdf_date_now, show_date};
pub use range::{PageRanges, RangeError};

/// What went wrong, worded for the person running the tool.
#[derive(Debug)]
pub enum Error {
    /// Reading or writing a file failed.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The bytes are not a PDF lopdf can read.
    NotPdf(String),
    /// The PDF is encrypted and no password was given.
    Encrypted,
    /// The password does not open the PDF.
    WrongPassword,
    /// A page range is malformed or outside the document.
    Range(RangeError),
    /// An option or input the operation can't use.
    Invalid(String),
    /// lopdf failed while editing or writing.
    Pdf(String),
    /// The PDF can't be encrypted again the way the original was.
    Encryption(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Error::NotPdf(e) => write!(f, "not a readable PDF ({e})"),
            Error::Encrypted => f.write_str(
                "the PDF is encrypted; pass --password (use --password '' if it opens \
                 without one). The result is written without encryption",
            ),
            Error::WrongPassword => f.write_str("the password does not open this PDF"),
            Error::Range(e) => e.fmt(f),
            Error::Invalid(e) => f.write_str(e),
            Error::Pdf(e) => write!(f, "PDF error: {e}"),
            Error::Encryption(e) => write!(f, "can't keep the password protection ({e})"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            Error::Range(e) => Some(e),
            _ => None,
        }
    }
}

impl From<RangeError> for Error {
    fn from(e: RangeError) -> Self {
        Error::Range(e)
    }
}

impl From<lopdf::Error> for Error {
    fn from(e: lopdf::Error) -> Self {
        Error::Pdf(e.to_string())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A PDF loaded into memory.
#[derive(Debug, Clone)]
pub struct Pdf {
    pub(crate) doc: Document,
    was_encrypted: bool,
}

/// True when the trailer names an `/Encrypt` dictionary, or lopdf removed one
/// while decrypting.
fn encrypted(doc: &Document) -> bool {
    doc.was_encrypted() || doc.trailer.has(b"Encrypt")
}

impl Pdf {
    /// Parse PDF bytes. An encrypted PDF needs `password` (which may be
    /// empty for files that open without one); the loaded copy is decrypted.
    pub fn from_bytes(bytes: &[u8], password: Option<&str>) -> Result<Pdf> {
        let options = match password {
            Some(p) => LoadOptions::with_password(p),
            None => LoadOptions::default(),
        };
        let mut doc = match Document::load_mem_with_options(bytes, options) {
            Ok(doc) => doc,
            Err(lopdf::Error::InvalidPassword) => return Err(Error::WrongPassword),
            Err(_) if password.is_none() && sniff_encrypt(bytes) => return Err(Error::Encrypted),
            Err(e) => return Err(Error::NotPdf(e.to_string())),
        };
        let was_encrypted = encrypted(&doc);
        if !was_encrypted {
            recover_unsized_streams(&mut doc, bytes);
        }
        if was_encrypted {
            if password.is_none() {
                return Err(Error::Encrypted);
            }
            // lopdf leaves /Encrypt in the trailer when no password fits.
            if doc.trailer.has(b"Encrypt") {
                return Err(Error::WrongPassword);
            }
        }
        let pdf = Pdf { doc, was_encrypted };
        if pdf.page_count() == 0 {
            return Err(Error::NotPdf("no pages found".into()));
        }
        Ok(pdf)
    }

    /// Read and parse a PDF file.
    pub fn open(path: &Path, password: Option<&str>) -> Result<Pdf> {
        let bytes = read(path)?;
        Pdf::from_bytes(&bytes, password)
    }

    /// The number of pages.
    pub fn page_count(&self) -> u32 {
        self.doc.get_pages().len() as u32
    }

    /// True when the source file was encrypted (this copy is decrypted, and
    /// saving writes it without encryption).
    pub fn was_encrypted(&self) -> bool {
        self.was_encrypted
    }

    /// The underlying lopdf document, for inspection.
    pub fn document(&self) -> &Document {
        &self.doc
    }

    /// Serialize the PDF.
    ///
    /// Usage rights (`/Perms /UR3`) are dropped: they don't survive a
    /// rewrite.
    pub fn to_bytes(&mut self) -> Result<Vec<u8>> {
        sig::drop_usage_rights(&mut self.doc);
        // Edits remove objects; the trailer's /Size follows the highest id left.
        if let Some(&(max, _)) = self.doc.objects.keys().next_back() {
            self.doc.max_id = max;
        }
        let mut out = Vec::new();
        self.doc
            .save_to(&mut out)
            .map_err(|e| Error::Pdf(e.to_string()))?;
        Ok(out)
    }

    /// Write the PDF to `path`.
    pub fn save(&mut self, path: &Path) -> Result<()> {
        let bytes = self.to_bytes()?;
        std::fs::write(path, bytes).map_err(|source| Error::Io {
            path: path.to_owned(),
            source,
        })
    }
}

/// How a password-protected PDF is protected, and what the password that
/// opened it allows (ISO 32000-2 §7.6.4.2, Table 22).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Protection {
    /// The owner password opened it (or the user password is the same):
    /// everything is allowed.
    pub owner: bool,
    /// The `/P` permissions the owner set for people with the user password.
    pub permissions: lopdf::Permissions,
    /// The security handler's `/V` and `/R`.
    pub version: i64,
    pub revision: i64,
}

impl Protection {
    /// Adding a signature is allowed: with the owner password always;
    /// with the user password when the owner allowed commenting and form
    /// filling, or (revision 3 and later) filling in existing fields,
    /// signature fields included, as Acrobat's "Signing: Allowed".
    pub fn allows_signing(&self) -> bool {
        use lopdf::Permissions as P;
        self.owner
            || self.permissions.contains(P::ANNOTABLE)
            || (self.revision >= 3 && self.permissions.contains(P::FILLABLE))
    }

    /// The security handler in words: "AES-256", "AES-128", "RC4 128-bit"…
    pub fn method(&self) -> &'static str {
        match (self.version, self.revision) {
            (5, _) => "AES-256",
            (4, _) => "AES-128",
            (1, _) | (_, 2) => "RC4 40-bit",
            _ => "RC4 128-bit",
        }
    }
}

impl Pdf {
    /// For a PDF that was encrypted: how, and what `password` (the one
    /// that opened it; `""` for none) allows. `None` for unencrypted files.
    pub fn protection(&self, password: &str) -> Option<Protection> {
        let state = self.doc.encryption_state.as_ref()?;
        // Check the password against the encryption dictionary the way the
        // file has it: a stand-in document with the trailer's /ID.
        let owner = state.encode().ok().is_some_and(|dict| {
            let mut probe = Document::with_version("1.7");
            probe.trailer = self.doc.trailer.clone();
            let id = probe.add_object(dict);
            probe.trailer.set("Encrypt", id);
            probe.authenticate_owner_password(password).is_ok()
        });
        Some(Protection {
            owner,
            permissions: state.permissions(),
            version: state.version(),
            revision: state.revision(),
        })
    }
}

/// Streams whose dictionary has no usable `/Length`: lopdf leaves them
/// empty, so a save would drop their data (a page's whole content). Take the
/// bytes up to `endstream`, as poppler and qpdf do.
fn recover_unsized_streams(doc: &mut Document, bytes: &[u8]) {
    let bad: Vec<lopdf::ObjectId> = doc
        .objects
        .iter()
        .filter_map(|(id, o)| {
            let s = o.as_stream().ok()?;
            if !s.content.is_empty() || s.start_position.is_none() {
                return None;
            }
            let sized = s
                .dict
                .get(b"Length")
                .and_then(|l| doc.dereference(l))
                .is_ok_and(|(_, l)| l.as_i64().is_ok() || l.as_f32().is_ok());
            (!sized).then_some(*id)
        })
        .collect();
    for id in bad {
        if let Ok(lopdf::Object::Stream(s)) = doc.get_object_mut(id)
            && let Some(data) = s.start_position.and_then(|at| stream_data(bytes, at))
        {
            s.set_content(data.to_vec());
            s.start_position = None;
        }
    }
}

/// The data of a stream starting at `start` in `bytes`, up to its
/// `endstream` (without the end-of-line before it); `None` when `endobj`
/// comes first.
fn stream_data(bytes: &[u8], start: usize) -> Option<&[u8]> {
    let rest = bytes.get(start..)?;
    let find = |pat: &[u8]| rest.windows(pat.len()).position(|w| w == pat);
    let end = find(b"endstream")?;
    if find(b"endobj").is_some_and(|o| o < end) {
        return None;
    }
    let data = &rest[..end];
    let data = data
        .strip_suffix(b"\r\n")
        .or_else(|| data.strip_suffix(b"\n"))
        .or_else(|| data.strip_suffix(b"\r"))
        .unwrap_or(data);
    Some(data)
}

/// A cheap check for `/Encrypt` in the file's trailer area, for files lopdf
/// refuses to load at all without the password.
fn sniff_encrypt(bytes: &[u8]) -> bool {
    let tail = &bytes[bytes.len().saturating_sub(4096)..];
    tail.windows(8).any(|w| w == b"/Encrypt")
}

/// Read a file, naming it in the error.
pub fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|source| Error::Io {
        path: path.to_owned(),
        source,
    })
}
