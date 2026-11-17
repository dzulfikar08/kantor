//! Inline objects: images and XML Kantor keeps without modelling it.
//!
//! An object is one [`OBJECT`] character in a paragraph's text; the run
//! covering that character carries the [`Object`]. Editing treats it like
//! any character (select, delete, undo), layout gives it a box, and saving
//! writes its XML back.

use std::sync::Arc;

/// The character standing for an object in paragraph text (U+FFFC OBJECT
/// REPLACEMENT CHARACTER).
pub const OBJECT: char = '\u{FFFC}';

/// EMUs (English Metric Units) per CSS pixel at 96 dpi.
pub const EMU_PER_PX: f32 = 9525.0;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Object {
    Image(Arc<Image>),
    Opaque(Arc<Opaque>),
    /// The start (code) or end of a field whose result is ordinary text
    /// between the two (see [`crate::fields`]).
    Field(Arc<FieldMark>),
    /// A floating picture, shape or text box (`wp:anchor`): zero width in
    /// the text, placed on the page by its own position (see
    /// [`crate::float`]).
    Float(Arc<crate::float::Float>),
}

impl Object {
    /// Zero-size objects that only mark a position (bookmarks, the edges
    /// of a table of contents' content control): deleting text never
    /// removes them.
    pub fn is_marker(&self) -> bool {
        matches!(self, Self::Opaque(o) if matches!(o.level, Level::Marker | Level::Wrap))
    }

    /// Objects deleting text around them never removes: markers, and
    /// floating objects (deleted on their own, when selected).
    pub fn stays_on_delete(&self) -> bool {
        self.is_marker() || matches!(self, Self::Float(_))
    }

    /// Zero-size objects: markers and field starts and ends. The caret
    /// steps over them; unlike markers, field marks go with deleted text.
    pub fn is_zero_width(&self) -> bool {
        self.is_marker() || matches!(self, Self::Field(_) | Self::Float(_))
    }

    /// The floating object, if this is one.
    pub fn float(&self) -> Option<&crate::float::Float> {
        match self {
            Self::Float(f) => Some(f),
            _ => None,
        }
    }

    /// The start or end of a block-level wrapper, written between paragraphs.
    pub fn is_wrap(&self) -> bool {
        matches!(self, Self::Opaque(o) if o.level == Level::Wrap)
    }

    pub fn is_block(&self) -> bool {
        matches!(self, Self::Opaque(o) if o.level == Level::Block)
    }

    /// The field mark, if this is one.
    pub fn field(&self) -> Option<&FieldMark> {
        match self {
            Self::Field(f) => Some(f),
            _ => None,
        }
    }

    /// Short description for placeholders.
    pub fn label(&self) -> &str {
        match self {
            Self::Image(i) if i.descr.is_empty() => "Image",
            Self::Image(i) => &i.descr,
            Self::Opaque(o) => &o.label,
            Self::Field(_) => "",
            Self::Float(f) => f.label(),
        }
    }
}

/// An inline picture (`w:drawing/wp:inline` with an embedded blip).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Image {
    /// Relationship id of the picture part (`r:embed`).
    pub rel_id: String,
    /// Size in EMUs (`wp:extent`).
    pub cx: i64,
    pub cy: i64,
    /// Alt text (`wp:docPr/@descr`).
    pub descr: String,
    /// The original `<w:drawing>…</w:drawing>`, written back verbatim;
    /// `None` for an image inserted in Kantor (its XML is generated).
    pub xml: Option<Arc<str>>,
    /// `wp:docPr/@id` for generated XML.
    pub doc_pr: u32,
    /// Part name of the picture, for the placeholder label and format.
    pub part: String,
}

impl Image {
    /// Size in CSS px at 96 dpi.
    pub fn size_px(&self) -> (f32, f32) {
        (
            self.cx.max(0) as f32 / EMU_PER_PX,
            self.cy.max(0) as f32 / EMU_PER_PX,
        )
    }

    pub fn format(&self) -> ImageFormat {
        ImageFormat::from_name(&self.part)
    }
}

/// Picture formats Kantor draws; everything else gets a placeholder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Other,
}

impl ImageFormat {
    pub fn from_name(name: &str) -> Self {
        let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        match ext.as_str() {
            "png" => Self::Png,
            "jpg" | "jpeg" | "jpe" => Self::Jpeg,
            _ => Self::Other,
        }
    }

    pub fn sniff(bytes: &[u8]) -> Self {
        if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
            Self::Png
        } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
            Self::Jpeg
        } else {
            Self::Other
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Other => "bin",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Other => "application/octet-stream",
        }
    }
}

/// Pixel size of a PNG or JPEG from its header.
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    match ImageFormat::sniff(bytes) {
        ImageFormat::Png => {
            let b = bytes.get(16..24)?;
            let w = u32::from_be_bytes(b[0..4].try_into().ok()?);
            let h = u32::from_be_bytes(b[4..8].try_into().ok()?);
            (w > 0 && h > 0).then_some((w, h))
        }
        ImageFormat::Jpeg => {
            let mut i = 2;
            while i + 4 <= bytes.len() {
                if bytes[i] != 0xFF {
                    i += 1;
                    continue;
                }
                let marker = bytes[i + 1];
                if marker == 0xFF {
                    i += 1;
                    continue;
                }
                if matches!(marker, 0xD8 | 0x01 | 0xD0..=0xD7) {
                    i += 2;
                    continue;
                }
                let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
                let sof = matches!(marker, 0xC0..=0xCF) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
                if sof {
                    let b = bytes.get(i + 5..i + 9)?;
                    let h = u16::from_be_bytes([b[0], b[1]]) as u32;
                    let w = u16::from_be_bytes([b[2], b[3]]) as u32;
                    return (w > 0 && h > 0).then_some((w, h));
                }
                i += 2 + len;
            }
            None
        }
        ImageFormat::Other => None,
    }
}

/// Where an opaque element sits, which decides how it's written back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Level {
    /// Zero-size paragraph-level marker (`w:bookmarkStart`, `w:permEnd`…).
    Marker,
    /// Run content (`w:drawing` anchors, `w:sym`, footnote references…):
    /// written inside a `<w:r>` with the run's formatting.
    Run,
    /// Paragraph content (`w:sdt`, fields, `w:oMath`…): written between runs.
    Para,
    /// Body or cell content (`w:sdt`, nested tables…): written between
    /// paragraphs.
    Block,
    /// Zero-size start or end of a block-level element whose content Kantor
    /// shows as ordinary paragraphs (a table of contents' `w:sdt`): the
    /// start tag up to `<w:sdtContent>`, or the closing tags. Written
    /// between paragraphs.
    Wrap,
}

/// One edge of a field.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum FieldMark {
    /// The field's start: its instruction and the XML up to its result.
    Begin {
        /// The instruction (`w:instrText` of the top-level field, or
        /// `w:fldSimple/@w:instr`), e.g. ` PAGEREF _Toc1 \h `.
        instr: String,
        /// Complex field: the runs from `fldChar begin` up to (not
        /// including) the `separate` run, nested code fields included.
        /// Simple field: the `<w:fldSimple …>` start tag.
        code: Arc<str>,
        /// The `fldChar separate` run as read (`None`: the field had no
        /// result, or it's a simple field).
        sep: Option<Arc<str>>,
        /// A `w:fldSimple`.
        simple: bool,
    },
    /// The field's end: the `fldChar end` run, or `</w:fldSimple>`.
    End { xml: Arc<str>, simple: bool },
}

impl FieldMark {
    pub fn is_begin(&self) -> bool {
        matches!(self, Self::Begin { .. })
    }

    /// The instruction of a field start.
    pub fn instr(&self) -> Option<&str> {
        match self {
            Self::Begin { instr, .. } => Some(instr),
            Self::End { .. } => None,
        }
    }

    /// The code as Word's field codes view shows it: the instruction with
    /// the codes of the fields nested in it in braces
    /// (`IF { MERGEFIELD Amount } > 100 "Big"`), trimmed. `None` for an end.
    pub fn shown_code(&self) -> Option<String> {
        let Self::Begin {
            instr,
            code,
            simple,
            ..
        } = self
        else {
            return None;
        };
        if *simple || !code.contains("instrText") {
            return Some(instr.trim().to_owned());
        }
        let mut r = quick_xml::Reader::from_str(code);
        let mut out = String::new();
        // Fields open (this one is the first); inside a nested field's
        // result nothing is shown.
        let mut depth = 0usize;
        let mut in_result: Vec<bool> = Vec::new();
        let mut in_instr = false;
        // Just after a nested field's opening brace.
        let mut fresh = false;
        // Inside a tracked edit's old code (`w:del`): not shown.
        let mut in_del = false;
        loop {
            match r.read_event() {
                Ok(quick_xml::events::Event::Eof) | Err(_) => break,
                Ok(quick_xml::events::Event::Start(e)) if e.local_name().as_ref() == b"del" => {
                    in_del = true;
                }
                Ok(quick_xml::events::Event::End(e)) if e.local_name().as_ref() == b"del" => {
                    in_del = false;
                }
                _ if in_del => {}
                Ok(quick_xml::events::Event::Start(e)) | Ok(quick_xml::events::Event::Empty(e))
                    if e.local_name().as_ref() == b"fldChar" =>
                {
                    let kind = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == b"fldCharType")
                        .map(|a| a.value.into_owned());
                    match kind.as_deref() {
                        Some(b"begin") => {
                            depth += 1;
                            in_result.push(false);
                            if depth > 1 && !in_result[..depth - 1].iter().any(|x| *x) {
                                let t = out.trim_end().len();
                                out.truncate(t);
                                out.push_str(if out.is_empty() { "{ " } else { " { " });
                                fresh = true;
                            }
                        }
                        Some(b"separate") => {
                            if let Some(x) = in_result.last_mut() {
                                *x = true;
                            }
                        }
                        Some(b"end") => {
                            in_result.pop();
                            if depth > 1 && !in_result.iter().any(|x| *x) {
                                let t = out.trim_end().len();
                                out.truncate(t);
                                out.push_str(" }");
                            }
                            depth = depth.saturating_sub(1);
                        }
                        _ => {}
                    }
                }
                Ok(quick_xml::events::Event::Start(e))
                    if e.local_name().as_ref() == b"instrText" =>
                {
                    in_instr = true;
                }
                Ok(quick_xml::events::Event::End(e)) if e.local_name().as_ref() == b"instrText" => {
                    in_instr = false;
                }
                Ok(quick_xml::events::Event::Text(t)) if in_instr => {
                    if !in_result.iter().any(|x| *x)
                        && let Ok(v) = t.xml10_content()
                    {
                        out.push_str(if fresh { v.trim_start() } else { &v });
                        fresh = fresh && v.trim().is_empty();
                    }
                }
                Ok(quick_xml::events::Event::GeneralRef(g))
                    if in_instr && !in_result.iter().any(|x| *x) =>
                {
                    match g.resolve_char_ref().ok().flatten() {
                        Some(c) => out.push(c),
                        None => out.push_str(match &*g as &[u8] {
                            b"amp" => "&",
                            b"lt" => "<",
                            b"gt" => ">",
                            b"quot" => "\"",
                            b"apos" => "'",
                            _ => "",
                        }),
                    }
                }
                _ => {}
            }
        }
        Some(out.trim().to_owned())
    }

    /// The instruction as Word works it out when updating: each field
    /// nested in the code replaced by `nested(its code, its saved result)`
    /// (`IF { REF Count } > 3 …` → `IF 4 > 3 …`). Fields without nested
    /// ones give their instruction.
    pub fn code_with_results(
        &self,
        nested: &mut dyn FnMut(&str, &str) -> String,
    ) -> Option<String> {
        use quick_xml::events::Event;
        let Self::Begin {
            instr,
            code,
            simple,
            ..
        } = self
        else {
            return None;
        };
        if *simple || code.matches("fldCharType=\"begin\"").count() < 2 {
            return Some(instr.clone());
        }
        let mut r = quick_xml::Reader::from_str(code);
        let mut out = String::new();
        let mut depth = 0usize;
        // Whether each open field is in its result.
        let mut in_result: Vec<bool> = Vec::new();
        let (mut in_instr, mut in_t, mut in_del) = (false, false, false);
        let (mut sub_code, mut sub_result) = (String::new(), String::new());
        let entity = |g: &[u8]| match g {
            b"amp" => "&",
            b"lt" => "<",
            b"gt" => ">",
            b"quot" => "\"",
            b"apos" => "'",
            _ => "",
        };
        loop {
            match r.read_event() {
                Ok(Event::Eof) | Err(_) => break,
                Ok(Event::Start(e)) if e.local_name().as_ref() == b"del" => in_del = true,
                Ok(Event::End(e)) if e.local_name().as_ref() == b"del" => in_del = false,
                _ if in_del => {}
                Ok(Event::Start(e)) | Ok(Event::Empty(e))
                    if e.local_name().as_ref() == b"fldChar" =>
                {
                    let kind = e
                        .attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == b"fldCharType")
                        .map(|a| a.value.into_owned());
                    match kind.as_deref() {
                        Some(b"begin") => {
                            depth += 1;
                            in_result.push(false);
                            if depth == 2 {
                                sub_code.clear();
                                sub_result.clear();
                            }
                        }
                        Some(b"separate") => {
                            if let Some(x) = in_result.last_mut() {
                                *x = true;
                            }
                        }
                        Some(b"end") => {
                            in_result.pop();
                            if depth == 2 {
                                out.push_str(&nested(sub_code.trim(), &sub_result));
                            }
                            depth = depth.saturating_sub(1);
                        }
                        _ => {}
                    }
                }
                Ok(Event::Start(e)) if e.local_name().as_ref() == b"instrText" => in_instr = true,
                Ok(Event::End(e)) if e.local_name().as_ref() == b"instrText" => in_instr = false,
                Ok(Event::Start(e)) if e.local_name().as_ref() == b"t" => in_t = true,
                Ok(Event::End(e)) if e.local_name().as_ref() == b"t" => in_t = false,
                Ok(Event::Text(t)) => {
                    let v = t
                        .xml10_content()
                        .map(|v| v.into_owned())
                        .unwrap_or_default();
                    self::push_code_text(
                        &mut out,
                        &mut sub_code,
                        &mut sub_result,
                        &in_result,
                        depth,
                        in_instr,
                        in_t,
                        &v,
                    );
                }
                Ok(Event::GeneralRef(g)) => {
                    let v = match g.resolve_char_ref().ok().flatten() {
                        Some(c) => c.to_string(),
                        None => entity(&g).to_owned(),
                    };
                    self::push_code_text(
                        &mut out,
                        &mut sub_code,
                        &mut sub_result,
                        &in_result,
                        depth,
                        in_instr,
                        in_t,
                        &v,
                    );
                }
                _ => {}
            }
        }
        Some(out)
    }

    /// A field's start from a code typed with nested fields in braces (as
    /// [`FieldMark::shown_code`] shows them): the nested fields written as
    /// fields in its code (without results: Word works them out when the
    /// field is updated). Unbalanced braces are kept as text.
    pub fn begin_nested(code: &str) -> Self {
        let balanced = {
            let mut d = 0i32;
            code.chars().all(|c| {
                d += match c {
                    '{' => 1,
                    '}' => -1,
                    _ => 0,
                };
                d >= 0
            }) && d == 0
        };
        if !balanced || !code.contains('{') {
            return Self::begin(code);
        }
        let run = |t: &str| {
            format!(
                "<w:r><w:instrText xml:space=\"preserve\">{}</w:instrText></w:r>",
                crate::xml::escape(t)
            )
        };
        let mut xml = String::from("<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>");
        let mut instr = String::new();
        let mut depth = 0;
        let mut piece = String::new();
        let flush = |piece: &mut String, xml: &mut String, instr: &mut String, depth: i32| {
            if !piece.is_empty() {
                xml.push_str(&run(piece));
                if depth == 0 {
                    instr.push_str(piece);
                }
                piece.clear();
            }
        };
        for c in code.chars() {
            match c {
                '{' => {
                    flush(&mut piece, &mut xml, &mut instr, depth);
                    xml.push_str("<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>");
                    depth += 1;
                }
                '}' => {
                    flush(&mut piece, &mut xml, &mut instr, depth);
                    xml.push_str(FIELD_END);
                    depth -= 1;
                }
                c => piece.push(c),
            }
        }
        flush(&mut piece, &mut xml, &mut instr, depth);
        Self::Begin {
            instr,
            code: xml.into(),
            sep: None,
            simple: false,
        }
    }

    /// A new complex field's start (no result yet).
    pub fn begin(instr: &str) -> Self {
        Self::Begin {
            instr: instr.to_owned(),
            code: complex_code(instr).into(),
            sep: None,
            simple: false,
        }
    }

    /// A new complex field's end.
    pub fn end() -> Self {
        Self::End {
            xml: FIELD_END.into(),
            simple: false,
        }
    }
}

/// `fldChar separate` in a run of its own.
pub const FIELD_SEPARATE: &str = "<w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>";
/// `fldChar end` in a run of its own.
pub const FIELD_END: &str = "<w:r><w:fldChar w:fldCharType=\"end\"/></w:r>";

/// The `begin` and instruction runs of a complex field.
pub fn complex_code(instr: &str) -> String {
    format!(
        "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r><w:r><w:instrText xml:space=\"preserve\">{}</w:instrText></w:r>",
        crate::xml::escape(instr)
    )
}

/// An element Kantor keeps byte for byte without modelling it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Opaque {
    pub level: Level,
    /// The original XML of the element (or of a whole field's runs).
    pub xml: String,
    /// Placeholder text, e.g. `Floating image` or `Field: PAGE`.
    pub label: String,
}

/// Where text read from a field's code goes ([`FieldMark::code_with_results`]):
/// the outer code, a nested field's code, or its result.
#[allow(clippy::too_many_arguments)]
fn push_code_text(
    out: &mut String,
    sub_code: &mut String,
    sub_result: &mut String,
    in_result: &[bool],
    depth: usize,
    in_instr: bool,
    in_t: bool,
    v: &str,
) {
    match depth {
        1 if in_instr && !in_result[0] => out.push_str(v),
        d if d >= 2 && !in_result[0] => {
            if in_result[1] {
                if in_t {
                    sub_result.push_str(v);
                }
            } else if d == 2 && in_instr {
                sub_code.push_str(v);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod field_code_tests {
    use super::*;

    #[test]
    fn nested_codes_are_shown_in_braces_and_typed_back() {
        let code = "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>\
            <w:r><w:instrText xml:space=\"preserve\"> IF </w:instrText></w:r>\
            <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>\
            <w:r><w:instrText xml:space=\"preserve\"> MERGEFIELD Amount </w:instrText></w:r>\
            <w:r><w:fldChar w:fldCharType=\"separate\"/></w:r><w:r><w:t>250</w:t></w:r>\
            <w:r><w:fldChar w:fldCharType=\"end\"/></w:r>\
            <w:r><w:instrText xml:space=\"preserve\"> &gt; 100 &quot;Big&quot; &quot;Small&quot; </w:instrText></w:r>";
        let mark = FieldMark::Begin {
            instr: " IF  > 100 \"Big\" \"Small\" ".into(),
            code: code.into(),
            sep: None,
            simple: false,
        };
        let shown = mark.shown_code().unwrap();
        // The nested field's code, not its result.
        assert_eq!(shown, "IF { MERGEFIELD Amount } > 100 \"Big\" \"Small\"");
        // Typed back: the same code, the nested field a field again.
        let typed = FieldMark::begin_nested(&shown);
        assert_eq!(typed.shown_code().unwrap(), shown);
        assert_eq!(typed.instr().unwrap(), "IF  > 100 \"Big\" \"Small\"");
        // A plain code is its instruction.
        assert_eq!(FieldMark::begin(" PAGE ").shown_code().unwrap(), "PAGE");
        // Unbalanced braces stay text.
        assert_eq!(
            FieldMark::begin_nested("QUOTE {x").instr().unwrap(),
            "QUOTE {x"
        );
    }
}
