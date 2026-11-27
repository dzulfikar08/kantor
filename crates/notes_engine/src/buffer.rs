//! The note's text, exactly as it is on disk.
//!
//! The buffer holds the file's bytes as text (UTF-8, a byte-order mark set
//! aside) and never rewrites them: saving an unedited note writes the same
//! bytes back. Line endings stay what they are; a new line typed or pasted
//! gets the note's own ending ([`Eol`], the first one found).
//!
//! A *line* is the text between two `\n`s; its content excludes the ending
//! (`\n`, or `\r\n`). Offsets are byte offsets into [`Buffer::text`] and are
//! always kept on character boundaries and never between a `\r` and its `\n`.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation as _;

/// The line ending new lines get.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Eol {
    #[default]
    Lf,
    CrLf,
}

impl Eol {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
        }
    }
}

const BOM: &str = "\u{feff}";

/// Why a file can't be opened as a note.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// Not UTF-8 text (an offset of the first bad byte).
    NotUtf8(usize),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotUtf8(at) => write!(f, "not UTF-8 text (byte {at})"),
        }
    }
}

impl std::error::Error for OpenError {}

#[derive(Clone, Debug, Default)]
pub struct Buffer {
    text: String,
    /// The file started with a UTF-8 byte-order mark (kept out of `text`,
    /// written back on save).
    bom: bool,
    eol: Eol,
    /// Byte offset where each line starts; `line_starts[0] == 0`.
    line_starts: Vec<usize>,
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        let mut b = Self {
            text: text.to_owned(),
            bom: false,
            eol: Eol::Lf,
            line_starts: Vec::new(),
        };
        b.eol = detect_eol(&b.text).unwrap_or_default();
        b.reindex();
        b
    }

    /// A buffer for a file's bytes.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, OpenError> {
        let text = std::str::from_utf8(bytes).map_err(|e| OpenError::NotUtf8(e.valid_up_to()))?;
        let (bom, text) = match text.strip_prefix(BOM) {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let mut b = Self::new(text);
        b.bom = bom;
        Ok(b)
    }

    /// The bytes to write: the text, with the byte-order mark it was read with.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.text.len() + 3);
        if self.bom {
            out.extend_from_slice(BOM.as_bytes());
        }
        out.extend_from_slice(self.text.as_bytes());
        out
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn eol(&self) -> Eol {
        self.eol
    }

    pub fn has_bom(&self) -> bool {
        self.bom
    }

    pub fn slice(&self, r: Range<usize>) -> &str {
        &self.text[r]
    }

    fn reindex(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);
        self.line_starts.extend(
            self.text
                .bytes()
                .enumerate()
                .filter(|&(_, b)| b == b'\n')
                .map(|(i, _)| i + 1),
        );
    }

    /// Number of lines (an empty text, or one ending in a line break, has
    /// an empty last line).
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// The line holding `offset`.
    pub fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts[line.min(self.line_starts.len() - 1)]
    }

    /// Line `line`'s content, without its ending.
    pub fn line_range(&self, line: usize) -> Range<usize> {
        let start = self.line_start(line);
        let end = match self.line_starts.get(line + 1) {
            Some(&next) => {
                let mut e = next - 1;
                if e > start && self.text.as_bytes()[e - 1] == b'\r' {
                    e -= 1;
                }
                e
            }
            None => self.text.len(),
        };
        start..end
    }

    /// Line `line` with its ending.
    pub fn line_full_range(&self, line: usize) -> Range<usize> {
        let start = self.line_start(line);
        let end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.text.len());
        start..end
    }

    pub fn line(&self, line: usize) -> &str {
        &self.text[self.line_range(line)]
    }

    /// The nearest valid caret offset at or before `offset`: on a character
    /// boundary, not inside a `\r\n`.
    pub fn clamp(&self, offset: usize) -> usize {
        let mut o = offset.min(self.text.len());
        while !self.text.is_char_boundary(o) {
            o -= 1;
        }
        if o > 0 && o < self.text.len() && &self.text[o - 1..=o] == "\r\n" {
            o -= 1;
        }
        o
    }

    /// Replace `range` with `text` as is (see [`Self::normalize`] for text
    /// with line breaks). Returns the replaced text.
    pub fn replace(&mut self, range: Range<usize>, text: &str) -> String {
        let old = self.text[range.clone()].to_owned();
        self.text.replace_range(range, text);
        self.reindex();
        old
    }

    /// `text` with every line break turned into the note's own.
    pub fn normalize(&self, text: &str) -> String {
        normalize_eol(text, self.eol)
    }

    /// The offset one grapheme before `offset` (a `\r\n` is one step).
    pub fn prev_grapheme(&self, offset: usize) -> usize {
        let line = self.line_of(offset);
        let start = self.line_start(line);
        if offset == start {
            // To the end of the previous line's content.
            return if line == 0 {
                0
            } else {
                self.line_range(line - 1).end
            };
        }
        let s = &self.text[start..offset];
        s.grapheme_indices(true)
            .next_back()
            .map_or(start, |(i, _)| start + i)
    }

    /// The offset one grapheme after `offset`.
    pub fn next_grapheme(&self, offset: usize) -> usize {
        let line = self.line_of(offset);
        let r = self.line_range(line);
        if offset >= r.end {
            return if line + 1 < self.line_count() {
                self.line_start(line + 1)
            } else {
                self.text.len()
            };
        }
        let s = &self.text[offset..r.end];
        s.graphemes(true).next().map_or(r.end, |g| offset + g.len())
    }

    /// The start of the word before `offset` (skipping spaces and
    /// punctuation first), or the previous line's end at a line start.
    pub fn prev_word(&self, offset: usize) -> usize {
        let line = self.line_of(offset);
        let start = self.line_start(line);
        if offset == start {
            return self.prev_grapheme(offset);
        }
        let s = &self.text[start..offset];
        let mut last = 0;
        for (i, w) in s.split_word_bound_indices() {
            if w.chars().any(char::is_alphanumeric) {
                last = i;
            }
        }
        // Only spaces/punctuation before: go to the line start.
        start + last
    }

    /// The end of the word after `offset`, or the next line's start at a
    /// line end.
    pub fn next_word(&self, offset: usize) -> usize {
        let line = self.line_of(offset);
        let r = self.line_range(line);
        if offset >= r.end {
            return self.next_grapheme(offset);
        }
        let s = &self.text[offset..r.end];
        for (i, w) in s.split_word_bound_indices() {
            if w.chars().any(char::is_alphanumeric) {
                return offset + i + w.len();
            }
        }
        r.end
    }

    /// The word around `offset` (double-click), or the grapheme there.
    pub fn word_at(&self, offset: usize) -> Range<usize> {
        let line = self.line_of(offset);
        let r = self.line_range(line);
        let s = &self.text[r.clone()];
        let at = offset - r.start;
        for (i, w) in s.split_word_bound_indices() {
            if at >= i && at < i + w.len() || at == s.len() && i + w.len() == s.len() {
                return r.start + i..r.start + i + w.len();
            }
        }
        offset..offset
    }
}

/// The first line ending in `text`, if any.
pub fn detect_eol(text: &str) -> Option<Eol> {
    let i = text.find('\n')?;
    Some(if i > 0 && text.as_bytes()[i - 1] == b'\r' {
        Eol::CrLf
    } else {
        Eol::Lf
    })
}

/// `text` with `\r\n`, `\r` and `\n` all turned into `eol`.
pub fn normalize_eol(text: &str, eol: Eol) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                out.push_str(eol.as_str());
            }
            '\n' => out.push_str(eol.as_str()),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_round_trip_exactly() {
        for sample in [
            "",
            "a",
            "a\n",
            "a\r\nb\r\n",
            "\u{feff}# Title\n\nbody",
            "mixed\r\nends\nhere\r\n",
            "no newline at end",
        ] {
            let b = Buffer::from_bytes(sample.as_bytes()).unwrap();
            assert_eq!(b.to_bytes(), sample.as_bytes(), "{sample:?}");
        }
        assert!(Buffer::from_bytes(b"\xff\xfe").is_err());
    }

    #[test]
    fn lines_exclude_endings() {
        let b = Buffer::new("one\r\ntwo\n\nlast");
        assert_eq!(b.eol(), Eol::CrLf);
        assert_eq!(b.line_count(), 4);
        assert_eq!(b.line(0), "one");
        assert_eq!(b.line(1), "two");
        assert_eq!(b.line(2), "");
        assert_eq!(b.line(3), "last");
        assert_eq!(b.line_of(4), 0, "the \\n of a \\r\\n belongs to its line");
        assert_eq!(b.line_of(5), 1);
        let b = Buffer::new("x\n");
        assert_eq!(b.line_count(), 2);
        assert_eq!(b.line(1), "");
    }

    #[test]
    fn clamp_avoids_crlf_and_char_middles() {
        let b = Buffer::new("é\r\nb");
        assert_eq!(b.clamp(1), 0);
        assert_eq!(b.clamp(3), 2, "between \\r and \\n");
        assert_eq!(b.clamp(99), b.len());
    }

    #[test]
    fn grapheme_steps_cross_line_endings_as_one() {
        let b = Buffer::new("ab\r\ncd");
        assert_eq!(b.next_grapheme(2), 4);
        assert_eq!(b.prev_grapheme(4), 2);
        assert_eq!(b.next_grapheme(0), 1);
        let b = Buffer::new("e\u{301}x");
        assert_eq!(b.next_grapheme(0), 3);
    }

    #[test]
    fn words() {
        let b = Buffer::new("hello, big world");
        assert_eq!(b.next_word(0), 5);
        assert_eq!(b.next_word(5), 10);
        assert_eq!(b.prev_word(16), 11);
        assert_eq!(b.prev_word(11), 7);
        assert_eq!(b.word_at(8), 7..10);
    }

    #[test]
    fn normalizes_pasted_breaks() {
        assert_eq!(normalize_eol("a\r\nb\nc\rd", Eol::CrLf), "a\r\nb\r\nc\r\nd");
        assert_eq!(normalize_eol("a\r\nb", Eol::Lf), "a\nb");
    }
}
