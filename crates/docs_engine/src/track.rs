//! Track Changes for edits made in Kantor (Review > Track Changes, Word's
//! `w:trackRevisions` in settings.xml).
//!
//! While it's on, typed and pasted text becomes a `w:ins` by the user, and
//! deleted text stays in place as a `w:del` (shown struck through) instead
//! of disappearing; deleting text the same author inserted removes it, as
//! in Word. Enter inserts a tracked paragraph mark and deleting across
//! paragraphs marks the marks deleted, so accepting joins them. Formatting
//! changes aren't tracked. Every tracked edit is one ordinary undo step.

use std::sync::Arc;

use crate::document::{Document, Kind};
use crate::model::{Object, Paragraph, Position, RunProps, Selection, element_name};
use crate::review::{RevKind, Revision, now_utc};
use crate::xml;

impl Document {
    /// Word's compatibility mode (settings.xml `compatibilityMode`): 15 for
    /// Word 2013 and later and for new documents, 12 when a file doesn't
    /// say (Word 2007), 14 for Word 2010, 11 for converted Word 97 files.
    pub fn compat_mode(&self) -> u32 {
        self.compat_mode
    }

    /// Whether edits are tracked (Review > Track Changes).
    pub fn track_changes(&self) -> bool {
        self.track
    }

    /// Who tracked edits are by.
    pub fn track_author(&self) -> &str {
        &self.author
    }

    /// The name tracked edits (and new comments) are recorded under.
    pub fn set_author(&mut self, name: &str) {
        self.author = name.to_owned();
    }

    /// Turn Track Changes on or off (saved in settings.xml): one undo step.
    pub fn set_track_changes(&mut self, on: bool, sel: Selection) {
        if self.track == on {
            return;
        }
        let before = self.side_state();
        self.track = on;
        let after = self.side_state();
        self.push_command(
            0,
            Vec::new(),
            Vec::new(),
            sel,
            Some(Box::new((before, after))),
        );
    }

    /// The largest `w:id` of a tracked change (or other annotation) in the
    /// text.
    pub(crate) fn max_rev_id(&self) -> u64 {
        let mut max = 0u64;
        let mut scan = |mut s: &str| {
            while let Some(i) = s.find(" w:id=\"") {
                let rest = &s[i + 7..];
                if let Some(e) = rest.find('"')
                    && let Ok(v) = rest[..e].parse::<u64>()
                {
                    max = max.max(v);
                }
                s = rest;
            }
        };
        for t in self.tables.values() {
            for x in [&t.tbl_pr, &t.grid_change].into_iter().flatten() {
                scan(x);
            }
            for r in &t.rows {
                for x in r.tr_pr.iter() {
                    scan(x);
                }
                for c in &r.cells {
                    for x in c.tc_pr.iter() {
                        scan(x);
                    }
                }
            }
        }
        for p in &self.paragraphs {
            for x in p.props.extras.iter() {
                scan(x);
            }
            for r in &p.runs {
                if let Some(rev) = &r.rev {
                    scan(&rev.tag);
                }
                for x in r.props.extras.iter() {
                    scan(x);
                }
                if let Some(Object::Opaque(o)) = &r.object
                    && o.level == crate::object::Level::Marker
                {
                    scan(&o.xml);
                }
            }
        }
        max
    }

    /// A new revision by the user with id `id`.
    pub(crate) fn new_rev(&self, kind: RevKind, id: u64) -> Arc<Revision> {
        let name = match kind {
            RevKind::Insert => "ins",
            RevKind::Delete | RevKind::Format => "del",
        };
        let date = now_utc();
        Arc::new(Revision {
            kind,
            tag: format!(
                "<w:{name} w:id=\"{id}\" w:author=\"{}\" w:date=\"{date}\">",
                xml::escape(&self.author)
            ),
            author: self.author.clone(),
            date,
        })
    }

    /// The user's revision of `kind` on the run next to `offset` (before it
    /// for insertions, either side for deletions), to extend instead of
    /// starting a new one.
    fn adjacent_rev(
        p: &Paragraph,
        offset: usize,
        kind: RevKind,
        author: &str,
    ) -> Option<Arc<Revision>> {
        p.run_ranges()
            .filter(|(r, run)| {
                run.object.is_none()
                    && (r.end == offset || (kind == RevKind::Delete && r.start == offset))
            })
            .filter_map(|(_, run)| run.rev.clone())
            .find(|rev| rev.kind == kind && rev.author == author)
    }

    /// Mark `start..end` deleted (removing the user's own insertions in it)
    /// in a copy of paragraphs `start.para..=end.para`. Returns them and
    /// where `end` is afterwards (relative to the same first paragraph).
    pub(crate) fn tracked_cut(
        &self,
        start: Position,
        end: Position,
        ids: &mut u64,
    ) -> (Vec<Paragraph>, Position) {
        let first = start.para;
        let mut new: Vec<Paragraph> = self.paragraphs[first..=end.para].to_vec();
        let me = self.author.clone();
        let del = Self::adjacent_rev(&new[0], start.offset, RevKind::Delete, &me)
            .or_else(|| {
                let last = new.len() - 1;
                Self::adjacent_rev(&new[last], end.offset, RevKind::Delete, &me)
            })
            .unwrap_or_else(|| {
                *ids += 1;
                self.new_rev(RevKind::Delete, *ids)
            });
        let mut end_off = end.offset;
        let n = new.len();
        for (k, p) in new.iter_mut().enumerate() {
            let lo = if k == 0 { start.offset } else { 0 };
            let hi = if k == n - 1 { end.offset } else { p.text.len() };
            let dropped = strike(p, lo, hi, &del, &me);
            if k == n - 1 {
                end_off -= dropped;
            }
        }
        // Paragraph marks inside the range, back to front: the user's own
        // inserted marks go (the paragraphs join), others are marked deleted.
        let mut end_k = n - 1;
        for k in (0..n - 1).rev() {
            let pi = first + k;
            if !self.can_merge(pi, pi + 1) {
                continue;
            }
            let (ins_mine, mark_del) = mark_state(&new[k], &me);
            if ins_mine {
                strip_mark(&mut new[k], &["ins"]);
                let right = new.remove(k + 1);
                let len = new[k].text.len();
                new[k].append(right);
                if end_k == k + 1 {
                    end_off += len;
                }
                end_k -= 1;
            } else if !mark_del {
                *ids += 1;
                let el = format!(
                    "<w:del w:id=\"{ids}\" w:author=\"{}\" w:date=\"{}\"/>",
                    xml::escape(&del.author),
                    del.date
                );
                set_mark_rev(&mut new[k], &el);
            }
        }
        (new, Position::new(first + end_k, end_off))
    }

    /// [`Document::delete_range`] with Track Changes on. Returns the
    /// selection after (collapsed at `start`, or at the end of what's now
    /// struck through when `forward`).
    pub(crate) fn tracked_delete(
        &mut self,
        sel: Selection,
        kind: Kind,
        forward: bool,
    ) -> Selection {
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        if start == end {
            return Selection::caret(start);
        }
        let mut ids = self.max_rev_id();
        let (new, end_after) = self.tracked_cut(start, end, &mut ids);
        let after = Selection::caret(if forward { end_after } else { start });
        let boundary = start.para != end.para;
        self.replace(
            start.para,
            end.para - start.para + 1,
            new,
            (sel, after),
            kind,
            boundary,
        );
        after
    }

    /// [`Document::insert_text`] with Track Changes on: the selection is
    /// marked deleted and the text goes in after it as an insertion.
    pub(crate) fn tracked_insert(
        &mut self,
        sel: Selection,
        text: &str,
        props: Option<RunProps>,
        kind: Option<Kind>,
    ) -> Selection {
        let sel = self.clamp_selection(sel);
        let (start, end) = sel.ordered();
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut ids = self.max_rev_id();
        let (mut out, at) = if start == end {
            (vec![self.paragraphs[start.para].clone()], start)
        } else {
            self.tracked_cut(start, end, &mut ids)
        };
        let props = props.unwrap_or_else(|| self.run_props_at(start));
        let me = self.author.clone();
        let k0 = at.para - start.para;
        let ins =
            Self::adjacent_rev(&out[k0], at.offset, RevKind::Insert, &me).unwrap_or_else(|| {
                ids += 1;
                self.new_rev(RevKind::Insert, ids)
            });
        let mut pieces = text.split('\n');
        let first_piece = pieces.next().unwrap_or_default();
        insert_tracked(&mut out[k0], at.offset, first_piece, &props, &ins);
        let mut caret = Position::new(at.para, at.offset + first_piece.len());
        for piece in pieces {
            let k = caret.para - start.para;
            let at_end = caret.offset == out[k].text.len();
            let mut right = out[k].split_off(caret.offset);
            if at_end && right.text.is_empty() {
                right.props.style = self.styles.next_style(out[k].props.style.as_deref());
                if right.props.style.as_deref() == Some("Normal")
                    && self.styles.get("Normal").is_some_and(|s| s.is_default)
                {
                    right.props.style = None;
                }
            }
            // The left paragraph's mark is the new one.
            set_mark_rev(&mut out[k], &ins_tag(&ins));
            insert_tracked(&mut right, 0, piece, &props, &ins);
            caret = Position::new(caret.para + 1, piece.len());
            out.insert(k + 1, right);
        }
        let after = Selection::caret(caret);
        let typing = sel.is_empty() && !text.contains('\n') && text.chars().count() == 1;
        let boundary = text.chars().all(char::is_whitespace);
        let count = end.para - start.para + 1;
        self.replace(
            start.para,
            count,
            out,
            (sel, after),
            kind.unwrap_or(if typing { Kind::Typing } else { Kind::Other }),
            boundary,
        );
        after
    }

    /// [`Document::replace_ranges`] (Replace All) with Track Changes on:
    /// each match is struck through and the replacement goes in after it
    /// as an insertion, one revision pair per match as in Word. `ranges`
    /// are valid, sorted and don't overlap. One undo step.
    pub(crate) fn tracked_replace_ranges(
        &mut self,
        ranges: &[(usize, std::ops::Range<usize>)],
        with: &str,
        sel: Selection,
    ) -> Selection {
        let (Some(f), Some(l)) = (ranges.first(), ranges.last()) else {
            return sel;
        };
        let (first, last) = (f.0, l.0);
        let mut out: Vec<Paragraph> = self.paragraphs[first..=last].to_vec();
        let mut ids = self.max_rev_id();
        let me = self.author.clone();
        let mut caret = Position::new(last, 0);
        for para in first..=last {
            let p = &mut out[para - first];
            let mut delta: isize = 0;
            for (_, r) in ranges.iter().filter(|(q, _)| *q == para) {
                let a = (r.start as isize + delta) as usize;
                let b = (r.end as isize + delta) as usize;
                let first_char = p.text[a..].chars().next().map_or(0, char::len_utf8);
                let props = p.props_at(a + first_char.min(b - a));
                ids += 1;
                let del = self.new_rev(RevKind::Delete, ids);
                let dropped = strike(p, a, b, &del, &me);
                let at = b - dropped;
                ids += 1;
                let ins = self.new_rev(RevKind::Insert, ids);
                insert_tracked(p, at, with, &props, &ins);
                delta += with.len() as isize - dropped as isize;
                caret = Position::new(para, at + with.len());
            }
        }
        let after = Selection::caret(caret);
        self.replace(
            first,
            last - first + 1,
            out,
            (sel, after),
            Kind::Other,
            true,
        );
        self.clamp_selection(after)
    }

    /// With Track Changes on, mark the object at `offset` of `p` (a copy
    /// about to replace a paragraph) as a tracked insertion.
    pub(crate) fn track_object(&self, p: &mut Paragraph, offset: usize) {
        if !self.track {
            return;
        }
        let rev = Self::adjacent_rev(p, offset, RevKind::Insert, &self.author)
            .unwrap_or_else(|| self.new_rev(RevKind::Insert, self.max_rev_id() + 1));
        let i = p.split_run_at(offset);
        if let Some(run) = p.runs.get_mut(i)
            && run.object.is_some()
        {
            run.rev = Some(rev);
        }
        p.normalize();
    }
}

/// Mark `lo..hi` of `p` deleted as part of `del`, removing `me`'s own
/// insertions in it. Returns the bytes removed.
fn strike(p: &mut Paragraph, lo: usize, hi: usize, del: &Arc<Revision>, me: &str) -> usize {
    if lo >= hi {
        return 0;
    }
    p.split_run_at(lo);
    p.split_run_at(hi);
    let mut drop = Vec::new();
    let mut at = 0;
    for run in &mut p.runs {
        let r = at..at + run.len;
        at = r.end;
        if r.start < lo || r.end > hi || r.is_empty() {
            continue;
        }
        if run.object.as_ref().is_some_and(Object::is_zero_width) {
            continue;
        }
        match &run.rev {
            Some(rev) if rev.kind == RevKind::Insert && rev.author == me => drop.push(r),
            Some(rev) if rev.kind == RevKind::Delete => {}
            _ => run.rev = Some(del.clone()),
        }
    }
    let dropped: usize = drop.iter().map(|r| r.len()).sum();
    for r in drop.into_iter().rev() {
        p.delete(r);
    }
    p.normalize();
    dropped
}

/// Insert `text` at `offset` as part of the insertion `rev`.
fn insert_tracked(
    p: &mut Paragraph,
    offset: usize,
    text: &str,
    props: &RunProps,
    rev: &Arc<Revision>,
) {
    if text.is_empty() {
        return;
    }
    p.insert(offset, text, props.clone());
    p.split_run_at(offset);
    p.split_run_at(offset + text.len());
    let mut at = 0;
    for run in &mut p.runs {
        let r = at..at + run.len;
        at = r.end;
        if r.start >= offset && r.end <= offset + text.len() && run.object.is_none() {
            run.rev = Some(rev.clone());
        }
    }
    p.normalize();
}

/// The `w:ins` element a paragraph mark carries for `rev`.
fn ins_tag(rev: &Revision) -> String {
    format!("{}/>", rev.tag.trim_end_matches('>').trim_end_matches('/'))
}

/// Whether a paragraph's mark is inserted by `me`, and whether it's
/// deleted.
pub(crate) fn mark_state(p: &Paragraph, me: &str) -> (bool, bool) {
    let mut out = (false, false);
    for x in mark_elements(p) {
        let Some(el) = xml::parse(&x) else { continue };
        match el.name.as_str() {
            "ins" | "moveTo" if el.attr("author") == Some(me) => out.0 = true,
            "del" | "moveFrom" => out.1 = true,
            _ => {}
        }
    }
    out
}

/// The revision elements (`w:ins`, `w:del`…) on a paragraph's mark.
pub(crate) fn mark_elements(p: &Paragraph) -> Vec<String> {
    let mut out = Vec::new();
    if p.text.is_empty() {
        for x in p.runs[0].props.extras.iter() {
            if matches!(element_name(x), "ins" | "del" | "moveTo" | "moveFrom") {
                out.push(x.to_owned());
            }
        }
    }
    for raw in p.props.extras.iter().filter(|x| element_name(x) == "rPr") {
        let Some(el) = xml::parse(raw) else { continue };
        for c in &el.children {
            if matches!(c.name.as_str(), "ins" | "del" | "moveTo" | "moveFrom") {
                let attrs: String = c
                    .attrs
                    .iter()
                    .map(|(k, v)| format!(" w:{k}=\"{}\"", xml::escape(v)))
                    .collect();
                out.push(format!("<w:{}{attrs}/>", c.name));
            }
        }
    }
    out
}

/// Remove the mark's revision elements named in `names`.
pub(crate) fn strip_mark(p: &mut Paragraph, names: &[&str]) {
    for n in names {
        p.runs[0].props.extras.remove(n);
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
        for n in names {
            let pat = format!(":{n} ");
            while let Some(i) = s.find(&pat) {
                let start = s[..i].rfind('<').unwrap_or(i);
                let end = s[i..].find("/>").map_or(s.len(), |e| i + e + 2);
                s.replace_range(start..end, "");
            }
        }
        if s.replace(char::is_whitespace, "") != "<w:rPr></w:rPr>" {
            p.props.extras.push(&s);
        }
    }
}

/// Put a revision element (`<w:ins …/>` or `<w:del …/>`) on a paragraph's
/// mark.
pub(crate) fn set_mark_rev(p: &mut Paragraph, el: &str) {
    // A mark is inserted or deleted once.
    let name = element_name(el);
    if mark_elements(p).iter().any(|x| element_name(x) == name) {
        return;
    }
    if p.text.is_empty() {
        p.runs[0].props.extras.push(el);
    }
    // Non-empty paragraphs write their mark from the raw `w:rPr`; empty
    // ones from the first run, so keep both in step.
    let raw = p
        .props
        .extras
        .iter()
        .find(|x| element_name(x) == "rPr")
        .map(str::to_owned);
    match raw {
        Some(raw) => {
            let mut s = raw.clone();
            if let Some(i) = s.find("/>").filter(|&i| !s[..i].contains('>')) {
                // `<w:rPr/>`
                s = format!("{}>{el}</w:rPr>", &s[..i]);
            } else if element_name(el) == "del"
                && let Some(i) = s.find("<w:ins ")
                && let Some(e) = s[i..].find("/>")
            {
                // `w:ins` comes before `w:del` (CT_ParaRPr).
                s.insert_str(i + e + 2, el);
            } else if let Some(i) = s.find('>') {
                s.insert_str(i + 1, el);
            }
            p.props.extras.remove("rPr");
            p.props.extras.push(&s);
        }
        None => p.props.extras.push(&format!("<w:rPr>{el}</w:rPr>")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(paras: &[&str]) -> Document {
        let mut d = Document::from_paragraphs(paras.iter().map(|t| Paragraph::new(*t)).collect());
        d.set_author("Ani Wijaya");
        d.set_track_changes(true, Selection::default());
        d
    }

    fn caret(para: usize, offset: usize) -> Selection {
        Selection::caret(Position::new(para, offset))
    }

    fn texts(d: &Document) -> Vec<String> {
        d.paragraphs().iter().map(|p| p.text.clone()).collect()
    }

    fn kinds(d: &Document) -> Vec<(RevKind, String)> {
        d.changes().into_iter().map(|c| (c.kind, c.text)).collect()
    }

    #[test]
    fn typing_is_one_insertion_by_the_user_and_undoes() {
        let mut d = doc(&["hello"]);
        let mut c = caret(0, 5);
        for ch in " world".chars() {
            c = d.insert_text(c, &ch.to_string(), None);
        }
        assert_eq!(texts(&d), ["hello world"]);
        assert_eq!(kinds(&d), [(RevKind::Insert, " world".to_owned())]);
        let ch = &d.changes()[0];
        assert_eq!(ch.author, "Ani Wijaya");
        assert!(ch.date.ends_with('Z'));
        while d.paragraph(0).text != "hello" {
            d.undo().unwrap();
        }
        assert!(d.changes().is_empty());
        // Turning tracking on was an undo step too.
        assert!(d.track_changes());
        d.undo();
        assert!(!d.track_changes());
    }

    #[test]
    fn deleting_strikes_through_instead_of_removing() {
        let mut d = doc(&["hello world"]);
        let c = d.backspace(caret(0, 11));
        assert_eq!(c.head, Position::new(0, 10), "caret moves left");
        d.backspace(c);
        assert_eq!(texts(&d), ["hello world"], "text stays");
        assert_eq!(kinds(&d), [(RevKind::Delete, "ld".to_owned())]);
        // Delete moves the caret past the struck text.
        let c = d.delete_forward(caret(0, 0));
        assert_eq!(c.head, Position::new(0, 1));
        assert_eq!(d.changes().len(), 2);
        let _ = c;
        // Typing over a selection: the old text struck, the new after it.
        let mut d = doc(&["hello world"]);
        let s = Selection {
            anchor: Position::new(0, 6),
            head: Position::new(0, 11),
        };
        let c = d.insert_text(s, "there", None);
        assert_eq!(texts(&d), ["hello worldthere"]);
        assert_eq!(c.head, Position::new(0, 16));
        assert_eq!(
            kinds(&d),
            [
                (RevKind::Delete, "world".to_owned()),
                (RevKind::Insert, "there".to_owned())
            ]
        );
        d.undo();
        assert_eq!(texts(&d), ["hello world"]);
        assert!(d.changes().is_empty(), "one undo step");
    }

    #[test]
    fn deleting_your_own_insertion_removes_it() {
        let mut d = doc(&["ab"]);
        let c = d.insert_text(caret(0, 1), "xyz", None);
        let c = d.backspace(c);
        assert_eq!(texts(&d), ["axyb"]);
        assert_eq!(kinds(&d), [(RevKind::Insert, "xy".to_owned())]);
        // Someone else's insertion is marked deleted instead.
        d.set_author("Budi");
        d.backspace(c);
        assert_eq!(texts(&d), ["axyb"]);
        assert_eq!(
            kinds(&d),
            [
                (RevKind::Insert, "x".to_owned()),
                (RevKind::Delete, "y".to_owned())
            ]
        );
    }

    #[test]
    fn enter_and_joins_are_tracked_marks() {
        let mut d = doc(&["abcdef"]);
        let c = d.insert_text(caret(0, 3), "\n", None);
        assert_eq!(texts(&d), ["abc", "def"]);
        assert_eq!(c.head, Position::new(1, 0));
        assert_eq!(kinds(&d), [(RevKind::Insert, "\n".to_owned())]);
        // Rejecting the new mark joins the paragraphs again.
        d.reject_change(caret(0, 3)).unwrap();
        assert_eq!(texts(&d), ["abcdef"]);
        assert!(!d.has_changes());

        // Deleting across paragraphs marks text and mark deleted;
        // accepting joins.
        let mut d = doc(&["one", "two"]);
        d.delete_selection(Selection {
            anchor: Position::new(0, 2),
            head: Position::new(1, 1),
        });
        assert_eq!(texts(&d), ["one", "two"]);
        assert_eq!(kinds(&d), [(RevKind::Delete, "e\nt".to_owned())]);
        d.accept_change(caret(0, 2)).unwrap();
        assert_eq!(texts(&d), ["onwo"]);
        assert!(!d.has_changes());

        // Backspace over your own Enter removes it.
        let mut d = doc(&["abcdef"]);
        let c = d.insert_text(caret(0, 3), "\n", None);
        d.backspace(c);
        assert_eq!(texts(&d), ["abcdef"]);
        assert!(!d.has_changes());
    }

    #[test]
    fn accept_and_reject_all_resolve_own_edits() {
        let mut d = doc(&["keep this", "and this"]);
        let c = d.insert_text(caret(0, 4), " all", None);
        d.backspace(caret(1, 8));
        d.insert_text(c, "\nnew", None);
        let original = texts(&d);
        let sel = d.resolve_all(false, Selection::default());
        let _ = sel;
        assert_eq!(texts(&d), ["keep this", "and this"]);
        d.undo();
        assert_eq!(texts(&d), original);
        d.resolve_all(true, Selection::default());
        assert_eq!(texts(&d), ["keep all", "new this", "and thi"]);
        assert!(!d.has_changes());
    }

    #[test]
    fn tracked_edits_save_as_ins_and_del() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.docx");
        let mut d = doc(&["hello world"]);
        d.insert_text(caret(0, 5), ",", None);
        d.backspace(caret(0, 12));
        d.insert_text(caret(0, 0), "\n", None);
        d.save_as(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(&bytes[..])).unwrap();
        let mut read = |name: &str| {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut zip.by_name(name).unwrap(), &mut s).unwrap();
            s
        };
        let body = read("word/document.xml");
        assert!(body.contains("w:author=\"Ani Wijaya\""), "{body}");
        assert!(
            body.contains("<w:delText xml:space=\"preserve\">d</w:delText>"),
            "{body}"
        );
        assert!(
            body.contains("<w:t xml:space=\"preserve\">,</w:t>"),
            "{body}"
        );
        assert!(
            body.contains("<w:rPr><w:ins w:id="),
            "inserted mark: {body}"
        );
        assert!(read("word/settings.xml").contains("<w:trackRevisions/>"));
        let back = Document::open(&path).unwrap();
        assert!(back.track_changes());
        assert_eq!(texts(&back), texts(&d));
        let a: Vec<_> = back
            .changes()
            .into_iter()
            .map(|c| (c.kind, c.text, c.author))
            .collect();
        let b: Vec<_> = d
            .changes()
            .into_iter()
            .map(|c| (c.kind, c.text, c.author))
            .collect();
        assert_eq!(a, b);
        // Revision ids are unique.
        let ids: Vec<&str> = body
            .split(" w:id=\"")
            .skip(1)
            .map(|s| &s[..s.find('"').unwrap()])
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "{ids:?}");
    }
}
