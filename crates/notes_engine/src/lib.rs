//! Kantor Notes engine: Markdown notes edited as text.
//!
//! The `.md` text *is* the document. [`Buffer`] keeps the file's bytes
//! (line endings, byte-order mark, trailing newline) exactly; [`parse`]
//! reads it with pulldown-cmark into a per-line model that says which bytes
//! are content and which are markup, so the editor can draw a line
//! formatted with its markup hidden ("live preview") or as typed;
//! [`Editor`] holds the selection, the undo history and the editing
//! commands. No GPUI here: `notes_ui` draws it.
//!
//! Later phases build on these pieces: [`parse::Parsed::links`] already
//! carries `[[wiki links]]` (kind [`parse::LinkKind::Wiki`]) and the
//! headings outline, for backlinks and a folder of notes.

pub mod buffer;
pub mod editor;
pub mod parse;
pub mod prefix;

pub use buffer::{Buffer, Eol, OpenError};
pub use editor::{Editor, Merge, Selection};
pub use parse::{LineInfo, LineKind, ListMarker, Parsed, SpanKind, parse};

/// Extensions Notes opens.
pub const EXTENSIONS: &[&str] = &["md", "markdown"];

/// Whether `path` names a Markdown note.
pub fn is_note_path(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Words and characters in `text` (for the status bar).
pub fn count(text: &str) -> (usize, usize) {
    (text.split_whitespace().count(), text.chars().count())
}

/// A file name for a picture pasted into a note: `Pasted image
/// 20261127153000.png`, Obsidian's pattern, made unique in `dir`.
pub fn attachment_name(dir: &std::path::Path, stamp: &str, ext: &str) -> String {
    let base = format!("Pasted image {stamp}");
    let mut name = format!("{base}.{ext}");
    let mut n = 1;
    while dir.join(&name).exists() {
        n += 1;
        name = format!("{base} {n}.{ext}");
    }
    name
}

/// A link target for a relative path in Markdown: spaces and parentheses
/// break `![](…)` links, so they are written with `<…>` when present.
pub fn link_target(rel: &str) -> String {
    let rel = rel.replace('\\', "/");
    if rel.contains([' ', '(', ')']) {
        format!("<{rel}>")
    } else {
        rel
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_paths() {
        assert!(is_note_path(std::path::Path::new("a/B.MD")));
        assert!(is_note_path(std::path::Path::new("x.markdown")));
        assert!(!is_note_path(std::path::Path::new("x.txt")));
    }

    #[test]
    fn link_targets() {
        assert_eq!(link_target("attachments/a.png"), "attachments/a.png");
        assert_eq!(
            link_target("attachments\\Pasted image 1.png"),
            "<attachments/Pasted image 1.png>"
        );
    }

    #[test]
    fn image_link_with_angle_brackets_parses() {
        let b = Buffer::new("![](<attachments/Pasted image 1.png>)");
        let p = parse(&b);
        assert_eq!(p.images[0].dest, "attachments/Pasted image 1.png");
    }
}
