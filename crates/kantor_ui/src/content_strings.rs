//! English the engines write into new content or show in the UI, as
//! catalog keys.
//!
//! The engines don't depend on kantor_ui: they keep the English and ask a
//! translator the app sets at startup (`sheets_engine::set_content_translator`,
//! `slides_engine::display_text::set_display_translator`, both given
//! [`crate::i18n::t`]). The catalog scanner only reads the UI crates, so the
//! keys are listed here with `tk`.
//!
//! Docs and Slides describe content they can't keep with short English
//! phrases (`tracked changes`, `a missing slide part (ppt/slides/x.xml)`);
//! [`content_list`] words a list of them in the interface language.

use crate::i18n::{self, tk};

/// Keys the engines pass to their translator.
pub const ENGINE_STRINGS: &[&str] = &[
    // Pivot tables made in Kantor (sheets_engine::pivot).
    tk("Row Labels"),
    tk("Column Labels"),
    tk("Grand Total"),
    tk("Values"),
    tk("{} of {}"),
    tk("Total {}"),
    tk("{} Total"),
    tk("(blank)"),
    tk("(All)"),
    tk("(Multiple Items)"),
    tk("Sum"),
    tk("Count"),
    tk("Average"),
    tk("Max"),
    tk("Min"),
    tk("Product"),
    // Grouping (sheets_ui shows the date levels too).
    tk("Group{}"),
    // Blank headers of a new table.
    tk("Column{}"),
    // Slides placeholder prompts and chart fallbacks.
    tk("Click to add title"),
    tk("Click to add subtitle"),
    tk("Click to add text"),
    tk("Chart Title"),
    tk("Series{}"),
    // Chart legends and series Kantor names (slides_scene::chart).
    tk("Waterfall: Increase"),
    tk("Waterfall: Decrease"),
    tk("Total"),
    tk("Cumulative %"),
    tk("Other"),
    tk("Map: no map data saved with this chart"),
    tk("Map: the map data saved with this chart can't be read"),
];

/// Fixed phrases Docs and Slides use for content they can't keep or only
/// show as placeholders.
pub const CONTENT_ITEMS: &[&str] = &[
    tk("tracked changes"),
    tk("comments"),
    tk("fields spanning paragraphs"),
    tk("fields without an end"),
    tk("fields across hyperlinks or other elements"),
    tk("content controls around table rows or cells"),
    tk("charts"),
    tk("floating images and shapes"),
    tk("drawings"),
    tk("shapes and embedded objects"),
    tk("content controls"),
    tk("equations"),
    tk("nested tables"),
    tk("embedded documents"),
    tk("horizontally merged cells"),
    tk("fields"),
    tk("slide sections (dropped when slides are added, removed or reordered)"),
    tk("custom slide shows (dropped when slides are added, removed or reordered)"),
    // Import notes: .rtf (docs_engine::rtf), .doc (docs_engine::msdoc),
    // .ppt (slides_engine::ppt), .odt (docs_engine::odt).
    tk("footnotes were left out"),
    tk("comments were left out"),
    tk("text boxes and drawings were left out"),
    tk("pictures in formats other than PNG and JPEG were left out"),
    tk("comments not imported"),
    tk("text boxes not imported"),
    tk("floating pictures and shapes not imported"),
    tk("floating objects in headers and footers not imported"),
    tk("grouped shapes not imported"),
    tk("freeform shapes not imported"),
    tk("TIFF pictures not imported"),
    tk("hidden text left out"),
    tk("embedded objects not imported"),
    tk("nested tables flattened"),
    tk("1 TIFF picture shown as a box"),
    tk("drawing shapes were left out"),
    tk("pictures in formats Kantor can't show (SVG, TIFF) were left out"),
    tk("embedded objects without a preview picture were left out"),
    tk("tracked formatting changes were left out (shown as made)"),
    tk("form controls were left out"),
    // .odp (slides_engine::odp).
    tk("1 chart left out"),
    tk("1 embedded object left out"),
    tk("1 video or sound left out"),
    tk("1 picture in a format Kantor can't show left out"),
    tk("1 shape Kantor can't read left out"),
    tk("slide animations were left out"),
];

/// Phrases with a detail in them (an XML element or part name, kept as is).
pub const CONTENT_TEMPLATES: &[&str] = &[
    tk("{} in a table"),
    tk("a missing slide part ({})"),
    tk("speaker notes that can't be read ({})"),
    // A count (slides_engine::ppt).
    tk("{} TIFF pictures shown as boxes"),
    // A count (slides_engine::odp).
    tk("{} charts left out"),
    tk("{} embedded objects left out"),
    tk("{} videos and sounds left out"),
    tk("{} pictures in a format Kantor can't show left out"),
    tk("{} shapes Kantor can't read left out"),
];

/// One phrase in the interface language (as it is when unknown, e.g. an
/// XML element name like `<w:ruby>`).
pub fn content_item(en: &str) -> String {
    item_with(en, |k| i18n::t_dyn(k))
}

/// [`content_item`] with `word` giving a known key's translation.
fn item_with(en: &str, word: impl Fn(&'static str) -> &str) -> String {
    if let Some(k) = CONTENT_ITEMS.iter().find(|k| **k == en) {
        return word(k).to_owned();
    }
    for template in CONTENT_TEMPLATES {
        let Some((pre, post)) = template.split_once("{}") else {
            continue;
        };
        if let Some(detail) = en
            .strip_prefix(pre)
            .and_then(|r| r.strip_suffix(post))
            .filter(|d| !d.is_empty())
        {
            return i18n::fill(word(template), &[&detail]);
        }
    }
    en.to_owned()
}

/// A list of phrases, translated and joined with `, `.
pub fn content_list(items: &[String]) -> String {
    items
        .iter()
        .map(|s| content_item(s))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn every_string_has_indonesian() {
        let missing: Vec<&str> = ENGINE_STRINGS
            .iter()
            .chain(CONTENT_ITEMS)
            .chain(CONTENT_TEMPLATES)
            .filter(|en| i18n::indonesian(en).is_none())
            .copied()
            .collect();
        assert!(missing.is_empty(), "no Indonesian for {missing:?}");
    }

    #[std::prelude::v1::test]
    fn lists_keep_unknown_phrases_and_details() {
        // Tests run in English: the phrase comes back as it is.
        assert_eq!(
            content_list(&["tracked changes".into(), "<w:ruby>".into()]),
            "tracked changes, <w:ruby>"
        );
        assert_eq!(
            content_item("a missing slide part (ppt/slides/slide3.xml)"),
            "a missing slide part (ppt/slides/slide3.xml)"
        );
    }

    #[std::prelude::v1::test]
    fn indonesian_phrases_keep_their_detail() {
        let id = |en: &str| item_with(en, |k| i18n::indonesian(k).unwrap_or(k));
        assert_eq!(id("<w:sdt> in a table"), "<w:sdt> dalam tabel");
        assert_eq!(
            id("speaker notes that can't be read (ppt/notesSlides/n1.xml)"),
            "catatan pembicara yang tidak dapat dibaca (ppt/notesSlides/n1.xml)"
        );
        assert_eq!(id("tracked changes"), "perubahan terlacak");
        assert_eq!(id("<w:ruby>"), "<w:ruby>");
        // A template starting with its detail.
        assert_eq!(
            id("3 TIFF pictures shown as boxes"),
            "3 gambar TIFF ditampilkan sebagai kotak"
        );
        assert_eq!(
            id("1 TIFF picture shown as a box"),
            "1 gambar TIFF ditampilkan sebagai kotak"
        );
        assert_eq!(id("nested tables flattened"), "tabel bersarang diratakan");
    }
}
