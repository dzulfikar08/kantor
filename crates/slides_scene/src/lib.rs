//! Kantor Slides display lists (no GPUI).
//!
//! [`Builder::slide`] turns one slide into a [`SlideScene`]: a list of
//! primitives in slide pixels (EMU / 9525; 1280 × 720 for 16:9), bottom
//! to top, each with its own affine transform. The canvas, thumbnails and
//! presenter mode paint scenes with GPUI (`slides_ui`); `slides_pdf` writes
//! the same scenes with krilla, so the PDF matches the screen.
//!
//! - Shapes: filled and stroked [`Path`]s in the shape's own box; the
//!   transform turns and flips them. Geometry comes from
//!   [`geom::shape_outline`], the single place that knows shapes.
//! - Fills: solid, linear and radial gradients ([`Paint`]) with stops and
//!   opacity; picture fills as [`Picture`]s clipped to the outline.
//! - Lines: width, colour, dash lengths, caps, joins; arrowheads become
//!   their own fills and strokes.
//! - Pictures: cropping (`srcRect`), opacity.
//! - Text: glyph runs from `docs_layout`, turned with the shape but never
//!   flipped (as PowerPoint draws it).
//!
//! A scene is `Send`: it owns its glyph runs, fonts and picture bytes.

mod build;
pub mod geom;
pub mod paint;
pub mod text;

use std::sync::Arc;

pub use build::{Builder, Options};
pub use docs_layout::{Engine, Rgba};
pub use geom::{Affine, P, Path, PathFill, Seg, SubPath, p, shape_outline};
use slides_engine::{Layer, LineCap, LineJoin};
pub use text::{LaidText, TextLayout, TextRun};

/// A gradient stop: offset 0..1 and colour with opacity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub offset: f32,
    pub color: Rgba,
}

/// What a path is filled with (in the item's own coordinates).
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Solid(Rgba),
    /// Stop 0 on the line through `from`, stop 1 through `to`, both
    /// perpendicular to `to - from`; padded beyond.
    Linear {
        from: P,
        to: P,
        stops: Vec<Stop>,
    },
    /// Elliptical: stop 0 at `center`, stop 1 at radii `rx`, `ry`.
    Radial {
        center: P,
        rx: f32,
        ry: f32,
        stops: Vec<Stop>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FillRule {
    NonZero,
    #[default]
    EvenOdd,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stroke {
    pub width: f32,
    pub color: Rgba,
    /// Dash, gap, dash, gap... (absolute lengths); `None` = solid.
    pub dash: Option<Vec<f32>>,
    pub cap: LineCap,
    pub join: LineJoin,
}

/// A picture drawn into `rect` (`[x, y, w, h]`).
#[derive(Clone, Debug)]
pub struct Picture {
    /// The media part (for caches).
    pub part: Arc<str>,
    pub data: Arc<Vec<u8>>,
    pub rect: [f32; 4],
    /// Cut off each edge, left, top, right, bottom, as fractions of the
    /// picture (negative = padding).
    pub crop: [f32; 4],
    /// Opacity 0..1.
    pub alpha: f32,
    /// Only what's inside this path shows (picture fills of shapes).
    pub clip: Option<Path>,
}

impl Picture {
    /// Where the whole (uncropped) picture lands, `[x, y, w, h]`: the box
    /// shows the part left after cropping.
    pub fn image_rect(&self) -> [f32; 4] {
        let [x, y, w, h] = self.rect;
        let [l, t, r, b] = self.crop;
        let kw = (1.0 - l - r).max(1e-3);
        let kh = (1.0 - t - b).max(1e-3);
        let (iw, ih) = (w / kw, h / kh);
        [x - l * iw, y - t * ih, iw, ih]
    }

    /// Whether anything is cut off (or padded).
    pub fn cropped(&self) -> bool {
        self.crop.iter().any(|c| c.abs() > 1e-4)
    }
}

/// Text: glyph runs relative to `origin` (layout coordinates are slide px).
#[derive(Clone, Debug)]
pub struct Text {
    pub origin: P,
    pub runs: Vec<TextRun>,
}

/// One primitive.
#[derive(Clone, Debug)]
pub enum Prim {
    Fill {
        path: Path,
        paint: Paint,
        rule: FillRule,
    },
    Stroke {
        path: Path,
        stroke: Stroke,
    },
    Picture(Picture),
    Text(Text),
    /// Something Kantor keeps but can't draw (table, chart, group...): a
    /// `w` × `h` box. On screen only.
    Unsupported {
        w: f32,
        h: f32,
    },
    /// The editor's dashed outline of an empty placeholder (never printed).
    PlaceholderOutline {
        w: f32,
        h: f32,
    },
}

/// A primitive placed on the slide.
#[derive(Clone, Debug)]
pub struct Item {
    /// The shape it belongs to (`None` = the background).
    pub layer: Option<Layer>,
    /// From the item's own coordinates to slide px.
    pub transform: Affine,
    pub prim: Prim,
}

/// Everything drawn on one slide, bottom to top, in slide px.
#[derive(Clone, Debug)]
pub struct SlideScene {
    pub width: f32,
    pub height: f32,
    /// The background's main colour (for chrome that must contrast).
    pub background: slides_engine::Rgb,
    pub items: Vec<Item>,
    /// Hidden in the slide show (`show="0"`).
    pub hidden: bool,
}
