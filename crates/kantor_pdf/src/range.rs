//! Page-range syntax shared by every tool: `1-3,5,8-`.
//!
//! Pages are numbered from 1. Items are separated by commas:
//!
//! | item   | pages                          |
//! |--------|--------------------------------|
//! | `5`    | page 5                         |
//! | `1-3`  | pages 1, 2, 3                  |
//! | `8-`   | page 8 to the last page        |
//! | `-3`   | page 1 to page 3               |
//! | `5-3`  | pages 5, 4, 3 (reversed)       |
//! | `last` | the last page; `5-last`, `last-1` (all, reversed) |
//! | `all`  | every page                     |
//!
//! Parsing checks the syntax only; [`PageRanges::resolve`] checks the pages
//! against a document's page count.

use std::fmt;

/// A page number in a range item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bound {
    Page(u32),
    Last,
}

impl Bound {
    fn resolve(self, total: u32) -> u32 {
        match self {
            Bound::Page(p) => p,
            Bound::Last => total,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Item {
    text: String,
    start: Bound,
    /// `None` for an open end (`8-`).
    end: Option<Bound>,
    single: bool,
}

/// A parsed page selection such as `1-3,5,8-`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRanges {
    items: Vec<Item>,
}

/// A page range that is malformed or does not fit the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeError(String);

impl fmt::Display for RangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RangeError {}

fn parse_bound(s: &str, item: &str) -> Result<Bound, RangeError> {
    let s = s.trim();
    if s == "last" {
        return Ok(Bound::Last);
    }
    match s.parse::<u32>() {
        Ok(0) => Err(RangeError(format!(
            "invalid page range item \"{item}\": pages are numbered from 1"
        ))),
        Ok(n) => Ok(Bound::Page(n)),
        Err(_) => Err(RangeError(format!(
            "invalid page range item \"{item}\": \"{s}\" is not a page number"
        ))),
    }
}

fn parse_item(text: &str) -> Result<Item, RangeError> {
    let item = text.trim();
    let compact: String = item.chars().filter(|c| !c.is_whitespace()).collect();
    let lower = compact.to_ascii_lowercase();
    if lower == "all" {
        return Ok(Item {
            text: compact,
            start: Bound::Page(1),
            end: None,
            single: false,
        });
    }
    let (start, end, single) = match lower.split_once('-') {
        None => {
            let b = parse_bound(&lower, item)?;
            (b, Some(b), true)
        }
        Some((a, b)) => {
            if b.contains('-') {
                return Err(RangeError(format!(
                    "invalid page range item \"{item}\": use one '-' (like 2-5)"
                )));
            }
            let start = if a.is_empty() {
                Bound::Page(1)
            } else {
                parse_bound(a, item)?
            };
            let end = if b.is_empty() {
                if a.is_empty() {
                    return Err(RangeError(format!(
                        "invalid page range item \"{item}\": give a page before or after '-'"
                    )));
                }
                None
            } else {
                Some(parse_bound(b, item)?)
            };
            (start, end, false)
        }
    };
    Ok(Item {
        text: compact,
        start,
        end,
        single,
    })
}

impl PageRanges {
    /// Parse a range such as `1-3,5,8-`. Checks syntax only.
    pub fn parse(s: &str) -> Result<Self, RangeError> {
        if s.trim().is_empty() {
            return Err(RangeError("empty page range".into()));
        }
        let items = s
            .split(',')
            .map(|part| {
                if part.trim().is_empty() {
                    Err(RangeError(format!(
                        "invalid page range \"{}\": empty item between commas",
                        s.trim()
                    )))
                } else {
                    parse_item(part)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(PageRanges { items })
    }

    /// Every page.
    pub fn all() -> Self {
        PageRanges::parse("all").expect("valid")
    }

    /// The pages of each comma-separated item, in written order, checked
    /// against `total` pages. Each item gives one group (used by split).
    pub fn resolve_groups(&self, total: u32) -> Result<Vec<Vec<u32>>, RangeError> {
        if total == 0 {
            return Err(RangeError("the document has no pages".into()));
        }
        let out_of_range = |item: &Item, p: u32| {
            RangeError(format!(
                "page range item \"{}\": page {p} is out of range (the document has {total} page{})",
                item.text,
                if total == 1 { "" } else { "s" }
            ))
        };
        self.items
            .iter()
            .map(|item| {
                let start = item.start.resolve(total);
                let end = item.end.map_or(total, |b| b.resolve(total));
                if start > total {
                    return Err(out_of_range(item, start));
                }
                if end > total {
                    return Err(out_of_range(item, end));
                }
                Ok(if start <= end {
                    (start..=end).collect()
                } else {
                    (end..=start).rev().collect()
                })
            })
            .collect()
    }

    /// All selected pages in written order; a page written twice appears twice.
    pub fn resolve(&self, total: u32) -> Result<Vec<u32>, RangeError> {
        Ok(self.resolve_groups(total)?.into_iter().flatten().collect())
    }

    /// The selected pages as a sorted set without repeats.
    pub fn resolve_set(&self, total: u32) -> Result<Vec<u32>, RangeError> {
        let mut pages = self.resolve(total)?;
        pages.sort_unstable();
        pages.dedup();
        Ok(pages)
    }

    /// The text of each item, for naming split parts (`1-3`, `5`, `8-`).
    pub fn item_labels(&self) -> Vec<String> {
        self.items.iter().map(|i| i.text.clone()).collect()
    }

    /// True when every item names a single page.
    pub fn is_single_pages(&self) -> bool {
        self.items.iter().all(|i| i.single)
    }
}

impl std::str::FromStr for PageRanges {
    type Err = RangeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        PageRanges::parse(s)
    }
}

impl fmt::Display for PageRanges {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.item_labels().join(","))
    }
}

/// Format sorted page numbers compactly: `[1,2,3,5]` → `1-3,5`.
pub fn compact(pages: &[u32]) -> String {
    let mut out = Vec::new();
    let mut i = 0;
    while i < pages.len() {
        let start = pages[i];
        let mut end = start;
        while i + 1 < pages.len() && pages[i + 1] == end + 1 {
            end += 1;
            i += 1;
        }
        out.push(if start == end {
            start.to_string()
        } else {
            format!("{start}-{end}")
        });
        i += 1;
    }
    out.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str, total: u32) -> Vec<u32> {
        PageRanges::parse(s).unwrap().resolve(total).unwrap()
    }

    fn err(s: &str, total: u32) -> String {
        match PageRanges::parse(s) {
            Err(e) => e.to_string(),
            Ok(p) => p.resolve(total).unwrap_err().to_string(),
        }
    }

    #[test]
    fn single_pages_and_ranges() {
        assert_eq!(r("1", 5), [1]);
        assert_eq!(r("1-3", 5), [1, 2, 3]);
        assert_eq!(r("1-3,5", 5), [1, 2, 3, 5]);
        assert_eq!(r(" 2 - 3 , 5 ", 5), [2, 3, 5]);
    }

    #[test]
    fn open_ends() {
        assert_eq!(r("3-", 5), [3, 4, 5]);
        assert_eq!(r("-2", 5), [1, 2]);
        assert_eq!(r("1-3,5,8-", 9), [1, 2, 3, 5, 8, 9]);
        assert_eq!(r("all", 3), [1, 2, 3]);
        assert_eq!(r("ALL", 2), [1, 2]);
    }

    #[test]
    fn last_keyword() {
        assert_eq!(r("last", 7), [7]);
        assert_eq!(r("last-1", 4), [4, 3, 2, 1]);
        assert_eq!(r("5-last", 7), [5, 6, 7]);
        assert_eq!(r("last-", 7), [7]);
        assert_eq!(r("1,last", 1), [1, 1]);
    }

    #[test]
    fn reversed_range_counts_down() {
        assert_eq!(r("3-1", 5), [3, 2, 1]);
        assert_eq!(r("last-3", 5), [5, 4, 3]);
    }

    #[test]
    fn order_and_repeats_are_kept_but_set_is_sorted() {
        assert_eq!(r("3,1,3", 3), [3, 1, 3]);
        let p = PageRanges::parse("3,1-2,2").unwrap();
        assert_eq!(p.resolve_set(3).unwrap(), [1, 2, 3]);
    }

    #[test]
    fn groups_follow_items() {
        let p = PageRanges::parse("1-2,3,4-").unwrap();
        assert_eq!(
            p.resolve_groups(5).unwrap(),
            vec![vec![1, 2], vec![3], vec![4, 5]]
        );
        assert_eq!(p.item_labels(), ["1-2", "3", "4-"]);
        assert!(!p.is_single_pages());
        assert!(PageRanges::parse("1,4").unwrap().is_single_pages());
    }

    #[test]
    fn syntax_errors_are_clear() {
        assert_eq!(err("", 3), "empty page range");
        assert!(err("0", 3).contains("numbered from 1"), "{}", err("0", 3));
        assert!(err("a", 3).contains("\"a\" is not a page number"));
        assert!(err("1,,2", 3).contains("empty item"));
        assert!(err("1,", 3).contains("empty item"));
        assert!(err("1-2-3", 3).contains("one '-'"));
        assert!(err("-", 3).contains("before or after"));
        assert!(err("1.5", 3).contains("not a page number"));
        assert!(err("x-3", 3).contains("\"x\""));
    }

    #[test]
    fn out_of_range_errors_name_the_page_count() {
        let e = err("4", 3);
        assert!(e.contains("page 4 is out of range"), "{e}");
        assert!(e.contains("3 pages"), "{e}");
        assert!(err("2-9", 1).contains("1 page)"));
        assert!(err("last-5", 3).contains("page 5 is out of range"));
        assert!(err("lastx", 3).contains("not a page number"));
        assert!(err("1", 0).contains("no pages"));
    }

    #[test]
    fn compact_formatting() {
        assert_eq!(compact(&[1, 2, 3, 5, 7, 8]), "1-3,5,7-8");
        assert_eq!(compact(&[4]), "4");
        assert_eq!(compact(&[]), "");
    }

    #[test]
    fn display_round_trips() {
        let p: PageRanges = "1 - 3, 5,last".parse().unwrap();
        assert_eq!(p.to_string(), "1-3,5,last");
    }
}
