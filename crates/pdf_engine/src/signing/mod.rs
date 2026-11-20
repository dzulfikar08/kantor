//! Signing PDFs with a certificate: PAdES baseline signatures (ETSI EN
//! 319 142-1) as Acrobat's "Use a certificate" makes them.
//!
//! - B-B: a detached CMS (`/SubFilter /ETSI.CAdES.detached`) with the
//!   ESS signing-certificate-v2 attribute, optionally certifying (DocMDP).
//! - B-T: plus an RFC 3161 signature time stamp from a TSA.
//! - B-LT: plus the certificates, OCSP responses and CRLs in the DSS.
//! - B-LTA: plus a document time stamp over all of it.
//!
//! Keys come from PKCS #12 files ([`p12`]), PKCS #11 tokens and smart
//! cards, or the system's certificate store on macOS and Windows
//! ([`stores`], through the kantor_keystore crate). The network is used only
//! through the caller's [`Fetch`] (time stamps, revocation data), so
//! nothing goes online unless the user allowed it. Each step is an
//! incremental update: earlier signatures stay valid.

pub mod asn1;
pub mod cms;
pub mod key;
pub mod p12;
pub mod stores;
pub mod tsa;

#[cfg(test)]
mod tests;

use std::fmt;
use std::sync::{Arc, Mutex};

use kantor_pdf::sign::{self as pdfsign, Dss, Look, Place, Request, SigType};

use crate::signatures::chain::{Context, Purpose};
use crate::signatures::cms::SignedData;
use crate::signatures::x509::Cert;
use crate::signatures::{Fetch, TrustStore};

pub use key::{Hash, KeyType, SigningKey, SoftKey};

/// Why signing failed.
#[derive(Debug)]
pub enum Error {
    /// The PKCS #12 password (or token PIN) is wrong.
    WrongPassword,
    /// The token is locked after too many wrong PINs.
    Locked,
    /// Something Kantor doesn't support (the text says what).
    Unsupported(String),
    /// The key or certificate can't be used.
    Key(String),
    /// The time-stamp server failed.
    Tsa(String),
    /// A time stamp or LTV data was asked for while offline.
    Offline,
    /// The PDF can't be signed.
    Pdf(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::WrongPassword => f.write_str("the password or PIN is wrong"),
            Error::Locked => f.write_str("the token is locked (too many wrong PINs)"),
            Error::Unsupported(w) => write!(f, "not supported: {w}"),
            Error::Key(e) | Error::Tsa(e) | Error::Pdf(e) => f.write_str(e),
            Error::Offline => f.write_str(
                "time stamps and long-term validation need the internet; allow \
                 Kantor to go online for signatures first",
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<kantor_pdf::Error> for Error {
    fn from(e: kantor_pdf::Error) -> Self {
        Error::Pdf(e.to_string())
    }
}

/// Where a signing identity lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A .p12/.pfx file.
    File(std::path::PathBuf),
    /// A PKCS #11 token: module path and token label.
    Token { module: String, token: String },
    /// The macOS keychain or the Windows certificate store.
    System,
}

/// A certificate with its private key.
pub struct Identity {
    pub cert: Arc<Cert>,
    /// Other certificates to include (the issuers), signer excluded.
    pub chain: Vec<Arc<Cert>>,
    pub key: Box<dyn SigningKey>,
    pub source: Source,
}

impl Identity {
    /// Open a PKCS #12 file.
    pub fn from_p12(
        data: &[u8],
        password: &str,
        path: &std::path::Path,
    ) -> Result<Identity, Error> {
        let p = p12::open(data, password)?;
        Ok(Identity {
            cert: Arc::new(p.cert),
            chain: p.chain.into_iter().map(Arc::new).collect(),
            key: Box::new(p.key),
            source: Source::File(path.to_path_buf()),
        })
    }

    /// The signer's display name.
    pub fn name(&self) -> String {
        self.cert.subject_name().display_name()
    }
}

/// How much PAdES.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Signature only.
    #[default]
    B,
    /// With a signature time stamp.
    T,
    /// With validation data in the DSS.
    Lt,
    /// With a document time stamp over the validation data.
    Lta,
}

/// How to sign.
pub struct Options<'a> {
    pub place: Place,
    pub field_name: Option<String>,
    /// The visible appearance (ignored for invisible signatures).
    pub look: Option<Look>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    /// Certify with DocMDP permission 1, 2 or 3.
    pub certify: Option<u8>,
    /// Time-stamp authority URL (B-T and up).
    pub tsa: Option<String>,
    pub level: Level,
    /// RSASSA-PSS instead of PKCS #1 v1.5 for RSA keys.
    pub pss: bool,
    /// Network access; `None` = offline (then only B-B works).
    pub fetch: Option<&'a dyn Fetch>,
    /// Roots for building the chain for LTV data.
    pub trust: TrustStore,
    /// Signing time (Unix seconds).
    pub now: i64,
}

impl Options<'_> {
    pub fn new(place: Place) -> Options<'static> {
        Options {
            place,
            field_name: None,
            look: None,
            reason: None,
            location: None,
            contact: None,
            certify: None,
            tsa: None,
            level: Level::B,
            pss: false,
            fetch: None,
            trust: TrustStore::standard(),
            now: crate::signatures::now(),
        }
    }
}

/// Sign `pdf` with `id`. Returns the signed file (the input followed by
/// one to three incremental updates).
pub fn sign(pdf: &[u8], id: &Identity, opts: &Options<'_>) -> Result<Vec<u8>, Error> {
    let level = opts.level;
    let want_tsa = level >= Level::T;
    let tsa = opts.tsa.as_deref().filter(|u| !u.trim().is_empty());
    if want_tsa && tsa.is_none() {
        return Err(Error::Tsa(
            "a time stamp needs a time-stamp server (TSA) address".into(),
        ));
    }
    if (want_tsa || level >= Level::Lt) && opts.fetch.is_none() {
        return Err(Error::Offline);
    }
    let cert = &id.cert;
    if cert.not_after < opts.now {
        return Err(Error::Key(format!(
            "the certificate expired on {}",
            crate::signatures::format_time(cert.not_after)
        )));
    }
    if cert.not_before > opts.now {
        return Err(Error::Key("the certificate isn't valid yet".into()));
    }
    let kt = id.key.key_type();
    let hash = kt.hash();
    let chain: Vec<&Cert> = id.chain.iter().map(|c| c.as_ref()).collect();
    let certs_len: usize = cert.der.len() + chain.iter().map(|c| c.der.len()).sum::<usize>();
    let mut reserve = certs_len + 2048 + if want_tsa { 8192 } else { 0 };
    let mut tsa_token: Option<Vec<u8>> = None;
    let mut signed_file = None;
    let mut sig_cms = Vec::new();
    for _ in 0..3 {
        let mut req = Request::new(
            SigType::Signature {
                certify: opts.certify,
            },
            opts.place.clone(),
            opts.now,
            reserve,
        );
        req.field_name = opts.field_name.clone();
        req.look = opts.look.clone();
        req.name = Some(id.name());
        req.reason = opts.reason.clone();
        req.location = opts.location.clone();
        req.contact = opts.contact.clone();
        req.app = Some(("Kantor".into(), env!("CARGO_PKG_VERSION").into()));
        let prepared = pdfsign::prepare(pdf, &req)?;
        let [a, b] = prepared.signed_parts();
        let mut h = crate::signatures::crypto::Hasher::new(hash.oid())
            .ok_or_else(|| Error::Unsupported("digest".into()))?;
        h.update(a);
        h.update(b);
        let digest = h.finish();
        let signed = cms::sign_attributes(
            id.key.as_ref(),
            cert,
            cms::DATA,
            &digest,
            hash,
            opts.pss,
            None,
        )?;
        let mut unsigned = Vec::new();
        if want_tsa {
            let fetch = opts.fetch.ok_or(Error::Offline)?;
            let token = tsa::stamp(
                fetch,
                tsa.unwrap_or_default(),
                Hash::Sha256,
                &signed.signature,
            )?;
            unsigned.push(cms::timestamp_attribute(&token));
            tsa_token = Some(token);
        }
        let out = cms::assemble(&signed, cert, &chain, cms::DATA, None, unsigned);
        if out.len() <= prepared.capacity() {
            sig_cms = out.clone();
            signed_file = Some(prepared.finish(&out)?);
            break;
        }
        reserve = out.len() + 4096;
    }
    let mut file = signed_file.ok_or_else(|| Error::Pdf("the signature didn't fit".into()))?;
    if level >= Level::Lt {
        let fetch = opts.fetch.ok_or(Error::Offline)?;
        let mut dss = Dss::default();
        validation_data(&sig_cms, &opts.trust, fetch, opts.now, &mut dss);
        if let Some(t) = &tsa_token {
            validation_data(t, &opts.trust, fetch, opts.now, &mut dss);
        }
        file = pdfsign::add_dss(&file, &dss)?;
    }
    if level >= Level::Lta {
        let fetch = opts.fetch.ok_or(Error::Offline)?;
        file = document_timestamp(&file, tsa.unwrap_or_default(), fetch, opts.now)?;
    }
    Ok(file)
}

/// Add a document time stamp (`/DocTimeStamp`) as an incremental update.
pub fn document_timestamp(
    pdf: &[u8],
    tsa_url: &str,
    fetch: &dyn Fetch,
    now: i64,
) -> Result<Vec<u8>, Error> {
    let mut reserve = 12_288;
    for _ in 0..3 {
        let req = Request::new(SigType::DocTimeStamp, Place::Invisible, now, reserve);
        let prepared = pdfsign::prepare(pdf, &req)?;
        let [a, b] = prepared.signed_parts();
        let mut data = Vec::with_capacity(a.len() + b.len());
        data.extend_from_slice(a);
        data.extend_from_slice(b);
        let token = tsa::stamp(fetch, tsa_url, Hash::Sha256, &data)?;
        if token.len() <= prepared.capacity() {
            return Ok(prepared.finish(&token)?);
        }
        reserve = token.len() + 4096;
    }
    Err(Error::Pdf("the time stamp didn't fit".into()))
}

/// Records what the chain builder downloads, to put it in the DSS.
struct Recorder<'a> {
    inner: &'a dyn Fetch,
    ocsps: Mutex<Vec<Vec<u8>>>,
    gets: Mutex<Vec<(String, Vec<u8>)>>,
}

impl Fetch for Recorder<'_> {
    fn get(&self, url: &str) -> Option<Vec<u8>> {
        let body = self.inner.get(url)?;
        if let Ok(mut g) = self.gets.lock() {
            g.push((url.to_string(), body.clone()));
        }
        Some(body)
    }

    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Option<Vec<u8>> {
        let resp = self.inner.post(url, content_type, body)?;
        if content_type == "application/ocsp-request"
            && let Ok(mut o) = self.ocsps.lock()
        {
            o.push(resp.clone());
        }
        Some(resp)
    }
}

/// The chain of the signer of `cms` (a signature or time-stamp token) and
/// the revocation data for it, fetched now, added to `dss`.
fn validation_data(cms: &[u8], trust: &TrustStore, fetch: &dyn Fetch, now: i64, dss: &mut Dss) {
    let Ok(sd) = SignedData::parse(cms) else {
        return;
    };
    let Some(si) = sd.signers.first() else {
        return;
    };
    let Some(leaf) = sd.certs.iter().find(|c| si.names(c)).cloned() else {
        return;
    };
    let tsa = sd.content_type == crate::signatures::cms::CT_TST_INFO;
    let rec = Recorder {
        inner: fetch,
        ocsps: Mutex::new(Vec::new()),
        gets: Mutex::new(Vec::new()),
    };
    let mut ctx = Context::new(trust, Some(&rec));
    for c in sd.certs.iter().cloned() {
        ctx.add_cert(c);
    }
    let path = ctx.path(
        Arc::new(leaf),
        now,
        if tsa {
            Purpose::TimeStamping
        } else {
            Purpose::Signing
        },
    );
    let push = |list: &mut Vec<Vec<u8>>, b: Vec<u8>| {
        if !list.contains(&b) {
            list.push(b);
        }
    };
    for pc in &path.certs {
        push(&mut dss.certs, pc.cert.der.clone());
    }
    for o in rec.ocsps.into_inner().unwrap_or_default() {
        // Only successful responses; their responder certificates too.
        if let Ok(parsed) = crate::signatures::revocation::Ocsp::parse(&o) {
            for c in parsed.certs {
                push(&mut dss.certs, c.der);
            }
            push(&mut dss.ocsps, o);
        }
    }
    let crl_urls: Vec<&String> = path
        .certs
        .iter()
        .flat_map(|c| c.cert.ext.crl_urls.iter())
        .collect();
    for (url, body) in rec.gets.into_inner().unwrap_or_default() {
        if crl_urls.contains(&&url) {
            let der = if body.starts_with(b"-----BEGIN") {
                let text = String::from_utf8_lossy(&body);
                let b64: String = text.lines().filter(|l| !l.starts_with("-----")).collect();
                crate::signatures::x509::base64_decode(&b64).unwrap_or(body)
            } else {
                body
            };
            if crate::signatures::revocation::Crl::parse(&der).is_ok() {
                push(&mut dss.crls, der);
            }
        }
    }
}
