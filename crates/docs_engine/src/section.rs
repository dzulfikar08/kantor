//! Sections, headers and footers, and page number fields.
//!
//! A section ends at a paragraph whose `w:pPr` holds a `w:sectPr` (kept
//! verbatim in the paragraph's extras) or at the end of the body (the
//! body-level `w:sectPr`, [`Document::body_sect`]). Each one has its own
//! page size, margins, header/footer distances, "different first page"
//! (`w:titlePg`), page numbering and header/footer references. Sections
//! without a reference of some kind use the previous section's (Word's
//! "Link to Previous").
//!
//! Headers and footers ([`Story`]) are their own parts (`word/header1.xml`
//! …), read with the body reader into a small [`Document`] that shares the
//! main document's styles. Saving writes a story back only when it was
//! changed or created in Kantor; untouched parts stay byte for byte.
//!
//! Page numbers are fields (`PAGE`, `NUMPAGES`) kept as opaque objects;
//! [`page_field`] recognises them so layout can show the real number on
//! each page.

use std::sync::Arc;

use crate::document::{Document, PageGeometry};
use crate::model::{Align, Object, Opaque, Paragraph, RunProps, Selection, element_name};
use crate::object::Level;
use crate::page::{sect_prefix, set_sect_child};
use crate::xml;

/// Header or footer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HfKind {
    Header,
    Footer,
}

impl HfKind {
    fn element(self) -> &'static str {
        match self {
            Self::Header => "headerReference",
            Self::Footer => "footerReference",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Header => "Header",
            Self::Footer => "Footer",
        }
    }
}

/// Which pages a header or footer is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HfType {
    /// Every page (odd pages when odd and even differ).
    Default,
    /// The section's first page, when "different first page" is on.
    First,
    /// Even pages, when "different odd and even pages" is on.
    Even,
}

impl HfType {
    pub const ALL: [Self; 3] = [Self::Default, Self::First, Self::Even];

    fn attr(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::First => "first",
            Self::Even => "even",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "first" => Self::First,
            "even" => Self::Even,
            _ => Self::Default,
        }
    }
}

/// How a section starts (`w:type`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SectionStart {
    #[default]
    NextPage,
    Continuous,
    EvenPage,
    OddPage,
}

/// Page number format (`w:pgNumType w:fmt`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NumFormat {
    #[default]
    Decimal,
    LowerRoman,
    UpperRoman,
    LowerLetter,
    UpperLetter,
}

impl NumFormat {
    fn parse(s: &str) -> Self {
        match s {
            "lowerRoman" => Self::LowerRoman,
            "upperRoman" => Self::UpperRoman,
            "lowerLetter" => Self::LowerLetter,
            "upperLetter" => Self::UpperLetter,
            _ => Self::Decimal,
        }
    }

    /// `n` written in this format.
    pub fn format(self, n: u32) -> String {
        match self {
            Self::Decimal => n.to_string(),
            Self::LowerRoman => roman(n).to_lowercase(),
            Self::UpperRoman => roman(n),
            Self::LowerLetter => letters(n).to_lowercase(),
            Self::UpperLetter => letters(n),
        }
    }
}

fn roman(mut n: u32) -> String {
    if n == 0 {
        return "0".into();
    }
    const T: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
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

/// Word's letter numbering: A…Z, then AA…ZZ, AAA….
fn letters(n: u32) -> String {
    if n == 0 {
        return "0".into();
    }
    let c = (b'A' + ((n - 1) % 26) as u8) as char;
    c.to_string().repeat(((n - 1) / 26 + 1) as usize)
}

/// A section's properties, read from its `w:sectPr`.
#[derive(Clone, Debug, PartialEq)]
pub struct SectionProps {
    pub page: PageGeometry,
    /// Distance from the page's top edge to the header, twips.
    pub header_dist: u32,
    /// Distance from the page's bottom edge to the footer, twips.
    pub footer_dist: u32,
    /// Different first page.
    pub title_page: bool,
    pub start: SectionStart,
    /// Header and footer references: kind, type, relationship id.
    pub refs: Vec<(HfKind, HfType, String)>,
    /// Restart page numbering at this number.
    pub page_start: Option<u32>,
    pub num_format: NumFormat,
}

impl SectionProps {
    /// Properties from a `<w:sectPr>…</w:sectPr>` (`page` is the size and
    /// margins to assume where it has none).
    pub fn parse(sect_pr: &str, page: PageGeometry) -> Self {
        let mut s = Self {
            page,
            header_dist: 708,
            footer_dist: 708,
            title_page: false,
            start: SectionStart::NextPage,
            refs: Vec::new(),
            page_start: None,
            num_format: NumFormat::Decimal,
        };
        let Some(el) = xml::parse(sect_pr) else {
            return s;
        };
        for c in &el.children {
            match c.name.as_str() {
                "pgSz" => {
                    s.page.width = c.num("w").unwrap_or(s.page.width);
                    s.page.height = c.num("h").unwrap_or(s.page.height);
                }
                "pgMar" => {
                    let n = |k: &str| c.num::<i64>(k).map(|v| v.unsigned_abs() as u32);
                    let pg = &mut s.page;
                    pg.margin_top = n("top").unwrap_or(pg.margin_top);
                    pg.margin_bottom = n("bottom").unwrap_or(pg.margin_bottom);
                    pg.margin_left = n("left").unwrap_or(pg.margin_left);
                    pg.margin_right = n("right").unwrap_or(pg.margin_right);
                    s.header_dist = n("header").unwrap_or(s.header_dist);
                    s.footer_dist = n("footer").unwrap_or(s.footer_dist);
                }
                "titlePg" => s.title_page = c.on(),
                "type" => {
                    s.start = match c.attr("val") {
                        Some("continuous") => SectionStart::Continuous,
                        Some("evenPage") => SectionStart::EvenPage,
                        Some("oddPage") => SectionStart::OddPage,
                        _ => SectionStart::NextPage,
                    }
                }
                "pgNumType" => {
                    s.page_start = c.num("start");
                    s.num_format = c.attr("fmt").map(NumFormat::parse).unwrap_or_default();
                }
                "headerReference" | "footerReference" => {
                    let kind = if c.name == "headerReference" {
                        HfKind::Header
                    } else {
                        HfKind::Footer
                    };
                    let ty = HfType::parse(c.attr("type").unwrap_or("default"));
                    if let Some(id) = c.attr("id") {
                        s.refs.push((kind, ty, id.to_owned()));
                    }
                }
                _ => {}
            }
        }
        s
    }

    pub fn reference(&self, kind: HfKind, ty: HfType) -> Option<&str> {
        self.refs
            .iter()
            .find(|r| r.0 == kind && r.1 == ty)
            .map(|r| r.2.as_str())
    }
}

/// Where a section's `w:sectPr` lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SectAt {
    /// In the pPr of this (last) paragraph of the section.
    Para(usize),
    /// The body's final sectPr.
    Body,
}

/// A section: its paragraphs and properties.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub paras: std::ops::Range<usize>,
    pub at: SectAt,
    pub props: SectionProps,
}

/// A header or footer part.
#[derive(Clone)]
pub struct Story {
    pub kind: HfKind,
    /// Relationship id from the main document part.
    pub rel_id: String,
    /// Zip entry name (`word/header1.xml`).
    pub part: String,
    /// The original root start tag (`<w:hdr …>`).
    pub(crate) root_tag: String,
    pub doc: Document,
    /// Changed (or created) in Kantor: Save writes it.
    pub(crate) modified: bool,
    /// Created in Kantor and not saved yet (needs a relationship, a
    /// content type and the part itself).
    pub(crate) new: bool,
    /// Why Kantor won't edit it (it still shows and is kept as is).
    pub locked: Option<String>,
}

impl std::fmt::Debug for Story {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Story")
            .field("kind", &self.kind)
            .field("rel_id", &self.rel_id)
            .field("part", &self.part)
            .field("modified", &self.modified)
            .finish()
    }
}

impl PartialEq for Story {
    fn eq(&self, o: &Self) -> bool {
        self.kind == o.kind
            && self.rel_id == o.rel_id
            && self.part == o.part
            && self.modified == o.modified
            && self.doc.paragraphs == o.doc.paragraphs
    }
}

/// A page number field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageField {
    /// `PAGE`: this page's number.
    Page,
    /// `NUMPAGES`: pages in the document.
    NumPages,
}

impl PageField {
    /// How the field is written into header/footer text in Kantor's editor.
    pub fn token(self) -> &'static str {
        match self {
            Self::Page => "{PAGE}",
            Self::NumPages => "{PAGES}",
        }
    }

    fn instr(self) -> &'static str {
        match self {
            Self::Page => "PAGE",
            Self::NumPages => "NUMPAGES",
        }
    }

    /// A new field object (`w:fldSimple`), showing `shown` until laid out.
    pub fn object(self, shown: &str) -> Object {
        Object::Opaque(Arc::new(Opaque {
            level: Level::Para,
            xml: format!(
                "<w:fldSimple w:instr=\" {} \\* MERGEFORMAT \"><w:r><w:t>{}</w:t></w:r></w:fldSimple>",
                self.instr(),
                xml::escape(shown)
            ),
            label: shown.to_owned(),
        }))
    }
}

/// The field instruction of an opaque field object, if it is one.
pub fn field_instr(o: &Opaque) -> Option<String> {
    match element_name(&o.xml) {
        "fldSimple" => xml::parse(&o.xml).and_then(|e| e.attr("instr").map(str::to_owned)),
        "r" if o.xml.contains("fldChar") => Some(xml::inner_text(&o.xml, &["instrText"])),
        _ => None,
    }
}

/// Whether an object is a `PAGE` or `NUMPAGES` field.
pub fn page_field(o: &Object) -> Option<PageField> {
    let Object::Opaque(o) = o else { return None };
    let instr = field_instr(o)?;
    match instr
        .split_whitespace()
        .next()?
        .to_ascii_uppercase()
        .as_str()
    {
        "PAGE" => Some(PageField::Page),
        "NUMPAGES" => Some(PageField::NumPages),
        _ => None,
    }
}

/// `paras` with every PAGE / NUMPAGES field replaced by the number as text
/// (in the field's own formatting); `None` when there are none.
pub fn with_page_numbers(paras: &[Paragraph], page: &str, pages: &str) -> Option<Vec<Paragraph>> {
    let any = paras
        .iter()
        .any(|p| p.objects().any(|(_, o)| page_field(o).is_some()));
    if !any {
        return None;
    }
    Some(
        paras
            .iter()
            .map(|p| {
                let fields: Vec<(usize, PageField, RunProps)> = p
                    .objects()
                    .filter_map(|(i, o)| page_field(o).map(|f| (i, f, p.props_at(i + 1))))
                    .collect();
                let mut p = p.clone();
                // Back to front so earlier offsets stay valid.
                for (i, f, props) in fields.into_iter().rev() {
                    p.delete(i..i + crate::model::OBJECT.len_utf8());
                    let text = match f {
                        PageField::Page => page,
                        PageField::NumPages => pages,
                    };
                    p.insert(i, text, props);
                }
                p
            })
            .collect(),
    )
}

/// The sectPr element of a paragraph (in its extras), if it ends a section.
pub(crate) fn para_sect(p: &Paragraph) -> Option<&str> {
    p.props.extras.iter().find(|x| element_name(x) == "sectPr")
}

/// A fresh, empty body sectPr for `page`.
pub(crate) fn default_sect_pr(page: &PageGeometry) -> String {
    format!(
        "<w:sectPr><w:pgSz w:w=\"{}\" w:h=\"{}\"/><w:pgMar w:top=\"{}\" w:right=\"{}\" \
         w:bottom=\"{}\" w:left=\"{}\" w:header=\"708\" w:footer=\"708\" w:gutter=\"0\"/></w:sectPr>",
        page.width,
        page.height,
        page.margin_top,
        page.margin_right,
        page.margin_bottom,
        page.margin_left
    )
}

const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// Header/footer state that undo restores as a whole.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HfState {
    pub stories: Vec<Story>,
    pub body_sect: Option<String>,
    pub even_odd: bool,
}

impl Document {
    /// Every section in order (at least one).
    pub fn sections(&self) -> Vec<Section> {
        let mut out = Vec::new();
        let mut start = 0;
        for (i, p) in self.paragraphs.iter().enumerate() {
            if p.cell.is_some() {
                continue;
            }
            if let Some(sp) = para_sect(p) {
                out.push(Section {
                    paras: start..i + 1,
                    at: SectAt::Para(i),
                    props: SectionProps::parse(sp, self.page),
                });
                start = i + 1;
            }
        }
        let body = match &self.body_sect {
            Some(s) => SectionProps::parse(s, self.page),
            None => SectionProps::parse("<w:sectPr/>", self.page),
        };
        out.push(Section {
            paras: start..self.paragraphs.len().max(start),
            at: SectAt::Body,
            // The body's size and margins are the model's (Page setup).
            props: SectionProps {
                page: self.page,
                ..body
            },
        });
        out
    }

    /// Index of the section holding paragraph `para`.
    pub fn section_of(&self, para: usize) -> usize {
        self.sections()
            .iter()
            .position(|s| para < s.paras.end)
            .unwrap_or(0)
    }

    /// Different odd and even pages (`w:evenAndOddHeaders` in settings).
    pub fn even_odd_headers(&self) -> bool {
        self.even_odd
    }

    pub fn stories(&self) -> &[Story] {
        &self.stories
    }

    pub fn story(&self, rel_id: &str) -> Option<&Story> {
        self.stories.iter().find(|s| s.rel_id == rel_id)
    }

    /// The story a section uses for `kind`/`ty`, following "Link to
    /// Previous" back through earlier sections.
    pub fn story_index_for(
        &self,
        sections: &[Section],
        section: usize,
        kind: HfKind,
        ty: HfType,
    ) -> Option<usize> {
        for s in sections[..=section.min(sections.len().saturating_sub(1))]
            .iter()
            .rev()
        {
            if let Some(id) = s.props.reference(kind, ty) {
                return self.stories.iter().position(|st| st.rel_id == id);
            }
        }
        None
    }

    /// Text of a header or footer for the editor: paragraphs joined with
    /// `\n`, page fields as `{PAGE}` / `{PAGES}`, other objects left out.
    /// Also its first paragraph's alignment.
    pub fn hf_text(&self, section: usize, kind: HfKind, ty: HfType) -> (String, Option<Align>) {
        let sections = self.sections();
        let Some(i) = self.story_index_for(&sections, section, kind, ty) else {
            return (String::new(), None);
        };
        let doc = &self.stories[i].doc;
        let mut lines = Vec::new();
        for p in doc.paragraphs() {
            let mut line = String::new();
            for (range, run) in p.run_ranges() {
                match &run.object {
                    Some(o) => {
                        if let Some(f) = page_field(o) {
                            line.push_str(f.token());
                        }
                    }
                    None => line.push_str(&p.text[range]),
                }
            }
            lines.push(line);
        }
        let align = doc.paragraphs().first().map(|p| doc.resolve_para(p).align);
        (lines.join("\n"), align)
    }

    /// Why the header/footer used there can't be edited, if so.
    pub fn hf_locked(&self, section: usize, kind: HfKind, ty: HfType) -> Option<String> {
        let sections = self.sections();
        let i = self.story_index_for(&sections, section, kind, ty)?;
        let st = &self.stories[i];
        if let Some(l) = &st.locked {
            return Some(l.clone());
        }
        let has_other = st.doc.paragraphs().iter().any(|p| {
            p.cell.is_some()
                || p.objects()
                    .any(|(_, o)| page_field(o).is_none() && !o.is_marker())
        });
        has_other.then(|| {
            format!(
                "This {} has pictures, tables or other objects; Kantor shows it and keeps it as it is, but can't edit it yet",
                kind.label().to_lowercase()
            )
        })
    }

    pub(crate) fn set_hf_state(&mut self, s: HfState) {
        self.stories = s.stories;
        self.body_sect = s.body_sect;
        self.even_odd = s.even_odd;
    }

    fn hf_state(&self) -> HfState {
        HfState {
            stories: self.stories.clone(),
            body_sect: self.body_sect.clone(),
            even_odd: self.even_odd,
        }
    }

    /// Record a header/footer change made since `before` (and the
    /// paragraph `para` changing from `old`, for a section's own sectPr).
    fn push_hf(&mut self, before: HfState, para: Option<(usize, Paragraph)>, sel: Selection) {
        let after = self.hf_state();
        let (start, old, new) = match para {
            Some((i, old)) => (i, vec![old], vec![self.paragraphs[i].clone()]),
            None => (0, Vec::new(), Vec::new()),
        };
        if before == after && old == new {
            return;
        }
        self.push_command(start, old, new, sel, Some(Box::new((before, after))));
    }

    /// The sectPr of section `si` (creating the body's if missing).
    fn sect_xml(&self, sections: &[Section], si: usize) -> String {
        match sections[si].at {
            SectAt::Para(i) => para_sect(&self.paragraphs[i])
                .unwrap_or_default()
                .to_owned(),
            SectAt::Body => self
                .body_sect
                .clone()
                .unwrap_or_else(|| default_sect_pr(&self.page)),
        }
    }

    /// Replace section `si`'s sectPr; returns the paragraph's old value when
    /// the sectPr is a paragraph's.
    fn set_sect_xml(
        &mut self,
        sections: &[Section],
        si: usize,
        xml: String,
    ) -> Option<(usize, Paragraph)> {
        match sections[si].at {
            SectAt::Para(i) => {
                let old = self.paragraphs[i].clone();
                let p = &mut self.paragraphs[i];
                p.props.extras.remove("sectPr");
                p.props.extras.push(&xml);
                Some((i, old))
            }
            SectAt::Body => {
                self.body_sect = Some(xml);
                None
            }
        }
    }

    /// Turn "different first page" on or off for section `si`.
    pub fn set_title_page(&mut self, si: usize, on: bool, sel: Selection) {
        let sections = self.sections();
        if si >= sections.len() || sections[si].props.title_page == on {
            return;
        }
        let before = self.hf_state();
        let xml = self.sect_xml(&sections, si);
        let p = sect_prefix(&xml).to_owned();
        let xml = set_sect_child(
            &xml,
            "titlePg",
            |_| true,
            on.then(|| format!("<{p}titlePg/>")),
        );
        let para = self.set_sect_xml(&sections, si, xml);
        self.push_hf(before, para, sel);
    }

    /// Turn "different odd and even pages" on or off (whole document).
    pub fn set_even_odd_headers(&mut self, on: bool, sel: Selection) {
        if self.even_odd == on {
            return;
        }
        let before = self.hf_state();
        self.even_odd = on;
        self.push_hf(before, None, sel);
    }

    /// Set a header's or footer's text (`\n` separates paragraphs; `{PAGE}`
    /// and `{PAGES}` become page number fields) and alignment. Creates the
    /// part when the section has none. One undo step. Refused (`false`) for
    /// headers Kantor can't edit (see [`Self::hf_locked`]).
    pub fn set_hf_text(
        &mut self,
        si: usize,
        kind: HfKind,
        ty: HfType,
        text: &str,
        align: Option<Align>,
        sel: Selection,
    ) -> bool {
        if self.hf_locked(si, kind, ty).is_some() {
            return false;
        }
        let sections = self.sections();
        if si >= sections.len() {
            return false;
        }
        let before = self.hf_state();
        let mut para = None;
        let index = match self.story_index_for(&sections, si, kind, ty) {
            Some(i) => i,
            None => {
                if text.is_empty() {
                    return true;
                }
                let story = self.new_story(kind);
                let id = story.rel_id.clone();
                self.stories.push(story);
                let xml = self.sect_xml(&sections, si);
                let p = sect_prefix(&xml).to_owned();
                let r_decl = if self.root_declares_r() {
                    String::new()
                } else {
                    format!(" xmlns:r=\"{R_NS}\"")
                };
                let el = format!(
                    "<{p}{}{r_decl} {p}type=\"{}\" r:id=\"{id}\"/>",
                    kind.element(),
                    ty.attr()
                );
                // Next to the other references (they share a slot).
                let xml = insert_reference(&xml, &el);
                para = self.set_sect_xml(&sections, si, xml);
                self.stories.len() - 1
            }
        };
        let props = {
            let doc = &self.stories[index].doc;
            doc.paragraphs()
                .first()
                .map(|p| (p.runs[0].props.clone(), p.props.clone()))
        };
        let (run_props, para_props) = props.unwrap_or_else(|| {
            let style = match kind {
                HfKind::Header => "Header",
                HfKind::Footer => "Footer",
            };
            let mut pp = crate::model::ParaProps::default();
            if self.styles.get(style).is_some() {
                pp.style = Some(style.to_owned());
            }
            (RunProps::default(), pp)
        });
        let mut paras: Vec<Paragraph> = text
            .split('\n')
            .map(|line| {
                let mut p = Paragraph {
                    props: para_props.clone(),
                    ..Paragraph::default()
                };
                p.props.extras.remove("sectPr");
                p.runs[0].props = run_props.clone();
                let mut rest = line;
                while !rest.is_empty() {
                    let next = [PageField::Page, PageField::NumPages]
                        .into_iter()
                        .filter_map(|f| rest.find(f.token()).map(|at| (at, f)))
                        .min_by_key(|x| x.0);
                    match next {
                        Some((at, f)) => {
                            p.push(&rest[..at], run_props.clone());
                            p.push_object(f.object("1"), run_props.clone(), None);
                            rest = &rest[at + f.token().len()..];
                        }
                        None => {
                            p.push(rest, run_props.clone());
                            rest = "";
                        }
                    }
                }
                if let Some(a) = align {
                    p.props.align = Some(a);
                    p.props.extras.remove("jc");
                }
                p.normalize();
                p
            })
            .collect();
        if paras.is_empty() {
            paras.push(Paragraph::default());
        }
        let story = &mut self.stories[index];
        story.doc.paragraphs = paras;
        story.doc.tables.clear();
        story.modified = true;
        self.push_hf(before, para, sel);
        true
    }

    /// Whether the main part's root declares the `r:` prefix (new
    /// documents get docx-rs's root, which does).
    fn root_declares_r(&self) -> bool {
        self.package
            .as_ref()
            .is_none_or(|p| p.root_tag.contains(&format!("xmlns:r=\"{R_NS}\"")))
    }

    /// A new, empty header or footer part with an unused name and id.
    fn new_story(&self, kind: HfKind) -> Story {
        let (stem, root) = match kind {
            HfKind::Header => ("header", "hdr"),
            HfKind::Footer => ("footer", "ftr"),
        };
        let dir = self
            .package
            .as_ref()
            .and_then(|p| {
                p.document_part
                    .rsplit_once('/')
                    .map(|(d, _)| format!("{d}/"))
            })
            .unwrap_or_else(|| "word/".into());
        let taken = |name: &str| {
            self.stories.iter().any(|s| s.part == name)
                || self.package.as_ref().is_some_and(|p| p.has_part(name))
        };
        let part = (1..)
            .map(|n| format!("{dir}{stem}{n}.xml"))
            .find(|n| !taken(n))
            .expect("infinite names");
        let id_taken = |id: &str| {
            self.stories.iter().any(|s| s.rel_id == id)
                || self.package.as_ref().is_some_and(|p| p.has_rel(id))
        };
        let rel_id = (1..)
            .map(|n| format!("rIdKantor{}{n}", kind.label()))
            .find(|id| !id_taken(id))
            .expect("infinite ids");
        let mut doc = Document::new();
        doc.styles = self.styles.clone();
        doc.numbering = self.numbering.clone();
        Story {
            kind,
            rel_id,
            part,
            root_tag: format!("<w:{root} xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\">"),
            doc,
            modified: true,
            new: true,
            locked: None,
        }
    }

    /// Insert a section break after the caret's paragraph text: the text
    /// before the caret ends a section with the same settings as the one
    /// it was in; the rest starts a new one ("Next page"). One undo step.
    pub fn insert_section_break(&mut self, sel: Selection) -> Option<Selection> {
        let sel = self.clamp_selection(sel);
        let (start, _) = sel.ordered();
        if self.paragraphs[start.para].cell.is_some() {
            return None;
        }
        let sections = self.sections();
        let si = self.section_of(start.para);
        let mut xml = self.sect_xml(&sections, si);
        // The new break gets the section's own settings, but its
        // references stay with the section (the new part links to it).
        if sections[si].at == SectAt::Body {
            xml = crate::page::update_sect_pr(&xml, &self.page);
        }
        let mut first = self.paragraphs[start.para].clone();
        let mut second = first.split_off(start.offset);
        // The earlier part ends the section; a sectPr it had moves on.
        let had = para_sect(&first).map(str::to_owned);
        first.props.extras.remove("sectPr");
        first.props.extras.push(&xml);
        second.props.extras.remove("sectPr");
        if let Some(h) = had {
            second.props.extras.push(&h);
        }
        let after = Selection::caret(crate::model::Position::new(start.para + 1, 0));
        self.replace(
            start.para,
            1,
            vec![first, second],
            (sel, after),
            crate::document::Kind::Other,
            true,
        );
        Some(after)
    }

    /// Page setup of the section holding the caret: paper, orientation and
    /// margins. The last section's is [`Document::page`].
    pub fn section_page(&self, para: usize) -> PageGeometry {
        let sections = self.sections();
        sections[self.section_of(para)].props.page
    }

    /// Change the page setup of the section holding paragraph `para`: one
    /// undo step. The body's (last) section goes through
    /// [`Document::set_page`].
    pub fn set_section_page(&mut self, para: usize, page: PageGeometry, sel: Selection) {
        let sections = self.sections();
        let si = self.section_of(para);
        match sections[si].at {
            SectAt::Body => self.set_page(page, sel),
            SectAt::Para(i) => {
                let page = page.clamped();
                if page == sections[si].props.page {
                    return;
                }
                let xml = para_sect(&self.paragraphs[i])
                    .unwrap_or_default()
                    .to_owned();
                let mut p = self.paragraphs[i].clone();
                p.props.extras.remove("sectPr");
                p.props
                    .extras
                    .push(&crate::page::update_sect_pr(&xml, &page));
                self.replace(
                    i,
                    1,
                    vec![p],
                    (sel, sel),
                    crate::document::Kind::Other,
                    true,
                );
            }
        }
    }

    /// Whether `text` has page number fields (for tests and the UI).
    pub fn page_fields_in(paras: &[Paragraph]) -> usize {
        paras
            .iter()
            .flat_map(|p| p.objects())
            .filter(|(_, o)| page_field(o).is_some())
            .count()
    }

    /// Insert a page number field at the caret (body text).
    pub fn insert_page_field(&mut self, sel: Selection, field: PageField) -> Selection {
        self.insert_object(sel, field.object("1"))
    }
}

/// Insert a header/footer reference element with the others (first slot).
fn insert_reference(sect_pr: &str, el: &str) -> String {
    // After the last existing reference, else as the first child.
    let refs: Vec<usize> = ["headerReference", "footerReference"]
        .iter()
        .filter_map(|n| {
            let mut last = None;
            let mut from = 0;
            while let Some(i) = sect_pr[from..].find(&format!(":{n}")) {
                let at = from + i;
                let end = sect_pr[at..]
                    .find('>')
                    .map_or(sect_pr.len(), |e| at + e + 1);
                last = Some(end);
                from = end;
            }
            last
        })
        .collect();
    let at = refs.into_iter().max().or_else(|| {
        let open_end = sect_pr.find('>')? + 1;
        (!sect_pr[..open_end].ends_with("/>")).then_some(open_end)
    });
    match at {
        Some(at) => {
            let mut s = sect_pr.to_owned();
            s.insert_str(at, el);
            s
        }
        None => set_sect_child(sect_pr, "headerReference", |_| false, Some(el.to_owned())),
    }
}

/// Settings children that come after `w:evenAndOddHeaders` (schema order).
const SETTINGS_AFTER_EVEN_ODD: &[&str] = &[
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

/// Whether settings.xml turns on different odd and even headers.
pub(crate) fn settings_even_odd(settings: &str) -> bool {
    let Some(el) = xml::parse(settings) else {
        return false;
    };
    el.child("evenAndOddHeaders").is_some_and(|c| c.on())
}

/// settings.xml with `w:evenAndOddHeaders` set or removed (everything else
/// byte for byte).
pub(crate) fn set_settings_even_odd(settings: &str, on: bool) -> String {
    // Remove any existing element first.
    let mut s = settings.to_owned();
    if let Some(i) = s.find(":evenAndOddHeaders") {
        let start = s[..i].rfind('<').unwrap_or(i);
        let end = s[i..].find("/>").map_or(s.len(), |e| i + e + 2);
        s.replace_range(start..end, "");
    }
    if !on {
        return s;
    }
    let p = {
        let root = s.find("<w:settings").map(|_| "w:");
        root.unwrap_or("w:")
    };
    let el = format!("<{p}evenAndOddHeaders/>");
    let at = SETTINGS_AFTER_EVEN_ODD
        .iter()
        .filter_map(|n| s.find(&format!("<{p}{n}")))
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
    fn number_formats() {
        assert_eq!(NumFormat::UpperRoman.format(14), "XIV");
        assert_eq!(NumFormat::LowerRoman.format(4), "iv");
        assert_eq!(NumFormat::UpperLetter.format(28), "BB");
        assert_eq!(NumFormat::Decimal.format(7), "7");
    }

    #[test]
    fn section_props_parse() {
        let s = SectionProps::parse(
            r#"<w:sectPr><w:headerReference w:type="first" r:id="rId9"/><w:type w:val="continuous"/><w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/><w:pgMar w:top="1000" w:right="1000" w:bottom="1000" w:left="1000" w:header="500" w:footer="600" w:gutter="0"/><w:pgNumType w:fmt="lowerRoman" w:start="3"/><w:titlePg/></w:sectPr>"#,
            PageGeometry::default(),
        );
        assert_eq!(s.page.width, 16838);
        assert_eq!((s.header_dist, s.footer_dist), (500, 600));
        assert!(s.title_page);
        assert_eq!(s.start, SectionStart::Continuous);
        assert_eq!(s.page_start, Some(3));
        assert_eq!(s.num_format, NumFormat::LowerRoman);
        assert_eq!(s.reference(HfKind::Header, HfType::First), Some("rId9"));
        assert_eq!(s.reference(HfKind::Footer, HfType::Default), None);
    }

    #[test]
    fn settings_toggle_keeps_order() {
        let src = r#"<w:settings xmlns:w="x"><w:zoom w:percent="100"/><w:defaultTabStop w:val="720"/><w:characterSpacingControl w:val="doNotCompress"/><w:compat/></w:settings>"#;
        let on = set_settings_even_odd(src, true);
        assert!(on.contains(
            r#"<w:defaultTabStop w:val="720"/><w:evenAndOddHeaders/><w:characterSpacingControl"#
        ));
        assert!(settings_even_odd(&on));
        assert_eq!(set_settings_even_odd(&on, false), src);
    }

    #[test]
    fn references_go_first() {
        let s = r#"<w:sectPr><w:pgSz w:w="1" w:h="2"/></w:sectPr>"#;
        let out = insert_reference(s, r#"<w:footerReference w:type="default" r:id="rX"/>"#);
        assert_eq!(
            out,
            r#"<w:sectPr><w:footerReference w:type="default" r:id="rX"/><w:pgSz w:w="1" w:h="2"/></w:sectPr>"#
        );
        let out2 = insert_reference(&out, r#"<w:headerReference w:type="default" r:id="rY"/>"#);
        assert!(out2.contains(r#"r:id="rX"/><w:headerReference"#));
    }
}
