//! Find & Replace support: replacing many matches as one undo step, and
//! word / character counts of a range. Matching itself is the UI's
//! (`kantor_core::find`); matches are byte ranges inside one paragraph.

use std::ops::Range;

use crate::document::{Document, Kind};
use crate::model::{Position, Selection};
use crate::object::OBJECT;

/// Words and characters of `text` the way the status bar counts them:
/// whitespace-separated words, characters without object placeholders and
/// line breaks between paragraphs.
pub fn text_stats(text: &str) -> (usize, usize) {
    let words = text
        .split_whitespace()
        .filter(|w| w.chars().any(|c| c != OBJECT))
        .count();
    let chars = text.chars().filter(|&c| c != OBJECT && c != '\n').count();
    (words, chars)
}

impl Document {
    /// Words and characters between `start` and `end`.
    pub fn stats_in(&self, start: Position, end: Position) -> (usize, usize) {
        text_stats(&self.text_in(start, end))
    }

    /// Replace each `(paragraph, byte range)` with `with`, as one undo step.
    /// `ranges` must be sorted and must not overlap. Each replacement takes
    /// the formatting of the first character it replaces. `sel` is restored
    /// on undo; returns the caret after the last replacement.
    pub fn replace_ranges(
        &mut self,
        ranges: &[(usize, Range<usize>)],
        with: &str,
        sel: Selection,
    ) -> Selection {
        let valid: Vec<&(usize, Range<usize>)> = ranges
            .iter()
            .filter(|(para, r)| {
                self.paragraphs.get(*para).is_some_and(|p| {
                    r.start <= r.end
                        && r.end <= p.text.len()
                        && p.text.is_char_boundary(r.start)
                        && p.text.is_char_boundary(r.end)
                })
            })
            .collect();
        let (Some(first), Some(last)) = (valid.first(), valid.last()) else {
            return sel;
        };
        let (first, last) = (first.0, last.0);
        let mut out: Vec<_> = self.paragraphs[first..=last].to_vec();
        let mut caret = Position::new(last, 0);
        for para in first..=last {
            let mine: Vec<&Range<usize>> = valid
                .iter()
                .filter(|(p, _)| *p == para)
                .map(|(_, r)| r)
                .collect();
            let p = &mut out[para - first];
            // Back to front, so earlier ranges stay valid.
            for r in mine.iter().rev() {
                let first_char = p.text[r.start..].chars().next().map_or(0, char::len_utf8);
                let props = p.props_at(r.start + first_char.min(r.len()));
                p.delete((*r).clone());
                p.insert(r.start, with, props);
            }
            if para == last
                && let Some(r) = mine.last()
            {
                let shift: isize = mine[..mine.len() - 1]
                    .iter()
                    .map(|r| with.len() as isize - r.len() as isize)
                    .sum();
                caret.offset = (r.start as isize + shift) as usize + with.len();
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Paragraph;

    #[test]
    fn stats_count_words_and_chars() {
        assert_eq!(text_stats("one two  three"), (3, 14));
        assert_eq!(text_stats("a\nb"), (2, 2));
        assert_eq!(text_stats(""), (0, 0));
        let mut d = Document::from_paragraphs(vec![Paragraph::new("Hello big world")]);
        assert_eq!(
            d.stats_in(Position::new(0, 6), Position::new(0, 15)),
            (2, 9)
        );
        d.insert_text(Selection::caret(Position::new(0, 0)), "x", None);
        assert_eq!(d.word_count(), 3);
    }

    #[test]
    fn replace_ranges_is_one_undo_step() {
        let mut d = Document::from_paragraphs(vec![
            Paragraph::new("cat and cat"),
            Paragraph::new("no match"),
            Paragraph::new("a cat"),
        ]);
        let sel = Selection::caret(Position::new(1, 0));
        let after = d.replace_ranges(&[(0, 0..3), (0, 8..11), (2, 2..5)], "dog", sel);
        let texts: Vec<_> = d.paragraphs().iter().map(|p| p.text.clone()).collect();
        assert_eq!(texts, ["dog and dog", "no match", "a dog"]);
        assert_eq!(after.head, Position::new(2, 5));
        assert_eq!(d.undo(), Some(sel));
        assert_eq!(d.paragraph(0).text, "cat and cat");
        assert_eq!(d.paragraph(2).text, "a cat");
        assert!(!d.can_undo());
    }

    #[test]
    fn replacement_length_shifts_the_caret() {
        let mut d = Document::from_paragraphs(vec![Paragraph::new("ab ab ab")]);
        let after = d.replace_ranges(
            &[(0, 0..2), (0, 3..5), (0, 6..8)],
            "x",
            Selection::default(),
        );
        assert_eq!(d.paragraph(0).text, "x x x");
        assert_eq!(after.head, Position::new(0, 5));
        // Bad ranges are ignored.
        let s = d.replace_ranges(&[(9, 0..1)], "y", Selection::default());
        assert_eq!(s, Selection::default());
    }
}
