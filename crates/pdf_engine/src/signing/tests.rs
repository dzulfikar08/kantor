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

// ── signing as a batch step ────────────────────────────────────────────────

mod batch_step {
    use std::path::Path;

    use kantor_pdf::batch::{
        self as kb, Anchor, Applied, Batch, FileContext, Outcome, Plugin, Plugins, Prepared, Stamp,
        Step,
    };
    use kantor_pdf::testing::{sample_pdf, sample_pdf_encrypted, sample_png};

    use super::super::batch::{Appearance, Certificate, LevelName, SignPlugin, SignStep, why};
    use super::*;

    fn plugin(dir: &Path, net: Option<Arc<Net>>) -> Arc<SignPlugin> {
        let step = Certificate::File(format!("{DIR}/rsa.p12"));
        let id = super::super::batch::unlock(&step, dir, Some(PW)).unwrap();
        let mut p = SignPlugin::new(id, dir, None).trust(trust());
        if let Some(n) = net {
            p = p.online(n);
        }
        Arc::new(p)
    }

    fn sign_step(appearance: Appearance) -> SignStep {
        SignStep {
            certificate: Certificate::File(format!("{DIR}/rsa.p12")),
            appearance,
            reason: "Approved".into(),
            location: "Surabaya".into(),
            ..SignStep::default()
        }
    }

    fn stamp_step() -> Step {
        Step::Stamp(Stamp {
            image: "stamp.png".into(),
            pages: "first".into(),
            ..Stamp::default()
        })
    }

    fn prepared(dir: &Path, steps: Vec<Step>, p: Arc<SignPlugin>) -> kantor_pdf::Result<Prepared> {
        let mut plugins = Plugins::default();
        plugins.add(p);
        Prepared::new(
            &Batch {
                steps,
                ..Batch::default()
            },
            dir,
            None,
            &plugins,
        )
    }

    fn signed_bytes(run: &kb::FileRun) -> &[u8] {
        match &run.outcome {
            Outcome::Done { bytes, .. } => bytes,
            o => panic!("not done: {o:?} {:?}", run.notes),
        }
    }

    /// Every signature of `bytes` is intact and not invalid.
    fn all_good(bytes: &[u8], count: usize) {
        let r = check(bytes);
        assert_eq!(r.signatures.len(), count, "{}", sig::text_report(&r));
        for s in &r.signatures {
            assert_eq!(
                s.integrity,
                sig::Integrity::Intact,
                "{}",
                sig::text_report(&r)
            );
            assert_ne!(s.status, Status::Invalid, "{}", sig::text_report(&r));
        }
    }

    #[test]
    fn stamp_then_sign_many_files() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("stamp.png"), sample_png(80, 40, true)).unwrap();
        let other = identity("p384.p12", PW);
        // An unsigned file, one signed before (approval), one certified
        // allowing form filling and signing, one allowing no changes, an
        // encrypted one.
        let unsigned = sample_pdf(3);
        let signed_before = sign(&pdf(), &other, &opts(Place::Invisible, None)).unwrap();
        let mut c2 = opts(Place::Invisible, None);
        c2.certify = Some(2);
        let certified2 = sign(&pdf(), &other, &c2).unwrap();
        let mut c1 = opts(Place::Invisible, None);
        c1.certify = Some(1);
        let certified1 = sign(&pdf(), &other, &c1).unwrap();
        let encrypted = sample_pdf_encrypted(1, "owner", "");

        let boxed = sign_step(Appearance::Box {
            page: "last".into(),
            place: kb::Place::Anchor {
                anchor: Anchor::BottomRight,
                dx: 36.0,
                dy: 36.0,
            },
            width: 180.0,
            height: 60.0,
        });
        let p = prepared(d, vec![stamp_step(), boxed.to_step()], plugin(d, None)).unwrap();

        let run = kb::run_file(&p, "unsigned.pdf", &unsigned, None);
        assert!(run.notes.is_empty(), "{:?}", run.notes);
        let out = signed_bytes(&run);
        all_good(out, 1);
        let r = check(out);
        let s = &r.signatures[0];
        assert_eq!(s.reason.as_deref(), Some("Approved"));
        assert_eq!(s.page, Some(3), "on the last page");
        // The stamp is covered by the signature (it came before).
        assert!(s.covers_whole_file);

        let run = kb::run_file(&p, "signed.pdf", &signed_before, None);
        assert!(run.notes.is_empty(), "{:?}", run.notes);
        let out = signed_bytes(&run);
        assert!(
            out.starts_with(&signed_before),
            "earlier signature's bytes kept"
        );
        all_good(out, 2);

        // Certified P=2: the stamp is refused by the certification, and a
        // new visible box would change the pages: nothing written.
        let run = kb::run_file(&p, "certified2.pdf", &certified2, None);
        assert_eq!(run.outcome, Outcome::Skipped, "{:?}", run.notes);
        let reasons: Vec<String> = run.notes.iter().map(|n| n.reason.to_string()).collect();
        assert!(
            reasons.contains(&why::CERTIFIED_BOX.to_string()),
            "{reasons:?}"
        );

        // Certified P=1: no changes at all.
        let run = kb::run_file(&p, "certified1.pdf", &certified1, None);
        assert_eq!(run.outcome, Outcome::Skipped);
        assert!(
            run.notes
                .iter()
                .any(|n| n.reason.to_string() == why::NO_CHANGES),
            "{:?}",
            run.notes
        );

        // Encrypted: stamped (kept encrypted) but not signed.
        let run = kb::run_file(&p, "locked.pdf", &encrypted, None);
        assert!(
            run.notes
                .iter()
                .any(|n| n.reason.to_string() == why::ENCRYPTED),
            "{:?}",
            run.notes
        );

        // An invisible signature on the certified P=2 file is allowed.
        let invisible = sign_step(Appearance::Invisible);
        let p2 = prepared(d, vec![invisible.to_step()], plugin(d, None)).unwrap();
        let run = kb::run_file(&p2, "certified2.pdf", &certified2, None);
        let out = signed_bytes(&run);
        assert!(out.starts_with(&certified2));
        all_good(out, 2);

        // Certifying an already signed file is refused per file.
        let mut certify = sign_step(Appearance::Invisible);
        certify.certify = Some(3);
        let p3 = prepared(d, vec![certify.to_step()], plugin(d, None)).unwrap();
        let run = kb::run_file(&p3, "signed.pdf", &signed_before, None);
        assert_eq!(run.outcome, Outcome::Skipped);
        assert_eq!(run.notes[0].reason.to_string(), why::CERTIFY_SIGNED);
        let run = kb::run_file(&p3, "unsigned.pdf", &unsigned, None);
        let r = check(signed_bytes(&run));
        assert_eq!(r.signatures[0].certification, Some(3));
    }

    #[test]
    fn signs_existing_fields_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        // A file with a signed field "Approver"; sign it again by name.
        let id = identity("p256.p12", PW);
        let mut o = opts(
            Place::New {
                page: 0,
                rect: [72.0, 600.0, 272.0, 650.0],
            },
            None,
        );
        o.field_name = Some("Approver".into());
        let src = sign(&pdf(), &id, &o).unwrap();
        let p = prepared(
            d,
            vec![
                sign_step(Appearance::Field {
                    name: "Approver".into(),
                })
                .to_step(),
            ],
            plugin(d, None),
        )
        .unwrap();
        let run = kb::run_file(&p, "a.pdf", &src, None);
        assert_eq!(run.notes[0].reason.to_string(), why::FIELD_SIGNED);
        let p = prepared(
            d,
            vec![
                sign_step(Appearance::Field {
                    name: "Missing".into(),
                })
                .to_step(),
            ],
            plugin(d, None),
        )
        .unwrap();
        let run = kb::run_file(&p, "a.pdf", &src, None);
        assert_eq!(run.notes[0].reason.to_string(), why::NO_FIELD);
    }

    #[test]
    fn time_stamped_batch_signature() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let mut s = sign_step(Appearance::Invisible);
        s.level = LevelName::BLt;
        s.tsa = format!("{BASE}/tsa");
        // Offline: refused when the batch is prepared, not per file.
        let e = prepared(d, vec![s.to_step()], plugin(d, None))
            .err()
            .unwrap();
        assert!(e.to_string().contains("internet"), "{e}");
        let p = prepared(d, vec![s.to_step()], plugin(d, Some(Arc::new(Net::new())))).unwrap();
        let run = kb::run_file(&p, "a.pdf", &pdf(), None);
        let out = signed_bytes(&run);
        let r = check(out);
        assert!(r.signatures[0].timestamp.is_some());
        all_good(out, 1);
    }

    #[test]
    fn sign_must_be_last_and_passwords_stay_out() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::write(d.join("stamp.png"), sample_png(20, 20, false)).unwrap();
        let s = sign_step(Appearance::Invisible);
        let e = prepared(d, vec![s.to_step(), stamp_step()], plugin(d, None))
            .err()
            .unwrap();
        assert!(e.to_string().contains("must be the last step"), "{e}");
        // Two plugin steps: the sign step still has to be the last.
        let e = prepared(d, vec![s.to_step(), s.to_step()], plugin(d, None))
            .err()
            .unwrap();
        assert!(e.to_string().contains("last step"), "{e}");
        // The saved batch names the certificate, never its password.
        let b = Batch {
            steps: vec![stamp_step(), s.to_step()],
            ..Batch::default()
        };
        let json = b.to_json();
        assert!(json.contains("\"type\": \"sign\""), "{json}");
        assert!(json.contains("rsa.p12"), "{json}");
        assert!(!json.contains(PW), "{json}");
        let back = Batch::from_json(&json).unwrap();
        let (i, found) = super::super::batch::find(&back).unwrap();
        assert_eq!(i, 1);
        assert_eq!(found.unwrap(), s);
        // A password written into a batch file by hand is refused.
        let mut v = serde_json::to_value(&s).unwrap();
        v["password"] = "kantor-test".into();
        assert!(SignStep::from_value(&v).unwrap_err().contains("password"));
        // B-T without a server, bad certify levels.
        let mut bad = s.clone();
        bad.level = LevelName::BT;
        assert!(SignStep::from_value(&serde_json::to_value(&bad).unwrap()).is_err());
        let mut bad = s.clone();
        bad.certify = Some(4);
        assert!(SignStep::from_value(&serde_json::to_value(&bad).unwrap()).is_err());
        // Wrong password: an error when unlocking, before any file.
        let c = Certificate::File(format!("{DIR}/rsa.p12"));
        assert!(matches!(
            super::super::batch::unlock(&c, d, Some("nope")),
            Err(Error::WrongPassword)
        ));
        // The plugin interface the batch engine calls.
        let pl = plugin(d, None);
        let ctx = FileContext {
            name: "x.pdf",
            password: None,
            signed: false,
        };
        let opts = serde_json::to_value(&s).unwrap();
        assert!(matches!(
            pl.apply(&opts, &ctx, &pdf()),
            Ok(Applied::Changed(_))
        ));
    }

    #[test]
    fn upright_box_on_turned_pages() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let mut doc = lopdf::Document::load_mem(&sample_pdf(1)).unwrap();
        let page = *doc.get_pages().values().next().unwrap();
        doc.get_dictionary_mut(page).unwrap().set("Rotate", 90);
        let mut src = Vec::new();
        doc.save_to(&mut src).unwrap();
        let s = sign_step(Appearance::Box {
            page: "1".into(),
            place: kb::Place::Anchor {
                anchor: Anchor::BottomRight,
                dx: 36.0,
                dy: 36.0,
            },
            width: 180.0,
            height: 60.0,
        });
        let p = prepared(d, vec![s.to_step()], plugin(d, None)).unwrap();
        let run = kb::run_file(&p, "turned.pdf", &src, None);
        let out = signed_bytes(&run);
        all_good(out, 1);
        let pdf = kantor_pdf::Pdf::from_bytes(out, None).unwrap();
        let f = &kantor_pdf::sign::signature_fields(&pdf)[0];
        // Shown 180 × 60, so 60 wide and 180 high in user space.
        let (w, h) = (f.rect[2] - f.rect[0], f.rect[3] - f.rect[1]);
        assert!(
            (w - 60.0).abs() < 0.5 && (h - 180.0).abs() < 0.5,
            "{:?}",
            f.rect
        );
        // Bottom right as shown is the top right of the unturned page.
        assert!(f.rect[3] > 600.0 && f.rect[2] > 400.0, "{:?}", f.rect);
        // The widget records the turn (/MK /R) and its appearance turns back.
        let doc = lopdf::Document::load_mem(out).unwrap();
        let w = doc
            .objects
            .values()
            .filter_map(|o| o.as_dict().ok())
            .find(|d| d.get(b"FT").ok().and_then(|n| n.as_name().ok()) == Some(b"Sig"))
            .unwrap();
        let r = w.get(b"MK").unwrap().as_dict().unwrap().get(b"R").unwrap();
        assert_eq!(r.as_i64().unwrap(), 90);
    }
}
