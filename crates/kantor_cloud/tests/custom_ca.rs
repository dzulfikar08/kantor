//! A drive signed by an organisation's own CA: refused while the CA is
//! unknown, trusted once it is in the system's certificate store
//! (`SSL_CERT_FILE` here, which is how the Linux store is pointed
//! elsewhere). A throwaway CA and a local HTTPS WebDAV-ish server, no
//! network. Its own test binary: the store is read once per process.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::Arc;

use kantor_cloud::{Auth, Client};
use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair, KeyUsagePurpose};

/// A CA (PEM) and a server certificate for `localhost`/127.0.0.1 it signed.
fn throwaway_ca() -> (String, rustls::pki_types::CertificateDer<'static>, KeyPair) {
    let ca_key = KeyPair::generate().unwrap();
    let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    ca.distinguished_name
        .push(rcgen::DnType::CommonName, "Kantor Test Organisation CA");
    let ca_cert = ca.self_signed(&ca_key).unwrap();
    let key = KeyPair::generate().unwrap();
    let params =
        CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()]).unwrap();
    let cert = params.signed_by(&key, &ca_cert, &ca_key).unwrap();
    (ca_cert.pem(), cert.der().clone(), key)
}

/// Serve PROPFIND with a one-file listing over TLS on a free port; returns
/// the base URL.
fn serve(cert: rustls::pki_types::CertificateDer<'static>, key: &KeyPair) -> String {
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into());
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(vec![cert], key)
    .unwrap();
    let config = Arc::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut tcp) = stream else { continue };
            let config = config.clone();
            std::thread::spawn(move || {
                let Ok(mut conn) = rustls::ServerConnection::new(config) else {
                    return;
                };
                let mut tls = rustls::Stream::new(&mut conn, &mut tcp);
                let mut reader = BufReader::new(&mut tls);
                let mut len = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap_or(0);
                    }
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = vec![0; len];
                let _ = reader.read_exact(&mut body);
                let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response><d:response><d:href>/dav/Laporan.xlsx</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>5</d:getcontentlength><d:getetag>"e1"</d:getetag></d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>"#;
                let reply = format!(
                    "HTTP/1.1 207 Multi-Status\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{xml}",
                    xml.len()
                );
                let _ = tls.write_all(reply.as_bytes());
                let _ = tls.flush();
                conn.send_close_notify();
                let _ = conn.complete_io(&mut tcp);
            });
        }
    });
    format!("https://localhost:{port}/dav/")
}

#[test]
fn organisation_ca_from_the_system_store_is_trusted() {
    let (ca_pem, cert, key) = throwaway_ca();
    let base = serve(cert, &key);

    // The CA is nobody's yet: the public roots alone refuse the server.
    let roots = kantor_cloud::tls::merge_roots(std::iter::empty());
    let strict: ureq::Agent = ureq::Agent::config_builder()
        .tls_config(
            ureq::tls::TlsConfig::builder()
                .root_certs(ureq::tls::RootCerts::Specific(Arc::new(roots)))
                .build(),
        )
        .build()
        .into();
    let err = strict.get(&base).call().unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("certificate")
            || err.to_string().to_lowercase().contains("issuer"),
        "{err}"
    );

    // Installed in the system store: the cloud client trusts it. The store
    // is read once per process, so a child process runs with it.
    let dir = tempfile::tempdir().unwrap();
    let ca_file = dir.path().join("org-ca.pem");
    std::fs::write(&ca_file, ca_pem).unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "listing_with_the_system_store",
            "--ignored",
            "--nocapture",
        ])
        .env("SSL_CERT_FILE", &ca_file)
        .env("KANTOR_TEST_CA_URL", &base)
        .output()
        .unwrap();
    let text =
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success() && text.contains("1 passed"), "{text}");
}

/// Run by [`organisation_ca_from_the_system_store_is_trusted`] in a child
/// process whose `SSL_CERT_FILE` holds the throwaway CA.
#[test]
#[ignore]
fn listing_with_the_system_store() {
    let Ok(base) = std::env::var("KANTOR_TEST_CA_URL") else {
        return;
    };
    let client = Client::new(&base, Auth::None);
    let entries = client.list("").expect("listing over the organisation's CA");
    assert!(
        entries.iter().any(|e| e.name == "Laporan.xlsx"),
        "{entries:?}"
    );
    // Public roots are kept next to the system's.
    let count = match kantor_cloud::tls::root_certs() {
        ureq::tls::RootCerts::Specific(c) => c.len(),
        _ => 0,
    };
    assert!(count > webpki_root_certs_len(), "{count}");
}

fn webpki_root_certs_len() -> usize {
    kantor_cloud::tls::merge_roots(std::iter::empty()).len()
}
