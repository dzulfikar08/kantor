//! The providers against local mocks of their documented APIs, and the
//! results checked with Kantor's signature checker.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kantor_core::settings::EsignProviderSettings;
use kantor_pdf::batch::{self, Batch, Outcome, Plugins};
use kantor_pdf::sign::Place;
use pdf_engine::signatures::{self as sigs, Integrity, Status, TrustStore, x509};
use pdf_engine::signing::Identity;

use crate::check::Kept;
use crate::emeterai::{self, EMeteraiProvider, StampRequest};
use crate::mock::{RsaKey, Server};
use crate::providers::{mekari, vida, xignature};
use crate::signer::{self, RemoteSigner, SignRequest, Signer};
use crate::{Account, Credentials, Error, http};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn meterai_id() -> Identity {
    let p = fixture("meterai.p12");
    Identity::from_p12(&std::fs::read(&p).unwrap(), "kantor-test", &p).unwrap()
}

fn trust() -> TrustStore {
    let mut t = TrustStore::empty();
    for c in x509::parse_pem_or_der(&std::fs::read(fixture("root.pem")).unwrap()) {
        t.add(c);
    }
    t
}

/// `KANTOR_ESIGN_OUT=DIR` keeps results for checking with pyHanko or
/// pdfsig (see docs/decisions/0004-esign-providers.md).
fn keep(name: &str, pdf: &[u8]) {
    if let Ok(dir) = std::env::var("KANTOR_ESIGN_OUT") {
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(PathBuf::from(dir).join(name), pdf).unwrap();
    }
}

fn verify(pdf: &[u8]) -> sigs::Report {
    let mut o = sigs::Options::standard();
    o.trust = trust();
    sigs::verify(pdf, &o)
}

fn account(info: &'static crate::Info, url: &str, creds: &[(&str, &str)]) -> Account {
    let mut c = Credentials::default();
    for (k, v) in creds {
        c.set(k, v);
    }
    Account {
        info,
        settings: EsignProviderSettings {
            id: info.id.into(),
            allow_network: true,
            sandbox: true,
            base_url: url.into(),
            doc_type: String::new(),
        },
        creds: c,
        transport: Arc::new(http::Ureq::new()),
    }
}

/// The mock e-meterai: Peruri's stamp as a signature by the test identity.
fn vida_server(pending: usize) -> Server {
    let id = Arc::new(meterai_id());
    let stamp = Arc::new(move |pdf: &[u8], page: u32, x: f32, y: f32| {
        crate::mock::stamp_with(&id, pdf, page, [x, y, x + 60.0, y + 60.0])
    });
    Server::start(vida::mock("kantor-client", "s3cret", stamp, pending)).unwrap()
}

fn vida_client(url: &str, secret: &str) -> vida::Vida {
    let mut v = vida::Vida::new(account(
        &vida::INFO,
        url,
        &[("client_id", "kantor-client"), ("client_secret", secret)],
    ));
    v.poll_every = Duration::from_millis(10);
    v
}

#[test]
fn vida_stamps_and_the_stamp_verifies() {
    let server = vida_server(2);
    let v = vida_client(&server.url, "s3cret");
    assert_eq!(v.balance().unwrap(), Some(5));
    let types = v.doc_types().unwrap();
    assert!(
        types
            .iter()
            .any(|t| t.code == "VD010" && t.name == "Surat Perjanjian")
    );
    let pdf = kantor_pdf::testing::sample_pdf(2);
    let req = StampRequest::new("kontrak.pdf", 1, emeterai::box_at(300.0, 200.0), "VD010");
    let (out, kept) = emeterai::stamp_checked(&v, &pdf, &req, false).unwrap();
    assert_eq!(kept, Kept::Unsigned);
    assert_eq!(out.serial.as_deref(), Some("08MO5DUHLD0GUV2N0000A9"));
    assert!(
        out.pdf.starts_with(&pdf),
        "stamped as an incremental update"
    );
    // What was sent, as VIDA documents it: strings, 1-based page, the
    // lower-left corner in points, a UUID refNum, the PDF in base64.
    let up = &server.calls("/api/v1/emeterai/upload")[0];
    let b = up.json();
    assert_eq!(b["codeDocument"], "VD010");
    assert_eq!(b["page"], "2");
    assert_eq!(b["llx"], "265");
    assert_eq!(b["lly"], "165");
    assert_eq!(b["refNum"].as_str().unwrap().len(), 36);
    assert_eq!(up.header("x-partner-id"), Some("kantor-client"));
    let polls = server
        .requests()
        .into_iter()
        .filter(|r| r.route().starts_with("/api/v1/emeterai/docstamp/"))
        .count();
    assert_eq!(polls, 3, "two pending answers, then the document");
    // One token for all calls.
    assert_eq!(
        server
            .calls("/auth/realms/vida/protocol/openid-connect/token")
            .len(),
        1
    );
    // The stamp is a valid signature by the (test) e-meterai certificate.
    let r = verify(&out.pdf);
    assert_eq!(r.signatures.len(), 1);
    let s = &r.signatures[0];
    assert_eq!(s.integrity, Integrity::Intact);
    assert_eq!(s.status, Status::Valid, "{:?}", s.notes);
    assert_eq!(s.page, Some(2), "1-based");
    assert_eq!(s.signer_name(), "Mock e-Meterai Stamp");
}

#[test]
fn vida_errors_are_clear() {
    let server = vida_server(0);
    let bad = vida_client(&server.url, "wrong");
    assert!(
        matches!(bad.balance(), Err(Error::Auth(_))),
        "{:?}",
        bad.balance()
    );
    let v = vida_client(&server.url, "s3cret");
    let pdf = kantor_pdf::testing::sample_pdf(1);
    let req = StampRequest::new("a.pdf", 0, emeterai::box_at(300.0, 200.0), "XX01");
    let e = emeterai::stamp_checked(&v, &pdf, &req, false).unwrap_err();
    assert!(e.to_string().contains("VIDA code 24"), "{e}");
    // Use up the quota.
    for _ in 0..5 {
        let req = StampRequest::new("a.pdf", 0, emeterai::box_at(300.0, 200.0), "VD001");
        emeterai::stamp_checked(&v, &pdf, &req, false).unwrap();
    }
    let req = StampRequest::new("a.pdf", 0, emeterai::box_at(300.0, 200.0), "VD001");
    assert!(matches!(
        emeterai::stamp_checked(&v, &pdf, &req, false),
        Err(Error::Quota(_))
    ));
    // A box outside the size rules or on a missing page is refused before
    // anything is sent.
    let before = server.requests().len();
    let tiny = StampRequest::new("a.pdf", 0, [10.0, 10.0, 30.0, 30.0], "VD001");
    assert!(matches!(
        emeterai::stamp_checked(&v, &pdf, &tiny, false),
        Err(Error::Pdf(_))
    ));
    let page9 = StampRequest::new("a.pdf", 8, emeterai::box_at(300.0, 200.0), "VD001");
    assert!(matches!(
        emeterai::stamp_checked(&v, &pdf, &page9, false),
        Err(Error::Pdf(_))
    ));
    assert_eq!(server.requests().len(), before);
}

#[test]
fn nothing_goes_online_unless_allowed() {
    let server = vida_server(0);
    let mut a = account(
        &vida::INFO,
        &server.url,
        &[("client_id", "kantor-client"), ("client_secret", "s3cret")],
    );
    a.settings.allow_network = false;
    let v = vida::Vida::new(a.clone());
    assert!(matches!(v.balance(), Err(Error::NetworkOff(_))));
    let mut missing = a.clone();
    missing.settings.allow_network = true;
    missing.creds = Credentials::default();
    assert!(matches!(
        vida::Vida::new(missing).balance(),
        Err(Error::NotConfigured(_))
    ));
    assert!(server.requests().is_empty());
    // Stubs say why they can't be used.
    let stub = account(crate::find("peruri").unwrap(), &server.url, &[]);
    let e = crate::providers::emeterai(stub).err().unwrap();
    assert!(
        matches!(e, Error::NotImplemented(ref w) if w.contains("isn't public")),
        "{e}"
    );
    // Production VIDA needs the addresses VIDA gives.
    let mut prod = account(
        &vida::INFO,
        "",
        &[("client_id", "kantor-client"), ("client_secret", "s3cret")],
    );
    prod.settings.sandbox = false;
    assert!(matches!(
        vida::Vida::new(prod).balance(),
        Err(Error::NotConfigured(_))
    ));
}

fn mekari_server(pending: usize) -> Server {
    let id = Arc::new(meterai_id());
    let stamp = Arc::new(move |pdf: &[u8], page: u32, b: [f32; 6]| {
        // Back from the top-left canvas to user space (unturned pages).
        let [x, y, w, h, _, ch] = b;
        crate::mock::stamp_with(&id, pdf, page, [x, ch - y - h, x + w, ch - y])
    });
    Server::start(mekari::mock("hmac-id", "hmac-secret", stamp, pending)).unwrap()
}

#[test]
fn mekari_stamps_with_hmac_and_polls() {
    let server = mekari_server(1);
    let url = format!("{}/v2/esign-hmac/v1", server.url);
    let mut m = mekari::Mekari::new(account(
        &mekari::INFO,
        &url,
        &[("client_id", "hmac-id"), ("client_secret", "hmac-secret")],
    ));
    m.poll_every = Duration::from_millis(10);
    m.test().unwrap();
    let pdf = kantor_pdf::testing::sample_pdf(1);
    let rect = [400.0, 100.0, 470.0, 170.0];
    let req = StampRequest::new("inv.pdf", 0, rect, mekari::ACCOUNT_DOC_TYPE);
    let (out, _) = emeterai::stamp_checked(&m, &pdf, &req, false).unwrap();
    let sent = server.calls("/v2/esign-hmac/v1/documents/stamp")[0].json();
    let a = &sent["annotations"][0];
    assert_eq!(a["type_of"], "meterai");
    assert_eq!(a["page"], 1);
    assert_eq!(a["position_x"], 400.0);
    // sample pages are 800 pt tall: top edge 170 → 630 from the top.
    assert_eq!(a["canvas_height"], 800.0);
    assert_eq!(a["position_y"], 630.0);
    assert_eq!(a["element_width"], 70.0);
    assert_eq!(sent["filename"], "inv.pdf");
    let r = verify(&out.pdf);
    assert_eq!(r.signatures[0].status, Status::Valid);
    // Wrong secret: refused (the mock checks the HMAC).
    let bad = mekari::Mekari::new(account(
        &mekari::INFO,
        &url,
        &[("client_id", "hmac-id"), ("client_secret", "nope")],
    ));
    assert!(matches!(bad.test(), Err(Error::Auth(_))));
}

#[test]
fn mekari_canvas_follows_turned_pages() {
    let mut doc = kantor_pdf::Pdf::from_bytes(&kantor_pdf::testing::sample_pdf(1), None).unwrap();
    kantor_pdf::rotate(&mut doc, &kantor_pdf::PageRanges::parse("1").unwrap(), 90).unwrap();
    let pdf = doc.to_bytes().unwrap();
    // Page 1 is 601 × 800; turned, it shows 800 wide and 601 tall.
    let b = mekari::canvas_box(&pdf, 0, [0.0, 0.0, 70.0, 70.0]).unwrap();
    assert_eq!((b[4], b[5]), (800.0, 601.0));
    assert_eq!((b[2], b[3]), (70.0, 70.0));
    // User-space origin (bottom-left) ends up at the shown top-left.
    assert_eq!((b[0], b[1]), (0.0, 0.0));
}

#[test]
fn hmac_signature_matches_mekari_recipe() {
    // signing string "date: …\nGET /path HTTP/1.1", HMAC-SHA256, base64.
    let a = mekari::hmac_authorization(
        "id",
        "secret",
        "Thu, 20 Nov 2026 07:00:00 GMT",
        "GET",
        "/v2/esign-hmac/v1/profile",
    );
    assert!(a.starts_with(
        "hmac username=\"id\", algorithm=\"hmac-sha256\", headers=\"date request-line\", signature=\""
    ));
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"secret");
    let sig = crate::util::b64(
        ring::hmac::sign(
            &key,
            b"date: Thu, 20 Nov 2026 07:00:00 GMT\nGET /v2/esign-hmac/v1/profile HTTP/1.1",
        )
        .as_ref(),
    );
    assert!(a.ends_with(&format!("signature=\"{sig}\"")));
}

fn xignature_server(garbage: bool) -> Server {
    let pem = |n: &str| std::fs::read_to_string(fixture(n)).unwrap();
    let key = RsaKey::from_pkcs8(&std::fs::read(fixture("signer-key.der")).unwrap()).unwrap();
    let sign: crate::mock::DigestSigner = if garbage {
        Arc::new(|_d: &[u8]| vec![7u8; 256])
    } else {
        Arc::new(move |d: &[u8]| key.sign_sha512(d))
    };
    Server::start(xignature::mock(
        "xig-key",
        vec![(
            "mocksigner01".into(),
            vec![pem("signer.pem"), pem("ca.pem"), pem("root.pem")],
        )],
        sign,
    ))
    .unwrap()
}

fn xignature_client(url: &str) -> xignature::Xignature {
    xignature::Xignature::new(account(
        &xignature::INFO,
        &format!("{url}/v3"),
        &[("api_key", "xig-key")],
    ))
}

#[test]
fn xignature_signs_hashes_and_kantor_builds_a_valid_pades() {
    let server = xignature_server(false);
    let x = xignature_client(&server.url);
    x.test().unwrap();
    let pdf = kantor_pdf::testing::sample_pdf(1);
    let mut req = SignRequest::new(
        "a.pdf",
        Place::New {
            page: 0,
            rect: [72.0, 72.0, 252.0, 132.0],
        },
    );
    req.reason = "Setuju".into();
    let who = Signer {
        id: "mocksigner01".into(),
        otp: None,
    };
    let (out, kept) = signer::sign(&x, &pdf, &req, &who, false).unwrap();
    keep("xignature-signed.pdf", &out);
    assert_eq!(kept, Kept::Unsigned);
    assert!(out.starts_with(&pdf));
    // Only the hash went out: no request is anywhere near the PDF's size.
    let hash = &server.calls("/v3/sign/hash")[0];
    assert!(hash.body.len() < 300, "{}", hash.body.len());
    assert_eq!(hash.header("api-key"), Some("xig-key"));
    assert!(hash.header("pat").is_some());
    let r = verify(&out);
    let s = &r.signatures[0];
    assert_eq!(s.integrity, Integrity::Intact);
    assert_eq!(s.status, Status::Valid, "{:?}", s.notes);
    assert_eq!(s.signer_name(), "Mock Signer Xignature");
    assert_eq!(s.reason.as_deref(), Some("Setuju"));
    assert_eq!(s.kind, sigs::SigKind::Cades);
    // Unknown signer.
    let who = Signer {
        id: "nobody".into(),
        otp: None,
    };
    assert!(signer::sign(&x, &pdf, &req, &who, false).is_err());
}

#[test]
fn a_signature_that_does_not_match_the_certificate_is_refused() {
    let server = xignature_server(true);
    let x = xignature_client(&server.url);
    let pdf = kantor_pdf::testing::sample_pdf(1);
    let who = Signer {
        id: "mocksigner01".into(),
        otp: None,
    };
    let e = signer::sign(
        &x,
        &pdf,
        &SignRequest::new("a.pdf", Place::Invisible),
        &who,
        false,
    )
    .unwrap_err();
    assert!(e.to_string().contains("doesn't match"), "{e}");
}

#[test]
fn earlier_signatures_stay_valid_or_nothing_is_saved() {
    // Signed first with Xignature, then stamped by VIDA's mock.
    let xs = xignature_server(false);
    let x = xignature_client(&xs.url);
    let who = Signer {
        id: "mocksigner01".into(),
        otp: None,
    };
    let pdf = kantor_pdf::testing::sample_pdf(1);
    let (signed, _) = signer::sign(
        &x,
        &pdf,
        &SignRequest::new("a.pdf", Place::Invisible),
        &who,
        false,
    )
    .unwrap();
    let vs = vida_server(0);
    let v = vida_client(&vs.url, "s3cret");
    let req = StampRequest::new("a.pdf", 0, emeterai::box_at(400.0, 120.0), "VD004");
    let (out, kept) = emeterai::stamp_checked(&v, &signed, &req, false).unwrap();
    keep("signed-then-emeterai.pdf", &out.pdf);
    assert_eq!(kept, Kept::Incremental);
    let r = verify(&out.pdf);
    assert_eq!(r.signatures.len(), 2);
    assert!(
        r.signatures
            .iter()
            .all(|s| s.integrity == Integrity::Intact)
    );
    assert!(
        r.signatures.iter().all(|s| s.status.is_ok()),
        "{:?}",
        r.signatures
            .iter()
            .map(|s| (&s.status, &s.notes))
            .collect::<Vec<_>>()
    );

    // A provider that rebuilds the file breaks the first signature: refused
    // unless the user agreed.
    let rebuild = Arc::new(|pdf: &[u8], _: u32, _: f32, _: f32| {
        let mut d = kantor_pdf::Pdf::from_bytes(pdf, None).unwrap();
        d.to_bytes().unwrap()
    });
    let rs = Server::start(vida::mock("kantor-client", "s3cret", rebuild, 0)).unwrap();
    let v = vida_client(&rs.url, "s3cret");
    let e = emeterai::stamp_checked(&v, &signed, &req, false).unwrap_err();
    assert!(
        matches!(e, Error::BreaksSignatures(ref f) if f.len() == 1),
        "{e}"
    );
    let (_, kept) = emeterai::stamp_checked(&v, &signed, &req, true).unwrap();
    assert!(matches!(kept, Kept::Broken(_)));
}

#[test]
fn batch_runs_emeterai_then_provider_sign() {
    let vs = vida_server(0);
    let xs = xignature_server(false);
    let mut plugins = Plugins::default();
    plugins.add(Arc::new(crate::batch::EMeteraiPlugin::new(vec![Arc::new(
        vida_client(&vs.url, "s3cret"),
    )])));
    plugins.add(Arc::new(crate::batch::ProviderSignPlugin::new(vec![
        Arc::new(xignature_client(&xs.url)) as Arc<dyn RemoteSigner>,
    ])));
    let json = r#"{"version":1,"steps":[
        {"type":"emeterai","provider":"vida","doc_type":"VD010","page":"last"},
        {"type":"provider-sign","provider":"xignature","signer":"mocksigner01",
         "appearance":{"kind":"invisible"},"reason":"Batch"}]}"#;
    let b: Batch = serde_json::from_str(json).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let prepared = batch::Prepared::new(&b, dir.path(), None, &plugins).unwrap();
    for pages in [1, 3] {
        let pdf = kantor_pdf::testing::sample_pdf(pages);
        let run = batch::run_file(&prepared, "a.pdf", &pdf, None);
        let Outcome::Done { bytes, .. } = run.outcome else {
            panic!("{:?}", run.outcome);
        };
        let r = verify(&bytes);
        assert_eq!(r.signatures.len(), 2);
        assert_eq!(
            r.signatures[0].page,
            Some(pages as usize),
            "meterai on the last page"
        );
        assert!(r.signatures.iter().all(|s| s.status.is_ok()));
    }
    // A provider-sign step that isn't last is refused.
    let bad: Batch = serde_json::from_str(
        r#"{"version":1,"steps":[
        {"type":"provider-sign","provider":"xignature","signer":"x","appearance":{"kind":"invisible"}},
        {"type":"emeterai","provider":"vida","doc_type":"VD010"}]}"#,
    )
    .unwrap();
    assert!(batch::Prepared::new(&bad, dir.path(), None, &plugins).is_err());
}
