//! The VIDA, Mekari and Xignature mocks as a standalone server, for running
//! `kantor pdf emeterai` / `kantor pdf sign --provider` over many files (the
//! real-world sweep). Prints one JSON line with each mock's `--server` URL
//! and writes the matching `--credentials-file`s into DIR, then serves until
//! standard input closes.
//!
//!     cargo run -p kantor_esign --example esign_mocks -- DIR
//!
//! Stamps are signatures by the test e-meterai identity and Xignature signs
//! with the test signer key (crates/kantor_esign/tests/fixtures; trust
//! root.pem there). VIDA's mock allows 5 stamps per token, so it is rebuilt
//! at every token request: one CLI run, one fresh quota.

use kantor_esign::mock::{Handler, Received, Reply, RsaKey, Server, stamp_with};
use kantor_esign::providers::{mekari, vida, xignature};
use pdf_engine::signing::Identity;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

fn fixture(n: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(n)
}

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: esign_mocks DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let p12 = fixture("meterai.p12");
    let id =
        Arc::new(Identity::from_p12(&std::fs::read(&p12).unwrap(), "kantor-test", &p12).unwrap());

    let vid = id.clone();
    let fresh_vida = move || {
        let id = vid.clone();
        let stamp = Arc::new(move |pdf: &[u8], page: u32, x: f32, y: f32| {
            stamp_with(&id, pdf, page, [x, y, x + 60.0, y + 60.0])
        });
        vida::mock("kantor-client", "s3cret", stamp, 1)
    };
    let current: Arc<Mutex<Arc<Handler>>> = Arc::new(Mutex::new(fresh_vida()));
    let vida_handler: Arc<Handler> = Arc::new(move |r: &Received, base: &str| -> Reply {
        let h = {
            let mut cur = current.lock().unwrap();
            if r.route().ends_with("/openid-connect/token") {
                *cur = fresh_vida();
            }
            cur.clone()
        };
        h(r, base)
    });
    let vs = Server::start(vida_handler).unwrap();

    let mid = id.clone();
    let canvas = Arc::new(move |pdf: &[u8], page: u32, b: [f32; 6]| {
        // Back from Mekari's top-left canvas to user space (unturned pages).
        let [x, y, w, h, _, ch] = b;
        stamp_with(&mid, pdf, page, [x, ch - y - h, x + w, ch - y])
    });
    let ms = Server::start(mekari::mock("hmac-id", "hmac-secret", canvas, 1)).unwrap();

    let pem = |n: &str| std::fs::read_to_string(fixture(n)).unwrap();
    let key = RsaKey::from_pkcs8(&std::fs::read(fixture("signer-key.der")).unwrap()).unwrap();
    let xs = Server::start(xignature::mock(
        "xig-key",
        vec![(
            "mocksigner01".into(),
            vec![pem("signer.pem"), pem("ca.pem"), pem("root.pem")],
        )],
        Arc::new(move |d: &[u8]| key.sign_sha512(d)),
    ))
    .unwrap();

    std::fs::write(
        dir.join("vida.json"),
        r#"{"client_id":"kantor-client","client_secret":"s3cret"}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("mekari.json"),
        r#"{"client_id":"hmac-id","client_secret":"hmac-secret"}"#,
    )
    .unwrap();
    std::fs::write(dir.join("xignature.txt"), "api_key = xig-key\n").unwrap();
    println!(
        "{}",
        serde_json::json!({
            "vida": vs.url,
            "mekari": format!("{}/v2/esign-hmac/v1", ms.url),
            "xignature": format!("{}/v3", xs.url),
            "xignature_signer": "mocksigner01",
            "root": fixture("root.pem"),
        })
    );
    let mut sink = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut sink);
}
