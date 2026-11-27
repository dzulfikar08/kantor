//! CSL styles: found by id (a Zotero style URL such as
//! `http://www.zotero.org/styles/apa`), short name (`apa`, `ieee`) or
//! `.csl` file path.
//!
//! Where a style comes from, in order: a `.csl` file named by path;
//! hayagriva's archive (by id, then by the id's last part as a name);
//! Zotero's `styles` folder (`~/Zotero/styles/<name>.csl`), where a
//! university's own style installed in Zotero lives. A dependent style
//! (a journal that uses APA) is resolved to its parent the same way.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use hayagriva::archive::ArchivedStyle;
use hayagriva::citationberg::{IndependentStyle, InfoLinkRel, StyleClass};

/// Kantor's default: APA 7th edition.
pub const DEFAULT_STYLE: &str = "http://www.zotero.org/styles/apa";

/// The styles Kantor offers by name: id and what the menu shows.
pub const BUNDLED: [(&str, &str); 6] = [
    ("http://www.zotero.org/styles/apa", "APA 7th edition"),
    ("http://www.zotero.org/styles/ieee", "IEEE"),
    (
        "http://www.zotero.org/styles/harvard-cite-them-right",
        "Harvard (Cite Them Right)",
    ),
    (
        "http://www.zotero.org/styles/chicago-author-date",
        "Chicago author-date",
    ),
    ("http://www.zotero.org/styles/vancouver", "Vancouver"),
    (
        "http://www.zotero.org/styles/modern-language-association",
        "MLA 9th edition",
    ),
];

/// A loaded style.
#[derive(Clone, Debug)]
pub struct CslStyle {
    /// The id it was asked for (written back into the document).
    pub id: String,
    /// Its title (`APA Style 7th edition`).
    pub title: String,
    pub style: IndependentStyle,
    /// A dependent style's own default locale, else the style's.
    pub default_locale: Option<String>,
}

impl CslStyle {
    /// Whether citations go in footnotes (Kantor renders them in the text).
    pub fn is_note(&self) -> bool {
        self.style.settings.class == StyleClass::Note
    }

    /// Whether the style has a bibliography.
    pub fn has_bibliography(&self) -> bool {
        self.style.bibliography.is_some()
    }
}

/// The last part of a style id: `apa` from
/// `http://www.zotero.org/styles/apa`, `ieee` from `ieee.csl`.
pub fn short_name(id: &str) -> &str {
    let last = id.trim_end_matches('/').rsplit('/').next().unwrap_or(id);
    last.strip_suffix(".csl").unwrap_or(last)
}

/// The name a style id shows as: the bundled name, else the archive's
/// title, else its short name.
pub fn display_name(id: &str) -> String {
    if let Some((_, name)) = BUNDLED.iter().find(|(b, _)| *b == id) {
        return (*name).to_owned();
    }
    archived(id)
        .map(|a| a.display_name().to_owned())
        .unwrap_or_else(|| short_name(id).to_owned())
}

/// The id a style is written into documents with: Zotero's URL for a
/// short name the archive knows (`apa` → `http://www.zotero.org/styles/apa`),
/// else as given.
pub fn canonical_id(id: &str) -> String {
    let id = id.trim();
    if id.contains("://") || id.ends_with(".csl") {
        return id.to_owned();
    }
    if let Some((b, _)) = BUNDLED.iter().find(|(b, _)| short_name(b) == id) {
        return (*b).to_owned();
    }
    ArchivedStyle::by_name(id)
        .map(|a| a.csl_id().to_owned())
        .unwrap_or_else(|| id.to_owned())
}

fn archived(id: &str) -> Option<ArchivedStyle> {
    ArchivedStyle::by_id(id)
        .or_else(|| ArchivedStyle::by_id(&id.replace("https://", "http://")))
        .or_else(|| ArchivedStyle::by_name(short_name(id)))
}

/// Zotero's styles folder (`~/Zotero/styles`).
pub fn zotero_styles_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join("Zotero").join("styles"))
}

/// Find a style by id, short name or path (see the module docs).
pub fn load_style(id: &str) -> anyhow::Result<CslStyle> {
    load_in(id, zotero_styles_dir().as_deref(), 0)
}

/// [`load_style`] looking for installed styles in `dir`.
pub fn load_style_in(id: &str, dir: Option<&Path>) -> anyhow::Result<CslStyle> {
    load_in(id, dir, 0)
}

fn load_in(id: &str, dir: Option<&Path>, depth: usize) -> anyhow::Result<CslStyle> {
    use hayagriva::citationberg::Style;
    if depth > 3 {
        bail!("style {id}: too many dependent styles in a row");
    }
    let id = id.trim();
    let from_file = |path: &Path| -> anyhow::Result<Style> {
        let xml =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Style::from_xml(&xml).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
    };
    let style = if id.ends_with(".csl") && Path::new(id).is_file() {
        from_file(Path::new(id))?
    } else if let Some(a) = archived(id) {
        a.get()
    } else {
        let file = dir
            .map(|d| d.join(format!("{}.csl", short_name(id))))
            .filter(|p| p.is_file());
        match file {
            Some(p) => from_file(&p)?,
            None => bail!(
                "unknown citation style {id}: not one Kantor bundles, and not in Zotero's styles folder"
            ),
        }
    };
    match style {
        Style::Independent(s) => Ok(CslStyle {
            id: id.to_owned(),
            title: s.info.title.value.clone(),
            default_locale: s.default_locale.as_ref().map(|l| l.0.clone()),
            style: s,
        }),
        Style::Dependent(d) => {
            let parent = d
                .info
                .link
                .iter()
                .find(|l| l.rel == InfoLinkRel::IndependentParent)
                .map(|l| l.href.clone())
                .unwrap_or_else(|| d.parent_link.href.clone());
            let mut p = load_in(&parent, dir, depth + 1)?;
            p.id = id.to_owned();
            p.title = d.info.title.value.clone();
            if let Some(l) = &d.default_locale {
                p.default_locale = Some(l.0.clone());
            }
            Ok(p)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_styles_load() {
        for (id, _) in BUNDLED {
            let s = load_style_in(id, None).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert!(s.has_bibliography(), "{id}");
            assert!(!s.is_note(), "{id}");
            assert_eq!(s.id, id);
        }
        assert_eq!(load_style_in("apa", None).unwrap().id, "apa");
        assert!(load_style_in("http://www.zotero.org/styles/no-such-style", None).is_err());
    }

    #[test]
    fn styles_from_a_folder_and_dependents() {
        let dir = tempfile_dir();
        // A dependent style pointing at IEEE.
        std::fs::write(
            dir.join("jurnal-kampus.csl"),
            r#"<?xml version="1.0" encoding="utf-8"?>
<style xmlns="http://purl.org/net/xbiblio/csl" version="1.0" default-locale="id-ID">
  <info>
    <title>Jurnal Kampus</title>
    <id>http://www.zotero.org/styles/jurnal-kampus</id>
    <link href="http://www.zotero.org/styles/jurnal-kampus" rel="self"/>
    <link href="http://www.zotero.org/styles/ieee" rel="independent-parent"/>
    <updated>2026-01-01T00:00:00+00:00</updated>
  </info>
</style>"#,
        )
        .unwrap();
        let s = load_style_in("http://www.zotero.org/styles/jurnal-kampus", Some(&dir)).unwrap();
        assert_eq!(s.title, "Jurnal Kampus");
        assert_eq!(s.default_locale.as_deref(), Some("id-ID"));
        assert!(s.has_bibliography());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempfile_dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("kantor-cite-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn names() {
        assert_eq!(canonical_id("apa"), "http://www.zotero.org/styles/apa");
        assert_eq!(
            canonical_id("vancouver"),
            "http://www.zotero.org/styles/vancouver"
        );
        assert_eq!(
            canonical_id("chicago-author-date"),
            "http://www.zotero.org/styles/chicago-author-date"
        );
        assert_eq!(short_name("http://www.zotero.org/styles/apa"), "apa");
        assert_eq!(display_name("http://www.zotero.org/styles/ieee"), "IEEE");
    }
}
