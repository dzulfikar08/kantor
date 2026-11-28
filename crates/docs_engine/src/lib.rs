//! Kantor Docs engine (no GPUI): the document model, editing commands with
//! undo/redo, and `.docx` open/save.
//!
//! - [`model`]: paragraphs of styled runs, positions and selections.
//! - [`style`]: the style sheet and resolving effective formatting.
//! - [`Document`]: edits (each one undoable), dirty tracking, open/save.
//! - [`docx`]: reading and writing `.docx`, and the round-trip safety rules
//!   (what's modelled, kept verbatim, carried as an opaque object, or
//!   "unsupported content" that sends Save to Save As).
//! - `msdoc`: importing Word 97-2003 `.doc` / `.dot` files (read only).
//! - [`numbering`]: lists; [`table`]: tables; [`object`]: inline pictures
//!   and opaque elements.
//! - [`rtf`]: importing `.rtf` files (read-only; Save writes `.docx`).
//! - [`odt`]: importing OpenDocument text (`.odt`, `.ott`, `.fodt`;
//!   read-only, Save writes `.docx`).

pub mod address;
pub mod agent;
pub mod citations;
pub mod cite_edit;
mod code_track;
pub mod columns;
mod comment_thread;
mod document;
pub mod docx;
mod edit;
mod field_calc;
mod field_edit;
mod field_track;
pub mod fields;
pub mod find;
pub mod float;
pub mod fmt_track;
pub mod merge;
pub mod model;
mod msdoc;
pub mod notes;
pub mod numbering;
pub mod object;
pub mod odt;
pub mod page;
pub mod part_edit;
pub mod review;
mod rewrite;
pub mod rtf;
pub mod section;
pub mod style;
pub mod table;
pub mod table_merge;
mod table_props;
pub mod tabs;
mod track;
mod track_table;
pub mod xml;

pub use citations::{Bibliography, Citation, CiteAt, CiteItem, CiteSource};
pub use cite_edit::{CitePrefs, PrefsFrom, RefreshReport};
pub use columns::{COLUMN_BREAK, Column, ColumnPreset, Columns, ColumnsApply};
pub use document::{
    Document, LINE_BREAK, NewMedia, PAGE_BREAK, PageGeometry, SaveJob, SavedJob, Splice, Toggle,
    is_import_path,
};
pub use edit::TableEdit;
pub use field_edit::{XrefKind, XrefTarget, XrefWhat, valid_bookmark_name};
pub use fields::{
    CodeView, FieldContext, FieldSpan, Instr, LinkTarget, PageLookup, StyleRefs, style_ref_results,
    with_style_ref_results,
};
pub use float::{Float, FloatKind, LayoutOption, Placement, Wrap};
pub use fmt_track::FormatItem;
pub use merge::{Filter, FilterOp, MailMergeLink, MergeData, MergeOutcome, MergeValue, Query};
pub use model::{
    Align, CellRef, Fonts, Indent, LineRule, LineSpacing, NumRef, ParaProps, Paragraph, Position,
    Rgb, Run, RunProps, Selection,
};
pub use notes::{Note, NoteKind, NoteProps, NoteRef, NoteRestart};
pub use numbering::ListKind;
pub use object::{FieldMark, Image, ImageFormat, Level, OBJECT, Object, Opaque, drawable_picture};
pub use page::{MarginPreset, Orientation, PaperSize, cm_to_twips, twips_to_cm};
pub use part_edit::{BoxRef, PartEdit, PartRef};
pub use review::{Anchor, Change, Comment, RevKind, Revision};
pub use section::{
    ChapterSep, HfKind, HfType, NumFormat, PageField, PageNumbering, Section, SectionProps,
    SectionStart, Story, page_field, with_page_numbers,
};
pub use style::{
    HEADING_STYLES, Proofing, ResolvedLine, ResolvedPara, ResolvedRun, StyleSheet, font_stack,
};
pub use table::{Border, Borders, Table, TableAlign, VMerge};
pub use table_props::TableProps;
pub use tabs::{TabKind, TabLeader, TabStop};
pub use track_table::TableChange;
