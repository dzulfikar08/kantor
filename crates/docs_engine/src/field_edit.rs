//! Making and changing fields as Word's Insert ▸ Field…, field codes
//! (Alt+F9 / Shift+F9, editing a code), Insert ▸ Bookmark… and
//! Insert ▸ Cross-reference…, plus updating the fields of headers,
//! footers and notes (see [`crate::fields`] for what fields show).
//!
//! Every change here is one undo step; a new or changed field gets its
//! result straight away, as in Word.

use std::collections::HashSet;
use std::sync::Arc;

use crate::document::{Document, Kind};
use crate::fields::{self, FieldContext, PageLookup};
use crate::model::{OBJECT, Object, Paragraph, Position, Run, RunProps, Selection};
use crate::notes::NoteKind;
use crate::object::{FieldMark, Level, Opaque};
use crate::section::HfState;

const OBJ: usize = OBJECT.len_utf8();

/// What a cross-reference points at (Word's "Reference type").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum XrefKind {
    Heading,
    Bookmark,
    Footnote,
    Endnote,
    /// Captions numbered with `SEQ <label>` (Figure, Table, Equation).
    Caption(String),
}

/// What a cross-reference shows (Word's "Insert reference to").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum XrefWhat {
    /// The heading's or bookmark's text, the whole caption.
    Text,
    PageNumber,
    /// The heading's paragraph number, the note's number.
    Number,
    /// "above" or "below".
    AboveBelow,
}

/// Something a cross-reference can point at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XrefTarget {
    /// What the list shows.
    pub label: String,
    /// A bookmark already on it.
    pub bookmark: Option<String>,
    /// Where to bookmark it otherwise (start, end).
    pub range: (Position, Position),
}

/// A bookmark start or end marker.
fn marker(xml: String) -> Object {
    Object::Opaque(Arc::new(Opaque {
        level: Level::Marker,
        xml,
        label: String::new(),
    }))
}

fn bookmark_parts(o: &Object) -> Option<(bool, String, String)> {
    let Object::Opaque(op) = o else { return None };
    if op.level != Level::Marker {
        return None;
    }
    let start = match crate::model::element_name(&op.xml) {
        "bookmarkStart" => true,
        "bookmarkEnd" => false,
        _ => return None,
    };
    let el = crate::xml::parse(&op.xml)?;
    Some((
        start,
        el.attr("id").unwrap_or_default().to_owned(),
        el.attr("name").unwrap_or_default().to_owned(),
    ))
}

/// Whether `name` can name a bookmark (Word: a letter first, then
/// letters, digits and underscores, at most 40).
pub fn valid_bookmark_name(name: &str) -> bool {
    let mut c = name.chars();
    c.next().is_some_and(char::is_alphabetic)
        && name.chars().count() <= 40
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

impl Document {
    /// Commit `paras` as the new body (only what differs is replaced): one
    /// undo step from `sel` to `after`.
    fn commit_paras(&mut self, paras: Vec<Paragraph>, sel: Selection, after: Selection) {
        if paras == self.paragraphs {
            return;
        }
        let old = &self.paragraphs;
        let start = old.iter().zip(&paras).take_while(|(a, b)| a == b).count();
        let tail = old[start..]
            .iter()
            .rev()
            .zip(paras[start..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let count = old.len() - start - tail;
        let new: Vec<Paragraph> = paras[start..paras.len() - tail].to_vec();
        let new = self.track_rewrite(&self.paragraphs[start..start + count], new);
        self.replace(start, count, new, (sel, after), Kind::Other, true);
    }

    /// `paras` with the fields `ids` updated.
    fn updated(
        &self,
        paras: Vec<Paragraph>,
        ids: Vec<usize>,
        cx: &FieldContext,
        lay_out: &mut dyn FnMut(&Document) -> Box<dyn PageLookup>,
    ) -> Vec<Paragraph> {
        let mut snap = self.snapshot(paras);
        snap.update_targets(Selection::default(), ids, cx, lay_out);
        snap.paragraphs
    }

    /// Insert a field with instruction `instr` at the selection (Insert ▸
    /// Field…, Ctrl+F9 with an empty one), its result worked out: one undo
    /// step. Page number fields become the objects headers use. Returns
    /// the selection after it.
    pub fn insert_field(
        &mut self,
        sel: Selection,
        instr: &str,
        cx: &FieldContext,
        lay_out: &mut dyn FnMut(&Document) -> Box<dyn PageLookup>,
    ) -> Selection {
        let instr = format!(" {} ", instr.trim());
        let sel = self.clamp_selection(sel);
        let kind = fields::Instr::parse(&instr).kind;
        if matches!(kind.as_str(), "PAGE" | "NUMPAGES" | "SECTIONPAGES") {
            let f = match kind.as_str() {
                "PAGE" => crate::section::PageField::Page,
                "NUMPAGES" => crate::section::PageField::NumPages,
                _ => crate::section::PageField::SectionPages,
            };
            return self.insert_object(sel, f.object_with(&instr, "1"));
        }
        // The selection goes (its text is replaced, as typing would).
        let (base, at) = if sel.is_empty() {
            (self.paragraphs.clone(), sel.head)
        } else {
            let mut d = self.snapshot(self.paragraphs.clone());
            let at = d.delete_selection(sel).head;
            (std::mem::take(&mut d.paragraphs), at)
        };
        let mut paras = base;
        let p = &mut paras[at.para];
        let props = p.props_at(at.offset);
        let begin = Arc::new(FieldMark::begin_nested(&instr));
        let id = Arc::as_ptr(&begin) as usize;
        let mut b = Run::text(OBJ, props.clone());
        b.object = Some(Object::Field(begin));
        let mut e = Run::text(OBJ, props);
        e.object = Some(Object::Field(Arc::new(FieldMark::end())));
        p.insert_runs(at.offset, vec![b, e]);
        let paras = self.updated(paras, vec![id], cx, lay_out);
        // After the field's end mark.
        let end = fields::spans(&paras)
            .into_iter()
            .find(|s| s.begin == at)
            .map_or(Position::new(at.para, at.offset + 2 * OBJ), |s| {
                Position::new(s.end.para, s.end.offset + OBJ)
            });
        let after = Selection::caret(end);
        let rev = self.revision();
        self.commit_paras(paras, sel, after);
        if kind == "INDEX" {
            self.index_columns(self.revision() != rev);
        }
        after
    }

    /// Change the code of the field starting at `begin` (editing it with
    /// its code shown) and update it: one undo step. `false` when there's
    /// no field there.
    pub fn set_field_code(
        &mut self,
        begin: Position,
        instr: &str,
        sel: Selection,
        cx: &FieldContext,
        lay_out: &mut dyn FnMut(&Document) -> Box<dyn PageLookup>,
    ) -> bool {
        let Some(span) = self.fields().into_iter().find(|s| s.begin == begin) else {
            return false;
        };
        if span.shown_code() == instr.trim() {
            return true;
        }
        let (paras, id) = self.with_code(&span, instr);
        let paras = self.updated(paras, vec![id], cx, lay_out);
        let after = self.clamp_selection(sel);
        self.commit_paras(paras, sel, after);
        true
    }

    /// Change the code of the field starting at `begin` without updating
    /// its result, as typing a code in the text does in Word (F9 updates
    /// it): one undo step. `false` when there's no field there.
    pub fn set_field_instr(&mut self, begin: Position, instr: &str, sel: Selection) -> bool {
        let Some(span) = self.fields().into_iter().find(|s| s.begin == begin) else {
            return false;
        };
        if span.shown_code() == instr.trim() {
            return true;
        }
        let (paras, _) = self.with_code(&span, instr);
        let after = self.clamp_selection(sel);
        self.commit_paras(paras, sel, after);
        true
    }

    /// The paragraphs with field `span`'s code replaced by `instr`, and the
    /// new begin mark's id.
    fn with_code(&self, span: &fields::FieldSpan, instr: &str) -> (Vec<Paragraph>, usize) {
        let instr = format!(" {} ", instr.trim());
        let mut paras = self.paragraphs.clone();
        // Written as a complex field (a simple one's code is an attribute);
        // fields typed in braces are nested in its code.
        let has_result = span.result().0 != span.end;
        let new = Arc::new(match FieldMark::begin_nested(&instr) {
            FieldMark::Begin { instr, code, .. } => FieldMark::Begin {
                instr,
                code,
                sep: has_result.then(|| crate::object::FIELD_SEPARATE.into()),
                simple: false,
            },
            end => end,
        });
        // With Track Changes on, the old code's runs stay as a deletion and
        // the new ones go in as an insertion (crate::code_track), unless
        // the field itself is new.
        let new = match self.begin_run(span.begin) {
            Some((old, None)) if self.track => {
                let id = self.max_rev_id();
                let tag = |id: u64| {
                    format!(
                        "w:id=\"{id}\" w:author=\"{}\" w:date=\"{}\"",
                        crate::xml::escape(&self.author),
                        crate::review::now_utc()
                    )
                };
                crate::code_track::track(&old, &new, (&tag(id + 1), &tag(id + 2)))
                    .map_or(new, Arc::new)
            }
            _ => new,
        };
        let id = Arc::as_ptr(&new) as usize;
        set_mark(&mut paras, span.begin, Object::Field(new));
        set_mark(
            &mut paras,
            span.end,
            Object::Field(Arc::new(FieldMark::end())),
        );
        (paras, id)
    }

    // ── bookmarks ──────────────────────────────────────────────────────────

    /// Bookmark names in document order; Word's hidden ones (`_Toc…`,
    /// `_Ref…`) only with `hidden`.
    pub fn bookmark_names(&self, hidden: bool) -> Vec<String> {
        let mut seen = HashSet::new();
        self.paragraphs
            .iter()
            .flat_map(|p| p.objects())
            .filter_map(|(_, o)| bookmark_parts(o))
            .filter(|(start, _, n)| *start && (hidden || !n.starts_with('_')))
            .filter_map(|(_, _, n)| seen.insert(n.clone()).then_some(n))
            .collect()
    }

    fn next_bookmark_id(paras: &[Paragraph]) -> u64 {
        paras
            .iter()
            .flat_map(|p| p.objects())
            .filter_map(|(_, o)| bookmark_parts(o))
            .filter_map(|(_, id, _)| id.parse::<u64>().ok())
            .max()
            .map_or(0, |m| m + 1)
    }

    /// `paras` with the bookmark `name` removed (start and end).
    fn without_bookmark(paras: &mut [Paragraph], name: &str) -> bool {
        let mut ids = HashSet::new();
        for p in paras.iter() {
            for (_, o) in p.objects() {
                if let Some((true, id, n)) = bookmark_parts(o)
                    && n == name
                {
                    ids.insert(id);
                }
            }
        }
        if ids.is_empty() {
            return false;
        }
        for p in paras.iter_mut() {
            let at: Vec<usize> = p
                .objects()
                .filter(|(_, o)| bookmark_parts(o).is_some_and(|(_, id, _)| ids.contains(&id)))
                .map(|(i, _)| i)
                .collect();
            for i in at.into_iter().rev() {
                p.remove_object(i);
            }
        }
        true
    }

    /// `paras` with a new bookmark `name` from `a` to `b`.
    fn with_bookmark(paras: &mut [Paragraph], name: &str, a: Position, b: Position) {
        let id = Self::next_bookmark_id(paras);
        let mut end = Run::text(OBJ, RunProps::default());
        end.object = Some(marker(format!("<w:bookmarkEnd w:id=\"{id}\"/>")));
        let b_off = b.offset.min(paras[b.para].text.len());
        paras[b.para].insert_runs(b_off, vec![end]);
        let mut start = Run::text(OBJ, RunProps::default());
        start.object = Some(marker(format!(
            "<w:bookmarkStart w:id=\"{id}\" w:name=\"{}\"/>",
            crate::xml::escape(name)
        )));
        let a_off = a.offset.min(paras[a.para].text.len());
        paras[a.para].insert_runs(a_off, vec![start]);
    }

    /// Insert ▸ Bookmark… ▸ Add: a bookmark `name` on the selection (or at
    /// the caret): one undo step. An existing bookmark of that name moves
    /// here.
    pub fn add_bookmark(&mut self, sel: Selection, name: &str) -> Result<Selection, String> {
        if !valid_bookmark_name(name) {
            return Err(
                "Bookmark names start with a letter and have only letters, numbers and underscores"
                    .into(),
            );
        }
        let sel = self.clamp_selection(sel);
        let old = self.paragraphs.clone();
        let mut paras = old.clone();
        // An old one of that name goes first; positions after it shift.
        let ids: HashSet<String> = old
            .iter()
            .flat_map(|p| p.objects())
            .filter_map(|(_, o)| bookmark_parts(o))
            .filter(|(s, _, n)| *s && n == name)
            .map(|(_, id, _)| id)
            .collect();
        Self::without_bookmark(&mut paras, name);
        let fix = |pos: Position| -> Position {
            let removed = old[pos.para]
                .objects()
                .filter(|(i, o)| {
                    *i < pos.offset && bookmark_parts(o).is_some_and(|(_, id, _)| ids.contains(&id))
                })
                .count();
            Position::new(pos.para, pos.offset - removed * OBJ)
        };
        let (a, b) = sel.ordered();
        let (a, b) = (fix(a), fix(b));
        Self::with_bookmark(&mut paras, name, a, b);
        let after = Selection::caret(Position::new(
            b.para,
            b.offset + if a.para == b.para { 2 * OBJ } else { OBJ },
        ));
        self.commit_paras(paras, sel, after);
        Ok(self.clamp_selection(after))
    }

    /// Insert ▸ Bookmark… ▸ Delete: one undo step.
    pub fn delete_bookmark(&mut self, name: &str, sel: Selection) -> bool {
        let mut paras = self.paragraphs.clone();
        if !Self::without_bookmark(&mut paras, name) {
            return false;
        }
        let after = Selection::caret(sel.head);
        self.commit_paras(paras, sel, after);
        true
    }

    // ── cross-references ───────────────────────────────────────────────────

    /// What Insert ▸ Cross-reference… lists for `kind`.
    pub fn xref_targets(&self, kind: &XrefKind) -> Vec<XrefTarget> {
        let paras = &self.paragraphs;
        let bookmark_on = |p: &Paragraph| {
            p.objects().find_map(|(_, o)| {
                bookmark_parts(o)
                    .filter(|(s, _, n)| *s && n.starts_with("_Ref"))
                    .map(|(_, _, n)| n)
            })
        };
        let visible = |p: &Paragraph| p.text.replace(OBJECT, "").trim().to_owned();
        match kind {
            XrefKind::Heading => {
                let markers = self.list_markers();
                paras
                    .iter()
                    .enumerate()
                    .filter(|(_, p)| p.cell.is_none() && self.heading_level(p).is_some())
                    .filter(|(_, p)| !visible(p).is_empty())
                    .map(|(i, p)| {
                        let number = markers.get(i).cloned().flatten().unwrap_or_default();
                        let label = format!("{number} {}", visible(p)).trim().to_owned();
                        XrefTarget {
                            label,
                            bookmark: bookmark_on(p),
                            range: (Position::new(i, 0), Position::new(i, p.text.len())),
                        }
                    })
                    .collect()
            }
            XrefKind::Bookmark => self
                .bookmark_names(false)
                .into_iter()
                .filter_map(|n| {
                    let at = self.bookmark_position(&n)?;
                    Some(XrefTarget {
                        label: n.clone(),
                        bookmark: Some(n),
                        range: (at, at),
                    })
                })
                .collect(),
            XrefKind::Footnote | XrefKind::Endnote => {
                let want = if *kind == XrefKind::Footnote {
                    NoteKind::Footnote
                } else {
                    NoteKind::Endnote
                };
                self.note_refs()
                    .into_iter()
                    .filter(|r| r.kind == want)
                    .map(|r| {
                        let text = self.note_text(r.kind, &r.id);
                        let short: String = text.chars().take(60).collect();
                        // A bookmark right around the reference mark.
                        let p = &paras[r.pos.para];
                        let bm = p
                            .objects()
                            .filter(|(i, _)| *i + OBJ == r.pos.offset)
                            .find_map(|(_, o)| {
                                bookmark_parts(o)
                                    .filter(|(s, _, n)| *s && n.starts_with("_Ref"))
                                    .map(|(_, _, n)| n)
                            });
                        XrefTarget {
                            label: format!("{} {short}", r.label),
                            bookmark: bm,
                            range: (r.pos, Position::new(r.pos.para, r.pos.offset + OBJ)),
                        }
                    })
                    .collect()
            }
            XrefKind::Caption(label) => {
                let spans = self.fields();
                let mut out = Vec::new();
                let mut seen = HashSet::new();
                for s in spans {
                    let i = fields::Instr::parse(&s.instr);
                    if i.kind != "SEQ"
                        || !i
                            .args
                            .first()
                            .is_some_and(|a| a.eq_ignore_ascii_case(label))
                        || !seen.insert(s.begin.para)
                    {
                        continue;
                    }
                    let p = &paras[s.begin.para];
                    out.push(XrefTarget {
                        label: visible(p),
                        bookmark: bookmark_on(p),
                        range: (
                            Position::new(s.begin.para, 0),
                            Position::new(s.begin.para, p.text.len()),
                        ),
                    });
                }
                out
            }
        }
    }

    /// The caption labels used in the document (`SEQ Figure`…), Word's
    /// Figure, Table and Equation first.
    pub fn caption_labels(&self) -> Vec<String> {
        let mut out: Vec<String> = vec!["Figure".into(), "Table".into(), "Equation".into()];
        for s in self.fields() {
            let i = fields::Instr::parse(&s.instr);
            if i.kind == "SEQ"
                && let Some(a) = i.args.first()
                && !out.iter().any(|o| o.eq_ignore_ascii_case(a))
            {
                out.push(a.clone());
            }
        }
        out
    }

    /// Insert ▸ Cross-reference… ▸ Insert: a `REF`, `PAGEREF` or `NOTEREF`
    /// field to `target` (bookmarking it with a hidden `_Ref` bookmark
    /// first when it has none, as Word does), updated: one undo step.
    #[allow(clippy::too_many_arguments)]
    pub fn insert_cross_reference(
        &mut self,
        sel: Selection,
        kind: &XrefKind,
        target: &XrefTarget,
        what: XrefWhat,
        link: bool,
        cx: &FieldContext,
        lay_out: &mut dyn FnMut(&Document) -> Box<dyn PageLookup>,
    ) -> Selection {
        let sel = self.clamp_selection(sel);
        let mut paras = self.paragraphs.clone();
        let mut caret = sel.ordered().0;
        let name = match &target.bookmark {
            Some(n) => n.clone(),
            None => {
                let taken: HashSet<String> = self.bookmark_names(true).into_iter().collect();
                let mut n = 100_000_000u64 + Self::next_bookmark_id(&paras);
                let name = loop {
                    let name = format!("_Ref{n}");
                    if !taken.contains(&name) {
                        break name;
                    }
                    n += 1;
                };
                let (a, b) = target.range;
                // Headings and captions: the text, not the paragraph mark.
                Self::with_bookmark(&mut paras, &name, a, b);
                // Markers inserted before the caret in its paragraph move it.
                if caret.para == b.para && caret.offset >= b.offset {
                    caret.offset += OBJ;
                }
                if caret.para == a.para && caret.offset >= a.offset {
                    caret.offset += OBJ;
                }
                name
            }
        };
        let h = if link { " \\h" } else { "" };
        let instr = match (kind, what) {
            (XrefKind::Footnote | XrefKind::Endnote, XrefWhat::Number | XrefWhat::Text) => {
                format!("NOTEREF {name}{h}")
            }
            (_, XrefWhat::PageNumber) => format!("PAGEREF {name}{h}"),
            (_, XrefWhat::AboveBelow) => format!("PAGEREF {name} \\p{h}"),
            (_, XrefWhat::Number) => format!("REF {name} \\r{h}"),
            (_, XrefWhat::Text) => format!("REF {name}{h}"),
        };
        // The field goes in on top of the bookmarked paragraphs.
        let mut snap = self.snapshot(paras);
        let after = snap.insert_field(Selection::caret(caret), &instr, cx, lay_out);
        let paras = std::mem::take(&mut snap.paragraphs);
        self.commit_paras(paras, sel, after);
        self.clamp_selection(after)
    }

    // ── fields in headers, footers and notes ───────────────────────────────

    /// Update the fields in every header, footer and note (F9 over the
    /// whole document): one undo step with the body's update before it
    /// left to the caller. Returns whether anything changed.
    pub fn update_part_fields(&mut self, sel: Selection, cx: &FieldContext) -> bool {
        let before: HfState = self.side_state();
        let props = Arc::new(fields::Props::of(self));
        let mut changed = false;
        let mut none = |_: &Document| -> Box<dyn PageLookup> { Box::new(NoPages) };
        for i in 0..self.stories.len() {
            if self.stories[i].locked.is_some() {
                continue;
            }
            let mut d = std::mem::take(&mut self.stories[i].doc);
            d.field_props = Some(props.clone());
            d.track = self.track;
            d.author = self.author.clone();
            let all = Selection {
                anchor: Position::default(),
                head: d.end(),
            };
            let rev = d.revision();
            d.update_fields(all, cx, &mut none);
            d.field_props = None;
            d.track = false;
            if d.revision() != rev {
                d.clear_history();
                self.stories[i].modified = true;
                changed = true;
            }
            self.stories[i].doc = d;
        }
        for i in 0..self.notes.len() {
            if self.notes[i].separator {
                continue;
            }
            let mut d = std::mem::take(&mut self.notes[i].doc);
            d.field_props = Some(props.clone());
            d.track = self.track;
            d.author = self.author.clone();
            let all = Selection {
                anchor: Position::default(),
                head: d.end(),
            };
            let rev = d.revision();
            d.update_fields(all, cx, &mut none);
            d.field_props = None;
            d.track = false;
            if d.revision() != rev {
                d.clear_history();
                self.notes[i].xml = None;
                changed = true;
            }
            self.notes[i].doc = d;
        }
        if changed {
            self.push_hf(before, None, sel);
        }
        changed
    }
}

/// No layout: page references keep their results.
struct NoPages;

impl PageLookup for NoPages {
    fn page_of(&self, _: Position) -> Option<usize> {
        None
    }
    fn page_number(&self, _: usize) -> u32 {
        1
    }
    fn page_label(&self, _: usize) -> String {
        "1".into()
    }
}

/// Swap the object at `pos` (same run formatting).
impl Document {
    /// The tracked code edit of the field starting at `pos`: its old and
    /// new code (as the codes view shows them).
    pub fn tracked_field_code(&self, pos: Position) -> Option<(String, String)> {
        let (mark, rev) = self.begin_run(pos)?;
        if rev.is_some() {
            return None;
        }
        crate::code_track::change(&mark).map(|(old, new, _, _)| (old, new))
    }

    /// The field start at `pos` and its run's tracked change.
    fn begin_run(&self, pos: Position) -> Option<(FieldMark, Option<crate::review::RevKind>)> {
        let p = self.paragraphs.get(pos.para)?;
        p.run_ranges().find_map(|(r, run)| {
            if r.start != pos.offset {
                return None;
            }
            match run.object.as_ref()? {
                Object::Field(f) if f.is_begin() => {
                    Some(((**f).clone(), run.rev.as_ref().map(|v| v.kind)))
                }
                _ => None,
            }
        })
    }
}

fn set_mark(paras: &mut [Paragraph], pos: Position, o: Object) {
    let p = &mut paras[pos.para];
    let i = p.split_run_at(pos.offset);
    if let Some(run) = p.runs.get_mut(i) {
        run.object = Some(o);
    }
}
