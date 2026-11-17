//! Floating objects: `w:drawing/wp:anchor` pictures, shapes (`wps:wsp`,
//! with or without a text box), groups and canvases, as Word positions
//! them.
//!
//! A floating object is one [`crate::OBJECT`] character in its anchor
//! paragraph (zero width: the caret steps over it) whose run carries a
//! [`Float`]. The model holds what positioning needs ([`Placement`]: where
//! from, how far, size, wrapping, z-order) and what drawing needs
//! ([`FloatKind`], a text box's paragraphs). The original XML is the
//! source of truth for everything else: an object Kantor didn't change is
//! written back byte for byte, and a moved, resized or rewrapped one gets
//! only its `wp:anchor` attributes, `wp:positionH`/`wp:positionV`,
//! `wp:extent`, wrap element and graphic size (`a:xfrm/a:ext`) rewritten
//! ([`Float::write_xml`]). In `mc:AlternateContent` (Word's shapes, with a
//! VML fallback) the `mc:Choice` is patched and the fallback kept as it
//! was (Word 2010 and later read the choice).

use std::borrow::Cow;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use quick_xml::events::{BytesStart, Event};

use crate::Document;
use crate::object::EMU_PER_PX;
use crate::xml::{self, El};

/// What a horizontal position is measured from (`wp:positionH/@relativeFrom`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RelH {
    Page,
    Margin,
    Column,
    Character,
    LeftMargin,
    RightMargin,
    InsideMargin,
    OutsideMargin,
}

impl RelH {
    pub fn parse(s: &str) -> Self {
        match s {
            "page" => Self::Page,
            "margin" => Self::Margin,
            "character" => Self::Character,
            "leftMargin" => Self::LeftMargin,
            "rightMargin" => Self::RightMargin,
            "insideMargin" => Self::InsideMargin,
            "outsideMargin" => Self::OutsideMargin,
            _ => Self::Column,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Margin => "margin",
            Self::Column => "column",
            Self::Character => "character",
            Self::LeftMargin => "leftMargin",
            Self::RightMargin => "rightMargin",
            Self::InsideMargin => "insideMargin",
            Self::OutsideMargin => "outsideMargin",
        }
    }
}

/// What a vertical position is measured from (`wp:positionV/@relativeFrom`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RelV {
    Page,
    Margin,
    Paragraph,
    Line,
    TopMargin,
    BottomMargin,
    InsideMargin,
    OutsideMargin,
}

impl RelV {
    pub fn parse(s: &str) -> Self {
        match s {
            "page" => Self::Page,
            "margin" => Self::Margin,
            "line" => Self::Line,
            "topMargin" => Self::TopMargin,
            "bottomMargin" => Self::BottomMargin,
            "insideMargin" => Self::InsideMargin,
            "outsideMargin" => Self::OutsideMargin,
            _ => Self::Paragraph,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Margin => "margin",
            Self::Paragraph => "paragraph",
            Self::Line => "line",
            Self::TopMargin => "topMargin",
            Self::BottomMargin => "bottomMargin",
            Self::InsideMargin => "insideMargin",
            Self::OutsideMargin => "outsideMargin",
        }
    }
}

/// `wp:align`: left/top, center, right/bottom, inside, outside.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FloatAlign {
    Start,
    Center,
    End,
    Inside,
    Outside,
}

impl FloatAlign {
    fn parse(s: &str) -> Self {
        match s.trim() {
            "center" => Self::Center,
            "right" | "bottom" => Self::End,
            "inside" => Self::Inside,
            "outside" => Self::Outside,
            _ => Self::Start,
        }
    }

    fn as_str(self, horizontal: bool) -> &'static str {
        match (self, horizontal) {
            (Self::Start, true) => "left",
            (Self::Start, false) => "top",
            (Self::Center, _) => "center",
            (Self::End, true) => "right",
            (Self::End, false) => "bottom",
            (Self::Inside, _) => "inside",
            (Self::Outside, _) => "outside",
        }
    }
}

/// How far from its reference an object sits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Place {
    Align(FloatAlign),
    /// `wp:posOffset`, EMU.
    Offset(i64),
    /// `wp14:pctPosHOffset` / `pctPosVOffset`: thousandths of a percent of
    /// the reference's size.
    Pct(i32),
}

/// How text wraps around the object (Word's Layout Options).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Wrap {
    /// No wrapping: in front of the text, or behind it with
    /// [`Placement::behind`].
    None,
    Square,
    /// Text follows the wrap polygon ([`Float::wrap_polygon`]).
    Tight,
    /// As [`Wrap::Tight`] (Kantor doesn't let text into the polygon's
    /// hollows).
    Through,
    TopAndBottom,
}

/// Which sides of the object text may flow on (`@wrapText`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WrapSide {
    Both,
    Left,
    Right,
    Largest,
}

impl WrapSide {
    fn parse(s: Option<&str>) -> Self {
        match s {
            Some("left") => Self::Left,
            Some("right") => Self::Right,
            Some("largest") => Self::Largest,
            _ => Self::Both,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Both => "bothSides",
            Self::Left => "left",
            Self::Right => "right",
            Self::Largest => "largest",
        }
    }
}

/// Where a floating object goes and how text treats it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Placement {
    pub h: (RelH, Place),
    pub v: (RelV, Place),
    /// Size, EMU (`wp:extent`).
    pub cx: i64,
    pub cy: i64,
    /// Distance kept from text: top, bottom, left, right (EMU).
    pub dist: [i64; 4],
    pub wrap: Wrap,
    pub side: WrapSide,
    /// Behind the text (`@behindDoc`, with [`Wrap::None`]).
    pub behind: bool,
    pub allow_overlap: bool,
    /// Z-order (`@relativeHeight`): higher is drawn on top.
    pub z: u32,
    /// `@layoutInCell`.
    pub in_cell: bool,
}

impl Placement {
    /// Size in px.
    pub fn size_px(&self) -> (f32, f32) {
        (
            self.cx.max(0) as f32 / EMU_PER_PX,
            self.cy.max(0) as f32 / EMU_PER_PX,
        )
    }

    /// The wrap choice as Word's Layout Options list it.
    pub fn layout_option(&self) -> LayoutOption {
        match self.wrap {
            Wrap::Square => LayoutOption::Square,
            Wrap::Tight => LayoutOption::Tight,
            Wrap::Through => LayoutOption::Through,
            Wrap::TopAndBottom => LayoutOption::TopAndBottom,
            Wrap::None if self.behind => LayoutOption::Behind,
            Wrap::None => LayoutOption::InFront,
        }
    }

    /// Set the wrap as a Layout Options choice ([`LayoutOption::Inline`]
    /// isn't a floating placement: see [`Document::set_layout_option`]).
    pub fn set_layout_option(&mut self, o: LayoutOption) {
        let (wrap, behind) = match o {
            LayoutOption::Square | LayoutOption::Inline => (Wrap::Square, false),
            LayoutOption::Tight => (Wrap::Tight, false),
            LayoutOption::Through => (Wrap::Through, false),
            LayoutOption::TopAndBottom => (Wrap::TopAndBottom, false),
            LayoutOption::Behind => (Wrap::None, true),
            LayoutOption::InFront => (Wrap::None, false),
        };
        self.wrap = wrap;
        self.behind = behind;
    }
}

/// Word's Layout Options (the picker next to a selected picture).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutOption {
    Inline,
    Square,
    Tight,
    Through,
    TopAndBottom,
    Behind,
    InFront,
}

impl LayoutOption {
    pub const ALL: [Self; 7] = [
        Self::Inline,
        Self::Square,
        Self::Tight,
        Self::Through,
        Self::TopAndBottom,
        Self::Behind,
        Self::InFront,
    ];

    /// Word's name for the option.
    pub fn label(self) -> &'static str {
        match self {
            Self::Inline => "In Line with Text",
            Self::Square => "Square",
            Self::Tight => "Tight",
            Self::Through => "Through",
            Self::TopAndBottom => "Top and Bottom",
            Self::Behind => "Behind Text",
            Self::InFront => "In Front of Text",
        }
    }
}

/// What a floating object draws.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FloatKind {
    /// A picture (`pic:pic` with an embedded blip): relationship id, part
    /// name, and the crop (`a:srcRect` l, t, r, b in thousandths of a
    /// percent; negative values pad).
    Picture {
        rel_id: String,
        part: String,
        crop: [i32; 4],
    },
    /// A shape (`wps:wsp`), possibly holding a text box.
    Shape,
    /// A group of shapes and pictures (`wpg:wgp`) or a drawing canvas
    /// (`wpc:wpc`): drawn from the XML, moved and sized as one.
    Group { canvas: bool },
    /// A chart (`c:chart` in the graphic frame): relationship id and the
    /// chart part's name.
    Chart { rel_id: String, part: String },
    /// SmartArt (`dgm:relIds`): its data model part, and the drawing
    /// parts its anchor's part relates to (relationship id, part name;
    /// the data model's `dsp:dataModelExt/@relId` picks one). Drawn from
    /// Word's pre-drawn shapes (the drawing part).
    SmartArt {
        data: String,
        drawings: Vec<(String, String)>,
        /// The layout definition part (`r:lo`), if any.
        layout: Option<String>,
    },
    /// Other content: placed, shown as a box.
    Other(String),
}

/// A text box's content: its own small document (styles shared with the
/// main one), inner margins and vertical alignment.
#[derive(Clone)]
pub struct TextBox {
    pub doc: Document,
    /// Inner margins left, top, right, bottom (EMU; `a:bodyPr` `lIns`…).
    pub insets: [i64; 4],
    /// `a:bodyPr/@anchor`: top, middle or bottom.
    pub v_align: FloatAlign,
    /// "Resize shape to fit text" (`a:spAutoFit`): the shape is as tall
    /// as its text and inner margins.
    pub auto_fit: bool,
}

impl std::fmt::Debug for TextBox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextBox")
            .field("paragraphs", &self.doc.len())
            .field("insets", &self.insets)
            .finish()
    }
}

/// A floating object.
#[derive(Clone, Debug)]
pub struct Float {
    /// The run content as read (`<w:drawing>…` or `<mc:AlternateContent>…`).
    pub xml: Arc<str>,
    /// The placement the XML says.
    pub read: Placement,
    /// The placement now (moved, resized, rewrapped in Kantor).
    pub at: Placement,
    pub kind: FloatKind,
    /// `wp:docPr` name and alt text.
    pub name: String,
    pub descr: String,
    /// A shape's text box.
    pub text: Option<Arc<TextBox>>,
    /// The text box's text was changed in Kantor: its `w:txbxContent` is
    /// written from [`Float::text`].
    pub text_edited: bool,
    /// A group's or canvas's shapes' text boxes, in the order their
    /// `w:txbxContent`s are written ([`Float::text_box_in`]).
    pub boxes: Vec<GroupBox>,
    /// Parts of the package changed with this object (SmartArt text
    /// edited in Kantor: its drawing and data model), by name: written in
    /// place of the originals on save.
    pub parts: Vec<(String, Arc<str>)>,
}

/// The text box of one shape in a group.
#[derive(Clone, Debug)]
pub struct GroupBox {
    pub text: Arc<TextBox>,
    /// Changed in Kantor: its `w:txbxContent` is written from `text`.
    pub edited: bool,
}

impl GroupBox {
    fn edited_paras(&self) -> Option<&[crate::Paragraph]> {
        self.edited.then(|| self.text.doc.paragraphs())
    }
}

impl PartialEq for Float {
    fn eq(&self, o: &Self) -> bool {
        // The text box is read from the XML.
        self.xml == o.xml
            && self.at == o.at
            && self.read == o.read
            && self.kind == o.kind
            && self.text_edited == o.text_edited
            && (!self.text_edited
                || self.text.as_ref().map(|t| t.doc.paragraphs())
                    == o.text.as_ref().map(|t| t.doc.paragraphs()))
            && self.parts == o.parts
            && self.boxes.len() == o.boxes.len()
            && self
                .boxes
                .iter()
                .zip(&o.boxes)
                .all(|(a, b)| a.edited_paras() == b.edited_paras())
    }
}

impl Eq for Float {}

impl Hash for Float {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.xml.hash(h);
        self.at.hash(h);
        self.kind.hash(h);
        if self.text_edited
            && let Some(t) = &self.text
        {
            t.doc.paragraphs().hash(h);
        }
        for b in &self.boxes {
            b.edited_paras().hash(h);
        }
        self.parts.hash(h);
    }
}

impl Float {
    /// Short description for the status bar and placeholders.
    pub fn label(&self) -> &str {
        match &self.kind {
            FloatKind::Picture { .. } => "Floating picture",
            FloatKind::Shape if self.text.is_some() => "Text box",
            FloatKind::Shape => "Shape",
            FloatKind::Group { canvas: false } => "Group",
            FloatKind::Group { canvas: true } => "Drawing canvas",
            FloatKind::Chart { .. } => "Chart",
            FloatKind::SmartArt { .. } => "SmartArt",
            FloatKind::Other(what) => what,
        }
    }

    /// A floating object from its run content (`<w:drawing>` or
    /// `<mc:AlternateContent>`), pictures' parts unresolved.
    pub fn from_xml(xml_src: &str) -> Option<Self> {
        read(xml_src, &|_| None)
    }

    /// Whether Kantor changed it (its XML gets rewritten on save).
    pub fn is_modified(&self) -> bool {
        self.at != self.read || self.text_edited || self.boxes.iter().any(|b| b.edited)
    }

    /// A copy placed at `at`.
    pub fn placed(&self, at: Placement) -> Self {
        Self { at, ..self.clone() }
    }

    /// The XML to write: as read when unchanged, else with the placement
    /// patched in.
    pub fn write_xml(&self) -> Cow<'_, str> {
        if !self.is_modified() {
            return Cow::Borrowed(&self.xml);
        }
        let mut xml = Cow::Borrowed(&*self.xml);
        if self.at != self.read
            && let Some(s) = patch(&xml, &self.read, &self.at)
        {
            xml = Cow::Owned(s);
        }
        if self.text_edited
            && let Some(t) = &self.text
        {
            xml = Cow::Owned(replace_text_box(&xml, &crate::docx::body_xml(&t.doc)));
        }
        for (k, b) in self.boxes.iter().enumerate() {
            if b.edited {
                xml = Cow::Owned(replace_nth_text_box(
                    &xml,
                    k,
                    &crate::docx::body_xml(&b.text.doc),
                ));
            }
        }
        xml
    }

    /// The shape's own text box (`which` = `None`) or the text box of a
    /// group's shape number `which` ([`Float::boxes`]).
    pub fn text_box_in(&self, which: Option<usize>) -> Option<&Arc<TextBox>> {
        match which {
            None => self.text.as_ref(),
            Some(k) => self.boxes.get(k).map(|b| &b.text),
        }
    }

    /// Whether it has text Kantor can edit in place: a text box, or a
    /// group with text in its shapes.
    pub fn has_text(&self) -> bool {
        self.text.is_some() || !self.boxes.is_empty()
    }

    /// Why the text box can't be edited as plain text (tables, pictures
    /// or other objects in it), if so; `None` also when it has no text box.
    pub fn text_locked(&self) -> Option<&'static str> {
        self.text_locked_in(None)
    }

    /// [`Float::text_locked`] for the text box `which` ([`Float::text_box_in`]).
    pub fn text_locked_in(&self, which: Option<usize>) -> Option<&'static str> {
        let t = self.text_box_in(which)?;
        let other = t
            .doc
            .paragraphs()
            .iter()
            .any(|p| p.cell.is_some() || p.objects().any(|(_, o)| !o.is_zero_width()));
        other.then_some(
            "This text box has pictures, tables or other objects; Kantor shows it and keeps it as it is, but can't edit it yet",
        )
    }

    /// The text box's text for the editor: paragraphs joined with `\n`.
    pub fn text_box_text(&self) -> Option<String> {
        self.text_box_text_in(None)
    }

    /// [`Float::text_box_text`] for the text box `which` ([`Float::text_box_in`]).
    pub fn text_box_text_in(&self, which: Option<usize>) -> Option<String> {
        let t = self.text_box_in(which)?;
        let lines: Vec<String> = t
            .doc
            .paragraphs()
            .iter()
            .map(|p| p.text.replace(crate::OBJECT, ""))
            .collect();
        Some(lines.join("\n"))
    }

    /// The wrap polygon of a Tight or Through object (`wp:wrapPolygon`),
    /// as fractions of its box (Word writes it in a 21600 × 21600 space),
    /// when the XML has one for the wrap it has now.
    pub fn wrap_polygon(&self) -> Option<Vec<(f32, f32)>> {
        if !matches!(self.at.wrap, Wrap::Tight | Wrap::Through) || self.at.wrap != self.read.wrap {
            return None;
        }
        let choice = choice_part(&self.xml);
        let r = element_range(choice, "wrapPolygon")?;
        let el = xml::parse(&choice[r])?;
        let pts: Vec<(f32, f32)> = el
            .children
            .iter()
            .filter(|c| c.name == "start" || c.name == "lineTo")
            .filter_map(|c| Some((c.num::<f32>("x")? / 21_600.0, c.num::<f32>("y")? / 21_600.0)))
            .collect();
        (pts.len() >= 3).then_some(pts)
    }

    /// How far it's turned, degrees clockwise: its own `a:xfrm/@rot` (a
    /// picture's or shape's, a group's `grpSpPr`).
    pub fn rotation(&self) -> f32 {
        let choice = choice_part(&self.xml);
        let Some(g) = element_range(choice, "graphic") else {
            return 0.0;
        };
        element_range(&choice[g.clone()], "xfrm")
            .and_then(|r| {
                let x = &choice[g.start + r.start..g.start + r.end];
                let tag = &x[..x.find('>').map_or(x.len(), |i| i + 1)];
                let i = tag.find(" rot=\"")? + 6;
                let e = tag[i..].find('"')? + i;
                tag[i..e].parse::<f32>().ok()
            })
            .map_or(0.0, |r| r / 60_000.0)
    }

    /// The pictures (`a:blip/@r:embed`) a picture or group draws.
    pub fn picture_rels(&self) -> Vec<String> {
        match &self.kind {
            FloatKind::Picture { rel_id, .. } => vec![rel_id.clone()],
            FloatKind::Group { .. } => blip_rels(choice_part(&self.xml)),
            _ => Vec::new(),
        }
    }

    /// The graphic's content (`a:graphicData`'s child, e.g. `wpg:wgp`), as
    /// written, for drawing groups and canvases.
    pub fn graphic_xml(&self) -> Option<&str> {
        let choice = choice_part(&self.xml);
        element_range(choice, "graphicData").map(|r| &choice[r])
    }

    /// The `a:prstGeom` / `a:custGeom` element of a shape or picture, as
    /// written (for drawing its outline).
    pub fn geometry_xml(&self) -> Option<&str> {
        let choice = choice_part(&self.xml);
        for name in ["prstGeom", "custGeom"] {
            if let Some(r) = element_range(choice, name) {
                return Some(&choice[r]);
            }
        }
        None
    }

    /// The shape's properties (`wps:spPr` or `pic:spPr`) and style
    /// (`wps:style`), as written: fill, outline, rotation.
    pub fn shape_xml(&self) -> (Option<&str>, Option<&str>) {
        let choice = choice_part(&self.xml);
        let sp = element_range(choice, "spPr").map(|r| &choice[r]);
        let style = element_range(choice, "style").map(|r| &choice[r]);
        (sp, style)
    }
}

/// Give the text boxes in `paras` the main document's styles, lists and
/// pictures (they're read before those are known).
pub(crate) fn share_parts(paras: &mut [crate::Paragraph], doc: &Document) {
    for p in paras {
        for run in &mut p.runs {
            let Some(crate::Object::Float(f)) = &run.object else {
                continue;
            };
            if f.text.is_none() && f.boxes.is_empty() {
                continue;
            }
            let share = |tb: &Arc<TextBox>| {
                let mut inner = tb.doc.clone();
                inner.styles = doc.styles.clone();
                inner.numbering = doc.numbering.clone();
                inner.media = doc.media.clone();
                inner.default_tab = doc.default_tab;
                inner.decimal = doc.decimal;
                Arc::new(TextBox {
                    doc: inner,
                    ..(**tb).clone()
                })
            };
            let mut nf = (**f).clone();
            nf.text = f.text.as_ref().map(share);
            for b in &mut nf.boxes {
                b.text = share(&b.text);
            }
            run.object = Some(crate::Object::Float(Arc::new(nf)));
        }
    }
}

/// Byte offset of the next start tag with local name `name`.
fn next_start_tag(src: &str, name: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(i) = src[from..].find(name).map(|i| from + i) {
        from = i + name.len();
        let lt = src[..i].rfind('<')?;
        let tag = &src[lt + 1..i];
        let after = src[from..].chars().next();
        if !tag.starts_with('/')
            && (tag.is_empty() || (tag.ends_with(':') && !tag.contains(char::is_whitespace)))
            && matches!(after, Some(' ' | '>' | '/' | '\t' | '\n' | '\r'))
        {
            return Some(lt);
        }
    }
    None
}

/// `src` with the inside of every `w:txbxContent` (the shape's and its
/// VML fallback's) replaced by `body`.
fn replace_text_box(src: &str, body: &str) -> String {
    let mut out = String::with_capacity(src.len() + body.len());
    let mut rest = src;
    // The next start tag (searching from inside an element, so not with a
    // reader: its end tags wouldn't match).
    while let Some(lt) = next_start_tag(rest, "txbxContent") {
        out.push_str(&rest[..lt]);
        rest = &rest[lt..];
        let Some(r) = element_range(rest, "txbxContent") else {
            break;
        };
        let whole = &rest[r.clone()];
        let open_end = whole.find('>').map_or(whole.len(), |i| i + 1);
        let open = &whole[..open_end];
        let qname = open
            .trim_start_matches('<')
            .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .next()
            .unwrap_or("w:txbxContent")
            .to_owned();
        out.push_str(&rest[..r.start]);
        if open.ends_with("/>") {
            out.push_str(open.trim_end_matches("/>"));
            out.push('>');
        } else {
            out.push_str(open);
        }
        out.push_str(body);
        out.push_str(&format!("</{qname}>"));
        rest = &rest[r.end..];
    }
    out.push_str(rest);
    out
}

/// `src` with the inside of the `n`th `w:txbxContent` of the shape's
/// XML (in `mc:Choice`) and the `n`th of its VML fallback replaced by
/// `body`.
fn replace_nth_text_box(src: &str, n: usize, body: &str) -> String {
    // Start tags of every text box, and where the fallback begins.
    let mut starts = Vec::new();
    let mut at = 0;
    while let Some(i) = next_start_tag(&src[at..], "txbxContent") {
        starts.push(at + i);
        at += i + src[at + i..].find('>').map_or(1, |e| e + 1);
    }
    let fallback = next_start_tag(src, "Fallback").unwrap_or(src.len());
    let (choice, fb): (Vec<usize>, Vec<usize>) = starts.iter().partition(|&&s| s < fallback);
    let mut targets: Vec<usize> = [choice.get(n), fb.get(n)]
        .into_iter()
        .flatten()
        .copied()
        .collect();
    targets.sort_unstable();
    let mut out = String::with_capacity(src.len() + body.len());
    let mut done = 0;
    for t in targets {
        let Some(r) = element_range(&src[t..], "txbxContent") else {
            continue;
        };
        let whole = &src[t..t + r.end];
        out.push_str(&src[done..t]);
        let replaced = replace_text_box(whole, body);
        out.push_str(&replaced);
        done = t + r.end;
    }
    out.push_str(&src[done..]);
    out
}

/// The text boxes of a group's or canvas's shapes (`wps:wsp` with a
/// `wps:txbx`), in the order they're written: their content, insets and
/// vertical anchor (as [`text_box_xml`]).
pub(crate) fn group_text_boxes(xml_src: &str) -> Vec<(&str, [i64; 4], FloatAlign)> {
    let choice = choice_part(xml_src);
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = next_start_tag(&choice[at..], "wsp") {
        let s = at + i;
        let Some(r) = element_range(&choice[s..], "wsp") else {
            break;
        };
        let wsp = &choice[s..s + r.end];
        at = s + r.end.max(1);
        if next_start_tag(wsp, "txbx").is_none() {
            continue;
        }
        if let Some(tb) = text_box_xml(wsp) {
            out.push(tb);
        }
    }
    out
}

// ── editing ────────────────────────────────────────────────────────────────

impl Document {
    /// Replace a text box's text (`\n` separates paragraphs): one undo
    /// step. Each paragraph keeps the formatting of the box's first
    /// paragraph and first text run.
    pub fn set_text_box_text(&mut self, para: usize, index: usize, text: &str) -> bool {
        self.set_text_box_text_in(para, index, None, text)
    }

    /// [`Document::set_text_box_text`] for the text box `which`
    /// ([`Float::text_box_in`]: a group's shapes have one each).
    pub fn set_text_box_text_in(
        &mut self,
        para: usize,
        index: usize,
        which: Option<usize>,
        text: &str,
    ) -> bool {
        let Some(f) = self.float_at(para, index) else {
            return false;
        };
        if f.text_locked_in(which).is_some() || f.text_box_text_in(which).is_none_or(|t| t == text)
        {
            return false;
        }
        let tb = f.text_box_in(which).cloned().expect("text box");
        let doc = match self.track {
            true => self.tracked_text_box(
                &tb.doc,
                &f.text_box_text_in(which).unwrap_or_default(),
                text,
            ),
            false => None,
        };
        let doc = doc.unwrap_or_else(|| {
            let first = tb.doc.paragraphs().first().cloned().unwrap_or_default();
            let props = first
                .runs
                .iter()
                .find(|r| r.object.is_none())
                .map(|r| r.props.clone())
                .unwrap_or_default();
            let paras: Vec<crate::Paragraph> = text
                .split('\n')
                .map(|line| {
                    let mut p = crate::Paragraph {
                        props: first.props.clone(),
                        ..crate::Paragraph::styled(line, props.clone())
                    };
                    p.props.extras.remove("sectPr");
                    p
                })
                .collect();
            let mut doc = tb.doc.clone();
            doc.paragraphs = paras;
            doc.tables.clear();
            doc
        });
        let text = Arc::new(TextBox {
            doc,
            ..(*tb).clone()
        });
        let mut nf = f.clone();
        match which {
            None => {
                nf.text = Some(text);
                nf.text_edited = true;
            }
            Some(k) => {
                nf.boxes[k] = GroupBox { text, edited: true };
            }
        }
        self.swap_object(para, index, Some(crate::Object::Float(Arc::new(nf))))
    }

    /// A text box's document with `old` (its text) changed to `new` as a
    /// tracked edit by the user: the part that differs struck through and
    /// the new text inserted. `None` when its text has objects in it.
    fn tracked_text_box(&self, doc: &Document, old: &str, new: &str) -> Option<Document> {
        if doc
            .paragraphs
            .iter()
            .any(|p| p.text.contains(crate::OBJECT))
        {
            return None;
        }
        // The common start and end (on character boundaries).
        let pre = old
            .char_indices()
            .zip(new.chars())
            .find(|((_, a), b)| a != b)
            .map_or(old.len().min(new.len()), |((i, _), _)| i);
        let mut pre = (0..=pre)
            .rev()
            .find(|&i| old.is_char_boundary(i) && new.is_char_boundary(i))?;
        let max_suf = (old.len() - pre).min(new.len() - pre);
        let mut suf = old[old.len() - max_suf..]
            .chars()
            .rev()
            .zip(new[new.len() - max_suf..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a.len_utf8())
            .sum::<usize>();
        // Whole words, as a reader sees the change.
        while let Some(c) = old[..pre]
            .chars()
            .next_back()
            .filter(|c| c.is_alphanumeric())
        {
            pre -= c.len_utf8();
        }
        while let Some(c) = old[old.len() - suf..]
            .chars()
            .next()
            .filter(|c| c.is_alphanumeric())
        {
            suf -= c.len_utf8();
        }
        let at = |byte: usize| {
            let mut left = byte;
            for (i, line) in old.split('\n').enumerate() {
                if left <= line.len() {
                    return crate::Position::new(i, left);
                }
                left -= line.len() + 1;
            }
            doc.end()
        };
        let mut d = doc.clone();
        d.track = true;
        d.author = self.author.clone();
        let sel = crate::Selection {
            anchor: at(pre),
            head: at(old.len() - suf),
        };
        let middle = &new[pre..new.len() - suf];
        if middle.is_empty() {
            d.delete_selection(sel);
        } else {
            d.insert_text(sel, middle, None);
        }
        d.clear_history();
        Some(d)
    }

    /// The floating object whose character is at `para`/`index`.
    pub fn float_at(&self, para: usize, index: usize) -> Option<&Float> {
        self.paragraphs
            .get(para)?
            .object_at(index)
            .and_then(crate::Object::float)
    }

    /// Replace the object at `para`/`index` (undoable).
    fn swap_object(&mut self, para: usize, index: usize, object: Option<crate::Object>) -> bool {
        let Some(p) = self.paragraphs.get(para) else {
            return false;
        };
        if p.object_at(index).is_none() {
            return false;
        }
        let mut p = p.clone();
        let i = p.split_run_at(index);
        match object {
            Some(o) => p.runs[i].object = Some(o),
            None => {
                p.text
                    .replace_range(index..index + crate::OBJECT.len_utf8(), "");
                p.runs.remove(i);
                if p.runs.is_empty() {
                    p.runs.push(crate::Run::text(0, Default::default()));
                }
            }
        }
        p.normalize();
        let sel = crate::Selection::caret(crate::Position::new(para, index));
        self.replace(
            para,
            1,
            vec![p],
            (sel, sel),
            crate::document::Kind::Other,
            true,
        );
        true
    }

    /// Give the object at `para`/`index` edited package parts
    /// ([`Float::parts`]; a part already there is replaced): one undo step.
    pub fn set_float_parts(
        &mut self,
        para: usize,
        index: usize,
        parts: Vec<(String, Arc<str>)>,
    ) -> bool {
        let Some(f) = self.float_at(para, index) else {
            return false;
        };
        let mut nf = f.clone();
        for (name, xml) in parts {
            match nf.parts.iter_mut().find(|p| p.0 == name) {
                Some(p) => p.1 = xml,
                None => nf.parts.push((name, xml)),
            }
        }
        if nf.parts == f.parts {
            return false;
        }
        self.swap_object(para, index, Some(crate::Object::Float(Arc::new(nf))))
    }

    /// A part's XML as the document has it now: edited with a floating
    /// object ([`Float::parts`], body, headers and footers), else as read.
    pub fn current_part_xml(&self, name: &str) -> Option<String> {
        self.edited_parts()
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, x)| x.to_string())
            .or_else(|| self.part_xml(name))
    }

    /// Every package part edited with a floating object (the body's
    /// first, then headers' and footers').
    pub(crate) fn edited_parts(&self) -> Vec<(String, Arc<str>)> {
        let mut out = Vec::new();
        let docs = std::iter::once(self).chain(self.stories().iter().map(|s| &s.doc));
        for d in docs {
            for p in d.paragraphs() {
                for (_, o) in p.objects() {
                    if let crate::Object::Float(f) = o {
                        out.extend(f.parts.iter().cloned());
                    }
                }
            }
        }
        out
    }

    /// Move or resize a floating object and give it edited package parts
    /// at once (a SmartArt laid out again for its new size): one undo step.
    pub fn set_float_placement_parts(
        &mut self,
        para: usize,
        index: usize,
        at: Placement,
        parts: Vec<(String, Arc<str>)>,
    ) -> bool {
        let Some(f) = self.float_at(para, index) else {
            return false;
        };
        let mut nf = f.placed(at);
        for (name, xml) in parts {
            match nf.parts.iter_mut().find(|p| p.0 == name) {
                Some(p) => p.1 = xml,
                None => nf.parts.push((name, xml)),
            }
        }
        if nf.at == f.at && nf.parts == f.parts {
            return false;
        }
        self.swap_object(para, index, Some(crate::Object::Float(Arc::new(nf))))
    }

    /// Move, resize or rewrap a floating object (one undo step).
    pub fn set_float_placement(&mut self, para: usize, index: usize, at: Placement) -> bool {
        let Some(f) = self.float_at(para, index) else {
            return false;
        };
        if f.at == at {
            return false;
        }
        let o = crate::Object::Float(Arc::new(f.placed(at)));
        self.swap_object(para, index, Some(o))
    }

    /// Delete a floating object. With Track Changes on it's marked
    /// deleted instead (its anchor in a `w:del`, as Word does), unless the
    /// user inserted it with tracking on.
    pub fn delete_float(&mut self, para: usize, index: usize) -> bool {
        if self.float_at(para, index).is_none() {
            return false;
        }
        if self.track {
            let mut p = self.paragraphs[para].clone();
            let i = p.split_run_at(index);
            let own = p.runs[i]
                .rev
                .as_ref()
                .is_some_and(|r| r.kind == crate::RevKind::Insert && r.author == self.author);
            if !own {
                if p.runs[i]
                    .rev
                    .as_ref()
                    .is_some_and(|r| r.kind == crate::RevKind::Delete)
                {
                    return false;
                }
                p.runs[i].rev = Some(self.new_rev(crate::RevKind::Delete, self.max_rev_id() + 1));
                p.normalize();
                let sel = crate::Selection::caret(crate::Position::new(para, index));
                self.replace(
                    para,
                    1,
                    vec![p],
                    (sel, sel),
                    crate::document::Kind::Other,
                    true,
                );
                return true;
            }
        }
        self.swap_object(para, index, None)
    }

    /// The Layout Options choice of the object at `para`/`index`: inline
    /// pictures are [`LayoutOption::Inline`].
    pub fn layout_option(&self, para: usize, index: usize) -> Option<LayoutOption> {
        match self.paragraphs.get(para)?.object_at(index)? {
            crate::Object::Image(_) => Some(LayoutOption::Inline),
            crate::Object::Float(f) => Some(f.at.layout_option()),
            _ => None,
        }
    }

    /// Whether the object can be put in line with text (pictures can;
    /// shapes and text boxes stay floating in Kantor).
    pub fn can_be_inline(&self, para: usize, index: usize) -> bool {
        match self.paragraphs.get(para).and_then(|p| p.object_at(index)) {
            Some(crate::Object::Image(_)) => true,
            Some(crate::Object::Float(f)) => matches!(f.kind, FloatKind::Picture { .. }),
            _ => false,
        }
    }

    /// Word's Layout Options for a picture or floating object: in line
    /// with text, or floating with a wrap. An inline picture made floating
    /// starts where it stood (column and paragraph offsets, as Word does);
    /// a floating picture put in line loses its crop.
    pub fn set_layout_option(&mut self, para: usize, index: usize, o: LayoutOption) -> bool {
        let Some(obj) = self.paragraphs.get(para).and_then(|p| p.object_at(index)) else {
            return false;
        };
        match (obj, o) {
            (crate::Object::Image(_), LayoutOption::Inline) => false,
            (crate::Object::Image(img), o) => {
                let img = img.clone();
                let mut at = Placement {
                    h: (RelH::Column, Place::Offset(0)),
                    v: (RelV::Paragraph, Place::Offset(0)),
                    cx: img.cx,
                    cy: img.cy,
                    dist: [0, 0, 114300, 114300],
                    wrap: Wrap::Square,
                    side: WrapSide::Both,
                    behind: false,
                    allow_overlap: true,
                    z: self.top_z() + 1,
                    in_cell: true,
                };
                at.set_layout_option(o);
                let xml = anchor_picture_xml(&img, &at);
                let Some(mut f) = read(&xml, &|_| Some(img.part.clone())) else {
                    return false;
                };
                // Read from the XML just made: it's what gets written.
                f.read = f.at;
                self.swap_object(para, index, Some(crate::Object::Float(Arc::new(f))))
            }
            (crate::Object::Float(f), LayoutOption::Inline) => {
                let FloatKind::Picture { rel_id, part, .. } = &f.kind else {
                    return false;
                };
                let img = crate::Image {
                    rel_id: rel_id.clone(),
                    cx: f.at.cx,
                    cy: f.at.cy,
                    descr: f.descr.clone(),
                    xml: None,
                    doc_pr: doc_pr_id(&f.xml).unwrap_or(self.top_doc_pr() + 1),
                    part: part.clone(),
                };
                self.swap_object(para, index, Some(crate::Object::Image(Arc::new(img))))
            }
            (crate::Object::Float(f), o) => {
                let mut at = f.at;
                at.set_layout_option(o);
                self.set_float_placement(para, index, at)
            }
            _ => false,
        }
    }

    /// Highest z-order of the document's floating objects.
    fn top_z(&self) -> u32 {
        self.paragraphs
            .iter()
            .flat_map(|p| p.objects())
            .filter_map(|(_, o)| o.float().map(|f| f.at.z))
            .max()
            .unwrap_or(251_658_240)
    }

    fn top_doc_pr(&self) -> u32 {
        self.paragraphs
            .iter()
            .flat_map(|p| p.objects())
            .filter_map(|(_, o)| match o {
                crate::Object::Float(f) => doc_pr_id(&f.xml),
                crate::Object::Image(i) => Some(i.doc_pr),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    }
}

/// `wp:docPr/@id` of a drawing.
fn doc_pr_id(xml_src: &str) -> Option<u32> {
    let r = element_range(xml_src, "docPr")?;
    xml::parse(&xml_src[r])?.num("id")
}

/// A floating picture's `w:drawing` for an inline picture.
fn anchor_picture_xml(img: &crate::Image, at: &Placement) -> String {
    let inline = crate::docx::image_xml(img);
    // Same graphic; an anchor instead of the inline frame.
    let (Some(g0), Some(g1)) = (inline.find("<a:graphic "), inline.rfind("</a:graphic>")) else {
        return inline;
    };
    let graphic = &inline[g0..g1 + "</a:graphic>".len()];
    let doc_pr = element_range(&inline, "docPr").map_or("", |r| &inline[r]);
    let frame = element_range(&inline, "cNvGraphicFramePr").map_or("", |r| &inline[r]);
    let wp = "xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\"";
    let [t, b, l, r] = at.dist;
    let mut s = format!(
        "<w:drawing><wp:anchor distT=\"{t}\" distB=\"{b}\" distL=\"{l}\" distR=\"{r}\" \
         simplePos=\"0\" relativeHeight=\"{z}\" behindDoc=\"{behind}\" locked=\"0\" \
         layoutInCell=\"1\" allowOverlap=\"1\" {wp}><wp:simplePos x=\"0\" y=\"0\"/>",
        z = at.z,
        behind = at.behind as u8,
    );
    s.push_str(&position_xml(
        "wp:",
        "positionH",
        at.h.0.as_str(),
        at.h.1,
        true,
    ));
    s.push_str(&position_xml(
        "wp:",
        "positionV",
        at.v.0.as_str(),
        at.v.1,
        false,
    ));
    s.push_str(&format!(
        "<wp:extent cx=\"{}\" cy=\"{}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>",
        at.cx, at.cy
    ));
    s.push_str(&wrap_xml("wp:", at));
    s.push_str(doc_pr);
    s.push_str(frame);
    s.push_str(graphic);
    s.push_str("</wp:anchor></w:drawing>");
    s
}

/// The part of an `mc:AlternateContent` Word reads (its first choice),
/// or the whole XML.
/// Every `r:embed` of an `a:blip` in a fragment.
pub(crate) fn blip_rels(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(i) = src[at..].find(":blip ") {
        let s = at + i;
        let e = src[s..].find('>').map_or(src.len(), |e| s + e);
        let tag = &src[s..e];
        if let Some(k) = tag.find("r:embed=\"") {
            let v = k + "r:embed=\"".len();
            if let Some(q) = tag[v..].find('"') {
                let id = tag[v..v + q].to_owned();
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        at = e;
    }
    out
}

fn choice_part(xml: &str) -> &str {
    match element_range(xml, "Choice") {
        Some(r) => &xml[r],
        None => xml,
    }
}

/// Byte range of the first element with local name `name` (start tag to
/// end tag) in a fragment.
pub(crate) fn element_range(src: &str, name: &str) -> Option<std::ops::Range<usize>> {
    let mut r = quick_xml::Reader::from_str(src);
    let mut depth = 0usize;
    let mut start: Option<(usize, usize)> = None;
    loop {
        let before = r.buffer_position() as usize;
        let ev = r.read_event().ok()?;
        let after = r.buffer_position() as usize;
        match ev {
            Event::Start(e) => {
                depth += 1;
                if start.is_none() && local(e.name().as_ref()) == name {
                    start = Some((before, depth));
                }
            }
            Event::Empty(e) => {
                if start.is_none() && local(e.name().as_ref()) == name {
                    return Some(before..after);
                }
            }
            Event::End(_) => {
                if let Some((s, d)) = start
                    && d == depth
                {
                    return Some(s..after);
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

fn local(name: &[u8]) -> &str {
    let s = std::str::from_utf8(name).unwrap_or("");
    s.rsplit_once(':').map_or(s, |(_, l)| l)
}

fn place_of(pos: &El, horizontal: bool) -> Place {
    for c in &pos.children {
        match c.name.as_str() {
            "align" => return Place::Align(FloatAlign::parse(&c.text)),
            "posOffset" => return Place::Offset(c.text.trim().parse().unwrap_or(0)),
            "pctPosHOffset" if horizontal => {
                return Place::Pct(c.text.trim().parse().unwrap_or(0));
            }
            "pctPosVOffset" if !horizontal => {
                return Place::Pct(c.text.trim().parse().unwrap_or(0));
            }
            _ => {}
        }
    }
    Place::Offset(0)
}

/// `wp:positionH` / `positionV`: directly in the anchor, or (Word 2010's
/// percentage positions) in an `mc:AlternateContent` whose choice Word
/// reads.
fn position<'e>(anchor: &'e El, name: &str) -> Option<&'e El> {
    for c in &anchor.children {
        if c.name == name {
            return Some(c);
        }
        if c.name == "AlternateContent" {
            let found = c
                .children
                .iter()
                .find_map(|b| b.children.iter().find(|x| x.name == name));
            if found.is_some() {
                return found;
            }
        }
    }
    None
}

/// Read the placement of a `wp:anchor` element.
pub(crate) fn placement(anchor: &El) -> Placement {
    let n = |k: &str| anchor.num::<i64>(k).unwrap_or(0);
    let on = |k: &str| matches!(anchor.attr(k), Some("1" | "true" | "on"));
    let simple = on("simplePos");
    let (mut h, mut v) = (
        position(anchor, "positionH").map_or((RelH::Column, Place::Offset(0)), |p| {
            (
                RelH::parse(p.attr("relativeFrom").unwrap_or("column")),
                place_of(p, true),
            )
        }),
        position(anchor, "positionV").map_or((RelV::Paragraph, Place::Offset(0)), |p| {
            (
                RelV::parse(p.attr("relativeFrom").unwrap_or("paragraph")),
                place_of(p, false),
            )
        }),
    );
    if simple && let Some(sp) = anchor.child("simplePos") {
        h = (RelH::Page, Place::Offset(sp.num("x").unwrap_or(0)));
        v = (RelV::Page, Place::Offset(sp.num("y").unwrap_or(0)));
    }
    let extent = anchor.child("extent");
    let (mut wrap, mut side) = (Wrap::None, WrapSide::Both);
    for c in &anchor.children {
        let w = match c.name.as_str() {
            "wrapSquare" => Wrap::Square,
            "wrapTight" => Wrap::Tight,
            "wrapThrough" => Wrap::Through,
            "wrapTopAndBottom" => Wrap::TopAndBottom,
            "wrapNone" => Wrap::None,
            _ => continue,
        };
        wrap = w;
        side = WrapSide::parse(c.attr("wrapText"));
        break;
    }
    Placement {
        h,
        v,
        cx: extent.and_then(|e| e.num("cx")).unwrap_or(0),
        cy: extent.and_then(|e| e.num("cy")).unwrap_or(0),
        dist: [n("distT"), n("distB"), n("distL"), n("distR")],
        wrap,
        side,
        behind: on("behindDoc"),
        allow_overlap: anchor.attr("allowOverlap").is_none_or(|v| v != "0"),
        z: anchor.num("relativeHeight").unwrap_or(0),
        in_cell: on("layoutInCell"),
    }
}

/// First descendant with local name `name`.
fn find<'e>(el: &'e El, name: &str) -> Option<&'e El> {
    el.children.iter().find_map(|c| {
        if c.name == name {
            Some(c)
        } else {
            find(c, name)
        }
    })
}

/// A floating object from a run's `w:drawing` or `mc:AlternateContent`;
/// `None` when it holds no `wp:anchor`. `rel_part` resolves a picture's
/// relationship id to its part name. The text box is the caller's
/// ([`Float::text`]).
pub(crate) fn read(xml_src: &str, rel_part: &dyn Fn(&str) -> Option<String>) -> Option<Float> {
    let root = xml::parse(xml_src)?;
    // In mc:AlternateContent, only the choice (the fallback is VML).
    let scope = if root.name == "AlternateContent" {
        let choice = root.children.iter().find(|c| c.name == "Choice")?;
        choice.children.iter().find(|c| c.name == "drawing")?
    } else if root.name == "drawing" {
        &root
    } else {
        return None;
    };
    let anchor = scope.children.iter().find(|c| c.name == "anchor")?;
    let at = placement(anchor);
    let doc_pr = anchor.child("docPr");
    let data = find(anchor, "graphicData");
    let uri = data.and_then(|d| d.attr("uri")).unwrap_or("");
    let kind = if let Some(pic) = data.and_then(|d| d.child("pic")) {
        let blip = find(pic, "blip");
        match blip.and_then(|b| b.attr("embed")) {
            Some(rel) => {
                let crop = find(pic, "srcRect").map_or([0; 4], |s| {
                    ["l", "t", "r", "b"].map(|k| s.num::<i32>(k).unwrap_or(0))
                });
                FloatKind::Picture {
                    rel_id: rel.to_owned(),
                    part: rel_part(rel).unwrap_or_default(),
                    crop,
                }
            }
            None => FloatKind::Other("Picture".into()),
        }
    } else if data.and_then(|d| d.child("wsp")).is_some() {
        FloatKind::Shape
    } else if uri.ends_with("/chart") || uri.ends_with("/chartex") {
        // ChartML, or the Office 2016 types (`cx:chart`).
        match data
            .and_then(|d| find(d, "chart"))
            .and_then(|c| c.attr("id"))
        {
            Some(rel) => FloatKind::Chart {
                rel_id: rel.to_owned(),
                part: rel_part(rel).unwrap_or_default(),
            },
            None => FloatKind::Other("Chart".into()),
        }
    } else if uri.contains("chartex") {
        // Office 2016 charts (waterfall, treemap…): not drawn.
        FloatKind::Other("Chart".into())
    } else if uri.contains("wordprocessingGroup") {
        FloatKind::Group { canvas: false }
    } else if uri.contains("wordprocessingCanvas") {
        FloatKind::Group { canvas: true }
    } else if uri.contains("diagram") {
        let ids = data.and_then(|d| find(d, "relIds"));
        match ids.and_then(|c| c.attr("dm")).and_then(rel_part) {
            Some(part) => FloatKind::SmartArt {
                data: part,
                drawings: Vec::new(),
                layout: ids.and_then(|c| c.attr("lo")).and_then(rel_part),
            },
            None => FloatKind::Other("SmartArt".into()),
        }
    } else {
        FloatKind::Other("Drawing".into())
    };
    Some(Float {
        xml: xml_src.into(),
        read: at,
        at,
        kind,
        name: doc_pr
            .and_then(|d| d.attr("name"))
            .unwrap_or_default()
            .to_owned(),
        descr: doc_pr
            .and_then(|d| d.attr("descr"))
            .unwrap_or_default()
            .to_owned(),
        text: None,
        text_edited: false,
        boxes: Vec::new(),
        parts: Vec::new(),
    })
}

/// The inside of a shape's `w:txbxContent` (block content), its
/// `a:bodyPr` insets and vertical anchor.
pub(crate) fn text_box_xml(xml_src: &str) -> Option<(&str, [i64; 4], FloatAlign)> {
    let choice = choice_part(xml_src);
    let r = element_range(choice, "txbxContent")?;
    let whole = &choice[r];
    let open_end = whole.find('>')? + 1;
    if whole[..open_end].ends_with("/>") {
        return Some(("", [91440, 45720, 91440, 45720], FloatAlign::Start));
    }
    let close = whole.rfind("</")?;
    let inner = &whole[open_end..close];
    let body = element_range(choice, "bodyPr")
        .and_then(|r| xml::parse(&choice[r]))
        .unwrap_or_default();
    let ins = |k: &str, d: i64| body.num::<i64>(k).unwrap_or(d);
    let insets = [
        ins("lIns", 91440),
        ins("tIns", 45720),
        ins("rIns", 91440),
        ins("bIns", 45720),
    ];
    let v_align = match body.attr("anchor") {
        Some("ctr") => FloatAlign::Center,
        Some("b") => FloatAlign::End,
        _ => FloatAlign::Start,
    };
    Some((inner, insets, v_align))
}

/// Whether a shape's text box resizes the shape to fit its text
/// (`a:bodyPr/a:spAutoFit`; Word's VML fallback says `mso-fit-shape-to-text`).
pub(crate) fn text_box_auto_fit(xml_src: &str) -> bool {
    let choice = choice_part(xml_src);
    element_range(choice, "bodyPr")
        .is_some_and(|r| next_start_tag(&choice[r], "spAutoFit").is_some())
}

// ── writing ────────────────────────────────────────────────────────────────

/// A direct child of the anchor: local name (an `mc:AlternateContent`
/// wrapping a position counts as that position), byte range.
struct Child {
    name: String,
    range: std::ops::Range<usize>,
}

/// The anchor's start tag range, its children and its end tag range.
type AnchorParts = (
    std::ops::Range<usize>,
    String,
    Vec<Child>,
    std::ops::Range<usize>,
);

fn anchor_parts(src: &str) -> Option<AnchorParts> {
    let mut r = quick_xml::Reader::from_str(src);
    let mut depth = 0usize;
    let mut anchor: Option<(std::ops::Range<usize>, String, usize)> = None;
    let mut children = Vec::new();
    let mut child: Option<(String, usize)> = None;
    loop {
        let before = r.buffer_position() as usize;
        let ev = r.read_event().ok()?;
        let after = r.buffer_position() as usize;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(ev, Event::Empty(_));
                let name = local(e.name().as_ref()).to_owned();
                if !empty {
                    depth += 1;
                }
                match &anchor {
                    None if name == "anchor" && !empty => {
                        let qname = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                        anchor = Some((before..after, qname, depth));
                    }
                    Some((_, _, d)) if child.is_none() => {
                        let at_child = if empty { depth + 1 } else { depth };
                        if at_child == d + 1 {
                            if empty {
                                children.push(Child {
                                    name,
                                    range: before..after,
                                });
                            } else {
                                child = Some((name, before));
                            }
                        }
                    }
                    _ => {}
                }
            }
            Event::End(_) => {
                if let Some((start, qname, d)) = &anchor {
                    if depth == *d {
                        return Some((start.clone(), qname.clone(), children, before..after));
                    }
                    if depth == d + 1
                        && let Some((name, s)) = child.take()
                    {
                        let mut name = name;
                        if name == "AlternateContent" {
                            let inner = &src[s..after];
                            if inner.contains("positionH") {
                                name = "positionH".into();
                            } else if inner.contains("positionV") {
                                name = "positionV".into();
                            }
                        }
                        children.push(Child {
                            name,
                            range: s..after,
                        });
                    }
                }
                depth = depth.saturating_sub(1);
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

fn position_xml(prefix: &str, name: &str, rel: &str, place: Place, horizontal: bool) -> String {
    let inner = match place {
        Place::Align(a) => format!("<{prefix}align>{}</{prefix}align>", a.as_str(horizontal)),
        Place::Offset(o) => format!("<{prefix}posOffset>{o}</{prefix}posOffset>"),
        // Percentages need wp14; written as an offset instead (the caller
        // converts), this is only a fallback.
        Place::Pct(_) => format!("<{prefix}posOffset>0</{prefix}posOffset>"),
    };
    format!("<{prefix}{name} relativeFrom=\"{rel}\">{inner}</{prefix}{name}>")
}

fn wrap_xml(prefix: &str, at: &Placement) -> String {
    let side = at.side.as_str();
    let polygon = format!(
        "<{prefix}wrapPolygon edited=\"0\"><{prefix}start x=\"0\" y=\"0\"/>\
         <{prefix}lineTo x=\"0\" y=\"21600\"/><{prefix}lineTo x=\"21600\" y=\"21600\"/>\
         <{prefix}lineTo x=\"21600\" y=\"0\"/><{prefix}lineTo x=\"0\" y=\"0\"/>\
         </{prefix}wrapPolygon>"
    );
    match at.wrap {
        Wrap::None => format!("<{prefix}wrapNone/>"),
        Wrap::Square => format!("<{prefix}wrapSquare wrapText=\"{side}\"/>"),
        Wrap::Tight => {
            format!("<{prefix}wrapTight wrapText=\"{side}\">{polygon}</{prefix}wrapTight>")
        }
        Wrap::Through => {
            format!("<{prefix}wrapThrough wrapText=\"{side}\">{polygon}</{prefix}wrapThrough>")
        }
        Wrap::TopAndBottom => format!("<{prefix}wrapTopAndBottom/>"),
    }
}

/// The anchor start tag with the placement's attributes.
fn start_tag(tag: &str, at: &Placement, positioned: bool) -> String {
    let inner = tag
        .trim_start_matches('<')
        .trim_end_matches('>')
        .trim_end_matches('/');
    let e = BytesStart::from_content(
        inner,
        inner.find(char::is_whitespace).unwrap_or(inner.len()),
    );
    let qname = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let mut out = format!("<{qname}");
    let mut seen: Vec<String> = Vec::new();
    let value = |k: &str| -> Option<String> {
        Some(match k {
            "distT" => at.dist[0].to_string(),
            "distB" => at.dist[1].to_string(),
            "distL" => at.dist[2].to_string(),
            "distR" => at.dist[3].to_string(),
            "relativeHeight" => at.z.to_string(),
            "behindDoc" => (at.behind as u8).to_string(),
            "allowOverlap" => (at.allow_overlap as u8).to_string(),
            "simplePos" if positioned => "0".into(),
            _ => return None,
        })
    };
    for a in e.attributes().flatten() {
        let k = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let raw = String::from_utf8_lossy(&a.value).into_owned();
        let v = value(&k).unwrap_or(raw);
        out.push_str(&format!(" {k}=\"{v}\""));
        seen.push(k);
    }
    // Attributes the schema requires, if the original lacked them.
    for k in [
        "distT",
        "distB",
        "distL",
        "distR",
        "simplePos",
        "relativeHeight",
        "behindDoc",
        "locked",
        "layoutInCell",
        "allowOverlap",
    ] {
        if !seen.iter().any(|s| s == k) {
            let v = value(k).unwrap_or_else(|| "0".into());
            let v = if k == "layoutInCell" { "1".into() } else { v };
            out.push_str(&format!(" {k}=\"{v}\""));
        }
    }
    out.push('>');
    out
}

/// Set `cx`/`cy` on the first `a:ext` inside the first `a:xfrm` of a
/// graphic (the shape's or picture's own size).
fn patch_graphic_ext(graphic: &str, cx: i64, cy: i64) -> String {
    let Some(x) = element_range(graphic, "xfrm") else {
        return graphic.to_owned();
    };
    let Some(e) = element_range(&graphic[x.clone()], "ext") else {
        return graphic.to_owned();
    };
    let (s, t) = (x.start + e.start, x.start + e.end);
    let tag = &graphic[s..t];
    let tag = set_attr(&set_attr(tag, "cx", &cx.to_string()), "cy", &cy.to_string());
    format!("{}{}{}", &graphic[..s], tag, &graphic[t..])
}

/// Replace attribute `k`'s value in a start tag.
fn set_attr(tag: &str, k: &str, v: &str) -> String {
    let pat = format!(" {k}=\"");
    let Some(i) = tag.find(&pat) else {
        return tag.to_owned();
    };
    let start = i + pat.len();
    let Some(len) = tag[start..].find('"') else {
        return tag.to_owned();
    };
    format!("{}{}{}", &tag[..start], v, &tag[start + len..])
}

/// `src` with the anchor placement `at` written in (`read` is what the XML
/// says, so only what changed is rewritten).
fn patch(src: &str, read: &Placement, at: &Placement) -> Option<String> {
    let (start, qname, children, end) = anchor_parts(src)?;
    let prefix = qname
        .rsplit_once(':')
        .map_or(String::new(), |(p, _)| format!("{p}:"));
    let moved_h = at.h != read.h;
    let moved_v = at.v != read.v;
    let resized = (at.cx, at.cy) != (read.cx, read.cy);
    let rewrapped = (at.wrap, at.side) != (read.wrap, read.side);
    let mut out = String::with_capacity(src.len() + 128);
    out.push_str(&src[..start.start]);
    out.push_str(&start_tag(&src[start.clone()], at, moved_h || moved_v));
    let mut wrote_wrap = false;
    for c in &children {
        let orig = &src[c.range.clone()];
        match c.name.as_str() {
            "positionH" if moved_h => out.push_str(&position_xml(
                &prefix,
                "positionH",
                at.h.0.as_str(),
                at.h.1,
                true,
            )),
            "positionV" if moved_v => out.push_str(&position_xml(
                &prefix,
                "positionV",
                at.v.0.as_str(),
                at.v.1,
                false,
            )),
            "extent" if resized => out.push_str(&format!(
                "<{prefix}extent cx=\"{}\" cy=\"{}\"/>",
                at.cx, at.cy
            )),
            "wrapNone" | "wrapSquare" | "wrapTight" | "wrapThrough" | "wrapTopAndBottom" => {
                wrote_wrap = true;
                if rewrapped {
                    out.push_str(&wrap_xml(&prefix, at));
                } else {
                    out.push_str(orig);
                }
            }
            "docPr" if !wrote_wrap => {
                // The schema puts a wrap element before docPr.
                wrote_wrap = true;
                out.push_str(&wrap_xml(&prefix, at));
                out.push_str(orig);
            }
            "graphic" if resized => out.push_str(&patch_graphic_ext(orig, at.cx, at.cy)),
            _ => out.push_str(orig),
        }
    }
    out.push_str(&src[end.start..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WP: &str = "xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\" xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

    fn drawing(position: &str, wrap: &str) -> String {
        format!(
            "<w:drawing {WP}><wp:anchor distT=\"0\" distB=\"0\" distL=\"114300\" distR=\"114300\" \
             simplePos=\"0\" relativeHeight=\"251659264\" behindDoc=\"0\" locked=\"0\" \
             layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/>{position}\
             <wp:extent cx=\"952500\" cy=\"476250\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>\
             {wrap}<wp:docPr id=\"5\" name=\"Picture 5\" descr=\"Logo\"/><a:graphic><a:graphicData \
             uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic><pic:blipFill>\
             <a:blip r:embed=\"rId9\"/><a:srcRect l=\"10000\" t=\"0\" r=\"5000\" b=\"0\"/></pic:blipFill>\
             <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"952500\" cy=\"476250\"/></a:xfrm>\
             <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData>\
             </a:graphic></wp:anchor></w:drawing>"
        )
    }

    const POS: &str = "<wp:positionH relativeFrom=\"column\"><wp:posOffset>95250</wp:posOffset></wp:positionH>\
        <wp:positionV relativeFrom=\"paragraph\"><wp:align>top</wp:align></wp:positionV>";

    #[test]
    fn reads_placement_kind_and_crop() {
        let xml = drawing(POS, "<wp:wrapSquare wrapText=\"largest\"/>");
        let f = read(&xml, &|id| Some(format!("word/media/{id}.png"))).unwrap();
        assert_eq!(f.at.h, (RelH::Column, Place::Offset(95250)));
        assert_eq!(f.at.v, (RelV::Paragraph, Place::Align(FloatAlign::Start)));
        assert_eq!((f.at.cx, f.at.cy), (952500, 476250));
        assert_eq!(f.at.size_px(), (100.0, 50.0));
        assert_eq!(f.at.wrap, Wrap::Square);
        assert_eq!(f.at.side, WrapSide::Largest);
        assert_eq!(f.at.dist, [0, 0, 114300, 114300]);
        assert_eq!(f.at.z, 251659264);
        assert!(!f.at.behind);
        assert_eq!(f.descr, "Logo");
        assert_eq!(
            f.kind,
            FloatKind::Picture {
                rel_id: "rId9".into(),
                part: "word/media/rId9.png".into(),
                crop: [10000, 0, 5000, 0]
            }
        );
        assert_eq!(f.write_xml(), xml.as_str());
    }

    #[test]
    fn percentage_positions_in_alternate_content() {
        let pos = "<mc:AlternateContent><mc:Choice Requires=\"wp14\"><wp:positionH relativeFrom=\"page\">\
            <wp14:pctPosHOffset>25000</wp14:pctPosHOffset></wp:positionH></mc:Choice><mc:Fallback>\
            <wp:positionH relativeFrom=\"page\"><wp:posOffset>1900000</wp:posOffset></wp:positionH>\
            </mc:Fallback></mc:AlternateContent><wp:positionV relativeFrom=\"margin\">\
            <wp:posOffset>-9525</wp:posOffset></wp:positionV>";
        let xml = drawing(pos, "<wp:wrapNone/>");
        let f = read(&xml, &|_| None).unwrap();
        assert_eq!(f.at.h, (RelH::Page, Place::Pct(25000)));
        assert_eq!(f.at.v, (RelV::Margin, Place::Offset(-9525)));
        assert_eq!(f.at.wrap, Wrap::None);
        // Moving replaces the whole alternate content with an offset.
        let mut at = f.at;
        at.h = (RelH::Column, Place::Offset(190500));
        let out = f.placed(at).write_xml().into_owned();
        assert!(!out.contains("pctPosHOffset"), "{out}");
        assert!(
            out.contains("<wp:positionH relativeFrom=\"column\"><wp:posOffset>190500</wp:posOffset></wp:positionH>"),
            "{out}"
        );
        // The vertical position is kept as it was.
        assert!(out.contains("<wp:posOffset>-9525</wp:posOffset>"));
        let back = read(&out, &|_| None).unwrap();
        assert_eq!(back.at, at);
    }

    #[test]
    fn patches_size_wrap_and_order() {
        let xml = drawing(POS, "<wp:wrapSquare wrapText=\"bothSides\"/>");
        let f = read(&xml, &|_| None).unwrap();
        let mut at = f.at;
        at.cx = 1905000;
        at.cy = 952500;
        at.v = (RelV::Paragraph, Place::Offset(12700));
        at.set_layout_option(LayoutOption::Tight);
        at.z = 251660000;
        let out = f.placed(at).write_xml().into_owned();
        assert!(out.contains("<wp:extent cx=\"1905000\" cy=\"952500\"/>"));
        assert!(out.contains("<a:ext cx=\"1905000\" cy=\"952500\"/>"));
        assert!(out.contains("<wp:wrapTight wrapText=\"bothSides\"><wp:wrapPolygon"));
        assert!(out.contains("relativeHeight=\"251660000\""));
        // Schema order: positions, extent, effectExtent, wrap, docPr.
        let i = |s: &str| out.find(s).unwrap();
        assert!(i("positionH") < i("positionV"));
        assert!(i("positionV") < i("wp:extent"));
        assert!(i("effectExtent") < i("wrapTight"));
        assert!(i("wrapTight") < i("docPr"));
        let back = read(&out, &|_| None).unwrap();
        assert_eq!(back.at, at);
        // Behind text: no wrap, behindDoc on.
        let mut at2 = at;
        at2.set_layout_option(LayoutOption::Behind);
        let out2 = f.placed(at2).write_xml().into_owned();
        assert!(out2.contains("behindDoc=\"1\"") && out2.contains("<wp:wrapNone/>"));
        assert_eq!(
            read(&out2, &|_| None).unwrap().at.layout_option(),
            LayoutOption::Behind
        );
    }

    #[test]
    fn shapes_and_text_boxes() {
        let xml = format!(
            "<mc:AlternateContent><mc:Choice Requires=\"wps\"><w:drawing><wp:anchor distT=\"45720\" \
             distB=\"45720\" distL=\"114300\" distR=\"114300\" simplePos=\"0\" relativeHeight=\"5\" \
             behindDoc=\"0\" locked=\"0\" layoutInCell=\"1\" allowOverlap=\"1\"><wp:simplePos x=\"0\" y=\"0\"/>\
             {POS}<wp:extent cx=\"1828800\" cy=\"914400\"/><wp:wrapTopAndBottom/><wp:docPr id=\"2\" \
             name=\"Text Box 2\"/><a:graphic><a:graphicData uri=\"http://schemas.microsoft.com/office/word/2010/wordprocessingShape\">\
             <wps:wsp><wps:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"1828800\" cy=\"914400\"/></a:xfrm>\
             <a:prstGeom prst=\"roundRect\"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val=\"FFEEDD\"/></a:solidFill>\
             </wps:spPr><wps:txbx><w:txbxContent><w:p><w:r><w:t>Hello</w:t></w:r></w:p></w:txbxContent></wps:txbx>\
             <wps:bodyPr lIns=\"0\" anchor=\"ctr\"/></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing>\
             </mc:Choice><mc:Fallback><w:pict><v:rect/></w:pict></mc:Fallback></mc:AlternateContent>"
        );
        let f = read(&xml, &|_| None).unwrap();
        assert_eq!(f.kind, FloatKind::Shape);
        assert_eq!(f.at.wrap, Wrap::TopAndBottom);
        assert!(f.geometry_xml().unwrap().contains("roundRect"));
        assert!(f.shape_xml().0.unwrap().contains("FFEEDD"));
        let (inner, insets, v) = text_box_xml(&xml).unwrap();
        assert_eq!(inner, "<w:p><w:r><w:t>Hello</w:t></w:r></w:p>");
        assert_eq!(insets, [0, 45720, 91440, 45720]);
        assert_eq!(v, FloatAlign::Center);
        let mut at = f.at;
        at.h.1 = Place::Offset(0);
        let out = f.placed(at).write_xml().into_owned();
        // The fallback stays as it was.
        assert!(out.ends_with(
            "<mc:Fallback><w:pict><v:rect/></w:pict></mc:Fallback></mc:AlternateContent>"
        ));
        assert!(out.contains("<wp:posOffset>0</wp:posOffset>"));
    }
}
