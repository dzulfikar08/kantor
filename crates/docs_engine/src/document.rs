//! A document: paragraphs, styles, page geometry, its file, and the undo
//! history.
//!
//! Every edit is one command, "replace paragraphs `start..start+old.len()`
//! with `new`", which makes undo/redo generic: undo puts `old` back. Typing
//! and deleting coalesce into one step per word.

use std::path::{Path, PathBuf};

use crate::docx::{self, Package};
use crate::model::*;
use crate::style::{ResolvedRun, StyleSheet};

/// Line break inside a paragraph (`w:br`). U+2028 is a mandatory break for
/// the line breaker, so layout needs no special case.
pub const LINE_BREAK: char = '\u{2028}';

/// Page size and margins, in twips (1/1440 inch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageGeometry {
    pub width: u32,
    pub height: u32,
    pub margin_top: u32,
    pub margin_bottom: u32,
    pub margin_left: u32,
    pub margin_right: u32,
}

impl Default for PageGeometry {
    /// A4 portrait, 2.54 cm margins.
    fn default() -> Self {
        Self {
            width: 11906,
            height: 16838,
            margin_top: 1440,
            margin_bottom: 1440,
            margin_left: 1440,
            margin_right: 1440,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Typing,
    Deleting,
    /// IME composition: every preedit update and the commit are one step.
    Composing,
    Other,
}

#[derive(Clone, Debug)]
struct Command {
    id: u64,
    start: usize,
    old: Vec<Paragraph>,
    new: Vec<Paragraph>,
    before: Selection,
    after: Selection,
    kind: Kind,
}

#[derive(Debug, Default)]
struct History {
    undo: Vec<Command>,
    redo: Vec<Command>,
    next_id: u64,
    /// Id of the command on top of the undo stack when last saved (`None`:
    /// the stack was empty).
    saved: Option<u64>,
    /// Don't merge the next command into the top one (composition ended).
    sealed: bool,
}

impl History {
    fn top(&self) -> Option<u64> {
        self.undo.last().map(|c| c.id)
    }

    /// Record `cmd`; merge it into the previous step when it continues the
    /// same word of typing or deleting.
    fn push(&mut self, mut cmd: Command, word_boundary: bool) {
        self.redo.clear();
        let sealed = std::mem::take(&mut self.sealed);
        if let Some(last) = self.undo.last_mut()
            && !sealed
            && cmd.kind != Kind::Other
            && cmd.kind == last.kind
            && Some(last.id) != self.saved
            && last.start == cmd.start
            && last.new.len() == 1
            && cmd.old.len() == 1
            && cmd.new.len() == 1
            && (cmd.kind == Kind::Composing || (!word_boundary && last.after == cmd.before))
        {
            last.new = cmd.new;
            last.after = cmd.after;
            if last.old == last.new {
                // A cancelled composition: nothing to undo.
                self.undo.pop();
            }
            return;
        }
        self.next_id += 1;
        cmd.id = self.next_id;
        self.undo.push(cmd);
    }
}

/// What Ctrl+B / Ctrl+I / Ctrl+U toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    Bold,
    Italic,
    Underline,
}

impl Toggle {
    fn get(self, r: &ResolvedRun) -> bool {
        match self {
            Self::Bold => r.bold,
            Self::Italic => r.italic,
            Self::Underline => r.underline,
        }
    }

    pub fn set(self, p: &mut RunProps, on: bool) {
        match self {
            Self::Bold => p.bold = Some(on),
            Self::Italic => p.italic = Some(on),
            Self::Underline => p.underline = Some(on),
        }
    }
}

pub struct Document {
    paragraphs: Vec<Paragraph>,
    pub styles: StyleSheet,
    pub page: PageGeometry,
    path: Option<PathBuf>,
    /// The `.docx` package this document was read from: Save copies its
    /// parts and regenerates only the body.
    package: Option<Package>,
    /// Content the model doesn't hold (empty for a fully modelled file).
    unsupported: Vec<String>,
    history: History,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    /// A blank document with Word's default styles.
    pub fn new() -> Self {
        Self {
            paragraphs: vec![Paragraph::default()],
            styles: StyleSheet::builtin(),
            page: PageGeometry::default(),
            path: None,
            package: None,
            unsupported: Vec::new(),
            history: History::default(),
        }
    }

    /// A document holding `paragraphs` (tests, examples).
    pub fn from_paragraphs(paragraphs: Vec<Paragraph>) -> Self {
        let mut doc = Self::new();
        if !paragraphs.is_empty() {
            doc.paragraphs = paragraphs;
        }
        doc
    }

    /// Open a `.docx` file.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("Can't read {}: {e}", path.display()))?;
        let read = docx::read(bytes)
            .map_err(|e| anyhow::anyhow!("Can't open {}: {e:#}", path.display()))?;
        Ok(Self {
            paragraphs: if read.paragraphs.is_empty() {
                vec![Paragraph::default()]
            } else {
                read.paragraphs
            },
            styles: read.styles,
            page: read.page,
            path: Some(path.to_path_buf()),
            package: Some(read.package),
            unsupported: read.unsupported,
            history: History::default(),
        })
    }

    // ── queries ────────────────────────────────────────────────────────────

    pub fn paragraphs(&self) -> &[Paragraph] {
        &self.paragraphs
    }

    pub fn paragraph(&self, i: usize) -> &Paragraph {
        &self.paragraphs[i.min(self.paragraphs.len() - 1)]
    }

    pub fn len(&self) -> usize {
        self.paragraphs.len()
    }

    /// Never true: a document always has at least one paragraph.
    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// File name for tabs, e.g. `report.docx`.
    pub fn title(&self) -> String {
        self.path
            .as_deref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.docx".to_owned())
    }

    /// Default name for Save As.
    pub fn suggested_file_name(&self) -> String {
        match self.path.as_deref().and_then(|p| p.file_stem()) {
            Some(stem) if self.has_unsupported_content() => {
                format!("{} (Kantor).docx", stem.to_string_lossy())
            }
            Some(stem) => format!("{}.docx", stem.to_string_lossy()),
            None => "Untitled.docx".to_owned(),
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.history.top() != self.history.saved
    }

    pub fn can_undo(&self) -> bool {
        !self.history.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.history.redo.is_empty()
    }

    /// What the opened file has that Kantor doesn't model (e.g. "tables").
    /// Saving such a document never overwrites the original: Save goes to
    /// Save As, a new file without that content.
    pub fn unsupported_content(&self) -> &[String] {
        &self.unsupported
    }

    pub fn has_unsupported_content(&self) -> bool {
        !self.unsupported.is_empty()
    }

    /// Whether Save must ask for a new file (untitled, or the original has
    /// content Kantor would drop).
    pub fn needs_save_as(&self) -> bool {
        self.path.is_none() || self.has_unsupported_content()
    }

    pub fn word_count(&self) -> usize {
        self.paragraphs
            .iter()
            .map(|p| p.text.split_whitespace().count())
            .sum()
    }

    pub fn char_count(&self) -> usize {
        self.paragraphs.iter().map(|p| p.text.chars().count()).sum()
    }

    /// Clamp a position into the document (and onto a char boundary).
    pub fn clamp(&self, pos: Position) -> Position {
        let para = pos.para.min(self.paragraphs.len() - 1);
        let text = &self.paragraphs[para].text;
        let mut offset = pos.offset.min(text.len());
        while !text.is_char_boundary(offset) {
            offset -= 1;
        }
        Position { para, offset }
    }

    pub fn clamp_selection(&self, sel: Selection) -> Selection {
        Selection {
            anchor: self.clamp(sel.anchor),
            head: self.clamp(sel.head),
        }
    }

    pub fn end(&self) -> Position {
        let para = self.paragraphs.len() - 1;
        Position::new(para, self.paragraphs[para].text.len())
    }

    /// One grapheme back, crossing into the previous paragraph's end.
    pub fn prev_position(&self, pos: Position) -> Position {
        if pos.offset > 0 {
            let text = &self.paragraphs[pos.para].text;
            Position::new(pos.para, prev_grapheme(text, pos.offset))
        } else if pos.para > 0 {
            Position::new(pos.para - 1, self.paragraphs[pos.para - 1].text.len())
        } else {
            pos
        }
    }

    /// One grapheme forward, crossing into the next paragraph's start.
    pub fn next_position(&self, pos: Position) -> Position {
        let text = &self.paragraphs[pos.para].text;
        if pos.offset < text.len() {
            Position::new(pos.para, next_grapheme(text, pos.offset))
        } else if pos.para + 1 < self.paragraphs.len() {
            Position::new(pos.para + 1, 0)
        } else {
            pos
        }
    }

    pub fn prev_word_position(&self, pos: Position) -> Position {
        if pos.offset == 0 {
            return self.prev_position(pos);
        }
        Position::new(
            pos.para,
            prev_word(&self.paragraphs[pos.para].text, pos.offset),
        )
    }

    pub fn next_word_position(&self, pos: Position) -> Position {
        let text = &self.paragraphs[pos.para].text;
        if pos.offset >= text.len() {
            return self.next_position(pos);
        }
        Position::new(pos.para, next_word(text, pos.offset))
    }

    /// Plain text of a range; paragraphs are separated by `\n`.
    pub fn text_in(&self, start: Position, end: Position) -> String {
        let mut out = String::new();
        for i in start.para..=end.para {
            let text = &self.paragraphs[i].text;
            let a = if i == start.para { start.offset } else { 0 };
            let b = if i == end.para {
                end.offset
            } else {
                text.len()
            };
            out.push_str(&text[a..b]);
            if i != end.para {
                out.push('\n');
            }
        }
        out.replace(LINE_BREAK, "\n")
    }

    /// Formatting typed text takes at `pos`.
    pub fn run_props_at(&self, pos: Position) -> RunProps {
        self.paragraph(pos.para).props_at(pos.offset)
    }

    pub fn resolved_at(&self, pos: Position) -> ResolvedRun {
        let p = self.paragraph(pos.para);
        self.styles.resolve_run(p, &p.props_at(pos.offset))
    }

    /// Whether every character in the selection (or the caret's formatting)
    /// has `toggle` on.
    pub fn is_on(&self, sel: Selection, toggle: Toggle) -> bool {
        let (start, end) = sel.ordered();
        if start == end {
            return toggle.get(&self.resolved_at(start));
        }
        let mut any = false;
        for i in start.para..=end.para {
            let p = &self.paragraphs[i];
            let a = if i == start.para { start.offset } else { 0 };
            let b = if i == end.para {
                end.offset
            } else {
                p.text.len()
            };
            for (range, run) in p.run_ranges() {
                if range.end > a && range.start < b {
                    any = true;
                    if !toggle.get(&self.styles.resolve_run(p, &run.props)) {
                        return false;
                    }
                }
            }
        }
        any
    }

    // ── edits ──────────────────────────────────────────────────────────────

    fn replace(
        &mut self,
        start: usize,
        count: usize,
        new: Vec<Paragraph>,
        (before, after): (Selection, Selection),
        kind: Kind,
        word_boundary: bool,
    ) {
        debug_assert!(!new.is_empty());
        let old: Vec<Paragraph> = self
            .paragraphs
            .splice(start..start + count, new.iter().cloned())
            .collect();
        if old == new {
            return;
        }
        self.history.push(
            Command {
                id: 0,
                start,
                old,
                new,
                before,
                after,
                kind,
            },
            word_boundary,
        );
    }

    /// The paragraphs `start.para..=end.para` with the range removed, merged
    /// into one.
    fn cut(&self, start: Position, end: Position) -> Paragraph {
        let mut first = self.paragraphs[start.para].clone();
        if start.para == end.para {
            first.delete(start.offset..end.offset);
            return first;
        }
        first.delete(start.offset..first.text.len());
        let mut last = self.paragraphs[end.para].clone();
        last.delete(0..end.offset);
        first.append(last);
        first
    }

    /// Replace the selection with `text` (`\n` starts a new paragraph).
    /// `props` is the formatting for the new text (default: the formatting
    /// at the selection start). Returns the caret after the text.
    pub fn insert_text(
        &mut self,
        sel: Selection,
        text: &str,
        props: Option<RunProps>,
    ) -> Selection {
        self.insert_with(sel, text, props, None)
    }

    /// Like [`Self::insert_text`] for IME composition: preedit updates and
    /// the final commit merge into one undo step until
    /// [`Self::end_composition`].
    pub fn compose(&mut self, sel: Selection, text: &str, props: Option<RunProps>) -> Selection {
        self.insert_with(sel, text, props, Some(Kind::Composing))
    }

    /// The composition is committed or cancelled; later edits are new steps.
    pub fn end_composition(&mut self) {
        self.history.sealed = true;
    }

    fn insert_with(
        &mut self,
        sel: Selection,
        text: &str,
        props: Option<RunProps>,
        kind: Option<Kind>,
    ) -> Selection {
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        let props = props.unwrap_or_else(|| self.run_props_at(start));
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut merged = self.cut(start, end);
        let mut pieces = text.split('\n');
        let first = pieces.next().unwrap_or_default();
        merged.insert(start.offset, first, props.clone());
        let mut caret = Position::new(start.para, start.offset + first.len());
        let mut out = vec![merged];
        for piece in pieces {
            let last = out.last_mut().expect("non-empty");
            let at_end = caret.offset == last.text.len();
            let mut right = last.split_off(caret.offset);
            // Enter at the end of a heading continues with its "next" style.
            if at_end && right.text.is_empty() {
                right.props.style = self.styles.next_style(last.props.style.as_deref());
                if right.props.style.as_deref() == Some("Normal")
                    && self.styles.get("Normal").is_some_and(|s| s.is_default)
                {
                    right.props.style = None;
                }
            }
            right.insert(0, piece, props.clone());
            caret = Position::new(caret.para + 1, piece.len());
            out.push(right);
        }
        let after = Selection::caret(caret);
        let typing = sel.is_empty() && !text.contains('\n') && text.chars().count() == 1;
        let boundary = text.chars().all(char::is_whitespace);
        self.replace(
            start.para,
            end.para - start.para + 1,
            out,
            (sel, after),
            kind.unwrap_or(if typing { Kind::Typing } else { Kind::Other }),
            boundary,
        );
        after
    }

    /// Delete the selection; returns the caret.
    pub fn delete_selection(&mut self, sel: Selection) -> Selection {
        self.delete_range(sel, Kind::Other)
    }

    fn delete_range(&mut self, sel: Selection, kind: Kind) -> Selection {
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        if start == end {
            return Selection::caret(start);
        }
        let merged = self.cut(start, end);
        let after = Selection::caret(start);
        let boundary = start.para != end.para;
        self.replace(
            start.para,
            end.para - start.para + 1,
            vec![merged],
            (sel, after),
            kind,
            boundary,
        );
        after
    }

    /// Backspace: delete the selection, or the grapheme before the caret
    /// (joining with the previous paragraph at its start).
    pub fn backspace(&mut self, sel: Selection) -> Selection {
        if !sel.is_empty() {
            return self.delete_selection(sel);
        }
        let head = self.clamp(sel.head);
        let prev = self.prev_position(head);
        self.delete_range(
            Selection {
                anchor: head,
                head: prev,
            },
            Kind::Deleting,
        )
    }

    /// Delete: the selection, or the grapheme after the caret.
    pub fn delete_forward(&mut self, sel: Selection) -> Selection {
        if !sel.is_empty() {
            return self.delete_selection(sel);
        }
        let head = self.clamp(sel.head);
        let next = self.next_position(head);
        let after = self.delete_range(
            Selection {
                anchor: head,
                head: next,
            },
            Kind::Deleting,
        );
        // Keep the "after" caret where it was (the range collapsed onto it).
        Selection::caret(after.head)
    }

    /// Change the character formatting of a non-empty selection.
    pub fn format(&mut self, sel: Selection, f: impl Fn(&mut RunProps)) -> bool {
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        if start == end {
            return false;
        }
        let mut out = Vec::with_capacity(end.para - start.para + 1);
        for i in start.para..=end.para {
            let mut p = self.paragraphs[i].clone();
            let a = if i == start.para { start.offset } else { 0 };
            let b = if i == end.para {
                end.offset
            } else {
                p.text.len()
            };
            if a < b {
                p.format(a..b, &f);
            }
            out.push(p);
        }
        self.replace(start.para, out.len(), out, (sel, sel), Kind::Other, true);
        true
    }

    /// Bold/italic/underline on for the whole selection, or off if it already
    /// was. Returns `false` for an empty selection (the caller keeps a
    /// pending format for the next typed text instead).
    pub fn toggle(&mut self, sel: Selection, toggle: Toggle) -> bool {
        let on = !self.is_on(sel, toggle);
        self.format(sel, |p| toggle.set(p, on))
    }

    /// Change the paragraph formatting of every paragraph the selection touches.
    pub fn format_paragraphs(&mut self, sel: Selection, f: impl Fn(&mut ParaProps)) {
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        let out: Vec<Paragraph> = self.paragraphs[start.para..=end.para]
            .iter()
            .cloned()
            .map(|mut p| {
                f(&mut p.props);
                p
            })
            .collect();
        self.replace(start.para, out.len(), out, (sel, sel), Kind::Other, true);
    }

    /// Apply a paragraph style (`None` = the default style).
    pub fn set_style(&mut self, sel: Selection, style: Option<&str>) {
        self.format_paragraphs(sel, |p| p.style = style.map(str::to_owned));
    }

    pub fn set_align(&mut self, sel: Selection, align: Align) {
        self.format_paragraphs(sel, |p| p.align = Some(align));
    }

    /// Undo the last step; returns the selection to restore.
    pub fn undo(&mut self) -> Option<Selection> {
        let cmd = self.history.undo.pop()?;
        self.paragraphs.splice(
            cmd.start..cmd.start + cmd.new.len(),
            cmd.old.iter().cloned(),
        );
        let sel = cmd.before;
        self.history.redo.push(cmd);
        Some(self.clamp_selection(sel))
    }

    pub fn redo(&mut self) -> Option<Selection> {
        let cmd = self.history.redo.pop()?;
        self.paragraphs.splice(
            cmd.start..cmd.start + cmd.old.len(),
            cmd.new.iter().cloned(),
        );
        let sel = cmd.after;
        self.history.undo.push(cmd);
        Some(self.clamp_selection(sel))
    }

    // ── saving ─────────────────────────────────────────────────────────────

    /// Save to the file it was opened from. Fails (without writing) when the
    /// document is untitled or has unsupported content; use [`Self::save_as`].
    pub fn save(&mut self) -> anyhow::Result<()> {
        let Some(path) = self.path.clone() else {
            anyhow::bail!("This document has no file yet; use Save As");
        };
        if self.has_unsupported_content() {
            anyhow::bail!(
                "{} has content Kantor can't keep ({}); use Save As to write a new file",
                self.title(),
                self.unsupported.join(", ")
            );
        }
        self.write(&path)
    }

    /// Save to `path` and make it this document's file. Refuses to write over
    /// the original of a document with unsupported content.
    pub fn save_as(&mut self, path: &Path) -> anyhow::Result<()> {
        if self.has_unsupported_content()
            && let Some(orig) = self.path.as_deref()
            && same_file(orig, path)
        {
            anyhow::bail!(
                "Not overwriting {}: it has content Kantor can't keep ({}). Choose a new name.",
                self.title(),
                self.unsupported.join(", ")
            );
        }
        self.write(path)?;
        self.path = Some(path.to_path_buf());
        // The new file holds exactly what's in memory.
        self.unsupported.clear();
        Ok(())
    }

    fn write(&mut self, path: &Path) -> anyhow::Result<()> {
        let bytes = docx::write(self, self.package.as_ref())?;
        atomic_write(path, &bytes)?;
        // The written file is the template for the next save.
        self.package = Some(docx::read(bytes)?.package);
        self.history.saved = self.history.top();
        Ok(())
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Write to a temporary file next to `path`, then rename over it.
fn atomic_write(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write as _;
    let dir = path
        .parent()
        .filter(|d| !d.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("Can't write {}: {}", path.display(), e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(paras: &[&str]) -> Document {
        Document::from_paragraphs(paras.iter().map(|t| Paragraph::new(*t)).collect())
    }

    fn texts(d: &Document) -> Vec<&str> {
        d.paragraphs().iter().map(|p| p.text.as_str()).collect()
    }

    fn sel(a: (usize, usize), b: (usize, usize)) -> Selection {
        Selection {
            anchor: Position::new(a.0, a.1),
            head: Position::new(b.0, b.1),
        }
    }

    #[test]
    fn typing_coalesces_per_word_and_undoes() {
        let mut d = doc(&[""]);
        let mut caret = Selection::default();
        for ch in "hello world".chars() {
            caret = d.insert_text(caret, &ch.to_string(), None);
        }
        assert_eq!(texts(&d), vec!["hello world"]);
        assert!(d.is_dirty());
        d.undo();
        assert_eq!(texts(&d), vec!["hello"]);
        d.undo();
        assert_eq!(texts(&d), vec![""]);
        assert!(!d.is_dirty());
        d.redo();
        d.redo();
        assert_eq!(texts(&d), vec!["hello world"]);
    }

    #[test]
    fn enter_splits_and_backspace_joins() {
        let mut d = doc(&["abcdef"]);
        let c = d.insert_text(sel((0, 3), (0, 3)), "\n", None);
        assert_eq!(texts(&d), vec!["abc", "def"]);
        assert_eq!(c.head, Position::new(1, 0));
        let c = d.backspace(c);
        assert_eq!(texts(&d), vec!["abcdef"]);
        assert_eq!(c.head, Position::new(0, 3));
    }

    #[test]
    fn enter_after_heading_gives_normal() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Title").with_style("Heading1")]);
        d.insert_text(sel((0, 5), (0, 5)), "\n", None);
        assert_eq!(d.paragraph(1).props.style, None);
        // Splitting in the middle keeps the heading on both halves.
        d.insert_text(sel((0, 2), (0, 2)), "\n", None);
        assert_eq!(d.paragraph(1).props.style.as_deref(), Some("Heading1"));
    }

    #[test]
    fn multi_paragraph_delete_and_paste() {
        let mut d = doc(&["one", "two", "three"]);
        let c = d.delete_selection(sel((0, 1), (2, 2)));
        assert_eq!(texts(&d), vec!["oree"]);
        d.insert_text(c, "x\ny\nz", None);
        assert_eq!(texts(&d), vec!["ox", "y", "zree"]);
        d.undo();
        d.undo();
        assert_eq!(texts(&d), vec!["one", "two", "three"]);
    }

    #[test]
    fn toggle_bold_over_range() {
        let mut d = doc(&["hello world"]);
        let s = sel((0, 0), (0, 5));
        assert!(!d.is_on(s, Toggle::Bold));
        d.toggle(s, Toggle::Bold);
        assert!(d.is_on(s, Toggle::Bold));
        assert_eq!(d.paragraph(0).runs.len(), 2);
        d.toggle(s, Toggle::Bold);
        assert!(!d.is_on(s, Toggle::Bold));
        d.undo();
        assert!(d.is_on(s, Toggle::Bold));
    }

    #[test]
    fn delete_forward_at_paragraph_end_joins() {
        let mut d = doc(&["ab", "cd"]);
        let c = d.delete_forward(sel((0, 2), (0, 2)));
        assert_eq!(texts(&d), vec!["abcd"]);
        assert_eq!(c.head, Position::new(0, 2));
    }

    #[test]
    fn composition_is_one_undo_step() {
        let mut d = doc(&["ab"]);
        let at = sel((0, 1), (0, 1));
        d.compose(at, "k", None);
        d.compose(sel((0, 1), (0, 2)), "ka", None);
        d.compose(sel((0, 1), (0, 3)), "か", None);
        d.end_composition();
        assert_eq!(texts(&d), vec!["aかb"]);
        let c = d.insert_text(sel((0, 4), (0, 4)), "x", None);
        assert_eq!(c.head, Position::new(0, 5));
        d.undo();
        assert_eq!(texts(&d), vec!["aかb"]);
        d.undo();
        assert_eq!(texts(&d), vec!["ab"]);
        assert!(!d.can_undo());
        // A cancelled composition leaves no step behind.
        d.compose(at, "k", None);
        d.compose(sel((0, 1), (0, 2)), "", None);
        d.end_composition();
        assert_eq!(texts(&d), vec!["ab"]);
        assert!(!d.can_undo());
    }

    #[test]
    fn text_in_range() {
        let d = doc(&["one", "two\u{2028}lines", "three"]);
        assert_eq!(
            d.text_in(Position::new(0, 1), Position::new(2, 2)),
            "ne\ntwo\nlines\nth"
        );
    }
}
