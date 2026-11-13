//! Footnotes and endnotes.
//!
//! A reference in the text (`w:footnoteReference` / `w:endnoteReference`)
//! is a run object; the notes themselves live in `footnotes.xml` and
//! `endnotes.xml`, each read with the body reader into a small [`Document`]
//! (like headers and footers). The note's own number at its start
//! (`w:footnoteRef`) is a run object too. Numbers follow the text order
//! of the references, in the format and with the restarts of
//! `w:footnotePr` / `w:endnotePr` (settings.xml, then the section's).
//! Notes not touched in Kantor are written back byte for byte, the
//! separators included; the part (with its relationship and content type)
//! is created when a document gets its first note of a kind.

use std::sync::Arc;

use crate::document::{Document, Kind};
use crate::model::{
    LineRule, LineSpacing, OBJECT, Object, Opaque, ParaProps, Paragraph, Position, RunProps,
    Selection, element_name,
};
use crate::object::Level;
use crate::section::NumFormat;
use crate::xml::{self, El};

/// Footnote or endnote.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NoteKind {
    Footnote,
    Endnote,
}

impl NoteKind {
    pub const ALL: [Self; 2] = [Self::Footnote, Self::Endnote];

    /// The note element (`footnote`).
    pub(crate) fn element(self) -> &'static str {
        match self {
            Self::Footnote => "footnote",
            Self::Endnote => "endnote",
        }
    }

    /// The reference element in the text.
    pub fn reference(self) -> &'static str {
        match self {
            Self::Footnote => "footnoteReference",
            Self::Endnote => "endnoteReference",
        }
    }

    /// The note's own number mark.
    pub fn own_mark(self) -> &'static str {
        match self {
            Self::Footnote => "footnoteRef",
            Self::Endnote => "endnoteRef",
        }
    }

    /// The part's root element (`footnotes`).
    pub(crate) fn root(self) -> &'static str {
        match self {
            Self::Footnote => "footnotes",
            Self::Endnote => "endnotes",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Footnote => "Footnote",
            Self::Endnote => "Endnote",
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    fn ref_style(self) -> &'static str {
        match self {
            Self::Footnote => "FootnoteReference",
            Self::Endnote => "EndnoteReference",
        }
    }

    fn text_style(self) -> &'static str {
        match self {
            Self::Footnote => "FootnoteText",
            Self::Endnote => "EndnoteText",
        }
    }

    /// Which kind a reference or mark element is.
    pub fn of_element(name: &str) -> Option<Self> {
        match name {
            "footnoteReference" | "footnoteRef" => Some(Self::Footnote),
            "endnoteReference" | "endnoteRef" => Some(Self::Endnote),
            _ => None,
        }
    }
}

/// A footnote or endnote.
#[derive(Clone)]
pub struct Note {
    pub kind: NoteKind,
    /// `w:id`.
    pub id: String,
    pub doc: Document,
    /// The original element, written back as is unless the note was
    /// edited (then `None`).
    pub(crate) xml: Option<String>,
    /// A separator or continuation note (`w:type`): kept, never shown.
    pub separator: bool,
}

impl std::fmt::Debug for Note {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Note")
            .field("kind", &self.kind)
            .field("id", &self.id)
            .field("edited", &self.xml.is_none())
            .finish()
    }
}

impl PartialEq for Note {
    fn eq(&self, o: &Self) -> bool {
        self.kind == o.kind
            && self.id == o.id
            && self.xml == o.xml
            && self.separator == o.separator
            && self.doc.paragraphs == o.doc.paragraphs
    }
}

/// A notes part as read: its name and root start tag.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct NotesPart {
    pub name: String,
    pub root_tag: String,
}

/// When numbering starts again (`w:numRestart`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NoteRestart {
    #[default]
    Continuous,
    EachSection,
    EachPage,
}

/// Numbering of a kind of note (`w:footnotePr` / `w:endnotePr`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NoteProps {
    pub format: NumFormat,
    pub start: u32,
    pub restart: NoteRestart,
}

impl NoteProps {
    /// Word's defaults: footnotes 1, 2, 3; endnotes i, ii, iii.
    pub fn default_for(kind: NoteKind) -> Self {
        Self {
            format: match kind {
                NoteKind::Footnote => NumFormat::Decimal,
                NoteKind::Endnote => NumFormat::LowerRoman,
            },
            start: 1,
            restart: NoteRestart::Continuous,
        }
    }

    /// Settings in a `w:footnotePr` / `w:endnotePr` replace these.
    pub(crate) fn overlay(&mut self, el: &El) {
        for c in &el.children {
            match c.name.as_str() {
                "numFmt" => {
                    if let Some(v) = c.attr("val") {
                        self.format = NumFormat::parse(v);
                    }
                }
                "numStart" => {
                    if let Some(v) = c.num("val") {
                        self.start = v;
                    }
                }
                "numRestart" => {
                    self.restart = match c.attr("val") {
                        Some("eachSect") => NoteRestart::EachSection,
                        Some("eachPage") => NoteRestart::EachPage,
                        _ => NoteRestart::Continuous,
                    }
                }
                _ => {}
            }
        }
    }
}

/// Numbering settings for both kinds from settings.xml.
pub(crate) fn settings_note_props(settings: &str) -> [NoteProps; 2] {
    let mut out = NoteKind::ALL.map(NoteProps::default_for);
    if let Some(el) = xml::parse(settings) {
        for kind in NoteKind::ALL {
            let name = format!("{}Pr", kind.element());
            if let Some(c) = el.child(&name) {
                out[kind.index()].overlay(c);
            }
        }
    }
    out
}

/// A note reference in the text, numbered.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteRef {
    pub pos: Position,
    pub kind: NoteKind,
    pub id: String,
    /// The number shown ("3", "iv"); empty for a custom mark.
    pub label: String,
    /// The section the reference is in.
    pub section: usize,
}

/// The id and custom-mark flag of a note reference object.
pub fn reference_of(o: &Object) -> Option<(NoteKind, String, bool)> {
    let Object::Opaque(o) = o else { return None };
    let kind = match element_name(&o.xml) {
        "footnoteReference" => NoteKind::Footnote,
        "endnoteReference" => NoteKind::Endnote,
        _ => return None,
    };
    let el = xml::parse(&o.xml)?;
    let custom = el
        .attr("customMarkFollows")
        .is_some_and(|v| v == "1" || v == "true" || v == "on");
    Some((kind, el.attr("id")?.to_owned(), custom))
}

/// Separator notes for a new part (Word writes these two).
fn separators(kind: NoteKind) -> [(String, String); 2] {
    let el = kind.element();
    [
        (
            "-1".to_owned(),
            format!(
                "<w:{el} w:type=\"separator\" w:id=\"-1\"><w:p><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr><w:r><w:separator/></w:r></w:p></w:{el}>"
            ),
        ),
        (
            "0".to_owned(),
            format!(
                "<w:{el} w:type=\"continuationSeparator\" w:id=\"0\"><w:p><w:pPr><w:spacing w:after=\"0\" w:line=\"240\" w:lineRule=\"auto\"/></w:pPr><w:r><w:continuationSeparator/></w:r></w:p></w:{el}>"
            ),
        ),
    ]
}

/// The note elements of a notes part: (`w:id`, `w:type`, whole element,
/// inner XML).
pub(crate) fn split_notes(
    src: &str,
    kind: NoteKind,
) -> Vec<(String, Option<String>, String, String)> {
    let el = kind.element();
    let open_a = format!("<w:{el} ");
    let open_b = format!("<w:{el}>");
    let close = format!("</w:{el}>");
    let mut out = Vec::new();
    let mut at = 0;
    loop {
        let next = [src[at..].find(&open_a), src[at..].find(&open_b)]
            .into_iter()
            .flatten()
            .min();
        let Some(i) = next else { break };
        let start = at + i;
        let Some(tag_end) = src[start..].find('>').map(|e| start + e + 1) else {
            break;
        };
        let tag = &src[start..tag_end];
        let (end, inner) = if tag.ends_with("/>") {
            (tag_end, "")
        } else {
            match src[tag_end..].find(&close) {
                Some(e) => (tag_end + e + close.len(), &src[tag_end..tag_end + e]),
                None => break,
            }
        };
        let parsed = xml::parse(&format!(
            "{}/>",
            tag.trim_end_matches('>').trim_end_matches('/')
        ));
        let id = parsed
            .as_ref()
            .and_then(|e| e.attr("id").map(str::to_owned))
            .unwrap_or_default();
        let ty = parsed
            .as_ref()
            .and_then(|e| e.attr("type").map(str::to_owned));
        out.push((id, ty, src[start..end].to_owned(), inner.to_owned()));
        at = end;
    }
    out
}

/// A notes part's XML.
pub(crate) fn write_notes(
    root_tag: &str,
    kind: NoteKind,
    notes: &[Note],
    body: &dyn Fn(&Document) -> String,
) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n");
    let mut root = root_tag.to_owned();
    if root.ends_with("/>") {
        root.replace_range(root.len() - 2.., ">");
    }
    out.push_str(&root);
    let el = kind.element();
    for n in notes.iter().filter(|n| n.kind == kind) {
        match &n.xml {
            Some(x) => out.push_str(x),
            None => {
                out.push_str(&format!("<w:{el} w:id=\"{}\">", xml::escape(&n.id)));
                out.push_str(&body(&n.doc));
                out.push_str(&format!("</w:{el}>"));
            }
        }
    }
    out.push_str(&format!("</w:{}>", kind.root()));
    out
}

pub(crate) const W_DECL: &str =
    "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
pub(crate) const R_DECL: &str =
    "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

impl Document {
    /// Footnotes and endnotes (separators included), in file order.
    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    pub fn note(&self, kind: NoteKind, id: &str) -> Option<&Note> {
        self.notes
            .iter()
            .find(|n| n.kind == kind && n.id == id && !n.separator)
    }

    /// Index of a note in [`Self::notes`].
    pub fn note_index(&self, kind: NoteKind, id: &str) -> Option<usize> {
        self.notes
            .iter()
            .position(|n| n.kind == kind && n.id == id && !n.separator)
    }

    /// Numbering of `kind` in section `si`: settings.xml's, then the
    /// section's own `w:footnotePr` / `w:endnotePr`.
    pub fn note_props(&self, kind: NoteKind, si: usize) -> NoteProps {
        let mut p = self.note_settings[kind.index()];
        let sections = self.sections();
        if si < sections.len() {
            let sect = self.sect_xml(&sections, si);
            if let Some(el) = xml::parse(&sect)
                && let Some(c) = el.child(&format!("{}Pr", kind.element()))
            {
                p.overlay(c);
            }
        }
        p
    }

    /// Every note reference in text order, numbered. Restarts per section
    /// are applied; restarts per page count on (layout renumbers them).
    pub fn note_refs(&self) -> Vec<NoteRef> {
        let sections = self.sections();
        let props: Vec<[NoteProps; 2]> = (0..sections.len())
            .map(|si| NoteKind::ALL.map(|k| self.note_props(k, si)))
            .collect();
        let mut count = [0u32; 2];
        let mut out = Vec::new();
        let mut si = 0;
        let mut started = usize::MAX;
        for (pi, p) in self.paragraphs.iter().enumerate() {
            while si + 1 < sections.len() && pi >= sections[si].paras.end {
                si += 1;
            }
            if started != si {
                for k in NoteKind::ALL {
                    let pr = props[si][k.index()];
                    if started == usize::MAX || pr.restart != NoteRestart::Continuous {
                        count[k.index()] = 0;
                    }
                }
                started = si;
            }
            for (off, o) in p.objects() {
                let Some((kind, id, custom)) = reference_of(o) else {
                    continue;
                };
                let pr = props[si][kind.index()];
                let label = if custom {
                    String::new()
                } else {
                    let n = pr.start + count[kind.index()];
                    count[kind.index()] += 1;
                    pr.format.format(n)
                };
                out.push(NoteRef {
                    pos: Position::new(pi, off),
                    kind,
                    id,
                    label,
                    section: si,
                });
            }
        }
        out
    }

    /// The note whose reference is right at or before `pos`.
    pub fn note_at(&self, pos: Position) -> Option<(NoteKind, String)> {
        let p = self.paragraph(pos.para);
        let before = pos.offset.saturating_sub(OBJECT.len_utf8());
        [pos.offset, before]
            .into_iter()
            .filter_map(|o| p.object_at(o))
            .find_map(reference_of)
            .map(|(k, id, _)| (k, id))
    }

    /// Why a note can't be edited as plain text, if so.
    pub fn note_locked(&self, kind: NoteKind, id: &str) -> Option<String> {
        let n = self.note(kind, id)?;
        let other = n.doc.paragraphs.iter().any(|p| {
            p.cell.is_some()
                || p.objects().any(|(_, o)| {
                    !o.is_marker()
                        && !matches!(o, Object::Opaque(op) if NoteKind::of_element(element_name(&op.xml)).is_some())
                })
        });
        other.then(|| {
            format!(
                "This {} has pictures, tables or other objects; Kantor shows it and keeps it as it is, but can't edit it yet",
                kind.label().to_lowercase()
            )
        })
    }

    /// A note's text for the editor: paragraphs joined with `\n`, without
    /// its number mark and the space after it.
    pub fn note_text(&self, kind: NoteKind, id: &str) -> String {
        let Some(n) = self.note(kind, id) else {
            return String::new();
        };
        let lines: Vec<String> = n
            .doc
            .paragraphs
            .iter()
            .map(|p| p.text.replace(OBJECT, ""))
            .collect();
        let mut text = lines.join("\n");
        if text.starts_with(' ') {
            text.remove(0);
        }
        text
    }

    /// Replace a note's text (`\n` separates paragraphs): one undo step.
    /// The first paragraph's formatting and the number mark stay; the text
    /// takes the formatting of the note's first text run.
    pub fn set_note_text(&mut self, kind: NoteKind, id: &str, text: &str, sel: Selection) -> bool {
        if self.note_locked(kind, id).is_some() {
            return false;
        }
        let Some(i) = self.note_index(kind, id) else {
            return false;
        };
        if self.note_text(kind, id) == text {
            return true;
        }
        let before = self.side_state();
        let note = &self.notes[i];
        let first = note.doc.paragraphs.first().cloned().unwrap_or_default();
        let mark = note
            .doc
            .paragraphs
            .iter()
            .flat_map(|p| p.runs.iter())
            .find(|r| {
                matches!(&r.object, Some(Object::Opaque(o)) if element_name(&o.xml) == kind.own_mark())
            })
            .cloned();
        let text_props = note
            .doc
            .paragraphs
            .iter()
            .flat_map(|p| {
                p.run_ranges()
                    .map(move |(r, run)| (p.text[r].to_owned(), run))
            })
            .find(|(t, run)| run.object.is_none() && !t.trim().is_empty())
            .map(|(_, run)| run.props.clone())
            .unwrap_or_else(|| self.note_text_props(kind));
        let mut paras = Vec::new();
        for (k, line) in text.split('\n').enumerate() {
            let mut p = Paragraph {
                props: first.props.clone(),
                ..Paragraph::default()
            };
            p.props.extras.remove("sectPr");
            p.runs[0].props = text_props.clone();
            if k == 0 {
                match &mark {
                    Some(m) => p.push_object(
                        m.object.clone().expect("mark object"),
                        m.props.clone(),
                        None,
                    ),
                    None => p.push_object(self.own_mark(kind), self.note_ref_props(kind), None),
                }
                p.push(&format!(" {line}"), text_props.clone());
            } else {
                p.push(line, text_props.clone());
            }
            p.normalize();
            paras.push(p);
        }
        let note = &mut self.notes[i];
        note.doc.paragraphs = paras;
        note.doc.tables.clear();
        note.xml = None;
        let after = self.side_state();
        self.push_command(
            0,
            Vec::new(),
            Vec::new(),
            sel,
            Some(Box::new((before, after))),
        );
        true
    }

    /// Formatting of a note reference mark: the style if the document has
    /// it, else superscript.
    fn note_ref_props(&self, kind: NoteKind) -> RunProps {
        let mut p = RunProps::default();
        if self.styles.get(kind.ref_style()).is_some() {
            p.style = Some(kind.ref_style().to_owned());
        } else {
            p.extras.push("<w:vertAlign w:val=\"superscript\"/>");
        }
        p
    }

    /// Formatting of a new note's text: 10 pt unless the style sets it.
    fn note_text_props(&self, kind: NoteKind) -> RunProps {
        let mut p = RunProps::default();
        if self.styles.get(kind.text_style()).is_none() {
            p.size = Some(20);
        }
        p
    }

    fn own_mark(&self, kind: NoteKind) -> Object {
        Object::Opaque(Arc::new(Opaque {
            level: Level::Run,
            xml: format!("<w:{}/>", kind.own_mark()),
            label: kind.label().to_owned(),
        }))
    }

    /// Insert a footnote or endnote at the end of the selection, as Word's
    /// Insert Footnote (Ctrl+Alt+F) and Insert Endnote (Ctrl+Alt+D): a
    /// numbered reference in the text and an empty note. One undo step.
    /// Returns the caret after the reference and the new note's id.
    pub fn insert_note(&mut self, kind: NoteKind, sel: Selection) -> (Selection, String) {
        let sel = self.clamp_selection(sel);
        let (_, at) = sel.ordered();
        let before = self.side_state();
        // A new part gets Word's separator notes.
        let has_part = self.notes.iter().any(|n| n.kind == kind)
            || self
                .package
                .as_ref()
                .is_some_and(|p| p.notes[kind.index()].is_some());
        if !has_part {
            for (id, xml) in separators(kind) {
                let mut doc = Document::new();
                doc.styles = self.styles.clone();
                self.notes.push(Note {
                    kind,
                    id,
                    doc,
                    xml: Some(xml),
                    separator: true,
                });
            }
        }
        let next = self
            .notes
            .iter()
            .filter(|n| n.kind == kind)
            .filter_map(|n| n.id.parse::<i64>().ok())
            .chain(
                self.note_refs()
                    .iter()
                    .filter(|r| r.kind == kind)
                    .filter_map(|r| r.id.parse().ok()),
            )
            .max()
            .unwrap_or(0)
            .max(0)
            + 1;
        let id = next.to_string();
        // The note: its number mark and a space, ready for text.
        let mut p = Paragraph::default();
        if self.styles.get(kind.text_style()).is_some() {
            p.props.style = Some(kind.text_style().to_owned());
        } else {
            p.props = ParaProps {
                after: Some(0),
                line: Some(LineSpacing {
                    line: 240,
                    rule: LineRule::Auto,
                }),
                ..ParaProps::default()
            };
        }
        let text_props = self.note_text_props(kind);
        p.runs[0].props = text_props.clone();
        p.push_object(self.own_mark(kind), self.note_ref_props(kind), None);
        p.push(" ", text_props);
        let mut doc = Document::new();
        doc.styles = self.styles.clone();
        doc.numbering = self.numbering.clone();
        doc.paragraphs = vec![p];
        self.notes.push(Note {
            kind,
            id: id.clone(),
            doc,
            xml: None,
            separator: false,
        });
        // The reference in the text.
        let reference = Object::Opaque(Arc::new(Opaque {
            level: Level::Run,
            xml: format!("<w:{} w:id=\"{id}\"/>", kind.reference()),
            label: kind.label().to_owned(),
        }));
        let mut para = self.paragraphs[at.para].clone();
        para.insert_object(at.offset, reference, self.note_ref_props(kind));
        self.track_object(&mut para, at.offset);
        let after = Selection::caret(Position::new(at.para, at.offset + OBJECT.len_utf8()));
        self.replace(at.para, 1, vec![para], (sel, after), Kind::Other, true);
        let after_state = self.side_state();
        self.set_last_side(before, after_state);
        (after, id)
    }

    /// Delete a note and its reference: one undo step.
    pub fn delete_note(&mut self, kind: NoteKind, id: &str, sel: Selection) -> bool {
        let Some(i) = self.note_index(kind, id) else {
            return false;
        };
        let before = self.side_state();
        self.notes.remove(i);
        let refs: Vec<Position> = self
            .note_refs()
            .into_iter()
            .filter(|r| r.kind == kind && r.id == id)
            .map(|r| r.pos)
            .collect();
        match (refs.first(), refs.last()) {
            (Some(&a), Some(&b)) => {
                let mut new: Vec<Paragraph> = self.paragraphs[a.para..=b.para].to_vec();
                for pos in refs.iter().rev() {
                    new[pos.para - a.para].remove_object(pos.offset);
                }
                let n = new.len();
                let after = self.clamp_selection(sel);
                self.replace(a.para, n, new, (sel, after), Kind::Other, true);
                let after_state = self.side_state();
                self.set_last_side(before, after_state);
            }
            _ => {
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
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_elements_split() {
        let src = r#"<w:footnotes xmlns:w="w"><w:footnote w:type="separator" w:id="-1"><w:p/></w:footnote><w:footnote w:id="1"><w:p><w:r><w:footnoteRef/></w:r><w:r><w:t> One</w:t></w:r></w:p></w:footnote><w:footnote w:id="2"/></w:footnotes>"#;
        let notes = split_notes(src, NoteKind::Footnote);
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0].1.as_deref(), Some("separator"));
        assert_eq!(notes[1].0, "1");
        assert!(notes[1].3.starts_with("<w:p>"));
        assert_eq!(notes[2].3, "");
    }

    #[test]
    fn numbering_settings() {
        let s = r#"<w:settings xmlns:w="w"><w:footnotePr><w:numFmt w:val="upperRoman"/><w:numStart w:val="3"/><w:numRestart w:val="eachPage"/></w:footnotePr></w:settings>"#;
        let [f, e] = settings_note_props(s);
        assert_eq!(f.format, NumFormat::UpperRoman);
        assert_eq!(f.start, 3);
        assert_eq!(f.restart, NoteRestart::EachPage);
        assert_eq!(e, NoteProps::default_for(NoteKind::Endnote));
        assert_eq!(e.format.format(4), "iv");
    }
}
