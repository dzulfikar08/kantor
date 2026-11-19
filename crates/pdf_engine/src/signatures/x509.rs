//! X.509 certificates: the parts the checker reads (names, validity, key,
//! extensions) and the details the certificate dialog shows.

use super::crypto::{self, AlgId};
use super::der::{self, Tlv};

pub const KU_DIGITAL_SIGNATURE: u16 = 1 << 0;
pub const KU_NON_REPUDIATION: u16 = 1 << 1;
pub const KU_KEY_CERT_SIGN: u16 = 1 << 5;
pub const KU_CRL_SIGN: u16 = 1 << 6;

pub const EKU_TIME_STAMPING: &str = "1.3.6.1.5.5.7.3.8";
pub const EKU_OCSP_SIGNING: &str = "1.3.6.1.5.5.7.3.9";

/// The public key of a certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicKey {
    /// Algorithm OID (rsaEncryption, id-ecPublicKey…).
    pub alg: String,
    /// The named curve for EC keys.
    pub curve: Option<String>,
    /// The BIT STRING's bytes: an RSAPublicKey, or an EC point.
    pub key: Vec<u8>,
}

impl PublicKey {
    /// "RSA 2048 bits", "ECDSA P-256".
    pub fn describe(&self) -> String {
        match self.alg.as_str() {
            crypto::RSA => {
                let bits = Tlv::parse(&self.key)
                    .ok()
                    .and_then(|t| t.reader().read().ok())
                    .map(|n| n.uint_bytes().len() * 8)
                    .unwrap_or(0);
                format!("RSA {bits} bits")
            }
            crypto::EC_PUBLIC_KEY => match self.curve.as_deref() {
                Some(crypto::P256) => "ECDSA P-256".into(),
                Some(crypto::P384) => "ECDSA P-384".into(),
                Some(crypto::P521) => "ECDSA P-521".into(),
                other => format!("ECDSA {}", other.unwrap_or("?")),
            },
            other => other.to_string(),
        }
    }
}

/// A parsed certificate.
#[derive(Clone, Debug)]
pub struct Cert {
    pub der: Vec<u8>,
    tbs: std::ops::Range<usize>,
    pub sig_alg: AlgId,
    pub signature: Vec<u8>,
    pub version: u8,
    pub serial: Vec<u8>,
    pub issuer: Vec<u8>,
    pub subject: Vec<u8>,
    pub not_before: i64,
    pub not_after: i64,
    pub key: PublicKey,
    pub ext: Extensions,
}

#[derive(Clone, Debug, Default)]
pub struct Extensions {
    pub key_usage: Option<u16>,
    pub ca: Option<bool>,
    pub path_len: Option<u64>,
    pub eku: Vec<String>,
    pub subject_key_id: Option<Vec<u8>>,
    pub authority_key_id: Option<Vec<u8>>,
    pub ocsp_urls: Vec<String>,
    pub ca_issuer_urls: Vec<String>,
    pub crl_urls: Vec<String>,
    pub ocsp_no_check: bool,
    pub policies: Vec<String>,
    /// Critical extensions the checker doesn't understand.
    pub unknown_critical: Vec<String>,
}

impl Cert {
    pub fn parse(der_bytes: &[u8]) -> der::Result<Cert> {
        let top = Tlv::parse(der_bytes)?.expect(der::SEQUENCE)?;
        let der_bytes = top.raw;
        let mut r = top.reader();
        let tbs = r.read_tag(der::SEQUENCE)?;
        let sig_alg = AlgId::parse(r.read_tag(der::SEQUENCE)?)?;
        let signature = r.read_tag(der::BIT_STRING)?.bits()?.to_vec();
        let start = tbs.raw.as_ptr() as usize - der_bytes.as_ptr() as usize;
        let tbs_range = start..start + tbs.raw.len();

        let mut t = tbs.reader();
        let version = match t.read_opt(der::ctx(0))? {
            Some(v) => v.reader().read()?.small_int()? as u8 + 1,
            None => 1,
        };
        let serial = t.read_tag(der::INTEGER)?.uint_bytes().to_vec();
        let _inner_alg = t.read_tag(der::SEQUENCE)?;
        let issuer = t.read_tag(der::SEQUENCE)?.raw.to_vec();
        let mut validity = t.read_tag(der::SEQUENCE)?.reader();
        let not_before = validity.read()?.time()?;
        let not_after = validity.read()?.time()?;
        let subject = t.read_tag(der::SEQUENCE)?.raw.to_vec();
        let key = parse_spki(t.read_tag(der::SEQUENCE)?)?;
        let mut ext = Extensions::default();
        while !t.is_empty() {
            let el = t.read()?;
            if el.tag == der::ctx(3) {
                let list = el.reader().read_tag(der::SEQUENCE)?;
                for e in list.reader().all()? {
                    read_extension(e, &mut ext)?;
                }
            }
        }
        Ok(Cert {
            der: der_bytes.to_vec(),
            tbs: tbs_range,
            sig_alg,
            signature,
            version,
            serial,
            issuer,
            subject,
            not_before,
            not_after,
            key,
            ext,
        })
    }

    pub fn tbs(&self) -> &[u8] {
        &self.der[self.tbs.clone()]
    }

    pub fn is_self_issued(&self) -> bool {
        names_equal(&self.issuer, &self.subject)
    }

    /// Whether `issuer` signed this certificate.
    pub fn signed_by(&self, issuer: &Cert) -> bool {
        crypto::verify(
            &issuer.key,
            &self.sig_alg,
            None,
            self.tbs(),
            &self.signature,
        )
        .is_ok()
    }

    pub fn subject_name(&self) -> Name {
        Name::parse(&self.subject)
    }

    pub fn issuer_name(&self) -> Name {
        Name::parse(&self.issuer)
    }

    pub fn sha256(&self) -> Vec<u8> {
        crypto::sha256(&self.der)
    }

    pub fn sha1(&self) -> Vec<u8> {
        crypto::digest(crypto::SHA1, &self.der).unwrap_or_default()
    }

    /// The same certificate (byte for byte).
    pub fn same(&self, other: &Cert) -> bool {
        self.der == other.der
    }

    /// Same subject and key: a re-issued copy of the same CA counts as
    /// the trust anchor.
    pub fn same_entity(&self, other: &Cert) -> bool {
        self.key == other.key && names_equal(&self.subject, &other.subject)
    }

    pub fn valid_at(&self, t: i64) -> bool {
        self.not_before <= t && t <= self.not_after
    }

    pub fn is_ca(&self) -> bool {
        self.ext.ca == Some(true) || (self.version < 3 && self.is_self_issued())
    }
}

/// Distinguished names compare byte for byte, then attribute by attribute
/// (case and spacing aside) for CAs that re-encode names.
pub fn names_equal(a: &[u8], b: &[u8]) -> bool {
    if a == b {
        return true;
    }
    let (na, nb) = (Name::parse(a), Name::parse(b));
    !na.0.is_empty()
        && na.0.len() == nb.0.len()
        && na
            .0
            .iter()
            .zip(&nb.0)
            .all(|(x, y)| x.0 == y.0 && normalize(&x.1) == normalize(&y.1))
}

fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn parse_spki(spki: Tlv<'_>) -> der::Result<PublicKey> {
    let mut r = spki.reader();
    let alg = r.read_tag(der::SEQUENCE)?;
    let mut a = alg.reader();
    let oid = a.read()?.oid()?;
    let curve = match a.read_opt(der::OID)? {
        Some(c) => Some(c.oid()?),
        None => None,
    };
    let key = r.read_tag(der::BIT_STRING)?.bits()?.to_vec();
    Ok(PublicKey {
        alg: oid,
        curve,
        key,
    })
}

fn read_extension(e: Tlv<'_>, ext: &mut Extensions) -> der::Result<()> {
    let mut r = e.reader();
    let oid = r.read()?.oid()?;
    let critical = match r.read_opt(der::BOOLEAN)? {
        Some(b) => b.body.first().is_some_and(|&v| v != 0),
        None => false,
    };
    let value = r.read_tag(der::OCTET_STRING)?.body;
    let parsed: der::Result<bool> = (|| {
        match oid.as_str() {
            // keyUsage
            "2.5.29.15" => {
                let bits = Tlv::parse(value)?.expect(der::BIT_STRING)?;
                let bytes = bits.bits()?;
                let mut ku = 0u16;
                for i in 0..9 {
                    let byte = bytes.get(i / 8).copied().unwrap_or(0);
                    if byte & (0x80 >> (i % 8)) != 0 {
                        ku |= 1 << i;
                    }
                }
                ext.key_usage = Some(ku);
            }
            // basicConstraints
            "2.5.29.19" => {
                let mut s = Tlv::parse(value)?.reader();
                let mut ca = false;
                if let Some(b) = s.read_opt(der::BOOLEAN)? {
                    ca = b.body.first().is_some_and(|&v| v != 0);
                }
                if let Some(n) = s.read_opt(der::INTEGER)? {
                    ext.path_len = n.small_int().ok();
                }
                ext.ca = Some(ca);
            }
            // extKeyUsage
            "2.5.29.37" => {
                for k in Tlv::parse(value)?.reader().all()? {
                    ext.eku.push(k.oid()?);
                }
            }
            // subjectKeyIdentifier
            "2.5.29.14" => ext.subject_key_id = Some(Tlv::parse(value)?.body.to_vec()),
            // authorityKeyIdentifier
            "2.5.29.35" => {
                let mut s = Tlv::parse(value)?.reader();
                if let Some(k) = s.read_opt(der::ctx_prim(0))? {
                    ext.authority_key_id = Some(k.body.to_vec());
                }
            }
            // authorityInfoAccess
            "1.3.6.1.5.5.7.1.1" => {
                for ad in Tlv::parse(value)?.reader().all()? {
                    let mut a = ad.reader();
                    let method = a.read()?.oid()?;
                    let loc = a.read()?;
                    if loc.tag == der::ctx_prim(6) {
                        let url = String::from_utf8_lossy(loc.body).into_owned();
                        match method.as_str() {
                            "1.3.6.1.5.5.7.48.1" => ext.ocsp_urls.push(url),
                            "1.3.6.1.5.5.7.48.2" => ext.ca_issuer_urls.push(url),
                            _ => {}
                        }
                    }
                }
            }
            // cRLDistributionPoints: every URI in a full name.
            "2.5.29.31" => {
                let mut urls = Vec::new();
                collect_uris(Tlv::parse(value)?, &mut urls)?;
                ext.crl_urls.extend(urls);
            }
            // id-pkix-ocsp-nocheck
            "1.3.6.1.5.5.7.48.1.5" => ext.ocsp_no_check = true,
            // certificatePolicies
            "2.5.29.32" => {
                for p in Tlv::parse(value)?.reader().all()? {
                    if let Ok(id) = p.reader().read() {
                        ext.policies.push(id.oid()?);
                    }
                }
            }
            // Understood well enough to ignore: subjectAltName, policy
            // constraints/mappings, name constraints (not enforced), qcStatements.
            "2.5.29.17" | "2.5.29.18" | "2.5.29.36" | "2.5.29.33" | "2.5.29.30"
            | "1.3.6.1.5.5.7.1.3" | "2.5.29.54" => {}
            _ => return Ok(false),
        }
        Ok(true)
    })();
    match parsed {
        Ok(true) => {}
        Ok(false) | Err(_) if critical => ext.unknown_critical.push(oid),
        _ => {}
    }
    Ok(())
}

fn collect_uris(t: Tlv<'_>, out: &mut Vec<String>) -> der::Result<()> {
    if t.tag == der::ctx_prim(6) {
        if let Ok(s) = std::str::from_utf8(t.body) {
            out.push(s.to_string());
        }
    } else if t.tag & 0x20 != 0 {
        for k in t.reader().all()? {
            collect_uris(k, out)?;
        }
    }
    Ok(())
}

/// A distinguished name as (attribute, value) pairs in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Name(pub Vec<(String, String)>);

impl Name {
    pub fn parse(raw: &[u8]) -> Name {
        let mut out = Vec::new();
        let Ok(seq) = Tlv::parse(raw) else {
            return Name(out);
        };
        let mut r = seq.reader();
        while let Ok(rdn) = r.read() {
            let mut set = rdn.reader();
            while let Ok(atv) = set.read() {
                let mut a = atv.reader();
                let (Ok(oid), Ok(val)) = (a.read().and_then(|o| o.oid()), a.read()) else {
                    continue;
                };
                out.push((short_name(&oid), val.text()));
            }
        }
        Name(out)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The common name, or the organisation, or the whole name.
    pub fn display_name(&self) -> String {
        self.get("CN")
            .or_else(|| self.get("O"))
            .map(str::to_string)
            .unwrap_or_else(|| self.to_string())
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts: Vec<String> = self.0.iter().map(|(k, v)| format!("{k}={v}")).collect();
        f.write_str(&parts.join(", "))
    }
}

fn short_name(oid: &str) -> String {
    match oid {
        "2.5.4.3" => "CN",
        "2.5.4.4" => "SN",
        "2.5.4.5" => "serialNumber",
        "2.5.4.6" => "C",
        "2.5.4.7" => "L",
        "2.5.4.8" => "ST",
        "2.5.4.9" => "street",
        "2.5.4.10" => "O",
        "2.5.4.11" => "OU",
        "2.5.4.12" => "title",
        "2.5.4.42" => "GN",
        "2.5.4.97" => "organizationIdentifier",
        "1.2.840.113549.1.9.1" => "E",
        "0.9.2342.19200300.100.1.25" => "DC",
        "0.9.2342.19200300.100.1.1" => "UID",
        other => return other.to_string(),
    }
    .to_string()
}

/// Key usage bits as names, in the order RFC 5280 lists them.
pub fn key_usage_names(ku: u16) -> Vec<&'static str> {
    const NAMES: [&str; 9] = [
        "Digital Signature",
        "Non-Repudiation",
        "Key Encipherment",
        "Data Encipherment",
        "Key Agreement",
        "Certificate Sign",
        "CRL Sign",
        "Encipher Only",
        "Decipher Only",
    ];
    (0..9)
        .filter(|i| ku & (1 << i) != 0)
        .map(|i| NAMES[i])
        .collect()
}

/// Extended key usage OIDs as names where known.
pub fn eku_name(oid: &str) -> String {
    match oid {
        "1.3.6.1.5.5.7.3.1" => "Server Authentication".into(),
        "1.3.6.1.5.5.7.3.2" => "Client Authentication".into(),
        "1.3.6.1.5.5.7.3.3" => "Code Signing".into(),
        "1.3.6.1.5.5.7.3.4" => "Email Protection".into(),
        EKU_TIME_STAMPING => "Time Stamping".into(),
        EKU_OCSP_SIGNING => "OCSP Signing".into(),
        "1.3.6.1.4.1.311.10.3.12" => "Document Signing (Microsoft)".into(),
        "1.2.840.113583.1.1.5" => "Authentic Documents (Adobe)".into(),
        "1.3.6.1.5.5.7.3.36" => "Document Signing".into(),
        other => other.to_string(),
    }
}

/// `AB:CD:…` hex.
pub fn colon_hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Certificates in a PEM or DER file (or a bundle of PEM blocks).
pub fn parse_pem_or_der(data: &[u8]) -> Vec<Cert> {
    let text = String::from_utf8_lossy(data);
    if !text.contains("-----BEGIN") {
        return Cert::parse(data).into_iter().collect();
    }
    let mut out = Vec::new();
    let mut b64 = String::new();
    let mut inside = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("-----BEGIN CERTIFICATE") || line.starts_with("-----BEGIN TRUSTED") {
            inside = true;
            b64.clear();
        } else if line.starts_with("-----END") {
            if inside
                && let Some(der) = base64_decode(&b64)
                && let Ok(c) = Cert::parse(&der)
            {
                out.push(c);
            }
            inside = false;
        } else if inside {
            b64.push_str(line);
        }
    }
    out
}

pub fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b' ' | b'\r' | b'\n' | b'\t' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}
