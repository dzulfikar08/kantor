//! Tab stops (`w:tabs` in a paragraph's or style's `w:pPr`) and the
//! document's default tab interval (`w:defaultTabStop` in settings.xml).
//!
//! A paragraph's stops are its styles' (root first, through `basedOn`),
//! then its own: a `w:tab` sets a stop at its position, one with
//! `w:val="clear"` removes an inherited one. Setting stops in Kantor
//! rewrites the paragraph's own `w:tabs` against what it inherits, as Word
//! does, so the result reads the same in Word.

use crate::document::Document;
use crate::model::{Extras, Paragraph, Selection, element_name};
use crate::xml;

/// How text lines up at a tab stop (`w:tab/@w:val`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TabKind {
    #[default]
    Left,
    Center,
    Right,
    /// The decimal point sits on the stop.
    Decimal,
    /// Not a stop for tab characters: a vertical line at the position.
    Bar,
}

impl TabKind {
    pub const ALL: [TabKind; 5] = [
        TabKind::Left,
        TabKind::Center,
        TabKind::Right,
        TabKind::Decimal,
        TabKind::Bar,
    ];

    /// `w:val` (`start`/`end` from bidi-aware files read as left/right).
    pub fn from_xml(v: &str) -> Option<Self> {
        Some(match v {
            "left" | "start" | "num" => Self::Left,
            "center" => Self::Center,
            "right" | "end" => Self::Right,
            "decimal" => Self::Decimal,
            "bar" => Self::Bar,
            _ => return None,
        })
    }

    pub fn xml(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Center => "center",
            Self::Right => "right",
            Self::Decimal => "decimal",
            Self::Bar => "bar",
        }
    }

    /// Word's name in the Tabs dialog.
    pub fn label(self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Center => "Center",
            Self::Right => "Right",
            Self::Decimal => "Decimal",
            Self::Bar => "Bar",
        }
    }
}

/// What fills the space before a tab stop (`w:tab/@w:leader`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TabLeader {
    #[default]
    None,
    /// `.......`
    Dot,
    /// `-------`
    Hyphen,
    /// `_______`
    Underscore,
    /// A thick line.
    Heavy,
    /// `·······`
    MiddleDot,
}

impl TabLeader {
    pub const ALL: [TabLeader; 6] = [
        TabLeader::None,
        TabLeader::Dot,
        TabLeader::Hyphen,
        TabLeader::Underscore,
        TabLeader::Heavy,
        TabLeader::MiddleDot,
    ];

    pub fn from_xml(v: &str) -> Self {
        match v {
            "dot" => Self::Dot,
            "hyphen" => Self::Hyphen,
            "underscore" => Self::Underscore,
            "heavy" => Self::Heavy,
            "middleDot" => Self::MiddleDot,
            _ => Self::None,
        }
    }

    pub fn xml(self) -> Option<&'static str> {
        Some(match self {
            Self::None => return None,
            Self::Dot => "dot",
            Self::Hyphen => "hyphen",
            Self::Underscore => "underscore",
            Self::Heavy => "heavy",
            Self::MiddleDot => "middleDot",
        })
    }

    /// How the Tabs dialog shows it (Word's "1 None", "2 ......." …).
    pub fn sample(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Dot => "........",
            Self::Hyphen => "--------",
            Self::Underscore => "________",
            Self::Heavy => "▬▬▬▬",
            Self::MiddleDot => "········",
        }
    }
}

/// A tab stop: position in twips from the column's left edge.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TabStop {
    pub pos: i32,
    pub kind: TabKind,
    pub leader: TabLeader,
}

impl TabStop {
    pub fn new(pos: i32, kind: TabKind, leader: TabLeader) -> Self {
        Self { pos, kind, leader }
    }

    fn xml(&self) -> String {
        let leader = self
            .leader
            .xml()
            .map(|l| format!(" w:leader=\"{l}\""))
            .unwrap_or_default();
        format!(
            "<w:tab w:val=\"{}\"{leader} w:pos=\"{}\"/>",
            self.kind.xml(),
            self.pos
        )
    }
}

/// Apply the `w:tabs` elements in `extras` to `stops`.
fn apply(stops: &mut Vec<TabStop>, extras: &Extras) {
    for x in extras.iter().filter(|x| element_name(x) == "tabs") {
        let Some(el) = xml::parse(x) else { continue };
        for t in el.children.iter().filter(|c| c.name == "tab") {
            let Some(pos) = t.num::<i32>("pos") else {
                continue;
            };
            stops.retain(|s| s.pos != pos);
            let Some(kind) = t.attr("val").and_then(TabKind::from_xml) else {
                continue; // clear (or unknown)
            };
            let leader = t
                .attr("leader")
                .map_or(TabLeader::None, TabLeader::from_xml);
            stops.push(TabStop { pos, kind, leader });
        }
    }
}

/// `w:tabs` that turns `inherited` into `stops` (`None`: nothing to say).
fn own_tabs_xml(inherited: &[TabStop], stops: &[TabStop]) -> Option<String> {
    let mut els: Vec<(i32, String)> = Vec::new();
    for s in inherited {
        if !stops.iter().any(|n| n.pos == s.pos) {
            els.push((
                s.pos,
                format!("<w:tab w:val=\"clear\" w:pos=\"{}\"/>", s.pos),
            ));
        }
    }
    for n in stops {
        if !inherited.contains(n) {
            els.push((n.pos, n.xml()));
        }
    }
    if els.is_empty() {
        return None;
    }
    els.sort_by_key(|e| e.0);
    let body: String = els.into_iter().map(|e| e.1).collect();
    Some(format!("<w:tabs>{body}</w:tabs>"))
}

/// A change to the tab stops of the selected paragraphs (the Tabs dialog's
/// Set / Clear / Clear All, or a ruler drag).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TabEdit {
    /// Remove every stop first.
    pub clear_all: bool,
    /// Positions to remove (twips).
    pub clear: Vec<i32>,
    /// Stops to add, replacing any at the same position.
    pub set: Vec<TabStop>,
}

impl TabEdit {
    pub fn is_empty(&self) -> bool {
        !self.clear_all && self.clear.is_empty() && self.set.is_empty()
    }

    /// `stops` with the edit applied, sorted by position.
    pub fn apply_to(&self, stops: &[TabStop]) -> Vec<TabStop> {
        let mut out: Vec<TabStop> = if self.clear_all {
            Vec::new()
        } else {
            stops.to_vec()
        };
        out.retain(|s| !self.clear.contains(&s.pos));
        for n in &self.set {
            out.retain(|s| s.pos != n.pos);
            out.push(*n);
        }
        out.sort_by_key(|s| s.pos);
        out
    }
}

impl Document {
    /// Stops the paragraph's styles give it (document defaults, then its
    /// style's `basedOn` chain, root first).
    pub fn inherited_tab_stops(&self, p: &Paragraph) -> Vec<TabStop> {
        let mut stops = Vec::new();
        apply(&mut stops, &self.styles.default_para.extras);
        for st in self.styles.chain(p.props.style.as_deref()) {
            apply(&mut stops, &st.para.extras);
        }
        stops.sort_by_key(|s| s.pos);
        stops
    }

    /// A paragraph's tab stops, sorted by position (bar tabs included):
    /// its styles', then its own (`clear` removes an inherited one).
    pub fn tab_stops(&self, p: &Paragraph) -> Vec<TabStop> {
        let mut stops = self.inherited_tab_stops(p);
        apply(&mut stops, &p.props.extras);
        stops.sort_by_key(|s| s.pos);
        stops
    }

    /// Default tab stop distance (twips, `w:defaultTabStop`; 720 = 1.27 cm).
    pub fn default_tab(&self) -> u32 {
        self.default_tab
    }

    /// Stops every paragraph in the selection has (what the Tabs dialog
    /// lists when several paragraphs are selected, as Word).
    pub fn common_tab_stops(&self, sel: Selection) -> Vec<TabStop> {
        let (start, end) = self.clamp_selection(sel).ordered();
        let mut out: Option<Vec<TabStop>> = None;
        for p in &self.paragraphs()[start.para..=end.para] {
            let s = self.tab_stops(p);
            out = Some(match out {
                None => s,
                Some(o) => o.into_iter().filter(|x| s.contains(x)).collect(),
            });
        }
        out.unwrap_or_default()
    }

    /// Change the tab stops of every paragraph the selection touches: one
    /// undo step. Each paragraph's own `w:tabs` is rewritten against what
    /// its style gives it; paragraphs left with the same stops keep their
    /// XML unchanged.
    pub fn edit_tab_stops(&mut self, sel: Selection, edit: &TabEdit) {
        if edit.is_empty() {
            return;
        }
        let (start, end) = self.clamp_selection(sel).ordered();
        let plans: Vec<Option<Option<String>>> = self.paragraphs()[start.para..=end.para]
            .iter()
            .map(|p| {
                let now = self.tab_stops(p);
                let want = edit.apply_to(&now);
                (want != now).then(|| own_tabs_xml(&self.inherited_tab_stops(p), &want))
            })
            .collect();
        if plans.iter().all(Option::is_none) {
            return;
        }
        let i = std::cell::Cell::new(0);
        self.format_paragraphs(sel, |p| {
            let plan = plans.get(i.get()).cloned().flatten();
            i.set(i.get() + 1);
            if let Some(xml) = plan {
                p.extras.remove("tabs");
                if let Some(x) = xml {
                    p.extras.push(&x);
                }
            }
        });
    }

    /// Set the default tab interval (twips, at least 1): one undo step.
    pub fn set_default_tab(&mut self, twips: u32, sel: Selection) {
        let twips = twips.clamp(1, 31680);
        if twips == self.default_tab {
            return;
        }
        let before = self.side_state();
        self.default_tab = twips;
        let after = self.side_state();
        self.push_command(
            0,
            Vec::new(),
            Vec::new(),
            sel,
            Some(Box::new((before, after))),
        );
    }
}

/// `w:defaultTabStop` from settings.xml (twips).
pub(crate) fn settings_default_tab(settings: &str) -> Option<u32> {
    xml::parse(settings)?
        .child("defaultTabStop")?
        .num::<u32>("val")
        .filter(|v| *v > 0)
}

/// Settings children that come after `w:defaultTabStop` (CT_Settings order).
const SETTINGS_AFTER_DEFAULT_TAB: &[&str] = &[
    "autoHyphenation",
    "consecutiveHyphenLimit",
    "hyphenationZone",
    "doNotHyphenateCaps",
    "showEnvelope",
    "summaryLength",
    "clickAndTypeStyle",
    "defaultTableStyle",
    "evenAndOddHeaders",
    "bookFoldRevPrinting",
    "bookFoldPrinting",
    "bookFoldPrintingSheets",
    "drawingGridHorizontalSpacing",
    "drawingGridVerticalSpacing",
    "displayHorizontalDrawingGridEvery",
    "displayVerticalDrawingGridEvery",
    "doNotUseMarginsForDrawingGridOrigin",
    "drawingGridHorizontalOrigin",
    "drawingGridVerticalOrigin",
    "doNotShadeFormData",
    "noPunctuationKerning",
    "characterSpacingControl",
    "printTwoOnOne",
    "strictFirstAndLastChars",
    "noLineBreaksAfter",
    "noLineBreaksBefore",
    "savePreviewPicture",
    "doNotValidateAgainstSchema",
    "saveInvalidXml",
    "ignoreMixedContent",
    "alwaysShowPlaceholderText",
    "doNotDemarcateInvalidXml",
    "saveXmlDataOnly",
    "useXSLTWhenSaving",
    "saveThroughXslt",
    "showXMLTags",
    "alwaysMergeEmptyNamespace",
    "updateFields",
    "hdrShapeDefaults",
    "footnotePr",
    "endnotePr",
    "compat",
    "docVars",
    "rsids",
    "mathPr",
    "attachedSchema",
    "themeFontLang",
    "clrSchemeMapping",
    "doNotIncludeSubdocsInStats",
    "doNotAutoCompressPictures",
    "forceUpgrade",
    "captions",
    "readModeInkLockDown",
    "smartTagType",
    "schemaLibrary",
    "shapeDefaults",
    "doNotEmbedSmartTags",
    "decimalSymbol",
    "listSeparator",
];

/// settings.xml with `w:defaultTabStop` set to `twips` (everything else
/// byte for byte).
pub(crate) fn set_settings_default_tab(settings: &str, twips: u32) -> String {
    let mut s = settings.to_owned();
    let el = format!("<w:defaultTabStop w:val=\"{twips}\"/>");
    if let Some(i) = s.find(":defaultTabStop") {
        let start = s[..i].rfind('<').unwrap_or(i);
        let end = s[i..].find("/>").map_or(s.len(), |e| i + e + 2);
        s.replace_range(start..end, &el);
        return s;
    }
    let at = SETTINGS_AFTER_DEFAULT_TAB
        .iter()
        .filter_map(|n| s.find(&format!("<w:{n}")))
        .min()
        .or_else(|| s.rfind("</"));
    match at {
        Some(at) => s.insert_str(at, &el),
        None => s.push_str(&el),
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_tab_setting_rewrites_in_place() {
        let src = r#"<w:settings xmlns:w="w"><w:zoom w:percent="100"/><w:defaultTabStop w:val="720"/><w:compat/></w:settings>"#;
        assert_eq!(settings_default_tab(src), Some(720));
        let out = set_settings_default_tab(src, 567);
        assert_eq!(
            out,
            src.replace("w:val=\"720\"", "w:val=\"567\""),
            "only the value changes"
        );
        let none = r#"<w:settings xmlns:w="w"><w:zoom w:percent="100"/><w:compat/></w:settings>"#;
        assert_eq!(
            set_settings_default_tab(none, 851),
            r#"<w:settings xmlns:w="w"><w:zoom w:percent="100"/><w:defaultTabStop w:val="851"/><w:compat/></w:settings>"#
        );
    }

    #[test]
    fn own_tabs_clear_inherited_and_skip_unchanged() {
        let a = TabStop::new(1440, TabKind::Left, TabLeader::None);
        let b = TabStop::new(9026, TabKind::Right, TabLeader::Dot);
        assert_eq!(own_tabs_xml(&[a, b], &[a, b]), None);
        assert_eq!(
            own_tabs_xml(&[a], &[b]).unwrap(),
            r#"<w:tabs><w:tab w:val="clear" w:pos="1440"/><w:tab w:val="right" w:leader="dot" w:pos="9026"/></w:tabs>"#
        );
        // Same position, new alignment: one element, no clear.
        let c = TabStop::new(1440, TabKind::Center, TabLeader::None);
        assert_eq!(
            own_tabs_xml(&[a], &[c]).unwrap(),
            r#"<w:tabs><w:tab w:val="center" w:pos="1440"/></w:tabs>"#
        );
    }

    #[test]
    fn edits_set_replace_and_clear() {
        let a = TabStop::new(1440, TabKind::Left, TabLeader::None);
        let b = TabStop::new(2880, TabKind::Right, TabLeader::Dot);
        let e = TabEdit {
            set: vec![TabStop::new(1440, TabKind::Decimal, TabLeader::None)],
            clear: vec![2880],
            ..TabEdit::default()
        };
        assert_eq!(
            e.apply_to(&[a, b]),
            [TabStop::new(1440, TabKind::Decimal, TabLeader::None)]
        );
        let all = TabEdit {
            clear_all: true,
            set: vec![b],
            ..TabEdit::default()
        };
        assert_eq!(all.apply_to(&[a]), [b]);
    }
}
