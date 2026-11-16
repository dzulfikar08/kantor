//! Which certificates the cloud client trusts: the public CAs (Mozilla's
//! list, as before) **and** the operating system's certificate store, so a
//! drive or sign-in server with an organisation's own CA works once that CA
//! is installed on the computer, the way browsers and Office treat it:
//!
//! - Linux: the system bundle (`/etc/ssl/certs`, `update-ca-certificates`,
//!   `update-ca-trust`); `SSL_CERT_FILE` / `SSL_CERT_DIR` point elsewhere.
//! - macOS: the keychains' trusted roots (System, admin and user trust
//!   settings).
//! - Windows: the certificate store's trusted roots (including CAs pushed
//!   by Group Policy).
//!
//! Verification itself is unchanged (rustls with webpki); there is no way
//! to turn it off.

use std::sync::{Arc, OnceLock};

use ureq::tls::{Certificate, RootCerts, TlsConfig};

/// The TLS settings every cloud request uses.
pub fn tls_config() -> TlsConfig {
    TlsConfig::builder().root_certs(root_certs()).build()
}

/// Public roots plus the system's, loaded once per run.
pub fn root_certs() -> RootCerts {
    static ROOTS: OnceLock<Arc<Vec<Certificate<'static>>>> = OnceLock::new();
    let roots = ROOTS.get_or_init(|| {
        let system = rustls_native_certs::load_native_certs();
        for e in &system.errors {
            log::warn!("system certificate store: {e}");
        }
        let roots = merge_roots(system.certs.iter().map(|c| c.as_ref()));
        log::info!(
            "TLS roots: {} public, {} from the system",
            webpki_root_certs::TLS_SERVER_ROOT_CERTS.len(),
            system.certs.len()
        );
        Arc::new(roots)
    });
    RootCerts::Specific(roots.clone())
}

/// Mozilla's public roots followed by `system` (DER), duplicates dropped.
pub fn merge_roots<'a>(system: impl IntoIterator<Item = &'a [u8]>) -> Vec<Certificate<'static>> {
    let mut seen = std::collections::HashSet::new();
    webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .map(|c| c.as_ref())
        .chain(system)
        .filter(|der| {
            seen.insert(
                ring::digest::digest(&ring::digest::SHA256, der)
                    .as_ref()
                    .to_vec(),
            )
        })
        .map(|der| Certificate::from_der(der).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_roots_are_kept_and_duplicates_dropped() {
        let public = webpki_root_certs::TLS_SERVER_ROOT_CERTS.len();
        assert!(public > 100);
        let first = webpki_root_certs::TLS_SERVER_ROOT_CERTS[0].as_ref();
        // A system store that repeats a public root adds nothing.
        assert_eq!(merge_roots([first]).len(), public);
        assert_eq!(merge_roots(std::iter::empty()).len(), public);
    }
}
