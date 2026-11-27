//! Review: tracked changes and comments.
//!
//! **Tracked changes.** Runs inside `w:ins` / `w:del` (and `w:moveTo` /
//! `w:moveFrom`) carry the [`Revision`] they belong to: the original start
//! tag (written back verbatim around the same runs), its kind, author and
//! date. Deleted text is `w:delText` in the file and ordinary text in the
//! model, so it stays visible (struck through) until the change is
//! accepted. Paragraph-mark changes (`w:ins`/`w:del` in the mark's
//! `w:rPr`) and formatting changes (`w:rPrChange`, `w:pPrChange`) ride in
//! the property extras; Accept All and Reject All resolve them too.
//! Paragraph-mark changes that can join two paragraphs are also part of
//! [`Document::changes`], so they can be accepted or rejected one by one.
//! Edits made in Kantor are tracked when Track Changes is on (see
//! `track.rs`).
//!
//! **Comments.** `w:commentRangeStart` / `w:commentRangeEnd` are position
//! markers and `w:commentReference` a run object (all opaque objects); the
//! comments themselves are read from `comments.xml` into [`Comment`]s.
//! Comments not touched in Kantor are written back byte for byte; new and
//! edited ones are generated; the part (with its relationship and content
//! type) is created when a document gets its first comment.

use crate::document::{Document, Kind};
use crate::fmt_track;
use crate::model::{OBJECT, Object, Paragraph, Position, Selection, element_name};
use crate::track_table;
use crate::xml;

/// What a tracked change does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RevKind {
    Insert,
    Delete,
    /// A formatting change (`w:rPrChange`, `w:pPrChange`): only in
    /// [`Change`]s, never on a run's [`Revision`].
    Format,
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
    /// What a formatting change did (empty for insertions and deletions).
    pub format: Vec<crate::fmt_track::FormatItem>,
    /// A paragraph's formatting change (`w:pPrChange`) rather than its
    /// text's.
    pub para: bool,
    /// An inserted or deleted table row (`w:trPr/w:ins|w:del`), from its
    /// first cell to its last.
    pub row: bool,
    /// A change to a table's structure or properties rather than its text:
    /// a row or cell inserted or deleted (the row's or cell's paragraphs),
    /// or the table's properties (the whole table; [`RevKind::Format`]).
    pub table: Option<track_table::TableChange>,
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
    /// The comment this one replies to (Word 2013+ threads).
    pub parent: Option<String>,
    /// Resolved (`w15:done`); a thread's state is its first comment's.
    pub done: bool,
    /// `w14:paraId` of the last paragraph (keys `commentsExtended.xml`).
    pub(crate) para_id: Option<String>,
    /// `w16cid:durableId` (keys `commentsExtensible.xml`).
    pub(crate) durable_id: Option<String>,
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
                parent: None,
                done: false,
                para_id: None,
                durable_id: None,
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
                let n = c.text.split('\n').count();
                for (k, line) in c.text.split('\n').enumerate() {
                    match c.para_id.as_deref().filter(|_| k + 1 == n) {
                        Some(p) => out.push_str(&format!("<w:p w14:paraId=\"{p}\">")),
                        None => out.push_str("<w:p>"),
                    }
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
pub(crate) fn comment_id(o: &Object) -> Option<(&'static str, String)> {
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
/// Resolve the tracked code edit of a field start (crate::code_track).
fn resolve_code(run: &mut crate::model::Run, accept: bool) {
    if let Some(crate::model::Object::Field(f)) = &run.object
        && let Some(m) = crate::code_track::resolve(f, accept)
    {
        run.object = Some(crate::model::Object::Field(std::sync::Arc::new(m)));
    }
}

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

/// The kind, author and date of a paragraph mark's revision.
fn mark_rev(p: &Paragraph) -> Option<(RevKind, String, String)> {
    crate::track::mark_elements(p).iter().find_map(|x| {
        let el = xml::parse(x)?;
        let kind = match el.name.as_str() {
            "ins" | "moveTo" => RevKind::Insert,
            "del" | "moveFrom" => RevKind::Delete,
            _ => return None,
        };
        Some((
            kind,
            el.attr("author").unwrap_or_default().to_owned(),
            el.attr("date").unwrap_or_default().to_owned(),
        ))
    })
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

/// Join `right` onto `left` as a removed paragraph mark does: an empty
/// left paragraph takes the right one's formatting (the mark that's left
/// is the right one's, as in Word).
fn join(left: &mut Paragraph, right: Paragraph) {
    if left.text.is_empty() {
        let props = right.props.clone();
        left.append(right);
        left.props = props;
    } else {
        left.append(right);
    }
}

/// Remove a resolved change's text: a floating object marked deleted
/// goes too (deleting text keeps them otherwise).
fn drop_change(p: &mut Paragraph, r: std::ops::Range<usize>) {
    if r.len() == OBJECT.len_utf8() && matches!(p.object_at(r.start), Some(Object::Float(_))) {
        p.remove_object(r.start);
    } else {
        p.delete(r);
    }
}

impl Document {
    /// Tracked changes in document order: insertions and deletions
    /// (consecutive runs of one change merged), paragraph marks that can
    /// join two paragraphs, and formatting changes (a paragraph's first,
    /// then its text's).
    pub fn changes(&self) -> Vec<Change> {
        let mut out: Vec<Change> = Vec::new();
        let mut add = |c: Change| {
            if let Some(last) = out.last_mut()
                && last.end == c.start
                && last.kind == c.kind
                && last.author == c.author
                && last.date == c.date
                && last.format == c.format
                && !last.para
                && !c.para
                && !last.row
                && !c.row
                && last.table.is_none()
                && c.table.is_none()
            {
                last.end = c.end;
                last.text.push_str(&c.text);
                return;
            }
            out.push(c);
        };
        let n = self.paragraphs.len();
        for (pi, p) in self.paragraphs.iter().enumerate() {
            // A table's property changes, at its first paragraph.
            if let Some(c) = p.cell
                && (pi == 0
                    || self.paragraphs[pi - 1]
                        .cell
                        .is_none_or(|q| q.table != c.table))
                && let Some(t) = self.tables.get(&c.table)
                && let Some((author, date)) = track_table::props_changed(t)
            {
                let last = self.table_range(c.table).end - 1;
                add(Change {
                    start: Position::new(pi, 0),
                    end: Position::new(last, self.paragraphs[last].text.len()),
                    kind: RevKind::Format,
                    author,
                    date,
                    text: String::new(),
                    format: track_table::props_items(t),
                    para: false,
                    row: false,
                    table: Some(track_table::TableChange::Props),
                });
            }
            if let Some(c) = p.cell
                && (pi == 0
                    || self.paragraphs[pi - 1]
                        .cell
                        .is_none_or(|q| q.table != c.table || q.row != c.row))
                && let Some(row) = self
                    .tables
                    .get(&c.table)
                    .and_then(|t| t.rows.get(c.row as usize))
                && let Some((kind, author, date)) = track_table::row_rev(row)
                && let Some((_, b)) = Self::row_paras(&self.paragraphs[pi..], c.table, c.row)
            {
                let b = pi + b;
                add(Change {
                    start: Position::new(pi, 0),
                    end: Position::new(b, self.paragraphs[b].text.len()),
                    kind,
                    author,
                    date,
                    text: String::new(),
                    format: Vec::new(),
                    para: false,
                    row: true,
                    table: Some(track_table::TableChange::Row),
                });
            }
            // An inserted or deleted cell, at its first paragraph.
            if let Some(c) = p.cell
                && (pi == 0 || self.paragraphs[pi - 1].cell != Some(c))
                && let Some(cell) = self
                    .tables
                    .get(&c.table)
                    .and_then(|t| t.rows.get(c.row as usize))
                    .and_then(|r| r.cells.get(c.cell as usize))
                && let Some((kind, author, date)) = track_table::cell_rev(cell)
                && let Some((_, b)) =
                    track_table::cell_paras(&self.paragraphs[pi..], c.table, c.row, c.cell)
            {
                let b = pi + b;
                add(Change {
                    start: Position::new(pi, 0),
                    end: Position::new(b, self.paragraphs[b].text.len()),
                    kind,
                    author,
                    date,
                    text: String::new(),
                    format: Vec::new(),
                    para: false,
                    row: false,
                    table: Some(track_table::TableChange::Cell),
                });
            }
            if let Some(ch) = fmt_track::para_change(&p.props) {
                let (author, date) = fmt_track::who_when(ch);
                add(Change {
                    start: Position::new(pi, 0),
                    end: Position::new(pi, p.text.len()),
                    kind: RevKind::Format,
                    author,
                    date,
                    text: p.text.clone(),
                    format: self.para_format_items(&p.props, ch),
                    para: true,
                    row: false,
                    table: None,
                });
            }
            for (range, run) in p.run_ranges() {
                // A field code edited with Track Changes on: the old code
                // deleted, the new one inserted.
                if run.rev.is_none()
                    && let Some(crate::model::Object::Field(f)) = &run.object
                    && let Some((old, new, author, date)) = crate::code_track::change(f)
                {
                    for (kind, text) in [(RevKind::Delete, old), (RevKind::Insert, new)] {
                        add(Change {
                            start: Position::new(pi, range.start),
                            end: Position::new(pi, range.end),
                            kind,
                            author: author.clone(),
                            date: date.clone(),
                            text,
                            format: Vec::new(),
                            para: false,
                            row: false,
                            table: None,
                        });
                    }
                }
                if let Some(rev) = &run.rev {
                    add(Change {
                        start: Position::new(pi, range.start),
                        end: Position::new(pi, range.end),
                        kind: rev.kind,
                        author: rev.author.clone(),
                        date: rev.date.clone(),
                        text: p.text[range.clone()].to_owned(),
                        format: Vec::new(),
                        para: false,
                        row: false,
                        table: None,
                    });
                }
                if !range.is_empty()
                    && let Some(ch) = fmt_track::run_change(&run.props)
                {
                    let (author, date) = fmt_track::who_when(ch);
                    add(Change {
                        start: Position::new(pi, range.start),
                        end: Position::new(pi, range.end),
                        kind: RevKind::Format,
                        author,
                        date,
                        text: p.text[range.clone()].to_owned(),
                        format: self.run_format_items(&run.props, ch),
                        para: false,
                        row: false,
                        table: None,
                    });
                }
            }
            // An inserted or deleted paragraph mark (not the last one,
            // which has nothing to join).
            if pi + 1 < n
                && self.can_merge(pi, pi + 1)
                && let Some((kind, author, date)) = mark_rev(p)
            {
                add(Change {
                    start: Position::new(pi, p.text.len()),
                    end: Position::new(pi + 1, 0),
                    kind,
                    author,
                    date,
                    text: "\n".into(),
                    format: Vec::new(),
                    para: false,
                    row: false,
                    table: None,
                });
            }
        }
        out
    }

    /// Paragraph-mark changes that can't join paragraphs (resolved only by
    /// Accept All / Reject All).
    pub fn other_changes(&self) -> usize {
        self.paragraphs
            .iter()
            .enumerate()
            .map(|(pi, p)| {
                // Marks that can join are in [`Self::changes`].
                let (i, d) = if pi + 1 < self.paragraphs.len() && self.can_merge(pi, pi + 1) {
                    (false, false)
                } else {
                    mark_revs(p)
                };
                i as usize + d as usize
            })
            .sum()
    }

    /// Whether the document has tracked changes of any kind.
    pub fn has_changes(&self) -> bool {
        !self.changes().is_empty() || self.other_changes() > 0 || self.box_changes() > 0
    }

    /// The change at (or touching) the caret / overlapping the selection.
    pub fn change_at(&self, sel: Selection) -> Option<Change> {
        let (a, b) = self.clamp_selection(sel).ordered();
        let all = self.changes();
        let hit =
            |c: &&Change| c.start <= b && c.end >= a && (a != b || (c.start <= a && a <= c.end));
        // The text's own change (or its row's or cell's, which come first)
        // before its paragraph's formatting, and that before its table's.
        let rank = |c: &Change| match (c.para, c.table) {
            (_, Some(track_table::TableChange::Props)) => 2,
            (true, _) => 1,
            _ => 0,
        };
        all.iter().filter(hit).min_by_key(|c| rank(c)).cloned()
    }

    /// Accept or reject the tracked changes in `start..end` (`only`: just
    /// that one kind; formatting changes of the paragraph or of its text):
    /// one undo step. Returns where the caret goes.
    fn resolve_runs(
        &mut self,
        start: Position,
        end: Position,
        accept: bool,
        sel: Selection,
        only: Option<(RevKind, bool)>,
    ) -> Selection {
        let wants = |kind: RevKind, para: bool| only.is_none_or(|o| o == (kind, para));
        // Whole tables, so their rows can be resolved too.
        let mut first = start.para;
        let mut last = end.para.min(self.paragraphs.len() - 1);
        if let Some(c) = self.paragraphs[first].cell {
            first = self.table_range(c.table).start;
        }
        if let Some(c) = self.paragraphs[last].cell {
            last = self.table_range(c.table).end - 1;
        }
        let mut new: Vec<Paragraph> = self.paragraphs[first..=last].to_vec();
        for (k, p) in new.iter_mut().enumerate() {
            let pi = first + k;
            if pi < start.para || pi > end.para {
                continue;
            }
            let lo = if pi == start.para { start.offset } else { 0 };
            let hi = if pi == end.para {
                end.offset
            } else {
                p.text.len()
            };
            if wants(RevKind::Format, true) {
                fmt_track::resolve_para(&mut p.props, accept);
            }
            if wants(RevKind::Format, false) && lo < hi {
                p.split_run_at(lo);
                p.split_run_at(hi);
                let mut at = 0;
                for run in &mut p.runs {
                    let r = at..at + run.len;
                    at = r.end;
                    if r.start >= lo && r.end <= hi && !r.is_empty() {
                        fmt_track::resolve_run(&mut run.props, accept);
                    }
                }
            }
            // Ranges to drop (back to front) and runs to keep as plain text.
            let mut drop: Vec<std::ops::Range<usize>> = Vec::new();
            let mut at = 0;
            for run in &mut p.runs {
                let r = at..at + run.len;
                at = r.end;
                if r.end > lo
                    && r.start < hi
                    && (wants(RevKind::Insert, false) || wants(RevKind::Delete, false))
                {
                    resolve_code(run, accept);
                }
                let Some(rev) = run.rev.clone() else { continue };
                if r.end <= lo || r.start >= hi || !wants(rev.kind, false) {
                    continue;
                }
                let remove = matches!(
                    (rev.kind, accept),
                    (RevKind::Delete, true) | (RevKind::Insert, false)
                );
                if remove {
                    drop.push(r);
                } else {
                    run.rev = None;
                }
            }
            for r in drop.into_iter().rev() {
                // Markers (bookmarks, comment ranges) survive the removal.
                drop_change(p, r);
            }
            p.normalize();
        }
        // Paragraph marks in the range, back to front: accepting a deleted
        // mark or rejecting an inserted one joins the paragraphs.
        for k in (0..new.len().saturating_sub(1)).rev() {
            let pi = first + k;
            let mark = Position::new(pi, self.paragraphs[pi].text.len());
            if mark < start || Position::new(pi + 1, 0) > end || !self.can_merge(pi, pi + 1) {
                continue;
            }
            let (ins, del) = mark_revs(&new[k]);
            if !ins && !del {
                continue;
            }
            let kind = if ins {
                RevKind::Insert
            } else {
                RevKind::Delete
            };
            if !wants(kind, false) {
                continue;
            }
            clear_mark_revs(&mut new[k]);
            if (del && accept) || (ins && !accept) {
                let right = new.remove(k + 1);
                join(&mut new[k], right);
            }
        }
        let count = last - first + 1;
        let mut tables = self.tables.clone();
        let only_rows = match only {
            Some((_, true)) | Some((RevKind::Format, _)) => Some(None),
            Some((k, false)) => Some(Some(k)),
            None => None,
        };
        let rows = match only_rows {
            Some(None) => false,
            Some(k) => track_table::resolve_rows(
                &mut new,
                &mut tables,
                first,
                Some((start, end)),
                accept,
                k,
            ),
            None => track_table::resolve_rows(
                &mut new,
                &mut tables,
                first,
                Some((start, end)),
                accept,
                None,
            ),
        };
        let mut rows = rows;
        if only.is_none() {
            let span = Some((start, end));
            rows |= track_table::resolve_table_props(&mut new, &mut tables, first, span, accept);
            rows |= track_table::resolve_cells(&mut new, &mut tables, first, span, accept, None);
        }
        if new.is_empty() && count == self.paragraphs.len() {
            new.push(Paragraph::default());
        }
        let after = Selection::caret(self.clamp_pos_in(&new, first, start, count));
        self.replace_with_tables(
            first,
            count,
            new,
            rows.then_some(tables),
            (sel, after),
            Kind::Other,
            true,
        );
        self.clamp_selection(after)
    }

    /// `pos` after paragraphs `first..first + count` became `paras`.
    fn clamp_pos_in(
        &self,
        paras: &[Paragraph],
        first: usize,
        pos: Position,
        count: usize,
    ) -> Position {
        let total = self.paragraphs.len() - count + paras.len();
        if pos.para >= first + paras.len() {
            let para = (first + paras.len()).min(total.saturating_sub(1));
            return Position::new(para, 0);
        }
        let p = &paras[pos.para - first];
        Position::new(pos.para, pos.offset.min(p.text.len()))
    }

    /// Accept or reject one table change: an inserted or deleted cell, or
    /// the table's property changes. One undo step.
    fn resolve_table_change(
        &mut self,
        c: &Change,
        what: track_table::TableChange,
        accept: bool,
        sel: Selection,
    ) -> Selection {
        let Some(id) = self.paragraphs[c.start.para].cell.map(|x| x.table) else {
            return sel;
        };
        let range = self.table_range(id);
        let mut paras: Vec<Paragraph> = self.paragraphs[range.clone()].to_vec();
        let mut tables = self.tables.clone();
        let changed = match what {
            track_table::TableChange::Props => {
                let done = tables
                    .get_mut(&id)
                    .is_some_and(|t| track_table::resolve_props(t, accept));
                if done && accept {
                    track_table::normalize_hmerge(&mut paras, &mut tables, id);
                }
                done
            }
            _ => track_table::resolve_cells(
                &mut paras,
                &mut tables,
                range.start,
                Some((c.start, c.end)),
                accept,
                Some(c.kind),
            ),
        };
        if !changed {
            return sel;
        }
        let lone = paras.is_empty() && self.paragraphs.len() == range.len();
        if lone {
            paras.push(Paragraph::default());
        }
        let after = Selection::caret(self.clamp_pos_in(&paras, range.start, c.start, range.len()));
        self.replace_with_tables(
            range.start,
            range.len(),
            paras,
            Some(tables),
            (sel, after),
            Kind::Other,
            true,
        );
        self.clamp_selection(after)
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
        if a == b {
            let c = self.change_at(sel)?;
            if let Some(what) = c.table.filter(|w| *w != track_table::TableChange::Row) {
                return Some(self.resolve_table_change(&c, what, accept, sel));
            }
            return Some(self.resolve_runs(c.start, c.end, accept, sel, Some((c.kind, c.para))));
        }
        let cs: Vec<Change> = self
            .changes()
            .into_iter()
            .filter(|c| c.start < b && c.end > a || (c.para && c.start <= b && c.end >= a))
            .collect();
        let start = cs.iter().map(|c| c.start).min()?.min(a);
        let end = cs.iter().map(|c| c.end).max()?.max(b);
        Some(self.resolve_runs(start, end, accept, sel, None))
    }

    /// Accept (or reject) every tracked change: insertions, deletions,
    /// paragraph marks, formatting and table rows, in the text and in the
    /// headers, footers and notes. One undo step.
    pub fn resolve_all(&mut self, accept: bool, sel: Selection) -> Selection {
        let before = self.side_state();
        let rev = self.revision();
        let after = self.resolve_body(accept, sel);
        let body = self.revision() != rev;
        if self.resolve_parts(accept) {
            if body {
                let now = self.side_state();
                self.set_last_side(before, now);
            } else {
                self.push_hf(before, None, sel);
            }
        }
        after
    }

    fn resolve_body(&mut self, accept: bool, sel: Selection) -> Selection {
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
                resolve_code(run, accept);
                if let Some(rev) = run.rev.take() {
                    let remove = (rev.kind == RevKind::Delete) == accept;
                    if remove {
                        drop.push(r);
                    }
                }
                // Formatting changes: accept drops the record, reject puts
                // the old properties back.
                fmt_track::resolve_run(&mut run.props, accept);
            }
            for r in drop.into_iter().rev() {
                drop_change(&mut p, r);
            }
            fmt_track::resolve_para(&mut p.props, accept);
            crate::part_edit::resolve_boxes(&mut p, accept);
            let (ins, del) = mark_revs(&p);
            let joins = (del && accept) || (ins && !accept);
            clear_mark_revs(&mut p);
            p.normalize();
            if join_next && let Some(prev) = out.last_mut() {
                if prev.cell == p.cell {
                    join(prev, p);
                } else {
                    out.push(p);
                }
            } else {
                out.push(p);
            }
            join_next = joins;
        }
        let mut tables = self.tables.clone();
        let mut rows = track_table::resolve_table_props(&mut out, &mut tables, 0, None, accept);
        rows |= track_table::resolve_cells(&mut out, &mut tables, 0, None, accept, None);
        rows |= track_table::resolve_rows(&mut out, &mut tables, 0, None, accept, None);
        if out.is_empty() {
            out.push(Paragraph::default());
        }
        if out == old && !rows {
            return sel;
        }
        let n = old.len();
        let after = self.clamp_selection_in(&out, sel);
        self.replace_with_tables(
            0,
            n,
            out,
            rows.then_some(tables),
            (sel, after),
            Kind::Other,
            true,
        );
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
        let id = self.fresh_comment_id();
        let c = self.new_comment(&id, text, author);
        self.comments.push(c);
        self.comments_modified = true;
        self.insert_comment_marks(&id, a, b, false, sel);
        // Fold the comment list change into the same undo step.
        let after_comments = self.review_state();
        self.attach_side_state(before_comments, after_comments);
        Some(id)
    }

    pub(crate) fn has_comment_marker(&self, id: &str) -> bool {
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

    /// Delete a comment, its replies and their markers: one undo step.
    pub fn delete_comment(&mut self, id: &str, sel: Selection) -> bool {
        let Some(i) = self.comments.iter().position(|c| c.id == id) else {
            return false;
        };
        let before = self.review_state();
        // The comment and its replies (a thread when it's the first).
        let mut gone: Vec<String> = vec![self.comments[i].id.clone()];
        loop {
            let more: Vec<String> = self
                .comments
                .iter()
                .filter(|c| {
                    c.parent.as_ref().is_some_and(|p| gone.contains(p)) && !gone.contains(&c.id)
                })
                .map(|c| c.id.clone())
                .collect();
            if more.is_empty() {
                break;
            }
            gone.extend(more);
        }
        self.comments.retain(|c| !gone.contains(&c.id));
        self.comments_modified = true;
        let hit = |o: &Object| comment_id(o).is_some_and(|(_, x)| gone.contains(&x));
        let touched: Vec<usize> = (0..self.paragraphs.len())
            .filter(|&pi| self.paragraphs[pi].objects().any(|(_, o)| hit(o)))
            .collect();
        if let (Some(&first), Some(&last)) = (touched.first(), touched.last()) {
            let mut new: Vec<Paragraph> = self.paragraphs[first..=last].to_vec();
            for p in &mut new {
                let offs: Vec<usize> = p
                    .objects()
                    .filter(|(_, o)| hit(o))
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
