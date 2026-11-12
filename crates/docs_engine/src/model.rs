//! The document model: paragraphs of styled runs.
//!
//! A paragraph is its plain text plus a list of runs that cover the text
//! end to end (run `len`s are byte lengths and add up to `text.len()`). An
//! empty paragraph keeps one zero-length run so typing into it picks up its
//! formatting. Units are the file's: font sizes in half-points, spacing in
//! twentieths of a point (twips), line spacing in 240ths of a line (auto) or
//! twips (exact / at least), so values round-trip exactly.
//!
//! Inline objects (images, and XML Kantor keeps without modelling it) are
//! one [`OBJECT`] character in the text whose run carries the [`Object`].
//! Paragraphs inside a table carry the [`CellRef`] of their cell; the table
//! structure itself lives in [`crate::table`].

use std::ops::Range;
use std::sync::Arc;

pub use crate::object::{Image, ImageFormat, Level, OBJECT, Object, Opaque};
pub use crate::review::{RevKind, Revision};
pub use crate::table::CellRef;

use unicode_segmentation::UnicodeSegmentation as _;

/// Font names of a run (`w:rFonts`): explicit faces and/or theme slots.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Fonts {
    pub ascii: Option<String>,
    pub h_ansi: Option<String>,
    pub east_asia: Option<String>,
    pub cs: Option<String>,
    pub ascii_theme: Option<String>,
    pub h_ansi_theme: Option<String>,
    pub east_asia_theme: Option<String>,
    pub cs_theme: Option<String>,
    pub hint: Option<String>,
}

impl Fonts {
    /// A single face for every script.
    pub fn family(name: &str) -> Self {
        Self {
            ascii: Some(name.into()),
            h_ansi: Some(name.into()),
            east_asia: Some(name.into()),
            cs: Some(name.into()),
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Attributes set here replace the ones in `self`.
    pub fn overlay(&mut self, o: &Fonts) {
        macro_rules! take {
            ($($f:ident),*) => {$( if o.$f.is_some() { self.$f = o.$f.clone(); } )*};
        }
        take!(
            ascii,
            h_ansi,
            east_asia,
            cs,
            ascii_theme,
            h_ansi_theme,
            east_asia_theme,
            cs_theme,
            hint
        );
        // An explicit face overrides an inherited theme slot and vice versa.
        if o.ascii.is_some() && o.ascii_theme.is_none() {
            self.ascii_theme = None;
        }
        if o.ascii_theme.is_some() && o.ascii.is_none() {
            self.ascii = None;
        }
    }
}

/// Child elements of a property element (`w:rPr`, `w:pPr`, `w:tcPr`) kept
/// verbatim: the ones Kantor doesn't model, and modelled ones whose XML
/// says more than the model holds (e.g. a theme colour). See `docx.rs`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Extras(pub Vec<Arc<str>>);

impl Extras {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn push(&mut self, xml: &str) {
        self.0.push(xml.into());
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|s| &**s)
    }

    /// Drop the elements with local name `name`.
    pub fn remove(&mut self, name: &str) {
        self.0.retain(|x| element_name(x) != name);
    }
}

/// Local name of the first element in an XML fragment (`<w:b/>` → `b`).
pub fn element_name(xml: &str) -> &str {
    let s = xml.trim_start().trim_start_matches('<');
    let end = s
        .find(|c: char| c.is_whitespace() || c == '/' || c == '>')
        .unwrap_or(s.len());
    let name = &s[..end];
    name.rsplit_once(':').map_or(name, |(_, l)| l)
}

/// Character formatting. `None` means "inherit from the style".
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct RunProps {
    pub fonts: Fonts,
    /// Font size in half-points (`w:sz`).
    pub size: Option<u32>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    /// `RRGGBB`, or `None` for automatic (black).
    pub color: Option<Rgb>,
    /// Character style id (`w:rStyle`).
    pub style: Option<String>,
    /// Kept verbatim (not used by styles or layout).
    pub extras: Extras,
}

/// An sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub [u8; 3]);

impl Rgb {
    pub fn parse(hex: &str) -> Option<Self> {
        if hex.len() != 6 {
            return None;
        }
        let v = u32::from_str_radix(hex, 16).ok()?;
        Some(Self([(v >> 16) as u8, (v >> 8) as u8, v as u8]))
    }

    pub fn hex(self) -> String {
        let [r, g, b] = self.0;
        format!("{r:02X}{g:02X}{b:02X}")
    }
}

impl RunProps {
    pub fn overlay(&mut self, o: &RunProps) {
        self.fonts.overlay(&o.fonts);
        if o.size.is_some() {
            self.size = o.size;
        }
        if o.bold.is_some() {
            self.bold = o.bold;
        }
        if o.italic.is_some() {
            self.italic = o.italic;
        }
        if o.underline.is_some() {
            self.underline = o.underline;
        }
        if o.color.is_some() {
            self.color = o.color;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
    Justify,
}

impl Align {
    pub fn from_jc(val: &str) -> Option<Self> {
        Some(match val {
            "left" | "start" => Self::Left,
            "center" => Self::Center,
            "right" | "end" => Self::Right,
            "both" | "justify" => Self::Justify,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LineRule {
    /// `line` is in 240ths of a single line.
    #[default]
    Auto,
    /// `line` is a minimum height in twips.
    AtLeast,
    /// `line` is an exact height in twips.
    Exact,
}

/// Line spacing (`w:spacing/@w:line` + `@w:lineRule`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineSpacing {
    pub line: i32,
    pub rule: LineRule,
}

/// Paragraph indentation (`w:ind`), in twips. Each side inherits separately.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Indent {
    pub left: Option<i32>,
    pub right: Option<i32>,
    /// First-line indent; negative for a hanging indent.
    pub first: Option<i32>,
}

impl Indent {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn overlay(&mut self, o: &Indent) {
        if o.left.is_some() {
            self.left = o.left;
        }
        if o.right.is_some() {
            self.right = o.right;
        }
        if o.first.is_some() {
            self.first = o.first;
        }
    }
}

/// List membership (`w:numPr`): a `w:num` id and a level 0–8. Id 0 means
/// "not a list" (it switches off a list inherited from the style).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct NumRef {
    pub id: u32,
    pub level: u8,
}

/// Paragraph formatting. `None` means "inherit from the style".
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ParaProps {
    /// Paragraph style id (`w:pStyle`), e.g. `Heading1`.
    pub style: Option<String>,
    pub align: Option<Align>,
    /// Space before / after, in twips.
    pub before: Option<u32>,
    pub after: Option<u32>,
    pub line: Option<LineSpacing>,
    pub indent: Indent,
    pub num: Option<NumRef>,
    pub page_break_before: Option<bool>,
    /// Ignore spacing between paragraphs of the same style (`w:contextualSpacing`).
    pub contextual: Option<bool>,
    /// Kept verbatim (not used by styles or layout).
    pub extras: Extras,
}

impl ParaProps {
    /// Formatting fields (not the style) set here replace the ones in `self`.
    pub fn overlay(&mut self, o: &ParaProps) {
        if o.align.is_some() {
            self.align = o.align;
        }
        if o.before.is_some() {
            self.before = o.before;
        }
        if o.after.is_some() {
            self.after = o.after;
        }
        if o.line.is_some() {
            self.line = o.line;
        }
        self.indent.overlay(&o.indent);
        if o.num.is_some() {
            self.num = o.num;
        }
        if o.page_break_before.is_some() {
            self.page_break_before = o.page_break_before;
        }
        if o.contextual.is_some() {
            self.contextual = o.contextual;
        }
    }
}

/// A stretch of text with one formatting.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Run {
    /// Length in bytes of the paragraph text.
    pub len: usize,
    pub props: RunProps,
    /// The object an [`OBJECT`] character stands for (such a run is exactly
    /// that one character).
    pub object: Option<Object>,
    /// The hyperlink this run is part of: the original `<w:hyperlink …>`
    /// start tag, written back around consecutive runs with the same link.
    pub link: Option<Arc<str>>,
    /// A tracked insertion or deletion this run is part of (see
    /// [`crate::review`]).
    pub rev: Option<Arc<Revision>>,
}

impl Run {
    pub fn text(len: usize, props: RunProps) -> Self {
        Self {
            len,
            props,
            object: None,
            link: None,
            rev: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Paragraph {
    /// Text; tabs are `\t`, line breaks (`w:br`) are U+2028, page breaks
    /// U+000C, objects [`OBJECT`].
    pub text: String,
    pub runs: Vec<Run>,
    pub props: ParaProps,
    /// The table cell holding this paragraph (`None` in the body).
    pub cell: Option<CellRef>,
}

impl Default for Paragraph {
    fn default() -> Self {
        Self::new("")
    }
}

impl Paragraph {
    pub fn new(text: impl Into<String>) -> Self {
        Self::styled(text, RunProps::default())
    }

    pub fn styled(text: impl Into<String>, props: RunProps) -> Self {
        let text = text.into();
        Self {
            runs: vec![Run::text(text.len(), props)],
            text,
            props: ParaProps::default(),
            cell: None,
        }
    }

    pub fn with_style(mut self, style: &str) -> Self {
        self.props.style = Some(style.into());
        self
    }

    /// Append text with its own formatting.
    pub fn push(&mut self, text: &str, props: RunProps) {
        if self.text.is_empty() {
            self.runs.clear();
        }
        self.text.push_str(text);
        self.runs.push(Run::text(text.len(), props));
        self.normalize();
    }

    /// Append text inside a hyperlink (`link` is its start tag).
    pub fn push_linked(&mut self, text: &str, props: RunProps, link: Option<Arc<str>>) {
        self.push_tracked(text, props, link, None);
    }

    /// Append text inside a hyperlink and/or a tracked change.
    pub fn push_tracked(
        &mut self,
        text: &str,
        props: RunProps,
        link: Option<Arc<str>>,
        rev: Option<Arc<Revision>>,
    ) {
        if self.text.is_empty() {
            self.runs.clear();
        }
        self.text.push_str(text);
        self.runs.push(Run {
            link,
            rev,
            ..Run::text(text.len(), props)
        });
        self.normalize();
    }

    /// Append an object (one [`OBJECT`] character).
    pub fn push_object(&mut self, object: Object, props: RunProps, link: Option<Arc<str>>) {
        self.push_object_tracked(object, props, link, None);
    }

    /// Append an object inside a tracked change.
    pub fn push_object_tracked(
        &mut self,
        object: Object,
        props: RunProps,
        link: Option<Arc<str>>,
        rev: Option<Arc<Revision>>,
    ) {
        if self.text.is_empty() {
            self.runs.clear();
        }
        self.text.push(OBJECT);
        self.runs.push(Run {
            object: Some(object),
            link,
            rev,
            ..Run::text(OBJECT.len_utf8(), props)
        });
        self.normalize();
    }

    /// Insert an object at `offset`.
    pub fn insert_object(&mut self, offset: usize, object: Object, props: RunProps) {
        if self.text.is_empty() {
            self.runs.clear();
        }
        self.text.insert(offset, OBJECT);
        let i = self.split_run_at(offset);
        self.runs.insert(
            i,
            Run {
                object: Some(object),
                ..Run::text(OBJECT.len_utf8(), props)
            },
        );
        self.normalize();
    }

    /// The object whose character starts at `offset`.
    pub fn object_at(&self, offset: usize) -> Option<&Object> {
        self.run_ranges()
            .find(|(r, run)| r.start == offset && run.object.is_some())
            .and_then(|(_, run)| run.object.as_ref())
    }

    /// Objects with their byte offsets.
    pub fn objects(&self) -> impl Iterator<Item = (usize, &Object)> {
        self.run_ranges()
            .filter_map(|(r, run)| run.object.as_ref().map(|o| (r.start, o)))
    }

    /// Whether the paragraph is only a block-level object (e.g. a content
    /// control kept verbatim).
    pub fn is_block_object(&self) -> bool {
        self.runs.len() == 1
            && matches!(&self.runs[0].object, Some(Object::Opaque(o)) if o.level == Level::Block)
    }

    /// Runs with their byte ranges.
    pub fn run_ranges(&self) -> impl Iterator<Item = (Range<usize>, &Run)> {
        let mut start = 0;
        self.runs.iter().map(move |r| {
            let range = start..start + r.len;
            start += r.len;
            (range, r)
        })
    }

    /// Formatting that text typed at `offset` takes: the character before
    /// it, or the first run at the start (Word's behaviour).
    pub fn props_at(&self, offset: usize) -> RunProps {
        let mut last = None;
        for (range, run) in self.run_ranges() {
            if offset > range.start && offset <= range.end {
                return run.props.clone();
            }
            last.get_or_insert(run);
        }
        last.or(self.runs.first())
            .map(|r| r.props.clone())
            .unwrap_or_default()
    }

    /// Cut runs so that one starts at `offset`; returns that run's index.
    fn split_run_at(&mut self, offset: usize) -> usize {
        let mut start = 0;
        for i in 0..self.runs.len() {
            let len = self.runs[i].len;
            if offset == start {
                return i;
            }
            if offset < start + len {
                let right = Run {
                    len: start + len - offset,
                    props: self.runs[i].props.clone(),
                    object: self.runs[i].object.clone(),
                    link: self.runs[i].link.clone(),
                    rev: self.runs[i].rev.clone(),
                };
                self.runs[i].len = offset - start;
                self.runs.insert(i + 1, right);
                return i + 1;
            }
            start += len;
        }
        self.runs.len()
    }

    pub fn insert(&mut self, offset: usize, text: &str, props: RunProps) {
        if text.is_empty() {
            return;
        }
        if self.text.is_empty() {
            self.text.push_str(text);
            self.runs = vec![Run::text(text.len(), props)];
            return;
        }
        // Typing inside a hyperlink extends it; at its edges it doesn't.
        let link = self
            .run_ranges()
            .find(|(r, run)| offset > r.start && offset < r.end && run.object.is_none())
            .and_then(|(_, run)| run.link.clone());
        self.text.insert_str(offset, text);
        let i = self.split_run_at(offset);
        self.runs.insert(
            i,
            Run {
                link,
                ..Run::text(text.len(), props)
            },
        );
        self.normalize();
    }

    /// Delete a byte range. Position markers (bookmarks) in it survive at
    /// the range start: deleting text never drops them.
    pub fn delete(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let markers = self.markers_in(range.clone());
        let keep = self.props_at(range.start.max(1).min(self.text.len()));
        let a = self.split_run_at(range.start);
        let b = self.split_run_at(range.end);
        self.runs.drain(a..b);
        self.text.replace_range(range.clone(), "");
        if self.text.is_empty() {
            self.runs = vec![Run::text(0, keep)];
        }
        self.normalize();
        self.insert_runs(range.start, markers);
    }

    /// Clones of the marker-object runs inside `range`.
    pub fn markers_in(&self, range: Range<usize>) -> Vec<Run> {
        self.run_ranges()
            .filter(|(r, run)| {
                r.start >= range.start
                    && r.end <= range.end
                    && run.object.as_ref().is_some_and(Object::is_marker)
            })
            .map(|(_, run)| run.clone())
            .collect()
    }

    /// Insert whole object runs (markers) at `offset`.
    pub fn insert_runs(&mut self, offset: usize, runs: Vec<Run>) {
        let mut at = offset;
        for run in runs {
            if let Some(o) = run.object {
                if self.text.is_empty() {
                    self.runs.clear();
                }
                self.text.insert(at, OBJECT);
                let i = self.split_run_at(at);
                self.runs.insert(
                    i,
                    Run {
                        object: Some(o),
                        link: run.link,
                        rev: run.rev,
                        ..Run::text(OBJECT.len_utf8(), run.props)
                    },
                );
                at += OBJECT.len_utf8();
            }
        }
        self.normalize();
    }

    /// Whether the character at `offset` is a position marker.
    pub fn is_marker_at(&self, offset: usize) -> bool {
        self.object_at(offset).is_some_and(Object::is_marker)
    }

    /// Split at `offset`; `self` keeps the left part, the right part is returned
    /// with the same paragraph formatting.
    pub fn split_off(&mut self, offset: usize) -> Paragraph {
        let carry = self.props_at(offset);
        let i = self.split_run_at(offset);
        let mut runs = self.runs.split_off(i);
        let text = self.text.split_off(offset);
        if text.is_empty() {
            runs = vec![Run::text(0, carry.clone())];
        }
        if self.text.is_empty() {
            self.runs = vec![Run::text(0, carry)];
        }
        let mut right = Paragraph {
            text,
            runs,
            props: self.props.clone(),
            cell: self.cell,
        };
        // A section break belongs to the paragraph mark, which moves right.
        self.props.extras.remove("sectPr");
        right.normalize();
        self.normalize();
        right
    }

    /// Append `other`'s text and runs (keeping `self`'s paragraph formatting).
    pub fn append(&mut self, other: Paragraph) {
        // Keep a section break that ended the joined paragraph.
        if !self
            .props
            .extras
            .iter()
            .any(|x| element_name(x) == "sectPr")
            && let Some(sect) = other
                .props
                .extras
                .iter()
                .find(|x| element_name(x) == "sectPr")
        {
            self.props.extras.push(sect);
        }
        if other.text.is_empty() {
            return;
        }
        if self.text.is_empty() {
            self.text = other.text;
            self.runs = other.runs;
            return;
        }
        self.text.push_str(&other.text);
        self.runs.extend(other.runs);
        self.normalize();
    }

    /// Change the formatting of every run in `range`.
    pub fn format(&mut self, range: Range<usize>, f: &impl Fn(&mut RunProps)) {
        if self.text.is_empty() {
            f(&mut self.runs[0].props);
            return;
        }
        let a = self.split_run_at(range.start);
        let b = self.split_run_at(range.end);
        for run in &mut self.runs[a..b] {
            f(&mut run.props);
        }
        self.normalize();
    }

    /// Drop empty runs and merge neighbours with equal formatting.
    pub fn normalize(&mut self) {
        if self.text.is_empty() {
            self.runs.truncate(1);
            if self.runs.is_empty() {
                self.runs.push(Run::text(0, RunProps::default()));
            }
            self.runs[0].len = 0;
            self.runs[0].object = None;
            self.runs[0].link = None;
            self.runs[0].rev = None;
            return;
        }
        self.runs.retain(|r| r.len > 0);
        let mut out: Vec<Run> = Vec::with_capacity(self.runs.len());
        for run in self.runs.drain(..) {
            match out.last_mut() {
                Some(last)
                    if last.object.is_none()
                        && run.object.is_none()
                        && last.props == run.props
                        && last.link == run.link
                        && last.rev == run.rev =>
                {
                    last.len += run.len
                }
                _ => out.push(run),
            }
        }
        self.runs = out;
        debug_assert_eq!(
            self.runs.iter().map(|r| r.len).sum::<usize>(),
            self.text.len()
        );
    }
}

/// A caret position: paragraph index and byte offset in its text.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Position {
    pub para: usize,
    pub offset: usize,
}

impl Position {
    pub const fn new(para: usize, offset: usize) -> Self {
        Self { para, offset }
    }
}

/// A selection; `head` is where the caret is, `anchor` where it started.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: Position,
    pub head: Position,
}

impl Selection {
    pub const fn caret(pos: Position) -> Self {
        Self {
            anchor: pos,
            head: pos,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }

    /// `(start, end)` in document order.
    pub fn ordered(&self) -> (Position, Position) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
}

/// Previous grapheme boundary before `offset` (0 at the start).
pub fn prev_grapheme(text: &str, offset: usize) -> usize {
    text[..offset]
        .grapheme_indices(true)
        .next_back()
        .map_or(0, |(i, _)| i)
}

/// Next grapheme boundary after `offset` (the length at the end).
pub fn next_grapheme(text: &str, offset: usize) -> usize {
    text[offset..]
        .graphemes(true)
        .next()
        .map_or(text.len(), |g| offset + g.len())
}

/// Start of the word before `offset` (Ctrl+Left).
pub fn prev_word(text: &str, offset: usize) -> usize {
    text[..offset]
        .split_word_bound_indices()
        .rev()
        .find(|(_, w)| !w.trim().is_empty())
        .map_or(0, |(i, _)| i)
}

/// End of the word after `offset` (Ctrl+Right).
pub fn next_word(text: &str, offset: usize) -> usize {
    text[offset..]
        .split_word_bound_indices()
        .find(|(_, w)| !w.trim().is_empty())
        .map_or(text.len(), |(i, w)| offset + i + w.len())
}

/// The word around `offset` (double click).
pub fn word_at(text: &str, offset: usize) -> Range<usize> {
    for (i, w) in text.split_word_bound_indices() {
        if offset >= i && offset < i + w.len() {
            return i..i + w.len();
        }
    }
    offset..offset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bold() -> RunProps {
        RunProps {
            bold: Some(true),
            ..RunProps::default()
        }
    }

    fn lens(p: &Paragraph) -> Vec<usize> {
        p.runs.iter().map(|r| r.len).collect()
    }

    #[test]
    fn insert_splits_and_merges_runs() {
        let mut p = Paragraph::new("hello world");
        p.insert(5, " big", bold());
        assert_eq!(p.text, "hello big world");
        assert_eq!(lens(&p), vec![5, 4, 6]);
        p.insert(9, "!", bold());
        assert_eq!(lens(&p), vec![5, 5, 6], "merged with the bold run");
        assert_eq!(p.props_at(10), bold());
        assert_eq!(p.props_at(0), RunProps::default());
    }

    #[test]
    fn delete_keeps_formatting_of_empty_paragraph() {
        let mut p = Paragraph::styled("abc", bold());
        p.delete(0..3);
        assert_eq!(p.text, "");
        assert_eq!(p.runs.len(), 1);
        assert_eq!(p.runs[0].props, bold());
    }

    #[test]
    fn split_and_append_round_trip() {
        let mut p = Paragraph::new("one ");
        p.push("two", bold());
        let original = p.clone();
        let right = p.split_off(5);
        assert_eq!(p.text, "one t");
        assert_eq!(right.text, "wo");
        assert_eq!(right.runs[0].props, bold());
        p.append(right);
        assert_eq!(p, original);
    }

    #[test]
    fn split_at_end_carries_formatting() {
        let mut p = Paragraph::styled("x", bold());
        let right = p.split_off(1);
        assert_eq!(right.text, "");
        assert_eq!(right.runs[0].props, bold());
    }

    #[test]
    fn section_break_moves_with_the_paragraph_mark() {
        let mut p = Paragraph::new("ab");
        p.props
            .extras
            .push("<w:sectPr><w:type w:val=\"nextPage\"/></w:sectPr>");
        let right = p.split_off(1);
        assert!(p.props.extras.is_empty());
        assert_eq!(right.props.extras.0.len(), 1);
        p.append(right);
        assert_eq!(p.props.extras.0.len(), 1, "joined back, kept once");
    }

    #[test]
    fn format_range() {
        let mut p = Paragraph::new("abcdef");
        p.format(2..4, &|r| r.italic = Some(true));
        assert_eq!(lens(&p), vec![2, 2, 2]);
        p.format(0..6, &|r| r.italic = Some(true));
        assert_eq!(lens(&p), vec![6]);
    }

    #[test]
    fn graphemes_and_words() {
        let t = "añb 👍🏽 cd";
        let e = t.find('👍').unwrap();
        assert_eq!(next_grapheme(t, e), e + "👍🏽".len());
        assert_eq!(prev_grapheme(t, e + "👍🏽".len()), e);
        assert_eq!(prev_word("hello big world", 15), 10);
        assert_eq!(next_word("hello big world", 5), 9);
        assert_eq!(word_at("hello big world", 7), 6..9);
    }
}
