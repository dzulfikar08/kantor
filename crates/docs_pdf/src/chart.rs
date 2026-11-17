//! Charts in floating graphic frames (`c:chart`), drawn with Slides' chart
//! code (`slides_scene::chart`: ChartML model, Excel-like axes, layout) from
//! the values cached in the chart part, as Word shows a chart without
//! opening its embedded workbook. Fills and strokes become the floating
//! object's paths; labels are laid out with the Docs engine like any other
//! text (turned ones, such as a vertical axis title, are turned when
//! painted).

use std::sync::Arc;

use docs_engine::{Document, Float, FloatKind};
use docs_layout::{PageSetup, PagedDocument};
use slides_scene::chart::{self, Chart, HAlign, Label, Mark, Measure, TextFont};
use slides_scene::{Affine, Paint, Prim};

use crate::floats::{DrawPath, FloatPrim, Painter, Seg};
use crate::pages::{LayoutFn, StoryLayout};

/// A chart's drawing on the page.
#[derive(Clone)]
pub struct ChartDraw {
    pub prims: Vec<FloatPrim>,
    pub labels: Vec<ChartLabel>,
}

/// A chart label: its text laid out, with where its layout's (0, 0) goes
/// (page px), and its turn about the centre of `rect`.
#[derive(Clone)]
pub struct ChartLabel {
    pub layout: Arc<StoryLayout>,
    pub x: f32,
    pub y: f32,
    pub rot: f32,
    pub rect: [f32; 4],
}

/// The chart a floating object at `rect` shows, if it is one Kantor can
/// draw. `anchor` is its anchor (story, paragraph, byte index).
pub fn chart_draw(
    doc: &Document,
    f: &Float,
    anchor: (Option<usize>, usize, usize),
    rect: [f32; 4],
    painter: &Painter,
    layout: &mut LayoutFn,
) -> Option<ChartDraw> {
    let FloatKind::Chart { part, .. } = &f.kind else {
        return None;
    };
    let xml = doc.part_xml(part)?;
    let [x0, y0, w, h] = rect;
    if w < 2.0 || h < 2.0 {
        return None;
    }
    let (mut chart, defaults) = painter.chart(&xml)?;
    word_defaults(&mut chart, &defaults);
    if !chart.drawable() {
        return None;
    }
    let marks = chart::layout::layout(&chart, w, h, &defaults, &mut LayoutMeasure { layout });
    let mut out = ChartDraw {
        prims: Vec::new(),
        labels: Vec::new(),
    };
    let place = Affine::translate(x0, y0);
    for m in marks {
        match m {
            Mark::Prim { prim, at } => {
                let t = at.then(place);
                match prim {
                    Prim::Fill { path, paint, .. } => {
                        let color = paint.main_color();
                        // Gradients and patterns (column and area fills) on the page.
                        let gradient = match paint {
                            Paint::Solid(_) => None,
                            other => Some(other.mapped(&t)),
                        };
                        out.prims.push(FloatPrim::Path(DrawPath {
                            segs: segs(&path, &t),
                            fill: Some(color),
                            stroke: None,
                            gradient,
                        }));
                    }
                    Prim::Stroke { path, stroke } => {
                        let k = (t.a * t.a + t.b * t.b).sqrt().max(1e-3);
                        out.prims.push(FloatPrim::Path(DrawPath {
                            segs: segs(&path, &t),
                            fill: None,
                            stroke: Some((
                                stroke.color,
                                stroke.width * k,
                                stroke.dash.map(|d| d.iter().map(|v| v * k).collect()),
                            )),
                            gradient: None,
                        }));
                    }
                    _ => {}
                }
            }
            Mark::Text(l) => {
                if let Some(label) = label(l, (x0, y0), anchor, layout) {
                    out.labels.push(label);
                }
            }
        }
    }
    Some(out)
}

/// What Word (like Excel) uses where a chart part says nothing (the
/// renderer's own defaults are PowerPoint's): 10 pt text with a 14 pt
/// title, the chart area filled with the background inside a light grey
/// border.
fn word_defaults(chart: &mut Chart, d: &chart::Defaults) {
    use slides_engine::Fill;
    use slides_scene::chart::model::Line;
    chart.font.size_pt.get_or_insert(10.0);
    if let Some(t) = chart.title.as_mut() {
        t.font.size_pt.get_or_insert(14.0);
    }
    if chart.space_look.fill.is_none() {
        chart.space_look.fill = Some(Fill::Solid(d.background, 255));
    }
    if chart.space_look.line.is_none() {
        chart.space_look.line = Some(Line {
            fill: Some(Fill::Solid(docs_engine::Rgb([0xD9, 0xD9, 0xD9]), 255)),
            width: Some(1.0),
            style: Default::default(),
        });
    }
}

fn segs(path: &slides_scene::Path, t: &Affine) -> Vec<Seg> {
    let q = |p: slides_scene::P| {
        let r = t.apply(p);
        [r.x, r.y]
    };
    path.segs
        .iter()
        .map(|s| match *s {
            slides_scene::Seg::Move(a) => Seg::Move(q(a)),
            slides_scene::Seg::Line(a) => Seg::Line(q(a)),
            slides_scene::Seg::Quad(c, a) => Seg::Quad(q(c), q(a)),
            slides_scene::Seg::Cubic(c1, c2, a) => Seg::Cubic(q(c1), q(c2), q(a)),
            slides_scene::Seg::Close => Seg::Close,
        })
        .collect()
}

/// A label's lines as Docs layout paragraphs.
fn paragraphs(text: &str, font: &TextFont, align: HAlign) -> Vec<docs_layout::Paragraph> {
    text.split('\n')
        .map(|line| {
            let mut p = docs_layout::Paragraph::new(line.to_owned());
            p.family = docs_engine::font_stack(&font.face);
            p.size_pt = font.size_pt;
            p.line_height = Some(docs_layout::parley::LineHeight::MetricsRelative(1.0));
            p.space_after_pt = 0.0;
            p.align = match align {
                HAlign::Left => docs_layout::Align::Start,
                HAlign::Center => docs_layout::Align::Center,
                HAlign::Right => docs_layout::Align::End,
            };
            p.spans = vec![docs_layout::Span {
                range: 0..line.len(),
                bold: font.bold,
                italic: font.italic,
                underline: false,
                size_pt: Some(font.size_pt),
                color: Some(font.color),
                family: Some(docs_engine::font_stack(&font.face)),
            }];
            p
        })
        .collect()
}

fn setup(width: f32) -> PageSetup {
    PageSetup {
        width: width.max(1.0),
        height: 1.0e7,
        margin_top: 0.0,
        margin_bottom: 0.0,
        margin_left: 0.0,
        margin_right: 0.0,
    }
}

/// `text` laid out `width` px wide: the pages and its height.
fn lay(
    text: &str,
    font: &TextFont,
    align: HAlign,
    width: f32,
    layout: &mut LayoutFn,
) -> (PagedDocument, Vec<docs_layout::Paragraph>, f32) {
    let paras = paragraphs(text, font, align);
    let layouts = paras.iter().map(|p| layout(p, width.max(1.0))).collect();
    let paged = PagedDocument::paginate(&paras, layouts, setup(width));
    let height = paged
        .pages
        .first()
        .and_then(|p| p.lines.last())
        .map_or(0.0, |l| l.top + l.height);
    (paged, paras, height)
}

fn label(
    l: Label,
    (x0, y0): (f32, f32),
    anchor: (Option<usize>, usize, usize),
    layout: &mut LayoutFn,
) -> Option<ChartLabel> {
    // A little slack so the measured width doesn't wrap.
    let width = l.w + 1.0;
    let (paged, paras, height) = lay(&l.text, &l.font, l.align, width, layout);
    let x = x0
        + l.x
        + match l.align {
            HAlign::Left => 0.0,
            HAlign::Center => -0.5,
            HAlign::Right => -1.0,
        };
    let y = y0 + l.y;
    let story = StoryLayout {
        story: 0,
        note: None,
        text_box: Some(anchor),
        group_box: None,
        paged,
        paras: paras
            .iter()
            .map(|p| docs_engine::Paragraph::new(p.text.clone()))
            .collect(),
        height,
        strikes: Arc::default(),
    };
    Some(ChartLabel {
        layout: Arc::new(story),
        x,
        y,
        rot: l.rot,
        rect: [x0 + l.x, y, l.w, l.h],
    })
}

/// Measures labels with the layout function (the Docs engine).
struct LayoutMeasure<'a, 'b> {
    layout: &'a mut LayoutFn<'b>,
}

impl Measure for LayoutMeasure<'_, '_> {
    fn measure(&mut self, text: &str, font: &TextFont, wrap: Option<f32>) -> (f32, f32) {
        let width = wrap.unwrap_or(100_000.0);
        let paras = paragraphs(text, font, HAlign::Left);
        let mut w = 0.0f32;
        let mut h = 0.0f32;
        for p in &paras {
            let l = (self.layout)(p, width);
            w = w.max(l.layout.width());
            h += l.layout.height();
        }
        (w, h)
    }
}

impl Painter {
    /// A chart part parsed with the document's theme, and the theme's
    /// chart colours and fonts.
    pub(crate) fn chart(&self, xml: &str) -> Option<(Chart, chart::Defaults)> {
        let cx = self.ctx();
        let chart = Chart::parse(xml, &cx)?;
        Some((chart, chart::defaults(&cx)))
    }
}
