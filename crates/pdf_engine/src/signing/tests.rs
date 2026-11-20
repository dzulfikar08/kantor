//! Signing with the synthetic test PKI from
//! scripts/pdf/gen_signing_fixtures.py, checked with Kantor's own checker.
//! The TSA, OCSP responder and CRL server are answered from memory.

use std::sync::Arc;

use kantor_pdf::sign::{Look, Place};

use super::asn1::{generalized_time, int, oid, seq};
use super::*;
use crate::signatures::der::{self, Tlv};
use crate::signatures::{self as sig, Fetch, Status, TrustStore, x509};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/signing");
const PW: &str = "kantor-test";
const BASE: &str = "http://127.0.0.1:8977";

fn file(name: &str) -> Vec<u8> {
    std::fs::read(format!("{DIR}/{name}")).unwrap()
}

fn identity(name: &str, pw: &str) -> Identity {
    Identity::from_p12(&file(name), pw, std::path::Path::new(name)).unwrap()
}

fn trust() -> TrustStore {
    let mut t = TrustStore::empty();
    for c in x509::parse_pem_or_der(&file("root.pem")) {
        t.add(c);
    }
    t
}

fn pdf() -> Vec<u8> {
    kantor_pdf::testing::sample_pdf(2)
}

/// The test network: TSA, OCSP and CRLs.
struct Net {
    tsa: Identity,
}

impl Net {
    fn new() -> Net {
        Net {
            tsa: identity("tsa.p12", PW),
        }
    }

    fn timestamp(&self, req: &[u8]) -> Vec<u8> {
        let t = Tlv::parse(req).unwrap();
        let mut r = t.reader();
        r.read_tag(der::INTEGER).unwrap();
        let imprint = r.read_tag(der::SEQUENCE).unwrap().raw.to_vec();
        let nonce = r.read_opt(der::INTEGER).unwrap().map(|n| n.raw.to_vec());
        let mut parts = vec![
            int(1),
            oid("1.3.6.1.4.1.99999.1"),
            imprint,
            int(42),
            generalized_time(sig::now()),
        ];
        if let Some(n) = nonce {
            parts.push(n);
        }
        let info = seq(&parts.iter().map(Vec::as_slice).collect::<Vec<_>>());
        let ct = crate::signatures::cms::CT_TST_INFO;
        let signed = cms::sign_attributes(
            self.tsa.key.as_ref(),
            &self.tsa.cert,
            ct,
            &Hash::Sha256.digest(&info),
            Hash::Sha256,
            false,
            Some(sig::now()),
        )
        .unwrap();
        let chain: Vec<&x509::Cert> = self.tsa.chain.iter().map(|c| c.as_ref()).collect();
        let token = cms::assemble(&signed, &self.tsa.cert, &chain, ct, Some(&info), vec![]);
        seq(&[&seq(&[&int(0)]), &token])
    }
}

impl Fetch for Net {
    fn get(&self, url: &str) -> Option<Vec<u8>> {
        let name = url.strip_prefix(BASE)?.trim_start_matches('/');
        std::fs::read(format!("{DIR}/{name}")).ok()
    }

    fn post(&self, url: &str, _: &str, body: &[u8]) -> Option<Vec<u8>> {
        match url.strip_prefix(BASE)? {
            "/tsa" => Some(self.timestamp(body)),
            "/ocsp" => Some(file("ocsp-rsa.der")),
            _ => None,
        }
    }
}

fn check(bytes: &[u8]) -> sig::Report {
    let r = sig::verify(
        bytes,
        &sig::Options {
            trust: trust(),
            fetch: None,
            now: sig::now(),
        },
    );
    assert!(r.error.is_none(), "{:?}", r.error);
    r
}

fn opts<'a>(place: Place, net: Option<&'a Net>) -> Options<'a> {
    let mut o = Options::new(place);
    o.trust = trust();
    o.fetch = net.map(|n| n as &dyn Fetch);
    o.reason = Some("I approve this document".into());
    o.location = Some("Jakarta".into());
    o
}

#[test]
fn p12_files_open_in_every_encoding() {
    for (f, pw, name) in [
        ("rsa.p12", PW, "Budi Santoso"),
        ("rsa-3des.p12", PW, "Budi Santoso"),
        ("rsa-rc2.p12", PW, "Budi Santoso"),
        ("p256.p12", PW, "Siti Rahayu"),
        ("p384.p12", PW, "Agus Wijaya"),
        ("empty.p12", "", "Siti Rahayu"),
    ] {
        let id = identity(f, pw);
        assert_eq!(id.name(), name, "{f}");
        assert!(!id.chain.is_empty(), "{f}");
    }
    assert!(matches!(
        Identity::from_p12(&file("rsa.p12"), "wrong", std::path::Path::new("x")),
        Err(Error::WrongPassword)
    ));
    assert!(matches!(
        Identity::from_p12(&file("rsa-rc2.p12"), "", std::path::Path::new("x")),
        Err(Error::WrongPassword)
    ));
}

#[test]
fn basic_signatures_verify_for_every_key_type() {
    for (f, pss) in [
        ("rsa.p12", false),
        ("rsa.p12", true),
        ("p256.p12", false),
        ("p384.p12", false),
    ] {
        let id = identity(f, PW);
        let mut o = opts(Place::Invisible, None);
        o.pss = pss;
        let src = pdf();
        let out = sign(&src, &id, &o).unwrap();
        assert!(out.starts_with(&src));
        let r = check(&out);
        assert_eq!(r.signatures.len(), 1);
        let s = &r.signatures[0];
        assert_eq!(s.status, Status::Unknown, "{f}: {}", sig::text_report(&r));
        assert_eq!(s.integrity, sig::Integrity::Intact, "{f} pss={pss}");
        assert!(s.covers_whole_file);
        assert_eq!(s.kind, sig::SigKind::Cades);
        assert!(s.path.as_ref().unwrap().trusted());
        assert_eq!(s.reason.as_deref(), Some("I approve this document"));
    }
}

#[test]
fn offline_requests_for_time_stamps_fail() {
    let id = identity("rsa.p12", PW);
    let mut o = opts(Place::Invisible, None);
    o.level = Level::T;
    o.tsa = Some(format!("{BASE}/tsa"));
    assert!(matches!(sign(&pdf(), &id, &o), Err(Error::Offline)));
}

#[test]
fn time_stamped_visible_signature() {
    let net = Net::new();
    let id = identity("p256.p12", PW);
    let mut o = opts(
        Place::New {
            page: 1,
            rect: [72.0, 72.0, 272.0, 132.0],
        },
        Some(&net),
    );
    o.level = Level::T;
    o.tsa = Some(format!("{BASE}/tsa"));
    o.look = Some(Look {
        lines: vec!["Digitally signed by Siti Rahayu".into(), "Jakarta".into()],
        art: Some(kantor_pdf::annot::Art::Image(Arc::new(
            kantor_pdf::testing::sample_png(40, 20, true),
        ))),
    });
    let out = sign(&pdf(), &id, &o).unwrap();
    let r = check(&out);
    let s = &r.signatures[0];
    assert_eq!(s.page, Some(2), "the checker counts pages from 1");
    let ts = s.timestamp.as_ref().expect("time stamp");
    assert_eq!(ts.integrity, sig::Integrity::Intact);
    assert!(
        ts.path.as_ref().unwrap().trusted(),
        "{}",
        sig::text_report(&r)
    );
    assert!(s.checked_at_timestamp);
}

#[test]
fn long_term_validation_and_archive_time_stamp() {
    let net = Net::new();
    let id = identity("rsa.p12", PW);
    let mut o = opts(Place::Invisible, Some(&net));
    o.level = Level::Lta;
    o.tsa = Some(format!("{BASE}/tsa"));
    let src = pdf();
    let out = sign(&src, &id, &o).unwrap();
    assert!(out.starts_with(&src));
    let r = check(&out);
    assert_eq!(r.signatures.len(), 2, "{}", sig::text_report(&r));
    let s = &r.signatures[0];
    // Revocation answered offline from the DSS.
    assert_eq!(s.status, Status::Valid, "{}", sig::text_report(&r));
    assert!(!s.path.as_ref().unwrap().revocation_unknown());
    let doc_ts = &r.signatures[1];
    assert_eq!(doc_ts.kind, sig::SigKind::DocTimestamp);
    assert_eq!(doc_ts.integrity, sig::Integrity::Intact);
    let pdf = kantor_pdf::Pdf::from_bytes(&out, None).unwrap();
    let doc = pdf.document();
    assert!(doc.catalog().unwrap().has(b"DSS"));
}

#[test]
fn certification_then_approval_signature() {
    let id = identity("rsa.p12", PW);
    let mut o = opts(Place::Invisible, None);
    o.certify = Some(2);
    let once = sign(&pdf(), &id, &o).unwrap();
    let r = check(&once);
    assert_eq!(r.signatures[0].certification, Some(2));
    // A second certification is refused; an approval signature is fine.
    assert!(sign(&once, &id, &o).is_err());
    let other = identity("p384.p12", PW);
    let mut o2 = opts(
        Place::New {
            page: 0,
            rect: [300.0, 72.0, 500.0, 132.0],
        },
        None,
    );
    o2.look = Some(Look {
        lines: vec!["Agus Wijaya".into()],
        art: None,
    });
    // A new visible field would change the certified page: refused.
    assert!(sign(&once, &other, &o2).is_err());
    let o2 = opts(Place::Invisible, None);
    let twice = sign(&once, &other, &o2).unwrap();
    assert!(twice.starts_with(&once));
    let r = check(&twice);
    assert_eq!(r.signatures.len(), 2);
    for s in &r.signatures {
        assert_eq!(s.integrity, sig::Integrity::Intact);
        assert_ne!(s.status, Status::Invalid, "{}", sig::text_report(&r));
    }
}

#[test]
fn signs_an_existing_empty_field() {
    // Make an empty signature field by preparing one and keeping only
    // the field (sign, then sign the copy's field by name).
    let id = identity("rsa.p12", PW);
    let src = pdf();
    let mut o = opts(
        Place::New {
            page: 0,
            rect: [72.0, 600.0, 272.0, 650.0],
        },
        None,
    );
    o.field_name = Some("Approver".into());
    let out = sign(&src, &id, &o).unwrap();
    let fields =
        kantor_pdf::sign::signature_fields(&kantor_pdf::Pdf::from_bytes(&out, None).unwrap());
    assert_eq!(fields[0].name, "Approver");
    assert!(fields[0].signed);
    // Signing the same (now signed) field again is refused.
    let o2 = opts(Place::Field("Approver".into()), None);
    assert!(sign(&out, &id, &o2).is_err());
    let o3 = opts(Place::Field("Missing".into()), None);
    assert!(sign(&out, &id, &o3).is_err());
}

#[test]
fn expired_certificate_is_refused() {
    let id = identity("rsa.p12", PW);
    let mut o = opts(Place::Invisible, None);
    o.now = id.cert.not_after + 10;
    assert!(matches!(sign(&pdf(), &id, &o), Err(Error::Key(_))));
}

#[test]
fn pkcs11_module_loads_and_lists() {
    // p11-kit's trust module ships with most Linux systems: it has
    // certificates (the system roots) but no private keys.
    let path = "/usr/lib/pkcs11/p11-kit-trust.so";
    if !std::path::Path::new(path).exists() {
        return;
    }
    let found = stores::token_candidates(path).unwrap();
    // Roots are CAs, so nothing is offered for signing.
    assert!(found.is_empty());
    assert!(stores::token_candidates("/nonexistent/module.so").is_err());
}
