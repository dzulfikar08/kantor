//! Display resolution: what to draw for a slide, with positions and text
//! formatting inherited from the layout, the master and the theme
//! (read-only). The UI lays out and paints the result; nothing here is
//! written to the file.

use std::ops::Range;

use docs_engine::Rgb;

use crate::fill::Fill;
use crate::frame::{Cell, DiagramRef, FrameContent, Group, Vert};
use crate::model::*;
use crate::presentation::{Layout, Master, Presentation, Slide};
use crate::theme::{LevelStyle, ListStyle};
use crate::xml::{is, parse_rels, rels_name};

/// Paragraph numbering through a text body: one counter per outline
/// level; a shallower paragraph restarts the deeper ones, a paragraph at
/// the same level without a number ends the list.
#[derive(Default)]
struct Numbering {
    counters: [Option<(String, u32)>; 9],
}

/// What a paragraph's marker shows.
enum Shown {
    Char(String),
    Number(String),
}

impl Numbering {
    /// The marker for the next paragraph. Empty paragraphs don't take a number.
    fn next(&mut self, level: u8, bullet: Option<&Bullet>, counts: bool) -> Option<Shown> {
        let l = (level as usize).min(8);
        for c in &mut self.counters[l + 1..] {
            *c = None;
        }
        match bullet? {
            Bullet::None => {
                self.counters[l] = None;
                None
            }
            Bullet::Char(c) => {
                self.counters[l] = None;
                Some(Shown::Char(c.clone()))
            }
            Bullet::Picture(_) => {
                self.counters[l] = None;
                Some(Shown::Char("•".into()))
            }
            Bullet::AutoNum { scheme, start } => {
                let n = match &self.counters[l] {
                    Some((s, n)) if s == scheme => n + 1,
                    _ => *start,
                };
                if counts {
                    self.counters[l] = Some((scheme.clone(), n));
                }
                Some(Shown::Number(auto_number(scheme, n)))
            }
        }
    }
}

fn roman(mut n: u32) -> String {
    const T: [(u32, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut s = String::new();
    for (v, r) in T {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn alpha(n: u32) -> String {
    // a..z, then aa..zz, aaa... (as PowerPoint).
    let n = n.max(1) - 1;
    let letter = (b'a' + (n % 26) as u8) as char;
    letter.to_string().repeat(n as usize / 26 + 1)
}

/// The text of number `n` in an `a:buAutoNum` scheme (`arabicPeriod` →
/// "3.", `alphaUcParenR` → "C)", `romanLcParenBoth` → "(iii)").
pub fn auto_number(scheme: &str, n: u32) -> String {
    let (digits, rest) = if let Some(r) = scheme.strip_prefix("arabic") {
        (n.to_string(), r)
    } else if let Some(r) = scheme.strip_prefix("alphaLc") {
        (alpha(n), r)
    } else if let Some(r) = scheme.strip_prefix("alphaUc") {
        (alpha(n).to_uppercase(), r)
    } else if let Some(r) = scheme.strip_prefix("romanLc") {
        (roman(n), r)
    } else if let Some(r) = scheme.strip_prefix("romanUc") {
        (roman(n).to_uppercase(), r)
    } else {
        // Circled, East Asian and other schemes: plain numbers.
        (n.to_string(), "Period")
    };
    match rest {
        "ParenR" => format!("{digits})"),
        "ParenBoth" => format!("({digits})"),
        "Plain" => digits,
        "Minus" => format!("- {digits} -"),
        _ => format!("{digits}."),
    }
}

/// A bullet character as Unicode: symbol fonts (Wingdings, Symbol) map
/// their private code points to common shapes. Returns the text and
/// whether the bullet font can be used to draw it.
pub fn bullet_text(c: &str, font: Option<&str>) -> (String, bool) {
    let symbol = font.is_some_and(|f| {
        let f = f.to_ascii_lowercase();
        f.starts_with("wingdings") || f == "symbol" || f.starts_with("webdings")
    });
    if !symbol {
        return (c.to_owned(), true);
    }
    let ch = c.chars().next().unwrap_or('•');
    // Symbol fonts are often addressed in the U+F0xx private area.
    let code = match ch as u32 {
        v @ 0xF020..=0xF0FF => v - 0xF000,
        v => v,
    };
    let mapped = match code {
        0xA7 | 0x6E => '■',
        0x6C | 0xB7 => '●',
        0xD8 => '➢',
        0xFC => '✓',
        0x71 => '❑',
        0x76 => '❖',
        0x75 => '◆',
        0xE0 | 0xE8 => '➔',
        0xA8 => '◻',
        0x77 => '⬥',
        _ => '•',
    };
    (mapped.to_string(), false)
}

/// Indents, spacing and the marker of a paragraph from its effective style.
fn apply_para_style(
    p: &mut RenderPara,
    st: &ParaStyle,
    shown: Option<Shown>,
    first: bool,
    scale: f32,
    reduction: Option<u32>,
) {
    p.indent_pt = st.mar_l.unwrap_or(0) as f32 / EMU_PER_PT;
    p.first_line_pt = st.indent.unwrap_or(0) as f32 / EMU_PER_PT;
    let line_pt = p.base.size_pt * 1.2;
    let space = |s: Option<Spacing>| match s {
        Some(Spacing::Pct(v)) => v as f32 / 100_000.0 * line_pt,
        Some(Spacing::Pts(v)) => v as f32 / 100.0 * scale,
        None => 0.0,
    };
    // PowerPoint doesn't add space before a text body's first paragraph.
    p.space_before_pt = if first { 0.0 } else { space(st.space_before) };
    p.space_after_pt = space(st.space_after);
    let reduce = 1.0 - reduction.unwrap_or(0) as f32 / 100_000.0;
    p.line = match st.line_spacing {
        Some(Spacing::Pct(v)) => Some(LineSpacing::Multiple(v as f32 / 100_000.0 * reduce)),
        Some(Spacing::Pts(v)) => Some(LineSpacing::Points(v as f32 / 100.0 * scale)),
        None if reduce < 1.0 => Some(LineSpacing::Multiple(reduce)),
        None => None,
    };
    let Some(shown) = shown else {
        return;
    };
    let base = &p.base;
    let bu_font = st.bu_font.clone().flatten();
    let (text, own_font) = match shown {
        Shown::Char(c) => bullet_text(&c, bu_font.as_deref()),
        Shown::Number(n) => (n, true),
    };
    let size_pt = match st.bu_size.flatten() {
        Some(Spacing::Pct(v)) => base.size_pt * v as f32 / 100_000.0,
        Some(Spacing::Pts(v)) => v as f32 / 100.0 * scale,
        None => base.size_pt,
    };
    p.marker = Some(RenderMarker {
        text,
        size_pt: size_pt.max(1.0),
        color: st.bu_color.flatten().unwrap_or(base.color),
        font: bu_font
            .filter(|_| own_font)
            .unwrap_or_else(|| base.font.clone()),
        bold: base.bold,
        italic: base.italic,
    });
}

/// Which part a drawn shape belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Master,
    Layout,
    /// A slide shape and its index in [`Slide::shapes`].
    Slide(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderRun {
    pub range: Range<usize>,
    pub size_pt: f32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub color: Rgb,
    pub font: String,
}

/// Line spacing of a paragraph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineSpacing {
    /// A multiple of the font's single line height.
    Multiple(f32),
    /// Exactly this many points.
    Points(f32),
}

/// A bullet or number drawn in the paragraph's hanging indent.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderMarker {
    pub text: String,
    pub size_pt: f32,
    pub color: Rgb,
    pub font: String,
    pub bold: bool,
    pub italic: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderPara {
    pub text: String,
    pub align: Align,
    pub level: u8,
    pub runs: Vec<RenderRun>,
    /// Formatting of an empty paragraph (its height) and of the caret.
    pub base: RenderRun,
    pub marker: Option<RenderMarker>,
    /// Left margin of the text and the first line's offset from it
    /// (negative = hanging), points.
    pub indent_pt: f32,
    pub first_line_pt: f32,
    pub space_before_pt: f32,
    pub space_after_pt: f32,
    /// `None` = single.
    pub line: Option<LineSpacing>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderText {
    pub paras: Vec<RenderPara>,
    /// Left, top, right, bottom (EMU).
    pub insets: [Emu; 4],
    pub anchor: Anchor,
    pub wrap: bool,
    /// Placeholder prompt ("Click to add title"), not real text.
    pub prompt: bool,
    /// Turned a quarter (`bodyPr/@vert`).
    pub vert: Vert,
}

#[derive(Clone, Debug)]
pub struct RenderShape<'a> {
    pub shape: &'a Shape,
    pub layer: Layer,
    pub xfrm: Xfrm,
    pub text: Option<RenderText>,
}

impl RenderShape<'_> {
    /// A slide placeholder with no text yet (title, body, subtitle...; not
    /// date/footer/slide number). The editor outlines these with a dashed
    /// line and shows a prompt in them; neither is ever saved.
    pub fn is_empty_placeholder(&self) -> bool {
        matches!(self.layer, Layer::Slide(_))
            && self.shape.ph.as_ref().is_some_and(|p| !p.is_footer())
            && self.shape.text.as_ref().is_some_and(|t| t.is_empty())
    }
}

/// Text goes in the geometry's text rectangle (`a:rect`: the inside of a
/// star, a callout's balloon...); the insets apply inside it.
fn apply_text_rect(t: &mut RenderText, shape: &Shape, xfrm: &Xfrm) {
    let Some(g) = shape.kind.geometry() else {
        return;
    };
    if g.preset_name() == Some("rect") || xfrm.cx <= 0 || xfrm.cy <= 0 {
        return;
    }
    let (w, h) = (xfrm.cx as f64, xfrm.cy as f64);
    let r = crate::geom::outline(&g, w, h).text_rect;
    let ok = |v: f64| v.is_finite();
    if !(ok(r.l) && ok(r.t) && ok(r.r) && ok(r.b)) || r.r <= r.l || r.b <= r.t {
        return;
    }
    let edge = |v: f64, max: f64| v.clamp(0.0, max).round() as Emu;
    t.insets[0] += edge(r.l, w);
    t.insets[1] += edge(r.t, h);
    t.insets[2] += edge(w - r.r, w);
    t.insets[3] += edge(h - r.b, h);
}

/// The placeholder in `shapes` that `ph` inherits from.
fn find_ph<'a>(shapes: &'a [Shape], ph: &Placeholder, by_idx: bool) -> Option<&'a Shape> {
    let phs = || shapes.iter().filter_map(|s| Some((s, s.ph.as_ref()?)));
    if by_idx
        && let Some(idx) = ph.idx
        && let Some((s, _)) = phs().find(|(_, p)| p.idx == Some(idx))
    {
        return Some(s);
    }
    let norm = |t: &str| -> &'static str {
        match t {
            "title" | "ctrTitle" => "title",
            "dt" => "dt",
            "ftr" => "ftr",
            "sldNum" => "sldNum",
            "hdr" => "hdr",
            "pic" => "pic",
            _ => "body",
        }
    };
    let want = ph.ty();
    phs()
        .find(|(_, p)| p.ty() == want)
        .or_else(|| phs().find(|(_, p)| norm(p.ty()) == norm(want)))
        .map(|(s, _)| s)
}

fn generic_prompt(ph: &Placeholder) -> Option<&'static str> {
    Some(match ph.ty() {
        "title" | "ctrTitle" => "Click to add title",
        "subTitle" => "Click to add subtitle",
        "body" | "obj" => "Click to add text",
        _ => return None,
    })
}

impl Presentation {
    pub fn layout_of(&self, slide: usize) -> &Layout {
        &self.layouts[self.slides[slide].layout.min(self.layouts.len() - 1)]
    }

    pub fn master_of(&self, slide: usize) -> &Master {
        &self.masters[self.layout_of(slide).master]
    }

    /// The slide's background colour (white when it has none we can show).
    pub fn background(&self, slide: usize) -> Rgb {
        let s = &self.slides[slide];
        s.bg.or(self.layout_of(slide).bg)
            .or(self.master_of(slide).bg)
            .unwrap_or(Rgb([255, 255, 255]))
    }

    /// The slide's background fill: its own, its layout's or its master's
    /// (white when none has one).
    pub fn background_fill(&self, slide: usize) -> Fill {
        let s = &self.slides[slide];
        s.background
            .clone()
            .or_else(|| self.layout_of(slide).background.clone())
            .or_else(|| self.master_of(slide).background.clone())
            .unwrap_or(Fill::Solid(Rgb([255, 255, 255]), 255))
    }

    /// Kinds of objects on the shown slides that are kept but not drawn
    /// (tables, charts, groups...), each once.
    pub fn unsupported_objects(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for i in (0..self.len()).filter(|&i| !self.slides[i].hidden) {
            for r in self.render_list(i, false) {
                if let ShapeKind::Other { what, frame } = &r.shape.kind
                    && !self.drawable(i, frame)
                    && !out.contains(what)
                {
                    out.push(what.clone());
                }
            }
        }
        out
    }

    /// The layout and master placeholders a slide shape inherits from.
    fn inherited(&self, slide: usize, shape: &Shape) -> (Option<&Shape>, Option<&Shape>) {
        let Some(ph) = &shape.ph else {
            return (None, None);
        };
        let layout = self.layout_of(slide);
        let lph = find_ph(&layout.shapes, ph, true);
        let mph = find_ph(
            &self.masters[layout.master].shapes,
            lph.and_then(|l| l.ph.as_ref()).unwrap_or(ph),
            false,
        );
        (lph, mph)
    }

    /// Where a slide shape is drawn: its own position, else its layout's
    /// or master's placeholder.
    pub fn effective_xfrm(&self, slide: usize, shape: &Shape) -> Option<Xfrm> {
        if shape.xfrm.is_some() {
            return shape.xfrm;
        }
        let (l, m) = self.inherited(slide, shape);
        l.and_then(|s| s.xfrm).or(m.and_then(|s| s.xfrm))
    }

    /// The list style and body properties a slide shape's text inherits.
    fn text_chain(&self, slide: usize, shape: &Shape) -> (ListStyle, BodyProps) {
        let master = self.master_of(slide);
        let mut ls = match shape.ph.as_ref().map(Placeholder::ty) {
            Some("title" | "ctrTitle") => master.title_style.clone(),
            Some(
                "body" | "obj" | "subTitle" | "pic" | "tbl" | "chart" | "media" | "clipArt" | "dgm",
            ) => master.body_style.clone(),
            Some(_) => master.other_style.clone(),
            None => {
                let mut ls = master.other_style.clone();
                ls.overlay(&self.default_style);
                ls
            }
        };
        let mut body = BodyProps::default();
        let (l, m) = self.inherited(slide, shape);
        for s in [m, l].into_iter().flatten() {
            if let Some(t) = &s.text {
                ls.overlay(&t.list);
                body.overlay(&t.body);
            }
        }
        if let Some(c) = shape.look.text_color {
            for lvl in &mut ls.0 {
                lvl.color = Some(c);
            }
        }
        if let Some(t) = &shape.text {
            ls.overlay(&t.list);
            body.overlay(&t.body);
        }
        (ls, body)
    }

    fn resolve(&self, slide: usize, level: &LevelStyle, props: &RunProps, scale: f32) -> RenderRun {
        let master = self.master_of(slide);
        let mut s = level.clone();
        let explicit = LevelStyle {
            align: None,
            size: props.size,
            bold: props.bold,
            italic: props.italic,
            underline: props.underline.or(props.shown_underline.then_some(true)),
            color: props.color.or(props.shown_color),
            font: props.shown_font.clone(),
            para: ParaStyle::default(),
        };
        s.overlay(&explicit);
        RenderRun {
            range: 0..0,
            size_pt: s.size.unwrap_or(1800) as f32 / 100.0 * scale,
            bold: s.bold.unwrap_or(false),
            italic: s.italic.unwrap_or(false),
            underline: s.underline.unwrap_or(false),
            color: s.color.unwrap_or(Rgb([0, 0, 0])),
            font: s
                .font
                .or_else(|| master.theme.minor_font.clone())
                .unwrap_or_else(|| "Calibri".into()),
        }
    }

    /// Resolved formatting of `props` in paragraph `para` of a slide shape.
    pub fn resolve_run(
        &self,
        slide: usize,
        shape: &Shape,
        para: usize,
        props: &RunProps,
    ) -> RenderRun {
        let (ls, _) = self.text_chain(slide, shape);
        let level = shape
            .text
            .as_ref()
            .and_then(|t| t.paras.get(para))
            .map_or(0, |p| p.props.level as usize);
        self.resolve(slide, &ls.0[level.min(8)], props, 1.0)
    }

    /// Effective paragraph formatting of paragraph `para` of a slide shape
    /// (list styles, then the paragraph's own `a:pPr` unless `own` is false).
    pub fn para_style(&self, slide: usize, shape: &Shape, para: usize, own: bool) -> ParaStyle {
        let (ls, _) = self.text_chain(slide, shape);
        let p = shape.text.as_ref().and_then(|t| t.paras.get(para));
        let level = p.map_or(0, |p| p.props.level as usize).min(8);
        let mut st = ls.0[level].para.clone();
        if own && let Some(p) = p {
            st.overlay(&p.props.style);
        }
        st
    }

    fn render_text(&self, slide: usize, shape: &Shape, prompts: bool) -> Option<RenderText> {
        let body = shape.text.as_ref()?;
        let (ls, bp) = self.text_chain(slide, shape);
        let scale = bp.font_scale.map_or(1.0, |s| s as f32 / 100_000.0);
        let empty_ph = shape
            .ph
            .as_ref()
            .filter(|p| !p.is_footer() && body.is_empty());
        let mut prompt = false;
        let paras: Vec<RenderPara> = match empty_ph.filter(|_| prompts) {
            Some(ph) => {
                let (lph, _) = self.inherited(slide, shape);
                let text = lph
                    .and_then(|l| l.text.as_ref())
                    .map(|t| t.plain_text())
                    .filter(|t| !t.trim().is_empty() && !t.starts_with("Click to edit"))
                    .or_else(|| generic_prompt(ph).map(str::to_owned));
                let text = text?;
                prompt = true;
                let level = &ls.0[0];
                let mut base = self.resolve(slide, level, &body.paras[0].runs[0].props, scale);
                let text = text.replace('\n', " ");
                base.range = 0..text.len();
                let mut st = level.para.clone();
                st.overlay(&body.paras[0].props.style);
                let mut numbers = Numbering::default();
                let shown = numbers.next(0, st.bullet.as_ref(), true);
                let mut para = RenderPara {
                    align: body.paras[0]
                        .props
                        .align
                        .or(level.align)
                        .unwrap_or_default(),
                    level: 0,
                    runs: vec![base.clone()],
                    base,
                    text,
                    marker: None,
                    indent_pt: 0.0,
                    first_line_pt: 0.0,
                    space_before_pt: 0.0,
                    space_after_pt: 0.0,
                    line: None,
                };
                apply_para_style(&mut para, &st, shown, true, scale, bp.line_reduction);
                vec![para]
            }
            None => self.render_paras(slide, body, &ls, scale, bp.line_reduction, prompts),
        };
        let d = [91_440, 45_720, 91_440, 45_720];
        Some(RenderText {
            paras,
            insets: std::array::from_fn(|i| bp.insets[i].unwrap_or(d[i])),
            anchor: bp.anchor.unwrap_or_default(),
            wrap: bp.no_wrap != Some(true),
            prompt,
            vert: bp.vert.unwrap_or_default(),
        })
    }

    /// The paragraphs of a text body with list style `ls`.
    fn render_paras(
        &self,
        slide: usize,
        body: &TextBody,
        ls: &ListStyle,
        scale: f32,
        line_reduction: Option<u32>,
        prompts: bool,
    ) -> Vec<RenderPara> {
        let mut numbers = Numbering::default();
        body.paras
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let level = &ls.0[p.props.level.min(8) as usize];
                let mut st = level.para.clone();
                st.overlay(&p.props.style);
                let runs: Vec<RenderRun> = p
                    .run_ranges()
                    .filter(|(r, _)| !r.is_empty())
                    .map(|(range, run)| RenderRun {
                        range,
                        ..self.resolve(slide, level, &run.props, scale)
                    })
                    .collect();
                // The bullet takes the first character's formatting.
                let first = p.runs.iter().find(|r| r.len > 0).unwrap_or(&p.runs[0]);
                let has_text = !p.text.is_empty();
                let shown = numbers.next(p.props.level, st.bullet.as_ref(), has_text);
                let mut para = RenderPara {
                    text: p.text.clone(),
                    align: p.props.align.or(level.align).unwrap_or_default(),
                    level: p.props.level,
                    base: self.resolve(slide, level, &first.props, scale),
                    runs,
                    marker: None,
                    indent_pt: 0.0,
                    first_line_pt: 0.0,
                    space_before_pt: 0.0,
                    space_after_pt: 0.0,
                    line: None,
                };
                // Empty paragraphs show their bullet only while editing.
                let shown = shown.filter(|_| has_text || prompts);
                apply_para_style(&mut para, &st, shown, i == 0, scale, line_reduction);
                para
            })
            .collect()
    }

    /// The text of a shape inside a group or SmartArt drawing, drawn in
    /// `xfrm` (no prompts; the geometry's text rectangle applies unless
    /// `text_box` gives the text its own box).
    pub fn shape_text(
        &self,
        slide: usize,
        shape: &Shape,
        xfrm: &Xfrm,
        text_box: bool,
    ) -> Option<RenderText> {
        shape.text.as_ref().filter(|t| !t.is_empty())?;
        let mut t = self.render_text(slide, shape, false)?;
        if !text_box {
            apply_text_rect(&mut t, shape, xfrm);
        }
        Some(t)
    }

    /// The text of a table cell: the slide's default text, the table
    /// style's cell text (`look`), then the body's own formatting; the
    /// cell's margins and anchoring.
    pub fn cell_text(
        &self,
        slide: usize,
        cell: &Cell,
        look: &crate::theme::LevelStyle,
    ) -> Option<RenderText> {
        let body = cell.text.as_ref()?;
        let master = self.master_of(slide);
        let mut ls = master.other_style.clone();
        ls.overlay(&self.default_style);
        for lvl in &mut ls.0 {
            lvl.overlay(look);
        }
        ls.overlay(&body.list);
        let bp = &body.body;
        let scale = bp.font_scale.map_or(1.0, |s| s as f32 / 100_000.0);
        let paras = self.render_paras(slide, body, &ls, scale, bp.line_reduction, false);
        let [l, r, t, b] = cell.margins;
        Some(RenderText {
            paras,
            insets: [l, t, r, b],
            anchor: cell.anchor,
            wrap: true,
            prompt: false,
            vert: cell.vert,
        })
    }

    /// Table styles for slide `slide` (its master's theme colours).
    pub fn table_styles(&self, slide: usize) -> &crate::frame::TableStyles {
        &self.master_of(slide).table_styles
    }

    /// Whether a group or frame on slide `slide` has something to draw.
    pub fn drawable(&self, slide: usize, frame: &crate::frame::Frame) -> bool {
        match frame.get() {
            Some(FrameContent::Group(_) | FrameContent::Table(_)) => true,
            Some(FrameContent::Diagram(d)) => self.diagram_drawing(slide, d).is_some(),
            Some(FrameContent::Chart(_)) | None => false,
        }
    }

    /// PowerPoint's pre-drawn shapes of a SmartArt frame on slide `slide`
    /// (`ppt/diagrams/drawingN.xml`), in the frame's own space (origin at
    /// its top left). `None` if the file has no drawing for it.
    pub fn diagram_drawing(&self, slide: usize, d: &DiagramRef) -> Option<Group> {
        let s = self.slides.get(slide)?;
        let data = d.data.as_deref()?;
        let bytes = self.part_bytes(data)?;
        let xml = std::str::from_utf8(&bytes).ok()?;
        let doc = roxmltree::Document::parse(xml).ok()?;
        let rel = doc
            .descendants()
            .find(|n| is(*n, crate::frame::NS_DSP, "dataModelExt"))
            .and_then(|n| n.attribute("relId"))?;
        let drawing = s
            .rels
            .iter()
            .find(|r| r.id == rel && r.ty.ends_with("/diagramDrawing"))
            .and_then(|r| r.part(&s.part))?;
        let dbytes = self.part_bytes(&drawing)?;
        let dxml = std::str::from_utf8(&dbytes).ok()?;
        let ddoc = roxmltree::Document::parse(dxml).ok()?;
        let tree = ddoc
            .root_element()
            .children()
            .find(|n| is(*n, crate::frame::NS_DSP, "spTree"))?;
        let rels = self
            .part_bytes(&rels_name(&drawing))
            .and_then(|b| String::from_utf8(b.to_vec()).ok())
            .map(|x| parse_rels(&x))
            .unwrap_or_default();
        let master = self.master_of(slide);
        let cx = crate::shape::ParseCtx {
            src: dxml,
            part: &drawing,
            rels: &rels,
            colors: crate::theme::ColorCtx {
                theme: &master.theme,
                map: &master.clr_map,
                ph: None,
            },
        };
        Some(crate::shape::parse_group(tree, &cx, None, None, None))
    }

    /// Everything drawn on slide `slide`, bottom to top: master and layout
    /// decorations (not their placeholders), then the slide's shapes.
    /// `prompts` adds "Click to add title" text to empty placeholders.
    pub fn render_list(&self, slide: usize, prompts: bool) -> Vec<RenderShape<'_>> {
        let s: &Slide = &self.slides[slide];
        let layout = self.layout_of(slide);
        let master = &self.masters[layout.master];
        let mut out = Vec::new();
        if s.show_master_sp {
            if layout.show_master_sp {
                for sh in master.shapes.iter().filter(|x| x.ph.is_none()) {
                    if let Some(xfrm) = sh.xfrm {
                        out.push(RenderShape {
                            shape: sh,
                            layer: Layer::Master,
                            xfrm,
                            text: self.decoration_text(slide, sh),
                        });
                    }
                }
            }
            for sh in layout.shapes.iter().filter(|x| x.ph.is_none()) {
                if let Some(xfrm) = sh.xfrm {
                    out.push(RenderShape {
                        shape: sh,
                        layer: Layer::Layout,
                        xfrm,
                        text: self.decoration_text(slide, sh),
                    });
                }
            }
        }
        for (i, sh) in s.shapes.iter().enumerate() {
            let Some(xfrm) = self.effective_xfrm(slide, sh) else {
                continue;
            };
            out.push(RenderShape {
                shape: sh,
                layer: Layer::Slide(i),
                xfrm,
                text: self.render_text(slide, sh, prompts),
            });
        }
        for r in &mut out {
            if let Some(t) = &mut r.text {
                apply_text_rect(t, r.shape, &r.xfrm);
            }
        }
        out
    }

    /// Text of a master/layout decoration (non-placeholder) shape.
    fn decoration_text(&self, slide: usize, shape: &Shape) -> Option<RenderText> {
        shape.text.as_ref().filter(|t| !t.is_empty())?;
        self.render_text(slide, shape, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbering_schemes() {
        assert_eq!(auto_number("arabicPeriod", 3), "3.");
        assert_eq!(auto_number("arabicParenR", 12), "12)");
        assert_eq!(auto_number("alphaUcParenBoth", 28), "(BB)");
        assert_eq!(auto_number("alphaLcPeriod", 1), "a.");
        assert_eq!(auto_number("romanLcPeriod", 14), "xiv.");
        assert_eq!(auto_number("romanUcPlain", 1999), "MCMXCIX");
        assert_eq!(auto_number("circleNumDbPlain", 2), "2.");
    }

    #[test]
    fn symbol_bullets_map_to_unicode() {
        assert_eq!(bullet_text("§", Some("Wingdings")), ("■".into(), false));
        assert_eq!(
            bullet_text("\u{F0D8}", Some("Wingdings")),
            ("➢".into(), false)
        );
        assert_eq!(bullet_text("–", Some("Arial")), ("–".into(), true));
        assert_eq!(bullet_text("•", None), ("•".into(), true));
    }

    #[test]
    fn numbering_restarts_per_level_and_skips_empty_paragraphs() {
        let num = Bullet::AutoNum {
            scheme: "arabicPeriod".into(),
            start: 1,
        };
        let mut n = Numbering::default();
        let text = |s: Option<Shown>| match s {
            Some(Shown::Number(t) | Shown::Char(t)) => t,
            None => "-".into(),
        };
        let seq = [
            text(n.next(0, Some(&num), true)),
            text(n.next(1, Some(&num), true)),
            text(n.next(1, Some(&num), false)),
            text(n.next(1, Some(&num), true)),
            text(n.next(0, Some(&num), true)),
            text(n.next(1, Some(&num), true)),
            text(n.next(0, Some(&Bullet::None), true)),
            text(n.next(0, Some(&num), true)),
        ];
        assert_eq!(seq, ["1.", "1.", "2.", "2.", "2.", "1.", "-", "1."]);
    }
}
