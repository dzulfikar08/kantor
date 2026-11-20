//! VIDA (PT Indonesia Digital Identity): e-Meterai API, from its public
//! documentation (https://docs.vida.id/identity-stack/sign/e-meterai-stamp/api-integration/e-meterai-api/api-reference,
//! read 2026-11-20).
//!
//! Flow: OAuth2 client-credentials token from VIDA's SSO → `GET quota`,
//! `GET doctype` → `POST upload {codeDocument, page, llx, lly, refNum,
//! document}` → poll `GET docstamp/{refNum}` until the stamped PDF and its
//! serial number come back (code 31 = still pending at Peruri). Every call
//! carries `Authorization: Bearer` and `X-PARTNER-ID: <client_id>`.
//!
//! Not in the documentation, so settable: the stamp gateway's production
//! address (the sandbox one, `sandbox-stamp-gateway.np.vida.id`, comes from
//! a community client) and the production token address. VIDA takes only
//! the stamp's lower-left corner (taken as PDF points from the page's bottom
//! left, to confirm with a sandbox account); the stamp's size is VIDA's.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::emeterai::{DocType, EMeteraiProvider, StampRequest, Stamped};
use crate::http::Request;
use crate::json::{at, message, need, str_at};
use crate::mock::{Handler, Received, Reply};
use crate::util::{b64, unb64, uuid4};
use crate::{Account, Error, Field, Info, Kind, Support};

pub static INFO: Info = Info {
    id: "vida",
    name: "VIDA",
    kinds: &[Kind::EMeterai],
    support: Support::Implemented,
    docs: "https://docs.vida.id/identity-stack/sign/e-meterai-stamp",
    // Not published: VIDA gives it with the production credentials.
    production_url: "",
    sandbox_url: "https://sandbox-stamp-gateway.np.vida.id",
    fields: &[
        Field {
            key: "client_id",
            label: "Client ID",
            secret: false,
            optional: false,
        },
        Field {
            key: "client_secret",
            label: "Client secret",
            secret: true,
            optional: false,
        },
        Field {
            key: "token_url",
            label: "Token address (from VIDA; optional in the sandbox)",
            secret: false,
            optional: true,
        },
    ],
    rewrites_file: false,
    hash_signing: false,
};

/// VIDA's sandbox SSO token endpoint (e-Meterai API authentication page).
pub const SANDBOX_TOKEN_URL: &str =
    "https://qa-sso-26.np.vida.id/auth/realms/vida/protocol/openid-connect/token";

/// The token path on VIDA's Keycloak SSO.
const TOKEN_PATH: &str = "/auth/realms/vida/protocol/openid-connect/token";

pub struct Vida {
    account: Account,
    token: Mutex<Option<(String, Instant)>>,
    /// How often and how long to ask whether Peruri has stamped.
    pub poll_every: Duration,
    pub poll_for: Duration,
}

impl Vida {
    pub fn new(account: Account) -> Vida {
        Vida {
            account,
            token: Mutex::new(None),
            poll_every: Duration::from_secs(3),
            poll_for: Duration::from_secs(15 * 60),
        }
    }

    fn token_url(&self) -> Result<String, Error> {
        let a = &self.account;
        if let Some(u) = a.creds.get("token_url") {
            return Ok(u.to_string());
        }
        if !a.settings.base_url.trim().is_empty() {
            return Ok(format!("{}{TOKEN_PATH}", a.base_url()));
        }
        if a.settings.sandbox {
            return Ok(SANDBOX_TOKEN_URL.to_string());
        }
        Err(Error::NotConfigured(
            "VIDA (production): type the token address VIDA gave you".into(),
        ))
    }

    fn token(&self) -> Result<String, Error> {
        if let Ok(t) = self.token.lock()
            && let Some((tok, until)) = &*t
            && Instant::now() < *until
        {
            return Ok(tok.clone());
        }
        let a = &self.account;
        let resp = a
            .send(Request::post(self.token_url()?).form(&[
                ("grant_type", "client_credentials"),
                ("client_id", &a.cred("client_id")),
                ("client_secret", &a.cred("client_secret")),
            ]))?
            .check()?;
        let v = resp.json()?;
        let tok = need(&v, "access_token")?;
        let secs = at(&v, "expires_in").and_then(Value::as_u64).unwrap_or(300);
        if let Ok(mut t) = self.token.lock() {
            // Renew a little early.
            *t = Some((
                tok.clone(),
                Instant::now() + Duration::from_secs(secs.saturating_sub(30).max(10)),
            ));
        }
        Ok(tok)
    }

    fn base(&self) -> Result<String, Error> {
        let b = self.account.base_url();
        if b.is_empty() {
            return Err(Error::NotConfigured(
                "VIDA (production): type the stamp gateway address VIDA gave you".into(),
            ));
        }
        Ok(b)
    }

    fn call(&self, req: Request) -> Result<Value, Error> {
        let tok = self.token()?;
        let req = req
            .bearer(&tok)
            .header("X-PARTNER-ID", self.account.cred("client_id"));
        let resp = self.account.send(req)?;
        // VIDA answers errors with a JSON body and a responseCode.
        let v = if resp.ok() {
            resp.json()?
        } else {
            match resp.json() {
                Ok(v) if at(&v, "responseCode").is_some() => v,
                _ => return Err(resp.check().err().unwrap_or(Error::Provider("?".into()))),
            }
        };
        Ok(v)
    }
}

/// VIDA's response codes (API reference ▸ Response codes) as errors.
fn code_error(v: &Value) -> Option<Error> {
    let code = at(v, "responseCode").and_then(Value::as_i64)?;
    if code == 200 {
        return None;
    }
    let msg = message(v).unwrap_or_default();
    let refund = str_at(v, "refund_status").unwrap_or_default();
    let text = |what: &str| {
        let mut s = format!("{what} (VIDA code {code})");
        if !msg.is_empty() {
            s.push_str(&format!(": {msg}"));
        }
        if refund == "REFUNDED" {
            s.push_str("; the meterai was refunded");
        }
        s
    };
    Some(match code {
        21 | 54 => Error::Quota(text("no e-meterai quota or balance left")),
        23 | 40 => Error::Auth(text("the account isn't known")),
        24 => Error::Provider(text("VIDA doesn't know this document type")),
        30 => Error::Provider(text("stamping failed")),
        _ => Error::Provider(text("VIDA refused the request")),
    })
}

impl EMeteraiProvider for Vida {
    fn info(&self) -> &'static Info {
        &INFO
    }

    fn doc_types(&self) -> Result<Vec<DocType>, Error> {
        let v = self.call(Request::get(format!(
            "{}/api/v1/emeterai/doctype",
            self.base()?
        )))?;
        if let Some(e) = code_error(&v) {
            return Err(e);
        }
        let list = at(&v, "data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(list
            .iter()
            .filter_map(|d| {
                Some(DocType {
                    code: str_at(d, "codeDocument")?,
                    name: str_at(d, "nameDocument").unwrap_or_default(),
                })
            })
            .collect())
    }

    fn balance(&self) -> Result<Option<u64>, Error> {
        let v = self.call(Request::get(format!(
            "{}/api/v1/emeterai/quota",
            self.base()?
        )))?;
        if let Some(e) = code_error(&v) {
            return Err(e);
        }
        // The quota is a string in VIDA's example.
        Ok(str_at(&v, "data/quota").and_then(|q| q.trim().parse().ok()))
    }

    fn stamp(&self, pdf: &[u8], req: &StampRequest) -> Result<Stamped, Error> {
        let base = self.base()?;
        let ref_num = uuid4();
        let num = |f: f32| format!("{}", (f * 100.0).round() / 100.0);
        let body = json!({
            "codeDocument": req.doc_type,
            "page": (req.page + 1).to_string(),
            "llx": num(req.rect[0]),
            "lly": num(req.rect[1]),
            "refNum": ref_num,
            "document": b64(pdf),
        });
        let v = self.call(Request::post(format!("{base}/api/v1/emeterai/upload")).json(&body))?;
        if let Some(e) = code_error(&v) {
            return Err(e);
        }
        // Poll until Peruri has stamped (31 = pending; 25 = the file was
        // already deleted: VIDA says to retry the status three times).
        let start = Instant::now();
        let mut deleted = 0;
        loop {
            let v = self.call(Request::get(format!(
                "{base}/api/v1/emeterai/docstamp/{ref_num}"
            )))?;
            let code = at(&v, "responseCode").and_then(Value::as_i64).unwrap_or(0);
            match code {
                200 => {
                    let doc = need(&v, "data/document")?;
                    let pdf = unb64(&doc).ok_or_else(|| {
                        Error::Provider("VIDA's stamped document isn't valid base64".into())
                    })?;
                    return Ok(Stamped {
                        pdf,
                        serial: str_at(&v, "data/serialNumber"),
                    });
                }
                31 => {}
                25 if deleted < 3 => deleted += 1,
                _ => {
                    return Err(code_error(&v).unwrap_or_else(|| {
                        Error::Provider(format!(
                            "VIDA: {}",
                            crate::http::snippet(v.to_string().as_bytes())
                        ))
                    }));
                }
            }
            if start.elapsed() > self.poll_for {
                return Err(Error::Provider(format!(
                    "VIDA hasn't finished stamping after {} minutes (reference {ref_num}); check its dashboard before trying again",
                    self.poll_for.as_secs() / 60
                )));
            }
            std::thread::sleep(self.poll_every);
        }
    }
}

/// A mock of VIDA's documented e-Meterai API. `stamp` does Peruri's part
/// (page 0-based, lower-left corner); `pending` status answers come before
/// each document is ready.
pub fn mock(
    client_id: &str,
    client_secret: &str,
    stamp: crate::mock::CornerStamp,
    pending: usize,
) -> Arc<Handler> {
    let (id, secret) = (client_id.to_string(), client_secret.to_string());
    let docs: crate::mock::Docs = Arc::new(Mutex::new(Default::default()));
    let quota = Arc::new(Mutex::new(5u32));
    Arc::new(move |r: &Received, _base: &str| -> Reply {
        let route = r.route();
        if route == TOKEN_PATH {
            let p = r.parts();
            let get = |k: &str| {
                p.iter()
                    .find(|x| x.name == k)
                    .map(|x| String::from_utf8_lossy(&x.data).into_owned())
                    .unwrap_or_default()
            };
            if get("grant_type") != "client_credentials" {
                return Reply::json(
                    400,
                    json!({"error":"unsupported_grant_type","error_description":"Unsupported grant_type"}),
                );
            }
            if get("client_id") != id || get("client_secret") != secret {
                return Reply::json(
                    401,
                    json!({"error":"unauthorized_client","error_description":"INVALID_CREDENTIALS: Invalid client credentials"}),
                );
            }
            return Reply::json(
                200,
                json!({"access_token":"vida-mock-token","token_type":"bearer","expires_in":899,"scope":"read write","jti":"j1"}),
            );
        }
        if r.header("authorization") != Some("Bearer vida-mock-token") {
            return Reply::json(401, json!({"error":"invalid_token"}));
        }
        if r.header("x-partner-id").unwrap_or("").is_empty() {
            return Reply::json(
                400,
                json!({"responseCode":23,"status":"ERROR","message":"X-PARTNER-ID is empty","refNum":""}),
            );
        }
        match (r.method.as_str(), route) {
            ("GET", "/api/v1/emeterai/quota") => {
                let q = *quota.lock().unwrap();
                Reply::json(
                    200,
                    json!({"responseCode":200,"message":"Success","refNum":"1703820925","data":{"quota":q.to_string(),"account":"1699856766"}}),
                )
            }
            ("GET", "/api/v1/emeterai/doctype") => Reply::json(
                200,
                json!({"responseCode":200,"message":"Success","data":[
                    {"codeDocument":"VD001","nameDocument":"Dokumen pelunasan utang (lebih dari 5 juta)"},
                    {"codeDocument":"VD004","nameDocument":"Surat Pernyataan"},
                    {"codeDocument":"VD010","nameDocument":"Surat Perjanjian"}
                ]}),
            ),
            ("POST", "/api/v1/emeterai/upload") => {
                let b = r.json();
                let s = |k: &str| b.get(k).and_then(Value::as_str).map(str::to_string);
                let (Some(code), Some(page), Some(llx), Some(lly), Some(rn), Some(doc)) = (
                    s("codeDocument"),
                    s("page"),
                    s("llx"),
                    s("lly"),
                    s("refNum"),
                    s("document"),
                ) else {
                    return Reply::json(
                        400,
                        json!({"responseCode":20,"status":"ERROR","message":"{document=document Not Empty & base64}","refNum":""}),
                    );
                };
                if !code.starts_with("VD") {
                    return Reply::json(
                        400,
                        json!({"responseCode":24,"status":"ERROR","message":"Code Document not found","refNum":rn}),
                    );
                }
                let mut q = quota.lock().unwrap();
                if *q == 0 {
                    return Reply::json(
                        400,
                        json!({"responseCode":21,"status":"ERROR","message":"Process Get QR failed","refNum":rn}),
                    );
                }
                let mut d = docs.lock().unwrap();
                if d.contains_key(&rn) {
                    return Reply::json(
                        400,
                        json!({"responseCode":11,"status":"ERROR","message":"Duplicate refNum","refNum":rn}),
                    );
                }
                let Some(pdf) = unb64(&doc) else {
                    return Reply::json(
                        400,
                        json!({"responseCode":20,"status":"ERROR","message":"{document=document Not Empty & base64}","refNum":""}),
                    );
                };
                let (Ok(page), Ok(x), Ok(y)) =
                    (page.parse::<u32>(), llx.parse::<f32>(), lly.parse::<f32>())
                else {
                    return Reply::json(
                        400,
                        json!({"responseCode":20,"status":"ERROR","message":"bad page or position","refNum":rn}),
                    );
                };
                *q -= 1;
                let out = stamp(&pdf, page.saturating_sub(1), x, y);
                d.insert(rn.clone(), (out, pending));
                Reply::json(
                    200,
                    json!({"responseCode":200,"message":"Upload Success","refNum":rn,"refund_status":"NOT_REFUNDED","data":null}),
                )
            }
            ("GET", p) if p.starts_with("/api/v1/emeterai/docstamp/") => {
                let rn = p.rsplit('/').next().unwrap_or("").to_string();
                let mut d = docs.lock().unwrap();
                let Some((pdf, left)) = d.get_mut(&rn) else {
                    return Reply::json(
                        200,
                        json!({"responseCode":12,"message":"refNum not found","refNum":rn}),
                    );
                };
                if *left > 0 {
                    *left -= 1;
                    return Reply::json(
                        200,
                        json!({"responseCode":31,"message":"Sign Pending","refNum":rn,"refund_status":"NOT_REFUNDED","data":null}),
                    );
                }
                Reply::json(
                    200,
                    json!({"responseCode":200,"message":"Signing Success","refNum":rn,"refund_status":"NOT_REFUNDED",
                        "data":{"document":b64(pdf),"serialNumber":"08MO5DUHLD0GUV2N0000A9","ref_id":rn}}),
                )
            }
            _ => Reply::not_found(),
        }
    })
}
