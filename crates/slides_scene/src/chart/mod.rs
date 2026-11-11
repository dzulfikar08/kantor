//! Charts (`c:chartSpace` parts of chart frames) drawn as scene items,
//! for display and PDF only: the chart part is never written.
//!
//! Three steps, the first two without fonts or GPUI:
//! - [`model`]: ChartML → [`Chart`] from the cached values in the part
//!   (the embedded workbook isn't opened); colours resolved with the
//!   slide's theme.
//! - [`layout`]: title, legend, axes (Excel-like auto scaling, see
//!   [`scale`]), series and data labels → fills, strokes and labels in the
//!   frame's px. Number formats in [`format`].
//! - here: labels laid out with the Docs engine (parley), the same path as
//!   slide text, so the PDF embeds the fonts and the text can be copied.
//!
//! Drawn: bar / column (clustered, stacked, 100 %), line, area, pie,
//! doughnut, scatter, and combinations of them on shared or secondary
//! axes. Anything else (radar, bubble, stock, surface, 3-D...) keeps the
//! placeholder box.
//!
//! The model and layout know nothing of slides beyond [`Defaults`]. Sheets
//! draws charts embedded in worksheets with the same code: it parses the
//! part ([`Chart::parse`]), puts the cells each series refers to
//! ([`SeriesRefs`]) in place of the caches, and lays it out with [`items`].

pub mod format;
pub mod layout;
pub mod model;
pub mod scale;

use std::collections::HashMap;

use docs_layout::parley::LineHeight;
use docs_layout::{Engine, PageSetup};
use slides_engine::Rgb;
use slides_engine::theme::ColorCtx;

pub use layout::{Defaults, HAlign, Label, Mark, Measure, TextFont};
pub use model::{Chart, SeriesRefs};

use crate::geom::{Affine, p};
use crate::text::{TextLayout, glyph_runs};
use crate::{Item, Prim, Text};

/// The theme's chart colours and fonts on a slide.
pub fn defaults(cx: &ColorCtx) -> Defaults {
    let d = Defaults::default();
    let slot = |n: &str, dflt: Rgb| cx.theme.color(n).unwrap_or(dflt);
    let accents = std::array::from_fn(|i| slot(&format!("accent{}", i + 1), d.accents[i]));
    let mapped = |name: &str, dflt: Rgb| {
        roxmltree::Document::parse(&format!(
            "<a:schemeClr xmlns:a=\"{}\" val=\"{name}\"/>",
            model::NS_A
        ))
        .ok()
        .and_then(|doc| cx.color(doc.root_element()))
        .unwrap_or(dflt)
    };
    Defaults {
        accents,
        text: mapped("tx1", d.text),
        background: mapped("bg1", d.background),
        minor: cx.theme.font("+mn-lt").unwrap_or(d.minor),
        major: cx.theme.font("+mj-lt").unwrap_or(d.major),
    }
}

fn paragraph(text: &str, font: &TextFont, align: HAlign) -> Vec<docs_layout::Paragraph> {
    let style = |len: usize| docs_layout::Span {
        range: 0..len,
        bold: font.bold,
        italic: font.italic,
        underline: false,
        size_pt: Some(font.size_pt),
        color: Some(font.color),
        family: Some(docs_engine::font_stack(&font.face)),
    };
    text.split('\n')
        .map(|line| {
            let mut lp = docs_layout::Paragraph::new(line.to_owned());
            lp.family = docs_engine::font_stack(&font.face);
            lp.size_pt = font.size_pt;
            lp.line_height = Some(LineHeight::MetricsRelative(1.0));
            lp.space_after_pt = 0.0;
            lp.align = match align {
                HAlign::Left => docs_layout::Align::Start,
                HAlign::Center => docs_layout::Align::Center,
                HAlign::Right => docs_layout::Align::End,
            };
            lp.spans = vec![style(line.len())];
            lp
        })
        .collect()
}

fn lay(engine: &mut Engine, text: &str, font: &TextFont, align: HAlign, width: f32) -> TextLayout {
    let paras = paragraph(text, font, align);
    let setup = PageSetup {
        width: width.max(1.0),
        height: 1.0e7,
        margin_top: 0.0,
        margin_bottom: 0.0,
        margin_left: 0.0,
        margin_right: 0.0,
    };
    let paged = engine.layout_document(&paras, setup);
    let height = paged
        .pages
        .first()
        .and_then(|p| p.lines.last())
        .map_or(0.0, |l| l.top + l.height);
    TextLayout {
        paged,
        texts: paras.into_iter().map(|p| p.text).collect(),
        height,
    }
}

type MeasureKey = (String, u32, bool, bool, String, Option<u32>);

/// Measures with the Docs engine, remembering what it measured (a layout
/// asks for the same labels several times while fitting them).
pub struct EngineMeasure<'e> {
    pub engine: &'e mut Engine,
    seen: HashMap<MeasureKey, (f32, f32)>,
}

impl<'e> EngineMeasure<'e> {
    pub fn new(engine: &'e mut Engine) -> Self {
        Self {
            engine,
            seen: HashMap::new(),
        }
    }
}

impl Measure for EngineMeasure<'_> {
    fn measure(&mut self, text: &str, font: &TextFont, wrap: Option<f32>) -> (f32, f32) {
        let key = (
            text.to_owned(),
            font.size_pt.to_bits(),
            font.bold,
            font.italic,
            font.face.clone(),
            wrap.map(f32::to_bits),
        );
        if let Some(m) = self.seen.get(&key) {
            return *m;
        }
        let m = self.measure_new(text, font, wrap);
        self.seen.insert(key, m);
        m
    }
}

impl EngineMeasure<'_> {
    fn measure_new(&mut self, text: &str, font: &TextFont, wrap: Option<f32>) -> (f32, f32) {
        let l = lay(
            self.engine,
            text,
            font,
            HAlign::Left,
            wrap.unwrap_or(100_000.0),
        );
        let w = glyph_runs(&l)
            .iter()
            .map(|r| r.run.origin_x + r.run.advance)
            .fold(0.0, f32::max);
        (w, l.height)
    }
}

/// A chart part's items in a `w` × `h` frame (frame px, origin top left;
/// `layer` unset). `None` when the part isn't a chart Kantor can draw.
pub fn chart_items(
    engine: &mut Engine,
    xml: &[u8],
    cx: &ColorCtx,
    w: f32,
    h: f32,
) -> Option<Vec<Item>> {
    let xml = std::str::from_utf8(xml).ok()?;
    let chart = Chart::parse(xml, cx)?;
    items(engine, &chart, &defaults(cx), w, h)
}

/// A parsed chart's items in a `w` × `h` frame, as [`chart_items`]: for
/// callers that change the chart first (Sheets puts live cell values in
/// its series). `None` when Kantor can't draw it.
pub fn items(
    engine: &mut Engine,
    chart: &Chart,
    d: &Defaults,
    w: f32,
    h: f32,
) -> Option<Vec<Item>> {
    if !chart.drawable() || w < 2.0 || h < 2.0 {
        return None;
    }
    let marks = layout::layout(chart, w, h, d, &mut EngineMeasure::new(engine));
    Some(
        marks
            .into_iter()
            .filter_map(|m| match m {
                Mark::Prim { prim, at } => Some(Item {
                    layer: None,
                    transform: at,
                    prim,
                }),
                Mark::Text(l) => label_item(engine, &l),
            })
            .collect(),
    )
}

fn label_item(engine: &mut Engine, l: &Label) -> Option<Item> {
    // A little slack so the measured width doesn't wrap.
    let width = l.w + 1.0;
    let laid = lay(engine, &l.text, &l.font, l.align, width);
    let runs = glyph_runs(&laid);
    if runs.is_empty() {
        return None;
    }
    let dx = match l.align {
        HAlign::Left => 0.0,
        HAlign::Center => -0.5,
        HAlign::Right => -1.0,
    };
    let (cx, cy) = (l.w / 2.0, l.h / 2.0);
    let transform = Affine::translate(-cx, -cy)
        .then(Affine::rotate(l.rot.to_radians()))
        .then(Affine::translate(l.x + cx, l.y + cy));
    Some(Item {
        layer: None,
        transform,
        prim: Prim::Text(Text {
            origin: p(dx, 0.0),
            runs,
        }),
    })
}

#[cfg(test)]
mod tests;
