//! Signed fixtures made by scripts/pdf/gen_signed_fixtures.py (synthetic
//! test PKI, pyHanko).

use super::*;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/signatures");

fn load(name: &str) -> Vec<u8> {
    std::fs::read(format!("{DIR}/{name}.pdf")).unwrap()
}

fn test_trust() -> TrustStore {
    let mut t = TrustStore::bundled();
    let pem = std::fs::read(format!("{DIR}/test-root.pem")).unwrap();
    for c in x509::parse_pem_or_der(&pem) {
        t.add(c);
    }
    t
}

fn check_with(name: &str, trust: TrustStore) -> Report {
    let opts = Options {
        trust,
        fetch: None,
        now: now(),
    };
    let r = verify(&load(name), &opts);
    assert!(r.error.is_none(), "{name}: {:?}", r.error);
    r
}

fn check(name: &str) -> Report {
    check_with(name, test_trust())
}

fn only(r: &Report) -> &Signature {
    assert_eq!(r.signatures.len(), 1, "{}", text_report(r));
    &r.signatures[0]
}

#[test]
fn unsigned_file_has_no_signatures() {
    let r = check("unsigned");
    assert!(!r.is_signed());
    assert!(!has_signatures(&load("unsigned")));
    assert!(has_signatures(&load("valid")));
}

#[test]
fn valid_cades_signature() {
    let r = check("valid");
    let s = only(&r);
    assert_eq!(s.status, Status::Valid, "{}", text_report(&r));
    assert_eq!(s.kind, SigKind::Cades);
    assert_eq!(s.signer_name(), "Budi Santoso");
    assert_eq!(s.organisation().as_deref(), Some("PT Contoh Uji"));
    assert_eq!(s.reason.as_deref(), Some("I approve this document"));
    assert_eq!(s.location.as_deref(), Some("Jakarta"));
    assert!(s.covers_whole_file);
    assert!(s.claimed_time.is_some());
    let path = s.path.as_ref().unwrap();
    assert_eq!(path.certs.len(), 3);
    assert_eq!(path.end, PathEnd::Trusted(RootSource::Extra));
    assert_eq!(path.certs[0].revocation, Revocation::NotPublished);
    assert!(r.all_valid());
}

#[test]
fn valid_pkcs7_with_ecdsa() {
    let r = check("valid_pkcs7");
    let s = only(&r);
    assert_eq!(s.kind, SigKind::Pkcs7Detached);
    assert_eq!(s.status, Status::Valid, "{}", text_report(&r));
    assert_eq!(s.signer_name(), "Siti Rahma");
}

#[test]
fn tampered_byte_is_invalid() {
    let r = check("tampered");
    let s = only(&r);
    assert_eq!(s.integrity, Integrity::DigestMismatch);
    assert_eq!(s.status, Status::Invalid);
    assert!(!r.all_valid());
}

#[test]
fn wrong_key_is_invalid() {
    let s = check("wrong_cert").signatures[0].clone();
    assert_eq!(s.integrity, Integrity::BadSignature);
    assert_eq!(s.status, Status::Invalid);
}

#[test]
fn untrusted_root_is_unknown() {
    let r = check("untrusted");
    let s = only(&r);
    assert_eq!(s.integrity, Integrity::Intact);
    assert_eq!(s.path.as_ref().unwrap().end, PathEnd::UntrustedRoot);
    assert_eq!(s.status, Status::Unknown);
    // The test root isn't trusted without --trust either.
    let r = check_with("valid", TrustStore::bundled());
    assert_eq!(only(&r).status, Status::Unknown);
}

#[test]
fn signature_time_stamp() {
    let r = check("timestamped");
    let s = only(&r);
    assert_eq!(s.status, Status::Valid, "{}", text_report(&r));
    let ts = s.timestamp.as_ref().unwrap();
    assert!(ts.trusted());
    assert_eq!(
        ts.tsa.as_ref().unwrap().subject_name().display_name(),
        "Kantor Test Time Stamping"
    );
    assert!(s.checked_at_timestamp);
    assert_eq!(s.checked_at, ts.time);
}

#[test]
fn document_time_stamp() {
    let r = check("doc_timestamp");
    assert_eq!(r.signatures.len(), 2, "{}", text_report(&r));
    let sig = &r.signatures[0];
    assert_eq!(sig.status, Status::Valid, "{}", text_report(&r));
    assert!(
        sig.changes.contains(&Change::TimestampAdded),
        "{:?}",
        sig.changes
    );
    let ts = &r.signatures[1];
    assert_eq!(ts.kind, SigKind::DocTimestamp);
    assert_eq!(ts.status, Status::Valid, "{}", text_report(&r));
    assert_eq!(ts.signer_name(), "Kantor Test Time Stamping");
}

#[test]
fn annotation_after_signing_is_allowed() {
    let r = check("annot_after");
    let s = only(&r);
    assert_eq!(s.status, Status::ValidWithChanges, "{}", text_report(&r));
    assert_eq!(
        s.changes,
        vec![Change::AnnotationAdded {
            kind: "Stamp".into(),
            page: Some(1)
        }]
    );
    assert_eq!(s.changes[0].msg().to_string(), "Stamp added on page 1");
    assert!(!s.covers_whole_file);
    assert_eq!(s.revision, 2);
    assert_eq!(s.revisions, 3);
}

#[test]
fn page_content_change_is_invalid() {
    let r = check("content_after");
    let s = only(&r);
    assert_eq!(s.status, Status::Invalid, "{}", text_report(&r));
    assert!(
        s.changes
            .contains(&Change::PageContentChanged { page: Some(1) }),
        "{:?}",
        s.changes
    );
}

#[test]
fn docmdp_no_changes_violated() {
    let r = check("docmdp_p1_violated");
    let s = only(&r);
    assert_eq!(s.certification, Some(1));
    assert_eq!(s.status, Status::Invalid, "{}", text_report(&r));
}

#[test]
fn docmdp_no_changes_allows_ltv() {
    let r = check("docmdp_p1_ltv");
    let s = only(&r);
    assert_eq!(s.changes, vec![Change::LtvAdded]);
    assert_eq!(s.status, Status::Valid, "{}", text_report(&r));
}

#[test]
fn form_filling_under_certification() {
    let r = check("form_filled");
    let s = only(&r);
    assert_eq!(s.certification, Some(2));
    assert_eq!(s.status, Status::ValidWithChanges, "{}", text_report(&r));
    assert!(s.changes.contains(&Change::FormFilled {
        field: "Name".into()
    }));
}

#[test]
fn two_signatures() {
    let r = check("two_signatures");
    assert_eq!(r.signatures.len(), 2);
    assert_eq!(r.signatures[0].status, Status::Valid, "{}", text_report(&r));
    assert!(r.signatures[0].changes.contains(&Change::SignatureAdded {
        field: "Signature2".into()
    }));
    assert_eq!(r.signatures[1].status, Status::Valid);
    assert!(r.signatures[1].covers_whole_file);
}

#[test]
fn revocation_from_embedded_ocsp() {
    let r = check("ltv_ocsp");
    let s = only(&r);
    assert_eq!(s.status, Status::Valid, "{}", text_report(&r));
    assert_eq!(
        s.path.as_ref().unwrap().certs[0].revocation,
        Revocation::Good {
            by: RevSource::Ocsp,
            online: false
        }
    );
}

#[test]
fn revocation_from_embedded_crl() {
    let r = check("ltv_crl");
    let s = only(&r);
    assert_eq!(s.status, Status::Valid, "{}", text_report(&r));
    assert_eq!(
        s.path.as_ref().unwrap().certs[0].revocation,
        Revocation::Good {
            by: RevSource::Crl,
            online: false
        }
    );
}

#[test]
fn revocation_unknown_offline() {
    let r = check("revocation_unknown");
    let s = only(&r);
    assert_eq!(s.status, Status::Unknown, "{}", text_report(&r));
    assert!(r.online_would_help);
    assert!(!r.checked_online);
}

#[test]
fn revoked_certificate_is_invalid() {
    let r = check("revoked");
    let s = only(&r);
    assert!(matches!(
        s.path.as_ref().unwrap().certs[0].revocation,
        Revocation::Revoked { .. }
    ));
    assert_eq!(s.status, Status::Invalid, "{}", text_report(&r));
}

/// A fetcher that fails everything: online checks that time out leave
/// revocation unknown, not failed.
struct Down;

impl Fetch for Down {
    fn get(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn post(&self, _: &str, _: &str, _: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

#[test]
fn unreachable_server_means_not_checked() {
    let opts = Options {
        trust: test_trust(),
        fetch: Some(&Down),
        now: now(),
    };
    let r = verify(&load("revocation_unknown"), &opts);
    let s = only(&r);
    assert_eq!(s.status, Status::Unknown);
    assert_eq!(
        s.path.as_ref().unwrap().certs[0].revocation,
        Revocation::Unknown
    );
}

#[test]
fn signed_version_is_the_signed_revision() {
    let bytes = load("annot_after");
    let r = check("annot_after");
    let signed = signed_version(&bytes, &r.signatures[0]);
    assert_eq!(&signed[..], &load("valid")[..]);
    let again = verify(
        &signed,
        &Options {
            trust: test_trust(),
            fetch: None,
            now: now(),
        },
    );
    assert_eq!(only(&again).status, Status::Valid);
}

#[test]
fn text_report_reads_well() {
    let t = text_report(&check("annot_after"));
    assert!(t.contains("Signed and all signatures are valid."), "{t}");
    assert!(t.contains("Stamp added on page 1"), "{t}");
    let t = text_report(&check("tampered"));
    assert!(t.contains("At least one signature has problems."), "{t}");
}
