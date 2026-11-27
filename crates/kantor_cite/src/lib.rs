//! Citations for Kantor Docs, without a document model (see
//! `docs/decisions/0007-citations.md`):
//!
//! - [`styles`]: CSL styles by id (hayagriva's archive, Zotero's `styles`
//!   folder, a `.csl` file) and the styles Kantor offers;
//! - [`render`]: citations and bibliographies as formatted text, from
//!   CSL-JSON items, through hayagriva;
//! - [`import`]: `.bib` (BibLaTeX/BibTeX), RIS and CSL-JSON files to
//!   CSL-JSON items;
//! - [`zotero`]: a running Zotero's local connector (Better BibTeX's
//!   Cite-as-you-Write picker, Zotero 7's local API).

pub mod import;
pub mod render;
pub mod styles;
pub mod zotero;

pub use render::{CiteIn, Fmt, Piece, Rendered, plain, render};
pub use styles::{BUNDLED, CslStyle, DEFAULT_STYLE, load_style};
