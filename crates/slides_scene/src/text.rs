//! Text bodies laid out with the Docs engine (parley): resolved paragraphs
//! become `docs_layout` paragraphs, laid out once per text and width and
//! cached. The canvas uses the layout for carets and hit-testing; scenes
//! take its glyph runs.

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::Arc;

use docs_layout::parley::LineHeight;
use docs_layout::{Engine, GlyphRun, Marker, PageSetup, PagedDocument, Rgba, RunSource, Span};
use slides_engine::render::LineSpacing;
use slides_engine::{Anchor, EMU_PER_PT, EMU_PER_PX, RenderText, Rgb, Xfrm};

/// A text body laid out in slide pixels (EMU / 9525), origin at the top
/// left of the box's inner (inset) area.
pub struct TextLayout {
    pub paged: PagedDocument,
    pub texts: Vec<String>,
    /// Height of the text.
    pub height: f32,
}

/// A laid-out text body placed in its shape.
#[derive(Clone)]
pub struct LaidText {
    pub layout: Rc<TextLayout>,
    /// Offset of the layout's origin from the shape's top left (slide px).
    pub dx: f32,
    pub dy: f32,
}

fn rgba(c: Rgb, alpha: u8) -> Rgba {
    let [r, g, b] = c.0;
    Rgba([r, g, b, alpha])
}

/// Layout paragraphs for a resolved text body.
pub fn layout_paragraphs(t: &RenderText) -> Vec<docs_layout::Paragraph> {
    // Prompts are the placeholder's own colour, faded to a muted grey.
    let alpha = if t.prompt { 110 } else { 255 };
    t.paras
        .iter()
        .map(|p| {
            let mut lp = docs_layout::Paragraph::new(p.text.clone());
            lp.family = docs_engine::font_stack(&p.base.font);
            lp.size_pt = p.base.size_pt;
            lp.line_height = Some(match p.line {
                Some(LineSpacing::Multiple(m)) => LineHeight::MetricsRelative(m.max(0.1)),
                // Layout units are slide px (1 pt = 4/3 px).
                Some(LineSpacing::Points(pt)) => {
                    LineHeight::Absolute(pt.max(1.0) * EMU_PER_PT / EMU_PER_PX)
                }
                None => LineHeight::MetricsRelative(1.0),
            });
            lp.space_before_pt = p.space_before_pt;
            lp.space_after_pt = p.space_after_pt;
            lp.indent_left_pt = p.indent_pt.max(0.0);
            lp.first_line_pt = p.first_line_pt.max(-p.indent_pt.max(0.0));
            lp.marker = p.marker.as_ref().map(|m| Marker {
                text: m.text.clone(),
                style: Span {
                    range: 0..0,
                    bold: m.bold,
                    italic: m.italic,
                    underline: false,
                    size_pt: Some(m.size_pt),
                    color: Some(rgba(m.color, alpha)),
                    family: Some(docs_engine::font_stack(&m.font)),
                },
            });
            lp.align = match p.align {
                slides_engine::Align::Left => docs_layout::Align::Start,
                slides_engine::Align::Center => docs_layout::Align::Center,
                slides_engine::Align::Right => docs_layout::Align::End,
                slides_engine::Align::Justify => docs_layout::Align::Justify,
            };
            lp.spans = p
                .runs
                .iter()
                .map(|r| Span {
                    range: r.range.clone(),
                    bold: r.bold,
                    italic: r.italic,
                    underline: r.underline,
                    size_pt: Some(r.size_pt),
                    color: Some(rgba(r.color, alpha)),
                    family: Some(docs_engine::font_stack(&r.font)),
                })
                .collect();
            lp
        })
        .collect()
}

fn layout_key(paras: &[docs_layout::Paragraph], width: f32) -> u64 {
    let mut h = DefaultHasher::new();
    width.to_bits().hash(&mut h);
    for p in paras {
        p.text.hash(&mut h);
        p.family.hash(&mut h);
        p.size_pt.to_bits().hash(&mut h);
        (p.align as u8).hash(&mut h);
        for v in [
            p.space_before_pt,
            p.space_after_pt,
            p.indent_left_pt,
            p.first_line_pt,
        ] {
            v.to_bits().hash(&mut h);
        }
        format!("{:?}", p.line_height).hash(&mut h);
        if let Some(m) = &p.marker {
            m.text.hash(&mut h);
            m.style.size_pt.map(f32::to_bits).hash(&mut h);
            m.style.color.hash(&mut h);
            m.style.family.hash(&mut h);
            (m.style.bold, m.style.italic).hash(&mut h);
        }
        for s in &p.spans {
            s.range.hash(&mut h);
            (s.bold, s.italic, s.underline).hash(&mut h);
            s.size_pt.map(f32::to_bits).hash(&mut h);
            s.color.hash(&mut h);
            s.family.hash(&mut h);
        }
    }
    h.finish()
}

/// Width of a text body's layout area in slide px.
pub fn text_width(t: &RenderText, xfrm: &Xfrm) -> f32 {
    if !t.wrap {
        return 100_000.0;
    }
    ((xfrm.cx - t.insets[0] - t.insets[2]) as f32 / EMU_PER_PX).max(1.0)
}

/// Text layouts by content and width.
#[derive(Default)]
pub struct TextCache {
    texts: RefCell<HashMap<u64, Rc<TextLayout>>>,
}

impl TextCache {
    pub fn layout(&self, engine: &mut Engine, t: &RenderText, xfrm: &Xfrm) -> Rc<TextLayout> {
        let paras = layout_paragraphs(t);
        let width = text_width(t, xfrm);
        let key = layout_key(&paras, width);
        if let Some(l) = self.texts.borrow().get(&key) {
            return l.clone();
        }
        let setup = PageSetup {
            width,
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
        let layout = Rc::new(TextLayout {
            paged,
            texts: paras.into_iter().map(|p| p.text).collect(),
            height,
        });
        let mut cache = self.texts.borrow_mut();
        if cache.len() > 2000 {
            cache.clear();
        }
        cache.insert(key, layout.clone());
        layout
    }

    /// Text placement of a resolved body inside `xfrm`.
    pub fn place(&self, engine: &mut Engine, t: &RenderText, xfrm: &Xfrm) -> LaidText {
        let layout = self.layout(engine, t, xfrm);
        let inner_h = (xfrm.cy - t.insets[1] - t.insets[3]) as f32 / EMU_PER_PX;
        let free = inner_h - layout.height;
        let dy = t.insets[1] as f32 / EMU_PER_PX
            + match t.anchor {
                Anchor::Top => 0.0,
                Anchor::Middle => free / 2.0,
                Anchor::Bottom => free,
            };
        LaidText {
            layout,
            dx: t.insets[0] as f32 / EMU_PER_PX,
            dy,
        }
    }
}

/// A glyph run and the text its clusters index (for ToUnicode, and to
/// pick a fallback face on screen).
#[derive(Clone, Debug)]
pub struct TextRun {
    pub run: GlyphRun,
    pub text: Arc<str>,
}

impl TextRun {
    /// The text this run shows.
    pub fn sample(&self) -> &str {
        self.text
            .get(self.run.text_range.clone())
            .unwrap_or_default()
    }
}

/// The glyph runs of a laid-out text body (layout coordinates).
pub fn glyph_runs(layout: &TextLayout) -> Vec<TextRun> {
    let paged = &layout.paged;
    if paged.pages.is_empty() {
        return Vec::new();
    }
    let mut paras: HashMap<usize, Arc<str>> = HashMap::new();
    let mut markers: HashMap<usize, Arc<str>> = HashMap::new();
    paged
        .glyph_runs(0)
        .into_iter()
        .filter_map(|run| {
            let text = match run.source {
                RunSource::Text => paras
                    .entry(run.para)
                    .or_insert_with(|| {
                        Arc::from(layout.texts.get(run.para).map_or("", String::as_str))
                    })
                    .clone(),
                RunSource::Marker => markers
                    .entry(run.para)
                    .or_insert_with(|| Arc::from(paged.run_text(&run, "")))
                    .clone(),
                RunSource::Label(_) => return None,
            };
            (!run.glyphs.is_empty()).then_some(TextRun { run, text })
        })
        .collect()
}
