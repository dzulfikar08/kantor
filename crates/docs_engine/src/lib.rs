//! Kantor Docs engine (no GPUI): the document model, editing commands with
//! undo/redo, and `.docx` open/save.
//!
//! - [`model`]: paragraphs of styled runs, positions and selections.
//! - [`style`]: the style sheet and resolving effective formatting.
//! - [`Document`]: edits (each one undoable), dirty tracking, open/save.
//! - [`docx`]: reading and writing `.docx`, and the round-trip safety rules
//!   (what's modelled, kept verbatim, carried as an opaque object, or
//!   "unsupported content" that sends Save to Save As).
//! - [`numbering`]: lists; [`table`]: tables; [`object`]: inline pictures
//!   and opaque elements.

mod document;
pub mod docx;
mod edit;
pub mod find;
pub mod model;
pub mod numbering;
pub mod object;
pub mod page;
mod rewrite;
pub mod style;
pub mod table;
mod xml;

pub use document::{Document, LINE_BREAK, NewMedia, PAGE_BREAK, PageGeometry, Toggle};
pub use edit::TableEdit;
pub use model::{
    Align, CellRef, Fonts, Indent, LineRule, LineSpacing, NumRef, ParaProps, Paragraph, Position,
    Rgb, Run, RunProps, Selection,
};
pub use numbering::ListKind;
pub use object::{Image, ImageFormat, Level, OBJECT, Object, Opaque};
pub use page::{MarginPreset, Orientation, PaperSize, cm_to_twips, twips_to_cm};
pub use style::{HEADING_STYLES, ResolvedLine, ResolvedPara, ResolvedRun, StyleSheet, font_stack};
pub use table::{Border, Borders, Table, VMerge};
