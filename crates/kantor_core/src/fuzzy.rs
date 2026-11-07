//! Fuzzy matching for the command palette.
//!
//! Scoring follows Voltrus Tools' `search.rs`: every query character must appear
//! in order (case-insensitive). Matches at the start of the text score +6, at the
//! start of a word +8, right after the previous match +5, and each skipped
//! character costs 1.

/// Score `text` against `query`; `None` if it doesn't match. Empty queries match everything.
pub fn score(query: &str, text: &str) -> Option<i32> {
    let query: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if query.is_empty() {
        return Some(0);
    }
    let chars: Vec<char> = text.chars().collect();
    let mut qi = 0;
    let mut total = 0;
    let mut last: Option<usize> = None;
    for (i, &c) in chars.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if c.to_lowercase().eq(std::iter::once(query[qi])) {
            let mut s = 1;
            if i == 0 {
                s += 6;
            } else if !chars[i - 1].is_alphanumeric() {
                s += 8;
            }
            match last {
                Some(l) if l + 1 == i => s += 5,
                Some(l) => s -= (i - l - 1).min(10) as i32,
                None => {}
            }
            total += s;
            last = Some(i);
            qi += 1;
        }
    }
    (qi == query.len()).then_some(total)
}

/// Indices of `items` matching `query`, best first (ties keep the original order).
pub fn rank<'a>(query: &str, items: impl IntoIterator<Item = &'a str>) -> Vec<usize> {
    let mut scored: Vec<(usize, i32)> = items
        .into_iter()
        .enumerate()
        .filter_map(|(i, t)| score(query, t).map(|s| (i, s)))
        .collect();
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_in_order_case_insensitive() {
        assert!(score("nsp", "New Spreadsheet").is_some());
        assert!(score("NEW", "new spreadsheet").is_some());
        assert!(score("psn", "New Spreadsheet").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn prefers_word_starts_and_runs() {
        let word_start = score("sa", "Save As").unwrap();
        let scattered = score("sa", "Close Tab").unwrap();
        assert!(word_start > scattered);
        let run = score("open", "Open File").unwrap();
        let spread = score("open", "Go Previous Tab Next").unwrap_or(i32::MIN);
        assert!(run > spread);
    }

    #[test]
    fn ranks_best_first() {
        let items = ["Close Tab", "Save", "Save As", "New Spreadsheet"];
        let r = rank("save", items);
        assert_eq!(r[0], 1);
        assert_eq!(r[1], 2);
        assert_eq!(r.len(), 2);
        assert_eq!(rank("", items), vec![0, 1, 2, 3]);
    }
}
