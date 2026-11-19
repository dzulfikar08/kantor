//! CMS (PKCS #7) SignedData, the container of PDF signatures, and RFC 3161
//! time-stamp tokens (a SignedData holding a TSTInfo).

use super::crypto::{self, AlgId};
use super::der::{self, Tlv};
use super::x509::Cert;

const SIGNED_DATA: &str = "1.2.840.113549.1.7.2";
pub const ATTR_CONTENT_TYPE: &str = "1.2.840.113549.1.9.3";
pub const ATTR_MESSAGE_DIGEST: &str = "1.2.840.113549.1.9.4";
pub const ATTR_SIGNING_TIME: &str = "1.2.840.113549.1.9.5";
pub const ATTR_SIGNING_CERT: &str = "1.2.840.113549.1.9.16.2.12";
pub const ATTR_SIGNING_CERT_V2: &str = "1.2.840.113549.1.9.16.2.47";
pub const ATTR_TIMESTAMP_TOKEN: &str = "1.2.840.113549.1.9.16.2.14";
/// Adobe's revocation info archival (CRLs and OCSP responses signed in).
pub const ATTR_ADOBE_REVOCATION: &str = "1.2.840.113583.1.1.8";
pub const CT_TST_INFO: &str = "1.2.840.113549.1.9.16.1.4";

/// How a SignerInfo names its certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignerId {
    IssuerSerial { issuer: Vec<u8>, serial: Vec<u8> },
    KeyId(Vec<u8>),
}

#[derive(Clone, Debug)]
pub struct Attribute {
    pub oid: String,
    /// Each value's full encoding.
    pub values: Vec<Vec<u8>>,
}

#[derive(Clone, Debug)]
pub struct SignerInfo {
    pub sid: SignerId,
    pub digest_alg: String,
    /// The signed attributes re-tagged as a SET (what was signed), and
    /// parsed.
    pub signed_attrs: Option<(Vec<u8>, Vec<Attribute>)>,
    pub sig_alg: AlgId,
    pub signature: Vec<u8>,
    pub unsigned_attrs: Vec<Attribute>,
}

impl SignerInfo {
    pub fn signed(&self, oid: &str) -> Option<&Attribute> {
        self.signed_attrs
            .as_ref()
            .and_then(|(_, a)| a.iter().find(|x| x.oid == oid))
    }

    pub fn unsigned(&self, oid: &str) -> Option<&Attribute> {
        self.unsigned_attrs.iter().find(|x| x.oid == oid)
    }

    /// Whether `cert` is the one this signer names.
    pub fn names(&self, cert: &Cert) -> bool {
        match &self.sid {
            SignerId::IssuerSerial { issuer, serial } => {
                serial == &cert.serial && super::x509::names_equal(issuer, &cert.issuer)
            }
            SignerId::KeyId(k) => cert.ext.subject_key_id.as_ref() == Some(k),
        }
    }
}

#[derive(Clone, Debug)]
pub struct SignedData {
    pub content_type: String,
    pub content: Option<Vec<u8>>,
    pub certs: Vec<Cert>,
    pub crls: Vec<Vec<u8>>,
    pub signers: Vec<SignerInfo>,
}

impl SignedData {
    /// Parse a ContentInfo holding SignedData. Trailing zero padding (the
    /// rest of a PDF's /Contents) is ignored.
    pub fn parse(data: &[u8]) -> der::Result<SignedData> {
        let ci = Tlv::parse(data)?.expect(der::SEQUENCE)?;
        let mut r = ci.reader();
        if r.read()?.oid()? != SIGNED_DATA {
            return Err(der::Error("not CMS SignedData"));
        }
        let sd = r.read_tag(der::ctx(0))?.reader().read_tag(der::SEQUENCE)?;
        let mut r = sd.reader();
        let _version = r.read_tag(der::INTEGER)?;
        let _digest_algs = r.read_tag(der::SET)?;
        let eci = r.read_tag(der::SEQUENCE)?;
        let mut e = eci.reader();
        let content_type = e.read()?.oid()?;
        let content = match e.read_opt(der::ctx(0))? {
            Some(c) => Some(c.reader().read()?.octets()?),
            None => None,
        };
        let mut certs = Vec::new();
        let mut crls = Vec::new();
        if let Some(cs) = r.read_opt(der::ctx(0))? {
            for c in cs.reader().all()? {
                // Only plain certificates (not attribute certificates).
                if c.tag == der::SEQUENCE
                    && let Ok(cert) = Cert::parse(c.raw)
                {
                    certs.push(cert);
                }
            }
        }
        if let Some(cs) = r.read_opt(der::ctx(1))? {
            for c in cs.reader().all()? {
                if c.tag == der::SEQUENCE {
                    crls.push(c.raw.to_vec());
                }
            }
        }
        let mut signers = Vec::new();
        for si in r.read_tag(der::SET)?.reader().all()? {
            signers.push(parse_signer(si)?);
        }
        Ok(SignedData {
            content_type,
            content,
            certs,
            crls,
            signers,
        })
    }
}

fn parse_attrs(t: Tlv<'_>) -> der::Result<Vec<Attribute>> {
    let mut out = Vec::new();
    for a in t.reader().all()? {
        let mut r = a.reader();
        let oid = r.read()?.oid()?;
        let values = r
            .read_tag(der::SET)?
            .reader()
            .all()?
            .into_iter()
            .map(|v| v.raw.to_vec())
            .collect();
        out.push(Attribute { oid, values });
    }
    Ok(out)
}

fn parse_signer(si: Tlv<'_>) -> der::Result<SignerInfo> {
    let mut r = si.reader();
    let _version = r.read_tag(der::INTEGER)?;
    let sid_t = r.read()?;
    let sid = if sid_t.tag == der::SEQUENCE {
        let mut s = sid_t.reader();
        let issuer = s.read_tag(der::SEQUENCE)?.raw.to_vec();
        let serial = s.read_tag(der::INTEGER)?.uint_bytes().to_vec();
        SignerId::IssuerSerial { issuer, serial }
    } else if sid_t.tag == der::ctx_prim(0) {
        SignerId::KeyId(sid_t.body.to_vec())
    } else {
        return Err(der::Error("bad signer identifier"));
    };
    let digest_alg = AlgId::parse(r.read_tag(der::SEQUENCE)?)?.oid;
    let signed_attrs = match r.read_opt(der::ctx(0))? {
        Some(t) => {
            let mut set = t.raw.to_vec();
            set[0] = der::SET;
            Some((set, parse_attrs(t)?))
        }
        None => None,
    };
    let sig_alg = AlgId::parse(r.read_tag(der::SEQUENCE)?)?;
    let signature = r.read_tag(der::OCTET_STRING)?.octets()?;
    let unsigned_attrs = match r.read_opt(der::ctx(1))? {
        Some(t) => parse_attrs(t)?,
        None => Vec::new(),
    };
    Ok(SignerInfo {
        sid,
        digest_alg,
        signed_attrs,
        sig_alg,
        signature,
        unsigned_attrs,
    })
}

/// The ESS signing-certificate attribute's first certificate hash:
/// `(digest oid, hash)`.
pub fn signing_cert_hash(attr: &Attribute, v2: bool) -> Option<(String, Vec<u8>)> {
    let sc = Tlv::parse(attr.values.first()?).ok()?;
    let certs = sc.reader().read().ok()?;
    let first = certs.reader().read().ok()?;
    let mut r = first.reader();
    if !v2 {
        return Some((crypto::SHA1.into(), r.read().ok()?.body.to_vec()));
    }
    let mut alg = crypto::SHA256.to_string();
    if r.peek_tag() == Some(der::SEQUENCE) {
        alg = AlgId::parse(r.read().ok()?).ok()?.oid;
    }
    Some((alg, r.read_tag(der::OCTET_STRING).ok()?.body.to_vec()))
}

/// RFC 3161 TSTInfo: what the time-stamp authority signed.
#[derive(Clone, Debug)]
pub struct TstInfo {
    pub hash_alg: String,
    pub hashed: Vec<u8>,
    pub time: i64,
}

impl TstInfo {
    pub fn parse(data: &[u8]) -> der::Result<TstInfo> {
        let mut r = Tlv::parse(data)?.expect(der::SEQUENCE)?.reader();
        let _version = r.read_tag(der::INTEGER)?;
        let _policy = r.read_tag(der::OID)?;
        let mut mi = r.read_tag(der::SEQUENCE)?.reader();
        let hash_alg = AlgId::parse(mi.read_tag(der::SEQUENCE)?)?.oid;
        let hashed = mi.read_tag(der::OCTET_STRING)?.body.to_vec();
        let _serial = r.read_tag(der::INTEGER)?;
        let time = r.read_tag(der::GENERALIZED_TIME)?.time()?;
        Ok(TstInfo {
            hash_alg,
            hashed,
            time,
        })
    }
}

/// The signing time attribute, if the signer wrote one.
pub fn signing_time(si: &SignerInfo) -> Option<i64> {
    let v = si.signed(ATTR_SIGNING_TIME)?.values.first()?;
    Tlv::parse(v).ok()?.time().ok()
}

/// CRLs and OCSP responses in Adobe's revocation-info-archival attribute.
pub fn adobe_revocation(si: &SignerInfo) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut crls = Vec::new();
    let mut ocsps = Vec::new();
    let Some(attr) = si.signed(ATTR_ADOBE_REVOCATION) else {
        return (crls, ocsps);
    };
    for v in &attr.values {
        let Ok(seq) = Tlv::parse(v) else { continue };
        for part in seq.reader().all().unwrap_or_default() {
            let Ok(list) = part.reader().read() else {
                continue;
            };
            for item in list.reader().all().unwrap_or_default() {
                match part.tag {
                    t if t == der::ctx(0) => crls.push(item.raw.to_vec()),
                    t if t == der::ctx(1) => ocsps.push(item.raw.to_vec()),
                    _ => {}
                }
            }
        }
    }
    (crls, ocsps)
}
