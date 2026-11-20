//! Mekari Sign: e-Meterai stamping through the Mekari eSign Open API, from
//! its public Postman collection "Mekari eSign Open API v1 - Public"
//! (https://documenter.getpostman.com/view/21582074/2s93K1oecc) and the
//! HMAC page (https://developers.mekari.com/docs/kb/hmac-authentication),
//! read 2026-11-20.
//!
//! Flow: `POST /documents/stamp {doc, filename, annotations: [{type_of:
//! "meterai", page, position_x, position_y, element_width,
//! element_height, canvas_width, canvas_height}]}` → poll `GET
//! /documents/{id}` until `stamping_status` is `success` → `GET
//! /documents/{id}/download`. Kantor has no public address for Mekari's
//! callback, so it polls.
//!
//! Requests are signed with HMAC-SHA256 (server-to-server credentials,
//! self-serve in Mekari's sandbox Developer Center): `Date` and
//! `Authorization: hmac username="…", algorithm="hmac-sha256",
//! headers="date request-line", signature="…"` over `date: …\nPOST
//! /path HTTP/1.1`.
//!
//! Not stated by Mekari, so assumed (check in the sandbox): positions are
//! measured from the page's top left in the declared canvas, which Kantor
//! sets to the page's size in points as displayed. The document type
//! (Peruri's jenis dokumen) isn't a request field; it comes from the
//! Mekari account.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::emeterai::{DocType, EMeteraiProvider, StampRequest, Stamped};
use crate::http::{Method, Request};
use crate::json::{need, str_at};
use crate::mock::{Handler, Received, Reply};
use crate::util::{b64, http_date, unb64};
use crate::{Account, Error, Field, Info, Kind, Support};

pub static INFO: Info = Info {
    id: "mekari",
    name: "Mekari Sign",
    kinds: &[Kind::EMeterai],
    support: Support::Implemented,
    docs: "https://documenter.getpostman.com/view/21582074/2s93K1oecc",
    production_url: "https://api.mekari.com/v2/esign-hmac/v1",
    sandbox_url: "https://sandbox-api.mekari.com/v2/esign-hmac/v1",
    fields: &[
        Field {
            key: "client_id",
            label: "HMAC client ID",
            secret: false,
            optional: false,
        },
        Field {
            key: "client_secret",
            label: "HMAC client secret",
            secret: true,
            optional: false,
        },
    ],
    rewrites_file: false,
    hash_signing: false,
};

/// The one "document type": Mekari takes it from the account.
pub const ACCOUNT_DOC_TYPE: &str = "account";

pub struct Mekari {
    account: Account,
    pub poll_every: Duration,
    pub poll_for: Duration,
}

/// The `Authorization` value for `method path` at `date`.
pub fn hmac_authorization(id: &str, secret: &str, date: &str, method: &str, path: &str) -> String {
    let signing = format!("date: {date}\n{method} {path} HTTP/1.1");
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let sig = b64(ring::hmac::sign(&key, signing.as_bytes()).as_ref());
    format!(
        "hmac username=\"{id}\", algorithm=\"hmac-sha256\", headers=\"date request-line\", signature=\"{sig}\""
    )
}

/// Path and query of a URL (`https://h/a/b?c` → `/a/b?c`).
fn path_of(url: &str) -> &str {
    let after = url.split_once("://").map_or(url, |(_, r)| r);
    after.find('/').map_or("/", |i| &after[i..])
}

impl Mekari {
    pub fn new(account: Account) -> Mekari {
        Mekari {
            account,
            poll_every: Duration::from_secs(3),
            poll_for: Duration::from_secs(15 * 60),
        }
    }

    fn signed(&self, mut req: Request) -> Result<crate::http::Response, Error> {
        let date = http_date(pdf_engine::signatures::now());
        let auth = hmac_authorization(
            &self.account.cred("client_id"),
            &self.account.cred("client_secret"),
            &date,
            req.method.as_str(),
            path_of(&req.url),
        );
        req = req.header("Date", date).header("Authorization", auth);
        self.account.send(req)
    }

    fn json(&self, req: Request) -> Result<Value, Error> {
        let r = self.signed(req)?;
        if !r.ok() {
            return Err(mekari_error(&r));
        }
        r.json()
    }
}

/// Mekari's 422 bodies: `{"data": {"message", "params": {field: [..]}}}`.
fn mekari_error(r: &crate::http::Response) -> Error {
    let v: Value = serde_json::from_slice(&r.body).unwrap_or(Value::Null);
    let mut parts = Vec::new();
    if let Some(m) = str_at(&v, "data/message").or_else(|| crate::json::message(&v)) {
        parts.push(m);
    }
    if let Some(p) = v.pointer("/data/params").and_then(Value::as_object) {
        for (k, msgs) in p {
            let list: Vec<String> = msgs
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|m| m.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            if !list.is_empty() {
                parts.push(format!("{k}: {}", list.join(", ")));
            }
        }
    }
    let text = if parts.is_empty() {
        crate::http::snippet(&r.body)
    } else {
        parts.join("; ")
    };
    match r.status {
        401 | 403 => Error::Auth(text),
        _ if text.contains("balance is insufficient") => Error::Quota(text),
        s => Error::Provider(format!("Mekari Sign (HTTP {s}): {text}")),
    }
}

/// The box in Mekari's canvas: the page as displayed, in points, measured
/// from the top left. Returns (x, y, w, h, canvas_w, canvas_h).
pub fn canvas_box(pdf: &[u8], page: u32, rect: [f32; 4]) -> Result<[f32; 6], Error> {
    let doc = kantor_pdf::Pdf::from_bytes(pdf, None)?;
    let (cw, ch, m) = kantor_pdf::batch::page_display(&doc, page)
        .ok_or_else(|| Error::Pdf(format!("there is no page {}", page + 1)))?;
    // Invert display → user space.
    let det = m[0] * m[3] - m[1] * m[2];
    if det.abs() < 1e-6 {
        return Err(Error::Pdf("the page can't be read".into()));
    }
    let inv = |x: f32, y: f32| {
        let (x, y) = (x - m[4], y - m[5]);
        ((m[3] * x - m[2] * y) / det, (-m[1] * x + m[0] * y) / det)
    };
    let (a, b) = (inv(rect[0], rect[1]), inv(rect[2], rect[3]));
    let (x0, x1) = (a.0.min(b.0), a.0.max(b.0));
    let (y0, y1) = (a.1.min(b.1), a.1.max(b.1));
    Ok([x0, ch - y1, x1 - x0, y1 - y0, cw, ch])
}

impl EMeteraiProvider for Mekari {
    fn info(&self) -> &'static Info {
        &INFO
    }

    fn doc_types(&self) -> Result<Vec<DocType>, Error> {
        Ok(vec![DocType {
            code: ACCOUNT_DOC_TYPE.into(),
            name: "Sesuai pengaturan akun Mekari Sign".into(),
        }])
    }

    fn balance(&self) -> Result<Option<u64>, Error> {
        // No balance call is documented.
        Ok(None)
    }

    fn test(&self) -> Result<(), Error> {
        let v = self.json(Request::get(self.account.url("/profile")))?;
        need(&v, "data/id").map(|_| ())
    }

    fn stamp(&self, pdf: &[u8], req: &StampRequest) -> Result<Stamped, Error> {
        let [x, y, w, h, cw, ch] = canvas_box(pdf, req.page, req.rect)?;
        let r1 = |f: f32| (f * 10.0).round() / 10.0;
        let body = json!({
            "doc": b64(pdf),
            "filename": req.file_name,
            "annotations": [{
                "page": req.page + 1,
                "position_x": r1(x),
                "position_y": r1(y),
                "element_width": r1(w),
                "element_height": r1(h),
                "canvas_width": r1(cw),
                "canvas_height": r1(ch),
                "type_of": "meterai"
            }]
        });
        let v = self.json(Request::post(self.account.url("/documents/stamp")).json(&body))?;
        let id = need(&v, "data/id")?;
        let start = Instant::now();
        loop {
            let d = self.json(Request::get(self.account.url(&format!("/documents/{id}"))))?;
            match str_at(&d, "data/attributes/stamping_status").as_deref() {
                Some("success") => break,
                Some("failed") => {
                    return Err(Error::Provider(format!(
                        "Mekari Sign couldn't stamp the document (document {id}); retry it from Mekari Sign"
                    )));
                }
                _ => {}
            }
            if start.elapsed() > self.poll_for {
                return Err(Error::Provider(format!(
                    "Mekari Sign hasn't finished stamping after {} minutes (document {id})",
                    self.poll_for.as_secs() / 60
                )));
            }
            std::thread::sleep(self.poll_every);
        }
        let r = self.signed(Request::new(
            Method::Get,
            self.account.url(&format!("/documents/{id}/download")),
        ))?;
        if !r.ok() {
            return Err(mekari_error(&r));
        }
        let pdf = if crate::check::looks_like_pdf(&r.body) {
            r.body
        } else {
            // A JSON answer with the file inline or a link to it.
            let v = r.json()?;
            if let Some(d) = str_at(&v, "data/attributes/doc").or_else(|| str_at(&v, "data/doc")) {
                unb64(&d).unwrap_or_default()
            } else {
                let url = str_at(&v, "data/attributes/url")
                    .or_else(|| str_at(&v, "data/url"))
                    .or_else(|| str_at(&v, "url"))
                    .ok_or_else(|| Error::Provider("Mekari Sign sent no document".into()))?;
                self.account.send(Request::get(url))?.check()?.body
            }
        };
        Ok(Stamped { pdf, serial: None })
    }
}

/// A mock of Mekari's documented API. It checks the HMAC signature;
/// `stamp` does the stamping (page 0-based, box from the top left in the
/// canvas: x, y, w, h, canvas w, canvas h).
pub fn mock(
    client_id: &str,
    client_secret: &str,
    stamp: crate::mock::CanvasStamp,
    pending: usize,
) -> Arc<Handler> {
    let (id, secret) = (client_id.to_string(), client_secret.to_string());
    let docs: crate::mock::Docs = Arc::new(Mutex::new(Default::default()));
    let counter = Arc::new(Mutex::new(0u32));
    Arc::new(move |r: &Received, _base: &str| -> Reply {
        let date = r.header("date").unwrap_or("").to_string();
        let want = hmac_authorization(&id, &secret, &date, &r.method, &r.path);
        if date.is_empty() || r.header("authorization") != Some(want.as_str()) {
            return Reply::json(
                401,
                json!({"error":"invalid_request","error_description":"HMAC signature does not match"}),
            );
        }
        let route = r.route().to_string();
        let tail = route.split("/v1").last().unwrap_or("").to_string();
        let tail = if tail.is_empty() { route.clone() } else { tail };
        match (r.method.as_str(), tail.as_str()) {
            ("GET", "/profile") => Reply::json(
                200,
                json!({"data":{"id":"0d29caaf-0000","type":"user","attributes":{"full_name":"Kantor Test","email":"test@example.com"}}}),
            ),
            ("POST", "/documents/stamp") => {
                let b = r.json();
                let Some(doc) = b.get("doc").and_then(Value::as_str).and_then(unb64) else {
                    return Reply::json(
                        422,
                        json!({"data":{"message":"Invalid field values","params":"Image not readable"},"status":"unprocessable_entity","error":true}),
                    );
                };
                if b.get("filename")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .is_empty()
                {
                    return Reply::json(
                        422,
                        json!({"data":{"message":"Validation error","params":{"filename":["can't be blank"]}}}),
                    );
                }
                let a = &b["annotations"][0];
                let f = |k: &str| a.get(k).and_then(Value::as_f64).map(|v| v as f32);
                let (Some(page), Some(x), Some(y), Some(w), Some(h), Some(cw), Some(ch)) = (
                    a.get("page").and_then(Value::as_u64),
                    f("position_x"),
                    f("position_y"),
                    f("element_width"),
                    f("element_height"),
                    f("canvas_width"),
                    f("canvas_height"),
                ) else {
                    return Reply::json(
                        422,
                        json!({"data":{"message":"Validation error","params":{"position_y[0]_signers[0]":["Param position_y is required","Param position_y type must be integer or float"]}}}),
                    );
                };
                if a.get("type_of").and_then(Value::as_str) != Some("meterai") {
                    return Reply::json(
                        422,
                        json!({"data":{"message":"Validation error","params":{"type_of":["must be meterai"]}}}),
                    );
                }
                let mut n = counter.lock().unwrap();
                *n += 1;
                let doc_id = format!("5e19bde3-11c4-4f9b-b7a6-{:012}", *n);
                let out = stamp(&doc, page.saturating_sub(1) as u32, [x, y, w, h, cw, ch]);
                docs.lock().unwrap().insert(doc_id.clone(), (out, pending));
                Reply::json(
                    200,
                    json!({"data":{"id":doc_id,"type":"document","attributes":{"filename":b["filename"],
                        "doc_url":format!("/documents/{doc_id}/download"),"status":"in_progress",
                        "created_at":"2026-11-20 00:00:00","updated_at":"2026-11-20 00:00:00"}}}),
                )
            }
            ("GET", p) if p.starts_with("/documents/") => {
                let rest = &p["/documents/".len()..];
                let (doc_id, download) = match rest.strip_suffix("/download") {
                    Some(d) => (d.to_string(), true),
                    None => (rest.to_string(), false),
                };
                let mut d = docs.lock().unwrap();
                let Some((pdf, left)) = d.get_mut(&doc_id) else {
                    return Reply::json(
                        404,
                        json!({"data":{"message":"Not found","params":{}},"status":"not_found","error":true}),
                    );
                };
                if download {
                    return Reply::bytes("application/pdf", pdf.clone());
                }
                let status = if *left > 0 {
                    *left -= 1;
                    "none"
                } else {
                    "success"
                };
                Reply::json(
                    200,
                    json!({"data":{"id":doc_id,"type":"document","attributes":{"filename":"doc.pdf","category":"global",
                        "doc_url":format!("/documents/{doc_id}/download"),"signing_status":"completed",
                        "stamping_status":status,"signers":[]}}}),
                )
            }
            _ => Reply::not_found(),
        }
    })
}
