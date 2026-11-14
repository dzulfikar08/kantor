//! Text Kantor writes into new content (pivot table captions, blank table
//! headers), in the interface language as Excel does.
//!
//! The engine keeps the English and asks a translator the app sets once
//! ([`set_content_translator`]); without one (tests, the CLI) it's English.
//! Text read from a file is never passed through it.

use std::fmt::Display;
use std::sync::OnceLock;

/// English in, the interface language out (English when it has no
/// translation).
pub type Translate = fn(&'static str) -> &'static str;

static TRANSLATE: OnceLock<Translate> = OnceLock::new();

/// Set the translator for new content. Only the first call counts.
pub fn set_content_translator(f: Translate) {
    let _ = TRANSLATE.set(f);
}

/// `en` in the interface language.
pub(crate) fn tr(en: &'static str) -> &'static str {
    TRANSLATE.get().map_or(en, |f| f(en))
}

/// `template` in the interface language with its `{}`s filled in order.
pub(crate) fn trf(template: &'static str, args: &[&dyn Display]) -> String {
    let mut out = String::new();
    let mut args = args.iter();
    let mut rest = tr(template);
    while let Some(i) = rest.find("{}") {
        out.push_str(&rest[..i]);
        if let Some(a) = args.next() {
            out.push_str(&a.to_string());
        }
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_without_a_translator() {
        assert_eq!(tr("Row Labels"), "Row Labels");
        assert_eq!(trf("{} of {}", &[&"Sum", &"Units"]), "Sum of Units");
    }
}
