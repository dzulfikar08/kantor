//! JavaScript and Python macros stored in the workbook.
//!
//! They live in one custom XML part (`customXml/itemN.xml`, linked from the
//! workbook part with the standard `customXml` relationship, with its
//! `itemPropsN.xml`), the mechanism OOXML has for an application's own data:
//! Excel keeps custom XML parts when it saves (they are its
//! `Workbook.CustomXMLParts`) and ignores what it doesn't know, so the
//! scripts travel with the file through Excel, in `.xlsx` and `.xlsm` alike.
//! The part is one element per script:
//!
//! ```xml
//! <scripts xmlns="https://kantor.paparanai.id/2026/scripts">
//!   <script name="Report" language="javascript">function go() { … }</script>
//! </scripts>
//! ```
//!
//! [`crate::Workbook::scripts`] reads them from the file the workbook was
//! opened from (or the list set since with [`crate::Workbook::set_scripts`]);
//! a save writes the set list in place of the old part, or carries the
//! original's part as it is.

use roxmltree::Document;

/// The namespace of the scripts part's root element.
pub const NAMESPACE: &str = "https://kantor.paparanai.id/2026/scripts";

/// The language of a script.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScriptLanguage {
    JavaScript,
    Python,
}

impl ScriptLanguage {
    /// The `language` attribute's value.
    pub fn key(self) -> &'static str {
        match self {
            ScriptLanguage::JavaScript => "javascript",
            ScriptLanguage::Python => "python",
        }
    }

    pub fn from_key(key: &str) -> Option<ScriptLanguage> {
        match key.to_ascii_lowercase().as_str() {
            "javascript" | "js" => Some(ScriptLanguage::JavaScript),
            "python" | "py" => Some(ScriptLanguage::Python),
            _ => None,
        }
    }

    /// The language a file extension (`js`, `py`) is in.
    pub fn from_extension(ext: &str) -> Option<ScriptLanguage> {
        match ext.to_ascii_lowercase().as_str() {
            "js" | "mjs" => Some(ScriptLanguage::JavaScript),
            "py" => Some(ScriptLanguage::Python),
            _ => None,
        }
    }

    /// `js` or `py`: the file name errors show (`Report.js`).
    pub fn extension(self) -> &'static str {
        match self {
            ScriptLanguage::JavaScript => "js",
            ScriptLanguage::Python => "py",
        }
    }

    /// As people name it.
    pub fn label(self) -> &'static str {
        match self {
            ScriptLanguage::JavaScript => "JavaScript",
            ScriptLanguage::Python => "Python",
        }
    }
}

/// One script: a module of JavaScript or Python, like a VBA module.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Script {
    /// The module name (`Report`), unique in the workbook (any case).
    pub name: String,
    pub language: ScriptLanguage,
    pub source: String,
}

impl Script {
    pub fn new(name: &str, language: ScriptLanguage, source: &str) -> Script {
        Script {
            name: name.to_owned(),
            language,
            source: source.to_owned(),
        }
    }

    /// `Report.js`.
    pub fn file_name(&self) -> String {
        format!("{}.{}", self.name, self.language.extension())
    }
}

/// Whether `name` can name a script: a letter, then letters, digits or
/// underscores (it is used as `Module.function`).
pub fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(char::is_alphabetic)
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && name.chars().count() <= 64
}

/// The scripts part's XML.
pub fn to_xml(scripts: &[Script]) -> String {
    let mut out = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<scripts xmlns=\"{NAMESPACE}\">"
    );
    for s in scripts {
        out.push_str(&format!(
            "\n<script name=\"{}\" language=\"{}\">{}</script>",
            escape(&s.name, true),
            s.language.key(),
            escape(&s.source, false)
        ));
    }
    out.push_str("\n</scripts>");
    out
}

/// Whether `xml` is a scripts part (its root element is `scripts` in
/// [`NAMESPACE`]).
pub fn is_scripts_part(xml: &str) -> bool {
    Document::parse(xml).is_ok_and(|d| {
        let root = d.root_element();
        root.tag_name().name() == "scripts" && root.tag_name().namespace() == Some(NAMESPACE)
    })
}

/// The scripts in a scripts part; `None` when `xml` isn't one. Scripts in a
/// language Kantor doesn't know are left out.
pub fn from_xml(xml: &str) -> Option<Result<Vec<Script>, String>> {
    let doc = match Document::parse(xml) {
        Ok(d) => d,
        Err(_) => return None,
    };
    let root = doc.root_element();
    if root.tag_name().name() != "scripts" || root.tag_name().namespace() != Some(NAMESPACE) {
        return None;
    }
    let mut out = Vec::new();
    for n in root.children().filter(|n| n.is_element()) {
        if n.tag_name().name() != "script" {
            continue;
        }
        let Some(name) = n.attribute("name") else {
            return Some(Err("a script without a name".into()));
        };
        let Some(language) = n.attribute("language").and_then(ScriptLanguage::from_key) else {
            continue;
        };
        let source: String = n
            .children()
            .filter(|c| c.is_text())
            .filter_map(|c| c.text())
            .collect();
        out.push(Script {
            name: name.to_owned(),
            language,
            source,
        });
    }
    Some(Ok(out))
}

/// XML text (or an attribute value) for `s`. Carriage returns are kept as
/// `&#13;` (an XML reader turns a raw CR LF into LF); characters XML 1.0
/// can't hold become U+FFFD.
pub(crate) fn escape(s: &str, attr: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            '\r' => out.push_str("&#13;"),
            '\t' | '\n' if attr => out.push_str(if c == '\t' { "&#9;" } else { "&#10;" }),
            '\t' | '\n' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => out.push('\u{FFFD}'),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let scripts = vec![
            Script::new(
                "Report",
                ScriptLanguage::JavaScript,
                "function go() {\r\n  if (a < b && c > d) print(\"]]>\");\r\n}\n",
            ),
            Script::new("Tools", ScriptLanguage::Python, "def go():\n\tprint('x')\n"),
        ];
        let xml = to_xml(&scripts);
        assert!(is_scripts_part(&xml));
        assert_eq!(from_xml(&xml).unwrap().unwrap(), scripts);
        assert!(from_xml("<root/>").is_none());
        assert!(!is_scripts_part("<scripts/>"));
    }

    #[test]
    fn names() {
        assert!(valid_name("Module1"));
        assert!(valid_name("my_tools"));
        assert!(!valid_name("1x"));
        assert!(!valid_name("a.b"));
        assert!(!valid_name(""));
    }
}
