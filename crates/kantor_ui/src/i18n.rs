//! Interface language: English or Indonesian.
//!
//! Strings are written in place as pairs, `tr("Open from Cloud…", "Buka
//! dari Cloud…")`, so the two languages can't drift apart and nothing needs
//! a lookup table. The language is chosen once per run: `KANTOR_LANG`, then
//! `language` in settings.toml, then the system locale (`LC_ALL`,
//! `LC_MESSAGES`, `LANG`; `id…` is Indonesian, anything else English).

use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    En,
    Id,
}

impl Lang {
    /// From a setting or locale string (`id`, `id_ID.UTF-8`, `en_US`, `ind`).
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        if s.is_empty() || s == "c" || s == "posix" {
            return None;
        }
        Some(
            if s.starts_with("id") || s.starts_with("ind") || s.starts_with("in_") {
                Self::Id
            } else {
                Self::En
            },
        )
    }

    /// The language from the environment and settings (see the module docs).
    pub fn detect() -> Self {
        let env = |k: &str| std::env::var(k).ok().and_then(|v| Self::parse(&v));
        env("KANTOR_LANG")
            .or_else(|| {
                kantor_core::settings::Settings::load()
                    .language
                    .as_deref()
                    .and_then(Self::parse)
            })
            .or_else(|| env("LC_ALL"))
            .or_else(|| env("LC_MESSAGES"))
            .or_else(|| env("LANG"))
            .unwrap_or(Self::En)
    }
}

/// The interface language for this run.
pub fn lang() -> Lang {
    static LANG: OnceLock<Lang> = OnceLock::new();
    *LANG.get_or_init(Lang::detect)
}

/// The string for the interface language.
pub fn tr(en: &'static str, id: &'static str) -> &'static str {
    pick(lang(), en, id)
}

/// The string for `lang` (testable without the global).
pub fn pick<T>(lang: Lang, en: T, id: T) -> T {
    match lang {
        Lang::En => en,
        Lang::Id => id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn parses_locales() {
        assert_eq!(Lang::parse("id_ID.UTF-8"), Some(Lang::Id));
        assert_eq!(Lang::parse("id"), Some(Lang::Id));
        assert_eq!(Lang::parse("en_US.UTF-8"), Some(Lang::En));
        assert_eq!(Lang::parse("de_DE"), Some(Lang::En));
        assert_eq!(Lang::parse("C"), None);
        assert_eq!(Lang::parse(""), None);
        assert_eq!(pick(Lang::Id, "Save", "Simpan"), "Simpan");
    }
}
