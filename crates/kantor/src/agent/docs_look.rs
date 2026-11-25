//! Document looks for `kantor docs build` (see docs/docs-json.md): the
//! presets (`report`, `proposal`, `letter`), the palette, real Word
//! paragraph, character and table styles, list and heading numbering, page
//! setup, headers and footers, the cover. A preset is JSON; a document's own
//! `fonts`, `palette`, `styles`, `table`, `callout`, `cover`, `header`,
//! `footer`, `page`, `labels` and `numbered_headings` go over it key by key.

use std::collections::BTreeMap;

use anyhow::{Context as _, Result, anyhow, bail};
use serde_json::{Map, Value, json};

use super::docs_pkg::{XML_DECL, esc};

pub const PRESETS: [&str; 3] = ["report", "proposal", "letter"];

/// Keys of a document that set its look (merged over the preset).
pub const LOOK_KEYS: &[&str] = &[
    "fonts",
    "palette",
    "styles",
    "table",
    "callout",
    "cover_style",
    "header",
    "footer",
    "page",
    "labels",
    "numbered_headings",
    "first_page_header",
];

/// Twips per point / inch / cm.
pub const TW_PT: f64 = 20.0;
pub const TW_IN: f64 = 1440.0;
pub const TW_CM: f64 = 1440.0 / 2.54;

// ── values ─────────────────────────────────────────────────────────────────

/// A number with an optional unit (`"2.5cm"` → (2.5, "cm")).
pub fn split_unit(s: &str) -> Option<(f64, &str)> {
    let s = s.trim();
    let at = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(s.len());
    let n: f64 = s[..at].trim().parse().ok()?;
    n.is_finite().then_some((n, s[at..].trim()))
}

/// A length in twips: a number of points, or a string with a unit (`pt`,
/// `cm`, `mm`, `in`, `px`, `emu`).
pub fn twips(v: &Value, key: &str) -> Result<i64> {
    let tw = match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN) * TW_PT,
        Value::String(s) => {
            let (n, unit) = split_unit(s).ok_or_else(|| {
                anyhow!("`{key}` \"{s}\" is not a length (e.g. \"2.5cm\", \"12pt\")")
            })?;
            n * match unit {
                "" | "pt" => TW_PT,
                "cm" => TW_CM,
                "mm" => TW_CM / 10.0,
                "in" => TW_IN,
                "px" => TW_IN / 96.0,
                "emu" => 1.0 / 635.0,
                "tw" | "twip" | "twips" => 1.0,
                _ => bail!("`{key}` \"{s}\": the unit must be pt, cm, mm, in, px or emu"),
            }
        }
        _ => bail!("`{key}` must be a number of points or a length such as \"2.5cm\""),
    };
    if !tw.is_finite() || tw.abs() > 1.0e7 {
        bail!("`{key}` is out of range");
    }
    Ok(tw.round() as i64)
}

/// Points (font sizes, spacing): a number or `"12pt"`.
pub fn points(v: &Value, key: &str) -> Result<f64> {
    let pt = match v {
        Value::Number(n) => n.as_f64().unwrap_or(f64::NAN),
        Value::String(s) => match split_unit(s) {
            Some((n, "" | "pt")) => n,
            _ => bail!("`{key}` must be in points, e.g. 12"),
        },
        _ => bail!("`{key}` must be a number of points"),
    };
    if !(0.0..=1600.0).contains(&pt) {
        bail!("`{key}` must be between 0 and 1600 points");
    }
    Ok(pt)
}

/// `v` as an object whose keys are all in `allowed`.
pub fn object<'a>(v: &'a Value, what: &str, allowed: &[&str]) -> Result<&'a Map<String, Value>> {
    let Some(obj) = v.as_object() else {
        bail!("{what} must be an object");
    };
    for k in obj.keys() {
        if !allowed.contains(&k.as_str()) {
            bail!(
                "{what} has an unknown key \"{k}\" (allowed: {})",
                allowed.join(", ")
            );
        }
    }
    Ok(obj)
}

pub fn get<'a>(o: &'a Map<String, Value>, k: &str) -> Option<&'a Value> {
    o.get(k).filter(|v| !v.is_null())
}

pub fn opt_str<'a>(o: &'a Map<String, Value>, k: &str) -> Result<Option<&'a str>> {
    match get(o, k) {
        None => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => bail!("`{k}` must be a string"),
    }
}

pub fn opt_bool(o: &Map<String, Value>, k: &str) -> Result<Option<bool>> {
    match get(o, k) {
        None => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => bail!("`{k}` must be true or false"),
    }
}

/// `"#1F3864"` / `"1F3864"` / `"#ABC"` → `1F3864`.
pub fn hex_color(s: &str) -> Option<String> {
    let h = s.trim().trim_start_matches('#');
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match h.len() {
        6 => Some(h.to_ascii_uppercase()),
        3 => Some(
            h.chars()
                .flat_map(|c| [c, c])
                .collect::<String>()
                .to_ascii_uppercase(),
        ),
        _ => None,
    }
}

pub fn rgb(hex: &str) -> [u8; 3] {
    let v = u32::from_str_radix(hex, 16).unwrap_or(0);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

pub fn mix(a: &str, b: &str, t: f64) -> String {
    let (a, b) = (rgb(a), rgb(b));
    let m = |i: usize| (a[i] as f64 * (1.0 - t) + b[i] as f64 * t).round() as u8;
    format!("{:02X}{:02X}{:02X}", m(0), m(1), m(2))
}

// ── presets ────────────────────────────────────────────────────────────────

fn base(fonts: Value, palette: Value) -> Value {
    json!({
        "fonts": fonts,
        "palette": palette,
        "numbered_headings": true,
        "first_page_header": true,
        "page": {"size": "A4", "orientation": "portrait",
                 "margins": {"top": "2.5cm", "bottom": "2.3cm", "left": "2.5cm", "right": "2.5cm"},
                 "header": "1.25cm", "footer": "1.1cm"},
        "styles": {
            "Normal": {"font": "body", "size": 10.5, "color": "ink", "space_after": 6, "line_spacing": 1.2},
            "Title": {"font": "heading", "size": 30, "color": "ink", "space_after": 6, "line_spacing": 1.0},
            "Subtitle": {"font": "body", "size": 14, "color": "muted", "space_after": 12, "line_spacing": 1.1},
            "Heading 1": {"font": "heading", "size": 17, "color": "accent", "space_before": 24, "space_after": 8, "line_spacing": 1.05},
            "Heading 2": {"font": "heading", "size": 13.5, "bold": true, "color": "ink", "space_before": 16, "space_after": 5, "line_spacing": 1.05},
            "Heading 3": {"font": "body", "size": 11, "bold": true, "color": "ink", "space_before": 12, "space_after": 3},
            "Heading 4": {"font": "body", "size": 10.5, "bold": true, "italic": true, "color": "muted", "space_before": 10, "space_after": 2},
            "Quote": {"italic": true, "color": "muted", "size": 11.5, "indent": "1cm", "indent_right": "1cm", "space_before": 6, "space_after": 10},
            "Caption": {"size": 9, "color": "muted", "space_before": 4, "space_after": 12},
            "List Bullet": {"space_after": 3},
            "List Number": {"space_after": 3},
            "Figure": {"align": "center", "keep_with_next": true, "space_before": 6, "space_after": 4, "line_spacing": 1.0},
            "Table Text": {"size": 9.5, "space_after": 0, "line_spacing": 1.1},
            "Table Header": {"size": 9, "bold": true, "color": "ink", "space_after": 0, "line_spacing": 1.1},
            "Callout": {"size": 10, "space_after": 4},
            "Callout Title": {"bold": true, "color": "accent", "space_after": 2},
            "Cover Label": {"size": 10, "bold": true, "caps": true, "letter_spacing": 1.5, "color": "accent", "space_after": 10},
            "Cover Meta": {"size": 9.5, "color": "muted", "space_after": 2},
            "TOC Heading": {"font": "heading", "size": 17, "color": "accent", "space_before": 0, "space_after": 14},
            "TOC 1": {"bold": true, "space_before": 6, "space_after": 2},
            "TOC 2": {"indent": "0.6cm", "space_after": 2},
            "TOC 3": {"indent": "1.2cm", "size": 10, "color": "muted", "space_after": 1},
            "Table of Figures": {"space_after": 2},
            "Header": {"size": 8.5, "color": "muted", "space_after": 0, "line_spacing": 1.0},
            "Footer": {"size": 8.5, "color": "muted", "space_after": 0, "line_spacing": 1.0},
            "Footnote Text": {"size": 8.5, "space_after": 0, "line_spacing": 1.0},
            "No Spacing": {"space_after": 0, "line_spacing": 1.0}
        },
        "table": {"look": "rules", "header_fill": null, "rule": "rule", "edge": "ink", "banded": null},
        "callout": {"fill": "tint", "bar": "accent"},
        "cover_style": {"kind": "bar", "color": "accent"},
        "header": {"left": "{title}", "right": "{heading}"},
        "footer": {"right": "{page}"}
    })
}

/// The JSON of preset `name` (see the module docs).
pub fn preset(name: &str) -> Result<Value> {
    Ok(match name {
        "report" => base(
            json!({"heading": "Cambria", "body": "Calibri", "mono": "Consolas"}),
            json!({"accent": "#2F4B7C", "ink": "#1F2328", "muted": "#5C6672", "rule": "#C8CED6",
                   "tint": "#EDF1F6", "bullet": "ink", "link": "accent",
                   "accent2": "#8DA4C7", "accent3": "#C27A3A", "accent4": "#5C6672",
                   "accent5": "#B8C3D1", "accent6": "#6B8A5A"}),
        ),
        "proposal" => {
            let mut v = base(
                json!({"heading": "Calibri", "body": "Calibri", "mono": "Consolas"}),
                json!({"accent": "#1F5E5B", "ink": "#1E2326", "muted": "#5B666D", "rule": "#D2D9DB",
                       "tint": "#EAF2F0", "bullet": "accent", "link": "accent",
                       "accent2": "#8DB5AF", "accent3": "#C8843E", "accent4": "#5B666D",
                       "accent5": "#BCCBCD", "accent6": "#7D5F43"}),
            );
            super::slides_style::merge(
                &mut v,
                &json!({
                    "styles": {
                        "Normal": {"line_spacing": 1.25, "space_after": 7},
                        "Title": {"size": 32, "bold": true, "line_spacing": 1.0},
                        "Heading 1": {"size": 21, "bold": true, "color": "ink", "page_break_before": true,
                                      "space_before": 0, "space_after": 14},
                        "Heading 2": {"size": 13.5, "bold": true, "color": "accent", "space_before": 18},
                        "Heading 3": {"size": 11, "bold": true, "color": "ink"},
                        "TOC Heading": {"size": 21, "bold": true, "color": "ink"}
                    },
                    "table": {"look": "header", "header_fill": "tint"},
                    "cover_style": {"kind": "band"},
                    "header": {"left": "{title}", "right": "{company}"},
                    "footer": {"left": "{heading}", "right": "{page} / {pages}"}
                }),
            );
            v
        }
        "letter" => {
            let mut v = base(
                json!({"heading": "Calibri", "body": "Calibri", "mono": "Consolas"}),
                json!({"accent": "#33415C", "ink": "#1F1F1F", "muted": "#5F6368", "rule": "#CFD3D8",
                       "tint": "#F0F2F5", "bullet": "ink", "link": "accent",
                       "accent2": "#8C99AD", "accent3": "#B07A45", "accent4": "#5F6368",
                       "accent5": "#C2C8D0", "accent6": "#6F8466"}),
            );
            super::slides_style::merge(
                &mut v,
                &json!({
                    "numbered_headings": false,
                    "first_page_header": false,
                    "page": {"margins": {"top": "2.5cm", "bottom": "2.2cm", "left": "2.5cm", "right": "2.5cm"}},
                    "styles": {
                        "Normal": {"size": 11, "line_spacing": 1.15, "space_after": 8},
                        "Heading 1": {"size": 13, "bold": true, "color": "ink", "space_before": 14, "space_after": 4},
                        "Heading 2": {"size": 11, "bold": true, "color": "ink", "space_before": 10, "space_after": 2},
                        "Title": {"size": 20, "bold": true}
                    },
                    "cover_style": {"kind": "none"},
                    "header": null,
                    "footer": {"right": "{page}"}
                }),
            );
            v
        }
        other => bail!("`preset` \"{other}\" is not one of {}", PRESETS.join(", ")),
    })
}

// ── styles ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Para,
    Char,
}

/// A built-in style: the JSON name, the style id, Word's own name (so Word
/// treats it as its built-in style), kind, based on, next.
pub type Builtin = (
    &'static str,
    &'static str,
    &'static str,
    Kind,
    Option<&'static str>,
    Option<&'static str>,
);

/// The built-in styles every document has.
pub const BUILTIN: &[Builtin] = &[
    ("Normal", "Normal", "Normal", Kind::Para, None, None),
    (
        "Title",
        "Title",
        "Title",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "Subtitle",
        "Subtitle",
        "Subtitle",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "Heading 1",
        "Heading1",
        "heading 1",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "Heading 2",
        "Heading2",
        "heading 2",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "Heading 3",
        "Heading3",
        "heading 3",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "Heading 4",
        "Heading4",
        "heading 4",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "Quote",
        "Quote",
        "Quote",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "Caption",
        "Caption",
        "caption",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "List Bullet",
        "ListBullet",
        "List Bullet",
        Kind::Para,
        Some("Normal"),
        None,
    ),
    (
        "List Bullet 2",
        "ListBullet2",
        "List Bullet 2",
        Kind::Para,
        Some("ListBullet"),
        None,
    ),
    (
        "List Bullet 3",
        "ListBullet3",
        "List Bullet 3",
        Kind::Para,
        Some("ListBullet"),
        None,
    ),
    (
        "List Number",
        "ListNumber",
        "List Number",
        Kind::Para,
        Some("Normal"),
        None,
    ),
    (
        "List Number 2",
        "ListNumber2",
        "List Number 2",
        Kind::Para,
        Some("ListNumber"),
        None,
    ),
    (
        "List Number 3",
        "ListNumber3",
        "List Number 3",
        Kind::Para,
        Some("ListNumber"),
        None,
    ),
    (
        "Figure",
        "Figure",
        "Figure",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "Table Text",
        "TableText",
        "Table Text",
        Kind::Para,
        Some("Normal"),
        None,
    ),
    (
        "Table Header",
        "TableHeader",
        "Table Header",
        Kind::Para,
        Some("TableText"),
        None,
    ),
    (
        "Callout",
        "Callout",
        "Callout",
        Kind::Para,
        Some("Normal"),
        None,
    ),
    (
        "Callout Title",
        "CalloutTitle",
        "Callout Title",
        Kind::Para,
        Some("Callout"),
        Some("Callout"),
    ),
    (
        "Cover Label",
        "CoverLabel",
        "Cover Label",
        Kind::Para,
        Some("Normal"),
        Some("Title"),
    ),
    (
        "Cover Meta",
        "CoverMeta",
        "Cover Meta",
        Kind::Para,
        Some("Normal"),
        None,
    ),
    (
        "TOC Heading",
        "TOCHeading",
        "TOC Heading",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "TOC 1",
        "TOC1",
        "toc 1",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "TOC 2",
        "TOC2",
        "toc 2",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "TOC 3",
        "TOC3",
        "toc 3",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "Table of Figures",
        "TableofFigures",
        "table of figures",
        Kind::Para,
        Some("Normal"),
        Some("Normal"),
    ),
    (
        "Header",
        "Header",
        "header",
        Kind::Para,
        Some("Normal"),
        None,
    ),
    (
        "Footer",
        "Footer",
        "footer",
        Kind::Para,
        Some("Normal"),
        None,
    ),
    (
        "Footnote Text",
        "FootnoteText",
        "footnote text",
        Kind::Para,
        Some("Normal"),
        None,
    ),
    (
        "No Spacing",
        "NoSpacing",
        "No Spacing",
        Kind::Para,
        None,
        None,
    ),
    (
        "Default Paragraph Font",
        "DefaultParagraphFont",
        "Default Paragraph Font",
        Kind::Char,
        None,
        None,
    ),
    (
        "Footnote Reference",
        "FootnoteReference",
        "footnote reference",
        Kind::Char,
        Some("DefaultParagraphFont"),
        None,
    ),
    (
        "Hyperlink",
        "Hyperlink",
        "Hyperlink",
        Kind::Char,
        Some("DefaultParagraphFont"),
        None,
    ),
    (
        "Strong",
        "Strong",
        "Strong",
        Kind::Char,
        Some("DefaultParagraphFont"),
        None,
    ),
    (
        "Emphasis",
        "Emphasis",
        "Emphasis",
        Kind::Char,
        Some("DefaultParagraphFont"),
        None,
    ),
    (
        "Code",
        "Code",
        "Code",
        Kind::Char,
        Some("DefaultParagraphFont"),
        None,
    ),
];

/// Keys of a style definition.
pub const STYLE_KEYS: &[&str] = &[
    "type",
    "based_on",
    "next",
    "font",
    "size",
    "bold",
    "italic",
    "underline",
    "strike",
    "caps",
    "small_caps",
    "color",
    "highlight",
    "letter_spacing",
    "align",
    "space_before",
    "space_after",
    "line_spacing",
    "indent",
    "indent_right",
    "first_line",
    "keep_with_next",
    "keep_together",
    "page_break_before",
    "outline_level",
];

/// Character formatting (style or direct).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunFmt {
    pub style: Option<String>,
    pub font: Option<String>,
    /// Half-points.
    pub size: Option<u32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub strike: Option<bool>,
    pub caps: Option<bool>,
    pub small_caps: Option<bool>,
    /// `RRGGBB`.
    pub color: Option<String>,
    /// Twips.
    pub spacing: Option<i64>,
    pub highlight: Option<String>,
    /// `superscript` / `subscript`.
    pub vert: Option<&'static str>,
    pub lang: Option<String>,
}

fn on_off(name: &str, v: Option<bool>) -> String {
    match v {
        Some(true) => format!("<w:{name}/>"),
        Some(false) => format!("<w:{name} w:val=\"0\"/>"),
        None => String::new(),
    }
}

impl RunFmt {
    /// The `w:rPr` children, in schema order.
    pub fn children(&self) -> String {
        let mut x = String::new();
        if let Some(s) = &self.style {
            x.push_str(&format!("<w:rStyle w:val=\"{}\"/>", esc(s)));
        }
        if let Some(f) = &self.font {
            let f = esc(f);
            x.push_str(&format!(
                "<w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\" w:eastAsia=\"{f}\" w:cs=\"{f}\"/>"
            ));
        }
        x.push_str(&on_off("b", self.bold));
        x.push_str(&on_off("bCs", self.bold));
        x.push_str(&on_off("i", self.italic));
        x.push_str(&on_off("iCs", self.italic));
        x.push_str(&on_off("caps", self.caps));
        x.push_str(&on_off("smallCaps", self.small_caps));
        x.push_str(&on_off("strike", self.strike));
        if let Some(c) = &self.color {
            x.push_str(&format!("<w:color w:val=\"{c}\"/>"));
        }
        if let Some(s) = self.spacing {
            x.push_str(&format!("<w:spacing w:val=\"{s}\"/>"));
        }
        if let Some(s) = self.size {
            x.push_str(&format!("<w:sz w:val=\"{s}\"/><w:szCs w:val=\"{s}\"/>"));
        }
        if let Some(h) = &self.highlight {
            x.push_str(&format!("<w:highlight w:val=\"{h}\"/>"));
        }
        match self.underline {
            Some(true) => x.push_str("<w:u w:val=\"single\"/>"),
            Some(false) => x.push_str("<w:u w:val=\"none\"/>"),
            None => {}
        }
        if let Some(v) = self.vert {
            x.push_str(&format!("<w:vertAlign w:val=\"{v}\"/>"));
        }
        if let Some(l) = &self.lang {
            x.push_str(&format!("<w:lang w:val=\"{}\"/>", esc(l)));
        }
        x
    }

    pub fn xml(&self) -> String {
        let c = self.children();
        if c.is_empty() {
            String::new()
        } else {
            format!("<w:rPr>{c}</w:rPr>")
        }
    }

    /// `o`'s settings over these.
    pub fn overlay(&mut self, o: &RunFmt) {
        macro_rules! take {
            ($($f:ident),*) => {$( if o.$f.is_some() { self.$f = o.$f.clone(); } )*};
        }
        take!(
            style, font, size, bold, italic, underline, strike, caps, small_caps, color, spacing,
            highlight, vert, lang
        );
    }
}

/// Paragraph formatting (style or direct). Lengths in twips.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParaFmt {
    pub align: Option<&'static str>,
    pub before: Option<i64>,
    pub after: Option<i64>,
    /// 240ths of a line.
    pub line: Option<i64>,
    pub indent: Option<i64>,
    pub indent_right: Option<i64>,
    /// Negative: hanging.
    pub first_line: Option<i64>,
    pub keep_next: Option<bool>,
    pub keep_lines: Option<bool>,
    pub page_break_before: Option<bool>,
    pub outline: Option<u8>,
    pub contextual: Option<bool>,
    /// `w:tab` elements.
    pub tabs: Vec<String>,
    /// numId, ilvl.
    pub num: Option<(u32, u8)>,
}

impl ParaFmt {
    /// The `w:pPr` children before `rPr` (style first), in schema order.
    pub fn children(&self, style: Option<&str>) -> String {
        let mut x = String::new();
        if let Some(s) = style {
            x.push_str(&format!("<w:pStyle w:val=\"{}\"/>", esc(s)));
        }
        x.push_str(&on_off("keepNext", self.keep_next));
        x.push_str(&on_off("keepLines", self.keep_lines));
        x.push_str(&on_off("pageBreakBefore", self.page_break_before));
        if let Some((id, lvl)) = self.num {
            x.push_str(&format!(
                "<w:numPr><w:ilvl w:val=\"{lvl}\"/><w:numId w:val=\"{id}\"/></w:numPr>"
            ));
        }
        if !self.tabs.is_empty() {
            x.push_str("<w:tabs>");
            for t in &self.tabs {
                x.push_str(t);
            }
            x.push_str("</w:tabs>");
        }
        if self.before.is_some() || self.after.is_some() || self.line.is_some() {
            x.push_str("<w:spacing");
            if let Some(b) = self.before {
                x.push_str(&format!(" w:before=\"{b}\""));
            }
            if let Some(a) = self.after {
                x.push_str(&format!(" w:after=\"{a}\""));
            }
            if let Some(l) = self.line {
                x.push_str(&format!(" w:line=\"{l}\" w:lineRule=\"auto\""));
            }
            x.push_str("/>");
        }
        if self.indent.is_some() || self.indent_right.is_some() || self.first_line.is_some() {
            x.push_str("<w:ind");
            if let Some(l) = self.indent {
                x.push_str(&format!(" w:left=\"{l}\""));
            }
            if let Some(r) = self.indent_right {
                x.push_str(&format!(" w:right=\"{r}\""));
            }
            match self.first_line {
                Some(f) if f < 0 => x.push_str(&format!(" w:hanging=\"{}\"", -f)),
                Some(f) => x.push_str(&format!(" w:firstLine=\"{f}\"")),
                None => {}
            }
            x.push_str("/>");
        }
        x.push_str(&on_off("contextualSpacing", self.contextual));
        if let Some(a) = self.align {
            x.push_str(&format!("<w:jc w:val=\"{a}\"/>"));
        }
        if let Some(o) = self.outline {
            x.push_str(&format!("<w:outlineLvl w:val=\"{o}\"/>"));
        }
        x
    }
}

/// A style to write into `styles.xml`.
#[derive(Clone, Debug, PartialEq)]
pub struct StyleDef {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub based_on: Option<String>,
    pub next: Option<String>,
    pub run: RunFmt,
    pub para: ParaFmt,
}

/// The built-in entry for a style name or id (case and spaces ignored).
pub fn builtin(name: &str) -> Option<&'static Builtin> {
    let key: String = name
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_lowercase();
    BUILTIN.iter().find(|b| {
        let n: String =
            b.0.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .to_lowercase();
        n == key || b.1.to_lowercase() == key
    })
}

/// The style id for a user's style name (letters and digits).
pub fn custom_id(name: &str) -> String {
    let id: String = name.chars().filter(|c| c.is_alphanumeric()).collect();
    if id.is_empty() { "Style".into() } else { id }
}

pub fn align_of(s: &str) -> Result<&'static str> {
    Ok(match s {
        "left" | "start" => "left",
        "center" | "centre" => "center",
        "right" | "end" => "right",
        "justify" | "both" => "both",
        other => bail!("`align` must be left, center, right or justify, not \"{other}\""),
    })
}

// ── the look ───────────────────────────────────────────────────────────────

/// How tables are drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct TableLook {
    /// `rules` (lines between rows, none down), `grid` (every line),
    /// `header` (a tinted header row, lines between rows), `plain` (none).
    pub look: String,
    pub header_fill: Option<String>,
    pub rule: String,
    pub edge: String,
    pub banded: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CalloutLook {
    pub fill: Option<String>,
    pub bar: Option<String>,
}

/// Header or footer text: left, centre, right (with `{page}`… tokens).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HfSpec {
    pub left: Option<String>,
    pub center: Option<String>,
    pub right: Option<String>,
}

impl HfSpec {
    pub fn is_empty(&self) -> bool {
        self.left.is_none() && self.center.is_none() && self.right.is_none()
    }
}

pub const HF_KEYS: &[&str] = &["left", "center", "right"];

/// `null`/`false`/`"none"` → none; a string → right-aligned text; an
/// object → its parts.
pub fn hf_spec(v: Option<&Value>, key: &str) -> Result<Option<HfSpec>> {
    match v {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Ok(None),
        Some(Value::String(s)) if s == "none" => Ok(None),
        Some(Value::String(s)) => Ok(Some(HfSpec {
            right: Some(s.clone()),
            ..HfSpec::default()
        })),
        Some(v @ Value::Object(_)) => {
            let o = object(v, &format!("`{key}`"), HF_KEYS)?;
            let s = |k: &str| -> Result<Option<String>> { Ok(opt_str(o, k)?.map(str::to_owned)) };
            Ok(Some(HfSpec {
                left: s("left")?,
                center: s("center")?,
                right: s("right")?,
            }))
        }
        Some(_) => bail!("`{key}` must be an object {{left, center, right}}, a string or \"none\""),
    }
}

/// Page setup in twips.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageSpec {
    pub width: i64,
    pub height: i64,
    pub landscape: bool,
    pub top: i64,
    pub bottom: i64,
    pub left: i64,
    pub right: i64,
    pub header: i64,
    pub footer: i64,
}

impl PageSpec {
    pub fn text_width(&self) -> i64 {
        (self.width - self.left - self.right).max(720)
    }
}

pub const PAGE_KEYS: &[&str] = &["size", "orientation", "margins", "header", "footer"];

/// `page` settings over `base`.
pub fn page_spec(v: &Value, base: PageSpec) -> Result<PageSpec> {
    let o = object(v, "`page`", PAGE_KEYS)?;
    let mut p = base;
    if let Some(s) = get(o, "size") {
        let (w, h) = match s {
            Value::String(name) => match name.to_ascii_lowercase().as_str() {
                "a4" => (11906, 16838),
                "letter" => (12240, 15840),
                "legal" => (12240, 20160),
                "a5" => (8391, 11906),
                "a3" => (16838, 23811),
                "f4" | "folio" => (12242, 18709),
                other => bail!(
                    "`size` must be A4, Letter, Legal, A5, A3, F4 or {{w, h}}, not \"{other}\""
                ),
            },
            Value::Object(_) => {
                let so = object(s, "`size`", &["w", "h"])?;
                (
                    twips(get(so, "w").context("`size` needs w")?, "w")?,
                    twips(get(so, "h").context("`size` needs h")?, "h")?,
                )
            }
            _ => bail!("`size` must be a name (A4, Letter) or {{w, h}}"),
        };
        if !(1440..=31680).contains(&w) || !(1440..=31680).contains(&h) {
            bail!("the page size must be between 1 and 22 inches each way");
        }
        p.width = w.min(h);
        p.height = w.max(h);
        if p.landscape {
            std::mem::swap(&mut p.width, &mut p.height);
        }
    }
    if let Some(or) = opt_str(o, "orientation")? {
        let land = match or {
            "portrait" => false,
            "landscape" => true,
            other => bail!("`orientation` must be portrait or landscape, not \"{other}\""),
        };
        if land != p.landscape {
            std::mem::swap(&mut p.width, &mut p.height);
            std::mem::swap(&mut p.top, &mut p.left);
            std::mem::swap(&mut p.bottom, &mut p.right);
            p.landscape = land;
        }
    }
    match get(o, "margins") {
        None => {}
        Some(m @ Value::Object(_)) => {
            let mo = object(m, "`margins`", &["top", "bottom", "left", "right"])?;
            for (k, slot) in [
                ("top", &mut p.top),
                ("bottom", &mut p.bottom),
                ("left", &mut p.left),
                ("right", &mut p.right),
            ] {
                if let Some(v) = get(mo, k) {
                    *slot = twips(v, k)?;
                }
            }
        }
        Some(m) => {
            let all = twips(m, "margins")?;
            p.top = all;
            p.bottom = all;
            p.left = all;
            p.right = all;
        }
    }
    if let Some(v) = get(o, "header") {
        p.header = twips(v, "header")?;
    }
    if let Some(v) = get(o, "footer") {
        p.footer = twips(v, "footer")?;
    }
    for m in [p.top, p.bottom, p.left, p.right, p.header, p.footer] {
        if !(0..=14400).contains(&m) {
            bail!("margins must be between 0 and 10 inches");
        }
    }
    if p.left + p.right > p.width - 1440 || p.top + p.bottom > p.height - 1440 {
        bail!("the margins leave less than an inch for the text");
    }
    Ok(p)
}

pub const A4: PageSpec = PageSpec {
    width: 11906,
    height: 16838,
    landscape: false,
    top: 1440,
    bottom: 1440,
    left: 1440,
    right: 1440,
    header: 709,
    footer: 709,
};

#[derive(Clone, Debug, PartialEq)]
pub struct Labels {
    pub figure: String,
    pub table: String,
    pub contents: String,
    pub figures: String,
    pub tables: String,
}

/// The cover's design.
#[derive(Clone, Debug, PartialEq)]
pub struct CoverLook {
    /// `bar` (an accent strip down the left edge), `band` (an accent block
    /// across the top), `plain` (text only), `none`.
    pub kind: String,
    pub color: String,
}

/// A document's look, read from its preset and its own settings.
#[derive(Clone, Debug)]
pub struct Look {
    pub fonts: BTreeMap<String, String>,
    pub palette: BTreeMap<String, String>,
    pub styles: Vec<StyleDef>,
    pub numbered: bool,
    pub first_page_header: bool,
    pub table: TableLook,
    pub callout: CalloutLook,
    pub cover: CoverLook,
    pub header: Option<HfSpec>,
    pub footer: Option<HfSpec>,
    pub page: PageSpec,
    pub labels: Labels,
    pub lang: String,
}

impl Look {
    /// A colour: a palette name or `#RRGGBB` → `RRGGBB`.
    pub fn color(&self, v: &str) -> Result<String> {
        let key = v.trim();
        if let Some(c) = self.palette.get(key) {
            return Ok(c.clone());
        }
        match key {
            "white" => return Ok("FFFFFF".into()),
            "black" => return Ok("000000".into()),
            "accent1" => return self.color("accent"),
            _ => {}
        }
        hex_color(key).ok_or_else(|| {
            anyhow!(
                "\"{v}\" is not a colour: use #RRGGBB or a palette name ({})",
                self.palette.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })
    }

    /// A font: `heading`, `body`, `mono` or a font name.
    pub fn font(&self, f: &str) -> String {
        self.fonts.get(f).cloned().unwrap_or_else(|| f.to_owned())
    }

    pub fn style(&self, id: &str) -> Option<&StyleDef> {
        self.styles.iter().find(|s| s.id == id)
    }

    /// The style id for a name given in JSON: a built-in's, or one of
    /// the document's own styles.
    pub fn style_id(&self, name: &str) -> Result<String> {
        if let Some(b) = builtin(name) {
            return Ok(b.1.to_owned());
        }
        let id = custom_id(name);
        if self.styles.iter().any(|s| s.id == id || s.name == name) {
            return Ok(id);
        }
        bail!(
            "there is no style \"{name}\" (define it in `styles`, or use one of: {})",
            self.styles
                .iter()
                .filter(|s| s.kind == Kind::Para)
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }

    /// Read character and paragraph keys of a style object.
    pub fn formats(&self, o: &Map<String, Value>) -> Result<(RunFmt, ParaFmt)> {
        let mut r = RunFmt::default();
        let mut p = ParaFmt::default();
        if let Some(f) = opt_str(o, "font")? {
            r.font = Some(self.font(f));
        }
        if let Some(v) = get(o, "size") {
            let pt = points(v, "size")?;
            if pt < 1.0 {
                bail!("`size` must be at least 1 point");
            }
            r.size = Some((pt * 2.0).round() as u32);
        }
        r.bold = opt_bool(o, "bold")?;
        r.italic = opt_bool(o, "italic")?;
        r.underline = opt_bool(o, "underline")?;
        r.strike = opt_bool(o, "strike")?;
        r.caps = opt_bool(o, "caps")?;
        r.small_caps = opt_bool(o, "small_caps")?;
        if let Some(c) = opt_str(o, "color")? {
            r.color = Some(self.color(c)?);
        }
        if let Some(h) = opt_str(o, "highlight")? {
            const HL: &[&str] = &[
                "yellow",
                "green",
                "cyan",
                "magenta",
                "blue",
                "red",
                "darkBlue",
                "darkCyan",
                "darkGreen",
                "darkMagenta",
                "darkRed",
                "darkYellow",
                "darkGray",
                "lightGray",
                "black",
            ];
            if !HL.contains(&h) {
                bail!("`highlight` must be one of {}", HL.join(", "));
            }
            r.highlight = Some(h.to_owned());
        }
        if let Some(v) = get(o, "letter_spacing") {
            r.spacing = Some(match v {
                Value::Number(n) => (n.as_f64().unwrap_or(0.0) * TW_PT).round() as i64,
                v => twips(v, "letter_spacing")?,
            });
        }
        if let Some(a) = opt_str(o, "align")? {
            p.align = Some(align_of(a)?);
        }
        if let Some(v) = get(o, "space_before") {
            p.before = Some((points(v, "space_before")? * TW_PT).round() as i64);
        }
        if let Some(v) = get(o, "space_after") {
            p.after = Some((points(v, "space_after")? * TW_PT).round() as i64);
        }
        if let Some(v) = get(o, "line_spacing") {
            let m = v.as_f64().filter(|m| (0.5..=5.0).contains(m)).context(
                "`line_spacing` must be a multiple of single spacing from 0.5 to 5 (1.15)",
            )?;
            p.line = Some((m * 240.0).round() as i64);
        }
        if let Some(v) = get(o, "indent") {
            p.indent = Some(twips(v, "indent")?);
        }
        if let Some(v) = get(o, "indent_right") {
            p.indent_right = Some(twips(v, "indent_right")?);
        }
        if let Some(v) = get(o, "first_line") {
            p.first_line = Some(twips(v, "first_line")?);
        }
        p.keep_next = opt_bool(o, "keep_with_next")?;
        p.keep_lines = opt_bool(o, "keep_together")?;
        p.page_break_before = opt_bool(o, "page_break_before")?;
        if let Some(v) = get(o, "outline_level") {
            let l = v
                .as_u64()
                .filter(|l| (1..=9).contains(l))
                .context("`outline_level` must be 1 to 9")?;
            p.outline = Some(l as u8 - 1);
        }
        Ok((r, p))
    }

    /// The look from a merged look JSON (a preset with the document's
    /// settings over it). `lang` is the document's language.
    pub fn read(v: &Value, lang: &str) -> Result<Self> {
        let o = v.as_object().context("the look must be an object")?;
        let mut fonts = BTreeMap::new();
        if let Some(f) = get(o, "fonts") {
            let fo = object(f, "`fonts`", &["heading", "body", "mono"])?;
            for (k, v) in fo {
                let name = v
                    .as_str()
                    .filter(|s| !s.trim().is_empty())
                    .with_context(|| format!("`fonts.{k}` must be a font name"))?;
                fonts.insert(k.clone(), name.trim().to_owned());
            }
        }
        for (k, d) in [
            ("heading", "Calibri"),
            ("body", "Calibri"),
            ("mono", "Consolas"),
        ] {
            fonts.entry(k.to_owned()).or_insert_with(|| d.to_owned());
        }
        let mut palette = BTreeMap::new();
        let given = get(o, "palette")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        // Hex values first, then names that point at other entries.
        for _ in 0..3 {
            for (k, v) in &given {
                let s = v
                    .as_str()
                    .with_context(|| format!("`palette.{k}` must be a colour"))?;
                if let Some(h) = hex_color(s) {
                    palette.insert(k.clone(), h);
                } else if let Some(h) = palette.get(s.trim()).cloned() {
                    palette.insert(k.clone(), h);
                }
            }
        }
        for (k, v) in &given {
            if !palette.contains_key(k) {
                bail!(
                    "`palette.{k}`: \"{}\" is not a colour (#RRGGBB or another palette name)",
                    v.as_str().unwrap_or("")
                );
            }
        }
        for (k, d) in [
            ("accent", "2F4B7C"),
            ("ink", "1F2328"),
            ("muted", "5C6672"),
            ("rule", "C8CED6"),
            ("tint", "EDF1F6"),
        ] {
            palette.entry(k.to_owned()).or_insert_with(|| d.to_owned());
        }
        let accent = palette["accent"].clone();
        let ink = palette["ink"].clone();
        palette
            .entry("bullet".into())
            .or_insert_with(|| ink.clone());
        palette
            .entry("link".into())
            .or_insert_with(|| accent.clone());
        for (k, t) in [("accent2", 0.45), ("accent5", 0.7)] {
            palette
                .entry(k.into())
                .or_insert_with(|| mix(&accent, "FFFFFF", t));
        }
        palette
            .entry("accent3".into())
            .or_insert_with(|| "C27A3A".into());
        palette
            .entry("accent4".into())
            .or_insert_with(|| "5C6672".into());
        palette
            .entry("accent6".into())
            .or_insert_with(|| "6B8A5A".into());
        palette.insert("white".into(), "FFFFFF".into());
        palette.insert("black".into(), "000000".into());

        let mut look = Look {
            fonts,
            palette,
            styles: Vec::new(),
            numbered: opt_bool(o, "numbered_headings")?.unwrap_or(true),
            first_page_header: opt_bool(o, "first_page_header")?.unwrap_or(true),
            table: TableLook {
                look: "rules".into(),
                header_fill: None,
                rule: String::new(),
                edge: String::new(),
                banded: None,
            },
            callout: CalloutLook {
                fill: None,
                bar: None,
            },
            cover: CoverLook {
                kind: "plain".into(),
                color: String::new(),
            },
            header: None,
            footer: None,
            page: A4,
            labels: Labels {
                figure: "Figure".into(),
                table: "Table".into(),
                contents: "Contents".into(),
                figures: "Figures".into(),
                tables: "Tables".into(),
            },
            lang: lang.to_owned(),
        };
        if lang.to_ascii_lowercase().starts_with("id") {
            look.labels = Labels {
                figure: "Gambar".into(),
                table: "Tabel".into(),
                contents: "Daftar Isi".into(),
                figures: "Daftar Gambar".into(),
                tables: "Daftar Tabel".into(),
            };
        }
        look.table.rule = look.palette["rule"].clone();
        look.table.edge = look.palette["ink"].clone();

        if let Some(t) = get(o, "table") {
            let to = object(
                t,
                "`table`",
                &["look", "header_fill", "rule", "edge", "banded"],
            )?;
            if let Some(l) = opt_str(to, "look")? {
                if !["rules", "grid", "header", "plain"].contains(&l) {
                    bail!("`table.look` must be rules, grid, header or plain");
                }
                look.table.look = l.to_owned();
            }
            look.table.header_fill = opt_str(to, "header_fill")?
                .map(|c| look.color(c))
                .transpose()?;
            if let Some(c) = opt_str(to, "rule")? {
                look.table.rule = look.color(c)?;
            }
            if let Some(c) = opt_str(to, "edge")? {
                look.table.edge = look.color(c)?;
            }
            look.table.banded = opt_str(to, "banded")?.map(|c| look.color(c)).transpose()?;
        }
        if let Some(c) = get(o, "callout") {
            let co = object(c, "`callout`", &["fill", "bar"])?;
            look.callout.fill = opt_str(co, "fill")?.map(|c| look.color(c)).transpose()?;
            look.callout.bar = opt_str(co, "bar")?.map(|c| look.color(c)).transpose()?;
        }
        if let Some(c) = get(o, "cover_style") {
            let co = object(c, "`cover_style`", &["kind", "color"])?;
            if let Some(k) = opt_str(co, "kind")? {
                if !["bar", "band", "plain", "none"].contains(&k) {
                    bail!("`cover_style.kind` must be bar, band, plain or none");
                }
                look.cover.kind = k.to_owned();
            }
            look.cover.color = look.color(opt_str(co, "color")?.unwrap_or("accent"))?;
        } else {
            look.cover.color = look.palette["accent"].clone();
        }
        look.header = hf_spec(o.get("header"), "header")?;
        look.footer = hf_spec(o.get("footer"), "footer")?;
        if let Some(p) = get(o, "page") {
            look.page = page_spec(p, A4)?;
        }
        if let Some(l) = get(o, "labels") {
            let lo = object(
                l,
                "`labels`",
                &["figure", "table", "contents", "figures", "tables"],
            )?;
            for (k, slot) in [
                ("figure", &mut look.labels.figure),
                ("table", &mut look.labels.table),
                ("contents", &mut look.labels.contents),
                ("figures", &mut look.labels.figures),
                ("tables", &mut look.labels.tables),
            ] {
                if let Some(s) = opt_str(lo, k)? {
                    *slot = s.to_owned();
                }
            }
        }

        // Styles: every built-in, then the document's own.
        let defs = get(o, "styles")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let mut user: Vec<(String, Value)> = Vec::new();
        for (name, v) in &defs {
            if builtin(name).is_none() {
                user.push((name.clone(), v.clone()));
            }
        }
        for b in BUILTIN {
            let (jname, id, wname, kind, based, next) = *b;
            let def = defs
                .iter()
                .find(|(n, _)| builtin(n).map(|x| x.1) == Some(id))
                .map(|(_, v)| v.clone());
            let mut s = StyleDef {
                id: id.into(),
                name: wname.into(),
                kind,
                based_on: based.map(str::to_owned),
                next: next.map(str::to_owned),
                run: RunFmt::default(),
                para: ParaFmt::default(),
            };
            if let Some(d) = &def {
                let dobj = object(d, &format!("style \"{jname}\""), STYLE_KEYS)?;
                let (r, p) = look.formats(dobj)?;
                s.run = r;
                s.para = p;
            }
            look.styles.push(s);
        }
        for (name, v) in user {
            let dobj = object(&v, &format!("style \"{name}\""), STYLE_KEYS)?;
            let kind = match opt_str(dobj, "type")?.unwrap_or("paragraph") {
                "paragraph" => Kind::Para,
                "character" => Kind::Char,
                other => bail!(
                    "style \"{name}\": `type` must be paragraph or character, not \"{other}\""
                ),
            };
            let id = custom_id(&name);
            if look.styles.iter().any(|s| s.id == id) {
                bail!("style \"{name}\" has the same id ({id}) as another style");
            }
            let (r, p) = look.formats(dobj)?;
            look.styles.push(StyleDef {
                id,
                name: name.clone(),
                kind,
                based_on: Some(
                    if kind == Kind::Para {
                        "Normal"
                    } else {
                        "DefaultParagraphFont"
                    }
                    .into(),
                ),
                next: None,
                run: r,
                para: p,
            });
        }
        // based_on / next of the document's styles, by name.
        for (name, v) in &defs {
            let Some(dobj) = v.as_object() else { continue };
            let id = builtin(name).map_or_else(|| custom_id(name), |b| b.1.to_owned());
            let based = opt_str(dobj, "based_on")?
                .map(|b| look.style_id(b))
                .transpose()?;
            let next = opt_str(dobj, "next")?
                .map(|b| look.style_id(b))
                .transpose()?;
            if let Some(s) = look.styles.iter_mut().find(|s| s.id == id) {
                if based.is_some() {
                    s.based_on = based;
                }
                if next.is_some() {
                    s.next = next;
                }
            }
        }
        look.fix_styles();
        Ok(look)
    }

    /// What every look needs whatever the JSON says: heading outline
    /// levels and keep-with-next, list and TOC tabs, the hyperlink and
    /// footnote reference looks.
    fn fix_styles(&mut self) {
        let width = self.page.text_width();
        let link = self.palette["link"].clone();
        let numbered = self.numbered;
        for s in &mut self.styles {
            match s.id.as_str() {
                "Heading1" | "Heading2" | "Heading3" | "Heading4" => {
                    let level = s.id[7..].parse::<u8>().unwrap_or(1);
                    s.para.outline.get_or_insert(level - 1);
                    s.para.keep_next.get_or_insert(true);
                    s.para.keep_lines.get_or_insert(true);
                    if numbered && level <= 3 {
                        s.para.num = Some((NUM_HEADINGS, level - 1));
                        s.para.indent.get_or_insert(HEADING_HANG);
                        s.para.first_line.get_or_insert(-HEADING_HANG);
                    }
                }
                "Title" | "TOCHeading" | "CoverLabel" => {
                    s.para.keep_next.get_or_insert(true);
                }
                "Caption" => {}
                "TOC1" | "TOC2" | "TOC3" | "TableofFigures" => {
                    // A column for the heading's number, then its text
                    // (wrapped lines line up with it), dots to the page.
                    let base = s.para.indent.unwrap_or(0).max(0);
                    let num = match s.id.as_str() {
                        "TOC1" => 440,
                        "TOC2" => 620,
                        "TOC3" => 800,
                        _ => 0,
                    };
                    let mut tabs = Vec::new();
                    if num > 0 {
                        s.para.indent = Some(base + num);
                        s.para.first_line = Some(-num);
                        tabs.push(format!("<w:tab w:val=\"left\" w:pos=\"{}\"/>", base + num));
                    }
                    tabs.push(format!(
                        "<w:tab w:val=\"right\" w:leader=\"dot\" w:pos=\"{}\"/>",
                        width.max(base + num + 720)
                    ));
                    s.para.tabs = tabs;
                }
                "Header" | "Footer" => {
                    s.para.tabs = vec![
                        format!("<w:tab w:val=\"center\" w:pos=\"{}\"/>", width / 2),
                        format!("<w:tab w:val=\"right\" w:pos=\"{width}\"/>"),
                    ];
                }
                "ListBullet" | "ListBullet2" | "ListBullet3" | "ListNumber" | "ListNumber2"
                | "ListNumber3" => {
                    s.para.contextual.get_or_insert(true);
                }
                "Hyperlink" => {
                    s.run.color.get_or_insert(link.clone());
                    s.run.underline.get_or_insert(true);
                }
                "FootnoteReference" => {
                    s.run.vert.get_or_insert("superscript");
                }
                "Strong" => {
                    s.run.bold.get_or_insert(true);
                }
                "Emphasis" => {
                    s.run.italic.get_or_insert(true);
                }
                "Code" => {
                    let mono = self.fonts["mono"].clone();
                    s.run.font.get_or_insert(mono);
                }
                _ => {}
            }
        }
        // A plain look (no preset): Word-like headings.
    }

    /// Fonts the document names (for `fontTable.xml`).
    pub fn font_names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.fonts.values().cloned().collect();
        for s in &self.styles {
            if let Some(f) = &s.run.font {
                v.push(f.clone());
            }
        }
        v.sort();
        v.dedup();
        v
    }
}

/// Numbering ids written by every build: headings, bullets, then one
/// `w:num` per numbered list from [`NUM_LISTS`].
pub const NUM_HEADINGS: u32 = 1;
pub const NUM_BULLETS: u32 = 2;
pub const NUM_LISTS: u32 = 3;
const ABS_HEADINGS: u32 = 1;
const ABS_BULLETS: u32 = 2;
const ABS_NUMBERS: u32 = 3;
/// Hanging indent of numbered headings (twips).
pub const HEADING_HANG: i64 = 720;
/// Indent per list level (twips).
pub const LIST_INDENT: i64 = 360;

/// `numbering.xml`: heading numbers, bullets, numbered lists (`lists`
/// `w:num`s from [`NUM_LISTS`], each restarting at `start`).
pub fn numbering_xml(look: &Look, lists: &[u32]) -> String {
    let bullet_color = &look.palette["bullet"];
    let mut x = format!(
        "{XML_DECL}<w:numbering xmlns:w=\"{}\">",
        super::docs_pkg::NS_W
    );
    x.push_str(&abstract_headings(look));
    x.push_str(&abstract_list(ABS_BULLETS, true, bullet_color));
    x.push_str(&abstract_list(ABS_NUMBERS, false, &look.palette["ink"]));
    x.push_str(&format!(
        "<w:num w:numId=\"{NUM_HEADINGS}\"><w:abstractNumId w:val=\"{ABS_HEADINGS}\"/></w:num>\
         <w:num w:numId=\"{NUM_BULLETS}\"><w:abstractNumId w:val=\"{ABS_BULLETS}\"/></w:num>"
    ));
    for (k, start) in lists.iter().enumerate() {
        x.push_str(&num_xml(NUM_LISTS + k as u32, ABS_NUMBERS, *start));
    }
    x.push_str("</w:numbering>");
    x
}

/// A `w:num` of abstract `abs` restarting at `start`.
pub fn num_xml(id: u32, abs: u32, start: u32) -> String {
    format!(
        "<w:num w:numId=\"{id}\"><w:abstractNumId w:val=\"{abs}\"/>\
         <w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"{start}\"/></w:lvlOverride></w:num>"
    )
}

fn abstract_headings(look: &Look) -> String {
    let color = look.palette["accent"].clone();
    let mut x = format!(
        "<w:abstractNum w:abstractNumId=\"{ABS_HEADINGS}\"><w:multiLevelType w:val=\"multilevel\"/>"
    );
    for l in 0..9u8 {
        let text: String = (1..=l + 1)
            .map(|k| format!("%{k}"))
            .collect::<Vec<_>>()
            .join(".");
        let style = if l < 3 {
            format!("<w:pStyle w:val=\"Heading{}\"/>", l + 1)
        } else {
            String::new()
        };
        let rpr = if l == 0 {
            format!("<w:rPr><w:color w:val=\"{color}\"/></w:rPr>")
        } else {
            String::new()
        };
        x.push_str(&format!(
            "<w:lvl w:ilvl=\"{l}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"decimal\"/>{style}\
             <w:suff w:val=\"tab\"/><w:lvlText w:val=\"{text}\"/><w:lvlJc w:val=\"left\"/>\
             <w:pPr><w:ind w:left=\"{HEADING_HANG}\" w:hanging=\"{HEADING_HANG}\"/></w:pPr>{rpr}</w:lvl>"
        ));
    }
    x.push_str("</w:abstractNum>");
    x
}

/// Bullets (`•`, `–`, `▪`) or numbers (`1.`, `a.`, `i.`) on nine levels.
pub fn abstract_list(id: u32, bullets: bool, color: &str) -> String {
    let mut x = format!(
        "<w:abstractNum w:abstractNumId=\"{id}\"><w:multiLevelType w:val=\"hybridMultilevel\"/>"
    );
    for l in 0..9u8 {
        let (fmt, text) = if bullets {
            (
                "bullet",
                ["\u{2022}", "\u{2013}", "\u{25AA}"][l as usize % 3].to_owned(),
            )
        } else {
            (
                ["decimal", "lowerLetter", "lowerRoman"][l as usize % 3],
                format!("%{}.", l + 1),
            )
        };
        let left = LIST_INDENT * (l as i64 + 1);
        let rpr = if bullets {
            format!("<w:rPr><w:color w:val=\"{color}\"/></w:rPr>")
        } else {
            String::new()
        };
        x.push_str(&format!(
            "<w:lvl w:ilvl=\"{l}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{fmt}\"/>\
             <w:lvlText w:val=\"{text}\"/><w:lvlJc w:val=\"left\"/>\
             <w:pPr><w:ind w:left=\"{left}\" w:hanging=\"{LIST_INDENT}\"/></w:pPr>{rpr}</w:lvl>"
        ));
    }
    x.push_str("</w:abstractNum>");
    x
}

fn style_xml(s: &StyleDef, default: bool) -> String {
    let ty = match s.kind {
        Kind::Para => "paragraph",
        Kind::Char => "character",
    };
    let mut x = format!(
        "<w:style w:type=\"{ty}\"{} w:styleId=\"{}\"><w:name w:val=\"{}\"/>",
        if default { " w:default=\"1\"" } else { "" },
        esc(&s.id),
        esc(&s.name)
    );
    if let Some(b) = &s.based_on {
        x.push_str(&format!("<w:basedOn w:val=\"{}\"/>", esc(b)));
    }
    if let Some(n) = &s.next {
        x.push_str(&format!("<w:next w:val=\"{}\"/>", esc(n)));
    }
    if s.id == "DefaultParagraphFont" {
        x.push_str("<w:uiPriority w:val=\"1\"/><w:semiHidden/><w:unhideWhenUsed/>");
    } else {
        x.push_str("<w:qFormat/>");
    }
    if s.kind == Kind::Para {
        let p = s.para.children(None);
        if !p.is_empty() {
            x.push_str(&format!("<w:pPr>{p}</w:pPr>"));
        }
    }
    x.push_str(&s.run.xml());
    x.push_str("</w:style>");
    x
}

/// A border element (`w:top`…): `size` in eighths of a point.
pub fn border(side: &str, size: u32, color: &str) -> String {
    format!("<w:{side} w:val=\"single\" w:sz=\"{size}\" w:space=\"0\" w:color=\"{color}\"/>")
}

/// The table style `DocumentTable` for the look.
fn table_style(look: &Look) -> String {
    let t = &look.table;
    let borders = match t.look.as_str() {
        "grid" => ["top", "left", "bottom", "right", "insideH", "insideV"]
            .iter()
            .map(|s| border(s, 4, &t.rule))
            .collect::<String>(),
        "plain" => String::new(),
        _ => format!(
            "{}{}{}",
            border("top", 8, &t.edge),
            border("bottom", 8, &t.edge),
            border("insideH", 4, &t.rule)
        ),
    };
    let fill = t
        .header_fill
        .as_deref()
        .map(|f| format!("<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{f}\"/>"))
        .unwrap_or_default();
    format!(
        "<w:style w:type=\"table\" w:styleId=\"DocumentTable\"><w:name w:val=\"Document Table\"/>\
         <w:basedOn w:val=\"TableNormal\"/><w:uiPriority w:val=\"59\"/><w:qFormat/>\
         <w:pPr><w:spacing w:after=\"0\"/></w:pPr>\
         <w:tblPr><w:tblBorders>{borders}</w:tblBorders>\
         <w:tblCellMar><w:top w:w=\"60\" w:type=\"dxa\"/><w:left w:w=\"100\" w:type=\"dxa\"/>\
         <w:bottom w:w=\"60\" w:type=\"dxa\"/><w:right w:w=\"100\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr>\
         <w:tblStylePr w:type=\"firstRow\"><w:rPr><w:b/><w:bCs/></w:rPr><w:tblPr/>\
         <w:tcPr><w:tcBorders>{}</w:tcBorders>{fill}</w:tcPr></w:tblStylePr></w:style>",
        border("bottom", 8, &t.edge)
    )
}

/// `styles.xml` for the look.
pub fn styles_xml(look: &Look) -> String {
    let body = look.font("body");
    let normal = look.style("Normal");
    let size = normal.and_then(|s| s.run.size).unwrap_or(21);
    let lang = esc(&look.lang);
    let mut x = format!(
        "{XML_DECL}<w:styles xmlns:w=\"{}\" xmlns:r=\"{}\"><w:docDefaults><w:rPrDefault><w:rPr>\
         <w:rFonts w:ascii=\"{b}\" w:hAnsi=\"{b}\" w:eastAsia=\"{b}\" w:cs=\"{b}\"/>\
         <w:sz w:val=\"{size}\"/><w:szCs w:val=\"{size}\"/><w:lang w:val=\"{lang}\" w:eastAsia=\"en-US\" w:bidi=\"ar-SA\"/>\
         </w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after=\"120\" w:line=\"264\" w:lineRule=\"auto\"/></w:pPr></w:pPrDefault></w:docDefaults>",
        super::docs_pkg::NS_W,
        super::docs_pkg::NS_R,
        b = esc(&body),
    );
    for s in &look.styles {
        let default = s.id == "Normal" || s.id == "DefaultParagraphFont";
        x.push_str(&style_xml(s, default));
    }
    x.push_str(
        "<w:style w:type=\"table\" w:default=\"1\" w:styleId=\"TableNormal\"><w:name w:val=\"Normal Table\"/>\
         <w:uiPriority w:val=\"99\"/><w:semiHidden/><w:unhideWhenUsed/><w:tblPr><w:tblInd w:w=\"0\" w:type=\"dxa\"/>\
         <w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"108\" w:type=\"dxa\"/>\
         <w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"108\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr></w:style>",
    );
    x.push_str(&table_style(look));
    x.push_str("</w:styles>");
    x
}

/// The theme part: the look's colours (charts' series take accent 1–6)
/// and fonts.
pub fn theme_xml(look: &Look) -> String {
    let p = &look.palette;
    let clr = |name: &str, v: &str| format!("<a:{name}><a:srgbClr val=\"{v}\"/></a:{name}>");
    let scheme = [
        "<a:dk1><a:sysClr val=\"windowText\" lastClr=\"000000\"/></a:dk1>".to_owned(),
        "<a:lt1><a:sysClr val=\"window\" lastClr=\"FFFFFF\"/></a:lt1>".to_owned(),
        clr("dk2", &p["ink"]),
        clr("lt2", &p["tint"]),
        clr("accent1", &p["accent"]),
        clr("accent2", &p["accent2"]),
        clr("accent3", &p["accent3"]),
        clr("accent4", &p["accent4"]),
        clr("accent5", &p["accent5"]),
        clr("accent6", &p["accent6"]),
        clr("hlink", &p["link"]),
        clr("folHlink", &p["muted"]),
    ]
    .concat();
    let (major, minor) = (esc(&look.font("heading")), esc(&look.font("body")));
    let fill = "<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>";
    let line = |w: u32| {
        format!(
            "<a:ln w=\"{w}\" cap=\"flat\" cmpd=\"sng\" algn=\"ctr\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>\
             <a:prstDash val=\"solid\"/><a:miter lim=\"800000\"/></a:ln>"
        )
    };
    let effect = "<a:effectStyle><a:effectLst/></a:effectStyle>";
    format!(
        "{XML_DECL}<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"Document\"><a:themeElements>\
         <a:clrScheme name=\"Document\">{scheme}</a:clrScheme>\
         <a:fontScheme name=\"Document\"><a:majorFont><a:latin typeface=\"{major}\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:majorFont>\
         <a:minorFont><a:latin typeface=\"{minor}\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:minorFont></a:fontScheme>\
         <a:fmtScheme name=\"Office\"><a:fillStyleLst>{fill}{fill}{fill}</a:fillStyleLst>\
         <a:lnStyleLst>{}{}{}</a:lnStyleLst><a:effectStyleLst>{effect}{effect}{effect}</a:effectStyleLst>\
         <a:bgFillStyleLst>{fill}{fill}{fill}</a:bgFillStyleLst></a:fmtScheme></a:themeElements>\
         <a:objectDefaults/><a:extraClrSchemeLst/></a:theme>",
        line(6350),
        line(12700),
        line(19050),
    )
}

/// `fontTable.xml` naming the look's fonts.
pub fn font_table_xml(look: &Look) -> String {
    let mut x = format!("{XML_DECL}<w:fonts xmlns:w=\"{}\">", super::docs_pkg::NS_W);
    for f in look.font_names() {
        let l = f.to_lowercase();
        let (family, pitch) = if ["consolas", "courier", "mono", "code"]
            .iter()
            .any(|m| l.contains(m))
        {
            ("modern", "fixed")
        } else if [
            "cambria", "georgia", "times", "garamond", "serif", "book", "palatino", "caladea",
        ]
        .iter()
        .any(|m| l.contains(m))
            && !l.contains("sans")
        {
            ("roman", "variable")
        } else {
            ("swiss", "variable")
        };
        x.push_str(&format!(
            "<w:font w:name=\"{}\"><w:charset w:val=\"00\"/><w:family w:val=\"{family}\"/><w:pitch w:val=\"{pitch}\"/></w:font>",
            esc(&f)
        ));
    }
    x.push_str("</w:fonts>");
    x
}

/// `settings.xml`.
pub fn settings_xml(look: &Look, footnotes: bool, even_odd: bool) -> String {
    let notes = if footnotes {
        "<w:footnotePr><w:footnote w:id=\"-1\"/><w:footnote w:id=\"0\"/></w:footnotePr>"
    } else {
        ""
    };
    let decimal = if look.lang.to_ascii_lowercase().starts_with("id") {
        "<w:decimalSymbol w:val=\",\"/><w:listSeparator w:val=\";\"/>"
    } else {
        "<w:decimalSymbol w:val=\".\"/><w:listSeparator w:val=\",\"/>"
    };
    let cs = |name: &str, v: &str| {
        format!(
            "<w:compatSetting w:name=\"{name}\" w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"{v}\"/>"
        )
    };
    format!(
        "{XML_DECL}<w:settings xmlns:w=\"{}\" xmlns:r=\"{}\"><w:zoom w:percent=\"100\"/>\
         <w:defaultTabStop w:val=\"720\"/>{}<w:characterSpacingControl w:val=\"doNotCompress\"/>{notes}\
         <w:compat>{}{}{}{}{}</w:compat><w:themeFontLang w:val=\"{}\"/>\
         <w:clrSchemeMapping w:bg1=\"light1\" w:t1=\"dark1\" w:bg2=\"light2\" w:t2=\"dark2\" w:accent1=\"accent1\" \
         w:accent2=\"accent2\" w:accent3=\"accent3\" w:accent4=\"accent4\" w:accent5=\"accent5\" w:accent6=\"accent6\" \
         w:hyperlink=\"hyperlink\" w:followedHyperlink=\"followedHyperlink\"/>{decimal}</w:settings>",
        super::docs_pkg::NS_W,
        super::docs_pkg::NS_R,
        if even_odd {
            "<w:evenAndOddHeaders/>"
        } else {
            ""
        },
        cs("compatibilityMode", "15"),
        cs("overrideTableStyleFontSizeAndJustification", "1"),
        cs("enableOpenTypeFeatures", "1"),
        cs("doNotFlipMirrorIndents", "1"),
        cs("differentiateMultirowTableHeaders", "1"),
        esc(&look.lang),
    )
}

/// The merged look JSON of a document (`preset` and its own look keys).
pub fn look_json(doc: &Map<String, Value>) -> Result<Value> {
    let mut v = match get(doc, "preset") {
        None => preset("report")?,
        Some(Value::String(p)) => preset(p)?,
        Some(_) => bail!("`preset` must be one of {}", PRESETS.join(", ")),
    };
    let mut over = Map::new();
    for k in LOOK_KEYS {
        if let Some(x) = doc.get(*k) {
            over.insert((*k).to_owned(), x.clone());
        }
    }
    // `accent` and `fonts` shortcuts for a client's brand.
    if let Some(a) = get(doc, "accent") {
        let s = a.as_str().context("`accent` must be a colour (#RRGGBB)")?;
        hex_color(s).with_context(|| format!("`accent` \"{s}\" is not a colour (#RRGGBB)"))?;
        let pal = over
            .entry("palette".to_owned())
            .or_insert_with(|| json!({}));
        if let Some(p) = pal.as_object_mut() {
            p.entry("accent".to_owned()).or_insert(a.clone());
            // The preset's derived colours follow the new accent.
            for k in ["accent2", "accent5"] {
                if !p.contains_key(k) {
                    let derived = mix(
                        &hex_color(s).unwrap_or_default(),
                        "FFFFFF",
                        if k == "accent2" { 0.45 } else { 0.7 },
                    );
                    p.insert(k.into(), json!(format!("#{derived}")));
                }
            }
            if !p.contains_key("tint") {
                p.insert(
                    "tint".into(),
                    json!(format!(
                        "#{}",
                        mix(&hex_color(s).unwrap_or_default(), "FFFFFF", 0.9)
                    )),
                );
            }
        }
    }
    // Built-in style names in any case or spacing ("heading 2") go over
    // the preset's ("Heading 2").
    if let Some(Value::Object(st)) = over.get_mut("styles") {
        let named: Map<String, Value> = std::mem::take(st)
            .into_iter()
            .map(|(k, v)| (builtin(&k).map_or(k, |b| b.0.to_owned()), v))
            .collect();
        *st = named;
    }
    super::slides_style::merge(&mut v, &Value::Object(over));
    // `null` removes a preset's header or footer.
    if let Some(o) = v.as_object_mut() {
        for k in ["header", "footer"] {
            if doc.get(k).is_some_and(Value::is_null) {
                o.insert(k.into(), Value::Null);
            }
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn presets_read_and_write_valid_parts() {
        for p in PRESETS {
            let look = Look::read(&preset(p).unwrap(), "en-US").unwrap();
            let styles = styles_xml(&look);
            assert!(
                docs_engine::xml::parse(&styles[XML_DECL.len()..]).is_some(),
                "{p}"
            );
            for id in [
                "Normal",
                "Heading1",
                "Caption",
                "TOC1",
                "ListBullet",
                "Hyperlink",
            ] {
                assert!(styles.contains(&format!("w:styleId=\"{id}\"")), "{p}: {id}");
            }
            assert!(styles.contains("<w:name w:val=\"heading 1\"/>"));
            let num = numbering_xml(&look, &[1, 4]);
            assert!(docs_engine::xml::parse(&num[XML_DECL.len()..]).is_some());
            assert_eq!(num.matches("<w:num ").count(), 4);
            assert!(theme_xml(&look).contains(&look.palette["accent"]));
            assert!(settings_xml(&look, true, false).contains("footnotePr"));
        }
        let report = Look::read(&preset("report").unwrap(), "en-US").unwrap();
        let h1 = report.style("Heading1").unwrap();
        assert_eq!(h1.para.num, Some((NUM_HEADINGS, 0)));
        assert_eq!(h1.para.outline, Some(0));
        let letter = Look::read(&preset("letter").unwrap(), "id-ID").unwrap();
        assert_eq!(letter.style("Heading1").unwrap().para.num, None);
        assert!(letter.header.is_none());
        assert_eq!(letter.labels.figure, "Gambar");
        assert!(preset("glossy").is_err());
    }

    #[std::prelude::v1::test]
    fn documents_override_presets_and_add_styles() {
        let doc = json!({
            "preset": "proposal",
            "accent": "#7A1F3D",
            "fonts": {"body": "Georgia"},
            "styles": {
                "heading 2": {"color": "#112233", "size": 15},
                "Lead": {"size": 13, "color": "muted", "space_after": 12},
                "Key": {"type": "character", "bold": true, "color": "accent"}
            },
            "page": {"size": "Letter", "orientation": "landscape", "margins": "2cm"},
            "header": null
        });
        let v = look_json(doc.as_object().unwrap()).unwrap();
        let look = Look::read(&v, "en-US").unwrap();
        assert_eq!(look.palette["accent"], "7A1F3D");
        assert_eq!(look.palette["tint"], mix("7A1F3D", "FFFFFF", 0.9));
        assert_eq!(look.font("body"), "Georgia");
        let h2 = look.style("Heading2").unwrap();
        assert_eq!(h2.run.color.as_deref(), Some("112233"));
        assert_eq!(h2.run.size, Some(30));
        assert!(h2.run.bold == Some(true), "the preset's bold stays");
        assert_eq!(look.style_id("Lead").unwrap(), "Lead");
        assert_eq!(look.style("Key").unwrap().kind, Kind::Char);
        assert_eq!(
            look.style("Key").unwrap().run.color.as_deref(),
            Some("7A1F3D")
        );
        assert!(look.page.landscape && look.page.width == 15840 && look.page.left == 1134);
        assert!(look.header.is_none());
        assert!(look.style_id("Nope").is_err());
        let bad = json!({"styles": {"Normal": {"colour": "red"}}});
        let v = look_json(bad.as_object().unwrap()).unwrap();
        assert!(
            Look::read(&v, "en-US")
                .unwrap_err()
                .to_string()
                .contains("unknown key")
        );
    }
}
