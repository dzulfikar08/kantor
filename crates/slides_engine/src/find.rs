//! Find & Replace support: which shapes' text can be searched, and
//! replacing many matches across slides as one undo step. Matching itself
//! is the UI's (`kantor_core::find`).

use std::ops::Range;

use anyhow::Context as _;
use docs_engine::{Position, Selection};

use crate::edit::ShapeRef;
use crate::history::{Focus, Kind};
use crate::presentation::Presentation;
use crate::text_ops;

/// A match: a byte range in one paragraph of one shape's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextMatch {
    pub shape: ShapeRef,
    pub para: usize,
    pub range: Range<usize>,
}

impl TextMatch {
    /// Sort key: reading order.
    pub fn key(&self) -> (usize, usize, usize, usize) {
        (self.shape.0, self.shape.1, self.para, self.range.start)
    }

    /// The match as a text selection in its shape.
    pub fn selection(&self) -> Selection {
        Selection {
            anchor: Position::new(self.para, self.range.start),
            head: Position::new(self.para, self.range.end),
        }
    }
}

impl Presentation {
    /// Every shape with text, in reading order (slide by slide, then the
    /// shapes' z-order), with its paragraphs' text.
    pub fn searchable_text(&self) -> Vec<(ShapeRef, Vec<&str>)> {
        let mut out = Vec::new();
        for s in 0..self.len() {
            for (i, shape) in self.slide(s).shapes.iter().enumerate() {
                if let Some(body) = &shape.text {
                    out.push(((s, i), body.paras.iter().map(|p| p.text.as_str()).collect()));
                }
            }
        }
        out
    }

    /// Whether the text of shape `r` can be changed (not locked).
    pub fn text_replaceable(&self, r: ShapeRef) -> bool {
        r.0 < self.len()
            && self.slide(r.0).locked.is_none()
            && self
                .slide(r.0)
                .shapes
                .get(r.1)
                .is_some_and(|s| s.text_editable())
    }

    /// Replace every match with `with` as one undo step. Matches in text
    /// that can't be edited are skipped; `matches` must be sorted and must
    /// not overlap. Each replacement takes the formatting of the first
    /// character it replaces. Returns how many were replaced.
    pub fn replace_matches(
        &mut self,
        matches: &[TextMatch],
        with: &str,
        before: Focus,
    ) -> anyhow::Result<usize> {
        let todo: Vec<&TextMatch> = matches
            .iter()
            .filter(|m| self.text_replaceable(m.shape))
            .collect();
        let Some(last) = todo.last() else {
            return Ok(0);
        };
        let after = Focus::shape(last.shape.0, last.shape.1);
        self.record(Kind::Other, None, before, true, |p| {
            // Back to front, so earlier ranges in the same paragraph stay valid.
            for m in todo.iter().rev() {
                let slide = p.slide_mut(m.shape.0)?;
                let shape = slide.shapes.get_mut(m.shape.1).context("no such shape")?;
                let body = shape.text.as_mut().context("This shape has no text")?;
                let Some(para) = body.paras.get(m.para) else {
                    continue;
                };
                let text = &para.text;
                if m.range.end > text.len()
                    || !text.is_char_boundary(m.range.start)
                    || !text.is_char_boundary(m.range.end)
                {
                    continue;
                }
                let first = text[m.range.start..]
                    .chars()
                    .next()
                    .map_or(0, char::len_utf8)
                    .min(m.range.len());
                let props = para.props_at(m.range.start + first);
                text_ops::insert_text(&mut body.paras, m.selection(), with, Some(props));
            }
            Ok(((), after))
        })?;
        Ok(todo.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ShapeKind;

    fn text_shape(p: &Presentation) -> Option<ShapeRef> {
        p.slide(0)
            .shapes
            .iter()
            .position(|s| matches!(s.kind, ShapeKind::Shape { .. }) && s.text.is_some())
            .map(|i| (0, i))
    }

    #[test]
    fn replace_matches_is_one_undo_step() {
        let mut p = Presentation::new();
        let r = text_shape(&p).expect("a placeholder with text");
        p.insert_text(r, Selection::default(), "cat and cat", None)
            .unwrap();
        p.seal_history();
        let found: Vec<TextMatch> = p
            .searchable_text()
            .into_iter()
            .filter(|(s, _)| *s == r)
            .flat_map(|(s, paras)| {
                paras[0]
                    .match_indices("cat")
                    .map(|(i, _)| TextMatch {
                        shape: s,
                        para: 0,
                        range: i..i + 3,
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(found.len(), 2);
        let n = p.replace_matches(&found, "dog", Focus::slide(0)).unwrap();
        assert_eq!(n, 2);
        assert_eq!(p.text_paras(r).unwrap()[0].text, "dog and dog");
        p.undo();
        assert_eq!(p.text_paras(r).unwrap()[0].text, "cat and cat");
        p.redo();
        assert_eq!(p.text_paras(r).unwrap()[0].text, "dog and dog");
    }
}
