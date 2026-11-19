//! Trusted roots and the network.
//!
//! A signer's chain is trusted when it ends at a root from the operating
//! system's trust store or from Kantor's small bundled list of national
//! signing roots the OS stores often lack (Root CA Indonesia DS G1, under
//! which Indonesian e-meterai and certified e-signatures are issued).
//! Adobe's AATL and the EU trusted lists are not bundled (yet).

use std::io::Read as _;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use super::x509::{self, Cert};

/// A bundled root.
pub struct BundledRoot {
    pub name: &'static str,
    pub pem: &'static [u8],
    /// SHA-256 of the DER, checked by a test against the publisher's value.
    pub sha256: &'static str,
    pub source: &'static str,
}

pub const BUNDLED: &[BundledRoot] = &[BundledRoot {
    name: "Root CA Indonesia DS G1",
    pem: include_bytes!("../../assets/roots/RootCAIndonesiaDSG1.crt"),
    sha256: "B5:F3:66:72:FD:7C:FD:40:1E:01:EF:64:0B:2F:E6:1F:81:6F:32:F4:47:B2:80:B7:6F:53:62:44:CF:42:FC:DF",
    source: "https://www.rootca.id/repository/01.root/RootCAIndonesiaDSG1.crt",
}];

/// Where a trusted root came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootSource {
    System,
    Bundled,
    /// Added by the caller (`kantor pdf verify --trust`, tests).
    Extra,
}

/// The roots a check trusts.
#[derive(Clone, Default)]
pub struct TrustStore {
    roots: Vec<(Arc<Cert>, RootSource)>,
}

impl TrustStore {
    /// No roots at all.
    pub fn empty() -> TrustStore {
        TrustStore::default()
    }

    /// The system's roots and Kantor's bundled ones.
    pub fn standard() -> TrustStore {
        let mut s = TrustStore {
            roots: system_roots()
                .iter()
                .map(|c| (c.clone(), RootSource::System))
                .collect(),
        };
        s.roots.extend(
            bundled_roots()
                .iter()
                .map(|c| (c.clone(), RootSource::Bundled)),
        );
        s
    }

    /// Only Kantor's bundled roots (tests: independent of the machine).
    pub fn bundled() -> TrustStore {
        TrustStore {
            roots: bundled_roots()
                .iter()
                .map(|c| (c.clone(), RootSource::Bundled))
                .collect(),
        }
    }

    pub fn add(&mut self, cert: Cert) {
        self.roots.push((Arc::new(cert), RootSource::Extra));
    }

    /// The trusted root `cert` is (same subject and key), if any.
    pub fn anchor_for(&self, cert: &Cert) -> Option<(Arc<Cert>, RootSource)> {
        self.roots
            .iter()
            .find(|(r, _)| r.same(cert))
            .or_else(|| self.roots.iter().find(|(r, _)| r.same_entity(cert)))
            .cloned()
    }

    /// Roots whose subject is `issuer_name`.
    pub fn by_subject(&self, issuer_name: &[u8]) -> Vec<(Arc<Cert>, RootSource)> {
        self.roots
            .iter()
            .filter(|(r, _)| x509::names_equal(&r.subject, issuer_name))
            .cloned()
            .collect()
    }
}

fn bundled_roots() -> &'static [Arc<Cert>] {
    static ROOTS: OnceLock<Vec<Arc<Cert>>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        BUNDLED
            .iter()
            .flat_map(|b| x509::parse_pem_or_der(b.pem))
            .map(Arc::new)
            .collect()
    })
}

/// The operating system's trusted roots, read once.
fn system_roots() -> &'static [Arc<Cert>] {
    static ROOTS: OnceLock<Vec<Arc<Cert>>> = OnceLock::new();
    ROOTS.get_or_init(|| {
        let found = rustls_native_certs::load_native_certs();
        found
            .certs
            .iter()
            .filter_map(|c| Cert::parse(c.as_ref()).ok())
            .map(Arc::new)
            .collect()
    })
}

/// Fetching certificates, OCSP responses and CRLs. Only used when the
/// user allowed checking online.
pub trait Fetch: Sync {
    fn get(&self, url: &str) -> Option<Vec<u8>>;
    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Option<Vec<u8>>;
}

/// HTTP with short timeouts: a slow server means "revocation not
/// checked", not a long wait.
pub struct Http {
    agent: ureq::Agent,
}

const MAX_BODY: u64 = 20 * 1024 * 1024;

impl Http {
    pub fn new() -> Http {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .max_redirects(3)
            .timeout_connect(Some(Duration::from_secs(8)))
            .timeout_global(Some(Duration::from_secs(15)))
            .user_agent(format!(
                "Kantor/{} ({}; {})",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH
            ))
            .build();
        Http {
            agent: config.into(),
        }
    }

    fn read(response: ureq::http::Response<ureq::Body>) -> Option<Vec<u8>> {
        if response.status().as_u16() != 200 {
            return None;
        }
        let mut body = Vec::new();
        response
            .into_body()
            .into_reader()
            .take(MAX_BODY)
            .read_to_end(&mut body)
            .ok()?;
        Some(body)
    }
}

impl Default for Http {
    fn default() -> Self {
        Http::new()
    }
}

impl Fetch for Http {
    fn get(&self, url: &str) -> Option<Vec<u8>> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return None;
        }
        Http::read(self.agent.get(url).call().ok()?)
    }

    fn post(&self, url: &str, content_type: &str, body: &[u8]) -> Option<Vec<u8>> {
        if !url.starts_with("http://") && !url.starts_with("https://") {
            return None;
        }
        Http::read(
            self.agent
                .post(url)
                .header("Content-Type", content_type)
                .send(body)
                .ok()?,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_root_fingerprints_match_the_publishers() {
        for b in BUNDLED {
            let certs = x509::parse_pem_or_der(b.pem);
            assert_eq!(certs.len(), 1, "{}", b.name);
            assert_eq!(x509::colon_hex(&certs[0].sha256()), b.sha256, "{}", b.name);
            assert_eq!(certs[0].subject_name().get("CN"), Some(b.name));
            assert!(certs[0].is_ca());
        }
    }
}
