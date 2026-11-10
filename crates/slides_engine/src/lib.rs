//! Kantor Slides engine (no GPUI): the presentation model, editing
//! commands with undo/redo, `.pptx` open/save, and display resolution.
//!
//! - [`model`]: shapes, text bodies, units (EMU, hundredths of a point).
//! - [`Presentation`]: slides, layouts, masters; open/save and the
//!   round-trip rules (see the module docs of [`presentation`]).
//! - Editing ([`edit`]): move/resize/rotate, text, lists, fill/outline,
//!   shapes, slides, speaker notes; each undoable. [`clip`]: copy/paste of
//!   shapes within and between presentations.
//! - [`render`]: what to draw, with layout/master/theme inheritance;
//!   [`fill`]: gradients, picture fills and line formatting for display.
//! - [`geom`]: shape outlines (preset and custom DrawingML geometry).

pub mod agent;
pub mod clip;
pub mod edit;
pub mod fill;
pub mod find;
pub mod geom;
mod history;
pub mod model;
pub mod presentation;
pub mod render;
mod shape;
mod template;
pub mod text_ops;
pub mod theme;
mod xml;

pub use clip::ShapeClip;
pub use docs_engine::{LINE_BREAK, Position, Rgb, Selection, Toggle};
pub use edit::{ShapeRef, image_ext, image_size, set_toggle};
pub use fill::{
    Arrow, ArrowKind, ArrowSize, BlipFill, Dash, Fill, GradPath, GradStop, Gradient, LineCap,
    LineJoin, LineStyle, PictureLook,
};
pub use find::TextMatch;
pub use history::Focus;
pub use model::*;
pub use presentation::{Layout, Master, Presentation, Slide, package_problems};
pub use render::{Layer, RenderPara, RenderRun, RenderShape, RenderText};
