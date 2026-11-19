//! Digital signatures in PDFs: find them, check them, report like
//! Acrobat's signature panel.
//!
//! For each signature field: who signed, when, why and where; whether the
//! signed bytes are intact (ByteRange, digest, CMS signature, signed
//! attributes); whether the signer's certificate chains up to a trusted
//! root (OS store plus Kantor's bundled national roots, see [`trust`]) and
//! isn't revoked (OCSP/CRL from the DSS or the signature first, online
//! only when the caller passes a [`Fetch`]); RFC 3161 time stamps; and what
//! changed after signing ([`diff`]), judged against DocMDP.
//!
//! Pure Rust: lopdf reads the objects, a small ASN.1 reader ([`der`]) the
//! CMS structures, ring checks digests and signatures.

pub mod chain;
pub mod cms;
pub mod crypto;
pub mod der;
pub mod diff;
pub mod pdf;
pub mod revocation;
pub mod trust;
pub mod x509;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use lopdf::Document;

pub use chain::{CertProblem, Path, PathCert, PathEnd, RevSource, Revocation};
pub use diff::{Change, Class};
pub use trust::{BUNDLED, Fetch, Http, RootSource, TrustStore};
pub use x509::{Cert, Name};

use chain::{Context, Purpose};
use cms::{SignedData, SignerInfo, TstInfo};

/// A message argument: a word from the catalog (translated where shown) or
/// text kept as is (names, dates, numbers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Arg {
    Word(&'static str),
    Text(String),
}

impl fmt::Display for Arg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arg::Word(w) => f.write_str(w),
            Arg::Text(t) => f.write_str(t),
        }
    }
}

/// A message in English with `{}` placeholders; the UI translates the
/// template (and [`Arg::Word`]s) through its catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Msg {
    pub template: &'static str,
    pub args: Vec<Arg>,
}

impl Msg {
    pub fn new(template: &'static str) -> Msg {
        Msg {
            template,
            args: Vec::new(),
        }
    }

    pub fn with(template: &'static str, args: Vec<Arg>) -> Msg {
        Msg { template, args }
    }
}

impl fmt::Display for Msg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut args = self.args.iter();
        let mut rest = self.template;
        while let Some(i) = rest.find("{}") {
            f.write_str(&rest[..i])?;
            if let Some(a) = args.next() {
                write!(f, "{a}")?;
            }
            rest = &rest[i + 2..];
        }
        f.write_str(rest)
    }
}

/// The verdict on one signature, worst last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Status {
    Valid,
    /// Intact and trusted; later changes are ones the signer allowed
    /// (form filling, comments).
    ValidWithChanges,
    /// Intact, but the signer's identity or certificate status can't be
    /// confirmed (untrusted root, revocation not checked…).
    Unknown,
    /// Altered, badly signed, revoked, or changed in ways not allowed.
    Invalid,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Valid => "Valid",
            Status::ValidWithChanges => "Valid, with allowed changes",
            Status::Unknown => "Validity unknown",
            Status::Invalid => "Invalid",
        }
    }

    pub fn is_ok(self) -> bool {
        matches!(self, Status::Valid | Status::ValidWithChanges)
    }
}

/// The signature format.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SigKind {
    /// adbe.pkcs7.detached
    Pkcs7Detached,
    /// adbe.pkcs7.sha1
    Pkcs7Sha1,
    /// ETSI.CAdES.detached (PAdES)
    Cades,
    /// ETSI.RFC3161 document time stamp
    DocTimestamp,
    Other(String),
}

impl SigKind {
    fn from_sub_filter(s: &str, doc_ts: bool) -> SigKind {
        match s {
            "adbe.pkcs7.detached" => SigKind::Pkcs7Detached,
            "adbe.pkcs7.sha1" => SigKind::Pkcs7Sha1,
            "ETSI.CAdES.detached" => SigKind::Cades,
            "ETSI.RFC3161" => SigKind::DocTimestamp,
            _ if doc_ts => SigKind::DocTimestamp,
            other => SigKind::Other(other.to_string()),
        }
    }

    pub fn label(&self) -> Msg {
        match self {
            SigKind::Pkcs7Detached => Msg::new("PKCS #7, detached (adbe.pkcs7.detached)"),
            SigKind::Pkcs7Sha1 => Msg::new("PKCS #7 with SHA-1 (adbe.pkcs7.sha1)"),
            SigKind::Cades => Msg::new("CAdES / PAdES (ETSI.CAdES.detached)"),
            SigKind::DocTimestamp => Msg::new("Document time stamp (RFC 3161)"),
            SigKind::Other(s) => Msg::with("Other ({})", vec![Arg::Text(s.clone())]),
        }
    }
}

/// Whether the signed bytes are what was signed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Integrity {
    Intact,
    /// The byte range is malformed or doesn't cover the revision.
    ByteRange,
    /// The bytes changed since signing.
    DigestMismatch,
    /// The CMS signature doesn't verify with the signer's key.
    BadSignature,
    /// The signing-certificate attribute names another certificate.
    SigningCertMismatch,
    /// The signer's certificate isn't in the signature (or the DSS).
    NoSignerCert,
    /// An algorithm Kantor can't check.
    Unsupported(String),
    /// The signature can't be read at all.
    Unreadable,
}

impl Integrity {
    pub fn msg(&self) -> Option<Msg> {
        Some(match self {
            Integrity::Intact => return None,
            Integrity::ByteRange => {
                Msg::new("The signature's byte range doesn't cover the signed revision correctly.")
            }
            Integrity::DigestMismatch => Msg::new(
                "The document was altered or damaged after signing: the signed bytes don't match the signature.",
            ),
            Integrity::BadSignature => {
                Msg::new("The signature doesn't match the signer's certificate.")
            }
            Integrity::SigningCertMismatch => Msg::new(
                "The signature names a different signing certificate than the one it carries.",
            ),
            Integrity::NoSignerCert => Msg::new("The signer's certificate isn't in the signature."),
            Integrity::Unsupported(what) => Msg::with(
                "Kantor can't check this kind of signature ({}).",
                vec![Arg::Text(what.clone())],
            ),
            Integrity::Unreadable => Msg::new("The signature data can't be read."),
        })
    }
}

/// An RFC 3161 time stamp on a signature.
#[derive(Clone, Debug)]
pub struct Timestamp {
    pub time: i64,
    pub tsa: Option<Arc<Cert>>,
    pub integrity: Integrity,
    pub path: Option<Path>,
}

impl Timestamp {
    /// Intact and from a trusted time-stamp authority.
    pub fn trusted(&self) -> bool {
        self.integrity == Integrity::Intact
            && self
                .path
                .as_ref()
                .is_some_and(|p| p.trusted() && !p.has_problems() && p.revoked().is_none())
    }
}

/// One signed field.
#[derive(Clone, Debug)]
pub struct Signature {
    pub field: String,
    pub page: Option<usize>,
    pub kind: SigKind,
    pub signer: Option<Arc<Cert>>,
    /// The /Name entry (some signers set it instead of a usable CN).
    pub name_entry: Option<String>,
    /// The time the signer's computer claimed (/M or signingTime).
    pub claimed_time: Option<i64>,
    pub timestamp: Option<Timestamp>,
    pub reason: Option<String>,
    pub location: Option<String>,
    pub contact: Option<String>,
    /// 1-based revision the signature closes, of `revisions`.
    pub revision: usize,
    pub revisions: usize,
    /// Byte length of the signed revision.
    pub signed_len: usize,
    pub covers_whole_file: bool,
    pub digest_alg: Option<String>,
    pub integrity: Integrity,
    pub path: Option<Path>,
    /// Certificates were checked as of this time (the time stamp's, or
    /// the time of the check).
    pub checked_at: i64,
    pub checked_at_timestamp: bool,
    /// DocMDP level when this is a certification signature.
    pub certification: Option<u8>,
    /// DocMDP level governing changes after this signature.
    pub docmdp: Option<u8>,
    pub changes: Vec<Change>,
    pub status: Status,
    /// Why the status is what it is, most important first.
    pub notes: Vec<Msg>,
}

impl Signature {
    /// The signer's name: the certificate's common name, else /Name.
    pub fn signer_name(&self) -> String {
        self.signer
            .as_ref()
            .map(|c| c.subject_name().display_name())
            .or_else(|| self.name_entry.clone())
            .unwrap_or_else(|| "Unknown signer".into())
    }

    pub fn organisation(&self) -> Option<String> {
        self.signer
            .as_ref()
            .and_then(|c| c.subject_name().get("O").map(str::to_string))
    }

    /// The time to show: the trusted time stamp's, else the claimed one.
    pub fn time(&self) -> Option<i64> {
        self.timestamp
            .as_ref()
            .filter(|t| t.integrity == Integrity::Intact)
            .map(|t| t.time)
            .or(self.claimed_time)
    }
}

/// A signature field nobody has signed yet.
#[derive(Clone, Debug)]
pub struct EmptyField {
    pub name: String,
    pub page: Option<usize>,
}

/// The whole check.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub signatures: Vec<Signature>,
    pub empty_fields: Vec<EmptyField>,
    pub revisions: usize,
    /// Some certificate's issuer or status could have been fetched online.
    pub online_would_help: bool,
    pub checked_online: bool,
    /// The file couldn't be read for signatures.
    pub error: Option<String>,
}

impl Report {
    pub fn is_signed(&self) -> bool {
        !self.signatures.is_empty()
    }

    /// The worst status, if signed.
    pub fn overall(&self) -> Option<Status> {
        self.signatures.iter().map(|s| s.status).max()
    }

    pub fn all_valid(&self) -> bool {
        self.is_signed() && self.signatures.iter().all(|s| s.status.is_ok())
    }
}

/// How to check.
pub struct Options<'a> {
    pub trust: TrustStore,
    /// Network access for missing issuers and revocation; `None` = offline.
    pub fetch: Option<&'a dyn Fetch>,
    /// "Now", Unix seconds.
    pub now: i64,
}

impl Options<'_> {
    /// Offline, with the system and bundled roots, as of now.
    pub fn standard() -> Options<'static> {
        Options {
            trust: TrustStore::standard(),
            fetch: None,
            now: now(),
        }
    }
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Quick test whether a file has any signature field (no checking).
pub fn has_signatures(bytes: &[u8]) -> bool {
    let probe = |pat: &[u8]| bytes.windows(pat.len()).any(|w| w == pat);
    if !probe(b"/ByteRange") && !probe(b"/FT/Sig") && !probe(b"/FT /Sig") {
        return false;
    }
    Document::load_mem(bytes)
        .map(|d| !pdf::fields(&d).is_empty())
        .unwrap_or(false)
}

/// Check every signature in `bytes`.
pub fn verify(bytes: &[u8], opts: &Options<'_>) -> Report {
    let mut report = Report {
        checked_online: opts.fetch.is_some(),
        ..Report::default()
    };
    let full = match Document::load_mem(bytes) {
        Ok(d) => d,
        Err(e) => {
            report.error = Some(e.to_string());
            return report;
        }
    };
    let ends = pdf::revision_ends(bytes);
    report.revisions = ends.len().max(1);
    let fields = pdf::fields(&full);
    let mut ctx = Context::new(&opts.trust, opts.fetch);
    let dss = pdf::dss(&full);
    for c in dss.certs.iter().filter_map(|d| Cert::parse(d).ok()) {
        ctx.add_cert(c);
    }
    for o in &dss.ocsps {
        ctx.add_ocsp(o);
    }
    for c in &dss.crls {
        ctx.add_crl(c);
    }

    // Parse every signature first so all their certificates are known.
    struct Parsed {
        field: pdf::Field,
        sig: pdf::SigDict,
        cms: Option<SignedData>,
    }
    let mut parsed = Vec::new();
    for f in fields {
        let Some(sig) = f.sig.clone() else {
            report.empty_fields.push(EmptyField {
                name: f.name.clone(),
                page: f.page,
            });
            continue;
        };
        let cms = SignedData::parse(&sig.contents).ok();
        if let Some(sd) = &cms {
            for c in sd.certs.clone() {
                ctx.add_cert(c);
            }
            for crl in &sd.crls {
                ctx.add_crl(crl);
            }
            if let Some(si) = sd.signers.first() {
                let (crls, ocsps) = cms::adobe_revocation(si);
                crls.iter().for_each(|c| ctx.add_crl(c));
                ocsps.iter().for_each(|o| ctx.add_ocsp(o));
                // Certificates inside the signature time stamp too.
                if let Some(tok) = si.unsigned(cms::ATTR_TIMESTAMP_TOKEN)
                    && let Some(v) = tok.values.first()
                    && let Ok(tsd) = SignedData::parse(v)
                {
                    for c in tsd.certs {
                        ctx.add_cert(c);
                    }
                }
            }
        }
        parsed.push(Parsed { field: f, sig, cms });
    }
    parsed.sort_by_key(|p| p.sig.byte_range.get(2..4).map(|r| r[0] + r[1]).unwrap_or(0));

    // The DocMDP level: the (first) certification signature's.
    let docmdp_level = parsed
        .iter()
        .find_map(|p| p.sig.docmdp)
        .or_else(|| pdf::catalog_docmdp(&full));

    let mut signed_docs: HashMap<usize, Option<Document>> = HashMap::new();
    for p in parsed {
        let mut s = check_one(bytes, &p.field, &p.sig, p.cms.as_ref(), &mut ctx, opts.now);
        s.revisions = report.revisions;
        s.revision = ends
            .iter()
            .position(|&e| e >= s.signed_len)
            .map(|i| i + 1)
            .unwrap_or(report.revisions);
        s.docmdp = docmdp_level;
        if s.integrity == Integrity::Intact && !s.covers_whole_file {
            let doc = signed_docs
                .entry(s.signed_len)
                .or_insert_with(|| Document::load_mem(&bytes[..s.signed_len]).ok());
            match doc {
                Some(d) => s.changes = diff::changes(d, &full),
                None => s.changes.push(Change::Other {
                    what: "unreadable signed revision".into(),
                }),
            }
        }
        judge(&mut s);
        report.signatures.push(s);
    }
    report.online_would_help = ctx.online_would_help;
    report
}

fn hash_pieces(alg: &str, pieces: &[&[u8]]) -> Option<Vec<u8>> {
    let mut h = crypto::Hasher::new(alg)?;
    for p in pieces {
        h.update(p);
    }
    Some(h.finish())
}

fn hex_decode(s: &[u8]) -> Option<Vec<u8>> {
    let digits: Vec<u8> = s
        .iter()
        .copied()
        .filter(|b| !b.is_ascii_whitespace())
        .collect();
    let val = |c: u8| (c as char).to_digit(16).map(|v| v as u8);
    let mut out = Vec::with_capacity(digits.len() / 2 + 1);
    for pair in digits.chunks(2) {
        let hi = val(pair[0])?;
        let lo = match pair.get(1) {
            Some(&c) => val(c)?,
            None => 0,
        };
        out.push(hi << 4 | lo);
    }
    Some(out)
}

/// The byte range as two slices, if well formed.
fn byte_range<'a>(bytes: &'a [u8], br: &[i64], contents: &[u8]) -> Option<[&'a [u8]; 2]> {
    let &[a, b, c, d] = br else { return None };
    if a != 0 || b < 0 || c < b || d < 0 {
        return None;
    }
    let (b, c, d) = (b as usize, c as usize, d as usize);
    if c.checked_add(d)? > bytes.len() {
        return None;
    }
    // The gap is exactly the /Contents hex string.
    let gap = &bytes[b..c];
    if gap.first() != Some(&b'<') || gap.last() != Some(&b'>') {
        return None;
    }
    let decoded = hex_decode(&gap[1..gap.len() - 1])?;
    if decoded != contents {
        return None;
    }
    Some([&bytes[..b], &bytes[c..c + d]])
}

struct CmsCheck {
    integrity: Integrity,
    signer: Option<Arc<Cert>>,
    digest_alg: Option<String>,
    signing_time: Option<i64>,
    /// The TSTInfo, for time-stamp tokens.
    tst: Option<TstInfo>,
}

/// Check a SignedData over external `pieces` (or its own content).
fn check_cms(sd: &SignedData, pieces: &[&[u8]], kind: &SigKind, ctx: &Context<'_>) -> CmsCheck {
    let mut out = CmsCheck {
        integrity: Integrity::Unreadable,
        signer: None,
        digest_alg: None,
        signing_time: None,
        tst: None,
    };
    let Some(si) = sd.signers.first() else {
        return out;
    };
    out.digest_alg = Some(si.digest_alg.clone());
    out.signing_time = cms::signing_time(si);
    let signer = sd
        .certs
        .iter()
        .find(|c| si.names(c))
        .map(|c| Arc::new(c.clone()))
        .or_else(|| ctx.pool.iter().find(|c| si.names(c)).cloned());
    out.signer = signer.clone();
    let Some(signer) = signer else {
        out.integrity = Integrity::NoSignerCert;
        return out;
    };

    // What the signer's digest covers.
    let owned;
    let content: Vec<&[u8]> = match kind {
        SigKind::DocTimestamp | SigKind::Pkcs7Sha1 => {
            let Some(c) = &sd.content else {
                return out;
            };
            owned = c.clone();
            // The embedded content itself must match the document.
            if *kind == SigKind::DocTimestamp {
                let Ok(tst) = TstInfo::parse(&owned) else {
                    return out;
                };
                match hash_pieces(&tst.hash_alg, pieces) {
                    None => {
                        out.integrity = Integrity::Unsupported(crypto::digest_label(&tst.hash_alg));
                        return out;
                    }
                    Some(h) if h != tst.hashed => {
                        out.integrity = Integrity::DigestMismatch;
                        out.tst = Some(tst);
                        return out;
                    }
                    Some(_) => {}
                }
                out.tst = Some(tst);
            } else if hash_pieces(crypto::SHA1, pieces).as_deref() != Some(&owned[..]) {
                out.integrity = Integrity::DigestMismatch;
                return out;
            }
            vec![&owned[..]]
        }
        _ => pieces.to_vec(),
    };
    out.integrity = signer_check(si, &signer, &content);
    out
}

/// The SignerInfo's digest and signature over `content`.
fn signer_check(si: &SignerInfo, signer: &Cert, content: &[&[u8]]) -> Integrity {
    let Some(digest) = hash_pieces(&si.digest_alg, content) else {
        return Integrity::Unsupported(crypto::digest_label(&si.digest_alg));
    };
    let verified = match &si.signed_attrs {
        Some((set, _)) => {
            let md = si
                .signed(cms::ATTR_MESSAGE_DIGEST)
                .and_then(|a| a.values.first())
                .and_then(|v| der::Tlv::parse(v).ok())
                .map(|t| t.body.to_vec());
            if md.as_deref() != Some(&digest[..]) {
                return Integrity::DigestMismatch;
            }
            crypto::verify(
                &signer.key,
                &si.sig_alg,
                Some(&si.digest_alg),
                set,
                &si.signature,
            )
        }
        None => {
            let joined: Vec<u8> = content.concat();
            crypto::verify(
                &signer.key,
                &si.sig_alg,
                Some(&si.digest_alg),
                &joined,
                &si.signature,
            )
        }
    };
    match verified {
        Ok(()) => {}
        Err(crypto::VerifyError::Bad) => return Integrity::BadSignature,
        Err(crypto::VerifyError::Unsupported(w)) => return Integrity::Unsupported(w),
    }
    // ESS signing-certificate(-v2): binds the certificate to the signature.
    let ess = si
        .signed(cms::ATTR_SIGNING_CERT_V2)
        .and_then(|a| cms::signing_cert_hash(a, true))
        .or_else(|| {
            si.signed(cms::ATTR_SIGNING_CERT)
                .and_then(|a| cms::signing_cert_hash(a, false))
        });
    if let Some((alg, hash)) = ess
        && crypto::digest(&alg, &signer.der).is_some_and(|h| h != hash)
    {
        return Integrity::SigningCertMismatch;
    }
    Integrity::Intact
}

fn check_timestamp(si: &SignerInfo, ctx: &mut Context<'_>, now: i64) -> Option<Timestamp> {
    let token = si.unsigned(cms::ATTR_TIMESTAMP_TOKEN)?.values.first()?;
    let Ok(sd) = SignedData::parse(token) else {
        return Some(Timestamp {
            time: 0,
            tsa: None,
            integrity: Integrity::Unreadable,
            path: None,
        });
    };
    let Some(tst) = sd.content.as_ref().and_then(|c| TstInfo::parse(c).ok()) else {
        return Some(Timestamp {
            time: 0,
            tsa: None,
            integrity: Integrity::Unreadable,
            path: None,
        });
    };
    let mut integrity = match crypto::digest(&tst.hash_alg, &si.signature) {
        Some(h) if h == tst.hashed => Integrity::Intact,
        Some(_) => Integrity::DigestMismatch,
        None => Integrity::Unsupported(crypto::digest_label(&tst.hash_alg)),
    };
    let tsa = sd.signers.first().and_then(|tsi| {
        sd.certs
            .iter()
            .find(|c| tsi.names(c))
            .map(|c| Arc::new(c.clone()))
            .or_else(|| ctx.pool.iter().find(|c| tsi.names(c)).cloned())
    });
    if integrity == Integrity::Intact {
        integrity = match (&tsa, sd.signers.first(), &sd.content) {
            (Some(tsa), Some(tsi), Some(content)) => signer_check(tsi, tsa, &[content]),
            (None, _, _) => Integrity::NoSignerCert,
            _ => Integrity::Unreadable,
        };
    }
    let path = tsa.clone().map(|c| ctx.path(c, now, Purpose::TimeStamping));
    Some(Timestamp {
        time: tst.time,
        tsa,
        integrity,
        path,
    })
}

fn check_one(
    bytes: &[u8],
    field: &pdf::Field,
    sig: &pdf::SigDict,
    sd: Option<&SignedData>,
    ctx: &mut Context<'_>,
    now: i64,
) -> Signature {
    let kind = SigKind::from_sub_filter(&sig.sub_filter, sig.is_doc_timestamp);
    let signed_len = sig
        .byte_range
        .get(2..4)
        .map(|r| (r[0] + r[1]).max(0) as usize)
        .unwrap_or(0)
        .min(bytes.len());
    let mut s = Signature {
        field: field.name.clone(),
        page: field.page,
        kind: kind.clone(),
        signer: None,
        name_entry: sig.name.clone(),
        claimed_time: sig.time,
        timestamp: None,
        reason: sig.reason.clone(),
        location: sig.location.clone(),
        contact: sig.contact.clone(),
        revision: 0,
        revisions: 0,
        signed_len,
        covers_whole_file: signed_len == bytes.len(),
        digest_alg: None,
        integrity: Integrity::Unreadable,
        path: None,
        checked_at: now,
        checked_at_timestamp: false,
        certification: sig.docmdp,
        docmdp: None,
        changes: Vec::new(),
        status: Status::Invalid,
        notes: Vec::new(),
    };
    let Some(pieces) = byte_range(bytes, &sig.byte_range, &sig.contents) else {
        s.integrity = Integrity::ByteRange;
        return s;
    };
    if let SigKind::Other(sf) = &kind
        && sf != "adbe.pkcs7.detached"
    {
        s.integrity = Integrity::Unsupported(if sf.is_empty() {
            sig.filter.clone()
        } else {
            sf.clone()
        });
        return s;
    }
    let Some(sd) = sd else {
        return s;
    };
    let check = check_cms(sd, &pieces, &kind, ctx);
    s.integrity = check.integrity;
    s.signer = check.signer;
    s.digest_alg = check.digest_alg;
    if s.claimed_time.is_none() {
        s.claimed_time = check.signing_time;
    }
    if let Some(tst) = &check.tst {
        s.claimed_time = Some(tst.time);
    }
    if let Some(si) = sd.signers.first()
        && kind != SigKind::DocTimestamp
    {
        s.timestamp = check_timestamp(si, ctx, now);
    }
    if let Some(ts) = &s.timestamp
        && ts.trusted()
    {
        s.checked_at = ts.time;
        s.checked_at_timestamp = true;
    }
    if let Some(signer) = s.signer.clone() {
        let purpose = if kind == SigKind::DocTimestamp {
            Purpose::TimeStamping
        } else {
            Purpose::Signing
        };
        s.path = Some(ctx.path(signer, s.checked_at, purpose));
    }
    s
}

/// Set the status and notes from the checks.
fn judge(s: &mut Signature) {
    let mut notes = Vec::new();
    let mut status = Status::Valid;
    let mut worse = |st: Status, m: Msg, notes: &mut Vec<Msg>| {
        status = status.max(st);
        notes.push(m);
    };
    if let Some(m) = s.integrity.msg() {
        worse(Status::Invalid, m, &mut notes);
    }
    if let Some(at) = s.path.as_ref().and_then(Path::revoked) {
        worse(
            Status::Invalid,
            Msg::with(
                "A certificate in the chain was revoked on {}.",
                vec![Arg::Text(format_time(at))],
            ),
            &mut notes,
        );
    }
    let disallowed: Vec<&Change> = s.changes.iter().filter(|c| !c.allowed(s.docmdp)).collect();
    if !disallowed.is_empty() {
        let m = if disallowed.iter().any(|c| c.class() == Class::Modification) {
            Msg::new("The document was modified after it was signed.")
        } else {
            Msg::new(
                "The document was changed after signing in ways the certifying signer didn't allow.",
            )
        };
        worse(Status::Invalid, m, &mut notes);
    }
    if let Some(path) = &s.path {
        match path.end {
            PathEnd::Trusted(_) => {}
            PathEnd::UntrustedRoot => worse(
                Status::Unknown,
                Msg::new(
                    "The signer's identity can't be confirmed: the certificate chain ends at a root that isn't trusted.",
                ),
                &mut notes,
            ),
            PathEnd::Incomplete => worse(
                Status::Unknown,
                Msg::new(
                    "The signer's identity can't be confirmed: an issuer's certificate is missing.",
                ),
                &mut notes,
            ),
        }
        let problems: Vec<&CertProblem> =
            path.certs.iter().flat_map(|c| c.problems.iter()).collect();
        for p in problems {
            let m = match p {
                CertProblem::Expired => {
                    Msg::new("A certificate in the chain had expired at the time it was checked.")
                }
                CertProblem::NotYetValid => Msg::new(
                    "A certificate in the chain wasn't valid yet at the time it was checked.",
                ),
                CertProblem::NotACa => {
                    Msg::new("A certificate in the chain isn't allowed to issue certificates.")
                }
                CertProblem::KeyUsage => {
                    Msg::new("A certificate's key usage doesn't allow it to be used this way.")
                }
                CertProblem::UnknownCritical(_) => {
                    Msg::new("A certificate has a critical extension Kantor doesn't understand.")
                }
            };
            if !notes.contains(&m) {
                worse(Status::Unknown, m, &mut notes);
            }
        }
        if path.revocation_unknown() {
            worse(
                Status::Unknown,
                Msg::new(
                    "Revocation not checked: Kantor couldn't confirm that the certificates weren't revoked.",
                ),
                &mut notes,
            );
        }
    }
    if let Some(ts) = &s.timestamp {
        if ts.integrity != Integrity::Intact {
            worse(
                Status::Unknown,
                Msg::new("The signature's time stamp couldn't be verified."),
                &mut notes,
            );
        } else if !ts.trusted() {
            notes.push(Msg::new(
                "The time stamp authority isn't trusted; the signature was checked as of now.",
            ));
        }
    }
    if status == Status::Valid {
        if s.changes.iter().any(|c| {
            // Metadata (/Info ModDate, Producer) changes with almost any
            // save, so like Acrobat it doesn't count as a change here.
            matches!(c.class(), Class::FormFill | Class::Annotation)
        }) {
            status = Status::ValidWithChanges;
            notes.insert(
                0,
                Msg::new("The document was changed after signing, in ways the signer allowed."),
            );
        } else if s.covers_whole_file {
            notes.insert(
                0,
                Msg::new("The document hasn't been changed since it was signed."),
            );
        } else {
            notes.insert(
                0,
                Msg::new("The signed revision is unchanged; only signatures or validation data were added after it."),
            );
        }
    }
    s.status = status;
    s.notes = notes;
}

/// An annotation subtype as a word for messages ("Stamp", "Comment").
pub fn annotation_word(subtype: &str) -> &'static str {
    match subtype {
        "Stamp" => "Stamp",
        "Text" | "Popup" => "Comment",
        "FreeText" => "Text box",
        "Highlight" => "Highlight",
        "Underline" => "Underline",
        "StrikeOut" => "Strikethrough",
        "Squiggly" => "Squiggly underline",
        "Ink" => "Drawing",
        "Square" => "Rectangle",
        "Circle" => "Oval",
        "Line" => "Line",
        "Polygon" => "Polygon",
        "PolyLine" => "Polyline",
        "FileAttachment" => "Attachment",
        "Link" => "Link",
        "Caret" => "Insertion mark",
        "Redact" => "Redaction",
        "Sound" => "Sound",
        _ => "Annotation",
    }
}

impl Change {
    pub fn msg(&self) -> Msg {
        let page = |p: &Option<usize>| Arg::Text(p.map(|n| n.to_string()).unwrap_or_default());
        match self {
            Change::LtvAdded => Msg::new("Long-term validation data added"),
            Change::SignatureAdded { field } => {
                Msg::with("Signature added ({})", vec![Arg::Text(field.clone())])
            }
            Change::TimestampAdded => Msg::new("Document time stamp added"),
            Change::FormFilled { field } if field.is_empty() => Msg::new("Form field filled in"),
            Change::FormFilled { field } => {
                Msg::with("Form field filled in: {}", vec![Arg::Text(field.clone())])
            }
            Change::AnnotationAdded {
                kind,
                page: Some(p),
            } => Msg::with(
                "{} added on page {}",
                vec![Arg::Word(annotation_word(kind)), page(&Some(*p))],
            ),
            Change::AnnotationAdded { kind, page: None } => {
                Msg::with("{} added", vec![Arg::Word(annotation_word(kind))])
            }
            Change::AnnotationChanged {
                kind,
                page: Some(p),
            } => Msg::with(
                "{} changed on page {}",
                vec![Arg::Word(annotation_word(kind)), page(&Some(*p))],
            ),
            Change::AnnotationChanged { kind, page: None } => {
                Msg::with("{} changed", vec![Arg::Word(annotation_word(kind))])
            }
            Change::AnnotationRemoved { page: p } => {
                Msg::with("Annotation removed on page {}", vec![page(p)])
            }
            Change::PageContentChanged { page: Some(p) } => {
                Msg::with("Page content changed on page {}", vec![page(&Some(*p))])
            }
            Change::PageContentChanged { page: None } => Msg::new("Page content changed"),
            Change::PagesChanged => Msg::new("Pages added, removed or reordered"),
            Change::MetadataChanged => Msg::new("Document properties changed"),
            Change::Other { what } => Msg::with("Other change ({})", vec![Arg::Text(what.clone())]),
        }
    }
}

impl Revocation {
    pub fn msg(&self) -> Msg {
        match self {
            Revocation::Root => Msg::new("Trusted root (not checked)"),
            Revocation::Good {
                by: RevSource::Ocsp,
                online: false,
            } => Msg::new("Not revoked (OCSP response in the file)"),
            Revocation::Good {
                by: RevSource::Crl,
                online: false,
            } => Msg::new("Not revoked (CRL in the file)"),
            Revocation::Good {
                by: RevSource::Ocsp,
                online: true,
            } => Msg::new("Not revoked (checked online, OCSP)"),
            Revocation::Good {
                by: RevSource::Crl,
                online: true,
            } => Msg::new("Not revoked (checked online, CRL)"),
            Revocation::Revoked { at, .. } => {
                Msg::with("Revoked on {}", vec![Arg::Text(format_time(*at))])
            }
            Revocation::NotPublished => Msg::new("The issuer publishes no revocation information"),
            Revocation::NoCheck => Msg::new("Not checked (OCSP responder certificate)"),
            Revocation::Unknown => Msg::new("Not checked"),
        }
    }
}

/// `2026-11-19 03:00:00 UTC`.
pub fn format_time(t: i64) -> String {
    let (y, mo, d, h, mi, s) = der::civil(t);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

/// The report as text (`kantor pdf verify`).
pub fn text_report(r: &Report) -> String {
    use std::fmt::Write as _;
    let mut o = String::new();
    if let Some(e) = &r.error {
        let _ = writeln!(o, "Can't read the signatures: {e}");
        return o;
    }
    if !r.is_signed() {
        o.push_str("Not signed.\n");
    } else if r.all_valid() {
        o.push_str("Signed and all signatures are valid.\n");
    } else {
        o.push_str("At least one signature has problems.\n");
    }
    for (i, s) in r.signatures.iter().enumerate() {
        let _ = writeln!(
            o,
            "\nSignature {} of {}: {}",
            i + 1,
            r.signatures.len(),
            s.field
        );
        let _ = writeln!(o, "  Status:     {}", s.status.label());
        for n in &s.notes {
            let _ = writeln!(o, "              {n}");
        }
        let _ = writeln!(o, "  Signer:     {}", s.signer_name());
        if let Some(org) = s.organisation() {
            let _ = writeln!(o, "  Organization: {org}");
        }
        if let Some(t) = s.time() {
            let how = if s.timestamp.as_ref().is_some_and(Timestamp::trusted) {
                " (trusted time stamp)"
            } else if s.kind == SigKind::DocTimestamp {
                " (time stamp)"
            } else {
                " (signer's computer clock)"
            };
            let _ = writeln!(o, "  Signed:     {}{how}", format_time(t));
        }
        let _ = writeln!(o, "  Type:       {}", s.kind.label());
        if s.certification.is_some() {
            let level = match s.certification {
                Some(1) => "no changes allowed",
                Some(3) => "form filling, signing and comments allowed",
                _ => "form filling and signing allowed",
            };
            let _ = writeln!(o, "  Certifies:  yes ({level})");
        }
        if let Some(r) = &s.reason {
            let _ = writeln!(o, "  Reason:     {r}");
        }
        if let Some(l) = &s.location {
            let _ = writeln!(o, "  Location:   {l}");
        }
        if let Some(p) = s.page {
            let _ = writeln!(o, "  Page:       {p}");
        }
        let _ = writeln!(o, "  Revision:   {} of {}", s.revision, s.revisions);
        if let Some(path) = &s.path {
            let _ = writeln!(o, "  Certificates:");
            for c in &path.certs {
                let _ = writeln!(
                    o,
                    "    - {} (valid {} to {}); revocation: {}",
                    c.cert.subject_name().display_name(),
                    &format_time(c.cert.not_before)[..10],
                    &format_time(c.cert.not_after)[..10],
                    c.revocation.msg()
                );
            }
            let end = match path.end {
                PathEnd::Trusted(RootSource::System) => "trusted root (system store)",
                PathEnd::Trusted(RootSource::Bundled) => "trusted root (bundled with Kantor)",
                PathEnd::Trusted(RootSource::Extra) => "trusted root (--trust)",
                PathEnd::UntrustedRoot => "root not trusted",
                PathEnd::Incomplete => "chain incomplete",
            };
            let _ = writeln!(o, "    ({end})");
        }
        if let Some(ts) = &s.timestamp {
            let tsa = ts
                .tsa
                .as_ref()
                .map(|c| c.subject_name().display_name())
                .unwrap_or_else(|| "unknown".into());
            let state = if ts.trusted() {
                "valid"
            } else if ts.integrity == Integrity::Intact {
                "intact, authority not trusted"
            } else {
                "invalid"
            };
            let _ = writeln!(
                o,
                "  Time stamp: {} by {tsa} ({state})",
                format_time(ts.time)
            );
        }
        if s.changes.is_empty() {
            if !s.covers_whole_file {
                o.push_str("  Changes after signing: none\n");
            }
        } else {
            o.push_str("  Changes after signing:\n");
            for c in &s.changes {
                let flag = if c.allowed(s.docmdp) {
                    ""
                } else {
                    " (not allowed)"
                };
                let _ = writeln!(o, "    - {}{flag}", c.msg());
            }
        }
    }
    for f in &r.empty_fields {
        let page = f.page.map(|p| format!(" on page {p}")).unwrap_or_default();
        let _ = writeln!(o, "\nEmpty signature field: {}{page}", f.name);
    }
    if r.online_would_help && !r.checked_online {
        o.push_str(
            "\nSome certificates could be checked online (issuers, revocation); run with --online.\n",
        );
    }
    o
}

/// The bytes of the revision a signature signed, to open read-only.
pub fn signed_version(bytes: &[u8], s: &Signature) -> Vec<u8> {
    bytes[..s.signed_len.min(bytes.len())].to_vec()
}

#[cfg(test)]
mod tests;
