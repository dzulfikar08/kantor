//! The operating system's own certificate store: identities (certificate +
//! private key) in the macOS keychain, or the Windows "Personal" store
//! (CryptoAPI + CNG, which also reaches smart cards with a Windows
//! minidriver). Linux has no system-wide store for personal keys: use a
//! PKCS #12 file or a PKCS #11 module there.

use crate::{Error, Mech};

/// A certificate with a private key in the system store.
pub struct SystemCert {
    /// The certificate, DER.
    pub der: Vec<u8>,
    handle: imp::Handle,
}

impl SystemCert {
    /// Sign `digest` (the system may ask for a PIN or permission).
    pub fn sign(&self, mech: Mech, digest: &[u8]) -> Result<Vec<u8>, Error> {
        imp::sign(&self.handle, mech, digest)
    }
}

/// Whether this platform has a system store Kantor can sign with.
pub const AVAILABLE: bool = cfg!(any(target_os = "macos", windows));

/// Certificates with private keys in the system store.
pub fn certificates() -> Result<Vec<SystemCert>, Error> {
    imp::list()
}

#[cfg(target_os = "macos")]
mod imp {
    use security_framework::identity::SecIdentity;
    use security_framework::item::{ItemClass, ItemSearchOptions, Limit, Reference, SearchResult};
    use security_framework::key::Algorithm;

    use super::SystemCert;
    use crate::{Error, HashAlg, Mech};

    pub struct Handle(SecIdentity);

    // SecIdentity is a CoreFoundation reference, safe to move between threads.
    pub fn list() -> Result<Vec<SystemCert>, Error> {
        let found = ItemSearchOptions::new()
            .class(ItemClass::identity())
            .load_refs(true)
            .limit(Limit::All)
            .search()
            .unwrap_or_default();
        let mut out = Vec::new();
        for r in found {
            if let SearchResult::Ref(Reference::Identity(id)) = r
                && let Ok(cert) = id.certificate()
            {
                out.push(SystemCert {
                    der: cert.to_der(),
                    handle: Handle(id),
                });
            }
        }
        Ok(out)
    }

    pub fn sign(h: &Handle, mech: Mech, digest: &[u8]) -> Result<Vec<u8>, Error> {
        let key =
            h.0.private_key()
                .map_err(|e| Error::Failed(format!("the keychain refused the key: {e}")))?;
        let alg = match mech {
            Mech::RsaPkcs1(HashAlg::Sha256) => Algorithm::RSASignatureDigestPKCS1v15SHA256,
            Mech::RsaPkcs1(HashAlg::Sha384) => Algorithm::RSASignatureDigestPKCS1v15SHA384,
            Mech::RsaPkcs1(HashAlg::Sha512) => Algorithm::RSASignatureDigestPKCS1v15SHA512,
            Mech::RsaPss(HashAlg::Sha256) => Algorithm::RSASignatureDigestPSSSHA256,
            Mech::RsaPss(HashAlg::Sha384) => Algorithm::RSASignatureDigestPSSSHA384,
            Mech::RsaPss(HashAlg::Sha512) => Algorithm::RSASignatureDigestPSSSHA512,
            Mech::Ecdsa(HashAlg::Sha256) => Algorithm::ECDSASignatureDigestX962SHA256,
            Mech::Ecdsa(HashAlg::Sha384) => Algorithm::ECDSASignatureDigestX962SHA384,
            Mech::Ecdsa(HashAlg::Sha512) => Algorithm::ECDSASignatureDigestX962SHA512,
        };
        // X9.62 ECDSA signatures are already DER.
        key.create_signature(alg, digest).map_err(|e| {
            if e.code() == -128 {
                Error::Cancelled
            } else {
                Error::Failed(format!("the keychain couldn't sign: {e}"))
            }
        })
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use std::ffi::c_void;

    use windows_sys::Win32::Security::Cryptography::{
        BCRYPT_PAD_PKCS1, BCRYPT_PAD_PSS, BCRYPT_PKCS1_PADDING_INFO, BCRYPT_PSS_PADDING_INFO,
        CERT_CONTEXT, CERT_KEY_PROV_INFO_PROP_ID, CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG,
        CertCloseStore, CertDuplicateCertificateContext, CertEnumCertificatesInStore,
        CertFreeCertificateContext, CertGetCertificateContextProperty, CertOpenSystemStoreW,
        CryptAcquireCertificatePrivateKey, HCERTSTORE, NCRYPT_KEY_HANDLE, NCryptFreeObject,
        NCryptSignHash,
    };

    use super::SystemCert;
    use crate::{Error, HashAlg, Mech, ecdsa_raw_to_der};

    pub struct Handle(*const CERT_CONTEXT);

    // A duplicated CERT_CONTEXT is reference counted and thread-safe.
    unsafe impl Send for Handle {}
    unsafe impl Sync for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: we own one reference (CertDuplicateCertificateContext).
            unsafe { CertFreeCertificateContext(self.0) };
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn list() -> Result<Vec<SystemCert>, Error> {
        let name = wide("MY");
        // SAFETY: plain CryptoAPI calls; every context we keep is duplicated.
        unsafe {
            let store: HCERTSTORE = CertOpenSystemStoreW(0, name.as_ptr());
            if store.is_null() {
                return Err(Error::Failed(
                    "can't open the Windows certificate store".into(),
                ));
            }
            let mut out = Vec::new();
            let mut ctx: *const CERT_CONTEXT = std::ptr::null();
            loop {
                ctx = CertEnumCertificatesInStore(store, ctx);
                if ctx.is_null() {
                    break;
                }
                let mut size = 0u32;
                let has_key = CertGetCertificateContextProperty(
                    ctx,
                    CERT_KEY_PROV_INFO_PROP_ID,
                    std::ptr::null_mut(),
                    &mut size,
                ) != 0;
                if !has_key {
                    continue;
                }
                let c = &*ctx;
                let der =
                    std::slice::from_raw_parts(c.pbCertEncoded, c.cbCertEncoded as usize).to_vec();
                out.push(SystemCert {
                    der,
                    handle: Handle(CertDuplicateCertificateContext(ctx)),
                });
            }
            CertCloseStore(store, 0);
            Ok(out)
        }
    }

    fn alg_name(h: HashAlg) -> Vec<u16> {
        wide(match h {
            HashAlg::Sha256 => "SHA256",
            HashAlg::Sha384 => "SHA384",
            HashAlg::Sha512 => "SHA512",
        })
    }

    pub fn sign(h: &Handle, mech: Mech, digest: &[u8]) -> Result<Vec<u8>, Error> {
        // SAFETY: CryptoAPI/CNG calls with buffers that outlive them.
        unsafe {
            let mut key: NCRYPT_KEY_HANDLE = 0;
            let mut spec = 0u32;
            let mut free = 0i32;
            if CryptAcquireCertificatePrivateKey(
                h.0,
                CRYPT_ACQUIRE_ONLY_NCRYPT_KEY_FLAG,
                std::ptr::null(),
                &mut key,
                &mut spec,
                &mut free,
            ) == 0
            {
                return Err(Error::Failed(
                    "Windows couldn't open this certificate's private key".into(),
                ));
            }
            let name = alg_name(mech.hash());
            let pkcs1 = BCRYPT_PKCS1_PADDING_INFO {
                pszAlgId: name.as_ptr(),
            };
            let pss = BCRYPT_PSS_PADDING_INFO {
                pszAlgId: name.as_ptr(),
                cbSalt: mech.hash().len() as u32,
            };
            let (info, flags): (*const c_void, u32) = match mech {
                Mech::RsaPkcs1(_) => (&pkcs1 as *const _ as *const c_void, BCRYPT_PAD_PKCS1),
                Mech::RsaPss(_) => (&pss as *const _ as *const c_void, BCRYPT_PAD_PSS),
                Mech::Ecdsa(_) => (std::ptr::null(), 0),
            };
            let mut n = 0u32;
            let mut status = NCryptSignHash(
                key,
                info,
                digest.as_ptr(),
                digest.len() as u32,
                std::ptr::null_mut(),
                0,
                &mut n,
                flags,
            );
            let mut out = vec![0u8; n as usize];
            if status == 0 {
                status = NCryptSignHash(
                    key,
                    info,
                    digest.as_ptr(),
                    digest.len() as u32,
                    out.as_mut_ptr(),
                    n,
                    &mut n,
                    flags,
                );
            }
            if free != 0 {
                NCryptFreeObject(key);
            }
            // NTE_USER_CANCELLED / SCARD_W_CANCELLED_BY_USER
            if status as u32 == 0x8009_0036 || status as u32 == 0x8010_006E {
                return Err(Error::Cancelled);
            }
            if status as u32 == 0x8010_006B {
                return Err(Error::WrongPin);
            }
            if status != 0 {
                return Err(Error::Failed(format!(
                    "Windows couldn't sign (error 0x{:08X})",
                    status as u32
                )));
            }
            out.truncate(n as usize);
            Ok(match mech {
                Mech::Ecdsa(_) => ecdsa_raw_to_der(&out),
                _ => out,
            })
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::SystemCert;
    use crate::{Error, Mech};

    pub enum Handle {}

    pub fn list() -> Result<Vec<SystemCert>, Error> {
        Ok(Vec::new())
    }

    pub fn sign(h: &Handle, _: Mech, _: &[u8]) -> Result<Vec<u8>, Error> {
        match *h {}
    }
}
