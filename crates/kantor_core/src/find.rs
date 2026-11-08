//! Find & Replace matching, shared by Docs, Sheets and Slides.
//!
//! Matches are byte ranges into the searched text, always on `char`
//! boundaries. Case-insensitive matching compares the lowercase expansion
//! of each character (so `İ`, whose lowercase is two characters, is safe),
//! and a match never ends in the middle of a character's expansion.

use std::cmp::Ordering;
use std::ops::Range;

/// How to match.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FindOptions {
    pub match_case: bool,
    /// The match must not touch a letter, digit or `_` on either side.
    pub whole_word: bool,
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn fold(c: char, match_case: bool, out: &mut Vec<char>) {
    if match_case {
        out.push(c);
    } else {
        out.extend(c.to_lowercase());
    }
}

/// If `needle` (already folded) matches `text` starting at byte `start`,
/// the byte where the match ends.
fn match_at(text: &str, start: usize, needle: &[char], match_case: bool) -> Option<usize> {
    let mut i = 0;
    let mut buf = Vec::with_capacity(2);
    for (off, c) in text[start..].char_indices() {
        buf.clear();
        fold(c, match_case, &mut buf);
        for &f in &buf {
            if needle.get(i) != Some(&f) {
                return None;
            }
            i += 1;
        }
        if i == needle.len() {
            return Some(start + off + c.len_utf8());
        }
    }
    None
}

/// Every non-overlapping match of `needle` in `text`, left to right.
/// An empty needle matches nothing.
pub fn find_all(text: &str, needle: &str, opts: FindOptions) -> Vec<Range<usize>> {
    let mut folded = Vec::with_capacity(needle.len());
    for c in needle.chars() {
        fold(c, opts.match_case, &mut folded);
    }
    let mut out = Vec::new();
    if folded.is_empty() {
        return out;
    }
    let mut from = 0;
    let mut prev: Option<char> = None;
    for (start, c) in text.char_indices() {
        if start >= from
            && let Some(end) = match_at(text, start, &folded, opts.match_case)
            && (!opts.whole_word || at_word_edges(text, start..end, prev))
        {
            out.push(start..end);
            from = end;
        }
        prev = Some(c);
    }
    out
}

/// Whole word: the match doesn't continue a word on either side. A match
/// that starts or ends with punctuation needs no edge on that side.
fn at_word_edges(text: &str, m: Range<usize>, before: Option<char>) -> bool {
    let found = &text[m.clone()];
    let after = text[m.end..].chars().next();
    let first = found.chars().next().is_some_and(is_word_char);
    let last = found.chars().next_back().is_some_and(is_word_char);
    !(first && before.is_some_and(is_word_char)) && !(last && after.is_some_and(is_word_char))
}

/// Whether `needle` occurs in `text` at all.
pub fn contains(text: &str, needle: &str, opts: FindOptions) -> bool {
    !find_all(text, needle, opts).is_empty()
}

/// `text` with every match replaced; also returns how many there were.
pub fn replace_all(
    text: &str,
    needle: &str,
    replacement: &str,
    opts: FindOptions,
) -> (String, usize) {
    let matches = find_all(text, needle, opts);
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for m in &matches {
        out.push_str(&text[last..m.start]);
        out.push_str(replacement);
        last = m.end;
    }
    out.push_str(&text[last..]);
    (out, matches.len())
}

/// The match to go to from `at` (a position in the same order as `keys`,
/// which are sorted): forward, the first match at or after `at`
/// (strictly after when `skip_at` is set); backward, the last one before
/// `at`. Wraps around; `None` when there are no matches.
pub fn pick<K: Ord>(keys: &[K], at: &K, forward: bool, skip_at: bool) -> Option<usize> {
    if keys.is_empty() {
        return None;
    }
    let ix = if forward {
        keys.iter()
            .position(|k| match k.cmp(at) {
                Ordering::Greater => true,
                Ordering::Equal => !skip_at,
                Ordering::Less => false,
            })
            .unwrap_or(0)
    } else {
        keys.iter().rposition(|k| k < at).unwrap_or(keys.len() - 1)
    };
    Some(ix)
}

/// The next (or previous) index in a list of `len` matches, wrapping.
pub fn step(current: Option<usize>, len: usize, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match current {
        None if forward => 0,
        None => len - 1,
        Some(i) if forward => (i + 1) % len,
        Some(i) => (i + len - 1) % len,
    })
}

/// `3 of 12`, `12 matches` (none selected yet) or `No results`.
pub fn count_label(current: Option<usize>, total: usize) -> String {
    match (current, total) {
        (_, 0) => "No results".to_owned(),
        (Some(i), n) if i < n => format!("{} of {n}", i + 1),
        (_, 1) => "1 match".to_owned(),
        (_, n) => format!("{n} matches"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ANY: FindOptions = FindOptions {
        match_case: false,
        whole_word: false,
    };
    const CASE: FindOptions = FindOptions {
        match_case: true,
        whole_word: false,
    };
    const WORD: FindOptions = FindOptions {
        match_case: false,
        whole_word: true,
    };

    #[test]
    fn finds_every_match_ignoring_case() {
        assert_eq!(find_all("Cat cat CAT", "cat", ANY), vec![0..3, 4..7, 8..11]);
        assert_eq!(find_all("Cat cat CAT", "cat", CASE), vec![4..7]);
        assert!(find_all("abc", "", ANY).is_empty());
        assert!(find_all("", "a", ANY).is_empty());
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(find_all("aaaa", "aa", ANY), vec![0..2, 2..4]);
    }

    #[test]
    fn whole_word_needs_word_edges() {
        assert_eq!(
            find_all("cat concat cat_x cat.", "cat", WORD),
            vec![0..3, 17..20]
        );
        // Edges that are punctuation in the needle itself need no boundary.
        assert_eq!(find_all("x.y", ".", WORD), vec![1..2]);
        // A whole-word miss at one spot doesn't hide a later hit.
        assert_eq!(find_all("cats cat", "cat", WORD), vec![5..8]);
    }

    #[test]
    fn unicode_ranges_stay_on_char_boundaries() {
        let text = "Ünïcode ünïcode";
        let m = find_all(text, "ÜNÏ", ANY);
        assert_eq!(m.len(), 2);
        for r in &m {
            assert!(text.is_char_boundary(r.start) && text.is_char_boundary(r.end));
            assert_eq!(text[r.clone()].to_lowercase(), "ünï");
        }
        // `İ` lowercases to two chars: "i̇". Searching "i" must not end
        // inside that expansion.
        assert!(find_all("İ", "i", ANY).is_empty());
        assert_eq!(find_all("İx", "i\u{307}x", ANY), vec![0..3]);
    }

    #[test]
    fn replace_all_counts_and_rebuilds() {
        assert_eq!(
            replace_all("a-b-a", "a", "xy", ANY),
            ("xy-b-xy".to_owned(), 2)
        );
        assert_eq!(replace_all("abc", "z", "y", ANY), ("abc".to_owned(), 0));
        assert!(contains("Hello", "LL", ANY));
        assert!(!contains("Hello", "LL", CASE));
    }

    #[test]
    fn pick_and_step_wrap_around() {
        let keys = [2, 5, 9];
        assert_eq!(pick(&keys, &5, true, false), Some(1));
        assert_eq!(pick(&keys, &5, true, true), Some(2));
        assert_eq!(pick(&keys, &10, true, false), Some(0));
        assert_eq!(pick(&keys, &5, false, false), Some(0));
        assert_eq!(pick(&keys, &1, false, false), Some(2));
        assert_eq!(pick::<i32>(&[], &1, true, false), None);
        assert_eq!(step(Some(2), 3, true), Some(0));
        assert_eq!(step(Some(0), 3, false), Some(2));
        assert_eq!(step(None, 3, false), Some(2));
        assert_eq!(step(None, 0, true), None);
    }

    #[test]
    fn count_labels() {
        assert_eq!(count_label(Some(2), 12), "3 of 12");
        assert_eq!(count_label(None, 12), "12 matches");
        assert_eq!(count_label(None, 1), "1 match");
        assert_eq!(count_label(Some(0), 0), "No results");
    }
}
