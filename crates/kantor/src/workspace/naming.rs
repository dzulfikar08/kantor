//! Names for unsaved new documents: `Untitled.xlsx`, `Untitled 2.xlsx`, …
//!
//! Each app numbers its own untitled documents. A new one takes the lowest
//! number not used by an open untitled document of the same app, so closing
//! "Untitled 2.pptx" frees "2" for the next new presentation.

use super::AppKind;

impl AppKind {
    /// The extension new documents of this app are saved with.
    pub(super) fn extension(self) -> &'static str {
        match self {
            Self::Docs => "docx",
            Self::Sheets => "xlsx",
            Self::Slides => "pptx",
        }
    }
}

/// The name of untitled document number `n` (from 1) with extension `ext`.
pub(super) fn untitled_name(n: u32, ext: &str) -> String {
    if n <= 1 {
        format!("Untitled.{ext}")
    } else {
        format!("Untitled {n}.{ext}")
    }
}

/// The number in a name made by [`untitled_name`] with extension `ext`.
pub(super) fn untitled_number(name: &str, ext: &str) -> Option<u32> {
    let stem = name.strip_suffix(ext)?.strip_suffix('.')?;
    let rest = stem.strip_prefix("Untitled")?;
    if rest.is_empty() {
        return Some(1);
    }
    let n: u32 = rest.strip_prefix(' ')?.parse().ok()?;
    // "Untitled 1.xlsx" and "Untitled 02.xlsx" aren't names we make.
    (n >= 2 && rest[1..] == n.to_string()).then_some(n)
}

/// The lowest number from 1 that isn't in `used`.
pub(super) fn lowest_free(used: impl IntoIterator<Item = u32>) -> u32 {
    let mut used: Vec<u32> = used.into_iter().collect();
    used.sort_unstable();
    used.dedup();
    let mut n = 1;
    for u in used {
        if u == n {
            n += 1;
        } else if u > n {
            break;
        }
    }
    n
}

/// The name for a new untitled document of `kind`, given the names of the
/// open untitled documents of that app.
pub(super) fn next_untitled_name<'a>(
    kind: AppKind,
    open_untitled: impl IntoIterator<Item = &'a str>,
) -> String {
    let ext = kind.extension();
    let n = lowest_free(
        open_untitled
            .into_iter()
            .filter_map(|name| untitled_number(name, ext)),
    );
    untitled_name(n, ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn names_carry_the_app_extension() {
        assert_eq!(next_untitled_name(AppKind::Sheets, []), "Untitled.xlsx");
        assert_eq!(next_untitled_name(AppKind::Docs, []), "Untitled.docx");
        assert_eq!(next_untitled_name(AppKind::Slides, []), "Untitled.pptx");
    }

    #[std::prelude::v1::test]
    fn duplicates_are_numbered_from_two() {
        assert_eq!(
            next_untitled_name(AppKind::Slides, ["Untitled.pptx"]),
            "Untitled 2.pptx"
        );
        assert_eq!(
            next_untitled_name(AppKind::Slides, ["Untitled.pptx", "Untitled 2.pptx"]),
            "Untitled 3.pptx"
        );
    }

    #[std::prelude::v1::test]
    fn takes_the_lowest_free_number() {
        // "Untitled.docx" was closed: its slot is reused first.
        assert_eq!(
            next_untitled_name(AppKind::Docs, ["Untitled 2.docx", "Untitled 3.docx"]),
            "Untitled.docx"
        );
        assert_eq!(
            next_untitled_name(AppKind::Docs, ["Untitled 3.docx", "Untitled.docx"]),
            "Untitled 2.docx"
        );
    }

    #[std::prelude::v1::test]
    fn apps_are_numbered_separately() {
        // Other apps' untitled documents don't take numbers.
        assert_eq!(
            next_untitled_name(AppKind::Sheets, ["Untitled.docx", "Untitled.pptx"]),
            "Untitled.xlsx"
        );
    }

    #[std::prelude::v1::test]
    fn parses_only_names_it_makes() {
        assert_eq!(untitled_number("Untitled.xlsx", "xlsx"), Some(1));
        assert_eq!(untitled_number("Untitled 7.xlsx", "xlsx"), Some(7));
        assert_eq!(untitled_number("Untitled 1.xlsx", "xlsx"), None);
        assert_eq!(untitled_number("Untitled 07.xlsx", "xlsx"), None);
        assert_eq!(untitled_number("Untitled2.xlsx", "xlsx"), None);
        assert_eq!(untitled_number("Untitled 2.docx", "xlsx"), None);
        assert_eq!(untitled_number("budget.xlsx", "xlsx"), None);
        assert_eq!(untitled_number("Untitled", "xlsx"), None);
    }

    #[std::prelude::v1::test]
    fn lowest_free_handles_gaps_and_repeats() {
        assert_eq!(lowest_free([]), 1);
        assert_eq!(lowest_free([1, 1, 2]), 3);
        assert_eq!(lowest_free([2, 5]), 1);
        assert_eq!(lowest_free([1, 3]), 2);
    }
}
