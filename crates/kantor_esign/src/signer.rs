//! Certified signatures by a licensed PSrE (Penyelenggara Sertifikasi
//! Elektronik): the signer's key and certificate stay with the provider,
//! which verified the signer (e-KYC) when they registered.
//!
//! Three flows, as the provider's API allows:
//! - remote key: the provider signs a hash with the signer's key and gives
//!   the certificate chain; Kantor builds the whole PAdES signature with
//!   its own code ([`RemoteSigner::identity`], like Sign with Certificate
//!   with the key elsewhere). The document never leaves the computer.
//! - hash to CMS: Kantor writes the signature field and `/ByteRange`,
//!   sends the digest, and puts the provider's CMS in
//!   ([`pdf_engine::signing::sign_external`]).
//! - document upload: the provider gets the PDF, signs it and Kantor
//!   downloads the signed file.

use kantor_pdf::sign::Place;
use pdf_engine::signing::{self, ExternalSigner, Identity};

use crate::check::{self, Kept};
use crate::{Error, Info};

/// Who signs, as the provider knows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signer {
    /// The signer's account at the provider: e-mail, phone or user id,
    /// depending on the provider.
    pub id: String,
    /// The one-time password the provider sent, when its flow uses one.
    pub otp: Option<String>,
}

/// Where and how the signature shows.
#[derive(Clone, Debug, PartialEq)]
pub struct SignRequest {
    pub file_name: String,
    pub place: Place,
    pub reason: String,
    pub location: String,
    /// The page's `/Rotate` (for a new visible signature).
    pub rotate: i64,
}

impl SignRequest {
    pub fn new(file_name: &str, place: Place) -> SignRequest {
        SignRequest {
            file_name: file_name.to_string(),
            place,
            reason: String::new(),
            location: String::new(),
            rotate: 0,
        }
    }
}

/// A PSrE's signing API.
pub trait RemoteSigner: Send + Sync {
    fn info(&self) -> &'static Info;
    /// Check the credentials (one cheap authenticated call).
    fn test(&self) -> Result<(), Error>;
    /// Ask the provider to send the signer a one-time password, when its
    /// flow needs one: where it went (as the provider shows it), or `None`
    /// when no OTP is needed.
    fn request_otp(&self, _signer: &Signer) -> Result<Option<String>, Error> {
        Ok(None)
    }
    /// The signer's certificate with a key that signs at the provider,
    /// when the API signs raw hashes.
    fn identity(&self, _signer: &Signer) -> Option<Result<Identity, Error>> {
        None
    }
    /// The hash-based signer for `signer`, when the API returns a CMS.
    fn hash_signer<'a>(&'a self, _signer: &'a Signer) -> Option<Box<dyn ExternalSigner + 'a>> {
        None
    }
    /// The document-upload flow: the signed PDF.
    fn sign_document(
        &self,
        pdf: &[u8],
        req: &SignRequest,
        signer: &Signer,
    ) -> Result<Vec<u8>, Error>;
}

/// Sign `pdf`: hash-based when the provider can, else by upload; then check
/// earlier signatures survived (unless `allow_breaking`).
pub fn sign(
    p: &dyn RemoteSigner,
    pdf: &[u8],
    req: &SignRequest,
    signer: &Signer,
    allow_breaking: bool,
) -> Result<(Vec<u8>, Kept), Error> {
    let mut opts = signing::Options::new(req.place.clone());
    opts.reason = Some(req.reason.clone()).filter(|r| !r.is_empty());
    opts.location = Some(req.location.clone()).filter(|r| !r.is_empty());
    opts.rotate = req.rotate;
    let look = |name: &str, opts: &signing::Options<'_>| {
        matches!(req.place, Place::New { .. }).then(|| kantor_pdf::sign::Look {
            lines: signing::look_lines(
                name,
                opts.now,
                opts.reason.as_deref(),
                opts.location.as_deref(),
                &signing::LookText::default(),
            ),
            art: None,
        })
    };
    let map = |e: signing::Error| match e {
        signing::Error::Key(m) | signing::Error::Pdf(m) => Error::Provider(m),
        other => Error::Provider(other.to_string()),
    };
    let out = if let Some(id) = p.identity(signer) {
        let id = id?;
        opts.look = look(&id.name(), &opts);
        signing::sign(pdf, &id, &opts).map_err(map)?
    } else {
        match p.hash_signer(signer) {
            Some(ext) => {
                let name = ext.name().unwrap_or_else(|| signer.id.clone());
                opts.look = look(&name, &opts);
                signing::sign_external(pdf, ext.as_ref(), &opts).map_err(map)?
            }
            None => p.sign_document(pdf, req, signer)?,
        }
    };
    if !check::looks_like_pdf(&out) {
        return Err(Error::Provider(format!(
            "{} didn't send back a PDF: {}",
            p.info().name,
            crate::http::snippet(&out)
        )));
    }
    let kept = check::kept(pdf, &out);
    if let Kept::Broken(f) = &kept
        && !allow_breaking
    {
        return Err(Error::BreaksSignatures(f.clone()));
    }
    Ok((out, kept))
}
