//! Field updates and new fields with Track Changes on.
//!
//! Word records updating a field (F9, Update Table) as tracked changes:
//! the old result is deleted and the new one inserted. Kantor works the
//! update out as usual and then compares the changed paragraphs with what
//! they were: in each paragraph, text and objects only in the old version
//! are struck through (a deletion by the user) and those only in the new
//! one inserted after them. A table of contents that gains or loses lines
//! is replaced as a whole (its old lines deleted, the new ones inserted),
//! as Word does.

use std::sync::Arc;

use crate::document::Document;
use crate::model::{Object, Paragraph, Run};
use crate::review::{RevKind, Revision};
use crate::track::{mark_state, set_mark_rev, strip_mark};
use crate::xml;

/// Diff atoms: one character, or one object.
#[derive(PartialEq)]
enum Atom<'a> {
    Char(char),
    Obj(&'a Object),
}

/// A paragraph's pieces: byte range, atom, its run.
fn atoms(p: &Paragraph) -> Vec<(std::ops::Range<usize>, Atom<'_>, &Run)> {
    let mut out = Vec::new();
    for (r, run) in p.run_ranges() {
        match &run.object {
            Some(o) => out.push((r, Atom::Obj(o), run)),
            None => {
                for (i, c) in p.text[r.clone()].char_indices() {
                    let a = r.start + i;
                    out.push((a..a + c.len_utf8(), Atom::Char(c), run));
                }
            }
        }
    }
    out
}

/// Pieces whose changes are never tracked: markers (bookmarks, comment
/// ranges) and field marks. They are always the new version's, so fields
/// stay whole and paired; the results between them are what's tracked.
fn untracked(run: &Run) -> bool {
    run.object
        .as_ref()
        .is_some_and(|o| o.is_marker() || matches!(o, Object::Field(_)))
}

fn deleted(run: &Run) -> bool {
    run.rev.as_ref().is_some_and(|r| r.kind == RevKind::Delete)
}

/// Longest common subsequence of `a` and `b` (index pairs), or `None` when
/// they're too long to compare.
fn lcs<T: PartialEq>(a: &[T], b: &[T]) -> Option<Vec<(usize, usize)>> {
    let (n, m) = (a.len(), b.len());
    if n.saturating_mul(m) > 4_000_000 {
        return None;
    }
    let mut t = vec![0u32; (n + 1) * (m + 1)];
    let w = m + 1;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            t[i * w + j] = if a[i] == b[j] {
                t[(i + 1) * w + j + 1] + 1
            } else {
                t[(i + 1) * w + j].max(t[i * w + j + 1])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if t[(i + 1) * w + j] >= t[i * w + j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    Some(out)
}

impl Document {
    /// `new` (replacing `old`, paragraphs from the same place) with the
    /// differences recorded as the user's tracked deletions and
    /// insertions. Unchanged when Track Changes is off.
    pub(crate) fn track_rewrite(&self, old: &[Paragraph], new: Vec<Paragraph>) -> Vec<Paragraph> {
        if !self.track || old == new.as_slice() {
            return new;
        }
        let mut ids = self.max_rev_id();
        // Line up the paragraphs that didn't change; between them, the
        // same number of old and new paragraphs are compared one by one,
        // other stretches replaced as a whole.
        let anchors = if old.len() == new.len() {
            Vec::new()
        } else {
            lcs(old, &new).unwrap_or_default()
        };
        let mut out = Vec::with_capacity(new.len());
        let (mut i, mut j) = (0, 0);
        for (ai, aj) in anchors
            .into_iter()
            .chain(std::iter::once((old.len(), new.len())))
        {
            let (og, ng) = (&old[i..ai], &new[j..aj]);
            if og.len() == ng.len() {
                for (o, n) in og.iter().zip(ng) {
                    out.push(if o == n {
                        n.clone()
                    } else {
                        self.diff_para(o, n, &mut ids)
                    });
                }
            } else if ng.is_empty() || og.is_empty() {
                // Only removed or only added lines: each one's mark goes
                // with it.
                out.extend(self.whole_lines(og, ng, &mut ids));
            } else {
                out.extend(self.replace_block(og, ng.to_vec(), &mut ids));
            }
            if ai < old.len() {
                out.push(new[aj].clone());
            }
            i = ai + 1;
            j = aj + 1;
        }
        out
    }

    /// One paragraph's changes as tracked deletions and insertions.
    fn diff_para(&self, old: &Paragraph, new: &Paragraph, ids: &mut u64) -> Paragraph {
        let me = self.author.as_str();
        let oa = atoms(old);
        let na = atoms(new);
        // Text already struck through stays as it is and isn't compared.
        let ok: Vec<usize> = (0..oa.len()).filter(|&i| !deleted(oa[i].2)).collect();
        let nk: Vec<usize> = (0..na.len()).filter(|&i| !deleted(na[i].2)).collect();
        let left: Vec<&Atom> = ok.iter().map(|&i| &oa[i].1).collect();
        let right: Vec<&Atom> = nk.iter().map(|&i| &na[i].1).collect();
        let pairs: Vec<(usize, usize)> = match lcs(&left, &right) {
            Some(p) => p.into_iter().map(|(a, b)| (ok[a], nk[b])).collect(),
            None => Vec::new(),
        };
        let mut out = Paragraph {
            props: new.props.clone(),
            cell: new.cell,
            ..Paragraph::default()
        };
        out.runs.clear();
        let mut del: Option<Arc<Revision>> = None;
        let mut ins: Option<Arc<Revision>> = None;
        let push = |out: &mut Paragraph,
                    (r, run): (&std::ops::Range<usize>, &Run),
                    text: &str,
                    rev: Option<Arc<Revision>>| {
            match &run.object {
                Some(o) => {
                    out.push_object_tracked(o.clone(), run.props.clone(), run.link.clone(), rev)
                }
                None => {
                    out.push_tracked(&text[r.clone()], run.props.clone(), run.link.clone(), rev)
                }
            }
        };
        let (mut i, mut j) = (0, 0);
        for (pi, pj) in pairs
            .into_iter()
            .chain(std::iter::once((oa.len(), na.len())))
        {
            // Old-only pieces: struck through (the user's own insertions
            // simply go).
            while i < pi {
                let (r, _, run) = &oa[i];
                i += 1;
                if untracked(run) {
                    continue; // markers are the new version's
                }
                if deleted(run) {
                    push(&mut out, (r, run), &old.text, run.rev.clone());
                    continue;
                }
                if run
                    .rev
                    .as_ref()
                    .is_some_and(|v| v.kind == RevKind::Insert && v.author == me)
                {
                    continue;
                }
                let rev = del
                    .get_or_insert_with(|| {
                        *ids += 1;
                        self.new_rev(RevKind::Delete, *ids)
                    })
                    .clone();
                push(&mut out, (r, run), &old.text, Some(rev));
            }
            // New-only pieces: inserted after them.
            while j < pj {
                let (r, _, run) = &na[j];
                j += 1;
                if deleted(run) {
                    continue; // the old side has it
                }
                if untracked(run) {
                    push(&mut out, (r, run), &new.text, None);
                    continue;
                }
                let rev = ins
                    .get_or_insert_with(|| {
                        *ids += 1;
                        self.new_rev(RevKind::Insert, *ids)
                    })
                    .clone();
                push(&mut out, (r, run), &new.text, Some(rev));
            }
            // The common piece, as the new version has it (its revision
            // as before).
            if pi < oa.len() {
                let (_, _, orun) = &oa[pi];
                let (r, _, nrun) = &na[pj];
                let run = Run {
                    rev: orun.rev.clone(),
                    ..(*nrun).clone()
                };
                push(&mut out, (r, &run), &new.text, orun.rev.clone());
                i = pi + 1;
                j = pj + 1;
                del = None;
                ins = None;
            }
        }
        if out.text.is_empty() {
            out.runs = new.runs.clone();
        }
        out.normalize();
        out
    }

    /// Lines removed (`old`) or added (`new`) as a whole: their text and
    /// marks deleted or inserted.
    fn whole_lines(&self, old: &[Paragraph], new: &[Paragraph], ids: &mut u64) -> Vec<Paragraph> {
        let me = self.author.as_str();
        *ids += 1;
        let kind = if old.is_empty() {
            RevKind::Insert
        } else {
            RevKind::Delete
        };
        let rev = self.new_rev(kind, *ids);
        *ids += 1;
        let name = if kind == RevKind::Insert {
            "ins"
        } else {
            "del"
        };
        let mark = format!(
            "<w:{name} w:id=\"{}\" w:author=\"{}\" w:date=\"{}\"/>",
            *ids,
            xml::escape(&rev.author),
            rev.date
        );
        let mut out = Vec::new();
        for p in old.iter().chain(new) {
            let mut p = p.clone();
            let mut drop = Vec::new();
            let mut at = 0;
            for run in &mut p.runs {
                let r = at..at + run.len;
                at = r.end;
                if r.is_empty() || untracked(run) || deleted(run) {
                    continue;
                }
                let own = run
                    .rev
                    .as_ref()
                    .is_some_and(|v| v.kind == RevKind::Insert && v.author == me);
                if kind == RevKind::Delete && own {
                    drop.push(r);
                } else if kind == RevKind::Delete || run.rev.is_none() {
                    run.rev = Some(rev.clone());
                }
            }
            for r in drop.into_iter().rev() {
                p.delete(r);
            }
            if kind == RevKind::Delete {
                // Field marks are the new version's: these lines have none.
                let marks: Vec<usize> = p
                    .run_ranges()
                    .filter(|(_, r)| matches!(r.object, Some(Object::Field(_))))
                    .map(|(r, _)| r.start)
                    .collect();
                for i in marks.into_iter().rev() {
                    p.remove_object(i);
                }
            }
            p.normalize();
            let (ins, del) = mark_state(&p, me);
            if kind == RevKind::Delete && ins {
                strip_mark(&mut p, &["ins"]);
                if p.text.is_empty() {
                    continue;
                }
            }
            if !(ins || del) || (kind == RevKind::Delete && !del) {
                set_mark_rev(&mut p, &mark);
            }
            out.push(p);
        }
        out
    }

    /// Old paragraphs struck through (marks included), then the new ones
    /// inserted.
    fn replace_block(
        &self,
        old: &[Paragraph],
        new: Vec<Paragraph>,
        ids: &mut u64,
    ) -> Vec<Paragraph> {
        let me = self.author.clone();
        *ids += 1;
        let del = self.new_rev(RevKind::Delete, *ids);
        *ids += 1;
        let ins = self.new_rev(RevKind::Insert, *ids);
        let mark = |name: &str, r: &Revision, id: u64| {
            format!(
                "<w:{name} w:id=\"{id}\" w:author=\"{}\" w:date=\"{}\"/>",
                xml::escape(&r.author),
                r.date
            )
        };
        let del_mark = mark("del", &del, *ids + 1);
        let ins_mark = mark("ins", &ins, *ids + 2);
        *ids += 2;
        let strike = |p: &Paragraph| {
            let mut p = p.clone();
            let mut drop = Vec::new();
            let mut at = 0;
            for run in &mut p.runs {
                let r = at..at + run.len;
                at = r.end;
                if r.is_empty() || untracked(run) || deleted(run) {
                    continue;
                }
                if run
                    .rev
                    .as_ref()
                    .is_some_and(|v| v.kind == RevKind::Insert && v.author == me)
                {
                    drop.push(r);
                } else {
                    run.rev = Some(del.clone());
                }
            }
            for r in drop.into_iter().rev() {
                p.delete(r);
            }
            p.normalize();
            p
        };
        let insert = |mut p: Paragraph| {
            for run in &mut p.runs {
                if run.len > 0 && !untracked(run) && !deleted(run) {
                    run.rev = Some(ins.clone());
                }
            }
            p.normalize();
            p
        };
        // Old lines but the last (marks deleted), new lines but the last
        // (marks inserted), then the last old and new line sharing the
        // mark that was there: accepting leaves the new lines, rejecting
        // the old ones, without a spare empty line either way.
        let (Some((last_old, old_rest)), Some(last_new)) = (old.split_last(), new.last().cloned())
        else {
            return new;
        };
        // Markers (bookmarks, a content control's edges) are the new
        // version's; those leading its first line start the whole block.
        let mut new = new;
        let lead: Vec<Run> = {
            let first = &mut new[0];
            let n = first.runs.iter().take_while(|r| untracked(r)).count();
            let lead: Vec<Run> = first.runs[..n].to_vec();
            for _ in 0..n {
                first.remove_object(0);
            }
            lead
        };
        let unmark = |mut p: Paragraph| {
            let at: Vec<usize> = p
                .run_ranges()
                .filter(|(_, r)| untracked(r))
                .map(|(r, _)| r.start)
                .collect();
            for i in at.into_iter().rev() {
                p.remove_object(i);
            }
            p
        };
        let last_old = &unmark(last_old.clone());
        let last_new = if new.len() == 1 {
            new[0].clone()
        } else {
            last_new
        };
        let mut out = Vec::with_capacity(old.len() + new.len());
        for p in old_rest {
            let p = &unmark(p.clone());
            let mut p = strike(p);
            if mark_state(&p, &me).0 {
                // A line the user inserted (an earlier tracked update)
                // simply goes.
                strip_mark(&mut p, &["ins"]);
                if p.text.is_empty() {
                    continue;
                }
            }
            if !mark_state(&p, &me).1 {
                set_mark_rev(&mut p, &del_mark);
            }
            out.push(p);
        }
        let n = new.len();
        for p in new.into_iter().take(n - 1) {
            let mut p = insert(p);
            set_mark_rev(&mut p, &ins_mark);
            out.push(p);
        }
        let mut merged = strike(last_old);
        let tail = insert(last_new);
        let mut props = tail.props.clone();
        for x in last_old.props.extras.iter() {
            if crate::model::element_name(x) == "sectPr" {
                props.extras.remove("sectPr");
                props.extras.push(x);
            }
        }
        merged.append(tail);
        merged.props = props;
        out.push(merged);
        out[0].insert_runs(0, lead);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Position, Selection};

    #[test]
    fn lcs_finds_the_common_part() {
        let a: Vec<char> = "Page 3".chars().collect();
        let b: Vec<char> = "Page 12".chars().collect();
        let p = lcs(&a, &b).unwrap();
        assert_eq!(p.len(), 5);
    }

    #[test]
    fn a_changed_result_is_a_deletion_and_an_insertion() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("See page 3 now")]);
        d.set_author("Ani Wijaya");
        d.set_track_changes(true, Selection::default());
        let old = d.paragraphs().to_vec();
        let new = vec![Paragraph::new("See page 12 now")];
        let out = d.track_rewrite(&old, new);
        let mut t = d.clone();
        t.paragraphs = out;
        let ch: Vec<(RevKind, String)> =
            t.changes().into_iter().map(|c| (c.kind, c.text)).collect();
        assert_eq!(
            ch,
            [
                (RevKind::Delete, "3".into()),
                (RevKind::Insert, "12".into())
            ]
        );
        assert_eq!(t.paragraph(0).text, "See page 312 now");
        let mut acc = t.clone();
        acc.resolve_all(true, Selection::default());
        assert_eq!(acc.paragraph(0).text, "See page 12 now");
        t.resolve_all(false, Selection::default());
        assert_eq!(t.paragraph(0).text, "See page 3 now");
        let _ = Position::default();
    }

    #[test]
    fn a_longer_block_is_replaced_whole() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("one"), Paragraph::new("after")]);
        d.set_author("Ani Wijaya");
        d.set_track_changes(true, Selection::default());
        let old = d.paragraphs()[..1].to_vec();
        let new = vec![Paragraph::new("uno"), Paragraph::new("dos")];
        let out = d.track_rewrite(&old, new);
        let mut t = d.clone();
        t.paragraphs.splice(0..1, out);
        assert_eq!(t.len(), 3);
        let mut acc = t.clone();
        acc.resolve_all(true, Selection::default());
        let texts: Vec<_> = acc.paragraphs().iter().map(|p| p.text.clone()).collect();
        assert_eq!(texts, ["uno", "dos", "after"]);
        t.resolve_all(false, Selection::default());
        let texts: Vec<_> = t.paragraphs().iter().map(|p| p.text.clone()).collect();
        assert_eq!(texts, ["one", "after"]);
    }
}
