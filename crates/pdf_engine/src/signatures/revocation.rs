//! Revocation: OCSP responses and CRLs, from the file (DSS, Adobe's
//! archival attribute) or fetched when the user allows it.

use super::crypto::{self, AlgId};
use super::der::{self, Tlv};
use super::x509::{self, Cert};

/// What a revocation source says about one certificate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Said {
    Good,
    Revoked { at: i64 },
}

/// A parsed OCSP response (BasicOCSPResponse).
#[derive(Clone, Debug)]
pub struct Ocsp {
    tbs: Vec<u8>,
    sig_alg: AlgId,
    signature: Vec<u8>,
    pub certs: Vec<Cert>,
    responder: Responder,
    pub produced_at: i64,
    singles: Vec<Single>,
}

#[derive(Clone, Debug)]
enum Responder {
    Name(Vec<u8>),
    KeyHash(Vec<u8>),
}

#[derive(Clone, Debug)]
struct Single {
    hash_alg: String,
    name_hash: Vec<u8>,
    key_hash: Vec<u8>,
    serial: Vec<u8>,
    /// None = unknown.
    status: Option<Said>,
    this_update: i64,
}

impl Ocsp {
    /// An OCSPResponse (or a bare BasicOCSPResponse, as some DSS hold).
    pub fn parse(data: &[u8]) -> der::Result<Ocsp> {
        let top = Tlv::parse(data)?.expect(der::SEQUENCE)?;
        let mut r = top.reader();
        let first = r.read()?;
        let basic = if first.tag == der::ENUMERATED {
            if first.small_int()? != 0 {
                return Err(der::Error("OCSP response is not successful"));
            }
            let rb = r.read_tag(der::ctx(0))?.reader().read_tag(der::SEQUENCE)?;
            let mut rb = rb.reader();
            let _type = rb.read_tag(der::OID)?;
            Tlv::parse(rb.read_tag(der::OCTET_STRING)?.body)?
        } else {
            top
        };
        let mut b = basic.reader();
        let tbs_t = b.read_tag(der::SEQUENCE)?;
        let sig_alg = AlgId::parse(b.read_tag(der::SEQUENCE)?)?;
        let signature = b.read_tag(der::BIT_STRING)?.bits()?.to_vec();
        let mut certs = Vec::new();
        if let Some(c) = b.read_opt(der::ctx(0))? {
            for cert in c.reader().read_tag(der::SEQUENCE)?.reader().all()? {
                if let Ok(c) = Cert::parse(cert.raw) {
                    certs.push(c);
                }
            }
        }
        let mut t = tbs_t.reader();
        t.read_opt(der::ctx(0))?;
        let rid = t.read()?;
        let responder = if rid.tag == der::ctx(1) {
            Responder::Name(rid.reader().read()?.raw.to_vec())
        } else if rid.tag == der::ctx(2) {
            Responder::KeyHash(rid.reader().read()?.body.to_vec())
        } else {
            return Err(der::Error("bad OCSP responder id"));
        };
        let produced_at = t.read_tag(der::GENERALIZED_TIME)?.time()?;
        let mut singles = Vec::new();
        for s in t.read_tag(der::SEQUENCE)?.reader().all()? {
            let mut s = s.reader();
            let mut id = s.read_tag(der::SEQUENCE)?.reader();
            let hash_alg = AlgId::parse(id.read_tag(der::SEQUENCE)?)?.oid;
            let name_hash = id.read_tag(der::OCTET_STRING)?.body.to_vec();
            let key_hash = id.read_tag(der::OCTET_STRING)?.body.to_vec();
            let serial = id.read_tag(der::INTEGER)?.uint_bytes().to_vec();
            let st = s.read()?;
            let status = match st.tag {
                0x80 => Some(Said::Good),
                0xa1 | 0x81 => {
                    let at = st.reader().read()?.time()?;
                    Some(Said::Revoked { at })
                }
                _ => None,
            };
            let this_update = s.read_tag(der::GENERALIZED_TIME)?.time()?;
            singles.push(Single {
                hash_alg,
                name_hash,
                key_hash,
                serial,
                status,
                this_update,
            });
        }
        Ok(Ocsp {
            tbs: tbs_t.raw.to_vec(),
            sig_alg,
            signature,
            certs,
            responder,
            produced_at,
            singles,
        })
    }

    /// What this response says about `cert` (issued by `issuer`), if it
    /// covers it and is signed by the issuer or a responder it authorised.
    /// Returns the answer and the time it was given.
    pub fn status_of(&self, cert: &Cert, issuer: &Cert) -> Option<(Option<Said>, i64)> {
        let single = self.singles.iter().find(|s| {
            s.serial == cert.serial
                && crypto::digest(&s.hash_alg, &issuer.subject).as_deref() == Some(&s.name_hash[..])
                && crypto::digest(&s.hash_alg, &issuer.key.key).as_deref() == Some(&s.key_hash[..])
        })?;
        if !self.signed_by_authority(issuer) {
            return None;
        }
        Some((single.status.clone(), single.this_update))
    }

    fn signed_by_authority(&self, issuer: &Cert) -> bool {
        let check = |c: &Cert| {
            crypto::verify(&c.key, &self.sig_alg, None, &self.tbs, &self.signature).is_ok()
        };
        if self.responder_is(issuer) && check(issuer) {
            return true;
        }
        // A delegated responder: issued by the issuer, for OCSP signing.
        self.certs.iter().any(|c| {
            self.responder_is(c)
                && c.ext.eku.iter().any(|e| e == x509::EKU_OCSP_SIGNING)
                && x509::names_equal(&c.issuer, &issuer.subject)
                && c.signed_by(issuer)
                && check(c)
        })
    }

    fn responder_is(&self, c: &Cert) -> bool {
        match &self.responder {
            Responder::Name(n) => x509::names_equal(n, &c.subject),
            Responder::KeyHash(h) => {
                crypto::digest(crypto::SHA1, &c.key.key).as_deref() == Some(&h[..])
            }
        }
    }
}

/// A parsed CRL.
#[derive(Clone, Debug)]
pub struct Crl {
    tbs: Vec<u8>,
    sig_alg: AlgId,
    signature: Vec<u8>,
    pub issuer: Vec<u8>,
    pub this_update: i64,
    revoked: Vec<(Vec<u8>, i64)>,
}

impl Crl {
    pub fn parse(data: &[u8]) -> der::Result<Crl> {
        let mut r = Tlv::parse(data)?.expect(der::SEQUENCE)?.reader();
        let tbs_t = r.read_tag(der::SEQUENCE)?;
        let sig_alg = AlgId::parse(r.read_tag(der::SEQUENCE)?)?;
        let signature = r.read_tag(der::BIT_STRING)?.bits()?.to_vec();
        let mut t = tbs_t.reader();
        t.read_opt(der::INTEGER)?;
        let _alg = t.read_tag(der::SEQUENCE)?;
        let issuer = t.read_tag(der::SEQUENCE)?.raw.to_vec();
        let this_update = t.read()?.time()?;
        if matches!(t.peek_tag(), Some(der::UTC_TIME | der::GENERALIZED_TIME)) {
            t.read()?;
        }
        let mut revoked = Vec::new();
        if let Some(list) = t.read_opt(der::SEQUENCE)? {
            for e in list.reader().all()? {
                let mut e = e.reader();
                let serial = e.read_tag(der::INTEGER)?.uint_bytes().to_vec();
                let at = e.read()?.time()?;
                revoked.push((serial, at));
            }
        }
        Ok(Crl {
            tbs: tbs_t.raw.to_vec(),
            sig_alg,
            signature,
            issuer,
            this_update,
            revoked,
        })
    }

    /// What this CRL says about `cert`, if `issuer` issued and signed it.
    pub fn status_of(&self, cert: &Cert, issuer: &Cert) -> Option<(Said, i64)> {
        if !x509::names_equal(&self.issuer, &cert.issuer)
            || !x509::names_equal(&self.issuer, &issuer.subject)
        {
            return None;
        }
        if issuer
            .ext
            .key_usage
            .is_some_and(|ku| ku & x509::KU_CRL_SIGN == 0)
        {
            return None;
        }
        crypto::verify(&issuer.key, &self.sig_alg, None, &self.tbs, &self.signature).ok()?;
        let said = match self.revoked.iter().find(|(s, _)| s == &cert.serial) {
            Some((_, at)) => Said::Revoked { at: *at },
            None => Said::Good,
        };
        Some((said, self.this_update))
    }
}

/// A DER OCSPRequest for `cert` (SHA-1 CertID, as responders expect).
pub fn ocsp_request(cert: &Cert, issuer: &Cert) -> Vec<u8> {
    let sha1 = |d: &[u8]| crypto::digest(crypto::SHA1, d).unwrap_or_default();
    let alg = der::encode(
        der::SEQUENCE,
        &[der::encode_oid(crypto::SHA1), der::encode(der::NULL, &[])].concat(),
    );
    let mut serial = cert.serial.clone();
    if serial.first().is_some_and(|b| b & 0x80 != 0) {
        serial.insert(0, 0);
    }
    let cert_id = der::encode(
        der::SEQUENCE,
        &[
            alg,
            der::encode(der::OCTET_STRING, &sha1(&issuer.subject)),
            der::encode(der::OCTET_STRING, &sha1(&issuer.key.key)),
            der::encode(der::INTEGER, &serial),
        ]
        .concat(),
    );
    let request = der::encode(der::SEQUENCE, &cert_id);
    let list = der::encode(der::SEQUENCE, &request);
    let tbs = der::encode(der::SEQUENCE, &list);
    der::encode(der::SEQUENCE, &tbs)
}
