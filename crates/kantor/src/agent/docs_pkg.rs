//! A `.docx` package held in memory as its parts, for writing new
//! documents (`kantor docs build`) and changing parts of existing ones
//! (`kantor docs edit`: inserted blocks, styles, headers, page setup).

use std::collections::BTreeMap;
use std::io::{Cursor, Read as _, Write as _};

use anyhow::{Context as _, Result, bail};

pub const XML_DECL: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";

pub const NS_W: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
pub const NS_R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
pub const NS_A: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";
pub const NS_PIC: &str = "http://schemas.openxmlformats.org/drawingml/2006/picture";
pub const NS_C: &str = "http://schemas.openxmlformats.org/drawingml/2006/chart";
pub const NS_WP: &str = "http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing";
pub const NS_WPS: &str = "http://schemas.microsoft.com/office/word/2010/wordprocessingShape";
pub const NS_MC: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
pub const NS_W14: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
pub const NS_M: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";

const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/";
pub const REL_PKG: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

/// Relationship type `kind` (`styles`, `image`, `header`…).
pub fn rel_type(kind: &str) -> String {
    format!("{REL}{kind}")
}

pub const CT_MAIN: &str =
    "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml";
const CT_WML: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.";
pub const CT_CHART: &str = "application/vnd.openxmlformats-officedocument.drawingml.chart+xml";
pub const CT_XLSX: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
pub const CT_THEME: &str = "application/vnd.openxmlformats-officedocument.theme+xml";
pub const CT_CORE: &str = "application/vnd.openxmlformats-package.core-properties+xml";
pub const CT_APP: &str = "application/vnd.openxmlformats-officedocument.extended-properties+xml";

/// Content type of WordprocessingML part `kind` (`styles`, `header`…).
pub fn wml_type(kind: &str) -> String {
    format!("{CT_WML}{kind}+xml")
}

/// Escape text for XML content and attribute values; characters XML 1.0
/// can't hold are dropped.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// A relationship of a part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rel {
    pub id: String,
    pub ty: String,
    pub target: String,
    pub external: bool,
}

/// `.rels` part name of `part` (`word/document.xml` → `word/_rels/document.xml.rels`).
pub fn rels_name(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// The part `target` names, relative to `source`'s folder.
pub fn resolve(source: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_owned();
    }
    let mut parts: Vec<&str> = match source.rsplit_once('/') {
        Some((dir, _)) => dir.split('/').collect(),
        None => Vec::new(),
    };
    for seg in target.split('/') {
        match seg {
            ".." => {
                parts.pop();
            }
            "." | "" => {}
            s => parts.push(s),
        }
    }
    parts.join("/")
}

/// `part` as a target relative to `source`'s folder.
pub fn relative(source: &str, part: &str) -> String {
    let dir = source.rsplit_once('/').map_or("", |(d, _)| d);
    if dir.is_empty() {
        return part.to_owned();
    }
    match part.strip_prefix(&format!("{dir}/")) {
        Some(rest) => rest.to_owned(),
        None => {
            let depth = dir.split('/').count();
            format!("{}{part}", "../".repeat(depth))
        }
    }
}

/// The value of attribute `name` (with its prefix as written, e.g.
/// `w:val`) in start tag `tag`.
pub fn attr_of(tag: &str, name: &str) -> Option<String> {
    let pat = format!(" {name}=\"");
    let i = tag.find(&pat)? + pat.len();
    let end = tag[i..].find('"')? + i;
    Some(
        tag[i..end]
            .replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

/// A package as parts by zip entry name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pkg {
    pub parts: BTreeMap<String, Vec<u8>>,
}

impl Pkg {
    /// An empty package with only `[Content_Types].xml` and no parts.
    pub fn new() -> Self {
        let mut p = Self::default();
        p.set_text(
            "[Content_Types].xml",
            format!(
                "{XML_DECL}<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
                 <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
                 <Default Extension=\"xml\" ContentType=\"application/xml\"/></Types>"
            ),
        );
        p
    }

    pub fn read(bytes: &[u8]) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).context("not a .docx (zip) file")?;
        let mut parts = BTreeMap::new();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            if f.is_dir() {
                continue;
            }
            let name = f.name()?.to_string();
            let mut v = Vec::new();
            f.read_to_end(&mut v)
                .with_context(|| format!("reading {name}"))?;
            parts.insert(name, v);
        }
        if !parts.contains_key("[Content_Types].xml") {
            bail!("not a .docx file (no [Content_Types].xml)");
        }
        Ok(Self { parts })
    }

    /// The zip: content types first, then the package relationships, then
    /// the rest by name. Pictures are stored, the rest deflated; entries
    /// carry no time, so the same parts give the same bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let deflate = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        let store = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored)
            .last_modified_time(zip::DateTime::default());
        let first = ["[Content_Types].xml", "_rels/.rels"];
        let names = first
            .iter()
            .map(|s| s.to_string())
            .filter(|n| self.parts.contains_key(n))
            .chain(
                self.parts
                    .keys()
                    .filter(|k| !first.contains(&k.as_str()))
                    .cloned(),
            );
        for name in names {
            let bytes = &self.parts[&name];
            let lower = name.to_ascii_lowercase();
            let stored = [".png", ".jpg", ".jpeg", ".gif"]
                .iter()
                .any(|e| lower.ends_with(e));
            out.start_file(name.as_str(), if stored { store } else { deflate })?;
            out.write_all(bytes)?;
        }
        Ok(out.finish()?.into_inner())
    }

    pub fn has(&self, name: &str) -> bool {
        self.parts.contains_key(name)
    }

    pub fn text(&self, name: &str) -> Option<String> {
        let b = self.parts.get(name)?;
        let b = b.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(b);
        String::from_utf8(b.to_vec()).ok()
    }

    pub fn set_text(&mut self, name: &str, text: String) {
        self.parts.insert(name.to_owned(), text.into_bytes());
    }

    pub fn set(&mut self, name: &str, bytes: Vec<u8>) {
        self.parts.insert(name.to_owned(), bytes);
    }

    /// Declare `part`'s content type (an `Override`), replacing one there.
    pub fn set_override(&mut self, part: &str, ct: &str) {
        let name = "[Content_Types].xml";
        let mut t = self.text(name).unwrap_or_default();
        let pat = format!("PartName=\"/{}\"", esc(part));
        if let Some(i) = t.find(&pat) {
            let start = t[..i].rfind('<').unwrap_or(i);
            let end = t[i..].find("/>").map_or(t.len(), |e| i + e + 2);
            t.replace_range(start..end, "");
        }
        let item = format!(
            "<Override PartName=\"/{}\" ContentType=\"{}\"/>",
            esc(part),
            esc(ct)
        );
        insert_before(&mut t, "</Types>", &item);
        self.set_text(name, t);
    }

    /// Declare the content type of files ending in `.ext` (kept if there).
    pub fn set_default(&mut self, ext: &str, ct: &str) {
        let name = "[Content_Types].xml";
        let mut t = self.text(name).unwrap_or_default();
        let lower = t.to_ascii_lowercase();
        if lower.contains(&format!("extension=\"{}\"", ext.to_ascii_lowercase())) {
            return;
        }
        let item = format!("<Default Extension=\"{ext}\" ContentType=\"{ct}\"/>");
        // Defaults go before the overrides.
        match t.find("<Override") {
            Some(i) => t.insert_str(i, &item),
            None => insert_before(&mut t, "</Types>", &item),
        }
        self.set_text(name, t);
    }

    /// The relationships of `part`.
    pub fn rels(&self, part: &str) -> Vec<Rel> {
        let Some(t) = self.text(&rels_name(part)) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut rest = t.as_str();
        while let Some(i) = rest.find("<Relationship ") {
            let end = rest[i..].find('>').map_or(rest.len(), |e| i + e + 1);
            let tag = &rest[i..end];
            out.push(Rel {
                id: attr_of(tag, "Id").unwrap_or_default(),
                ty: attr_of(tag, "Type").unwrap_or_default(),
                target: attr_of(tag, "Target").unwrap_or_default(),
                external: attr_of(tag, "TargetMode").as_deref() == Some("External"),
            });
            rest = &rest[end..];
        }
        out
    }

    /// Add a relationship from `part` (to another part, or an external
    /// URL); returns its id (`stem` with the lowest free number).
    pub fn add_rel(
        &mut self,
        part: &str,
        ty: &str,
        target: &str,
        external: bool,
        stem: &str,
    ) -> String {
        let name = rels_name(part);
        let mut t = self.text(&name).unwrap_or_else(|| {
            format!("{XML_DECL}<Relationships xmlns=\"{REL_PKG}\"></Relationships>")
        });
        let have: Vec<String> = self.rels(part).into_iter().map(|r| r.id).collect();
        let id = (1..)
            .map(|n| format!("{stem}{n}"))
            .find(|id| !have.contains(id))
            .unwrap_or_default();
        let mode = if external {
            " TargetMode=\"External\""
        } else {
            ""
        };
        let item = format!(
            "<Relationship Id=\"{id}\" Type=\"{}\" Target=\"{}\"{mode}/>",
            esc(ty),
            esc(target)
        );
        if t.contains("<Relationships")
            && t.trim_end().ends_with("/>")
            && !t.contains("</Relationships>")
        {
            // `<Relationships …/>`: open it.
            let i = t.rfind("/>").unwrap_or(t.len());
            t.replace_range(i.., "></Relationships>");
        }
        insert_before(&mut t, "</Relationships>", &item);
        self.set_text(&name, t);
        id
    }

    /// The main document part (`word/document.xml` usually).
    pub fn document_part(&self) -> Result<String> {
        self.rels("")
            .into_iter()
            .find(|r| r.ty.ends_with("/officeDocument") && !r.external)
            .map(|r| resolve("", &r.target))
            .filter(|p| self.has(p))
            .context("the package has no main document part")
    }

    /// The part `source` links to with a relationship of type `kind`.
    pub fn related(&self, source: &str, kind: &str) -> Option<String> {
        self.rels(source)
            .into_iter()
            .find(|r| !r.external && r.ty.ends_with(&format!("/{kind}")))
            .map(|r| resolve(source, &r.target))
    }

    /// A part name not in the package: `{stem}{n}.{ext}` from 1.
    pub fn free_name(&self, stem: &str, ext: &str) -> String {
        (1..)
            .map(|n| format!("{stem}{n}.{ext}"))
            .find(|n| !self.has(n))
            .unwrap_or_default()
    }
}

/// Insert `item` before the last `close` in `t` (or append it).
pub fn insert_before(t: &mut String, close: &str, item: &str) {
    match t.rfind(close) {
        Some(i) => t.insert_str(i, item),
        None => t.push_str(item),
    }
}

/// The largest number `attr="N"` takes in `xml` (0 when none).
pub fn max_attr(xml: &str, attr: &str) -> i64 {
    let pat = format!("{attr}=\"");
    let mut best = 0;
    let mut rest = xml;
    while let Some(i) = rest.find(&pat) {
        let s = &rest[i + pat.len()..];
        let end = s.find('"').unwrap_or(0);
        if let Ok(n) = s[..end].parse::<i64>() {
            best = best.max(n);
        }
        rest = &s[end..];
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn parts_rels_and_content_types() {
        let mut p = Pkg::new();
        p.set_override("word/document.xml", CT_MAIN);
        p.set_default("png", "image/png");
        p.set_default("PNG", "image/png");
        let id = p.add_rel(
            "",
            &rel_type("officeDocument"),
            "word/document.xml",
            false,
            "rId",
        );
        assert_eq!(id, "rId1");
        p.set_text("word/document.xml", "<w:document/>".into());
        assert_eq!(p.document_part().unwrap(), "word/document.xml");
        let a = p.add_rel(
            "word/document.xml",
            &rel_type("image"),
            "media/a.png",
            false,
            "rId",
        );
        let b = p.add_rel(
            "word/document.xml",
            &rel_type("hyperlink"),
            "https://x.org/?a&b",
            true,
            "rId",
        );
        assert_eq!((a.as_str(), b.as_str()), ("rId1", "rId2"));
        let rels = p.rels("word/document.xml");
        assert_eq!(rels[1].target, "https://x.org/?a&b");
        assert!(rels[1].external);
        assert_eq!(
            p.related("word/document.xml", "image").as_deref(),
            Some("word/media/a.png")
        );
        let ct = p.text("[Content_Types].xml").unwrap();
        assert_eq!(ct.matches("Extension=\"png\"").count(), 1);
        assert!(ct.find("Extension=\"png\"").unwrap() < ct.find("<Override").unwrap());
        // The same parts, the same bytes; and they read back.
        let bytes = p.to_bytes().unwrap();
        assert_eq!(bytes, p.to_bytes().unwrap());
        assert_eq!(Pkg::read(&bytes).unwrap(), p);
        assert_eq!(
            resolve("word/document.xml", "../customXml/a.xml"),
            "customXml/a.xml"
        );
        assert_eq!(
            relative("word/document.xml", "word/charts/chart1.xml"),
            "charts/chart1.xml"
        );
        assert_eq!(
            relative("word/charts/chart1.xml", "word/embeddings/a.xlsx"),
            "../../word/embeddings/a.xlsx"
        );
        assert_eq!(
            max_attr("<a w:id=\"3\"/><a w:id=\"12\"/><a w:id=\"x\"/>", "w:id"),
            12
        );
        assert_eq!(esc("a<b & \"c\"\u{1}"), "a&lt;b &amp; &quot;c&quot;");
    }
}
