//! PKCS #11 tokens: USB tokens and smart cards (as Indonesian CAs hand
//! out), through the vendor's module (`.so`, `.dylib`, `.dll`) loaded at
//! run time, so Kantor needs no C library to build.
//!
//! Only the calls signing needs are bound: initialise, list tokens, read
//! certificates, log in with the PIN, find the matching private key and
//! sign (CKM_RSA_PKCS with our DigestInfo, CKM_RSA_PKCS_PSS, CKM_ECDSA on
//! our digest). The PIN goes straight to the module and is never kept.

#![allow(
    unsafe_code,
    non_camel_case_types,
    non_snake_case,
    clippy::upper_case_acronyms
)]

use std::ffi::c_void;
use std::os::raw::c_ulong;
use std::sync::{Arc, Mutex};

use crate::{Error, HashAlg, Mech, ecdsa_raw_to_der};

type CK_ULONG = c_ulong;
type CK_RV = CK_ULONG;
type CK_SLOT_ID = CK_ULONG;
type CK_SESSION_HANDLE = CK_ULONG;
type CK_OBJECT_HANDLE = CK_ULONG;

const CKR_OK: CK_RV = 0;
const CKR_PIN_INCORRECT: CK_RV = 0xA0;
const CKR_PIN_LOCKED: CK_RV = 0xA4;
const CKR_USER_ALREADY_LOGGED_IN: CK_RV = 0x100;
const CKR_CRYPTOKI_ALREADY_INITIALIZED: CK_RV = 0x191;

const CKF_SERIAL_SESSION: CK_ULONG = 4;
const CKF_OS_LOCKING_OK: CK_ULONG = 2;
const CKF_LOGIN_REQUIRED: CK_ULONG = 4;
const CKF_PROTECTED_AUTHENTICATION_PATH: CK_ULONG = 0x100;
const CKU_USER: CK_ULONG = 1;

const CKA_CLASS: CK_ULONG = 0;
const CKA_LABEL: CK_ULONG = 3;
const CKA_VALUE: CK_ULONG = 0x11;
const CKA_ID: CK_ULONG = 0x102;
const CKO_CERTIFICATE: CK_ULONG = 1;
const CKO_PRIVATE_KEY: CK_ULONG = 3;

const CKM_RSA_PKCS: CK_ULONG = 1;
const CKM_RSA_PKCS_PSS: CK_ULONG = 0x0D;
const CKM_ECDSA: CK_ULONG = 0x1041;
const CKM_SHA256: CK_ULONG = 0x250;
const CKM_SHA384: CK_ULONG = 0x260;
const CKM_SHA512: CK_ULONG = 0x270;
const CKG_MGF1_SHA256: CK_ULONG = 2;
const CKG_MGF1_SHA384: CK_ULONG = 3;
const CKG_MGF1_SHA512: CK_ULONG = 4;

// Windows packs PKCS #11 structures to 1 byte; elsewhere they're natural.
#[cfg_attr(windows, repr(C, packed))]
#[cfg_attr(not(windows), repr(C))]
struct CK_ATTRIBUTE {
    type_: CK_ULONG,
    pValue: *mut c_void,
    ulValueLen: CK_ULONG,
}

#[cfg_attr(windows, repr(C, packed))]
#[cfg_attr(not(windows), repr(C))]
struct CK_MECHANISM {
    mechanism: CK_ULONG,
    pParameter: *mut c_void,
    ulParameterLen: CK_ULONG,
}

#[cfg_attr(windows, repr(C, packed))]
#[cfg_attr(not(windows), repr(C))]
struct CK_RSA_PKCS_PSS_PARAMS {
    hashAlg: CK_ULONG,
    mgf: CK_ULONG,
    sLen: CK_ULONG,
}

#[cfg_attr(windows, repr(C, packed))]
#[cfg_attr(not(windows), repr(C))]
struct CK_C_INITIALIZE_ARGS {
    CreateMutex: *mut c_void,
    DestroyMutex: *mut c_void,
    LockMutex: *mut c_void,
    UnlockMutex: *mut c_void,
    flags: CK_ULONG,
    pReserved: *mut c_void,
}

#[cfg_attr(windows, repr(C, packed))]
#[cfg_attr(not(windows), repr(C))]
struct CK_TOKEN_INFO {
    label: [u8; 32],
    manufacturerID: [u8; 32],
    model: [u8; 16],
    serialNumber: [u8; 16],
    flags: CK_ULONG,
    rest: [u8; 160],
}

type FnInitialize = unsafe extern "C" fn(*mut c_void) -> CK_RV;
type FnGetSlotList = unsafe extern "C" fn(u8, *mut CK_SLOT_ID, *mut CK_ULONG) -> CK_RV;
type FnGetTokenInfo = unsafe extern "C" fn(CK_SLOT_ID, *mut CK_TOKEN_INFO) -> CK_RV;
type FnOpenSession = unsafe extern "C" fn(
    CK_SLOT_ID,
    CK_ULONG,
    *mut c_void,
    *mut c_void,
    *mut CK_SESSION_HANDLE,
) -> CK_RV;
type FnCloseSession = unsafe extern "C" fn(CK_SESSION_HANDLE) -> CK_RV;
type FnLogin = unsafe extern "C" fn(CK_SESSION_HANDLE, CK_ULONG, *const u8, CK_ULONG) -> CK_RV;
type FnGetAttributeValue =
    unsafe extern "C" fn(CK_SESSION_HANDLE, CK_OBJECT_HANDLE, *mut CK_ATTRIBUTE, CK_ULONG) -> CK_RV;
type FnFindObjectsInit =
    unsafe extern "C" fn(CK_SESSION_HANDLE, *mut CK_ATTRIBUTE, CK_ULONG) -> CK_RV;
type FnFindObjects = unsafe extern "C" fn(
    CK_SESSION_HANDLE,
    *mut CK_OBJECT_HANDLE,
    CK_ULONG,
    *mut CK_ULONG,
) -> CK_RV;
type FnFindObjectsFinal = unsafe extern "C" fn(CK_SESSION_HANDLE) -> CK_RV;
type FnSignInit =
    unsafe extern "C" fn(CK_SESSION_HANDLE, *mut CK_MECHANISM, CK_OBJECT_HANDLE) -> CK_RV;
type FnSign =
    unsafe extern "C" fn(CK_SESSION_HANDLE, *const u8, CK_ULONG, *mut u8, *mut CK_ULONG) -> CK_RV;
type FnGetFunctionList = unsafe extern "C" fn(*mut *const u8) -> CK_RV;

/// Index of each function in CK_FUNCTION_LIST (after the 2-byte version).
mod ix {
    pub const INITIALIZE: usize = 0;
    pub const GET_SLOT_LIST: usize = 4;
    pub const GET_TOKEN_INFO: usize = 6;
    pub const OPEN_SESSION: usize = 12;
    pub const CLOSE_SESSION: usize = 13;
    pub const LOGIN: usize = 18;
    pub const GET_ATTRIBUTE_VALUE: usize = 24;
    pub const FIND_OBJECTS_INIT: usize = 26;
    pub const FIND_OBJECTS: usize = 27;
    pub const FIND_OBJECTS_FINAL: usize = 28;
    pub const SIGN_INIT: usize = 42;
    pub const SIGN: usize = 43;
    pub const COUNT: usize = 44;
}

/// A loaded PKCS #11 module.
pub struct Module {
    path: String,
    _lib: libloading::Library,
    fns: [usize; ix::COUNT],
}

// The module is initialised with CKF_OS_LOCKING_OK; calls are serialised
// per session by the callers here anyway.
unsafe impl Send for Module {}
unsafe impl Sync for Module {}

fn rv(code: CK_RV, what: &str) -> Result<(), Error> {
    match code {
        CKR_OK => Ok(()),
        CKR_PIN_INCORRECT => Err(Error::WrongPin),
        CKR_PIN_LOCKED => Err(Error::Locked),
        c => Err(Error::Failed(format!(
            "the token failed to {what} (error 0x{c:X})"
        ))),
    }
}

/// One certificate on a token.
#[derive(Clone)]
pub struct TokenCert {
    pub token: String,
    pub slot: c_ulong,
    pub label: String,
    pub id: Vec<u8>,
    /// The certificate, DER.
    pub der: Vec<u8>,
    /// The PIN is typed on the reader's own keypad.
    pub pin_pad: bool,
}

/// Whether this build loads PKCS #11 modules. The Mac App Store build
/// doesn't: a module is code from outside the app (App Review guideline
/// 2.5.2, and the hardened runtime refuses libraries not signed by Apple or
/// the app's team). Tokens and smart cards reach it through the keychain
/// instead (CryptoTokenKit, [`crate::system`]).
pub const AVAILABLE: bool = !kantor_core::APP_STORE;

impl Module {
    /// Load a module and initialise it.
    pub fn load(path: &str) -> Result<Arc<Module>, Error> {
        if !AVAILABLE {
            return Err(Error::Failed(
                "this version of Kantor can't load token modules; tokens and smart cards that macOS supports appear among the keychain's certificates".into(),
            ));
        }
        // SAFETY: loading a PKCS #11 module the user chose runs its
        // initialisers; that is what using it means.
        let lib = unsafe { libloading::Library::new(path) }
            .map_err(|e| Error::Failed(format!("can't load the token module {path}: {e}")))?;
        let get: libloading::Symbol<FnGetFunctionList> = unsafe { lib.get(b"C_GetFunctionList\0") }
            .map_err(|_| Error::Failed(format!("{path} is not a PKCS #11 module")))?;
        let mut list: *const u8 = std::ptr::null();
        rv(unsafe { get(&mut list) }, "start")?;
        if list.is_null() {
            return Err(Error::Failed(format!("{path} gave no function list")));
        }
        let ptr = std::mem::size_of::<*const c_void>();
        let start = if cfg!(windows) { 2 } else { ptr };
        let mut fns = [0usize; ix::COUNT];
        for (i, f) in fns.iter_mut().enumerate() {
            // SAFETY: CK_FUNCTION_LIST holds at least 68 pointers after
            // its version; read_unaligned covers Windows' 1-byte packing.
            *f = unsafe { std::ptr::read_unaligned(list.add(start + i * ptr) as *const usize) };
        }
        let m = Module {
            path: path.to_string(),
            _lib: lib,
            fns,
        };
        let mut args = CK_C_INITIALIZE_ARGS {
            CreateMutex: std::ptr::null_mut(),
            DestroyMutex: std::ptr::null_mut(),
            LockMutex: std::ptr::null_mut(),
            UnlockMutex: std::ptr::null_mut(),
            flags: CKF_OS_LOCKING_OK,
            pReserved: std::ptr::null_mut(),
        };
        let init: FnInitialize = m.f(ix::INITIALIZE)?;
        let code = unsafe { init(&mut args as *mut _ as *mut c_void) };
        if code != CKR_CRYPTOKI_ALREADY_INITIALIZED {
            rv(code, "start")?;
        }
        Ok(Arc::new(m))
    }

    fn f<T: Copy>(&self, i: usize) -> Result<T, Error> {
        let p = self.fns[i];
        if p == 0 {
            return Err(Error::Failed(format!(
                "the token module {} lacks a function Kantor needs",
                self.path
            )));
        }
        // SAFETY: T is the extern "C" fn type of entry i.
        Ok(unsafe { std::mem::transmute_copy::<usize, T>(&p) })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    fn slots(&self) -> Result<Vec<CK_SLOT_ID>, Error> {
        let f: FnGetSlotList = self.f(ix::GET_SLOT_LIST)?;
        let mut n: CK_ULONG = 0;
        rv(unsafe { f(1, std::ptr::null_mut(), &mut n) }, "list tokens")?;
        let mut v = vec![0 as CK_SLOT_ID; n as usize];
        rv(unsafe { f(1, v.as_mut_ptr(), &mut n) }, "list tokens")?;
        v.truncate(n as usize);
        Ok(v)
    }

    fn token_info(&self, slot: CK_SLOT_ID) -> Result<(String, CK_ULONG), Error> {
        let f: FnGetTokenInfo = self.f(ix::GET_TOKEN_INFO)?;
        // Generous buffer: the struct's tail differs little across versions.
        let mut info: CK_TOKEN_INFO = unsafe { std::mem::zeroed() };
        rv(unsafe { f(slot, &mut info) }, "read the token")?;
        let label = String::from_utf8_lossy(&info.label).trim().to_string();
        let flags = info.flags;
        Ok((label, flags))
    }

    fn open(self: &Arc<Self>, slot: CK_SLOT_ID) -> Result<Session, Error> {
        let f: FnOpenSession = self.f(ix::OPEN_SESSION)?;
        let mut h: CK_SESSION_HANDLE = 0;
        rv(
            unsafe {
                f(
                    slot,
                    CKF_SERIAL_SESSION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    &mut h,
                )
            },
            "open a session",
        )?;
        Ok(Session { m: self.clone(), h })
    }

    /// Every certificate on every token present.
    pub fn certificates(self: &Arc<Self>) -> Result<Vec<TokenCert>, Error> {
        let mut out = Vec::new();
        for slot in self.slots()? {
            let Ok((token, flags)) = self.token_info(slot) else {
                continue;
            };
            let Ok(s) = self.open(slot) else {
                continue;
            };
            for h in s.find(CKO_CERTIFICATE, None)? {
                let Some(der) = s.attr(h, CKA_VALUE) else {
                    continue;
                };
                let id = s.attr(h, CKA_ID).unwrap_or_default();
                let label = s
                    .attr(h, CKA_LABEL)
                    .map(|l| String::from_utf8_lossy(&l).into_owned())
                    .unwrap_or_default();
                out.push(TokenCert {
                    token: token.clone(),
                    slot,
                    label,
                    id,
                    der,
                    pin_pad: flags & CKF_PROTECTED_AUTHENTICATION_PATH != 0,
                });
            }
            let _ = flags & CKF_LOGIN_REQUIRED;
        }
        Ok(out)
    }

    /// Log in to `tc`'s token with `pin` (`None` on a PIN pad) and return
    /// a key that signs with the private key matching the certificate.
    pub fn key(self: &Arc<Self>, tc: &TokenCert, pin: Option<&str>) -> Result<TokenKey, Error> {
        let s = self.open(tc.slot)?;
        let login: FnLogin = self.f(ix::LOGIN)?;
        let (p, n) = match pin {
            Some(p) => (p.as_ptr(), p.len() as CK_ULONG),
            None => (std::ptr::null(), 0),
        };
        let code = unsafe { login(s.h, CKU_USER, p, n) };
        if code != CKR_USER_ALREADY_LOGGED_IN {
            rv(code, "log in")?;
        }
        let keys = s.find(CKO_PRIVATE_KEY, (!tc.id.is_empty()).then_some(&tc.id[..]))?;
        let key = *keys.first().ok_or_else(|| {
            Error::Failed("the token has no private key for this certificate".into())
        })?;
        Ok(TokenKey {
            session: Mutex::new(s),
            key,
        })
    }
}

struct Session {
    m: Arc<Module>,
    h: CK_SESSION_HANDLE,
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(f) = self.m.f::<FnCloseSession>(ix::CLOSE_SESSION) {
            unsafe { f(self.h) };
        }
    }
}

impl Session {
    fn find(&self, class: CK_ULONG, id: Option<&[u8]>) -> Result<Vec<CK_OBJECT_HANDLE>, Error> {
        let init: FnFindObjectsInit = self.m.f(ix::FIND_OBJECTS_INIT)?;
        let next: FnFindObjects = self.m.f(ix::FIND_OBJECTS)?;
        let fin: FnFindObjectsFinal = self.m.f(ix::FIND_OBJECTS_FINAL)?;
        let mut class_v = class;
        let mut tmpl = vec![CK_ATTRIBUTE {
            type_: CKA_CLASS,
            pValue: &mut class_v as *mut CK_ULONG as *mut c_void,
            ulValueLen: std::mem::size_of::<CK_ULONG>() as CK_ULONG,
        }];
        let mut idv = id.map(<[u8]>::to_vec).unwrap_or_default();
        if id.is_some() {
            tmpl.push(CK_ATTRIBUTE {
                type_: CKA_ID,
                pValue: idv.as_mut_ptr() as *mut c_void,
                ulValueLen: idv.len() as CK_ULONG,
            });
        }
        rv(
            unsafe { init(self.h, tmpl.as_mut_ptr(), tmpl.len() as CK_ULONG) },
            "search",
        )?;
        let mut out = Vec::new();
        loop {
            let mut buf = [0 as CK_OBJECT_HANDLE; 32];
            let mut n: CK_ULONG = 0;
            let code = unsafe { next(self.h, buf.as_mut_ptr(), 32, &mut n) };
            if code != CKR_OK || n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n as usize]);
            if out.len() > 4096 {
                break;
            }
        }
        unsafe { fin(self.h) };
        Ok(out)
    }

    fn attr(&self, obj: CK_OBJECT_HANDLE, t: CK_ULONG) -> Option<Vec<u8>> {
        let f: FnGetAttributeValue = self.m.f(ix::GET_ATTRIBUTE_VALUE).ok()?;
        let mut a = CK_ATTRIBUTE {
            type_: t,
            pValue: std::ptr::null_mut(),
            ulValueLen: 0,
        };
        if unsafe { f(self.h, obj, &mut a, 1) } != CKR_OK {
            return None;
        }
        let len = a.ulValueLen;
        if len as i64 <= 0 || len > 1 << 20 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        a.pValue = buf.as_mut_ptr() as *mut c_void;
        if unsafe { f(self.h, obj, &mut a, 1) } != CKR_OK {
            return None;
        }
        let len = a.ulValueLen;
        buf.truncate(len as usize);
        Some(buf)
    }
}

/// A private key on a token, logged in.
pub struct TokenKey {
    session: Mutex<Session>,
    key: CK_OBJECT_HANDLE,
}

impl TokenKey {
    /// Sign `digest` (made with `mech`'s hash).
    pub fn sign(&self, mech: Mech, digest: &[u8]) -> Result<Vec<u8>, Error> {
        let s = self
            .session
            .lock()
            .map_err(|_| Error::Failed("the token session failed".into()))?;
        let hash = mech.hash();
        let (mech_hash, mgf) = match hash {
            HashAlg::Sha256 => (CKM_SHA256, CKG_MGF1_SHA256),
            HashAlg::Sha384 => (CKM_SHA384, CKG_MGF1_SHA384),
            HashAlg::Sha512 => (CKM_SHA512, CKG_MGF1_SHA512),
        };
        let mut pss_params = CK_RSA_PKCS_PSS_PARAMS {
            hashAlg: mech_hash,
            mgf,
            sLen: hash.size() as CK_ULONG,
        };
        let plain = |m| CK_MECHANISM {
            mechanism: m,
            pParameter: std::ptr::null_mut(),
            ulParameterLen: 0,
        };
        let (mut m, input) = match mech {
            Mech::RsaPss(_) => (
                CK_MECHANISM {
                    mechanism: CKM_RSA_PKCS_PSS,
                    pParameter: &mut pss_params as *mut _ as *mut c_void,
                    ulParameterLen: std::mem::size_of::<CK_RSA_PKCS_PSS_PARAMS>() as CK_ULONG,
                },
                digest.to_vec(),
            ),
            Mech::RsaPkcs1(_) => (
                plain(CKM_RSA_PKCS),
                [hash.digest_info_prefix(), digest].concat(),
            ),
            Mech::Ecdsa(_) => (plain(CKM_ECDSA), digest.to_vec()),
        };
        let init: FnSignInit = s.m.f(ix::SIGN_INIT)?;
        let sign: FnSign = s.m.f(ix::SIGN)?;
        // SAFETY: the mechanism and buffers outlive the calls.
        rv(unsafe { init(s.h, &mut m, self.key) }, "start signing")?;
        let mut out = vec![0u8; 1024];
        let mut n = out.len() as CK_ULONG;
        rv(
            unsafe {
                sign(
                    s.h,
                    input.as_ptr(),
                    input.len() as CK_ULONG,
                    out.as_mut_ptr(),
                    &mut n,
                )
            },
            "sign",
        )?;
        out.truncate(n as usize);
        Ok(match mech {
            Mech::Ecdsa(_) => ecdsa_raw_to_der(&out),
            _ => out,
        })
    }
}

/// Where PKCS #11 modules usually are, for the module picker.
pub fn common_modules() -> Vec<String> {
    if !AVAILABLE {
        return Vec::new();
    }
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/Library/OpenSC/lib/opensc-pkcs11.so",
            "/usr/local/lib/opensc-pkcs11.so",
            "/opt/homebrew/lib/opensc-pkcs11.so",
            "/usr/local/lib/libeTPkcs11.dylib",
            "/usr/local/lib/libsofthsm2.so",
            "/opt/homebrew/lib/softhsm/libsofthsm2.so",
        ]
    } else if cfg!(windows) {
        &[
            "C:\\Windows\\System32\\opensc-pkcs11.dll",
            "C:\\Windows\\System32\\eTPKCS11.dll",
            "C:\\Program Files\\OpenSC Project\\OpenSC\\pkcs11\\opensc-pkcs11.dll",
            "C:\\SoftHSM2\\lib\\softhsm2-x64.dll",
        ]
    } else {
        &[
            "/usr/lib/opensc-pkcs11.so",
            "/usr/lib/x86_64-linux-gnu/opensc-pkcs11.so",
            "/usr/lib/pkcs11/opensc-pkcs11.so",
            "/usr/lib/libeTPkcs11.so",
            "/usr/lib/softhsm/libsofthsm2.so",
            "/usr/lib/x86_64-linux-gnu/softhsm/libsofthsm2.so",
            "/usr/lib/pkcs11/libsofthsm2.so",
        ]
    };
    candidates
        .iter()
        .filter(|p| std::path::Path::new(p).exists())
        .map(|p| p.to_string())
        .collect()
}
