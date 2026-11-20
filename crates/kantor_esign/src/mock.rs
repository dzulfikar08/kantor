//! A tiny local HTTP server that plays a provider's documented API, for
//! tests (here, in the CLI and in the app with `base_url` pointed at it).
//! Each provider module has a `mock` handler written from its public
//! documentation; [`Server`] runs one on `127.0.0.1` and records every
//! request so tests can check what Kantor sent.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

/// A request the mock received.
#[derive(Clone, Debug)]
pub struct Received {
    pub method: String,
    /// Path with the query string.
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Received {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The path without the query string.
    pub fn route(&self) -> &str {
        self.path.split('?').next().unwrap_or("")
    }

    pub fn query(&self, key: &str) -> Option<String> {
        let q = self.path.split_once('?')?.1;
        q.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k == key).then(|| v.to_string())
        })
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or(serde_json::Value::Null)
    }

    /// Fields of a multipart or urlencoded body.
    pub fn parts(&self) -> Vec<crate::http::Part> {
        let ct = self.header("content-type").unwrap_or("");
        if ct.starts_with("multipart/") {
            return crate::http::parse_multipart(ct, &self.body);
        }
        String::from_utf8_lossy(&self.body)
            .split('&')
            .filter(|s| !s.is_empty())
            .map(|kv| {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                crate::http::Part {
                    name: url_decode(k),
                    file_name: None,
                    data: url_decode(v).into_bytes(),
                }
            })
            .collect()
    }

    pub fn part(&self, name: &str) -> Option<Vec<u8>> {
        self.parts()
            .into_iter()
            .find(|p| p.name == name)
            .map(|p| p.data)
    }

    pub fn part_text(&self, name: &str) -> Option<String> {
        self.part(name)
            .map(|d| String::from_utf8_lossy(&d).into_owned())
    }
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The mock's answer.
#[derive(Clone, Debug)]
pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
}

impl Reply {
    pub fn json(status: u16, v: serde_json::Value) -> Reply {
        Reply {
            status,
            content_type: "application/json".into(),
            body: v.to_string().into_bytes(),
        }
    }

    pub fn bytes(content_type: &str, body: Vec<u8>) -> Reply {
        Reply {
            status: 200,
            content_type: content_type.into(),
            body,
        }
    }

    pub fn not_found() -> Reply {
        Reply::json(404, serde_json::json!({"message": "not found"}))
    }
}

/// A handler: the request and the server's own base URL (for answers that
/// carry download links).
pub type Handler = dyn Fn(&Received, &str) -> Reply + Send + Sync;

/// Documents a mock holds: id → (stamped bytes, pending answers left).
pub type Docs = std::sync::Arc<Mutex<std::collections::HashMap<String, (Vec<u8>, usize)>>>;

/// Peruri's part in the VIDA mock: (pdf, 0-based page, llx, lly) → stamped.
pub type CornerStamp = Arc<dyn Fn(&[u8], u32, f32, f32) -> Vec<u8> + Send + Sync>;

/// Peruri's part in the Mekari mock: (pdf, 0-based page, [x, y, w, h,
/// canvas w, canvas h] from the top left) → stamped.
pub type CanvasStamp = Arc<dyn Fn(&[u8], u32, [f32; 6]) -> Vec<u8> + Send + Sync>;

/// A remote key: SHA-512 digest → raw signature.
pub type DigestSigner = Arc<dyn Fn(&[u8]) -> Vec<u8> + Send + Sync>;

/// A running mock; stops when dropped.
pub struct Server {
    pub url: String,
    log: Arc<Mutex<Vec<Received>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    addr: std::net::SocketAddr,
}

impl Server {
    /// Serve `handler` on a free port of 127.0.0.1.
    pub fn start(handler: Arc<Handler>) -> std::io::Result<Server> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let addr = listener.local_addr()?;
        let url = format!("http://{addr}");
        let log = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (l2, s2, u2) = (log.clone(), stop.clone(), url.clone());
        thread::Builder::new()
            .name("kantor-esign-mock".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if s2.load(std::sync::atomic::Ordering::Relaxed) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let (h, l, u) = (handler.clone(), l2.clone(), u2.clone());
                    thread::spawn(move || {
                        let _ = serve(stream, &*h, &l, &u);
                    });
                }
            })?;
        Ok(Server {
            url,
            log,
            stop,
            addr,
        })
    }

    /// Every request so far.
    pub fn requests(&self) -> Vec<Received> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// Requests to `route` (path without query).
    pub fn calls(&self, route: &str) -> Vec<Received> {
        self.requests()
            .into_iter()
            .filter(|r| r.route() == route)
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        // Wake the accept loop.
        let _ = TcpStream::connect(self.addr);
    }
}

fn serve(
    stream: TcpStream,
    handler: &Handler,
    log: &Mutex<Vec<Received>>,
    base: &str,
) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let mut it = line.split_whitespace();
        let method = it.next().unwrap_or("").to_string();
        let path = it.next().unwrap_or("/").to_string();
        let mut headers = Vec::new();
        loop {
            let mut h = String::new();
            reader.read_line(&mut h)?;
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((n, v)) = h.split_once(':') {
                headers.push((n.trim().to_string(), v.trim().to_string()));
            }
        }
        let len = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0; len];
        reader.read_exact(&mut body)?;
        let req = Received {
            method,
            path,
            headers,
            body,
        };
        if let Ok(mut l) = log.lock() {
            l.push(req.clone());
        }
        let reply = handler(&req, base);
        let mut w = &stream;
        write!(
            w,
            "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n",
            reply.status,
            reply.content_type,
            reply.body.len()
        )?;
        w.write_all(&reply.body)?;
        w.flush()?;
    }
}

/// A test RSA private key that signs a SHA-512 digest with PKCS #1 v1.5,
/// as a remote signing service does (ring can't sign a precomputed digest,
/// so this is plain modular exponentiation). For mocks only.
pub struct RsaKey {
    n: num_bigint::BigUint,
    d: num_bigint::BigUint,
    size: usize,
}

impl RsaKey {
    /// From an unencrypted PKCS #8 PrivateKeyInfo (DER).
    pub fn from_pkcs8(der: &[u8]) -> Option<RsaKey> {
        use pdf_engine::signatures::der::{self, Tlv};
        let outer = Tlv::parse(der).ok()?.expect(der::SEQUENCE).ok()?;
        let mut r = outer.reader();
        r.read_tag(der::INTEGER).ok()?;
        r.read_tag(der::SEQUENCE).ok()?;
        let inner = r.read_tag(der::OCTET_STRING).ok()?;
        let key = Tlv::parse(inner.body).ok()?.expect(der::SEQUENCE).ok()?;
        let mut k = key.reader();
        k.read_tag(der::INTEGER).ok()?;
        let n = k.read_tag(der::INTEGER).ok()?;
        k.read_tag(der::INTEGER).ok()?;
        let d = k.read_tag(der::INTEGER).ok()?;
        let n_bytes = n.uint_bytes();
        Some(RsaKey {
            n: num_bigint::BigUint::from_bytes_be(n_bytes),
            d: num_bigint::BigUint::from_bytes_be(d.uint_bytes()),
            size: n_bytes.len(),
        })
    }

    /// PKCS #1 v1.5 signature of a SHA-512 `digest`.
    pub fn sign_sha512(&self, digest: &[u8]) -> Vec<u8> {
        // DigestInfo for SHA-512 (RFC 8017 §9.2 note 1).
        let mut t = vec![
            0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02,
            0x03, 0x05, 0x00, 0x04, 0x40,
        ];
        t.extend_from_slice(digest);
        let mut em = vec![0x00, 0x01];
        em.resize(self.size - t.len() - 1, 0xff);
        em.push(0x00);
        em.extend_from_slice(&t);
        let m = num_bigint::BigUint::from_bytes_be(&em);
        let s = m.modpow(&self.d, &self.n).to_bytes_be();
        let mut out = vec![0; self.size - s.len()];
        out.extend_from_slice(&s);
        out
    }
}

/// Peruri's part in the e-meterai mocks: an invisible-to-the-API visible
/// PAdES signature by `id` in `rect` on `page`, as an incremental update
/// (Peruri's stamp is a signature by its e-meterai certificate, drawn as
/// the meterai picture).
pub fn stamp_with(
    id: &pdf_engine::signing::Identity,
    pdf: &[u8],
    page: u32,
    rect: [f32; 4],
) -> Vec<u8> {
    use kantor_pdf::sign::{Look, Place};
    let mut o = pdf_engine::signing::Options::new(Place::New { page, rect });
    o.reason = Some("e-Meterai (mock)".into());
    o.look = Some(Look {
        lines: vec!["METERAI".into(), "ELEKTRONIK".into(), "10000".into()],
        art: None,
    });
    pdf_engine::signing::sign(pdf, id, &o)
        .unwrap_or_else(|e| format!("mock stamping failed: {e}").into_bytes())
}
