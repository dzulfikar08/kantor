//! Paragraph-level edits for programs driving Kantor (the MCP server):
//! insert, replace and delete whole paragraphs. Each call is one undo step
//! and leaves tables and pictures alone (it refuses rather than break them).

use std::ops::Range;

use crate::document::{Document, Kind};
use crate::model::{NumRef, Paragraph, Position, Selection};
use crate::numbering::ListKind;
use crate::object::OBJECT;
use crate::review::RevKind;
use crate::style::StyleKind;
use crate::track::{ins_tag, insert_tracked, set_mark_rev, strike, strip_mark};

/// A paragraph to insert.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NewParagraph {
    pub text: String,
    /// Paragraph style id (`Heading1`); see [`Document::find_style`].
    pub style: Option<String>,
    pub list: Option<ListKind>,
    /// List level, 0–8 (only with `list`).
    pub level: u8,
}

/// Text as a paragraph can hold it: no control characters but tabs, no
/// object placeholders. Line breaks are the caller's to split on.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|&c| c == '\t' || (!c.is_control() && c != OBJECT))
        .collect()
}

/// The lines of `text` (`\n`, `\r\n` or `\r` end a paragraph).
fn lines(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(clean)
        .collect()
}

impl Document {
    /// The id of the paragraph style called `name`: an id (`Heading1`) or a
    /// display name (`heading 1`), case-insensitive; `Normal` is `None`.
    pub fn find_style(&self, name: &str) -> Result<Option<String>, String> {
        let name = name.trim();
        if name.is_empty() || name.eq_ignore_ascii_case("normal") {
            return Ok(None);
        }
        let squash = |s: &str| s.replace(' ', "").to_ascii_lowercase();
        let wanted = squash(name);
        self.styles
            .styles
            .iter()
            .filter(|s| s.kind == StyleKind::Paragraph)
            .find(|s| squash(&s.id) == wanted || squash(&s.name) == wanted)
            .map(|s| Some(s.id.clone()))
            .ok_or_else(|| {
                format!(
                    "no paragraph style \"{name}\"; this document has: {}",
                    self.paragraph_style_names().join(", ")
                )
            })
    }

    /// Display names of the document's paragraph styles.
    pub fn paragraph_style_names(&self) -> Vec<String> {
        self.styles
            .styles
            .iter()
            .filter(|s| s.kind == StyleKind::Paragraph)
            .map(|s| self.styles.display_name(&s.id))
            .collect()
    }

    /// Whether a new body paragraph can go at index `at` (not inside a table).
    fn body_gap(&self, at: usize) -> bool {
        let before = at.checked_sub(1).and_then(|i| self.paragraphs.get(i));
        let after = self.paragraphs.get(at);
        match (before.and_then(|p| p.cell), after.and_then(|p| p.cell)) {
            (Some(a), Some(b)) => a.table != b.table,
            _ => true,
        }
    }

    /// Insert paragraphs after paragraph `after` (`None`: at the start;
    /// after a paragraph in a table: after that table). Text with line
    /// breaks becomes several paragraphs of the same kind. A blank new
    /// document's only paragraph is replaced. Returns the new paragraphs'
    /// indices and selects them for undo/redo; `sel` is restored on undo.
    ///
    /// With Track Changes on, the paragraphs go in as tracked insertions,
    /// their marks too, so rejecting removes them whole (a blank
    /// document's paragraph then stays, after them).
    pub fn insert_paragraphs(
        &mut self,
        after: Option<usize>,
        items: &[NewParagraph],
        sel: Selection,
    ) -> Result<Range<usize>, String> {
        self.insert_paragraphs_as(after, items, sel, self.track)
    }

    /// [`Document::insert_paragraphs`], never tracked (placeholders a
    /// program replaces right after).
    pub fn insert_paragraphs_untracked(
        &mut self,
        after: Option<usize>,
        items: &[NewParagraph],
        sel: Selection,
    ) -> Result<Range<usize>, String> {
        self.insert_paragraphs_as(after, items, sel, false)
    }

    fn insert_paragraphs_as(
        &mut self,
        after: Option<usize>,
        items: &[NewParagraph],
        sel: Selection,
        track: bool,
    ) -> Result<Range<usize>, String> {
        let mut at = match after {
            None => 0,
            Some(i) if i >= self.paragraphs.len() => {
                return Err(format!(
                    "paragraph {i} doesn't exist (the document has {})",
                    self.paragraphs.len()
                ));
            }
            Some(i) => match self.paragraphs[i].cell {
                Some(c) => self.table_range(c.table).end,
                None => i + 1,
            },
        };
        if !self.body_gap(at) {
            return Err("can't insert inside a table".into());
        }
        let blank = self.paragraphs.len() == 1
            && self.paragraphs[0].text.is_empty()
            && self.paragraphs[0].cell.is_none()
            && self.paragraphs[0].props.num.is_none();
        // Tracked, the blank paragraph stays for rejected ones to join.
        let replace = usize::from(blank && !track);
        if blank {
            at = 0;
        }

        // Lists continue the one just before them when it's the same kind.
        let mut prev_list = at.checked_sub(1).and_then(|i| {
            Some((
                self.list_kind(i)?,
                self.styles.effective_num(&self.paragraphs[i])?.id,
            ))
        });
        let mut new = Vec::new();
        for item in items {
            let num = match item.list {
                None => None,
                Some(kind) => {
                    let id = match prev_list {
                        Some((k, id)) if k == kind => id,
                        _ => self.numbering.new_list(kind),
                    };
                    prev_list = Some((kind, id));
                    Some(NumRef {
                        id,
                        level: item.level.min(8),
                    })
                }
            };
            if num.is_none() {
                prev_list = None;
            }
            for line in lines(&item.text) {
                let mut p = Paragraph::new(line);
                p.props.style = item.style.clone();
                p.props.num = num;
                new.push(p);
            }
        }
        if new.is_empty() {
            return Err("no paragraphs to insert".into());
        }
        let range = at..at + new.len();
        let last = new.last().map_or(0, |p| p.text.len());
        let after_sel = Selection {
            anchor: Position::new(range.start, 0),
            head: Position::new(range.end - 1, last),
        };
        let (mut first, mut count) = (at, replace);
        if track {
            let ins = self.new_rev(RevKind::Insert, self.max_rev_id() + 1);
            let mark = ins_tag(&ins);
            for p in &mut new {
                for run in &mut p.runs {
                    if run.object.is_none() && run.len > 0 {
                        run.rev = Some(ins.clone());
                    }
                }
                p.normalize();
                set_mark_rev(p, &mark);
            }
            // Rejected paragraphs join the one after them. At the end of the
            // document there's none: the paragraph before gets the inserted
            // mark instead and the last new one keeps a plain mark, as when
            // pressing Enter at the end in Word.
            if at == self.paragraphs.len()
                && let Some(prev) = at.checked_sub(1).map(|i| &self.paragraphs[i])
                && prev.cell.is_none()
            {
                let mut prev = prev.clone();
                set_mark_rev(&mut prev, &mark);
                if let Some(l) = new.last_mut() {
                    strip_mark(l, &["ins"]);
                }
                new.insert(0, prev);
                (first, count) = (at - 1, 1);
            }
        }
        self.replace(first, count, new, (sel, after_sel), Kind::Other, true);
        Ok(range)
    }

    /// Replace the text of paragraph `i`, keeping its style, list and the
    /// formatting of its first character. Line breaks in `text` add
    /// paragraphs of the same kind. Refuses paragraphs holding pictures or
    /// other objects. Returns the indices of the resulting paragraphs.
    ///
    /// With Track Changes on, the old text stays as a tracked deletion and
    /// the new text follows it as a tracked insertion (added paragraphs'
    /// marks inserted), so rejecting gives the paragraph back as it was.
    pub fn replace_paragraph_text(
        &mut self,
        i: usize,
        text: &str,
        sel: Selection,
    ) -> Result<Range<usize>, String> {
        let Some(p) = self.paragraphs.get(i) else {
            return Err(format!(
                "paragraph {i} doesn't exist (the document has {})",
                self.paragraphs.len()
            ));
        };
        if p.objects().any(|(_, o)| !o.is_marker()) {
            return Err(format!(
                "paragraph {i} holds a picture or another object Kantor would lose; edit its text with find_replace instead"
            ));
        }
        let props = p.props_at(0);
        let mut template = p.clone();
        template.delete(0..template.text.len());
        let new: Vec<Paragraph> = if self.track {
            let me = self.author.clone();
            let ids = self.max_rev_id();
            let del = self.new_rev(RevKind::Delete, ids + 1);
            let ins = self.new_rev(RevKind::Insert, ids + 2);
            let lines = lines(text);
            let mut out = Vec::with_capacity(lines.len());
            for (k, line) in lines.iter().enumerate() {
                let mut q = if k == 0 {
                    let mut q = p.clone();
                    let len = q.text.len();
                    strike(&mut q, 0, len, &del, &me);
                    q
                } else {
                    template.clone()
                };
                let end = q.text.len();
                insert_tracked(&mut q, end, line, &props, &ins);
                out.push(q);
            }
            // Every mark but the last (the paragraph's own) is new.
            let n = out.len();
            for q in &mut out[..n - 1] {
                set_mark_rev(q, &ins_tag(&ins));
            }
            out
        } else {
            lines(text)
                .into_iter()
                .map(|line| {
                    let mut q = template.clone();
                    q.insert(0, &line, props.clone());
                    q
                })
                .collect()
        };
        let range = i..i + new.len();
        let last = new.last().map_or(0, |p| p.text.len());
        let after_sel = Selection {
            anchor: Position::new(i, 0),
            head: Position::new(range.end - 1, last),
        };
        self.replace(i, 1, new, (sel, after_sel), Kind::Other, true);
        Ok(range)
    }

    /// Delete paragraphs `i..i + count` from the body. Refuses ranges that
    /// touch a table. Deleting everything leaves one empty paragraph.
    ///
    /// With Track Changes on, their text and paragraph marks are marked
    /// deleted instead (the user's own insertions in them go), so accepting
    /// removes the paragraphs and rejecting keeps them.
    pub fn delete_paragraphs(
        &mut self,
        i: usize,
        count: usize,
        sel: Selection,
    ) -> Result<(), String> {
        let end = i.saturating_add(count);
        if count == 0 || end > self.paragraphs.len() {
            return Err(format!(
                "paragraphs {i}..{end} don't exist (the document has {})",
                self.paragraphs.len()
            ));
        }
        if self.paragraphs[i..end].iter().any(|p| p.cell.is_some()) {
            return Err("those paragraphs include a table, which can't be deleted this way".into());
        }
        if self.track {
            // From the first paragraph's start to the next one's, or, at
            // the end of the document, from the end of the paragraph before.
            let n = self.paragraphs.len();
            let len = |k: usize| self.paragraphs[k].text.len();
            let (start, stop) = if end < n && self.paragraphs[end].cell.is_none() {
                (Position::new(i, 0), Position::new(end, 0))
            } else if i > 0 && self.paragraphs[i - 1].cell.is_none() {
                (
                    Position::new(i - 1, len(i - 1)),
                    Position::new(end - 1, len(end - 1)),
                )
            } else {
                (Position::new(i, 0), Position::new(end - 1, len(end - 1)))
            };
            if start == stop {
                return Ok(());
            }
            let mut ids = self.max_rev_id();
            let (new, _) = self.tracked_cut(start, stop, &mut ids);
            let caret = Selection::caret(Position::new(i, 0));
            let count = stop.para - start.para + 1;
            self.replace(start.para, count, new, (sel, caret), Kind::Other, true);
            return Ok(());
        }
        let new = if count == self.paragraphs.len() {
            vec![Paragraph::default()]
        } else {
            Vec::new()
        };
        let left = (self.paragraphs.len() - count).max(1);
        let caret = Selection::caret(Position::new(i.min(left - 1), 0));
        self.replace(i, count, new, (sel, caret), Kind::Other, true);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(doc: &Document) -> Vec<&str> {
        doc.paragraphs().iter().map(|p| p.text.as_str()).collect()
    }

    fn item(text: &str) -> NewParagraph {
        NewParagraph {
            text: text.into(),
            ..Default::default()
        }
    }

    const SEL: Selection = Selection::caret(Position::new(0, 0));

    #[test]
    fn inserts_into_a_blank_document_and_undoes_in_one_step() {
        let mut doc = Document::new();
        let heading = NewParagraph {
            style: doc.find_style("heading 1").unwrap(),
            ..item("Plan")
        };
        let r = doc
            .insert_paragraphs(Some(0), &[heading, item("One\nTwo")], SEL)
            .unwrap();
        assert_eq!(r, 0..3);
        assert_eq!(texts(&doc), ["Plan", "One", "Two"]);
        assert_eq!(doc.paragraph(0).props.style.as_deref(), Some("Heading1"));
        assert!(doc.undo().is_some());
        assert_eq!(texts(&doc), [""]);
        assert!(!doc.is_dirty());
    }

    #[test]
    fn inserts_lists_that_continue() {
        let mut doc = Document::from_paragraphs(vec![Paragraph::new("a"), Paragraph::new("b")]);
        let bullet = |t: &str, level| NewParagraph {
            list: Some(ListKind::Bullet),
            level,
            ..item(t)
        };
        doc.insert_paragraphs(Some(0), &[bullet("x", 0), bullet("y", 1)], SEL)
            .unwrap();
        assert_eq!(texts(&doc), ["a", "x", "y", "b"]);
        assert_eq!(doc.list_kind(1), Some(ListKind::Bullet));
        let (x, y) = (doc.paragraph(1).props.num, doc.paragraph(2).props.num);
        assert_eq!(x.unwrap().id, y.unwrap().id);
        assert_eq!(y.unwrap().level, 1);
        assert_eq!(doc.list_kind(3), None);
        assert!(doc.insert_paragraphs(Some(9), &[item("z")], SEL).is_err());
        assert!(doc.find_style("No Such Style").is_err());
    }

    #[test]
    fn replaces_and_deletes() {
        let mut doc = Document::from_paragraphs(vec![
            Paragraph::new("a").with_style("Heading1"),
            Paragraph::new("b"),
        ]);
        doc.replace_paragraph_text(0, "A\u{7}!\nA2", SEL).unwrap();
        assert_eq!(texts(&doc), ["A!", "A2", "b"]);
        assert_eq!(doc.paragraph(1).props.style.as_deref(), Some("Heading1"));
        doc.delete_paragraphs(1, 2, SEL).unwrap();
        assert_eq!(texts(&doc), ["A!"]);
        doc.delete_paragraphs(0, 1, SEL).unwrap();
        assert_eq!(texts(&doc), [""]);
        assert!(doc.delete_paragraphs(0, 2, SEL).is_err());
        doc.undo();
        doc.undo();
        doc.undo();
        assert_eq!(texts(&doc), ["a", "b"]);
    }

    fn tracked(paras: &[&str]) -> Document {
        let mut d = Document::from_paragraphs(paras.iter().map(|t| Paragraph::new(*t)).collect());
        d.set_author("AI");
        d.set_track_changes(true, SEL);
        d
    }

    /// The texts after accepting (or rejecting) every change.
    fn resolved(doc: &Document, accept: bool) -> Vec<String> {
        let mut d = doc.clone();
        d.resolve_all(accept, SEL);
        assert!(!d.has_changes());
        d.paragraphs().iter().map(|p| p.text.clone()).collect()
    }

    fn by_ai(doc: &Document) -> usize {
        let cs = doc.changes();
        assert!(cs.iter().all(|c| c.author == "AI"));
        cs.len()
    }

    #[test]
    fn tracked_replace_keeps_the_old_text_struck() {
        let mut doc = tracked(&["Judul", "Poin satu.", "Akhir"]);
        doc.replace_paragraph_text(1, "Diganti.", SEL).unwrap();
        assert_eq!(texts(&doc), ["Judul", "Poin satu.Diganti.", "Akhir"]);
        assert_eq!(by_ai(&doc), 2);
        assert_eq!(resolved(&doc, true), ["Judul", "Diganti.", "Akhir"]);
        assert_eq!(resolved(&doc, false), ["Judul", "Poin satu.", "Akhir"]);

        // Several lines: the added paragraphs' marks are inserted.
        let mut doc = tracked(&["a", "b"]);
        doc.paragraphs[0].props.style = Some("Heading1".into());
        let r = doc.replace_paragraph_text(0, "x\ny\nz", SEL).unwrap();
        assert_eq!(r, 0..3);
        assert_eq!(texts(&doc), ["ax", "y", "z", "b"]);
        assert_eq!(doc.paragraph(2).props.style.as_deref(), Some("Heading1"));
        assert_eq!(resolved(&doc, true), ["x", "y", "z", "b"]);
        assert_eq!(resolved(&doc, false), ["a", "b"]);
        // One undo step.
        doc.undo();
        assert_eq!(texts(&doc), ["a", "b"]);
    }

    #[test]
    fn tracked_inserts_go_whole_on_reject() {
        let numbered = |t: &str| NewParagraph {
            list: Some(ListKind::Numbered),
            ..item(t)
        };
        // In the middle.
        let mut doc = tracked(&["a", "b"]);
        let r = doc
            .insert_paragraphs(Some(0), &[numbered("x"), numbered("y")], SEL)
            .unwrap();
        assert_eq!(r, 1..3);
        assert_eq!(texts(&doc), ["a", "x", "y", "b"]);
        assert!(by_ai(&doc) >= 1);
        let num = doc.paragraph(1).props.num.unwrap().id;
        assert_eq!(doc.paragraph(2).props.num.unwrap().id, num);
        assert_eq!(resolved(&doc, true), ["a", "x", "y", "b"]);
        assert_eq!(resolved(&doc, false), ["a", "b"]);
        let mut back = doc.clone();
        back.resolve_all(false, SEL);
        assert_eq!(back.paragraph(1).props.num, None);

        // At the end of the document.
        let mut doc = tracked(&["a", "b"]);
        doc.insert_paragraphs(Some(1), &[numbered("x"), item("y")], SEL)
            .unwrap();
        assert_eq!(texts(&doc), ["a", "b", "x", "y"]);
        assert_eq!(doc.list_kind(2), Some(ListKind::Numbered));
        assert_eq!(resolved(&doc, true), ["a", "b", "x", "y"]);
        assert_eq!(resolved(&doc, false), ["a", "b"]);
        let mut back = doc.clone();
        back.resolve_all(false, SEL);
        assert_eq!(back.list_kind(1), None);

        // A blank document keeps its paragraph after the new ones.
        let mut doc = Document::new();
        doc.set_author("AI");
        doc.set_track_changes(true, SEL);
        doc.insert_paragraphs(None, &[item("x")], SEL).unwrap();
        assert_eq!(texts(&doc), ["x", ""]);
        assert_eq!(resolved(&doc, false), [""]);

        // Placeholders stay untracked.
        let mut doc = tracked(&["a"]);
        doc.insert_paragraphs_untracked(Some(0), &[item("m")], SEL)
            .unwrap();
        assert!(!doc.has_changes());
    }

    #[test]
    fn tracked_deletes_mark_text_and_marks() {
        // In the middle.
        let mut doc = tracked(&["a", "b", "c", "d"]);
        doc.delete_paragraphs(1, 2, SEL).unwrap();
        assert_eq!(texts(&doc), ["a", "b", "c", "d"]);
        assert!(by_ai(&doc) >= 1);
        assert_eq!(resolved(&doc, true), ["a", "d"]);
        assert_eq!(resolved(&doc, false), ["a", "b", "c", "d"]);

        // At the end.
        let mut doc = tracked(&["a", "b", "c"]);
        doc.delete_paragraphs(1, 2, SEL).unwrap();
        assert_eq!(resolved(&doc, true), ["a"]);
        assert_eq!(resolved(&doc, false), ["a", "b", "c"]);

        // Everything.
        let mut doc = tracked(&["a", "b"]);
        doc.delete_paragraphs(0, 2, SEL).unwrap();
        assert_eq!(resolved(&doc, true), [""]);
        assert_eq!(resolved(&doc, false), ["a", "b"]);

        // The author's own insertion just goes.
        let mut doc = tracked(&["a", "b"]);
        doc.insert_paragraphs(Some(0), &[item("x")], SEL).unwrap();
        doc.delete_paragraphs(1, 1, SEL).unwrap();
        assert_eq!(texts(&doc), ["a", "b"]);
        assert!(!doc.has_changes());
    }
}
