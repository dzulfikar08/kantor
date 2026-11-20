//! The providers Kantor knows. Only those whose API documentation is
//! public enough to implement faithfully are [`Support::Implemented`]; the
//! rest are listed as [`Support::Stub`] with the reason (see
//! `docs/decisions/0004-esign-providers.md`).

pub mod mekari;
pub mod vida;
pub mod xignature;

use std::sync::Arc;

use crate::emeterai::EMeteraiProvider;
use crate::signer::RemoteSigner;
use crate::{Account, Error, Field, Info, Kind, Support};

const NO_FIELDS: &[Field] = &[];

macro_rules! stub {
    ($name:ident, $id:literal, $title:literal, [$($kind:ident),+], $docs:literal, $why:literal) => {
        pub static $name: Info = Info {
            id: $id,
            name: $title,
            kinds: &[$(Kind::$kind),+],
            support: Support::Stub($why),
            docs: $docs,
            production_url: "",
            sandbox_url: "",
            fields: NO_FIELDS,
            rewrites_file: false,
            hash_signing: false,
        };
    };
}

stub!(
    PERURI,
    "peruri",
    "Peruri e-Meterai",
    [EMeterai],
    "https://e-meterai.co.id",
    "Peruri's e-Meterai API isn't public: it needs a contract with Peruri or a distributor."
);
stub!(
    PAJAKKU,
    "pajakku",
    "Pajakku",
    [EMeterai],
    "https://pajakku.com",
    "Pajakku shares its e-Meterai API documentation and sandbox only after a non-disclosure agreement."
);
stub!(
    MITRACOMM,
    "mitracomm",
    "Mitracomm",
    [EMeterai],
    "https://mitracomm.com",
    "Mitracomm's e-Meterai API documentation isn't public."
);
stub!(
    FINNET,
    "finnet",
    "Finnet",
    [EMeterai],
    "https://www.finnet.co.id",
    "Finnet's e-Meterai API documentation isn't public."
);
stub!(
    PRIVY,
    "privy",
    "Privy",
    [EMeterai, Signing],
    "https://docs.privy.id",
    "Privy's public documentation covers identity checks only; its signing and e-Meterai APIs aren't published."
);
stub!(
    TILAKA,
    "tilaka",
    "Tilaka",
    [Signing],
    "https://tilaka.id",
    "Tilaka's signing API, including hash signing, is documented only for partners."
);
stub!(
    PERURI_SIGN,
    "peruri-sign",
    "Peruri Sign",
    [Signing],
    "https://peruri.co.id",
    "Peruri's PSrE signing API documentation isn't public."
);
stub!(
    VIDA_SIGN,
    "vida-sign",
    "VIDA Sign",
    [Signing],
    "https://docs.vida.id/identity-stack/sign/digital-signature",
    "VIDA's signing APIs are public but need the signer's e-KYC data (photo of the ID card and a selfie) with every request, and its hash signing isn't documented yet."
);
stub!(
    DIGISIGN,
    "digisign",
    "Digisign",
    [Signing],
    "https://digisign.id",
    "Digisign's API documentation isn't public."
);

/// Every provider, implemented ones first.
pub static PROVIDERS: &[&Info] = &[
    &vida::INFO,
    &mekari::INFO,
    &xignature::INFO,
    &PERURI,
    &PAJAKKU,
    &MITRACOMM,
    &FINNET,
    &PRIVY,
    &TILAKA,
    &PERURI_SIGN,
    &VIDA_SIGN,
    &DIGISIGN,
];

/// The provider with id `id`.
pub fn find(id: &str) -> Option<&'static Info> {
    PROVIDERS.iter().copied().find(|p| p.id == id)
}

fn usable(account: &Account, kind: Kind) -> Result<(), Error> {
    if !account.info.offers(kind) {
        return Err(Error::NotImplemented(match kind {
            Kind::EMeterai => format!("{} doesn't stamp e-meterai", account.info.name),
            Kind::Signing => format!("{} doesn't make certified signatures", account.info.name),
        }));
    }
    if let Support::Stub(why) = account.info.support {
        return Err(Error::NotImplemented(format!(
            "{}: {why}",
            account.info.name
        )));
    }
    Ok(())
}

/// The e-meterai client for `account`.
pub fn emeterai(account: Account) -> Result<Arc<dyn EMeteraiProvider>, Error> {
    usable(&account, Kind::EMeterai)?;
    match account.info.id {
        "vida" => Ok(Arc::new(vida::Vida::new(account))),
        "mekari" => Ok(Arc::new(mekari::Mekari::new(account))),
        _ => Err(Error::NotImplemented(account.info.name.to_string())),
    }
}

/// The signing client for `account`.
pub fn signer(account: Account) -> Result<Arc<dyn RemoteSigner>, Error> {
    usable(&account, Kind::Signing)?;
    match account.info.id {
        "xignature" => Ok(Arc::new(xignature::Xignature::new(account))),
        _ => Err(Error::NotImplemented(account.info.name.to_string())),
    }
}
