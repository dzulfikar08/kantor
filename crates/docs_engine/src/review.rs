//! Review: tracked changes and comments.
//!
//! **Tracked changes.** Runs inside `w:ins` / `w:del` (and `w:moveTo` /
//! `w:moveFrom`) carry the [`Revision`] they belong to: the original start
//! tag (written back verbatim around the same runs), its kind, author and
//! date. Deleted text is `w:delText` in the file and ordinary text in the
//! model, so it stays visible (struck through) until the change is
//! accepted. Paragraph-mark changes (`w:ins`/`w:del` in the mark's
//! `w:rPr`) and formatting changes (`w:rPrChange`, `w:pPrChange`) ride in
//! the property extras; Accept All and Reject All resolve them too. Text
//! typed in Kantor isn't tracked.
//!
//! **Comments.** `w:commentRangeStart` / `w:commentRangeEnd` are position
//! markers and `w:commentReference` a run object (all opaque objects); the
//! comments themselves are read from `comments.xml` into [`Comment`]s.
//! Comments not touched in Kantor are written back byte for byte; new and
//! edited ones are generated; the part (with its relationship and content
//! type) is created when a document gets its first comment.

use std::sync::Arc;

use crate::document::{Document, Kind};
use crate::model::{
    OBJECT, Object, Opaque, Paragraph, Position, RunProps, Selection, element_name,
};
use crate::object::Level;
use crate::xml;

/// What a tracked change does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RevKind {
    Insert,
    Delete,
}

/// A tracked insertion or deletion: its start tag and who made it when.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Revision {
    pub kind: RevKind,
    /// The original start tag (`<w:ins w:id="3" w:author="…" w:date="…">`).
    pub tag: String,
    pub author: String,
    pub date: String,
}

impl Revision {
    /// From an `ins`/`del`/`moveTo`/`moveFrom` start tag.
    pub fn parse(tag: &str) -> Option<Self> {
        let el = xml::parse(&format!(
            "{}/>",
            tag.trim_end().trim_end_matches("/>").trim_end_matches('>')
        ))?;
        let kind = match el.name.as_str() {
            "ins" | "moveTo" => RevKind::Insert,
            "del" | "moveFrom" => RevKind::Delete,
            _ => return None,
        };
        Some(Self {
            kind,
            tag: tag.to_owned(),
            author: el.attr("author").unwrap_or_default().to_owned(),
            date: el.attr("date").unwrap_or_default().to_owned(),
        })
    }

    /// The closing tag matching [`Self::tag`].
    pub fn close_tag(&self) -> String {
        let name: String = self.tag[1..]
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '>' && *c != '/')
            .collect();
        format!("</{name}>")
    }
}

/// One tracked change in the text, for the review list.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub start: Position,
    pub end: Position,
    pub kind: RevKind,
    pub author: String,
    pub date: String,
    pub text: String,
}

/// A comment from `comments.xml` (or added in Kantor).
#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub initials: String,
    pub date: String,
    /// Plain text, paragraphs joined with `\n`.
    pub text: String,
    /// The original `<w:comment>` element, written back as is unless the
    /// comment was edited (then `None`).
    pub(crate) xml: Option<String>,
}

/// Where a comment points in the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub start: Position,
    pub end: Position,
}

/// The comments part as read.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CommentsPart {
    pub name: String,
    pub root_tag: String,
}

/// Comments read from a `comments.xml`.
pub(crate) fn parse_comments(src: &str) -> (Vec<Comment>, String) {
    let mut out = Vec::new();
    let root_tag = src
        .find("<w:comments")
        .and_then(|i| src[i..].find('>').map(|e| src[i..=i + e].to_owned()))
        .unwrap_or_else(|| format!("<w:comments {W_DECL} {R_DECL}>"));
    let mut at = 0;
    while let Some(i) = src[at..].find("<w:comment ") {
        let start = at + i;
        let Some(end) = src[start..]
            .find("</w:comment>")
            .map(|e| start + e + "</w:comment>".len())
        else {
            break;
        };
        let el_xml = &src[start..end];
        if let Some(el) = xml::parse(el_xml) {
            let paras: Vec<String> = el_xml
                .split("</w:p>")
                .filter(|chunk| chunk.contains("<w:p>") || chunk.contains("<w:p "))
                .map(|chunk| {
                    let at = chunk
                        .find("<w:p>")
                        .or_else(|| chunk.find("<w:p "))
                        .unwrap_or(0);
                    xml::inner_text(&format!("{}</w:p>", &chunk[at..]), &["t"])
                })
                .collect();
            out.push(Comment {
                id: el.attr("id").unwrap_or_default().to_owned(),
                author: el.attr("author").unwrap_or_default().to_owned(),
                initials: el.attr("initials").unwrap_or_default().to_owned(),
                date: el.attr("date").unwrap_or_default().to_owned(),
                text: paras.join("\n"),
                xml: Some(el_xml.to_owned()),
            });
        }
        at = end;
    }
    (out, root_tag)
}

const W_DECL: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"";
const R_DECL: &str =
    "xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\"";

/// A new comments part's XML from `comments`.
pub(crate) fn write_comments(root_tag: &str, comments: &[Comment], with_styles: bool) -> String {
    let mut out = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n");
    out.push_str(root_tag);
    for c in comments {
        match &c.xml {
            Some(x) => out.push_str(x),
            None => {
                out.push_str(&format!(
                    "<w:comment w:id=\"{}\" w:author=\"{}\" w:date=\"{}\" w:initials=\"{}\">",
                    xml::escape(&c.id),
                    xml::escape(&c.author),
                    xml::escape(&c.date),
                    xml::escape(&c.initials)
                ));
                for (k, line) in c.text.split('\n').enumerate() {
                    out.push_str("<w:p>");
                    if with_styles {
                        out.push_str("<w:pPr><w:pStyle w:val=\"CommentText\"/></w:pPr>");
                    }
                    if k == 0 {
                        out.push_str("<w:r>");
                        if with_styles {
                            out.push_str("<w:rPr><w:rStyle w:val=\"CommentReference\"/></w:rPr>");
                        }
                        out.push_str("<w:annotationRef/></w:r>");
                    }
                    if !line.is_empty() {
                        out.push_str("<w:r><w:t xml:space=\"preserve\">");
                        out.push_str(&xml::escape(line));
                        out.push_str("</w:t></w:r>");
                    }
                    out.push_str("</w:p>");
                }
                out.push_str("</w:comment>");
            }
        }
    }
    out.push_str("</w:comments>");
    out
}

/// The `w:id` of a comment marker or reference object.
fn comment_id(o: &Object) -> Option<(&'static str, String)> {
    let Object::Opaque(o) = o else { return None };
    let name = element_name(&o.xml);
    let kind = match name {
        "commentRangeStart" => "start",
        "commentRangeEnd" => "end",
        "commentReference" => "ref",
        _ => return None,
    };
    let el = xml::parse(&o.xml)?;
    Some((kind, el.attr("id")?.to_owned()))
}

/// Today's date and time in UTC as `YYYY-MM-DDTHH:MM:SSZ` (Word's format).
pub fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

/// Paragraph-mark revision elements kept in a paragraph's extras.
fn mark_revs(p: &Paragraph) -> (bool, bool) {
    let has = |name: &str| {
        let in_raw = p
            .props
            .extras
            .iter()
            .any(|x| element_name(x) == "rPr" && x.contains(&format!(":{name} ")));
        let in_empty = p.text.is_empty()
            && p.runs[0]
                .props
                .extras
                .iter()
                .any(|x| element_name(x) == name);
        in_raw || in_empty
    };
    (has("ins") || has("moveTo"), has("del") || has("moveFrom"))
}

/// Remove the mark's `w:ins`/`w:del` elements.
fn clear_mark_revs(p: &mut Paragraph) {
    for name in ["ins", "del", "moveTo", "moveFrom"] {
        p.runs[0].props.extras.remove(name);
    }
    let raws: Vec<String> = p
        .props
        .extras
        .iter()
        .filter(|x| element_name(x) == "rPr")
        .map(str::to_owned)
        .collect();
    if raws.is_empty() {
        return;
    }
    p.props.extras.remove("rPr");
    for raw in raws {
        let mut s = raw;
        for name in ["ins", "del", "moveTo", "moveFrom"] {
            while let Some(i) = s.find(&format!(":{name} ")) {
                let start = s[..i].rfind('<').unwrap_or(i);
                let end = s[i..].find("/>").map_or(s.len(), |e| i + e + 2);
                s.replace_range(start..end, "");
            }
        }
        let empty = s.trim_start().starts_with("<w:rPr>")
            && s.trim_end().ends_with("<w:rPr></w:rPr>")
            || s.replace(char::is_whitespace, "") == "<w:rPr></w:rPr>";
        if !empty {
            p.props.extras.push(&s);
        }
    }
}

/// The old properties a `w:rPrChange` / `w:pPrChange` holds, as the inner
/// property element.
fn change_inner(change: &str, inner: &str) -> Option<String> {
    let open = change.find(&format!("<w:{inner}"))?;
    let close = format!("</w:{inner}>");
    match change[open..].find(&close) {
        Some(e) => Some(change[open..open + e + close.len()].to_owned()),
        None => {
            // `<w:rPr/>`: no old properties.
            let end = change[open..].find("/>")? + open + 2;
            Some(change[open..end].to_owned())
        }
    }
}

impl Document {
    /// Tracked insertions and deletions in document order (consecutive runs
    /// of one change merged).
    pub fn changes(&self) -> Vec<Change> {
        let mut out: Vec<Change> = Vec::new();
        for (pi, p) in self.paragraphs.iter().enumerate() {
            for (range, run) in p.run_ranges() {
                let Some(rev) = &run.rev else { continue };
                if let Some(last) = out.last_mut()
                    && last.end == Position::new(pi, range.start)
                    && last.kind == rev.kind
                    && last.author == rev.author
                    && last.date == rev.date
                {
                    last.end = Position::new(pi, range.end);
                    last.text.push_str(&p.text[range]);
                    continue;
                }
                out.push(Change {
                    start: Position::new(pi, range.start),
                    end: Position::new(pi, range.end),
                    kind: rev.kind,
                    author: rev.author.clone(),
                    date: rev.date.clone(),
                    text: p.text[range].to_owned(),
                });
            }
        }
        out
    }

    /// Paragraph-mark and formatting changes (resolved only by Accept All /
    /// Reject All).
    pub fn other_changes(&self) -> usize {
        self.paragraphs
            .iter()
            .map(|p| {
                let (i, d) = mark_revs(p);
                let fmt = p
                    .props
                    .extras
                    .iter()
                    .filter(|x| element_name(x) == "pPrChange")
                    .count()
                    + p.runs
                        .iter()
                        .filter(|r| {
                            r.props
                                .extras
                                .iter()
                                .any(|x| element_name(x) == "rPrChange")
                        })
                        .count();
                i as usize + d as usize + fmt
            })
            .sum()
    }

    /// Whether the document has tracked changes of any kind.
    pub fn has_changes(&self) -> bool {
        !self.changes().is_empty() || self.other_changes() > 0
    }

    /// The change at (or touching) the caret / overlapping the selection.
    pub fn change_at(&self, sel: Selection) -> Option<Change> {
        let (a, b) = self.clamp_selection(sel).ordered();
        self.changes()
            .into_iter()
            .find(|c| c.start <= b && c.end >= a && (a != b || (c.start <= a && a <= c.end)))
    }

    /// Accept or reject the tracked insertions/deletions in `start..end`
    /// (runs only): one undo step. Returns where the caret goes.
    fn resolve_runs(
        &mut self,
        start: Position,
        end: Position,
        accept: bool,
        sel: Selection,
    ) -> Selection {
        let first = start.para;
        let last = end.para.min(self.paragraphs.len() - 1);
        let mut new: Vec<Paragraph> = self.paragraphs[first..=last].to_vec();
        for (k, p) in new.iter_mut().enumerate() {
            let pi = first + k;
            let lo = if pi == start.para { start.offset } else { 0 };
            let hi = if pi == end.para {
                end.offset
            } else {
                p.text.len()
            };
            // Ranges to drop (back to front) and runs to keep as plain text.
            let mut drop: Vec<std::ops::Range<usize>> = Vec::new();
            let mut at = 0;
            for run in &mut p.runs {
                let r = at..at + run.len;
                at = r.end;
                let Some(rev) = run.rev.clone() else { continue };
                if r.end <= lo || r.start >= hi {
                    continue;
                }
                let remove = matches!(
                    (rev.kind, accept),
                    (crate::review::RevKind::Delete, true)
                        | (crate::review::RevKind::Insert, false)
                );
                if remove {
                    drop.push(r);
                } else {
                    run.rev = None;
                }
            }
            for r in drop.into_iter().rev() {
                // Markers (bookmarks, comment ranges) survive the removal.
                p.delete(r);
            }
            p.normalize();
        }
        let count = last - first + 1;
        let after = Selection::caret(self.clamp_pos_in(&new, first, start));
        self.replace(first, count, new, (sel, after), Kind::Other, true);
        after
    }

    fn clamp_pos_in(&self, paras: &[Paragraph], first: usize, pos: Position) -> Position {
        let p = &paras[pos.para - first];
        Position::new(pos.para, pos.offset.min(p.text.len()))
    }

    /// Accept the change at the caret (or every change in the selection).
    pub fn accept_change(&mut self, sel: Selection) -> Option<Selection> {
        self.resolve_at(sel, true)
    }

    /// Reject the change at the caret (or every change in the selection).
    pub fn reject_change(&mut self, sel: Selection) -> Option<Selection> {
        self.resolve_at(sel, false)
    }

    fn resolve_at(&mut self, sel: Selection, accept: bool) -> Option<Selection> {
        let (a, b) = self.clamp_selection(sel).ordered();
        let (start, end) = if a == b {
            let c = self.change_at(sel)?;
            (c.start, c.end)
        } else {
            let cs: Vec<Change> = self
                .changes()
                .into_iter()
                .filter(|c| c.start < b && c.end > a)
                .collect();
            (cs.first()?.start.min(a), cs.last()?.end.max(b))
        };
        Some(self.resolve_runs(start, end, accept, sel))
    }

    /// Accept (or reject) every tracked change: insertions, deletions,
    /// paragraph marks and formatting. One undo step.
    pub fn resolve_all(&mut self, accept: bool, sel: Selection) -> Selection {
        let old = self.paragraphs.clone();
        let mut out: Vec<Paragraph> = Vec::with_capacity(old.len());
        // Paragraphs whose mark is removed join the next one.
        let mut join_next = false;
        for p in &old {
            let mut p = p.clone();
            // Runs.
            let mut drop = Vec::new();
            let mut at = 0;
            for run in &mut p.runs {
                let r = at..at + run.len;
                at = r.end;
                if let Some(rev) = run.rev.take() {
                    let remove = (rev.kind == RevKind::Delete) == accept;
                    if remove {
                        drop.push(r);
                    }
                }
                // Formatting changes: accept drops the record, reject puts
                // the old properties back.
                let change = run
                    .props
                    .extras
                    .iter()
                    .find(|x| element_name(x) == "rPrChange")
                    .map(str::to_owned);
                if let Some(change) = change {
                    if accept {
                        run.props.extras.remove("rPrChange");
                    } else {
                        let mut props = RunProps::default();
                        if let Some(inner) = change_inner(&change, "rPr") {
                            crate::docx::parse_rpr_into(&inner, &mut props);
                        }
                        run.props = props;
                    }
                }
            }
            for r in drop.into_iter().rev() {
                p.delete(r);
            }
            let pchange = p
                .props
                .extras
                .iter()
                .find(|x| element_name(x) == "pPrChange")
                .map(str::to_owned);
            if let Some(change) = pchange {
                p.props.extras.remove("pPrChange");
                if !accept && let Some(inner) = change_inner(&change, "pPr") {
                    let keep: Vec<String> = p
                        .props
                        .extras
                        .iter()
                        .filter(|x| matches!(element_name(x), "rPr" | "sectPr"))
                        .map(str::to_owned)
                        .collect();
                    let mut props = crate::model::ParaProps::default();
                    crate::docx::parse_ppr_into(&inner, &mut props);
                    for k in keep {
                        props.extras.push(&k);
                    }
                    p.props = props;
                }
            }
            let (ins, del) = mark_revs(&p);
            let joins = (del && accept) || (ins && !accept);
            clear_mark_revs(&mut p);
            p.normalize();
            if join_next && let Some(prev) = out.last_mut() {
                if prev.cell == p.cell {
                    prev.append(p);
                } else {
                    out.push(p);
                }
            } else {
                out.push(p);
            }
            join_next = joins;
        }
        if out.is_empty() {
            out.push(Paragraph::default());
        }
        if out == old {
            return sel;
        }
        let n = old.len();
        let after = self.clamp_selection_in(&out, sel);
        self.replace(0, n, out, (sel, after), Kind::Other, true);
        after
    }

    fn clamp_selection_in(&self, paras: &[Paragraph], sel: Selection) -> Selection {
        let clamp = |p: Position| {
            let para = p.para.min(paras.len() - 1);
            Position::new(para, p.offset.min(paras[para].text.len()))
        };
        Selection {
            anchor: clamp(sel.anchor),
            head: clamp(sel.head),
        }
    }

    // ── comments ───────────────────────────────────────────────────────────

    pub fn comments(&self) -> &[Comment] {
        &self.comments
    }

    /// Where each comment points (by comment id), from its range markers or
    /// its reference.
    pub fn comment_anchors(&self) -> Vec<(String, Anchor)> {
        let mut starts: Vec<(String, Position)> = Vec::new();
        let mut out: Vec<(String, Anchor)> = Vec::new();
        for (pi, p) in self.paragraphs.iter().enumerate() {
            for (off, o) in p.objects() {
                let Some((kind, id)) = comment_id(o) else {
                    continue;
                };
                let pos = Position::new(pi, off);
                match kind {
                    "start" => starts.push((id, pos)),
                    "end" | "ref" => {
                        if out.iter().any(|(i, _)| *i == id) {
                            continue;
                        }
                        let start = starts
                            .iter()
                            .find(|(i, _)| *i == id)
                            .map_or(pos, |(_, s)| *s);
                        out.push((id, Anchor { start, end: pos }));
                    }
                    _ => {}
                }
            }
        }
        out
    }

    /// Add a comment on the selection (or the word at the caret): one undo
    /// step. Returns its id.
    pub fn add_comment(&mut self, sel: Selection, text: &str, author: &str) -> Option<String> {
        let sel = self.clamp_selection(sel);
        let (mut a, mut b) = sel.ordered();
        if a == b {
            let w = crate::model::word_at(&self.paragraphs[a.para].text, a.offset);
            a.offset = w.start;
            b.offset = w.end;
        }
        let before_comments = self.review_state();
        let id = (0..)
            .map(|n: u32| n.to_string())
            .find(|id| !self.comments.iter().any(|c| c.id == *id) && !self.has_comment_marker(id))
            .expect("infinite ids");
        let initials: String = author
            .split_whitespace()
            .filter_map(|w| w.chars().next())
            .flat_map(char::to_uppercase)
            .take(3)
            .collect();
        self.comments.push(Comment {
            id: id.clone(),
            author: author.to_owned(),
            initials,
            date: now_utc(),
            text: text.to_owned(),
            xml: None,
        });
        self.comments_modified = true;
        let marker = |name: &str| {
            Object::Opaque(Arc::new(Opaque {
                level: Level::Marker,
                xml: format!("<w:{name} w:id=\"{id}\"/>"),
                label: String::new(),
            }))
        };
        let reference = Object::Opaque(Arc::new(Opaque {
            level: Level::Run,
            xml: format!("<w:commentReference w:id=\"{id}\"/>"),
            label: String::new(),
        }));
        let ref_props = RunProps {
            style: self
                .styles
                .get("CommentReference")
                .map(|_| "CommentReference".to_owned()),
            ..RunProps::default()
        };
        let first = a.para;
        let last = b.para;
        let mut new: Vec<Paragraph> = self.paragraphs[first..=last].to_vec();
        let n = new.len();
        // End first so the start offset stays valid.
        {
            let p = &mut new[n - 1];
            p.insert_object(b.offset, reference, ref_props);
            p.insert_object(b.offset, marker("commentRangeEnd"), RunProps::default());
        }
        new[0].insert_object(a.offset, marker("commentRangeStart"), RunProps::default());
        let after = sel;
        self.replace(first, n, new, (sel, after), Kind::Other, true);
        // Fold the comment list change into the same undo step.
        let after_comments = self.review_state();
        self.attach_side_state(before_comments, after_comments);
        Some(id)
    }

    fn has_comment_marker(&self, id: &str) -> bool {
        self.paragraphs.iter().any(|p| {
            p.objects()
                .any(|(_, o)| comment_id(o).is_some_and(|(_, i)| i == id))
        })
    }

    /// Change a comment's text: one undo step.
    pub fn set_comment_text(&mut self, id: &str, text: &str, sel: Selection) -> bool {
        let Some(i) = self.comments.iter().position(|c| c.id == id) else {
            return false;
        };
        if self.comments[i].text == text {
            return true;
        }
        let before = self.review_state();
        let c = &mut self.comments[i];
        c.text = text.to_owned();
        c.xml = None;
        self.comments_modified = true;
        let after = self.review_state();
        self.push_side(before, after, sel);
        true
    }

    pub(crate) fn review_state(&self) -> crate::section::HfState {
        self.side_state()
    }

    /// Put `before`/`after` side state on the step just recorded.
    fn attach_side_state(
        &mut self,
        before: crate::section::HfState,
        after: crate::section::HfState,
    ) {
        self.set_last_side(before, after);
    }

    fn push_side(
        &mut self,
        before: crate::section::HfState,
        after: crate::section::HfState,
        sel: Selection,
    ) {
        if before != after {
            self.push_command(
                0,
                Vec::new(),
                Vec::new(),
                sel,
                Some(Box::new((before, after))),
            );
        }
    }

    /// Delete a comment and its markers: one undo step.
    pub fn delete_comment(&mut self, id: &str, sel: Selection) -> bool {
        let Some(i) = self.comments.iter().position(|c| c.id == id) else {
            return false;
        };
        let before = self.review_state();
        self.comments.remove(i);
        self.comments_modified = true;
        let touched: Vec<usize> = (0..self.paragraphs.len())
            .filter(|&pi| {
                self.paragraphs[pi]
                    .objects()
                    .any(|(_, o)| comment_id(o).is_some_and(|(_, x)| x == id))
            })
            .collect();
        if let (Some(&first), Some(&last)) = (touched.first(), touched.last()) {
            let mut new: Vec<Paragraph> = self.paragraphs[first..=last].to_vec();
            for p in &mut new {
                let offs: Vec<usize> = p
                    .objects()
                    .filter(|(_, o)| comment_id(o).is_some_and(|(_, x)| x == id))
                    .map(|(off, _)| off)
                    .collect();
                for off in offs.into_iter().rev() {
                    // Remove the object run itself (markers survive plain
                    // deletion, so drop the run directly).
                    p.remove_object(off);
                }
            }
            let n = new.len();
            let after = self.clamp_selection_in(
                &{
                    let mut all = self.paragraphs.clone();
                    all.splice(first..=last, new.iter().cloned());
                    all
                },
                sel,
            );
            self.replace(first, n, new, (sel, after), Kind::Other, true);
            let after_state = self.review_state();
            self.attach_side_state(before, after_state);
        } else {
            let after_state = self.review_state();
            self.push_side(before, after_state, sel);
        }
        true
    }
}

impl Paragraph {
    /// Remove the object character at `offset` (markers included).
    pub fn remove_object(&mut self, offset: usize) {
        if self.object_at(offset).is_none() {
            return;
        }
        let mut at = 0;
        let mut idx = None;
        for (i, r) in self.runs.iter().enumerate() {
            if at == offset && r.object.is_some() {
                idx = Some(i);
                break;
            }
            at += r.len;
        }
        if let Some(i) = idx {
            self.runs.remove(i);
            self.text
                .replace_range(offset..offset + OBJECT.len_utf8(), "");
            if self.text.is_empty() {
                let props = self
                    .runs
                    .first()
                    .map(|r| r.props.clone())
                    .unwrap_or_default();
                self.runs = vec![crate::model::Run::text(0, props)];
            }
            self.normalize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn revision_tags() {
        let r = Revision::parse(r#"<w:del w:id="2" w:author="Ani" w:date="2026-01-02T03:04:05Z">"#)
            .unwrap();
        assert_eq!(r.kind, RevKind::Delete);
        assert_eq!(r.author, "Ani");
        assert_eq!(r.close_tag(), "</w:del>");
        assert_eq!(
            Revision::parse(r#"<w:moveTo w:id="1">"#).unwrap().kind,
            RevKind::Insert
        );
    }

    #[test]
    fn comments_round_trip_their_xml() {
        let src = r#"<?xml version="1.0"?><w:comments xmlns:w="w"><w:comment w:id="0" w:author="Ani" w:date="2026-11-10T00:00:00Z" w:initials="A"><w:p><w:r><w:annotationRef/></w:r><w:r><w:t>Check the price</w:t></w:r></w:p><w:p><w:r><w:t>clause.</w:t></w:r></w:p></w:comment></w:comments>"#;
        let (cs, root) = parse_comments(src);
        assert_eq!(root, r#"<w:comments xmlns:w="w">"#);
        assert_eq!(cs.len(), 1);
        assert_eq!(cs[0].text, "Check the price\nclause.");
        let out = write_comments(&root, &cs, false);
        assert!(out.contains(cs[0].xml.as_deref().unwrap()));
    }

    #[test]
    fn now_is_iso() {
        let n = now_utc();
        assert_eq!(n.len(), 20);
        assert!(n.ends_with('Z') && n.as_bytes()[10] == b'T');
    }
}
