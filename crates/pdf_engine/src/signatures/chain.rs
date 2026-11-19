//! Certificate paths: from the signer up to a trusted root, using the
//! certificates in the signature, the DSS, the trust store and (only when
//! allowed) the issuers' download addresses; then revocation per
//! certificate.

use std::sync::Arc;

use super::revocation::{Crl, Ocsp, Said, ocsp_request};
use super::trust::{Fetch, RootSource, TrustStore};
use super::x509::{self, Cert};

/// Everything known while checking one file: certificates and revocation
/// answers collected from the file (and the network), shared by all its
/// signatures.
pub struct Context<'a> {
    pub trust: &'a TrustStore,
    pub pool: Vec<Arc<Cert>>,
    pub ocsps: Vec<Ocsp>,
    pub crls: Vec<Crl>,
    pub fetch: Option<&'a dyn Fetch>,
    fetched: Vec<String>,
    /// Some check would have gone online if it were allowed.
    pub online_would_help: bool,
}

/// How far the path got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathEnd {
    /// Ends at a trusted root.
    Trusted(RootSource),
    /// Ends at a root nobody trusts here.
    UntrustedRoot,
    /// An issuer couldn't be found.
    Incomplete,
}

/// Revocation status of one certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Revocation {
    /// A trusted root: not checked.
    Root,
    Good {
        by: RevSource,
        online: bool,
    },
    Revoked {
        at: i64,
        by: RevSource,
    },
    /// The CA publishes no revocation information.
    NotPublished,
    /// OCSP-signing certificate marked "no check".
    NoCheck,
    /// Couldn't be checked (offline, server down, nothing embedded).
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevSource {
    Ocsp,
    Crl,
}

/// A problem with one certificate on the path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CertProblem {
    Expired,
    NotYetValid,
    NotACa,
    /// Key usage doesn't allow what it's used for.
    KeyUsage,
    /// A critical extension Kantor doesn't understand.
    UnknownCritical(String),
}

#[derive(Clone, Debug)]
pub struct PathCert {
    pub cert: Arc<Cert>,
    pub revocation: Revocation,
    pub problems: Vec<CertProblem>,
}

#[derive(Clone, Debug)]
pub struct Path {
    /// Signer first.
    pub certs: Vec<PathCert>,
    pub end: PathEnd,
}

impl Path {
    pub fn trusted(&self) -> bool {
        matches!(self.end, PathEnd::Trusted(_))
    }

    pub fn revoked(&self) -> Option<i64> {
        self.certs.iter().find_map(|c| match c.revocation {
            Revocation::Revoked { at, .. } => Some(at),
            _ => None,
        })
    }

    pub fn revocation_unknown(&self) -> bool {
        self.certs
            .iter()
            .any(|c| c.revocation == Revocation::Unknown)
    }

    pub fn has_problems(&self) -> bool {
        self.certs.iter().any(|c| !c.problems.is_empty())
    }
}

/// What the end certificate is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    Signing,
    TimeStamping,
}

impl<'a> Context<'a> {
    pub fn new(trust: &'a TrustStore, fetch: Option<&'a dyn Fetch>) -> Self {
        Context {
            trust,
            pool: Vec::new(),
            ocsps: Vec::new(),
            crls: Vec::new(),
            fetch,
            fetched: Vec::new(),
            online_would_help: false,
        }
    }

    pub fn add_cert(&mut self, c: Cert) {
        if !self.pool.iter().any(|p| p.same(&c)) {
            self.pool.push(Arc::new(c));
        }
    }

    pub fn add_ocsp(&mut self, der: &[u8]) {
        if let Ok(o) = Ocsp::parse(der) {
            for c in o.certs.clone() {
                self.add_cert(c);
            }
            self.ocsps.push(o);
        }
    }

    pub fn add_crl(&mut self, der: &[u8]) {
        if let Ok(c) = Crl::parse(der) {
            self.crls.push(c);
        }
    }

    fn fetch_once(&mut self, url: &str) -> Option<Vec<u8>> {
        let f = self.fetch?;
        if self.fetched.iter().any(|u| u == url) {
            return None;
        }
        self.fetched.push(url.to_string());
        f.get(url)
    }

    /// Issuers of `cert` known so far, trusted roots first.
    fn issuers(&self, cert: &Cert) -> Vec<(Arc<Cert>, Option<RootSource>)> {
        let mut out: Vec<(Arc<Cert>, Option<RootSource>)> = self
            .trust
            .by_subject(&cert.issuer)
            .into_iter()
            .map(|(c, s)| (c, Some(s)))
            .collect();
        out.extend(
            self.pool
                .iter()
                .filter(|c| !c.same(cert) && x509::names_equal(&c.subject, &cert.issuer))
                .map(|c| (c.clone(), None)),
        );
        // Prefer a matching key identifier.
        if let Some(aki) = &cert.ext.authority_key_id {
            out.sort_by_key(|(c, _)| c.ext.subject_key_id.as_ref() != Some(aki));
        }
        out.into_iter().filter(|(c, _)| cert.signed_by(c)).collect()
    }

    /// The certificates in a downloaded "CA issuers" file: one DER or PEM
    /// certificate, or a PKCS #7 bundle.
    fn fetch_issuers(&mut self, cert: &Cert) -> bool {
        let mut added = false;
        for url in cert.ext.ca_issuer_urls.clone() {
            if self.fetch.is_none() {
                self.online_would_help = true;
                continue;
            }
            let Some(body) = self.fetch_once(&url) else {
                continue;
            };
            let mut certs = x509::parse_pem_or_der(&body);
            if certs.is_empty()
                && let Ok(sd) = super::cms::SignedData::parse(&body)
            {
                certs = sd.certs;
            }
            for c in certs {
                self.add_cert(c);
                added = true;
            }
        }
        added
    }

    /// Build and check the path for `leaf` at time `at`.
    pub fn path(&mut self, leaf: Arc<Cert>, at: i64, purpose: Purpose) -> Path {
        let mut certs: Vec<Arc<Cert>> = vec![leaf];
        let mut anchors: Vec<Option<RootSource>> = vec![None];
        let end = loop {
            let cur = certs.last().cloned().expect("path has a leaf");
            if let Some((_, src)) = self.trust.anchor_for(&cur) {
                *anchors.last_mut().expect("path") = Some(src);
                break PathEnd::Trusted(src);
            }
            if certs.len() > 12 {
                break PathEnd::Incomplete;
            }
            let mut found = self.issuers(&cur);
            if found.is_empty() && !cur.is_self_issued() && self.fetch_issuers(&cur) {
                found = self.issuers(&cur);
            }
            let next = found
                .into_iter()
                .find(|(c, _)| !certs.iter().any(|p| p.same(c)));
            match next {
                Some((c, src)) => {
                    certs.push(c);
                    anchors.push(src);
                    if let Some(src) = src {
                        break PathEnd::Trusted(src);
                    }
                }
                None if cur.is_self_issued() && cur.signed_by(&cur) => {
                    break PathEnd::UntrustedRoot;
                }
                None => break PathEnd::Incomplete,
            }
        };
        let n = certs.len();
        let mut out = Vec::with_capacity(n);
        for (i, cert) in certs.iter().enumerate() {
            let is_anchor = anchors[i].is_some();
            let mut problems = Vec::new();
            if at > cert.not_after && !is_anchor {
                problems.push(CertProblem::Expired);
            }
            if at < cert.not_before && !is_anchor {
                problems.push(CertProblem::NotYetValid);
            }
            if i > 0 {
                let ku_ok = cert
                    .ext
                    .key_usage
                    .is_none_or(|ku| ku & x509::KU_KEY_CERT_SIGN != 0);
                if !cert.is_ca() {
                    problems.push(CertProblem::NotACa);
                } else if !ku_ok {
                    problems.push(CertProblem::KeyUsage);
                }
            } else {
                let ku_ok = cert.ext.key_usage.is_none_or(|ku| {
                    ku & (x509::KU_DIGITAL_SIGNATURE | x509::KU_NON_REPUDIATION) != 0
                });
                let eku_ok = purpose != Purpose::TimeStamping
                    || cert.ext.eku.iter().any(|e| e == x509::EKU_TIME_STAMPING);
                if !ku_ok || !eku_ok {
                    problems.push(CertProblem::KeyUsage);
                }
            }
            for oid in &cert.ext.unknown_critical {
                problems.push(CertProblem::UnknownCritical(oid.clone()));
            }
            let revocation = if is_anchor || (i + 1 == n && cert.is_self_issued()) {
                Revocation::Root
            } else {
                match certs.get(i + 1) {
                    Some(issuer) => self.revocation(cert, issuer, at),
                    None => Revocation::Unknown,
                }
            };
            out.push(PathCert {
                cert: cert.clone(),
                revocation,
                problems,
            });
        }
        Path { certs: out, end }
    }

    /// Revocation of `cert`: embedded answers first, then online.
    fn revocation(&mut self, cert: &Cert, issuer: &Cert, at: i64) -> Revocation {
        if cert.ext.ocsp_no_check {
            return Revocation::NoCheck;
        }
        if let Some(r) = self.known_status(cert, issuer, false) {
            return judge(r, at);
        }
        let urls = cert.ext.ocsp_urls.len() + cert.ext.crl_urls.len();
        if urls == 0 {
            return Revocation::NotPublished;
        }
        if self.fetch.is_none() {
            self.online_would_help = true;
            return Revocation::Unknown;
        }
        let fetch = self.fetch.expect("checked above");
        for url in cert.ext.ocsp_urls.clone() {
            let key = format!("ocsp {url} {}", x509::colon_hex(&cert.serial));
            if self.fetched.contains(&key) {
                continue;
            }
            self.fetched.push(key);
            if let Some(body) = fetch.post(
                &url,
                "application/ocsp-request",
                &ocsp_request(cert, issuer),
            ) {
                self.add_ocsp(&body);
                if let Some(r) = self.known_status(cert, issuer, true) {
                    return judge(r, at);
                }
            }
        }
        for url in cert.ext.crl_urls.clone() {
            if let Some(body) = self.fetch_once(&url) {
                // CRLs are sometimes served as PEM.
                let der = if body.starts_with(b"-----BEGIN") {
                    pem_body(&body).unwrap_or(body)
                } else {
                    body
                };
                self.add_crl(&der);
                if let Some(r) = self.known_status(cert, issuer, true) {
                    return judge(r, at);
                }
            }
        }
        Revocation::Unknown
    }

    /// What the collected OCSP responses and CRLs say (OCSP first).
    fn known_status(&self, cert: &Cert, issuer: &Cert, online: bool) -> Option<Known> {
        for o in self.ocsps.iter().rev() {
            if let Some((Some(said), _)) = o.status_of(cert, issuer) {
                return Some(Known {
                    said,
                    by: RevSource::Ocsp,
                    online,
                });
            }
        }
        for c in self.crls.iter().rev() {
            if let Some((said, _)) = c.status_of(cert, issuer) {
                return Some(Known {
                    said,
                    by: RevSource::Crl,
                    online,
                });
            }
        }
        None
    }
}

struct Known {
    said: Said,
    by: RevSource,
    online: bool,
}

/// A revocation after the time the signature is checked at (its trusted
/// time stamp) doesn't count against it.
fn judge(k: Known, at: i64) -> Revocation {
    match k.said {
        Said::Good => Revocation::Good {
            by: k.by,
            online: k.online,
        },
        Said::Revoked { at: when } if when > at => Revocation::Good {
            by: k.by,
            online: k.online,
        },
        Said::Revoked { at: when } => Revocation::Revoked { at: when, by: k.by },
    }
}

fn pem_body(data: &[u8]) -> Option<Vec<u8>> {
    let text = String::from_utf8_lossy(data);
    let b64: String = text.lines().filter(|l| !l.starts_with("-----")).collect();
    x509::base64_decode(&b64)
}
