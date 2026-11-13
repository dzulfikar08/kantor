//! A document: paragraphs, styles, page geometry, its file, and the undo
//! history.
//!
//! Every edit is one command, "replace paragraphs `start..start+old.len()`
//! with `new`", which makes undo/redo generic: undo puts `old` back. Table
//! structure edits also swap the table map (`tables`). Typing and deleting
//! coalesce into one step per word.
//!
//! Containers: the body and every table cell. An edit never merges
//! paragraphs of different containers; a selection across them is cleared
//! paragraph by paragraph instead (whole tables inside a body selection
//! are removed with it).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::docx::{self, Media, Package};
use crate::model::*;
use crate::numbering::{LevelDef, Numbering};
use crate::section::{HfState, Story};
use crate::style::{Proofing, ResolvedPara, ResolvedRun, StyleSheet};
use crate::table::{Table, Tables};

/// Line break inside a paragraph (`w:br`). U+2028 is a mandatory break for
/// the line breaker, so layout needs no special case.
pub const LINE_BREAK: char = '\u{2028}';

/// Page break inside a paragraph (`w:br w:type="page"`).
pub const PAGE_BREAK: char = '\u{000C}';

/// A picture inserted in Kantor, written as a new part on save.
#[derive(Clone, Debug)]
pub struct NewMedia {
    pub rel_id: String,
    pub part: String,
    pub bytes: Arc<[u8]>,
}

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
pub(crate) enum Kind {
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
    /// Table map before / after (structure edits only).
    tables: Option<Box<(Tables, Tables)>>,
    /// Page setup before / after (page setup changes only).
    page: Option<(PageGeometry, PageGeometry)>,
    /// Headers, footers and their settings before / after.
    hf: Option<Box<(HfState, HfState)>>,
}

#[derive(Clone, Debug, Default)]
struct History {
    undo: Vec<Command>,
    redo: Vec<Command>,
    next_id: u64,
    /// Id of the command on top of the undo stack when last saved (`None`:
    /// the stack was empty).
    saved: Option<u64>,
    /// Don't merge the next command into the top one (composition ended).
    sealed: bool,
    /// Counts edits, undos and redos (see [`Document::revision`]).
    changes: u64,
}

impl History {
    fn top(&self) -> Option<u64> {
        self.undo.last().map(|c| c.id)
    }

    /// Record `cmd`; merge it into the previous step when it continues the
    /// same word of typing or deleting.
    fn push(&mut self, mut cmd: Command, word_boundary: bool) {
        self.changes += 1;
        self.redo.clear();
        let sealed = std::mem::take(&mut self.sealed);
        if let Some(last) = self.undo.last_mut()
            && !sealed
            && cmd.kind != Kind::Other
            && cmd.kind == last.kind
            && cmd.tables.is_none()
            && last.tables.is_none()
            && cmd.page.is_none()
            && last.page.is_none()
            && cmd.hf.is_none()
            && last.hf.is_none()
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
        // A kept original element (e.g. a double underline) gives way.
        match self {
            Self::Bold => {
                p.bold = Some(on);
                p.extras.remove("b");
            }
            Self::Italic => {
                p.italic = Some(on);
                p.extras.remove("i");
            }
            Self::Underline => {
                p.underline = Some(on);
                p.extras.remove("u");
            }
        }
    }
}

#[derive(Clone)]
pub struct Document {
    pub(crate) paragraphs: Vec<Paragraph>,
    pub(crate) tables: Tables,
    pub styles: StyleSheet,
    /// List definitions (`numbering.xml`); Kantor only ever adds to them.
    pub numbering: Numbering,
    pub page: PageGeometry,
    path: Option<PathBuf>,
    /// The `.docx` package this document was read from: Save copies its
    /// parts and regenerates only the body.
    pub(crate) package: Option<Package>,
    /// Pictures by relationship id.
    pub(crate) media: HashMap<String, Media>,
    /// Pictures inserted since the last save.
    pub(crate) new_media: Vec<NewMedia>,
    /// Content the model doesn't hold (empty for a fully modelled file).
    unsupported: Vec<String>,
    /// Content kept verbatim but only shown as a placeholder.
    preserved: Vec<String>,
    /// The body's final `w:sectPr` (the last section's settings other than
    /// size and margins, which are [`Self::page`]); `None` for new
    /// documents until headers or footers are added.
    pub(crate) body_sect: Option<String>,
    /// Header and footer parts.
    pub(crate) stories: Vec<Story>,
    /// Different odd and even page headers (settings.xml).
    pub(crate) even_odd: bool,
    /// Comments (`comments.xml`) and whether Kantor changed them.
    pub(crate) comments: Vec<crate::review::Comment>,
    pub(crate) comments_modified: bool,
    /// `w:defaultTabStop` (twips).
    pub(crate) default_tab: u32,
    /// Track Changes is on (`w:trackRevisions`).
    pub(crate) track: bool,
    /// Who tracked edits are by.
    pub(crate) author: String,
    /// Footnotes and endnotes (separators included), in file order.
    pub(crate) notes: Vec<crate::notes::Note>,
    /// Note numbering from settings.xml (footnotes, endnotes).
    pub(crate) note_settings: [crate::notes::NoteProps; 2],
    /// The file this document was imported from (`.rtf`), while it has
    /// no file of its own.
    imported: Option<Imported>,
    history: History,
}

/// Where an imported document came from.
#[derive(Clone, Debug)]
struct Imported {
    source: PathBuf,
    notes: Vec<String>,
}

/// Whether Kantor opens `path` as a read-only import (Save writes a new
/// `.docx`) rather than a `.docx` file.
pub fn is_import_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("rtf"))
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
            tables: Tables::new(),
            styles: StyleSheet::builtin(),
            numbering: Numbering::default(),
            page: PageGeometry::default(),
            path: None,
            package: None,
            media: HashMap::new(),
            new_media: Vec::new(),
            unsupported: Vec::new(),
            preserved: Vec::new(),
            body_sect: None,
            stories: Vec::new(),
            even_odd: false,
            comments: Vec::new(),
            comments_modified: false,
            default_tab: 720,
            track: false,
            author: "Kantor".to_owned(),
            notes: Vec::new(),
            note_settings: crate::notes::NoteKind::ALL.map(crate::notes::NoteProps::default_for),
            imported: None,
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

    /// Open a `.docx` file, or import an `.rtf` one (see [`is_import_path`]).
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("Can't read {}: {e}", path.display()))?;
        if is_import_path(path) {
            return Self::import_rtf(&bytes, path)
                .map_err(|e| anyhow::anyhow!("Can't open {}: {e:#}", path.display()));
        }
        let read = docx::read(bytes)
            .map_err(|e| anyhow::anyhow!("Can't open {}: {e:#}", path.display()))?;
        Ok(Self::from_read(read, Some(path.to_path_buf())))
    }

    /// An `.rtf` file's contents as a new, unsaved document that remembers
    /// `source` (for its name and Save As).
    pub fn import_rtf(bytes: &[u8], source: &Path) -> anyhow::Result<Self> {
        let import = crate::rtf::to_docx(bytes)?;
        let read = docx::read(import.docx)?;
        let mut doc = Self::from_read(read, None);
        doc.imported = Some(Imported {
            source: source.to_path_buf(),
            notes: import.notes,
        });
        Ok(doc)
    }

    /// Opened from a format Kantor doesn't write, and not saved yet.
    pub fn is_imported(&self) -> bool {
        self.path.is_none() && self.imported.is_some()
    }

    /// The file this document shows: its own, or the one it was imported
    /// from (which Save never writes).
    pub fn source_path(&self) -> Option<&Path> {
        self.path
            .as_deref()
            .or(self.imported.as_ref().map(|i| i.source.as_path()))
    }

    /// What an import couldn't bring over ("footnotes were left out").
    pub fn import_notes(&self) -> &[String] {
        match &self.imported {
            Some(i) if self.path.is_none() => &i.notes,
            _ => &[],
        }
    }

    /// A document from read `.docx` contents.
    pub fn from_read(read: docx::Read, path: Option<PathBuf>) -> Self {
        let body_sect = read.package.sect_pr.clone();
        Self {
            paragraphs: if read.paragraphs.is_empty() {
                vec![Paragraph::default()]
            } else {
                read.paragraphs
            },
            tables: read.tables,
            styles: read.styles,
            numbering: read.numbering,
            page: read.page,
            path,
            package: Some(read.package),
            media: read.media,
            new_media: Vec::new(),
            unsupported: read.unsupported,
            preserved: read.preserved,
            body_sect,
            stories: read.stories,
            even_odd: read.even_odd,
            comments: read.comments,
            comments_modified: false,
            default_tab: read.default_tab,
            track: read.track,
            author: "Kantor".to_owned(),
            notes: read.notes,
            note_settings: read.note_settings,
            imported: None,
            history: History::default(),
        }
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
        self.source_path()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Untitled.docx".to_owned())
    }

    /// Default name for Save As.
    pub fn suggested_file_name(&self) -> String {
        match self.source_path().and_then(|p| p.file_stem()) {
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

    /// Changes with every edit, undo and redo (typing a word is several), so
    /// callers can tell the document changed.
    pub fn revision(&self) -> u64 {
        self.history.changes
    }

    /// Unsaved, whatever the undo history says (a restored recovery copy).
    pub fn mark_modified(&mut self) {
        self.history.saved = Some(u64::MAX);
    }

    /// The `.docx` Save would write, without saving (recovery copies).
    pub fn to_docx_bytes(&self) -> anyhow::Result<Vec<u8>> {
        docx::write(self, self.package.as_ref())
    }

    /// A recovery copy (see [`Self::to_docx_bytes`]) as the document it was
    /// made from: `original` with unsaved changes. `unsupported` is what the
    /// original had that Kantor can't keep, so Save still won't overwrite it.
    pub fn open_recovered(
        bytes: Vec<u8>,
        original: Option<PathBuf>,
        unsupported: Vec<String>,
    ) -> anyhow::Result<Self> {
        let read = docx::read(bytes)
            .map_err(|e| anyhow::anyhow!("Can't read the recovery copy: {e:#}"))?;
        // A recovered import still has no file of its own.
        let (original, imported) = match original {
            Some(p) if is_import_path(&p) => (
                None,
                Some(Imported {
                    source: p,
                    notes: Vec::new(),
                }),
            ),
            other => (other, None),
        };
        let mut doc = Self::from_read(read, original);
        doc.imported = imported;
        for u in unsupported {
            if !doc.unsupported.contains(&u) {
                doc.unsupported.push(u);
            }
        }
        doc.mark_modified();
        Ok(doc)
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

    /// Content kept byte for byte on save but shown only as a placeholder
    /// (or not at all), e.g. "floating images", "headers and footers".
    pub fn preserved_content(&self) -> &[String] {
        &self.preserved
    }

    pub fn table(&self, id: u32) -> Option<&Table> {
        self.tables.get(&id)
    }

    /// Bytes of a picture (PNG or JPEG), by relationship id.
    pub fn image_bytes(&self, rel_id: &str) -> Option<Arc<[u8]>> {
        self.media.get(rel_id).and_then(|m| m.bytes.clone())
    }

    pub fn new_media(&self) -> &[NewMedia] {
        &self.new_media
    }

    /// The table style of the table holding `p`.
    pub fn table_style_of(&self, p: &Paragraph) -> Option<&str> {
        let c = p.cell?;
        self.tables.get(&c.table)?.style.as_deref()
    }

    fn style_num(&self) -> impl Fn(&str) -> Option<u32> + '_ {
        |id| self.styles.get(id).and_then(|s| s.para.num).map(|n| n.id)
    }

    /// The list level a paragraph uses, if it's in a list.
    pub fn list_level(&self, p: &Paragraph) -> Option<&LevelDef> {
        let r = self.styles.effective_num(p)?;
        self.numbering.level(r, &self.style_num())
    }

    /// Effective paragraph formatting (style, table style, list indents).
    pub fn resolve_para(&self, p: &Paragraph) -> ResolvedPara {
        let link = self.style_num();
        self.styles
            .resolve_para_with(p, self.table_style_of(p), &|r| {
                self.numbering.level(r, &link).map(|l| l.indent)
            })
    }

    /// Effective character formatting of `run` in `p`.
    pub fn resolve_run(&self, p: &Paragraph, run: &RunProps) -> ResolvedRun {
        self.styles.resolve_run_with(p, run, self.table_style_of(p))
    }

    /// How paragraph `i`'s text is spell checked: byte ranges with their
    /// [`Proofing`], neighbours with the same one merged. Pictures and
    /// tracked deletions are skipped.
    pub fn proofing_spans(&self, i: usize) -> Vec<(std::ops::Range<usize>, Proofing)> {
        let p = &self.paragraphs[i];
        let table = self.table_style_of(p);
        let mut out: Vec<(std::ops::Range<usize>, Proofing)> = Vec::new();
        for (range, run) in p.run_ranges() {
            if range.is_empty() {
                continue;
            }
            let deleted = run.rev.as_ref().is_some_and(|r| r.kind == RevKind::Delete);
            let proof = if run.object.is_some() || deleted {
                Proofing::Skip
            } else {
                self.styles.proofing(p, &run.props, table)
            };
            match out.last_mut() {
                Some((r, last)) if *last == proof && r.end == range.start => r.end = range.end,
                _ => out.push((range, proof)),
            }
        }
        out
    }

    /// List marker text ("1.", "a)", "•") of every paragraph.
    pub fn list_markers(&self) -> Vec<Option<String>> {
        let refs: Vec<_> = self
            .paragraphs
            .iter()
            .map(|p| self.styles.effective_num(p))
            .collect();
        self.numbering.markers(refs, &self.style_num())
    }

    /// Whether Save must ask for a new file (untitled, or the original has
    /// content Kantor would drop).
    pub fn needs_save_as(&self) -> bool {
        self.path.is_none() || self.has_unsupported_content()
    }

    pub fn word_count(&self) -> usize {
        self.paragraphs
            .iter()
            .map(|p| {
                p.text
                    .split_whitespace()
                    .filter(|w| w.chars().any(|c| c != OBJECT))
                    .count()
            })
            .sum()
    }

    pub fn char_count(&self) -> usize {
        self.paragraphs
            .iter()
            .map(|p| p.text.chars().filter(|&c| c != OBJECT).count())
            .sum()
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
    /// Position markers (bookmarks) are stepped over with their neighbour.
    pub fn prev_position(&self, pos: Position) -> Position {
        if pos.offset > 0 {
            let p = &self.paragraphs[pos.para];
            let mut o = prev_grapheme(&p.text, pos.offset);
            while o > 0 && p.is_marker_at(o) {
                o = prev_grapheme(&p.text, o);
            }
            Position::new(pos.para, o)
        } else if pos.para > 0 {
            Position::new(pos.para - 1, self.paragraphs[pos.para - 1].text.len())
        } else {
            pos
        }
    }

    /// One grapheme forward, crossing into the next paragraph's start.
    pub fn next_position(&self, pos: Position) -> Position {
        let p = &self.paragraphs[pos.para];
        let text = &p.text;
        let mut o = pos.offset;
        while o < text.len() && p.is_marker_at(o) {
            o = next_grapheme(text, o);
        }
        if o < text.len() {
            Position::new(pos.para, next_grapheme(text, o))
        } else if pos.para + 1 < self.paragraphs.len() {
            Position::new(pos.para + 1, 0)
        } else {
            Position::new(pos.para, o)
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
        out.replace([LINE_BREAK, PAGE_BREAK], "\n")
            .replace(OBJECT, "")
    }

    /// Formatting typed text takes at `pos`.
    pub fn run_props_at(&self, pos: Position) -> RunProps {
        self.paragraph(pos.para).props_at(pos.offset)
    }

    pub fn resolved_at(&self, pos: Position) -> ResolvedRun {
        let p = self.paragraph(pos.para);
        self.resolve_run(p, &p.props_at(pos.offset))
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
                    if !toggle.get(&self.resolve_run(p, &run.props)) {
                        return false;
                    }
                }
            }
        }
        any
    }

    // ── edits ──────────────────────────────────────────────────────────────

    pub(crate) fn replace(
        &mut self,
        start: usize,
        count: usize,
        new: Vec<Paragraph>,
        (before, after): (Selection, Selection),
        kind: Kind,
        word_boundary: bool,
    ) {
        self.replace_with_tables(
            start,
            count,
            new,
            None,
            (before, after),
            kind,
            word_boundary,
        );
    }

    /// [`Self::replace`] that also swaps in a new table map.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn replace_with_tables(
        &mut self,
        start: usize,
        count: usize,
        new: Vec<Paragraph>,
        tables: Option<Tables>,
        (before, after): (Selection, Selection),
        kind: Kind,
        word_boundary: bool,
    ) {
        let old: Vec<Paragraph> = self
            .paragraphs
            .splice(start..start + count, new.iter().cloned())
            .collect();
        if self.paragraphs.is_empty() {
            self.paragraphs.push(Paragraph::default());
        }
        let tables =
            tables.map(|t| Box::new((std::mem::replace(&mut self.tables, t), self.tables.clone())));
        if old == new && tables.as_ref().is_none_or(|t| t.0 == t.1) {
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
                tables,
                page: None,
                hf: None,
            },
            word_boundary,
        );
    }

    /// Give the step on top of the undo stack side state to restore.
    pub(crate) fn set_last_side(&mut self, before: HfState, after: HfState) {
        if let Some(c) = self.history.undo.last_mut() {
            c.hf = Some(Box::new((before, after)));
        }
    }

    /// Record a step that replaced paragraphs `start..start + old.len()`
    /// with `new` (already applied) and changed headers/footers.
    pub(crate) fn push_command(
        &mut self,
        start: usize,
        old: Vec<Paragraph>,
        new: Vec<Paragraph>,
        sel: Selection,
        hf: Option<Box<(HfState, HfState)>>,
    ) {
        self.history.push(
            Command {
                id: 0,
                start,
                old,
                new,
                before: sel,
                after: sel,
                kind: Kind::Other,
                tables: None,
                page: None,
                hf,
            },
            true,
        );
    }

    /// Paragraph index range of a table.
    pub fn table_range(&self, id: u32) -> std::ops::Range<usize> {
        let first = self
            .paragraphs
            .iter()
            .position(|p| p.cell.is_some_and(|c| c.table == id));
        match first {
            Some(a) => {
                let len = self.paragraphs[a..]
                    .iter()
                    .take_while(|p| p.cell.is_some_and(|c| c.table == id))
                    .count();
                a..a + len
            }
            None => 0..0,
        }
    }

    /// Whether paragraphs `a < b` can be merged: same container, and in the
    /// body only whole tables between them.
    pub fn can_merge(&self, a: usize, b: usize) -> bool {
        let (ca, cb) = (self.paragraphs[a].cell, self.paragraphs[b].cell);
        if ca != cb {
            return false;
        }
        if ca.is_some() {
            return true;
        }
        let mut i = a + 1;
        while i < b {
            match self.paragraphs[i].cell {
                Some(c) => {
                    let r = self.table_range(c.table);
                    if r.start <= a || r.end > b {
                        return false;
                    }
                    i = r.end;
                }
                None => i += 1,
            }
        }
        true
    }

    /// The paragraphs `start.para..=end.para` with the range removed: merged
    /// into one when they share a container, else each one trimmed.
    fn cut(&self, start: Position, end: Position) -> Vec<Paragraph> {
        let mut first = self.paragraphs[start.para].clone();
        if start.para == end.para {
            first.delete(start.offset..end.offset);
            return vec![first];
        }
        if !self.can_merge(start.para, end.para) {
            return (start.para..=end.para)
                .map(|i| {
                    let mut p = self.paragraphs[i].clone();
                    let a = if i == start.para { start.offset } else { 0 };
                    let b = if i == end.para {
                        end.offset
                    } else {
                        p.text.len()
                    };
                    p.delete(a..b);
                    p
                })
                .collect();
        }
        let len = first.text.len();
        first.delete(start.offset..len);
        // Bookmarks in the paragraphs that disappear stay too (`delete`
        // keeps the ones in `first` and `last` itself).
        let mut markers = Vec::new();
        for p in &self.paragraphs[start.para + 1..end.para] {
            markers.extend(p.markers_in(0..p.text.len()));
        }
        let at = first.text.len();
        first.insert_runs(at, markers);
        let mut last = self.paragraphs[end.para].clone();
        last.delete(0..end.offset);
        first.append(last);
        vec![first]
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
        if self.track {
            return self.tracked_insert(sel, text, props, kind);
        }
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        let props = props.unwrap_or_else(|| self.run_props_at(start));
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut rest = self.cut(start, end);
        let mut merged = rest.remove(0);
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
        out.extend(rest);
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
        if self.track {
            return self.tracked_delete(sel, kind, false);
        }
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        if start == end {
            return Selection::caret(start);
        }
        let out = self.cut(start, end);
        let after = Selection::caret(start);
        let boundary = start.para != end.para;
        self.replace(
            start.para,
            end.para - start.para + 1,
            out,
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
        if head.offset == 0 || self.only_markers_before(head) {
            // At the start of a list item, Backspace ends the list first.
            if self
                .styles
                .effective_num(&self.paragraphs[head.para])
                .is_some()
            {
                self.remove_list_at(head.para, sel);
                return Selection::caret(head);
            }
            if head.para > 0 && !self.can_merge(head.para - 1, head.para) {
                return Selection::caret(self.prev_position(Position::new(head.para, 0)));
            }
        }
        let prev = self.prev_position(head);
        self.delete_range(
            Selection {
                anchor: head,
                head: prev,
            },
            Kind::Deleting,
        )
    }

    /// Whether only position markers precede `pos` in its paragraph.
    fn only_markers_before(&self, pos: Position) -> bool {
        pos.offset > 0
            && self.paragraphs[pos.para]
                .run_ranges()
                .filter(|(r, _)| r.start < pos.offset)
                .all(|(_, run)| run.object.as_ref().is_some_and(Object::is_marker))
    }

    /// Take paragraph `i` out of its list (keeps a style's list off with
    /// `numId` 0).
    pub(crate) fn remove_list_at(&mut self, i: usize, sel: Selection) {
        let mut p = self.paragraphs[i].clone();
        let style_has_list = {
            let mut q = p.clone();
            q.props.num = None;
            self.styles.effective_num(&q).is_some()
        };
        p.props.num = style_has_list.then_some(NumRef { id: 0, level: 0 });
        p.props.indent = Indent::default();
        self.replace(i, 1, vec![p], (sel, sel), Kind::Other, true);
    }

    /// Delete: the selection, or the grapheme after the caret.
    pub fn delete_forward(&mut self, sel: Selection) -> Selection {
        if !sel.is_empty() {
            return self.delete_selection(sel);
        }
        let head = self.clamp(sel.head);
        let next = self.next_position(head);
        if next.para != head.para && !self.can_merge(head.para, next.para) {
            return Selection::caret(head);
        }
        if self.track {
            // The caret moves past what's now struck through, as in Word.
            return self.tracked_delete(
                Selection {
                    anchor: head,
                    head: next,
                },
                Kind::Deleting,
                true,
            );
        }
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

    /// Change paper size, orientation or margins: one undo step. `sel` is
    /// restored on undo/redo.
    pub fn set_page(&mut self, page: PageGeometry, sel: Selection) {
        let page = page.clamped();
        if page == self.page {
            return;
        }
        let old = std::mem::replace(&mut self.page, page);
        self.history.push(
            Command {
                id: 0,
                start: 0,
                old: Vec::new(),
                new: Vec::new(),
                before: sel,
                after: sel,
                kind: Kind::Other,
                tables: None,
                page: Some((old, page)),
                hf: None,
            },
            true,
        );
    }

    /// Set direct indents (twips; `first` negative = hanging) on every
    /// paragraph the selection touches; `None` leaves that side alone. One
    /// undo step. A kept original `w:ind` gives way to the new values.
    pub fn set_indent(&mut self, sel: Selection, indent: Indent) {
        let resolved: Vec<_> = {
            let (start, end) = self.clamp_selection(sel).ordered();
            (start.para..=end.para)
                .map(|i| self.resolve_para(&self.paragraphs[i]))
                .collect()
        };
        let i = std::cell::Cell::new(0);
        self.format_paragraphs(sel, |p| {
            let r = &resolved[i.get().min(resolved.len() - 1)];
            i.set(i.get() + 1);
            let tw = |pt: f32| (pt * 20.0).round() as i32;
            // Keep the effective values of sides not being changed.
            p.indent.left = Some(indent.left.unwrap_or(tw(r.indent_left_pt)));
            p.indent.right = Some(indent.right.unwrap_or(tw(r.indent_right_pt)));
            p.indent.first = Some(indent.first.unwrap_or(tw(r.first_line_pt)));
            p.extras.remove("ind");
        });
    }

    /// Undo the last step; returns the selection to restore.
    pub fn undo(&mut self) -> Option<Selection> {
        let cmd = self.history.undo.pop()?;
        self.history.changes += 1;
        self.paragraphs.splice(
            cmd.start..cmd.start + cmd.new.len(),
            cmd.old.iter().cloned(),
        );
        if let Some(t) = &cmd.tables {
            self.tables = t.0.clone();
        }
        if let Some((old, _)) = cmd.page {
            self.page = old;
        }
        if let Some(hf) = &cmd.hf {
            self.set_hf_state(hf.0.clone());
        }
        let sel = cmd.before;
        self.history.redo.push(cmd);
        Some(self.clamp_selection(sel))
    }

    pub fn redo(&mut self) -> Option<Selection> {
        let cmd = self.history.redo.pop()?;
        self.history.changes += 1;
        self.paragraphs.splice(
            cmd.start..cmd.start + cmd.old.len(),
            cmd.new.iter().cloned(),
        );
        if let Some(t) = &cmd.tables {
            self.tables = t.1.clone();
        }
        if let Some((_, new)) = cmd.page {
            self.page = new;
        }
        if let Some(hf) = &cmd.hf {
            self.set_hf_state(hf.1.clone());
        }
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
        let read = docx::read(bytes)?;
        self.package = Some(read.package);
        // Written pictures and list definitions are in the package now.
        let pkg = self.package.as_ref().expect("just set");
        self.new_media.retain(|m| !pkg.has_rel(&m.rel_id));
        self.numbering.added_abstracts.clear();
        self.numbering.added_nums.clear();
        self.comments_modified = false;
        // Written stories are in the package now.
        for st in &mut self.stories {
            st.modified = false;
            st.new = false;
        }
        self.body_sect = self.package.as_ref().and_then(|p| p.sect_pr.clone());
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
    // The temp file is private (0600): keep the permissions the file had.
    if let Ok(meta) = std::fs::metadata(path) {
        std::fs::set_permissions(tmp.path(), meta.permissions())?;
    }
    tmp.persist(path)
        .map_err(|e| anyhow::anyhow!("Can't write {}: {}", path.display(), e.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn saving_keeps_the_file_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.docx");
        let mut d = doc(&["one"]);
        d.save_as(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        d.save().unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
    }

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
