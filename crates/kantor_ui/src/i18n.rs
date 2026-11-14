//! Interface language: English or Indonesian.
//!
//! English is the source language and the key: code writes `t("Insert Row
//! Above")`, and the Indonesian comes from the catalog files in
//! `crates/kantor_ui/i18n/` (one per app area, `English<TAB>Indonesian` per
//! line). Strings with values in them use `{}` placeholders:
//! `tf("Page {} of {}", &[&n, &m])`. A short string that is only used in one
//! place can also be written as a pair in place, `tr("Synced", "Tersinkron")`.
//!
//! The `catalog_*` tests fail when a `t`/`tf`/`tk` string in the UI crates
//! has no Indonesian, or a catalog line has no English use left.
//!
//! The language is chosen once per run: `KANTOR_LANG`, then `language` in
//! settings.toml, then the system locale (`LC_ALL`, `LC_MESSAGES`, `LANG`;
//! `id…` is Indonesian, anything else English).

use std::collections::HashMap;
use std::fmt::Display;
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

/// The catalog files, `English<TAB>Indonesian` per line. `#` starts a
/// comment line; blank lines are ignored. A literal tab or newline in a
/// string is written `\t` / `\n`.
pub const CATALOG_FILES: &[(&str, &str)] = &[
    ("common.tsv", include_str!("../i18n/common.tsv")),
    ("kantor.tsv", include_str!("../i18n/kantor.tsv")),
    ("sheets.tsv", include_str!("../i18n/sheets.tsv")),
    ("docs.tsv", include_str!("../i18n/docs.tsv")),
    ("slides.tsv", include_str!("../i18n/slides.tsv")),
];

/// One catalog line: `(english, indonesian)`, unescaped.
pub fn parse_catalog(text: &'static str) -> impl Iterator<Item = (String, String)> {
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (en, id) = l.split_once('\t')?;
            Some((unescape(en), unescape(id)))
        })
}

fn unescape(s: &str) -> String {
    s.replace("\\t", "\t").replace("\\n", "\n")
}

/// English → Indonesian, from all catalog files. Strings are leaked once
/// so lookups hand out `&'static str`.
fn catalog() -> &'static HashMap<&'static str, &'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut map = HashMap::new();
        for (_, text) in CATALOG_FILES {
            for (en, id) in parse_catalog(text) {
                let en: &'static str = Box::leak(en.into_boxed_str());
                let id: &'static str = Box::leak(id.into_boxed_str());
                map.insert(en, id);
            }
        }
        map
    })
}

/// The Indonesian for an English string, if the catalog has it.
pub fn indonesian(en: &str) -> Option<&'static str> {
    catalog().get(en).copied()
}

/// `en` in the interface language (English when the catalog lacks it).
pub fn t(en: &'static str) -> &'static str {
    t_in(lang(), en)
}

/// [`t`] for any string (titles kept in data, built at run time); the
/// result borrows `en` when there's no translation.
pub fn t_dyn(en: &str) -> &str {
    match lang() {
        Lang::En => en,
        Lang::Id => indonesian(en).unwrap_or(en),
    }
}

/// [`t`] for `lang` (testable without the global).
pub fn t_in(lang: Lang, en: &'static str) -> &'static str {
    match lang {
        Lang::En => en,
        Lang::Id => indonesian(en).unwrap_or(en),
    }
}

/// A catalog string with `{}` placeholders filled in order:
/// `tf("Page {} of {}", &[&3, &12])`.
pub fn tf(template: &'static str, args: &[&dyn Display]) -> String {
    fill(t(template), args)
}

/// Fill `{}` placeholders in order (extra placeholders stay empty).
pub fn fill(template: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut args = args.iter();
    let mut rest = template;
    while let Some(i) = rest.find("{}") {
        out.push_str(&rest[..i]);
        if let Some(a) = args.next() {
            out.push_str(&a.to_string());
        }
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

/// Marks an English string kept in data (a table of titles) as a catalog
/// key; translate it where it's shown with [`t`] or [`t_dyn`].
pub const fn tk(en: &'static str) -> &'static str {
    en
}

/// Whether `query` matches `text` or its translation, for searches that
/// should find a command by either language's name.
pub fn both_languages(en: &str) -> String {
    match indonesian(en) {
        Some(id) if id != en => format!("{en} {id}"),
        _ => en.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Path, PathBuf};

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

    #[std::prelude::v1::test]
    fn fills_placeholders() {
        assert_eq!(fill("Page {} of {}", &[&3, &12]), "Page 3 of 12");
        assert_eq!(fill("{} rows", &[&"5"]), "5 rows");
        assert_eq!(fill("none", &[]), "none");
        assert_eq!(fill("{} and {}", &[&1]), "1 and ");
    }

    #[std::prelude::v1::test]
    fn looks_up_the_catalog() {
        assert_eq!(t_in(Lang::Id, "Save"), "Simpan");
        assert_eq!(t_in(Lang::En, "Save"), "Save");
        assert_eq!(
            t_in(Lang::Id, "no such string, kept"),
            "no such string, kept"
        );
        assert!(both_languages("Save").contains("Simpan"));
    }

    /// The UI crates whose sources are scanned for catalog keys.
    const UI_CRATES: &[&str] = &["kantor", "kantor_ui", "sheets_ui", "docs_ui", "slides_ui"];

    fn crates_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("crates/")
            .to_path_buf()
    }

    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                rust_files(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }

    /// Every string literal passed straight to `t(`, `tf(` or `tk(`, with
    /// where it was found. Raw strings and escapes beyond `\"`, `\\`, `\n`
    /// and `\t` aren't used for UI text.
    pub(super) fn keys_in_sources() -> BTreeMap<String, String> {
        let mut files = Vec::new();
        for c in UI_CRATES {
            rust_files(&crates_dir().join(c).join("src"), &mut files);
        }
        let mut keys = BTreeMap::new();
        for f in files {
            let text = std::fs::read_to_string(&f).unwrap_or_default();
            // This file's own examples and tests aren't UI text.
            if f.ends_with("kantor_ui/src/i18n.rs") {
                continue;
            }
            let lines: Vec<&str> = text.lines().collect();
            for (line_no, line) in lines.iter().enumerate() {
                for call in ["t(", "tf(", "tk("] {
                    let mut rest = *line;
                    while let Some(i) = rest.find(call) {
                        let before = rest[..i].chars().next_back();
                        rest = &rest[i + call.len()..];
                        // `t(` must be the whole name, not `get(` or `split(`.
                        if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                            continue;
                        }
                        // The literal right after the call, or on the next
                        // line when rustfmt moved it there.
                        let arg = if let Some(r) = rest.strip_prefix('"') {
                            r
                        } else if rest.trim().is_empty()
                            && let Some(r) = lines
                                .get(line_no + 1)
                                .and_then(|l| l.trim_start().strip_prefix('"'))
                        {
                            r
                        } else {
                            continue;
                        };
                        if let Some(s) = literal(arg) {
                            keys.entry(s).or_insert_with(|| {
                                format!(
                                    "{}:{}",
                                    f.strip_prefix(crates_dir()).unwrap_or(&f).display(),
                                    line_no + 1
                                )
                            });
                        }
                    }
                }
            }
        }
        keys
    }

    /// The contents of a string literal starting at `s` (after its `"`).
    fn literal(s: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    c => out.push(c),
                },
                c => out.push(c),
            }
        }
        // A literal continued on the next line: not used for keys.
        None
    }

    #[std::prelude::v1::test]
    fn catalog_has_every_key() {
        let missing: Vec<String> = keys_in_sources()
            .into_iter()
            .filter(|(k, _)| indonesian(k).is_none())
            .map(|(k, at)| format!("{at}: {k:?}"))
            .collect();
        assert!(
            missing.is_empty(),
            "{} string(s) have no Indonesian in crates/kantor_ui/i18n/*.tsv:\n{}",
            missing.len(),
            missing.join("\n")
        );
    }

    #[std::prelude::v1::test]
    fn catalog_lines_are_well_formed() {
        let mut seen: BTreeMap<String, (String, &str)> = BTreeMap::new();
        let mut problems = Vec::new();
        for (file, text) in CATALOG_FILES {
            for (n, line) in text.lines().enumerate() {
                if line.trim().is_empty() || line.starts_with('#') {
                    continue;
                }
                let Some((en, id)) = line.split_once('\t') else {
                    problems.push(format!("{file}:{}: no tab: {line:?}", n + 1));
                    continue;
                };
                if en.trim().is_empty() || id.trim().is_empty() {
                    problems.push(format!("{file}:{}: empty side: {line:?}", n + 1));
                }
                if id.contains('\t') {
                    problems.push(format!("{file}:{}: more than one tab", n + 1));
                }
                if en.matches("{}").count() != id.matches("{}").count() {
                    problems.push(format!("{file}:{}: placeholders differ: {line:?}", n + 1));
                }
                if let Some((other, f)) = seen.get(en)
                    && other != id
                {
                    problems.push(format!(
                        "{file}:{}: {en:?} is {id:?} here but {other:?} in {f}",
                        n + 1
                    ));
                }
                seen.insert(en.to_owned(), (id.to_owned(), file));
            }
        }
        assert!(problems.is_empty(), "{}", problems.join("\n"));
    }

    /// Catalog lines no code uses any more. Keys kept in data and shown with
    /// `t_dyn` must be listed in a test that checks them (see
    /// `extra_keys`), or marked with `tk("…")` in the code.
    #[std::prelude::v1::test]
    fn catalog_has_no_unused_lines() {
        let used: BTreeSet<String> = keys_in_sources().into_keys().collect();
        let unused: Vec<String> = CATALOG_FILES
            .iter()
            .flat_map(|(f, text)| parse_catalog(text).map(move |(en, _)| (f, en)))
            .filter(|(_, en)| !used.contains(en))
            .map(|(f, en)| format!("{f}: {en:?}"))
            .collect();
        assert!(
            unused.is_empty(),
            "{} catalog line(s) are not used by any t/tf/tk call:\n{}",
            unused.len(),
            unused.join("\n")
        );
    }
}
