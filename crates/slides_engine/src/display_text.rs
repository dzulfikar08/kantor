//! Text the engine supplies where a file has none: placeholder prompts
//! ("Click to add title") and chart fallbacks ("Chart Title", "Series1"),
//! shown in the interface language as PowerPoint and Excel do.
//!
//! The English stays here; the app sets a translator once
//! ([`set_display_translator`]). Without one (tests, the CLI) it's English.

use std::fmt::Display;
use std::sync::OnceLock;

/// English in, the interface language out (English when it has no
/// translation).
pub type Translate = fn(&'static str) -> &'static str;

static TRANSLATE: OnceLock<Translate> = OnceLock::new();

/// Set the translator. Only the first call counts.
pub fn set_display_translator(f: Translate) {
    let _ = TRANSLATE.set(f);
}

/// `en` in the interface language.
pub fn tr(en: &'static str) -> &'static str {
    TRANSLATE.get().map_or(en, |f| f(en))
}

/// `template` in the interface language with its `{}`s filled in order.
pub fn trf(template: &'static str, args: &[&dyn Display]) -> String {
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
        assert_eq!(tr("Chart Title"), "Chart Title");
        assert_eq!(trf("Series{}", &[&2]), "Series2");
    }
}
