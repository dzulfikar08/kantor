//! Kantor Docs engine (no GPUI): the document model, editing commands with
//! undo/redo, and `.docx` open/save.
//!
//! - [`model`]: paragraphs of styled runs, positions and selections.
//! - [`style`]: the style sheet and resolving effective formatting.
//! - [`Document`]: edits (each one undoable), dirty tracking, open/save.
//! - [`docx`]: reading and writing `.docx`, and the round-trip safety rules
//!   (what counts as "unsupported content" and why Save then goes to Save As).

mod document;
pub mod docx;
pub mod model;
pub mod style;

pub use document::{Document, LINE_BREAK, PageGeometry, Toggle};
pub use model::{
    Align, Fonts, LineRule, LineSpacing, ParaProps, Paragraph, Position, Rgb, Run, RunProps,
    Selection,
};
pub use style::{HEADING_STYLES, ResolvedLine, ResolvedPara, ResolvedRun, StyleSheet, font_stack};
