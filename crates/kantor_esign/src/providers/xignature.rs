//! Xignature (PT Digital Tandatangan Asli): certified signatures through
//! its Open API, from the public documentation
//! (https://support.xignature.co.id/xignature-api-documentation/introduction)
//! and its OpenAPI spec, read 2026-11-20.
//!
//! "Xignature will not accept client's document to be signed. We will only
//! receive the Hash value sent by the client and return it in the form of a
//! digital signature": the document never leaves the computer. Kantor
//! builds the whole PAdES signature (signed attributes, CMS, signature
//! field) and asks Xignature only for the private-key operation:
//!
//! 1. `POST /auth/generate_pat {xignatureId}` (header `api-key`) → the
//!    signer's personal access token,
//! 2. `GET /certificate` (headers `api-key`, `pat`) → the signer's
//!    certificate chain as PEM strings,
//! 3. `POST /sign/hash {hash}` (headers `api-key`, `pat`) → `{signature}`,
//!    a raw RSA signature in base64.
//!
//! `hash` is written as in Xignature's example: base64 of the hex SHA-512
//! digest. Its documentation doesn't name the padding; Kantor verifies
//! every signature against the signer's certificate (PKCS #1 v1.5 with
//! SHA-512) before using it, so a different scheme fails with a clear
//! message instead of producing an invalid signature.

use std::sync::{Arc, Mutex};

use pdf_engine::signatures::crypto::{self, AlgId};
use pdf_engine::signatures::x509::Cert;
use pdf_engine::signing::{self, Hash, Identity, KeyType, SigningKey};
use serde_json::{Value, json};

use crate::http::Request;
use crate::json::{need, str_at};
use crate::mock::{Handler, Received, Reply};
use crate::signer::{RemoteSigner, SignRequest, Signer};
use crate::util::{b64, hex, pem_certs, unb64};
use crate::{Account, Error, Field, Info, Kind, Support};

pub static INFO: Info = Info {
    id: "xignature",
    name: "Xignature",
    kinds: &[Kind::Signing],
    support: Support::Implemented,
    docs: "https://support.xignature.co.id/xignature-api-documentation/introduction",
    production_url: "https://api.xignature.co.id/v3",
    sandbox_url: "https://api.xignature.dev/v3",
    fields: &[Field {
        key: "api_key",
        label: "API key",
        secret: true,
        optional: false,
    }],
    rewrites_file: false,
    hash_signing: true,
};

pub struct Xignature {
    account: Account,
}

impl Xignature {
    pub fn new(account: Account) -> Xignature {
        Xignature { account }
    }

    fn req(&self, r: Request) -> Request {
        r.header("api-key", self.account.cred("api_key"))
    }

    fn call(&self, r: Request) -> Result<Value, Error> {
        self.account.send(self.req(r))?.check()?.json()
    }

    /// The signer's personal access token.
    fn pat(&self, xignature_id: &str) -> Result<String, Error> {
        let v = self.call(
            Request::post(self.account.url("/auth/generate_pat"))
                .json(&json!({ "xignatureId": xignature_id })),
        )?;
        if let Some(e) = v.get("error").filter(|e| !e.is_null()) {
            return Err(Error::Auth(
                e.as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| e.to_string()),
            ));
        }
        need(&v, "data/token")
    }

    fn chain(&self, pat: &str) -> Result<Vec<Cert>, Error> {
        let r = self
            .account
            .send(
                self.req(Request::get(self.account.url("/certificate")))
                    .header("pat", pat),
            )?
            .check()?;
        let v = r.json()?;
        let pems: Vec<String> = match &v {
            Value::Array(a) => a
                .iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect(),
            other => crate::json::at(other, "data")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
        };
        let certs: Vec<Cert> = pems
            .iter()
            .flat_map(|p| pem_certs(p))
            .filter_map(|der| Cert::parse(&der).ok())
            .collect();
        if certs.is_empty() {
            return Err(Error::Provider(
                "Xignature sent no certificate for this signer".into(),
            ));
        }
        Ok(certs)
    }
}

/// The signer's key at Xignature.
struct RemoteKey {
    account: Account,
    pat: String,
    cert: Arc<Cert>,
    kind: KeyType,
}

impl SigningKey for RemoteKey {
    fn key_type(&self) -> KeyType {
        self.kind
    }

    fn hash(&self) -> Hash {
        Hash::Sha512
    }

    fn sign(&self, msg: &[u8], hash: Hash, pss: bool) -> Result<Vec<u8>, signing::Error> {
        if pss {
            return Err(signing::Error::Unsupported(
                "Xignature signs with PKCS #1 v1.5 only".into(),
            ));
        }
        let digest = hash.digest(msg);
        let body = json!({ "hash": b64(hex(&digest).as_bytes()) });
        let resp = self
            .account
            .send(
                Request::post(self.account.url("/sign/hash"))
                    .header("api-key", self.account.cred("api_key"))
                    .header("pat", self.pat.clone())
                    .json(&body),
            )
            .and_then(|r| r.check())
            .and_then(|r| r.json())
            .map_err(|e| signing::Error::Key(e.to_string()))?;
        let sig = str_at(&resp, "signature")
            .or_else(|| str_at(&resp, "data/signature"))
            .and_then(|s| unb64(&s))
            .ok_or_else(|| signing::Error::Key("Xignature sent no signature".into()))?;
        // Check it before it goes into the document.
        let alg = AlgId {
            oid: crypto::RSA.to_string(),
            params: Vec::new(),
        };
        crypto::verify(&self.cert.key, &alg, Some(hash.oid()), msg, &sig).map_err(|_| {
            signing::Error::Key(
                "Xignature's signature doesn't match the signer's certificate (expected RSA PKCS #1 v1.5 over SHA-512)".into(),
            )
        })?;
        Ok(sig)
    }
}

impl RemoteSigner for Xignature {
    fn info(&self) -> &'static Info {
        &INFO
    }

    fn test(&self) -> Result<(), Error> {
        // Any authenticated call: the billing usage of this month.
        let r = self
            .account
            .send(self.req(Request::get(self.account.url("/billing/usage"))))?;
        r.check().map(|_| ())
    }

    fn identity(&self, signer: &Signer) -> Option<Result<Identity, Error>> {
        Some((|| {
            if signer.id.trim().is_empty() {
                return Err(Error::Provider("type the signer's Xignature ID".into()));
            }
            let pat = self.pat(signer.id.trim())?;
            let mut certs = self.chain(&pat)?;
            let cert = Arc::new(certs.remove(0));
            let kind = KeyType::of(&cert.key)
                .filter(|k| k.is_rsa())
                .ok_or_else(|| Error::Provider("the signer's key isn't RSA".into()))?;
            Ok(Identity {
                cert: cert.clone(),
                chain: certs.into_iter().map(Arc::new).collect(),
                key: Box::new(RemoteKey {
                    account: self.account.clone(),
                    pat,
                    cert,
                    kind,
                }),
                source: signing::Source::Remote(INFO.id.to_string()),
            })
        })())
    }

    fn sign_document(&self, _: &[u8], _: &SignRequest, _: &Signer) -> Result<Vec<u8>, Error> {
        Err(Error::NotImplemented(
            "Xignature signs hashes; Kantor builds the signature".into(),
        ))
    }
}

/// A mock of Xignature's documented Open API: `api_key` is the account's
/// key, `signers` maps an Xignature ID to its chain (PEM, signer first),
/// and `sign_digest` makes the raw RSA signature of a SHA-512 digest.
pub fn mock(
    api_key: &str,
    signers: Vec<(String, Vec<String>)>,
    sign_digest: crate::mock::DigestSigner,
) -> Arc<Handler> {
    let key = api_key.to_string();
    let pats: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    Arc::new(move |r: &Received, _base: &str| -> Reply {
        if r.header("api-key") != Some(key.as_str()) {
            return Reply::json(401, json!({"statusCode":401,"message":"Unauthorized"}));
        }
        let route = r.route();
        let tail = route.strip_prefix("/v3").unwrap_or(route);
        let signer_of = |r: &Received| -> Option<String> {
            let pat = r.header("pat")?;
            pats.lock()
                .unwrap()
                .iter()
                .find(|(_, p)| p == pat)
                .map(|(id, _)| id.clone())
        };
        match (r.method.as_str(), tail) {
            ("GET", "/billing/usage") => {
                Reply::json(200, json!({"statusCode":200,"data":{"usage":0}}))
            }
            ("POST", "/auth/generate_pat") => {
                let id = r.json()["xignatureId"].as_str().unwrap_or("").to_string();
                if !signers.iter().any(|(s, _)| *s == id) {
                    return Reply::json(
                        400,
                        json!({"statusCode":400,"message":"xignatureId not found"}),
                    );
                }
                let mut p = pats.lock().unwrap();
                let token = format!("pat{}{}", p.len(), "tcPDrBLYsAGzwHijVFFRTOWzfFFT0g");
                p.push((id, token.clone()));
                Reply::json(201, json!({"error":null,"data":{"token":token}}))
            }
            ("GET", "/certificate") => match signer_of(r) {
                Some(id) => {
                    let chain = signers
                        .iter()
                        .find(|(s, _)| *s == id)
                        .map(|(_, c)| c.clone())
                        .unwrap_or_default();
                    // As in Xignature's example: PEM without line breaks.
                    let flat: Vec<String> =
                        chain.iter().map(|p| p.replace(['\n', '\r'], "")).collect();
                    Reply::json(200, json!(flat))
                }
                None => Reply::json(401, json!({"statusCode":401,"message":"invalid pat"})),
            },
            ("POST", "/sign/hash") => {
                if signer_of(r).is_none() {
                    return Reply::json(401, json!({"statusCode":401,"message":"invalid pat"}));
                }
                let h = r.json()["hash"]
                    .as_str()
                    .and_then(unb64)
                    .unwrap_or_default();
                let hex_text = String::from_utf8_lossy(&h).to_string();
                let digest: Option<Vec<u8>> = (hex_text.len() == 128)
                    .then(|| {
                        (0..64)
                            .map(|i| u8::from_str_radix(&hex_text[2 * i..2 * i + 2], 16).ok())
                            .collect()
                    })
                    .flatten();
                match digest {
                    Some(d) => Reply::json(201, json!({"signature": b64(&sign_digest(&d))})),
                    None => Reply::json(
                        400,
                        json!({"statusCode":400,"message":"hash must be base64 of a SHA-512 hex digest"}),
                    ),
                }
            }
            _ => Reply::not_found(),
        }
    })
}
