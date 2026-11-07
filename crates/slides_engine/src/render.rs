//! Display resolution: what to draw for a slide, with positions and text
//! formatting inherited from the layout, the master and the theme
//! (read-only). The UI lays out and paints the result; nothing here is
//! written to the file.

use std::ops::Range;

use docs_engine::Rgb;

use crate::model::*;
use crate::presentation::{Layout, Master, Presentation, Slide};
use crate::theme::{LevelStyle, ListStyle};

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

#[derive(Clone, Debug, PartialEq)]
pub struct RenderPara {
    pub text: String,
    pub align: Align,
    pub level: u8,
    pub runs: Vec<RenderRun>,
    /// Formatting of an empty paragraph (its height) and of the caret.
    pub base: RenderRun,
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
                vec![RenderPara {
                    align: body.paras[0]
                        .props
                        .align
                        .or(level.align)
                        .unwrap_or_default(),
                    level: 0,
                    runs: vec![base.clone()],
                    base,
                    text,
                }]
            }
            None => body
                .paras
                .iter()
                .map(|p| {
                    let level = &ls.0[p.props.level.min(8) as usize];
                    let runs: Vec<RenderRun> = p
                        .run_ranges()
                        .filter(|(r, _)| !r.is_empty())
                        .map(|(range, run)| RenderRun {
                            range,
                            ..self.resolve(slide, level, &run.props, scale)
                        })
                        .collect();
                    RenderPara {
                        text: p.text.clone(),
                        align: p.props.align.or(level.align).unwrap_or_default(),
                        level: p.props.level,
                        base: self.resolve(slide, level, &p.runs[0].props, scale),
                        runs,
                    }
                })
                .collect(),
        };
        let d = [91_440, 45_720, 91_440, 45_720];
        Some(RenderText {
            paras,
            insets: std::array::from_fn(|i| bp.insets[i].unwrap_or(d[i])),
            anchor: bp.anchor.unwrap_or_default(),
            wrap: bp.no_wrap != Some(true),
            prompt,
        })
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
        out
    }

    /// Text of a master/layout decoration (non-placeholder) shape.
    fn decoration_text(&self, slide: usize, shape: &Shape) -> Option<RenderText> {
        shape.text.as_ref().filter(|t| !t.is_empty())?;
        self.render_text(slide, shape, false)
    }
}
