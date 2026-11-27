//! OneDrive (Microsoft Graph) and Google Drive (Drive API v3) against
//! local mock servers: listing, download, small and resumable uploads
//! (resumed after a dropped link), version conflicts, token refresh, a
//! revoked sign-in, and the browser sign-in with PKCE.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use kantor_cloud::cache::{Cache, CacheEntry, Stamp};
use kantor_cloud::oauth::{self, App, Provider};
use kantor_cloud::oidc::{DonePage, Endpoints, TokenSource, parse_query};
use kantor_cloud::session::{self, Remote};
use kantor_cloud::sync::UploadOutcome;
use kantor_cloud::{Auth, Client, Error, Precondition, gdrive, sha1_hex};
use serde_json::{Value, json};

// ── a tiny HTTP server ─────────────────────────────────────────────────────

struct Req {
    method: String,
    path: String,
    query: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, k: &str) -> Option<&str> {
        self.headers.get(k).map(String::as_str)
    }
    fn param(&self, k: &str) -> Option<String> {
        parse_query(&self.query)
            .into_iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v)
    }
    fn bearer(&self) -> Option<&str> {
        self.header("authorization")?.strip_prefix("Bearer ")
    }
    fn form(&self) -> Vec<(String, String)> {
        parse_query(&String::from_utf8_lossy(&self.body))
    }
}

enum Resp {
    Reply(u16, Vec<(String, String)>, Vec<u8>),
    /// Close the connection without an answer (a dropped link).
    Drop,
}

fn json_reply(status: u16, v: Value) -> Resp {
    Resp::Reply(
        status,
        vec![("content-type".into(), "application/json".into())],
        v.to_string().into_bytes(),
    )
}

fn empty(status: u16, headers: Vec<(&str, String)>) -> Resp {
    Resp::Reply(
        status,
        headers
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
        Vec::new(),
    )
}

type Handler = Arc<dyn Fn(&Req, &str) -> Resp + Send + Sync>;

fn read_req(stream: &TcpStream) -> Option<Req> {
    let mut r = BufReader::new(stream.try_clone().ok()?);
    let mut first = String::new();
    r.read_line(&mut first).ok()?;
    let mut parts = first.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?.to_owned();
    let mut headers = HashMap::new();
    loop {
        let mut l = String::new();
        r.read_line(&mut l).ok()?;
        if l == "\r\n" || l.is_empty() {
            break;
        }
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_owned());
        }
    }
    let mut body = Vec::new();
    if let Some(n) = headers.get("content-length") {
        body = vec![0; n.parse().ok()?];
        r.read_exact(&mut body).ok()?;
    } else if headers
        .get("transfer-encoding")
        .is_some_and(|t| t.eq_ignore_ascii_case("chunked"))
    {
        loop {
            let mut size = String::new();
            r.read_line(&mut size).ok()?;
            let n = usize::from_str_radix(size.trim().split(';').next()?, 16).ok()?;
            let mut chunk = vec![0; n + 2];
            r.read_exact(&mut chunk).ok()?;
            if n == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..n]);
        }
    }
    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_owned(), q.to_owned()),
        None => (target, String::new()),
    };
    Some(Req {
        method,
        path,
        query,
        headers,
        body,
    })
}

fn serve(handler: Handler) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let origin = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let o = origin.clone();
    std::thread::spawn(move || {
        for s in listener.incoming() {
            let Ok(mut s) = s else { continue };
            let (handler, origin) = (handler.clone(), o.clone());
            std::thread::spawn(move || {
                let Some(req) = read_req(&s) else { return };
                match handler(&req, &origin) {
                    Resp::Drop => {
                        let _ = s.shutdown(std::net::Shutdown::Both);
                    }
                    Resp::Reply(status, headers, body) => {
                        let mut head = format!("HTTP/1.1 {status} X\r\n");
                        for (k, v) in headers {
                            head.push_str(&format!("{k}: {v}\r\n"));
                        }
                        head.push_str(&format!(
                            "content-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        ));
                        let _ = s.write_all(head.as_bytes());
                        let _ = s.write_all(&body);
                    }
                }
            });
        }
    });
    origin
}

fn decode(s: &str) -> String {
    parse_query(&format!("x={}", s.replace('+', "%2B")))
        .pop()
        .map(|(_, v)| v)
        .unwrap_or_default()
}

/// The provider side: refresh tokens it takes, access tokens it gave.
#[derive(Default)]
struct Tokens {
    access: Vec<String>,
    refresh: Vec<String>,
    refreshes: usize,
    /// A client secret the token endpoint wants (Google).
    secret: Option<String>,
}

fn token_endpoint(t: &Mutex<Tokens>, req: &Req) -> Resp {
    let form = req.form();
    let get = |k: &str| {
        form.iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    };
    let mut t = t.lock().unwrap();
    if let Some(s) = &t.secret
        && get("client_secret") != *s
    {
        return json_reply(401, json!({"error": "invalid_client"}));
    }
    let ok = match get("grant_type").as_str() {
        "refresh_token" => t.refresh.contains(&get("refresh_token")),
        "authorization_code" => get("code") == "the-code" && get("code_verifier").len() >= 43,
        _ => false,
    };
    if !ok {
        return json_reply(400, json!({"error": "invalid_grant"}));
    }
    t.refreshes += 1;
    let n = t.refreshes;
    let (at, rt) = (format!("at{n}"), format!("rt{n}"));
    t.access.push(at.clone());
    t.refresh = vec![rt.clone()];
    json_reply(
        200,
        json!({"access_token": at, "refresh_token": rt, "expires_in": 3600}),
    )
}

fn source(origin: &str, secret: Option<&str>) -> (Arc<TokenSource>, Arc<Mutex<Vec<String>>>) {
    let ep = Endpoints {
        issuer: origin.to_owned(),
        authorization_endpoint: format!("{origin}/authorize"),
        token_endpoint: format!("{origin}/token"),
        device_authorization_endpoint: None,
        revocation_endpoint: None,
    };
    let stored = Arc::new(Mutex::new(Vec::new()));
    let keep = stored.clone();
    let s = TokenSource::new(ep, "kantor-test".into(), "rt0".into(), move |r| {
        keep.lock().unwrap().push(r.to_owned())
    })
    .with_client_secret(secret.map(str::to_owned));
    (Arc::new(s), stored)
}

fn remote(client: Client, account: &str) -> Remote {
    Remote {
        client,
        account: account.into(),
        owner: "budi (Kantor)".into(),
    }
}

fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % 251) as u8).collect()
}

/// A new working copy for `remote` with `bytes` (not on the server yet).
fn new_working_copy(cache: &Cache, account: &str, remote: &str, bytes: &[u8]) -> CacheEntry {
    let local = cache.local_path(account, remote);
    std::fs::create_dir_all(local.parent().unwrap()).unwrap();
    std::fs::write(&local, bytes).unwrap();
    CacheEntry {
        account: account.into(),
        remote: remote.into(),
        local,
        ..Default::default()
    }
}

// ── Microsoft Graph ────────────────────────────────────────────────────────

#[derive(Default)]
struct Graph {
    tokens: Mutex<Tokens>,
    /// path → (bytes, version)
    files: Mutex<BTreeMap<String, (Vec<u8>, u32)>>,
    folders: Mutex<Vec<String>>,
    /// Upload sessions: (path, bytes so far, fail if the file exists).
    sessions: Mutex<Vec<(String, Vec<u8>, bool)>>,
    /// Drop this many session parts that don't start at 0.
    drop_parts: Mutex<u32>,
    /// Start offsets of the session parts received.
    part_starts: Mutex<Vec<u64>>,
    /// A token was sent to a pre-authorised address.
    token_leaked: AtomicBool,
}

fn g_etag(path: &str, v: u32) -> String {
    format!("\"{{{path}}},{v}\"")
}

fn g_item(path: &str, bytes: &[u8], v: u32) -> Value {
    json!({
        "id": path,
        "name": path.rsplit('/').next().unwrap(),
        "size": bytes.len(),
        "eTag": g_etag(path, v),
        "cTag": format!("c{v}"),
        "lastModifiedDateTime": format!("2026-11-27T00:00:{v:02}Z"),
        "file": {"mimeType": "application/octet-stream",
                 "hashes": {"sha1Hash": sha1_hex(bytes).to_ascii_uppercase()}}
    })
}

fn graph_handler(g: Arc<Graph>) -> Handler {
    Arc::new(move |req, origin| {
        if req.path == "/token" {
            return token_endpoint(&g.tokens, req);
        }
        if let Some(rest) = req.path.strip_prefix("/dl/") {
            if req.header("authorization").is_some() {
                g.token_leaked
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            let path = decode(rest);
            return match g.files.lock().unwrap().get(&path) {
                Some((b, _)) => Resp::Reply(200, vec![], b.clone()),
                None => json_reply(404, json!({"error": {"code": "itemNotFound"}})),
            };
        }
        if let Some(n) = req.path.strip_prefix("/up/") {
            if req.header("authorization").is_some() {
                g.token_leaked
                    .store(true, std::sync::atomic::Ordering::SeqCst);
            }
            let n: usize = n.parse().unwrap();
            let mut sessions = g.sessions.lock().unwrap();
            let (path, data, fail) = sessions[n].clone();
            if req.method == "GET" {
                return json_reply(
                    200,
                    json!({"nextExpectedRanges": [format!("{}-", data.len())]}),
                );
            }
            let range = req.header("content-range").unwrap().to_owned();
            let (span, total) = range
                .strip_prefix("bytes ")
                .unwrap()
                .split_once('/')
                .unwrap();
            let start: usize = span.split('-').next().unwrap().parse().unwrap();
            let total: usize = total.parse().unwrap();
            g.part_starts.lock().unwrap().push(start as u64);
            if start > 0 {
                let mut d = g.drop_parts.lock().unwrap();
                if *d > 0 {
                    *d -= 1;
                    return Resp::Drop;
                }
            }
            assert_eq!(start, data.len(), "parts arrive in order");
            let mut data = data;
            data.extend_from_slice(&req.body);
            sessions[n].1 = data.clone();
            if data.len() < total {
                return json_reply(
                    202,
                    json!({"nextExpectedRanges": [format!("{}-", data.len())]}),
                );
            }
            let mut files = g.files.lock().unwrap();
            if fail && files.contains_key(&path) {
                return json_reply(409, json!({"error": {"code": "nameAlreadyExists"}}));
            }
            let v = files.get(&path).map_or(1, |(_, v)| v + 1);
            files.insert(path.clone(), (data.clone(), v));
            return json_reply(201, g_item(&path, &data, v));
        }
        // Everything under /v1.0 needs a valid token.
        let valid = req
            .bearer()
            .is_some_and(|t| g.tokens.lock().unwrap().access.iter().any(|a| a == t));
        if !valid {
            return json_reply(
                401,
                json!({"error": {"code": "InvalidAuthenticationToken"}}),
            );
        }
        if req.path == "/v1.0/me" {
            return json_reply(
                200,
                json!({"id": "u1", "displayName": "Budi", "userPrincipalName": "budi@contoso.example", "mail": null}),
            );
        }
        let rest = req.path.strip_prefix("/v1.0/me/drive/").unwrap();
        let (path, tail) = if let Some(r) = rest.strip_prefix("root:/") {
            let i = r.rfind(':').unwrap();
            (decode(&r[..i]), r[i + 1..].to_owned())
        } else {
            (String::new(), rest.strip_prefix("root").unwrap().to_owned())
        };
        let files = g.files.lock().unwrap().clone();
        match (req.method.as_str(), tail.as_str()) {
            ("GET", "/children") => {
                let prefix = if path.is_empty() {
                    String::new()
                } else {
                    format!("{path}/")
                };
                let mut value: Vec<Value> = g
                    .folders
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|f| f.strip_prefix(&prefix).is_some_and(|r| !r.contains('/')))
                    .map(|f| json!({"id": f, "name": f.rsplit('/').next().unwrap(), "folder": {"childCount": 0}}))
                    .collect();
                value.extend(
                    files
                        .iter()
                        .filter(|(p, _)| p.strip_prefix(&prefix).is_some_and(|r| !r.contains('/')))
                        .map(|(p, (b, v))| g_item(p, b, *v)),
                );
                // Two pages: the client must follow nextLink.
                if req.param("page").is_none() && value.len() > 1 {
                    value.truncate(1);
                    return json_reply(
                        200,
                        json!({"value": value,
                               "@odata.nextLink": format!("{origin}{}?page=2", req.path)}),
                    );
                }
                if req.param("page").is_some() {
                    value.remove(0);
                }
                json_reply(200, json!({ "value": value }))
            }
            ("GET", "") => match files.get(&path) {
                Some((b, v)) => json_reply(200, g_item(&path, b, *v)),
                None if g.folders.lock().unwrap().contains(&path) => {
                    json_reply(200, json!({"id": path, "name": path, "folder": {}}))
                }
                None => json_reply(404, json!({"error": {"code": "itemNotFound"}})),
            },
            ("GET", "/content") => {
                if !files.contains_key(&path) {
                    return json_reply(404, json!({"error": {"code": "itemNotFound"}}));
                }
                empty(
                    302,
                    vec![("location", format!("{origin}/dl/{}", encode(&path)))],
                )
            }
            ("PUT", "/content") => {
                let current = files.get(&path).map(|(_, v)| *v);
                if let Some(m) = req.header("if-match")
                    && current.map(|v| g_etag(&path, v)).as_deref() != Some(m)
                {
                    return json_reply(412, json!({"error": {"code": "resourceModified"}}));
                }
                if req.param("@microsoft.graph.conflictBehavior").as_deref() == Some("fail")
                    && current.is_some()
                {
                    return json_reply(409, json!({"error": {"code": "nameAlreadyExists"}}));
                }
                let v = current.map_or(1, |v| v + 1);
                g.files
                    .lock()
                    .unwrap()
                    .insert(path.clone(), (req.body.clone(), v));
                json_reply(if v == 1 { 201 } else { 200 }, g_item(&path, &req.body, v))
            }
            ("POST", "/createUploadSession") => {
                let current = files.get(&path).map(|(_, v)| *v);
                if let Some(m) = req.header("if-match")
                    && current.map(|v| g_etag(&path, v)).as_deref() != Some(m)
                {
                    return json_reply(412, json!({"error": {"code": "resourceModified"}}));
                }
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let fail = body["item"]["@microsoft.graph.conflictBehavior"] == "fail";
                let mut s = g.sessions.lock().unwrap();
                s.push((path, Vec::new(), fail));
                json_reply(
                    200,
                    json!({"uploadUrl": format!("{origin}/up/{}", s.len() - 1)}),
                )
            }
            ("POST", "/children") => {
                let body: Value = serde_json::from_slice(&req.body).unwrap();
                let name = body["name"].as_str().unwrap();
                let full = if path.is_empty() {
                    name.to_owned()
                } else {
                    format!("{path}/{name}")
                };
                let mut f = g.folders.lock().unwrap();
                if f.contains(&full) {
                    return json_reply(409, json!({"error": {"code": "nameAlreadyExists"}}));
                }
                f.push(full.clone());
                json_reply(201, json!({"id": full, "name": name, "folder": {}}))
            }
            _ => json_reply(400, json!({"error": {"code": "invalidRequest"}})),
        }
    })
}

fn encode(path: &str) -> String {
    path.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn graph_server() -> (Arc<Graph>, String) {
    let g = Arc::new(Graph::default());
    g.tokens.lock().unwrap().refresh = vec!["rt0".into()];
    g.folders.lock().unwrap().push("Laporan".into());
    g.files
        .lock()
        .unwrap()
        .insert("Laporan/Q3 #1.docx".into(), (b"version one".to_vec(), 1));
    g.files
        .lock()
        .unwrap()
        .insert("top.xlsx".into(), (b"top".to_vec(), 4));
    let origin = serve(graph_handler(g.clone()));
    (g, origin)
}

#[test]
fn graph_list_get_save_and_conflict() {
    let (g, origin) = graph_server();
    let (src, stored) = source(&origin, None);
    let c = Client::graph_at(
        &format!("{origin}/v1.0/me/drive"),
        Auth::Source(src.clone()),
    );
    assert!(!c.has_locks());

    // First request refreshes (no access token yet); the rotated refresh
    // token is stored. Both pages of the listing arrive.
    let mut top = c.list("").unwrap();
    top.sort_by(|a, b| a.name.cmp(&b.name));
    assert_eq!(
        top.iter()
            .map(|e| (e.name.as_str(), e.is_dir))
            .collect::<Vec<_>>(),
        vec![("Laporan", true), ("top.xlsx", false)]
    );
    assert_eq!(*stored.lock().unwrap(), vec!["rt1".to_string()]);

    let e = c.stat("Laporan/Q3 #1.docx").unwrap();
    assert_eq!(
        e.etag.as_deref(),
        Some(g_etag("Laporan/Q3 #1.docx", 1).as_str())
    );
    assert_eq!(e.sha1(), Some(sha1_hex(b"version one")));
    assert!(matches!(c.stat("Laporan/none.docx"), Err(Error::NotFound)));

    // Download through the redirect, without the token.
    let got = c.get("Laporan/Q3 #1.docx").unwrap();
    assert_eq!(got.bytes, b"version one");
    assert!(!g.token_leaked.load(std::sync::atomic::Ordering::SeqCst));

    // Someone else saves: our save with the old version is refused.
    let old = e.etag.clone().unwrap();
    g.files
        .lock()
        .unwrap()
        .insert("Laporan/Q3 #1.docx".into(), (b"theirs".to_vec(), 2));
    let err = c
        .put(
            "Laporan/Q3 #1.docx",
            b"mine",
            Precondition::IfMatch(&old),
            None,
        )
        .unwrap_err();
    assert!(matches!(err, Error::Conflict), "{err:?}");
    // With the current version it goes through and returns the new one.
    let now = c.stat("Laporan/Q3 #1.docx").unwrap().etag.unwrap();
    let new = c
        .put(
            "Laporan/Q3 #1.docx",
            b"mine",
            Precondition::IfMatch(&now),
            None,
        )
        .unwrap();
    assert_eq!(
        new.as_deref(),
        Some(g_etag("Laporan/Q3 #1.docx", 3).as_str())
    );
    // Save As never replaces an existing file.
    let err = c
        .put("top.xlsx", b"x", Precondition::IfNoneMatch, None)
        .unwrap_err();
    assert!(matches!(err, Error::Conflict), "{err:?}");
    c.put("Laporan/new.xlsx", b"x", Precondition::IfNoneMatch, None)
        .unwrap();
    // Folders.
    c.mkcol("Laporan/2026").unwrap();
    c.mkcol("Laporan/2026").unwrap();
    assert!(
        g.folders
            .lock()
            .unwrap()
            .contains(&"Laporan/2026".to_string())
    );
    // No locks on OneDrive: the session falls back to version checks.
    let cache_dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(cache_dir.path());
    let r = remote(c, "od");
    assert!(matches!(
        session::take_lock(&r, &cache, "top.xlsx"),
        kantor_cloud::sync::LockState::Unsupported(_)
    ));
}

#[test]
fn graph_session_upload_resumes_after_a_drop() {
    let (g, origin) = graph_server();
    let (src, _) = source(&origin, None);
    let c = Client::graph_at(&format!("{origin}/v1.0/me/drive"), Auth::Source(src));
    let r = remote(c, "od");
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    let bytes = pattern(kantor_cloud::graph::PART + 1_000_000);
    let entry = new_working_copy(&cache, "od", "Laporan/big.pptx", &bytes);

    // The second part's link drops (the request may be retried once on a
    // fresh connection): the upload stops, offline, and is recorded.
    *g.drop_parts.lock().unwrap() = 2;
    let out = session::upload(&r, &cache, &entry, None, false, &mut |_, _| {});
    assert!(matches!(out, UploadOutcome::Offline(_)), "{out:?}");
    assert!(cache.pending_upload(&entry.local).is_some());
    // The link is back.
    *g.drop_parts.lock().unwrap() = 0;

    // Next try resumes the session from the second part.
    let mut seen = Vec::new();
    let out = session::upload(&r, &cache, &entry, None, false, &mut |s, t| {
        seen.push((s, t))
    });
    let UploadOutcome::Synced { server, .. } = out else {
        panic!("{out:?}")
    };
    assert_eq!(server.unwrap().size, Some(bytes.len() as u64));
    assert_eq!(
        g.files.lock().unwrap().get("Laporan/big.pptx").unwrap().0,
        bytes
    );
    let starts = g.part_starts.lock().unwrap().clone();
    assert_eq!(
        starts.iter().filter(|s| **s == 0).count(),
        1,
        "the first part was sent once: {starts:?}"
    );
    assert!(seen.last().is_some_and(|(s, t)| s == t));
    assert!(cache.pending_upload(&entry.local).is_none());
    assert!(!g.token_leaked.load(std::sync::atomic::Ordering::SeqCst));

    // A big save over a file changed meanwhile: a conflict, nothing sent.
    let mut base = entry.clone();
    base.etag = Some(g_etag("Laporan/big.pptx", 1));
    base.modified = Some("2026-11-27T00:00:01Z".into());
    base.size = Some(bytes.len() as u64);
    base.synced = None;
    g.files
        .lock()
        .unwrap()
        .insert("Laporan/big.pptx".into(), (b"theirs".to_vec(), 2));
    let out = session::upload(&r, &cache, &base, None, false, &mut |_, _| {});
    assert!(matches!(out, UploadOutcome::Conflict { .. }), "{out:?}");
}

#[test]
fn graph_revoked_sign_in_ends() {
    let (g, origin) = graph_server();
    let (src, _) = source(&origin, None);
    let c = Client::graph_at(
        &format!("{origin}/v1.0/me/drive"),
        Auth::Source(src.clone()),
    );
    c.list("").unwrap();
    let who = kantor_cloud::graph::whoami_with(&c, &format!("{origin}/v1.0/me")).unwrap();
    assert_eq!(
        who,
        ("u1".into(), "budi@contoso.example".into(), "Budi".into())
    );
    // The user revokes Kantor's access: tokens and refresh are refused.
    {
        let mut t = g.tokens.lock().unwrap();
        t.access.clear();
        t.refresh.clear();
    }
    let err = c.list("").unwrap_err();
    assert!(matches!(err, Error::Auth(401)), "{err:?}");
    assert!(src.needs_sign_in());
}

// ── Google Drive ───────────────────────────────────────────────────────────

#[derive(Clone)]
struct GFile {
    id: String,
    name: String,
    mime: String,
    parents: Vec<String>,
    data: Vec<u8>,
    version: u64,
    trashed: bool,
}

#[derive(Default)]
struct Gd {
    tokens: Mutex<Tokens>,
    files: Mutex<Vec<GFile>>,
    /// Resumable sessions: (file id or None for new, metadata, bytes so far, total).
    sessions: Mutex<Vec<(Option<String>, Value, Vec<u8>)>>,
    drop_parts: Mutex<u32>,
    part_starts: Mutex<Vec<u64>>,
    next: Mutex<u32>,
}

impl Gd {
    fn json(f: &GFile) -> Value {
        json!({
            "id": f.id, "name": f.name, "mimeType": f.mime, "parents": f.parents,
            "size": f.data.len().to_string(), "version": f.version.to_string(),
            "modifiedTime": format!("2026-11-27T00:00:{:02}.000Z", f.version % 60),
            "sha1Checksum": sha1_hex(&f.data), "trashed": f.trashed,
        })
    }
    fn new_id(&self) -> String {
        let mut n = self.next.lock().unwrap();
        *n += 1;
        format!("id{n}")
    }
    fn add(&self, name: &str, mime: &str, parent: &str, data: &[u8]) -> String {
        let id = self.new_id();
        self.files.lock().unwrap().push(GFile {
            id: id.clone(),
            name: name.into(),
            mime: mime.into(),
            parents: vec![parent.into()],
            data: data.to_vec(),
            version: 1,
            trashed: false,
        });
        id
    }
    fn create(&self, meta: &Value, data: Vec<u8>) -> GFile {
        let parent = meta["parents"][0].as_str().unwrap_or("root").to_owned();
        let parent = if parent == "root" {
            "myDriveRoot".to_owned()
        } else {
            parent
        };
        let id = self.add(
            meta["name"].as_str().unwrap(),
            meta["mimeType"]
                .as_str()
                .unwrap_or("application/octet-stream"),
            &parent,
            &data,
        );
        self.get(&id).unwrap()
    }
    fn get(&self, id: &str) -> Option<GFile> {
        self.files
            .lock()
            .unwrap()
            .iter()
            .find(|f| f.id == id)
            .cloned()
    }
    fn update(&self, id: &str, data: Vec<u8>) -> GFile {
        let mut files = self.files.lock().unwrap();
        let f = files.iter_mut().find(|f| f.id == id).unwrap();
        f.data = data;
        f.version += 1;
        f.clone()
    }
}

fn gdrive_handler(g: Arc<Gd>) -> Handler {
    Arc::new(move |req, origin| {
        if req.path == "/token" {
            return token_endpoint(&g.tokens, req);
        }
        let valid = req
            .bearer()
            .is_some_and(|t| g.tokens.lock().unwrap().access.iter().any(|a| a == t));
        if !valid {
            return json_reply(401, json!({"error": {"code": 401}}));
        }
        if let Some(n) = req.path.strip_prefix("/session/") {
            let n: usize = n.parse().unwrap();
            let mut sessions = g.sessions.lock().unwrap();
            let (id, meta, data) = sessions[n].clone();
            let range = req.header("content-range").unwrap().to_owned();
            let (span, total) = range
                .strip_prefix("bytes ")
                .unwrap()
                .split_once('/')
                .unwrap();
            let total: usize = total.parse().unwrap();
            let have = |len: usize| {
                if len == 0 {
                    vec![]
                } else {
                    vec![("range", format!("bytes=0-{}", len - 1))]
                }
            };
            if span == "*" {
                return empty(308, have(data.len()));
            }
            let start: usize = span.split('-').next().unwrap().parse().unwrap();
            g.part_starts.lock().unwrap().push(start as u64);
            if start > 0 {
                let mut d = g.drop_parts.lock().unwrap();
                if *d > 0 {
                    *d -= 1;
                    return Resp::Drop;
                }
            }
            assert_eq!(start, data.len());
            let mut data = data;
            data.extend_from_slice(&req.body);
            sessions[n].2 = data.clone();
            if data.len() < total {
                return empty(308, have(data.len()));
            }
            let f = match id {
                Some(id) => g.update(&id, data),
                None => g.create(&meta, data),
            };
            return json_reply(200, Gd::json(&f));
        }
        if req.path == "/drive/v3/about" {
            return json_reply(
                200,
                json!({"user": {"displayName": "Budi", "emailAddress": "budi@gmail.example", "permissionId": "p1"}}),
            );
        }
        let upload = req.path.starts_with("/upload/");
        let rest = req
            .path
            .trim_start_matches("/upload")
            .strip_prefix("/drive/v3/files")
            .unwrap();
        let id = rest.strip_prefix('/').map(str::to_owned);
        match (req.method.as_str(), id, upload) {
            ("GET", None, false) => {
                let q = req.param("q").unwrap();
                let files = g.files.lock().unwrap().clone();
                let hits: Vec<Value> = files
                    .iter()
                    .filter(|f| !f.trashed)
                    .filter(|f| match q.strip_prefix('\'') {
                        Some(r) => {
                            let parent = r.split('\'').next().unwrap();
                            let parent = if parent == "root" {
                                "myDriveRoot"
                            } else {
                                parent
                            };
                            f.parents.iter().any(|p| p == parent)
                        }
                        None => {
                            assert_eq!(q, "trashed = false");
                            true
                        }
                    })
                    .map(Gd::json)
                    .collect();
                json_reply(200, json!({ "files": hits }))
            }
            ("GET", Some(id), false) => match g.get(&id) {
                None => json_reply(404, json!({"error": {"code": 404}})),
                Some(f) if req.param("alt").as_deref() == Some("media") => {
                    Resp::Reply(200, vec![], f.data)
                }
                Some(f) => json_reply(200, Gd::json(&f)),
            },
            ("POST", None, false) => {
                let meta: Value = serde_json::from_slice(&req.body).unwrap();
                json_reply(200, Gd::json(&g.create(&meta, Vec::new())))
            }
            ("PATCH", Some(id), false) => {
                let meta: Value = serde_json::from_slice(&req.body).unwrap();
                let mut files = g.files.lock().unwrap();
                let f = files.iter_mut().find(|f| f.id == id).unwrap();
                f.trashed = meta["trashed"].as_bool().unwrap_or(f.trashed);
                json_reply(200, Gd::json(f))
            }
            (_, id, true) if req.param("uploadType").as_deref() == Some("resumable") => {
                let meta: Value = serde_json::from_slice(&req.body).unwrap();
                let mut s = g.sessions.lock().unwrap();
                s.push((id, meta, Vec::new()));
                empty(
                    200,
                    vec![("location", format!("{origin}/session/{}", s.len() - 1))],
                )
            }
            ("PATCH", Some(id), true) => {
                assert_eq!(req.param("uploadType").as_deref(), Some("media"));
                json_reply(200, Gd::json(&g.update(&id, req.body.clone())))
            }
            ("POST", None, true) => {
                assert_eq!(req.param("uploadType").as_deref(), Some("multipart"));
                let ctype = req.header("content-type").unwrap();
                let boundary = ctype.split("boundary=").nth(1).unwrap();
                let sep = format!("--{boundary}");
                let body = req.body.clone();
                let text = String::from_utf8_lossy(&body).into_owned();
                let parts: Vec<&str> = text.split(&sep).collect();
                let meta = parts[1].split("\r\n\r\n").nth(1).unwrap().trim();
                let meta: Value = serde_json::from_str(meta).unwrap();
                let data = parts[2].split_once("\r\n\r\n").unwrap().1;
                let data = data.strip_suffix("\r\n").unwrap().as_bytes().to_vec();
                json_reply(200, Gd::json(&g.create(&meta, data)))
            }
            _ => json_reply(400, json!({"error": {"code": 400}})),
        }
    })
}

/// Kantor sees: a file the user picked (in a folder Kantor can't see),
/// and its "Kantor" folder with a workbook in it.
fn gdrive_server() -> (Arc<Gd>, String, String) {
    let g = Arc::new(Gd::default());
    {
        let mut t = g.tokens.lock().unwrap();
        t.refresh = vec!["rt0".into()];
        t.secret = Some("not-so-secret".into());
    }
    g.add("picked.docx", "application/x", "hiddenFolder", b"picked");
    let k = g.add("Kantor", gdrive::FOLDER_MIME, "myDriveRoot", b"");
    g.add("a.xlsx", "application/x", &k, b"sheet one");
    let origin = serve(gdrive_handler(g.clone()));
    (g, origin, k)
}

fn gdrive_client(origin: &str, secret: &str) -> (Client, Arc<TokenSource>) {
    let (src, _) = source(origin, Some(secret));
    let c = Client::google_drive_at(
        &format!("{origin}/drive/v3/"),
        &format!("{origin}/upload/drive/v3/"),
        Auth::Source(src.clone()),
        Arc::new(gdrive::Ids::default()),
    );
    (c, src)
}

#[test]
fn gdrive_list_get_save_and_conflict() {
    let (g, origin, kantor) = gdrive_server();
    let (c, _) = gdrive_client(&origin, "not-so-secret");
    assert!(!c.has_locks());
    // Without listing first: the path is found from its folder.
    let e = c.stat("Kantor/a.xlsx").unwrap();
    assert_eq!(e.etag.as_deref(), Some("1"));
    assert_eq!(e.sha1(), Some(sha1_hex(b"sheet one")));
    let mut top: Vec<(String, bool)> = c
        .list("")
        .unwrap()
        .into_iter()
        .map(|e| (e.name, e.is_dir))
        .collect();
    top.sort();
    assert_eq!(
        top,
        vec![("Kantor".into(), true), ("picked.docx".into(), false)],
        "the top shows what isn't inside a visible folder"
    );
    assert_eq!(c.get("picked.docx").unwrap().bytes, b"picked");
    assert!(matches!(c.stat("Kantor/none.xlsx"), Err(Error::NotFound)));

    // Someone else saves: our save with the old version is refused.
    let id = g.files.lock().unwrap()[2].id.clone();
    g.update(&id, b"theirs".to_vec());
    let err = c
        .put("Kantor/a.xlsx", b"mine", Precondition::IfMatch("1"), None)
        .unwrap_err();
    assert!(matches!(err, Error::Conflict), "{err:?}");
    assert_eq!(g.get(&id).unwrap().data, b"theirs");
    let v = c
        .put("Kantor/a.xlsx", b"mine", Precondition::IfMatch("2"), None)
        .unwrap();
    assert_eq!(v.as_deref(), Some("3"));
    assert_eq!(g.get(&id).unwrap().data, b"mine");

    // A new file goes into its folder; Save As never replaces.
    c.put("Kantor/new.docx", b"fresh", Precondition::IfNoneMatch, None)
        .unwrap();
    let made = g
        .files
        .lock()
        .unwrap()
        .iter()
        .find(|f| f.name == "new.docx")
        .cloned()
        .unwrap();
    assert_eq!(
        (made.parents, made.data),
        (vec![kantor.clone()], b"fresh".to_vec())
    );
    let err = c
        .put("Kantor/new.docx", b"x", Precondition::IfNoneMatch, None)
        .unwrap_err();
    assert!(matches!(err, Error::Conflict), "{err:?}");
    // Folders: made once, found after.
    c.mkcol("Kantor/2026").unwrap();
    c.mkcol("Kantor/2026").unwrap();
    let n = g
        .files
        .lock()
        .unwrap()
        .iter()
        .filter(|f| f.name == "2026")
        .count();
    assert_eq!(n, 1);
    // Deleting moves to the bin.
    c.delete("Kantor/new.docx").unwrap();
    assert!(matches!(c.stat("Kantor/new.docx"), Err(Error::NotFound)));
    let who = gdrive::whoami(&c).unwrap();
    assert_eq!(who.1, "budi@gmail.example");
}

#[test]
fn gdrive_resumable_upload_resumes_after_a_drop() {
    let (g, origin, _) = gdrive_server();
    let (c, _) = gdrive_client(&origin, "not-so-secret");
    let r = remote(c, "gd");
    let dir = tempfile::tempdir().unwrap();
    let cache = Cache::new(dir.path());
    let bytes = pattern(kantor_cloud::chunked::RESUMABLE_MIN + 300_000);
    let entry = new_working_copy(&cache, "gd", "Kantor/big.pptx", &bytes);
    *g.drop_parts.lock().unwrap() = 2;
    let out = session::upload(&r, &cache, &entry, None, false, &mut |_, _| {});
    assert!(matches!(out, UploadOutcome::Offline(_)), "{out:?}");
    assert!(cache.pending_upload(&entry.local).is_some());
    // The link is back.
    *g.drop_parts.lock().unwrap() = 0;
    let out = session::upload(&r, &cache, &entry, None, false, &mut |_, _| {});
    let UploadOutcome::Synced { server, stamp, .. } = out else {
        panic!("{out:?}")
    };
    assert_eq!(stamp, Stamp::of(&entry.local));
    assert_eq!(server.unwrap().size, Some(bytes.len() as u64));
    let f = g
        .files
        .lock()
        .unwrap()
        .iter()
        .find(|f| f.name == "big.pptx")
        .cloned()
        .unwrap();
    assert_eq!(f.data, bytes);
    let starts = g.part_starts.lock().unwrap().clone();
    assert_eq!(starts.iter().filter(|s| **s == 0).count(), 1, "{starts:?}");
}

#[test]
fn gdrive_needs_the_client_secret_and_ends_when_revoked() {
    let (g, origin, _) = gdrive_server();
    // A wrong secret: the refresh is refused (sign-in can't go on).
    let (c, src) = gdrive_client(&origin, "wrong");
    assert!(c.list("").is_err());
    assert!(
        !src.needs_sign_in(),
        "a refused client isn't a revoked sign-in"
    );
    let (c, src) = gdrive_client(&origin, "not-so-secret");
    c.list("").unwrap();
    {
        let mut t = g.tokens.lock().unwrap();
        t.access.clear();
        t.refresh.clear();
    }
    let err = c.list("").unwrap_err();
    assert!(matches!(err, Error::Auth(401)), "{err:?}");
    assert!(src.needs_sign_in());
}

// ── browser sign-in ────────────────────────────────────────────────────────

#[test]
fn browser_sign_in_with_pkce_and_client_secret() {
    let g = Arc::new(Gd::default());
    g.tokens.lock().unwrap().secret = Some("s3".into());
    let origin = serve(gdrive_handler(g.clone()));
    let ep = Endpoints {
        issuer: origin.clone(),
        authorization_endpoint: format!("{origin}/authorize"),
        token_endpoint: format!("{origin}/token"),
        device_authorization_endpoint: None,
        revocation_endpoint: None,
    };
    let app = App {
        client_id: "cid".into(),
        client_secret: Some("s3".into()),
    };
    // The "browser": follow the redirect back with a code for our state.
    let open = |url: &str| {
        let q = parse_query(url.split_once('?').unwrap().1);
        let get = |k: &str| q.iter().find(|(n, _)| n == k).unwrap().1.clone();
        assert_eq!(get("code_challenge_method"), "S256");
        assert_eq!(get("access_type"), "offline");
        let back = format!(
            "{}/?code=the-code&state={}",
            get("redirect_uri"),
            get("state")
        );
        assert!(back.starts_with("http://127.0.0.1:"));
        std::thread::spawn(move || {
            let rest = back.strip_prefix("http://").unwrap();
            let (host, path) = rest.split_once('/').unwrap();
            let mut s = TcpStream::connect(host).unwrap();
            write!(s, "GET /{path} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
        });
    };
    let cancel = AtomicBool::new(false);
    let tokens = oauth::sign_in(
        Provider::GoogleDrive,
        &app,
        &ep,
        &open,
        &cancel,
        &DonePage::default(),
    )
    .unwrap();
    assert_eq!(tokens.access_token, "at1");
    assert_eq!(tokens.refresh_token.as_deref(), Some("rt1"));
}
