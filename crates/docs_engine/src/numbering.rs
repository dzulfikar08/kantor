//! Lists: `word/numbering.xml` (`w:abstractNum` level definitions and the
//! `w:num` instances paragraphs point at) and the marker text of every
//! list paragraph.
//!
//! Round trip: the original part is written back unchanged unless Kantor
//! added definitions (a new bulleted or numbered list); then the new
//! `w:abstractNum`s are inserted before the first `w:num` and the new
//! `w:num`s appended, leaving every original byte in place.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::model::{Indent, NumRef};
use crate::xml::{self, El};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum NumFmt {
    #[default]
    Decimal,
    DecimalZero,
    LowerLetter,
    UpperLetter,
    LowerRoman,
    UpperRoman,
    Bullet,
    None,
}

impl NumFmt {
    fn parse(v: &str) -> Self {
        match v {
            "decimalZero" => Self::DecimalZero,
            "lowerLetter" => Self::LowerLetter,
            "upperLetter" => Self::UpperLetter,
            "lowerRoman" => Self::LowerRoman,
            "upperRoman" => Self::UpperRoman,
            "bullet" => Self::Bullet,
            "none" => Self::None,
            // Other formats (ordinal, Chinese counting…) are shown as decimal;
            // numbering.xml keeps the real one.
            _ => Self::Decimal,
        }
    }

    fn format(self, n: u32) -> String {
        match self {
            Self::Decimal | Self::Bullet => n.to_string(),
            Self::DecimalZero => format!("{n:02}"),
            Self::LowerLetter => letters(n),
            Self::UpperLetter => letters(n).to_uppercase(),
            Self::LowerRoman => roman(n),
            Self::UpperRoman => roman(n).to_uppercase(),
            Self::None => String::new(),
        }
    }
}

/// a, b, … z, aa, bb, … (Word's scheme).
fn letters(n: u32) -> String {
    if n == 0 {
        return String::new();
    }
    let ch = (b'a' + ((n - 1) % 26) as u8) as char;
    std::iter::repeat_n(ch, ((n - 1) / 26 + 1) as usize).collect()
}

fn roman(mut n: u32) -> String {
    const T: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

/// Symbol-font bullets (private-use code points) as Unicode.
fn map_bullet(text: &str, font: Option<&str>) -> String {
    let symbolic = font.is_some_and(|f| {
        let f = f.to_ascii_lowercase();
        f.contains("symbol") || f.contains("wingdings")
    });
    text.chars()
        .map(|c| match c as u32 {
            0xF0B7 | 0x00B7 if symbolic => '•',
            0xF0A7 => '▪',
            0xF0D8 => '➢',
            0xF076 => '❖',
            0xF0FC => '✓',
            0xF0A8 => '◆',
            0xF06E => '■',
            0xF0E0 => '➔',
            0xF000..=0xF0FF => '•',
            _ if c == 'o' && font.is_some_and(|f| f.eq_ignore_ascii_case("Courier New")) => '◦',
            _ => c,
        })
        .collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LevelDef {
    pub start: u32,
    pub fmt: NumFmt,
    /// `w:lvlText`, e.g. `%1.` or a bullet character.
    pub text: String,
    pub indent: Indent,
    /// Marker font (`w:rPr/w:rFonts/@w:ascii`), e.g. `Symbol`.
    pub font: Option<String>,
}

impl LevelDef {
    fn parse(el: &El) -> Self {
        let mut d = LevelDef {
            start: 1,
            ..Self::default()
        };
        for c in &el.children {
            match c.name.as_str() {
                "start" => d.start = c.num("val").unwrap_or(1),
                "numFmt" => d.fmt = NumFmt::parse(c.attr("val").unwrap_or("decimal")),
                "lvlText" => d.text = c.attr("val").unwrap_or_default().to_owned(),
                "pPr" => {
                    if let Some(ind) = c.child("ind") {
                        d.indent = parse_ind(ind);
                    }
                }
                "rPr" => {
                    d.font = c
                        .child("rFonts")
                        .and_then(|f| f.attr("ascii").or(f.attr("hAnsi")))
                        .map(str::to_owned);
                }
                _ => {}
            }
        }
        d
    }
}

/// `w:ind` attributes (both the 2006 and the 2010 names).
pub fn parse_ind(el: &El) -> Indent {
    let n = |k: &str| el.num::<i32>(k);
    Indent {
        left: n("left").or(n("start")),
        right: n("right").or(n("end")),
        first: n("firstLine").or(n("hanging").map(|h| -h)),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AbstractNum {
    pub levels: BTreeMap<u8, LevelDef>,
    /// `w:numStyleLink`: the levels come from a numbering style.
    pub style_link: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Num {
    pub abstract_id: u32,
    /// `w:lvlOverride`: a new start value and/or a replaced level.
    pub start_overrides: BTreeMap<u8, u32>,
    pub level_overrides: BTreeMap<u8, LevelDef>,
}

/// What kind of list a paragraph is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListKind {
    Bullet,
    Numbered,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Numbering {
    pub abstracts: BTreeMap<u32, AbstractNum>,
    pub nums: BTreeMap<u32, Num>,
    /// Definitions Kantor added, as XML to write.
    pub added_abstracts: Vec<String>,
    pub added_nums: Vec<String>,
    /// The `w:num` ids Kantor created for new lists, by kind (reused by
    /// later lists of the same kind when continuing makes sense).
    pub kantor_bullet: Option<u32>,
    pub kantor_numbered_abstract: Option<u32>,
}

impl Numbering {
    pub fn parse(src: &str) -> Self {
        let mut out = Numbering::default();
        let Some(root) = xml::parse(src) else {
            return out;
        };
        for c in &root.children {
            match c.name.as_str() {
                "abstractNum" => {
                    let Some(id) = c.num("abstractNumId") else {
                        continue;
                    };
                    let mut a = AbstractNum::default();
                    for l in &c.children {
                        match l.name.as_str() {
                            "lvl" => {
                                let ilvl = l.num::<u8>("ilvl").unwrap_or(0);
                                a.levels.insert(ilvl, LevelDef::parse(l));
                            }
                            "numStyleLink" => a.style_link = l.attr("val").map(str::to_owned),
                            _ => {}
                        }
                    }
                    out.abstracts.insert(id, a);
                }
                "num" => {
                    let Some(id) = c.num("numId") else { continue };
                    let mut n = Num {
                        abstract_id: c
                            .child("abstractNumId")
                            .and_then(|a| a.num("val"))
                            .unwrap_or(0),
                        ..Num::default()
                    };
                    for o in c.children.iter().filter(|o| o.name == "lvlOverride") {
                        let ilvl = o.num::<u8>("ilvl").unwrap_or(0);
                        if let Some(s) = o.child("startOverride").and_then(|s| s.num("val")) {
                            n.start_overrides.insert(ilvl, s);
                        }
                        if let Some(l) = o.child("lvl") {
                            n.level_overrides.insert(ilvl, LevelDef::parse(l));
                        }
                    }
                    out.nums.insert(id, n);
                }
                _ => {}
            }
        }
        out
    }

    /// The abstract definition a num uses, following `w:numStyleLink`
    /// through `link` (style id → the style's `w:numId`).
    fn abstract_of(
        &self,
        num: u32,
        link: &dyn Fn(&str) -> Option<u32>,
    ) -> Option<(u32, &AbstractNum)> {
        let mut aid = self.nums.get(&num)?.abstract_id;
        for _ in 0..4 {
            let a = self.abstracts.get(&aid)?;
            match a.style_link.as_deref().and_then(link) {
                Some(n) if a.levels.is_empty() && n != num => {
                    aid = self.nums.get(&n)?.abstract_id;
                }
                _ => return Some((aid, a)),
            }
        }
        None
    }

    /// The level definition for a paragraph's list reference.
    pub fn level(&self, r: NumRef, link: &dyn Fn(&str) -> Option<u32>) -> Option<&LevelDef> {
        let num = self.nums.get(&r.id)?;
        if let Some(l) = num.level_overrides.get(&r.level) {
            return Some(l);
        }
        self.abstract_of(r.id, link)?.1.levels.get(&r.level)
    }

    pub fn kind(&self, id: u32, link: &dyn Fn(&str) -> Option<u32>) -> Option<ListKind> {
        let l = self.level(NumRef { id, level: 0 }, link)?;
        Some(if l.fmt == NumFmt::Bullet {
            ListKind::Bullet
        } else {
            ListKind::Numbered
        })
    }

    /// Marker text for each paragraph's effective list reference, in
    /// document order (counters continue across paragraphs of the same
    /// abstract list; deeper levels restart after a shallower item).
    pub fn markers(
        &self,
        refs: impl IntoIterator<Item = Option<NumRef>>,
        link: &dyn Fn(&str) -> Option<u32>,
    ) -> Vec<Option<String>> {
        let mut counters: HashMap<u32, [Option<u32>; 9]> = HashMap::new();
        let mut seen: HashSet<u32> = HashSet::new();
        refs.into_iter()
            .map(|r| {
                let r = r.filter(|r| r.id != 0)?;
                let level = r.level.min(8);
                let (aid, _) = self.abstract_of(r.id, link)?;
                let num = self.nums.get(&r.id)?;
                let c = counters.entry(aid).or_insert([None; 9]);
                if seen.insert(r.id) {
                    for (&l, &s) in &num.start_overrides {
                        if let Some(slot) = c.get_mut(l as usize) {
                            *slot = Some(s.saturating_sub(1));
                        }
                    }
                }
                let def = self.level(NumRef { id: r.id, level }, link)?;
                let n = c[level as usize].map_or(def.start, |v| v + 1);
                c[level as usize] = Some(n);
                for slot in &mut c[level as usize + 1..] {
                    *slot = None;
                }
                if def.fmt == NumFmt::Bullet {
                    return Some(map_bullet(&def.text, def.font.as_deref()));
                }
                let mut text = String::new();
                let mut chars = def.text.chars().peekable();
                while let Some(ch) = chars.next() {
                    if ch == '%'
                        && let Some(d) = chars.peek().and_then(|d| d.to_digit(10))
                    {
                        chars.next();
                        let l = (d.max(1) - 1) as u8;
                        let ld = self.level(NumRef { id: r.id, level: l }, link);
                        let fmt = ld.map_or(NumFmt::Decimal, |x| x.fmt);
                        let v = c
                            .get(l as usize)
                            .copied()
                            .flatten()
                            .unwrap_or_else(|| ld.map_or(1, |x| x.start));
                        text.push_str(&fmt.format(v));
                    } else {
                        text.push(ch);
                    }
                }
                Some(text)
            })
            .collect()
    }

    fn next_abstract_id(&self) -> u32 {
        self.abstracts.keys().max().map_or(1, |m| m + 1)
    }

    fn next_num_id(&self) -> u32 {
        self.nums.keys().max().map_or(1, |m| m + 1)
    }

    fn add_abstract(&mut self, kind: ListKind) -> u32 {
        let id = self.next_abstract_id();
        let mut xml = format!(
            "<w:abstractNum w:abstractNumId=\"{id}\"><w:multiLevelType w:val=\"hybridMultilevel\"/>"
        );
        let mut a = AbstractNum::default();
        for l in 0..9u8 {
            let (fmt, fmt_name, text) = match kind {
                ListKind::Bullet => {
                    let b = ["•", "◦", "▪"][l as usize % 3];
                    (NumFmt::Bullet, "bullet", b.to_owned())
                }
                ListKind::Numbered => {
                    let (f, n) = [
                        (NumFmt::Decimal, "decimal"),
                        (NumFmt::LowerLetter, "lowerLetter"),
                        (NumFmt::LowerRoman, "lowerRoman"),
                    ][l as usize % 3];
                    (f, n, format!("%{}.", l + 1))
                }
            };
            let left = 720 * (l as i32 + 1);
            xml.push_str(&format!(
                "<w:lvl w:ilvl=\"{l}\"><w:start w:val=\"1\"/><w:numFmt w:val=\"{fmt_name}\"/>\
                 <w:lvlText w:val=\"{}\"/><w:lvlJc w:val=\"left\"/><w:pPr><w:ind w:left=\"{left}\" \
                 w:hanging=\"360\"/></w:pPr></w:lvl>",
                xml::escape(&text)
            ));
            a.levels.insert(
                l,
                LevelDef {
                    start: 1,
                    fmt,
                    text,
                    indent: Indent {
                        left: Some(left),
                        right: None,
                        first: Some(-360),
                    },
                    font: None,
                },
            );
        }
        xml.push_str("</w:abstractNum>");
        self.abstracts.insert(id, a);
        self.added_abstracts.push(xml);
        id
    }

    fn add_num(&mut self, abstract_id: u32, restart: bool) -> u32 {
        let id = self.next_num_id();
        let mut n = Num {
            abstract_id,
            ..Num::default()
        };
        let mut xml = format!("<w:num w:numId=\"{id}\"><w:abstractNumId w:val=\"{abstract_id}\"/>");
        if restart {
            xml.push_str(
                "<w:lvlOverride w:ilvl=\"0\"><w:startOverride w:val=\"1\"/></w:lvlOverride>",
            );
            n.start_overrides.insert(0, 1);
        }
        xml.push_str("</w:num>");
        self.nums.insert(id, n);
        self.added_nums.push(xml);
        id
    }

    /// A `w:num` for a new list of `kind`: bullets share one Kantor list,
    /// each new numbered list restarts at 1.
    pub fn new_list(&mut self, kind: ListKind) -> u32 {
        match kind {
            ListKind::Bullet => {
                if let Some(id) = self.kantor_bullet {
                    return id;
                }
                let a = self.add_abstract(kind);
                let id = self.add_num(a, false);
                self.kantor_bullet = Some(id);
                id
            }
            ListKind::Numbered => {
                let a = match self.kantor_numbered_abstract {
                    Some(a) => a,
                    None => {
                        let a = self.add_abstract(kind);
                        self.kantor_numbered_abstract = Some(a);
                        a
                    }
                };
                self.add_num(a, true)
            }
        }
    }

    pub fn is_modified(&self) -> bool {
        !self.added_abstracts.is_empty() || !self.added_nums.is_empty()
    }

    /// The numbering part to write: the original with Kantor's definitions
    /// added, or a new part.
    pub fn write(&self, original: Option<&str>) -> String {
        let abstracts = self.added_abstracts.concat();
        let nums = self.added_nums.concat();
        match original {
            Some(src) => {
                let mut out = src.to_owned();
                // abstractNums come before every num (schema order).
                let at = find_tag(&out, "num")
                    .or_else(|| find_tag(&out, "numIdMacAtCleanup"))
                    .or_else(|| out.rfind("</w:numbering>"));
                match at {
                    Some(i) => out.insert_str(i, &abstracts),
                    None => out.push_str(&abstracts),
                }
                let end =
                    find_tag(&out, "numIdMacAtCleanup").or_else(|| out.rfind("</w:numbering>"));
                match end {
                    Some(i) => out.insert_str(i, &nums),
                    None => out.push_str(&nums),
                }
                out
            }
            None => format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n<w:numbering \
                 xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">{abstracts}{nums}</w:numbering>"
            ),
        }
    }
}

/// Byte offset of the first `<w:name ` or `<w:name>` start tag.
fn find_tag(src: &str, name: &str) -> Option<usize> {
    let open = format!("<w:{name}");
    let mut from = 0;
    while let Some(i) = src[from..].find(&open) {
        let at = from + i;
        if src[at + open.len()..].starts_with([' ', '>', '/']) {
            return Some(at);
        }
        from = at + open.len();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"<?xml version="1.0"?><w:numbering xmlns:w="w"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%1.%2)"/></w:lvl></w:abstractNum><w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/><w:lvlText w:val="&#xF0B7;"/><w:rPr><w:rFonts w:ascii="Symbol" w:hAnsi="Symbol" w:hint="default"/></w:rPr></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num><w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num><w:num w:numId="3"><w:abstractNumId w:val="0"/><w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride></w:num></w:numbering>"#;

    fn r(id: u32, level: u8) -> Option<NumRef> {
        Some(NumRef { id, level })
    }

    #[test]
    fn markers_count_restart_and_map_bullets() {
        let n = Numbering::parse(SRC);
        let none = |_: &str| None;
        let m = n.markers(
            [
                r(1, 0),
                r(1, 1),
                r(1, 1),
                None,
                r(1, 0),
                r(1, 1),
                r(2, 0),
                r(3, 0),
            ],
            &none,
        );
        let m: Vec<_> = m.iter().map(|x| x.as_deref().unwrap_or("-")).collect();
        assert_eq!(m, ["1.", "1.a)", "1.b)", "-", "2.", "2.a)", "•", "5."]);
        assert_eq!(
            n.level(NumRef { id: 1, level: 0 }, &none)
                .unwrap()
                .indent
                .first,
            Some(-360)
        );
        assert_eq!(roman(1994), "mcmxciv");
        assert_eq!(letters(28), "bb");
    }

    #[test]
    fn added_lists_keep_the_original_bytes() {
        let mut n = Numbering::parse(SRC);
        let b = n.new_list(ListKind::Bullet);
        assert_eq!(n.new_list(ListKind::Bullet), b, "bullets reuse one list");
        let a = n.new_list(ListKind::Numbered);
        let c = n.new_list(ListKind::Numbered);
        assert_ne!(a, c, "each numbered list restarts");
        assert_eq!(n.kind(b, &|_| None), Some(ListKind::Bullet));
        let out = n.write(Some(SRC));
        let back = Numbering::parse(&out);
        assert_eq!(back.nums.len(), 6);
        assert_eq!(back.abstracts.len(), 4);
        // The original definitions are untouched, in place.
        assert!(out.starts_with(&SRC[..SRC.find("<w:num ").unwrap()]));
        assert!(out.find("w:abstractNumId=\"2\"").unwrap() < out.find("<w:num ").unwrap());
        let fresh = Numbering::default().write(None);
        assert!(fresh.contains("<w:numbering"));
    }
}
