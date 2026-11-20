//! e-meterai: Peruri's electronic duty stamp (Bea Meterai Rp 10.000, UU
//! 10/2020), stamped by the provider on a box the user placed.

use crate::check::{self, Kept};
use crate::{Error, Info};

/// The value printed on today's e-meterai, in Rupiah.
pub const VALUE_RUPIAH: u32 = 10_000;

/// The meterai box Kantor proposes, in points (1/72 in): a square of about
/// 2.5 cm, as the stamp image is square. Peruri publishes no size rule;
/// public integrations use 1.6 to 2.8 cm (Mekari's default
/// 80 pt), and VIDA fixes the size itself (see
/// docs/decisions/0004-esign-providers.md).
pub const SIZE_PT: f32 = 70.0;
/// Smallest box Kantor accepts (1.6 cm): smaller, the stamp's QR code
/// can't be scanned from a print.
pub const SIZE_MIN_PT: f32 = 45.0;
/// Largest box Kantor accepts (3 cm).
pub const SIZE_MAX_PT: f32 = 85.0;

/// A document type (jenis dokumen) the provider accepts; Peruri's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DocType {
    /// The code the API takes.
    pub code: String,
    /// What it means (Indonesian, as the provider names it).
    pub name: String,
}

/// What to stamp and where.
#[derive(Clone, Debug, PartialEq)]
pub struct StampRequest {
    /// The file name the provider shows in its records.
    pub file_name: String,
    /// 0-based page.
    pub page: u32,
    /// The box in PDF user space: `[llx, lly, urx, ury]` from the page's
    /// bottom left, in points.
    pub rect: [f32; 4],
    /// Jenis dokumen: a [`DocType::code`].
    pub doc_type: String,
    /// Nomor dokumen (optional; some providers require it).
    pub doc_number: String,
    /// Tanggal dokumen, `YYYY-MM-DD` (default today).
    pub doc_date: String,
    /// Shown in the stamp's signature (optional).
    pub reason: String,
    pub location: String,
    /// The document's open password, when it has one.
    pub password: Option<String>,
}

impl StampRequest {
    pub fn new(file_name: &str, page: u32, rect: [f32; 4], doc_type: &str) -> StampRequest {
        StampRequest {
            file_name: file_name.to_string(),
            page,
            rect,
            doc_type: doc_type.to_string(),
            doc_number: String::new(),
            doc_date: today(),
            reason: String::new(),
            location: String::new(),
            password: None,
        }
    }
}

/// The stamped document.
#[derive(Clone, Debug, PartialEq)]
pub struct Stamped {
    pub pdf: Vec<u8>,
    /// The meterai's serial number, when the provider says.
    pub serial: Option<String>,
}

/// A provider that stamps e-meterai.
pub trait EMeteraiProvider: Send + Sync {
    fn info(&self) -> &'static Info;
    /// The document types it accepts.
    fn doc_types(&self) -> Result<Vec<DocType>, Error>;
    /// Meterai left on the account, when the API tells.
    fn balance(&self) -> Result<Option<u64>, Error>;
    /// Check the credentials (one cheap authenticated call).
    fn test(&self) -> Result<(), Error> {
        self.balance().map(|_| ())
    }
    /// Stamp `pdf`: send it as the API wants and return the provider's file.
    fn stamp(&self, pdf: &[u8], req: &StampRequest) -> Result<Stamped, Error>;
}

/// Problems with the box, before anything is sent.
pub fn check_box(pdf: &[u8], req: &StampRequest) -> Result<(), Error> {
    let doc = kantor_pdf::Pdf::from_bytes(pdf, req.password.as_deref())?;
    if req.page >= doc.page_count() {
        return Err(Error::Pdf(format!(
            "the document has {} pages; there is no page {}",
            doc.page_count(),
            req.page + 1
        )));
    }
    let [x0, y0, x1, y1] = req.rect;
    let (w, h) = (x1 - x0, y1 - y0);
    if !(w.is_finite() && h.is_finite()) || w < SIZE_MIN_PT - 0.5 || h < SIZE_MIN_PT - 0.5 {
        return Err(Error::Pdf(format!(
            "the e-meterai box is too small ({:.0} × {:.0} pt; at least {SIZE_MIN_PT:.0} pt)",
            w, h
        )));
    }
    if w > SIZE_MAX_PT + 0.5 || h > SIZE_MAX_PT + 0.5 {
        return Err(Error::Pdf(format!(
            "the e-meterai box is too large ({:.0} × {:.0} pt; at most {SIZE_MAX_PT:.0} pt)",
            w, h
        )));
    }
    if req.doc_type.trim().is_empty() {
        return Err(Error::Pdf("choose the document type".into()));
    }
    Ok(())
}

/// A box of the default size centred on `(x, y)` (user space).
pub fn box_at(x: f32, y: f32) -> [f32; 4] {
    let h = SIZE_PT / 2.0;
    [x - h, y - h, x + h, y + h]
}

/// Stamp and check the result: it must be a PDF, and earlier signatures
/// must still verify unless `allow_breaking` (the user confirmed the
/// warning).
pub fn stamp_checked(
    p: &dyn EMeteraiProvider,
    pdf: &[u8],
    req: &StampRequest,
    allow_breaking: bool,
) -> Result<(Stamped, Kept), Error> {
    check_box(pdf, req)?;
    let out = p.stamp(pdf, req)?;
    if !check::looks_like_pdf(&out.pdf) {
        return Err(Error::Provider(format!(
            "{} didn't send back a PDF: {}",
            p.info().name,
            crate::http::snippet(&out.pdf)
        )));
    }
    let kept = check::kept(pdf, &out.pdf);
    if let Kept::Broken(fields) = &kept
        && !allow_breaking
    {
        return Err(Error::BreaksSignatures(fields.clone()));
    }
    Ok((out, kept))
}

/// Today in WIB (UTC+7) as `YYYY-MM-DD`.
pub fn today() -> String {
    let t = pdf_engine::signatures::now() + 7 * 3600;
    let days = t.div_euclid(86_400);
    let (y, m, d) = civil(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since 1970-01-01 → (year, month, day).
pub(crate) fn civil_pub(z: i64) -> (i64, u32, u32) {
    civil(z)
}

/// Days since 1970-01-01 → (year, month, day) (Howard Hinnant's algorithm).
fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_412), (2025, 11, 20));
        assert_eq!(today().len(), 10);
    }
}
