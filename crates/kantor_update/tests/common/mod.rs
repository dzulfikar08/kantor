//! Shared by the end-to-end tests: a tiny HTTP server and signed manifests.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use kantor_update::{BuildId, Channel, Config, InstallKind};
use ring::signature::{Ed25519KeyPair, KeyPair as _};

pub type Routes = Arc<Mutex<HashMap<String, (u16, Vec<u8>)>>>;

/// A tiny HTTP/1.1 server: one response per connection, from `routes`.
/// A status of 302 sends the body as the Location.
pub fn serve() -> (String, Routes) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let routes: Routes = Arc::default();
    let r = routes.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).unwrap() <= 2 {
                    break;
                }
            }
            let resp = r.lock().unwrap().get(&path).cloned();
            let _ = match resp {
                Some((302, to)) => write!(
                    stream,
                    "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    String::from_utf8(to).unwrap()
                ),
                Some((status, body)) => write!(
                    stream,
                    "HTTP/1.1 {status} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                )
                .and_then(|_| stream.write_all(&body)),
                None => write!(
                    stream,
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                ),
            };
        }
    });
    (base, routes)
}

pub fn key() -> Ed25519KeyPair {
    Ed25519KeyPair::from_seed_unchecked(&[42; 32]).unwrap()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Publish a signed manifest for `version` with one asset under `asset_key`.
pub fn publish(
    base: &str,
    routes: &Routes,
    version: &str,
    asset_key: &str,
    asset: &[u8],
) -> String {
    let manifest = format!(
        r#"{{
  "channel": "stable",
  "version": "{version}",
  "date": "2026-11-10T00:00:00Z",
  "notes_url": "{base}/notes",
  "assets": {{
    "{asset_key}": {{ "url": "{base}/redirect-asset", "sha256": "{}", "size": {} }}
  }}
}}
"#,
        sha256_hex(asset),
        asset.len()
    );
    let sig = base64::engine::general_purpose::STANDARD.encode(key().sign(manifest.as_bytes()));
    let mut r = routes.lock().unwrap();
    // The site redirects the manifest, GitHub redirects the asset.
    r.insert(
        "/update/stable.json".into(),
        (302, format!("{base}/gh/update-stable.json").into_bytes()),
    );
    r.insert(
        "/gh/update-stable.json".into(),
        (200, manifest.into_bytes()),
    );
    r.insert("/update/stable.json.sig".into(), (200, sig.into_bytes()));
    r.insert(
        "/redirect-asset".into(),
        (302, format!("{base}/asset").into_bytes()),
    );
    r.insert("/asset".into(), (200, asset.to_vec()));
    format!("{base}/update/stable.json")
}

pub fn config(url: String, kind: InstallKind) -> Config {
    Config {
        channel: Channel::Stable,
        manifest_url: url,
        public_key: key().public_key().as_ref().try_into().unwrap(),
        current: BuildId::new("0.2.0", 0).unwrap(),
        kind,
        arch: "x86_64".into(),
    }
}

pub fn leftovers(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with('.'))
        .collect();
    v.sort();
    v
}
