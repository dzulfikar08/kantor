//! `kantor slides check` and the MCP tool `slides_check`: layout problems
//! a deck's author (often an AI agent) can't see without looking at it.
//!
//! Slides are laid out with Kantor's own text layout (`slides_scene`, the
//! layout the canvas, the PDF and the PNG export use), then checked for:
//!
//! - text that overflows its box (how much, lines needed vs lines that
//!   fit), text that wraps in a box made for one line, titles that wrap,
//!   shrink-on-overflow boxes and the scale they need;
//! - shapes partly or fully off the slide;
//! - text running into other text, other shapes covering text, text
//!   crossing the edge of the shape behind it (text fully inside a card or
//!   picture is fine: that's how cards are made);
//! - low contrast between text and what's drawn behind it (WCAG: 4.5:1,
//!   3:1 for text from 18 pt bold or 24 pt; pictures behind are skipped);
//! - fonts that aren't installed here, and text under 10 pt;
//! - empty placeholders, slides with no title, slides without notes.
//!
//! Text inside tables, groups, charts and SmartArt isn't checked.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use slides_engine::frame::Vert;
use slides_engine::render::LineSpacing;
use slides_engine::{EMU_PER_PX, Layer, Presentation, RenderShape, RenderText, TextFit, Xfrm};
use slides_scene::{Builder, Engine, Item, Options, Paint, Prim, Rgba};

/// EMU per inch.
const EMU_PER_IN: f64 = 914_400.0;
/// Slide px per inch.
const PX_PER_IN: f32 = 96.0;
/// Lengths below this (px, 1/96 in) are rounding, not problems.
const SLACK: f32 = 1.5;
/// Text smaller than this (pt) is hard to read on a projector.
const SMALL_PT: f32 = 10.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn name(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// A shape a finding is about.
#[derive(Clone, Debug, PartialEq)]
pub struct Who {
    pub id: u32,
    pub name: String,
    pub kind: String,
}

impl Who {
    fn json(&self) -> Value {
        json!({"id": self.id, "name": self.name, "kind": self.kind})
    }

    fn label(&self) -> String {
        format!("\"{}\" (id {})", self.name, self.id)
    }
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub severity: Severity,
    /// What was checked: `text_overflow`, `off_slide`, ...
    pub check: &'static str,
    /// Slide number from 1; `None` for the whole deck.
    pub slide: Option<usize>,
    pub shape: Option<Who>,
    /// The other shape (overlaps).
    pub other: Option<Who>,
    pub message: String,
    /// Where the problem is: x, y, w, h in EMU.
    pub rect: Option<[i64; 4]>,
    /// Numbers behind the message (check-specific).
    pub details: Map<String, Value>,
}

/// Everything found in a presentation.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// Slides in the presentation.
    pub slides: usize,
    /// The slides checked (numbers from 1).
    pub checked: Vec<usize>,
    /// Slide size, EMU.
    pub width: i64,
    pub height: i64,
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn count(&self, s: Severity) -> usize {
        self.findings.iter().filter(|f| f.severity == s).count()
    }

    pub fn has_errors(&self) -> bool {
        self.count(Severity::Error) > 0
    }

    /// The report as JSON (lengths in EMU, with inches beside them).
    pub fn json(&self) -> Value {
        let findings: Vec<Value> = self.findings.iter().map(finding_json).collect();
        json!({
            "slides": self.slides,
            "checked": self.checked,
            "size": rect_json([0, 0, self.width, self.height]),
            "errors": self.count(Severity::Error),
            "warnings": self.count(Severity::Warning),
            "info": self.count(Severity::Info),
            "findings": findings,
        })
    }

    /// The report for people: a summary line, then the findings by slide.
    pub fn text(&self, file: &str) -> String {
        let mut out = format!(
            "{file}: {} slide{} checked: {} error{}, {} warning{}, {} info\n",
            self.checked.len(),
            plural(self.checked.len()),
            self.count(Severity::Error),
            plural(self.count(Severity::Error)),
            self.count(Severity::Warning),
            plural(self.count(Severity::Warning)),
            self.count(Severity::Info),
        );
        let mut by_slide: BTreeMap<usize, Vec<&Finding>> = BTreeMap::new();
        for f in &self.findings {
            by_slide.entry(f.slide.unwrap_or(0)).or_default().push(f);
        }
        for (slide, list) in by_slide {
            if slide == 0 {
                out.push_str("deck\n");
            } else {
                out.push_str(&format!("slide {slide}\n"));
            }
            for f in list {
                let who = f.shape.as_ref().map(|w| format!("{}: ", w.label()));
                out.push_str(&format!(
                    "  {:<8}{:<20}{}{}\n",
                    f.severity.name(),
                    f.check,
                    who.unwrap_or_default(),
                    f.message
                ));
            }
        }
        out
    }
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn inches(emu: i64) -> f64 {
    (emu as f64 / EMU_PER_IN * 1000.0).round() / 1000.0
}

fn rect_json([x, y, w, h]: [i64; 4]) -> Value {
    json!({
        "x": x, "y": y, "w": w, "h": h,
        "in": {"x": inches(x), "y": inches(y), "w": inches(w), "h": inches(h)},
    })
}

fn finding_json(f: &Finding) -> Value {
    let mut v = json!({
        "severity": f.severity.name(),
        "check": f.check,
        "slide": f.slide,
        "message": f.message,
    });
    if let Some(w) = &f.shape {
        v["shape"] = w.json();
    }
    if let Some(w) = &f.other {
        v["other"] = w.json();
    }
    if let Some(r) = f.rect {
        v["rect"] = rect_json(r);
    }
    if let Some(m) = v.as_object_mut() {
        for (k, d) in &f.details {
            m.insert(k.clone(), d.clone());
        }
    }
    v
}

/// px (1/96 in) as inches for messages.
fn px_in(px: f32) -> String {
    format!("{:.2} in", px / PX_PER_IN)
}

fn px_emu(px: f32) -> i64 {
    (px * EMU_PER_PX).round() as i64
}

/// A box in slide px: x0, y0, x1, y1.
type R = [f32; 4];

fn emu_rect(r: R) -> [i64; 4] {
    [
        px_emu(r[0]),
        px_emu(r[1]),
        px_emu(r[2] - r[0]),
        px_emu(r[3] - r[1]),
    ]
}

fn area(r: R) -> f32 {
    (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0)
}

fn meet(a: R, b: R) -> R {
    [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ]
}

/// `a` lies inside `b`, give or take `slack` px.
fn inside(a: R, b: R, slack: f32) -> bool {
    a[0] >= b[0] - slack && a[1] >= b[1] - slack && a[2] <= b[2] + slack && a[3] <= b[3] + slack
}

/// A point of a shape's own (unturned, px) box on the slide.
fn on_slide(x: &Xfrm, lx: f32, ly: f32) -> (f32, f32) {
    let px = |e: i64| e as f32 / EMU_PER_PX;
    let (w, h) = (px(x.cx), px(x.cy));
    let (cx, cy) = (px(x.x) + w / 2.0, px(x.y) + h / 2.0);
    let (dx, dy) = (lx - w / 2.0, ly - h / 2.0);
    if x.rot == 0 {
        return (cx + dx, cy + dy);
    }
    let (s, c) = (x.angle() as f32).sin_cos();
    (cx + dx * c - dy * s, cy + dx * s + dy * c)
}

/// The slide box around a box of the shape's own space (turned with it).
fn turned_box(x: &Xfrm, r: R) -> R {
    let pts = [
        on_slide(x, r[0], r[1]),
        on_slide(x, r[2], r[1]),
        on_slide(x, r[0], r[3]),
        on_slide(x, r[2], r[3]),
    ];
    let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for (px, py) in pts {
        b[0] = b[0].min(px);
        b[1] = b[1].min(py);
        b[2] = b[2].max(px);
        b[3] = b[3].max(py);
    }
    b
}

fn shape_box(x: &Xfrm) -> R {
    turned_box(
        x,
        [0.0, 0.0, x.cx as f32 / EMU_PER_PX, x.cy as f32 / EMU_PER_PX],
    )
}

// ── text measures ──────────────────────────────────────────────────────────

/// A text body laid out in its box.
struct Measured {
    /// Inner (inset) size of the box, px.
    inner_w: f32,
    inner_h: f32,
    /// Height of the text, px.
    height: f32,
    /// Widest line, px (a long word in a narrow box, unwrapped text).
    wide: f32,
    /// Lines per paragraph.
    lines: Vec<usize>,
    /// Lines that fit the box's height.
    fit: usize,
    /// The first line's height, px.
    first_line: f32,
    /// Where the glyphs are, in the shape's own box (px).
    ink: Option<R>,
}

fn has_text(t: &RenderText) -> bool {
    !t.prompt && t.paras.iter().any(|p| !p.text.trim().is_empty())
}

fn measure(builder: &Builder, engine: &mut Engine, t: &RenderText, xfrm: &Xfrm) -> Measured {
    let laid = builder.place(engine, t, xfrm);
    let layout = &laid.layout;
    let inner_w = slides_scene::text::text_width(t, xfrm);
    let inner_h = ((xfrm.cy - t.insets[1] - t.insets[3]) as f32 / EMU_PER_PX).max(0.0);
    let page_lines = layout
        .paged
        .pages
        .first()
        .map(|p| p.lines.as_slice())
        .unwrap_or_default();
    let mut lines = vec![0; t.paras.len()];
    for l in page_lines {
        if let Some(n) = lines.get_mut(l.para) {
            *n += 1;
        }
    }
    let top = page_lines.first().map_or(0.0, |l| l.top);
    let fit = page_lines
        .iter()
        .filter(|l| l.top + l.height - top <= inner_h + 0.5)
        .count();
    let wide = layout
        .paged
        .layouts
        .iter()
        .map(|l| l.dx + l.layout.width())
        .fold(0.0, f32::max);
    let runs = slides_scene::text::glyph_runs(layout);
    let ink = if runs.is_empty() {
        None
    } else {
        let x0 = runs.iter().map(|r| r.run.origin_x).fold(f32::MAX, f32::min);
        let x1 = runs
            .iter()
            .map(|r| r.run.origin_x + r.run.advance)
            .fold(f32::MIN, f32::max);
        Some([
            laid.dx + x0,
            laid.dy + top,
            laid.dx + x1,
            laid.dy + layout.height,
        ])
    };
    Measured {
        inner_w,
        inner_h,
        height: layout.height - top,
        wide,
        lines,
        fit,
        first_line: page_lines.first().map_or(0.0, |l| l.height),
        ink,
    }
}

/// `t` with every size times `k` (what shrink-on-overflow does).
fn scaled(t: &RenderText, k: f32) -> RenderText {
    let mut t = t.clone();
    for p in &mut t.paras {
        for r in &mut p.runs {
            r.size_pt *= k;
        }
        p.base.size_pt *= k;
        if let Some(m) = &mut p.marker {
            m.size_pt *= k;
        }
        p.space_before_pt *= k;
        p.space_after_pt *= k;
        if let Some(LineSpacing::Points(pt)) = &mut p.line {
            *pt *= k;
        }
    }
    t
}

/// The largest scale (to 1 %) at which `t` fits its box's height.
fn fitting_scale(builder: &Builder, engine: &mut Engine, t: &RenderText, xfrm: &Xfrm) -> f32 {
    let (mut lo, mut hi) = (0.05_f32, 1.0_f32);
    for _ in 0..8 {
        let mid = (lo + hi) / 2.0;
        let m = measure(builder, engine, &scaled(t, mid), xfrm);
        if m.height <= m.inner_h + 0.5 && m.wide <= m.inner_w + SLACK {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo * 100.0).floor() / 100.0
}

// ── colours ────────────────────────────────────────────────────────────────

fn luminance([r, g, b]: [u8; 3]) -> f64 {
    let ch = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn over(below: [u8; 3], c: Rgba) -> [u8; 3] {
    let [r, g, b, a] = c.0;
    let k = f32::from(a) / 255.0;
    let mix = |top: u8, bot: u8| (f32::from(top) * k + f32::from(bot) * (1.0 - k)).round() as u8;
    [mix(r, below[0]), mix(g, below[1]), mix(b, below[2])]
}

fn stop_color(stops: &[slides_scene::Stop], t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    let Some(first) = stops.first() else {
        return Rgba::BLACK;
    };
    if t <= first.offset {
        return first.color;
    }
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.offset {
            let k = if b.offset > a.offset {
                (t - a.offset) / (b.offset - a.offset)
            } else {
                1.0
            };
            let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * k).round() as u8;
            let (ca, cb) = (a.color.0, b.color.0);
            return Rgba([
                m(ca[0], cb[0]),
                m(ca[1], cb[1]),
                m(ca[2], cb[2]),
                m(ca[3], cb[3]),
            ]);
        }
    }
    stops.last().map_or(first.color, |s| s.color)
}

/// The paint's colour at `q` (the item's own coordinates).
fn paint_at(paint: &Paint, q: slides_scene::P) -> Rgba {
    match paint {
        Paint::Linear { from, to, stops } => {
            let v = to.minus(*from);
            let len2 = v.dot(v).max(1e-6);
            stop_color(stops, q.minus(*from).dot(v) / len2)
        }
        Paint::Radial {
            center,
            rx,
            ry,
            stops,
        } => {
            let dx = (q.x - center.x) / rx.max(1e-3);
            let dy = (q.y - center.y) / ry.max(1e-3);
            stop_color(stops, (dx * dx + dy * dy).sqrt())
        }
        p => p.main_color(),
    }
}

/// Whether `q` is inside `path` (even-odd, flattened).
fn contains(path: &slides_scene::Path, q: slides_scene::P) -> bool {
    if let Some([x0, y0, x1, y1]) = path.bounds()
        && (q.x < x0 || q.x > x1 || q.y < y0 || q.y > y1)
    {
        return false;
    }
    let mut hit = false;
    for (pts, _) in path.flatten(0.5) {
        let n = pts.len();
        if n < 3 {
            continue;
        }
        let mut j = n - 1;
        for i in 0..n {
            let (a, b) = (pts[i], pts[j]);
            if (a.y > q.y) != (b.y > q.y) && q.x < (b.x - a.x) * (q.y - a.y) / (b.y - a.y) + a.x {
                hit = !hit;
            }
            j = i;
        }
    }
    hit
}

/// The colour drawn at slide point `q` by `items` (bottom to top), or
/// `None` when a picture shows there.
fn color_at(items: &[Item], q: slides_scene::P) -> Option<[u8; 3]> {
    let mut c = Some([255, 255, 255]);
    for it in items {
        let Some(inv) = it.transform.invert() else {
            continue;
        };
        let local = inv.apply(q);
        match &it.prim {
            Prim::Fill { path, paint, .. } if contains(path, local) => {
                let col = paint_at(paint, local);
                c = match c {
                    Some(below) => Some(over(below, col)),
                    // Over a picture: only an opaque fill is known.
                    None if col.0[3] == 255 => Some([col.0[0], col.0[1], col.0[2]]),
                    None => None,
                };
            }
            Prim::Picture(pic) => {
                let [x, y, w, h] = pic.rect;
                let in_rect = local.x >= x && local.x <= x + w && local.y >= y && local.y <= y + h;
                let in_clip = pic.clip.as_ref().is_none_or(|p| contains(p, local));
                if in_rect && in_clip && pic.alpha > 0.05 {
                    c = None;
                }
            }
            _ => {}
        }
    }
    c
}

// ── the checks ─────────────────────────────────────────────────────────────

/// What the checks keep of one slide shape.
struct Obj {
    who: Who,
    /// Its box on the slide, px.
    rect: R,
    /// Where its text's glyphs are on the slide, px.
    ink: Option<R>,
    /// Something is drawn for it (fill, outline, picture, table, chart...).
    body: bool,
    /// A line or connector.
    line: bool,
}

struct Ctx<'a> {
    slide: usize,
    out: &'a mut Vec<Finding>,
}

impl Ctx<'_> {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        severity: Severity,
        check: &'static str,
        shape: Option<&Who>,
        other: Option<&Who>,
        rect: Option<R>,
        message: String,
        details: Value,
    ) {
        self.out.push(Finding {
            severity,
            check,
            slide: Some(self.slide + 1),
            shape: shape.cloned(),
            other: other.cloned(),
            message,
            rect: rect.map(emu_rect),
            details: match details {
                Value::Object(m) => m,
                _ => Map::new(),
            },
        });
    }
}

/// Whether a font family is installed (or stands for an installed
/// family's style, as "Inter SemiBold").
fn installed(engine: &mut Engine, family: &str) -> bool {
    engine.fonts.collection.family_id(family).is_some()
        || engine.style_family(&format!("\"{family}\"")).is_some()
}

/// Fonts used, with the slides they're on.
type FontUse = BTreeMap<String, Vec<usize>>;

/// Check a presentation: every slide, or only `only` (0-based).
pub fn check(pres: &Presentation, engine: &mut Engine, only: Option<usize>) -> Report {
    let builder = Builder::new();
    let mut report = Report {
        slides: pres.len(),
        width: pres.size.cx,
        height: pres.size.cy,
        ..Report::default()
    };
    let mut fonts = FontUse::new();
    for slide in 0..pres.len() {
        if only.is_some_and(|o| o != slide) {
            continue;
        }
        report.checked.push(slide + 1);
        let mut cx = Ctx {
            slide,
            out: &mut report.findings,
        };
        check_slide(pres, engine, &builder, &mut cx, &mut fonts);
    }
    check_fonts(engine, &fonts, &mut report.findings);
    report
        .findings
        .sort_by_key(|f| (f.slide.unwrap_or(usize::MAX), f.severity));
    report
}

fn check_slide(
    pres: &Presentation,
    engine: &mut Engine,
    builder: &Builder,
    cx: &mut Ctx,
    fonts: &mut FontUse,
) {
    let slide = cx.slide;
    let list = pres.render_list(slide, false);
    let scene = builder.slide(engine, pres, slide, &Options::default());
    let (sw, sh) = (scene.width, scene.height);
    let slide_rect: R = [0.0, 0.0, sw, sh];
    let mut objs: Vec<Obj> = Vec::new();
    let mut has_title = false;
    let mut big_text_on_top = false;
    for r in &list {
        let Layer::Slide(_) = r.layer else {
            continue;
        };
        let shape = r.shape;
        let who = Who {
            id: shape.id,
            name: shape.name.clone(),
            kind: super::slides::shape_kind(shape),
        };
        let rect = shape_box(&r.xfrm);
        let line = matches!(
            shape.kind,
            slides_engine::ShapeKind::Shape {
                geom: slides_engine::Geom::Line,
                ..
            }
        );
        let body = scene.items.iter().any(|it| {
            it.layer == Some(r.layer)
                && match &it.prim {
                    Prim::Fill { paint, .. } => paint.main_color().0[3] > 0,
                    Prim::Stroke { stroke, .. } => stroke.color.0[3] > 0,
                    Prim::Picture(_) | Prim::Unsupported { .. } => true,
                    Prim::Text(_) | Prim::PlaceholderOutline { .. } => false,
                }
        });
        if r.is_empty_placeholder() {
            let ty = shape.ph.as_ref().map_or("obj", |p| p.ty());
            cx.add(
                Severity::Warning,
                "empty_placeholder",
                Some(&who),
                None,
                Some(rect),
                format!(
                    "empty {ty} placeholder: it shows \"Click to add…\" in editors and nothing in the show; fill it or delete it"
                ),
                json!({"placeholder": ty}),
            );
        }
        let text = r.text.as_ref().filter(|t| has_text(t));
        if let Some(t) = text {
            let title_ph = shape.ph.as_ref().is_some_and(|p| p.is_title());
            if title_ph {
                has_title = true;
            }
            let max_pt = t
                .paras
                .iter()
                .flat_map(|p| p.runs.iter().map(|r| r.size_pt))
                .fold(0.0, f32::max);
            if max_pt >= 24.0 && rect[1] < sh / 3.0 {
                big_text_on_top = true;
            }
            for p in &t.paras {
                for run in &p.runs {
                    if !run.font.is_empty() && !run.font.starts_with('+') {
                        let slides = fonts.entry(run.font.clone()).or_default();
                        if slides.last() != Some(&(slide + 1)) {
                            slides.push(slide + 1);
                        }
                    }
                }
            }
        }
        let footer = shape.ph.as_ref().is_some_and(|p| p.is_footer())
            || furniture_footer(&shape.name, rect, sh);
        let ink = match text {
            Some(t) if t.vert == Vert::Horz => {
                let m = measure(builder, engine, t, &r.xfrm);
                check_fit(pres, engine, builder, cx, r, t, &m, &who, rect);
                if !footer {
                    check_small(cx, t, &who, rect);
                }
                m.ink.map(|i| turned_box(&r.xfrm, i))
            }
            Some(t) => {
                if !footer {
                    check_small(cx, t, &who, rect);
                }
                None
            }
            None => None,
        };
        if let Some(t) = text {
            check_contrast(cx, &scene.items, r, t, ink.unwrap_or(rect), &who);
        }
        objs.push(Obj {
            who,
            rect,
            ink,
            body,
            line,
        });
    }
    for o in &objs {
        check_off_slide(cx, o, slide_rect);
    }
    check_overlaps(cx, &objs, slide_rect);
    if !has_title {
        let (sev, msg) = if big_text_on_top {
            (
                Severity::Info,
                "no title placeholder: the slide's big text isn't its title for the outline, links and screen readers (give the slide a `title`)",
            )
        } else {
            (
                Severity::Warning,
                "the slide has no title (outline, links and screen readers show none)",
            )
        };
        cx.add(sev, "no_title", None, None, None, msg.to_owned(), json!({}));
    }
    if pres.notes_text(slide).trim().is_empty() {
        cx.add(
            Severity::Info,
            "no_notes",
            None,
            None,
            None,
            "no speaker notes".to_owned(),
            json!({}),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn check_fit(
    pres: &Presentation,
    engine: &mut Engine,
    builder: &Builder,
    cx: &mut Ctx,
    r: &RenderShape,
    t: &RenderText,
    m: &Measured,
    who: &Who,
    rect: R,
) {
    let fit = pres.body_props(cx.slide, r.shape).fit;
    let lines: usize = m.lines.iter().sum();
    let over = m.height - m.inner_h;
    let over_w = m.wide - m.inner_w;
    let wrapped = m.lines.iter().any(|&n| n > 1);
    let details = json!({
        "lines": lines,
        "lines_that_fit": m.fit,
        "text_height": px_emu(m.height),
        "text_height_in": m.height / PX_PER_IN,
        "box_height": px_emu(m.inner_h),
        "box_height_in": m.inner_h / PX_PER_IN,
        "overflow": px_emu(over.max(0.0)),
        "overflow_in": over.max(0.0) / PX_PER_IN,
    });
    let one_line_box = m.first_line > 0.0 && m.inner_h < m.first_line * 1.6;
    if over > SLACK || over_w > SLACK {
        match fit {
            Some(TextFit::Shrink) => {
                let k = fitting_scale(builder, engine, t, &r.xfrm);
                let mut d = details.clone();
                d["scale_needed"] = json!(k);
                cx.add(
                    if k < 0.6 {
                        Severity::Error
                    } else {
                        Severity::Warning
                    },
                    "autofit_shrink",
                    Some(who),
                    None,
                    Some(rect),
                    format!(
                        "text overflows a shrink-to-fit box: it fits at {:.0} % of its size (PowerPoint shrinks it when the text is edited; the stored file, PDF and PNG show it at full size). Use a smaller size{}",
                        k * 100.0,
                        if k < 0.6 {
                            ", less text or a bigger box: that much shrinking is unreadable"
                        } else {
                            " or a bigger box"
                        }
                    ),
                    d,
                );
            }
            Some(TextFit::Resize) if over_w <= SLACK => {
                cx.add(
                    Severity::Info,
                    "autofit_resize",
                    Some(who),
                    None,
                    Some(rect),
                    format!(
                        "the box grows to fit its text: {} tall instead of {} (check what's below it)",
                        px_in(m.height + (r.xfrm.cy as f32 / EMU_PER_PX - m.inner_h)),
                        px_in(r.xfrm.cy as f32 / EMU_PER_PX)
                    ),
                    details,
                );
            }
            _ if over > SLACK && wrapped && one_line_box => {
                cx.add(
                    Severity::Error,
                    "text_wraps",
                    Some(who),
                    None,
                    Some(rect),
                    format!(
                        "text wraps to {lines} lines in a box made for one ({} tall); {} spills out. Widen the box, shorten the text or make it smaller",
                        px_in(m.inner_h),
                        px_in(over)
                    ),
                    details,
                );
            }
            // Under a fifth of a line: the line box's leading, not glyphs.
            _ if over > SLACK && over <= m.first_line * 0.2 => {
                cx.add(
                    Severity::Warning,
                    "text_overflow",
                    Some(who),
                    None,
                    Some(rect),
                    format!(
                        "text is {} tall, {} more than the box holds ({}): mostly line spacing, but anything just below gets crowded; make the box a little taller",
                        px_in(m.height),
                        px_in(over),
                        px_in(m.inner_h)
                    ),
                    details,
                );
            }
            _ if over > SLACK => {
                cx.add(
                    Severity::Error,
                    "text_overflow",
                    Some(who),
                    None,
                    Some(rect),
                    format!(
                        "text needs {lines} line{} ({}) but the box holds {} ({}); {} spills out",
                        plural(lines),
                        px_in(m.height),
                        m.fit,
                        px_in(m.inner_h),
                        px_in(over)
                    ),
                    details,
                );
            }
            _ => {
                let mut d = details;
                d["text_width_in"] = json!(m.wide / PX_PER_IN);
                d["box_width_in"] = json!(m.inner_w / PX_PER_IN);
                cx.add(
                    Severity::Error,
                    "text_overflow",
                    Some(who),
                    None,
                    Some(rect),
                    format!(
                        "a line is {} wide but the box holds {} ({}); it sticks out at the side",
                        px_in(m.wide),
                        px_in(m.inner_w),
                        if t.wrap {
                            "a word longer than the box"
                        } else {
                            "the text doesn't wrap"
                        }
                    ),
                    d,
                );
            }
        }
        return;
    }
    // Fits, but a title (or one line of big text) broke into more lines
    // than its paragraphs and line breaks ask for. A subtitle is not a
    // title: `kantor slides build` sizes its box for the lines it wraps to.
    let name = r.shape.name.to_lowercase();
    let title_like = r.shape.ph.as_ref().is_some_and(|p| p.is_title())
        || (name.contains("title") && !name.contains("subtitle"));
    let big = t
        .paras
        .iter()
        .flat_map(|p| p.runs.iter().map(|r| r.size_pt))
        .fold(0.0, f32::max)
        >= 24.0;
    let paras = t.paras.iter().filter(|p| !p.text.trim().is_empty()).count();
    let meant: usize = t.paras.iter().map(explicit_lines).sum();
    let wraps = t
        .paras
        .iter()
        .zip(&m.lines)
        .any(|(p, &n)| n > explicit_lines(p));
    if (title_like || (big && paras == 1)) && wraps {
        let what = if title_like { "the title" } else { "the text" };
        let msg = if meant > 1 {
            format!(
                "{what} wraps to {lines} lines where its line breaks make {meant} (it fits the box; check it reads well)"
            )
        } else {
            format!("{what} wraps to {lines} lines (it fits the box; check it reads well)")
        };
        cx.add(
            Severity::Info,
            "title_wraps",
            Some(who),
            None,
            Some(rect),
            msg,
            details,
        );
    }
}

/// The lines a paragraph's own line breaks make (1 without any).
fn explicit_lines(p: &slides_engine::render::RenderPara) -> usize {
    1 + p
        .text
        .chars()
        .filter(|&c| matches!(c, '\n' | '\u{b}' | '\u{2028}'))
        .count()
}

/// The footer text boxes `kantor slides build` draws for a deck `footer`
/// (its text, the slide's `footer_note` and the slide number): footers like
/// the footer placeholders, so small text there is fine. Known by their
/// names and by sitting in the bottom strip of the slide.
fn furniture_footer(name: &str, rect: R, slide_h: f32) -> bool {
    matches!(name, "Footer" | "Footer note" | "Slide number") && rect[1] >= slide_h * 0.85
}

fn check_small(cx: &mut Ctx, t: &RenderText, who: &Who, rect: R) {
    let min = t
        .paras
        .iter()
        .flat_map(|p| {
            p.runs
                .iter()
                .filter(|r| {
                    p.text
                        .get(r.range.clone())
                        .is_some_and(|s| !s.trim().is_empty())
                })
                .map(|r| r.size_pt)
        })
        .fold(f32::MAX, f32::min);
    if min < SMALL_PT {
        cx.add(
            Severity::Warning,
            "small_text",
            Some(who),
            None,
            Some(rect),
            format!("{min:.1} pt text is hard to read on a screen (use {SMALL_PT:.0} pt or more)"),
            json!({"size_pt": min}),
        );
    }
}

fn check_contrast(
    cx: &mut Ctx,
    items: &[Item],
    r: &RenderShape,
    t: &RenderText,
    ink: R,
    who: &Who,
) {
    // Everything drawn below the shape's text: its own fill included.
    let end = items
        .iter()
        .position(|it| it.layer == Some(r.layer) && matches!(it.prim, Prim::Text(_)))
        .unwrap_or(items.len());
    let below = &items[..end];
    let my = (ink[1] + ink[3]) / 2.0;
    let points = [0.25, 0.5, 0.75].map(|k| slides_scene::p(ink[0] + (ink[2] - ink[0]) * k, my));
    let backs: Vec<[u8; 3]> = points.iter().filter_map(|&q| color_at(below, q)).collect();
    if backs.is_empty() {
        return;
    }
    // The worst run against the worst background.
    // Ratio, ratio needed, text colour, background, size.
    type Worst = (f64, f64, [u8; 3], [u8; 3], f32);
    let mut worst: Option<Worst> = None;
    for p in &t.paras {
        for run in &p.runs {
            if p.text
                .get(run.range.clone())
                .is_none_or(|s| s.trim().is_empty())
            {
                continue;
            }
            let large = run.size_pt >= 24.0 || (run.bold && run.size_pt >= 18.0);
            let need = if large { 3.0 } else { 4.5 };
            for &b in &backs {
                let ratio = contrast(run.color.0, b);
                if ratio < need && worst.is_none_or(|w| ratio / need < w.0 / w.1) {
                    worst = Some((ratio, need, run.color.0, b, run.size_pt));
                }
            }
        }
    }
    if let Some((ratio, need, fg, bg, size)) = worst {
        cx.add(
            if ratio < 2.0 {
                Severity::Error
            } else {
                Severity::Warning
            },
            "low_contrast",
            Some(who),
            None,
            Some(ink),
            format!(
                "{} text on {} has contrast {ratio:.2}:1; {size:.0} pt text needs {need}:1",
                hex(fg),
                hex(bg)
            ),
            json!({
                "ratio": (ratio * 100.0).round() / 100.0,
                "needed": need,
                "color": hex(fg),
                "background": hex(bg),
                "size_pt": size,
            }),
        );
    }
}

fn check_off_slide(cx: &mut Ctx, o: &Obj, slide: R) {
    // Lines have no width or height: compare edges, not areas.
    let gone = o.rect[2] < slide[0]
        || o.rect[0] > slide[2]
        || o.rect[3] < slide[1]
        || o.rect[1] > slide[3];
    if gone {
        cx.add(
            Severity::Error,
            "off_slide",
            Some(&o.who),
            None,
            Some(o.rect),
            format!(
                "it is entirely off the slide (at {}, {}; the slide is {} × {})",
                px_in(o.rect[0]),
                px_in(o.rect[1]),
                px_in(slide[2]),
                px_in(slide[3])
            ),
            json!({}),
        );
        return;
    }
    let past = |r: R| {
        [
            slide[0] - r[0],
            slide[1] - r[1],
            r[2] - slide[2],
            r[3] - slide[3],
        ]
        .into_iter()
        .fold(0.0, f32::max)
    };
    let sides = |r: R| {
        let mut s = Vec::new();
        for (d, name) in [
            (slide[0] - r[0], "left"),
            (slide[1] - r[1], "top"),
            (r[2] - slide[2], "right"),
            (r[3] - slide[3], "bottom"),
        ] {
            if d > SLACK {
                s.push(name);
            }
        }
        s.join(", ")
    };
    if let Some(ink) = o.ink
        && past(ink) > SLACK
    {
        cx.add(
            Severity::Error,
            "off_slide",
            Some(&o.who),
            None,
            Some(ink),
            format!(
                "its text runs off the slide ({} past the {} edge)",
                px_in(past(ink)),
                sides(ink)
            ),
            json!({"past": px_emu(past(ink)), "past_in": past(ink) / PX_PER_IN, "text": true}),
        );
    } else if past(o.rect) > SLACK {
        let (sev, what) = if o.ink.is_some() {
            (Severity::Warning, "its box (not its text)")
        } else {
            (
                Severity::Info,
                "it bleeds (fine for a decoration, a mistake otherwise)",
            )
        };
        cx.add(
            sev,
            "off_slide",
            Some(&o.who),
            None,
            Some(o.rect),
            format!(
                "partly off the slide: {what} goes {} past the {} edge",
                px_in(past(o.rect)),
                sides(o.rect)
            ),
            json!({"past": px_emu(past(o.rect)), "past_in": past(o.rect) / PX_PER_IN, "text": false}),
        );
    }
}

fn check_overlaps(cx: &mut Ctx, objs: &[Obj], slide: R) {
    let slide_area = area(slide).max(1.0);
    for (a, oa) in objs.iter().enumerate() {
        let Some(ink) = oa.ink else { continue };
        for (b, ob) in objs.iter().enumerate() {
            if a == b {
                continue;
            }
            // Text against text: once per pair.
            if let Some(ink_b) = ob.ink {
                if b < a {
                    continue;
                }
                let m = meet(ink, ink_b);
                let (w, h) = (m[2] - m[0], m[3] - m[1]);
                if w > 2.0 && h > 2.0 {
                    cx.add(
                        Severity::Error,
                        "text_overlap",
                        Some(&oa.who),
                        Some(&ob.who),
                        Some(m),
                        format!(
                            "its text runs into the text of {} ({} × {} overlap)",
                            ob.who.label(),
                            px_in(w),
                            px_in(h)
                        ),
                        json!({}),
                    );
                    continue;
                }
            }
            if !ob.body || ob.line || area(ob.rect) >= slide_area * 0.9 {
                continue;
            }
            let m = meet(ink, ob.rect);
            let (w, h) = (m[2] - m[0], m[3] - m[1]);
            if w <= 2.0 || h <= 2.0 || inside(ink, ob.rect, SLACK) {
                continue;
            }
            if b > a {
                cx.add(
                    Severity::Warning,
                    "covers_text",
                    Some(&oa.who),
                    Some(&ob.who),
                    Some(m),
                    format!(
                        "{} ({}) is drawn over part of its text",
                        ob.who.label(),
                        ob.who.kind
                    ),
                    json!({}),
                );
            } else if !inside(ob.rect, ink, SLACK) {
                cx.add(
                    Severity::Warning,
                    "text_crosses_edge",
                    Some(&oa.who),
                    Some(&ob.who),
                    Some(m),
                    format!(
                        "its text crosses the edge of {} ({}) behind it: put the text inside it or clear of it",
                        ob.who.label(),
                        ob.who.kind
                    ),
                    json!({}),
                );
            }
        }
    }
}

fn check_fonts(engine: &mut Engine, fonts: &FontUse, out: &mut Vec<Finding>) {
    const FALLBACKS: [&str; 5] = [
        "Carlito",
        "Liberation Sans",
        "Noto Sans",
        "DejaVu Sans",
        "sans-serif",
    ];
    for (font, slides) in fonts {
        if installed(engine, font) {
            continue;
        }
        let substitute =
            kantor_core::fonts::metric_substitute(font).filter(|s| installed(engine, s));
        let (severity, message) = match substitute {
            Some(s) => (
                Severity::Info,
                format!(
                    "{font} isn't installed here: drawn with {s}, which has the same metrics (line breaks match)"
                ),
            ),
            None => {
                let used = FALLBACKS
                    .iter()
                    .find(|f| **f == "sans-serif" || installed(engine, f))
                    .copied()
                    .unwrap_or("a fallback font");
                (
                    Severity::Warning,
                    format!(
                        "{font} isn't installed here: text is drawn and measured with {used}, so line breaks and overflow can differ where it is installed (install it, or use a font this machine has)"
                    ),
                )
            }
        };
        out.push(Finding {
            severity,
            check: "font_missing",
            slide: None,
            shape: None,
            other: None,
            message: format!(
                "{message}; on slide{} {}",
                plural(slides.len()),
                ranges(slides)
            ),
            rect: None,
            details: match json!({"font": font, "slides": slides, "substitute": substitute}) {
                Value::Object(m) => m,
                _ => Map::new(),
            },
        });
    }
}

/// `1, 2, 3, 5` → `1-3, 5`.
fn ranges(n: &[usize]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < n.len() {
        let mut j = i;
        while j + 1 < n.len() && n[j + 1] == n[j] + 1 {
            j += 1;
        }
        out.push(if j > i {
            format!("{}-{}", n[i], n[j])
        } else {
            n[i].to_string()
        });
        i = j + 1;
    }
    out.join(", ")
}

/// The MCP tool's answer for an open presentation (`slide` from 1).
pub fn tool(pres: &Presentation, args: &Value) -> anyhow::Result<Value> {
    use std::cell::RefCell;
    thread_local! {
        static ENGINE: RefCell<Option<Engine>> = const { RefCell::new(None) };
    }
    let only = match super::opt_int(args, "slide")? {
        None => None,
        Some(_) => Some(super::slides::slide_arg(pres, args, "slide")?),
    };
    Ok(ENGINE.with(|e| {
        let mut e = e.borrow_mut();
        let engine = e.get_or_insert_with(Engine::new);
        check(pres, engine, only).json()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deck(slides: Value) -> Presentation {
        let json = json!({"units": "in", "slides": slides});
        crate::agent::slides_author::build(&json, None).unwrap()
    }

    fn run(p: &Presentation) -> Report {
        check(p, &mut Engine::new(), None)
    }

    fn checks(r: &Report, slide: usize) -> Vec<&'static str> {
        r.findings
            .iter()
            .filter(|f| f.slide == Some(slide))
            .map(|f| f.check)
            .collect()
    }

    fn find<'a>(r: &'a Report, check: &str) -> Option<&'a Finding> {
        r.findings.iter().find(|f| f.check == check)
    }

    /// A slide with a title, notes and `elements`.
    fn slide(elements: Value) -> Value {
        json!({"layout": "Title Only", "title": "T", "notes": "n", "elements": elements})
    }

    #[std::prelude::v1::test]
    fn a_clean_slide_has_no_findings() {
        let p = deck(json!([slide(json!([
            {"type": "shape", "x": 1, "y": 2, "w": 5, "h": 3, "fill": "#1F3A5F"},
            {"type": "text", "x": 1.2, "y": 2.2, "w": 4.6, "h": 2.6, "text": "Short", "size": 20, "color": "#FFFFFF"},
        ]))]));
        let r = run(&p);
        let worth: Vec<_> = r
            .findings
            .iter()
            .filter(|f| f.check != "font_missing" && f.check != "title_wraps")
            .collect();
        assert!(worth.is_empty(), "{}", r.text("t"));
    }

    /// A deck made only of a preset's furniture and components, the way
    /// `kantor slides build` lays them out.
    fn preset_deck(preset: &str) -> Presentation {
        let json = json!({"units": "in", "preset": preset, "footer": {"text": "Example Co"}, "slides": [
            {"kind": "cover", "eyebrow": "Board meeting", "title": "From 8 cities\nto 14", "subtitle": "Results and plan",
             "meta": "Strategy team", "mark": "14", "mark_label": "cities by 2027", "notes": "n"},
            {"eyebrow": "Summary", "title": "Ahead of target", "subtitle": "Net sales", "footer_note": "Source: sales.xlsx", "notes": "n",
             "elements": [
                {"type": "stat_cards", "y": 2.45, "h": 2.4, "items": [
                    {"value": "6.3 M", "label": "Sales", "detail": "Above target"},
                    {"value": "5 / 8", "label": "Cities", "detail": "On target"},
                    {"value": "-122 k", "label": "Gap", "detail": "Three cities", "highlight": true}]},
                {"type": "callout", "y": 5.5, "title": "Next:", "text": "fix three cities first."}]},
            {"eyebrow": "Causes", "title": "Four things to check", "footer_note": "Team notes", "notes": "n",
             "elements": [{"type": "cards", "y": 2.45, "h": 2.2, "columns": 3, "items": [
                {"title": "Opening", "text": "Check early sales"},
                {"title": "Rivals", "text": "Check new shops"},
                {"title": "Staff", "text": "Check tenure"}]}]},
            {"eyebrow": "Plan", "title": "Two waves", "notes": "n",
             "elements": [
                {"type": "steps", "y": 2.45, "h": 2.0, "current": 2, "items": [
                    {"title": "Wave 1", "text": "Three cities"},
                    {"title": "Review", "text": "Two of three"},
                    {"title": "Wave 2", "text": "Three more"}]},
                {"type": "comparison", "y": 4.6, "h": 1.9, "columns": [
                    {"tag": "Today", "title": "8 cities", "items": ["Jakarta"]},
                    {"tag": "Wave 1", "title": "3 cities", "items": ["Malang"], "highlight": true}]}]},
            {"eyebrow": "Roadmap", "title": "One decision point", "notes": "n",
             "elements": [
                {"type": "timeline", "y": 2.6, "h": 2.4, "current": 1, "items": [
                    {"label": "Q1", "title": "Fix", "text": "Three cities"},
                    {"label": "Q2", "title": "Wave 1", "text": "Open three"},
                    {"label": "Q3", "title": "Wave 2", "text": "If approved"}]},
                {"type": "table", "preset": "clean", "y": 5.2, "w": 8, "rows": [["Item", "Cost"], ["Shops", "3,300"]]}]},
            {"kind": "section", "eyebrow": "Decisions", "title": "Three approvals", "notes": "n",
             "elements": [
                {"type": "numbered_list", "y": 3.0, "h": 2.4, "items": [
                    {"title": "Program", "text": "for three cities"},
                    {"title": "Wave 1", "text": "in three cities"}]},
                {"type": "callout", "y": 6.0, "title": "Start:", "text": "January."}]}
        ]});
        crate::agent::slides_author::build(&json, None).unwrap()
    }

    #[std::prelude::v1::test]
    fn preset_decks_check_clean() {
        let mut engine = Engine::new();
        for preset in crate::agent::slides_style::PRESETS {
            let r = check(&preset_deck(preset), &mut engine, None);
            // Without the preset's fonts (CI) text is measured in another
            // font, so only the font-independent checks count there.
            let fonts_here = find(&r, "font_missing").is_none();
            let bad: Vec<_> = r
                .findings
                .iter()
                .filter(|f| f.severity <= Severity::Warning && f.check != "font_missing")
                .filter(|f| fonts_here || matches!(f.check, "small_text" | "low_contrast"))
                .collect();
            assert!(bad.is_empty(), "{preset}: {}", r.text("t"));
        }
    }

    #[std::prelude::v1::test]
    fn preset_furniture_names_dont_hide_real_problems() {
        // A small "Footer" box up on the slide is body text; a real
        // overflow in a preset deck is still one.
        let json = json!({"units": "in", "preset": "editorial", "slides": [
            {"title": "T", "notes": "n", "elements": [
                {"type": "text", "name": "Footer", "x": 1, "y": 3, "w": 4, "h": 0.4, "text": "Fine print", "size": 8},
                {"type": "text", "x": 1, "y": 4, "w": 1, "h": 0.3, "size": 18,
                 "text": "This sentence is far too long to stay in a box this small"}]}]});
        let p = crate::agent::slides_author::build(&json, None).unwrap();
        let r = run(&p);
        assert!(find(&r, "small_text").is_some(), "{}", r.text("t"));
        assert!(
            r.findings
                .iter()
                .any(|f| f.severity == Severity::Error && f.check.starts_with("text_")),
            "{}",
            r.text("t")
        );
    }

    #[std::prelude::v1::test]
    fn overflow_and_wrapping() {
        let long = "This sentence is far too long to stay on a single line in a box this narrow";
        let p = deck(json!([slide(json!([
            // One line high, wraps.
            {"type": "text", "name": "Wrapper", "x": 1, "y": 2, "w": 3, "h": 0.8, "text": long, "size": 28},
            // Tall enough for one line, many paragraphs.
            {"type": "text", "name": "Tall", "x": 6, "y": 2, "w": 6, "h": 1.0,
             "text": "one\ntwo\nthree\nfour\nfive\nsix", "size": 20},
        ]))]));
        let r = run(&p);
        let wraps = find(&r, "text_wraps").expect("wraps");
        assert_eq!(wraps.shape.as_ref().unwrap().name, "Wrapper");
        assert_eq!(wraps.severity, Severity::Error);
        let over = find(&r, "text_overflow").expect("overflow");
        assert_eq!(over.shape.as_ref().unwrap().name, "Tall");
        assert_eq!(over.details["lines"], 6);
        assert!(over.details["lines_that_fit"].as_u64().unwrap() < 6);
        assert!(over.details["overflow"].as_i64().unwrap() > 0);
        assert!(r.has_errors());
    }

    #[std::prelude::v1::test]
    fn title_line_breaks_are_not_wraps() {
        // A cover title broken on purpose, and a subtitle the builder sized
        // for the lines it wraps to: neither is a wrapping title.
        let json = json!({"units": "in", "preset": "editorial", "slides": [
            {"kind": "cover", "eyebrow": "Laporan", "title": "Uji coba Kantor:\ntiga bulan,\nsatu direktorat",
             "subtitle": "Hasil uji coba di satu direktorat selama tiga bulan, dan usulan untuk memperluasnya ke seluruh kantor pusat tahun depan",
             "notes": "n"}]});
        let p = crate::agent::slides_author::build(&json, None).unwrap();
        let r = run(&p);
        assert!(find(&r, "title_wraps").is_none(), "{}", r.text("t"));

        let long = "This line is far too long for a narrow box like this one";
        let p = deck(json!([slide(json!([
            // Two lines by its break, no wrapping.
            {"type": "text", "name": "Title one", "x": 1, "y": 2, "w": 5, "h": 2, "text": "Short\nAlso short", "size": 28},
            // One of its two lines wraps.
            {"type": "text", "name": "Title two", "x": 7, "y": 1, "w": 3, "h": 5, "text": format!("Short\n{long}"), "size": 28},
            // A subtitle may wrap.
            {"type": "text", "name": "Subtitle", "x": 1, "y": 5, "w": 3, "h": 2, "text": long, "size": 16},
        ]))]));
        let r = run(&p);
        let wraps: Vec<_> = r
            .findings
            .iter()
            .filter(|f| f.check == "title_wraps")
            .collect();
        assert_eq!(wraps.len(), 1, "{}", r.text("t"));
        assert_eq!(wraps[0].shape.as_ref().unwrap().name, "Title two");
        assert!(
            wraps[0].message.contains("where its line breaks make 2"),
            "{}",
            wraps[0].message
        );
    }

    #[std::prelude::v1::test]
    fn shrink_boxes_report_the_scale_they_need() {
        let p = deck(json!([slide(json!([
            {"type": "text", "x": 1, "y": 2, "w": 6, "h": 1.2, "autofit": "shrink",
             "text": "one\ntwo\nthree\nfour", "size": 20},
        ]))]));
        let r = run(&p);
        let f = find(&r, "autofit_shrink").expect("shrink");
        let k = f.details["scale_needed"].as_f64().unwrap();
        assert!(k > 0.2 && k < 0.95, "{k}");
        assert!(find(&r, "text_overflow").is_none());
    }

    #[std::prelude::v1::test]
    fn shapes_off_the_slide() {
        let p = deck(json!([slide(json!([
            {"type": "text", "name": "Gone", "x": 14, "y": 2, "w": 2, "h": 1, "text": "Lost"},
            {"type": "text", "name": "Half", "x": 12.5, "y": 4, "w": 3, "h": 1, "text": "Half of this is past the edge", "size": 20},
            {"type": "shape", "name": "Bleed", "x": -0.5, "y": 6, "w": 3, "h": 2, "fill": "#AA0000"},
            {"type": "line", "name": "Rule", "x1": 1, "y1": 5, "x2": 6, "y2": 5},
            {"type": "line", "name": "Lost rule", "x1": 1, "y1": 9, "x2": 6, "y2": 9},
        ]))]));
        let r = run(&p);
        let off: Vec<_> = r
            .findings
            .iter()
            .filter(|f| f.check == "off_slide")
            .collect();
        let by = |n: &str| {
            off.iter()
                .find(|f| f.shape.as_ref().unwrap().name == n)
                .copied()
        };
        assert_eq!(by("Gone").unwrap().severity, Severity::Error);
        assert_eq!(by("Half").unwrap().severity, Severity::Error);
        assert_eq!(by("Bleed").unwrap().severity, Severity::Info);
        assert!(by("Rule").is_none(), "{}", r.text("t"));
        assert_eq!(by("Lost rule").unwrap().severity, Severity::Error);
        let gone = by("Gone").unwrap();
        assert_eq!(gone.rect.unwrap()[0], 14 * 914_400);
    }

    #[std::prelude::v1::test]
    fn text_collisions_but_not_cards() {
        let p = deck(json!([slide(json!([
            // A card with text inside: fine.
            {"type": "shape", "name": "Card", "x": 1, "y": 2, "w": 4, "h": 2, "fill": "#EEEEEE"},
            {"type": "text", "name": "In card", "x": 1.2, "y": 2.2, "w": 3.6, "h": 1, "text": "Inside", "size": 18},
            // Two text boxes on top of each other.
            {"type": "text", "name": "A", "x": 6, "y": 2, "w": 4, "h": 1, "text": "First text here", "size": 24},
            {"type": "text", "name": "B", "x": 6.2, "y": 2.1, "w": 4, "h": 1, "text": "Second text here", "size": 24},
            // Text sticking out of a card behind it.
            {"type": "shape", "name": "Small card", "x": 1, "y": 5, "w": 1.5, "h": 1, "fill": "#DDDDDD"},
            {"type": "text", "name": "Wide", "x": 1.1, "y": 5.1, "w": 5, "h": 0.8, "text": "This text is wider than its card", "size": 18},
            // A picture-like shape on top of text.
            {"type": "text", "name": "Under", "x": 6, "y": 5, "w": 5, "h": 1, "text": "Hidden under a box", "size": 20},
            {"type": "shape", "name": "Lid", "x": 7, "y": 4.8, "w": 1.5, "h": 1.5, "fill": "#333333"},
        ]))]));
        let r = run(&p);
        let pair = |check: &str, a: &str, b: &str| {
            r.findings.iter().any(|f| {
                f.check == check
                    && f.shape.as_ref().is_some_and(|w| w.name == a)
                    && f.other.as_ref().is_some_and(|w| w.name == b)
            })
        };
        assert!(pair("text_overlap", "A", "B"), "{}", r.text("t"));
        assert!(
            pair("text_crosses_edge", "Wide", "Small card"),
            "{}",
            r.text("t")
        );
        assert!(pair("covers_text", "Under", "Lid"), "{}", r.text("t"));
        assert!(
            !r.findings
                .iter()
                .any(|f| f.shape.as_ref().is_some_and(|w| w.name == "In card")
                    && f.check != "font_missing"),
            "{}",
            r.text("t")
        );
    }

    #[std::prelude::v1::test]
    fn low_contrast_text() {
        let p = deck(json!([slide(json!([
            {"type": "shape", "x": 1, "y": 2, "w": 5, "h": 2, "fill": "#888888"},
            {"type": "text", "name": "Grey", "x": 1.2, "y": 2.2, "w": 4, "h": 1, "text": "Grey on grey", "size": 14, "color": "#777777"},
            {"type": "shape", "x": 7, "y": 2, "w": 5, "h": 2, "fill": "#102030"},
            {"type": "text", "name": "White", "x": 7.2, "y": 2.2, "w": 4, "h": 1, "text": "White on navy", "size": 14, "color": "#FFFFFF"},
            {"type": "text", "name": "Pale", "x": 1, "y": 5, "w": 5, "h": 1, "text": "Pale on the white slide", "size": 14, "color": "#DDDDDD"},
        ]))]));
        let r = run(&p);
        let low: Vec<&str> = r
            .findings
            .iter()
            .filter(|f| f.check == "low_contrast")
            .map(|f| f.shape.as_ref().unwrap().name.as_str())
            .collect();
        assert!(low.contains(&"Grey"), "{}", r.text("t"));
        assert!(low.contains(&"Pale"), "{}", r.text("t"));
        assert!(!low.contains(&"White"), "{}", r.text("t"));
        let grey = r
            .findings
            .iter()
            .find(|f| f.shape.as_ref().is_some_and(|w| w.name == "Grey"))
            .unwrap();
        assert_eq!(grey.details["background"], "#888888");
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 0.01);
    }

    #[std::prelude::v1::test]
    fn missing_fonts_and_small_text() {
        let p = deck(json!([slide(json!([
            {"type": "text", "x": 1, "y": 2, "w": 5, "h": 1, "text": "Strange font", "font": "Zqxw Nowhere Sans"},
            {"type": "text", "name": "Tiny", "x": 1, "y": 4, "w": 5, "h": 1, "text": "Fine print", "size": 8},
        ]))]));
        let r = run(&p);
        let font = r
            .findings
            .iter()
            .find(|f| f.check == "font_missing" && f.details["font"] == "Zqxw Nowhere Sans")
            .expect("missing font");
        assert_eq!(font.slide, None);
        assert_eq!(font.details["slides"], json!([1]));
        let small = find(&r, "small_text").unwrap();
        assert_eq!(small.shape.as_ref().unwrap().name, "Tiny");
    }

    #[std::prelude::v1::test]
    fn titles_placeholders_and_notes() {
        let p = deck(json!([
            {"layout": "Title Slide", "title": "Only a title"},
            {"layout": "Blank", "elements": [
                {"type": "text", "x": 1, "y": 3, "w": 5, "h": 1, "text": "Body text", "size": 14}
            ]},
        ]));
        let r = run(&p);
        assert!(
            checks(&r, 1).contains(&"empty_placeholder"),
            "{}",
            r.text("t")
        );
        assert!(checks(&r, 1).contains(&"no_notes"));
        assert!(!checks(&r, 1).contains(&"no_title"));
        let no_title = r
            .findings
            .iter()
            .find(|f| f.slide == Some(2) && f.check == "no_title")
            .unwrap();
        assert_eq!(no_title.severity, Severity::Warning);
        // One slide only.
        let one = check(&p, &mut Engine::new(), Some(1));
        assert_eq!(one.checked, vec![2]);
        assert!(one.findings.iter().all(|f| f.slide.is_none_or(|s| s == 2)));
    }

    #[std::prelude::v1::test]
    fn json_and_text_output() {
        let p = deck(json!([slide(json!([
            {"type": "text", "name": "Gone", "x": 14, "y": 2, "w": 2, "h": 1, "text": "Lost"},
        ]))]));
        let r = run(&p);
        let j = r.json();
        assert_eq!(j["slides"], 1);
        assert!(j["errors"].as_u64().unwrap() >= 1);
        let f = j["findings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["check"] == "off_slide")
            .unwrap();
        assert_eq!(f["rect"]["in"]["x"], 14.0);
        assert_eq!(f["shape"]["name"], "Gone");
        let t = r.text("deck.pptx");
        assert!(t.starts_with("deck.pptx: 1 slide checked: "), "{t}");
        assert!(t.contains("off_slide"), "{t}");
        assert_eq!(ranges(&[1, 2, 3, 5, 7, 8]), "1-3, 5, 7-8");
    }
}
