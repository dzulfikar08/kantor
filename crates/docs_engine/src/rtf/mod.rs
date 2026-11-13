//! Reading Rich Text Format (`.rtf`), import only.
//!
//! The file is tokenized ([`lex`]), read into a small document tree with a
//! stack of group states ([`parse`]), and written as a WordprocessingML
//! package ([`ooxml`]) that the `.docx` reader opens like any other file.
//! So an imported document is modelled exactly like a `.docx` one, and
//! Save writes a `.docx` (the source is never overwritten: the document has
//! no path, so Save goes to Save As).
//!
//! What is read:
//! * text in the document's code page (`\ansicpg`), per-font `\fcharset`,
//!   `\'hh` (multi-byte code pages too), `\uN` with `\ucN` fallback
//!   skipping and surrogate pairs;
//! * character formatting: bold, italic, underline types, strike, double
//!   strike, caps, small caps, size, font, colour, highlight, shading,
//!   superscript and subscript, character styles; hidden text is dropped;
//! * paragraphs: alignment, indents, spacing, line spacing, keep with next,
//!   keep lines, page break before, tab stops, line and page breaks, the
//!   style sheet (`heading N` styles become Word's heading styles; outline
//!   levels are kept);
//! * lists: `\listtable` / `\listoverridetable` with `\ls` / `\ilvl`, and
//!   Word 95 `\pn` paragraphs;
//! * tables: rows, cell widths, horizontal and vertical merges, borders,
//!   shading, vertical alignment, header rows, row heights (nested tables
//!   are flattened into their cell);
//! * sections: page size, orientation, margins, header/footer distance,
//!   section breaks, first-page and odd/even headers and footers;
//! * PNG and JPEG pictures (`\pict`, hex or `\bin`) at their shown size;
//! * fields show their result; `HYPERLINK` fields become links, `PAGE` and
//!   `NUMPAGES` stay page number fields.
//!
//! Left out (and listed in [`Import::notes`]): footnotes, comments, text
//! boxes and drawing objects, pictures in other formats (WMF, EMF…).
//! Unknown `\*` destinations are skipped. Malformed or truncated files
//! never panic; anything after the closing brace is ignored.

mod lex;
mod ooxml;
mod parse;

/// A read RTF file.
pub struct Import {
    /// The document as a `.docx` package.
    pub docx: Vec<u8>,
    /// What could not be brought over, as short sentences.
    pub notes: Vec<String>,
}

/// Whether `bytes` look like an RTF file (`{\rtf`, after any BOM or space).
pub fn is_rtf(bytes: &[u8]) -> bool {
    let b = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let start = b
        .iter()
        .position(|c| !c.is_ascii_whitespace())
        .unwrap_or(b.len());
    b[start..].starts_with(b"{\\rtf")
}

/// Convert an RTF file to a `.docx` package.
pub fn to_docx(bytes: &[u8]) -> anyhow::Result<Import> {
    if !is_rtf(bytes) {
        anyhow::bail!("not an RTF file");
    }
    let parsed = parse::parse(bytes);
    let notes = parsed.notes.iter().map(|n| (*n).to_owned()).collect();
    Ok(Import {
        docx: ooxml::package(&parsed)?,
        notes,
    })
}
