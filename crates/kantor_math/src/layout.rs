//! Equation layout with an OpenType MATH font, after the TeX rules as
//! MathML Core restates them for OpenType (W3C, "MathML Core", §3 Layout
//! of elements): fractions, scripts, radicals, large operators, stretchy
//! delimiters and accents use the font's MATH constants, size variants
//! and glyph assemblies.
//!
//! The result is positioned glyphs and filled rectangles; nothing is
//! rotated or scaled, so any renderer that draws glyph ids at points
//! (GPUI's `paint_glyph`, krilla's `draw_glyphs`) draws it.

use crate::alnum;
use crate::ast::*;
use crate::symbols::{self, Class};
use std::ops::Range;
use ttf_parser::{Face, GlyphId, math};

/// The math font Kantor ships: STIX Two Math (SIL OFL 1.1), unmodified.
pub static BUNDLED_FONT: &[u8] = include_bytes!("../fonts/STIXTwoMath-Regular.ttf");

/// A laid-out equation. Coordinates are px, x to the right from the
/// equation's left edge, y down from its baseline.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Layout {
    pub width: f32,
    /// Above and below the baseline (both positive).
    pub ascent: f32,
    pub descent: f32,
    pub glyphs: Vec<Glyph>,
    /// Filled rectangles `[x, y, w, h]` (fraction bars, radical rules).
    pub rules: Vec<[f32; 4]>,
    /// The characters drawn, in order: glyph clusters index it.
    pub text: String,
}

impl Layout {
    pub fn height(&self) -> f32 {
        self.ascent + self.descent
    }

    /// Moves everything by `(dx, dy)`.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        for g in &mut self.glyphs {
            g.x += dx;
            g.y += dy;
        }
        for r in &mut self.rules {
            r[0] += dx;
            r[1] += dy;
        }
    }
}

/// A glyph at a point.
#[derive(Clone, Debug, PartialEq)]
pub struct Glyph {
    /// 0: the math font; 1: the fallback text font.
    pub font: u8,
    pub id: u16,
    /// Pen position: x, and the glyph's baseline y.
    pub x: f32,
    pub y: f32,
    /// Font size in px.
    pub size: f32,
    /// Its characters in [`Layout::text`].
    pub cluster: Range<usize>,
}

/// Layout settings.
#[derive(Clone, Copy, Debug)]
pub struct Options<'a> {
    /// Font size in px.
    pub size: f32,
    /// The math font (must have a MATH table; [`BUNDLED_FONT`] otherwise).
    pub font: &'a [u8],
    /// A text font for characters the math font lacks.
    pub fallback: Option<&'a [u8]>,
    /// Width of the line a display equation sits on (justification and
    /// equation numbers); `None` sizes it to its content.
    pub width: Option<f32>,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self {
            size: 16.0,
            font: BUNDLED_FONT,
            fallback: None,
            width: None,
        }
    }
}

/// Lays an equation out.
pub fn layout(eq: &Equation, o: &Options) -> Layout {
    let face = Face::parse(o.font, 0)
        .ok()
        .filter(|f| f.tables().math.is_some())
        .or_else(|| Face::parse(BUNDLED_FONT, 0).ok());
    let Some(face) = face else {
        return Layout::default();
    };
    let fallback = o.fallback.and_then(|d| Face::parse(d, 0).ok());
    let mut cx = Ctx::new(face, fallback, o.size);
    let st = if eq.display { St::DISPLAY } else { St::TEXT };
    // Each line with its equation numbers (row baseline, number).
    let mut lines: Vec<(B, Vec<(f32, B)>)> = Vec::new();
    for line in &eq.lines {
        match line.as_slice() {
            [Node::EqArray { rows }] if eq.display => {
                // Rows numbered one by one.
                let (arr, nums) = cx.eq_array(rows, st, true);
                lines.push((arr, nums.into_iter().flatten().collect()));
            }
            _ => {
                let (body, num) = split_number(line);
                let b = cx.row(&body, st);
                let n = num.map(|n| (0.0, cx.row(&n, St::TEXT)));
                lines.push((b, n.into_iter().collect()));
            }
        }
    }
    let em = cx.size;
    let gap = 0.3 * em;
    let content_w = lines.iter().map(|(b, _)| b.w).fold(0.0f32, f32::max);
    let num_w = lines
        .iter()
        .flat_map(|(_, n)| n.iter().map(|(_, n)| n.w))
        .fold(0.0f32, f32::max);
    let width = match o.width {
        Some(w) if eq.display => w.max(content_w + if num_w > 0.0 { num_w + em } else { 0.0 }),
        _ => content_w + if num_w > 0.0 { num_w + em } else { 0.0 },
    };
    let mut out = B::default();
    let mut y = 0.0f32;
    let group_x = (width - content_w) / 2.0;
    for (k, (b, num)) in lines.into_iter().enumerate() {
        if k > 0 {
            y += b.asc + gap;
        }
        let x = if !eq.display {
            0.0
        } else {
            match eq.justify {
                Justify::Left => 0.0,
                Justify::Right => width - b.w - if num_w > 0.0 { num_w + em } else { 0.0 },
                Justify::Center => (width - b.w) / 2.0,
                Justify::CenterGroup => group_x,
            }
        };
        let (asc, mut desc) = (b.asc, b.desc);
        if k == 0 {
            out.asc = asc;
        }
        out.append(b, x, y);
        for (ny, n) in num {
            let (nasc, ndesc, nw) = (n.asc, n.desc, n.w);
            out.append(n, width - nw, y + ny);
            if k == 0 {
                out.asc = out.asc.max(nasc - ny);
            }
            desc = desc.max(ny + ndesc);
        }
        out.desc = out.desc.max(y + desc);
        y += desc;
    }
    out.w = width;
    Layout {
        width: out.w,
        ascent: out.asc,
        descent: out.desc,
        glyphs: out.glyphs,
        rules: out.rules,
        text: cx.text,
    }
}

/// A row's equation number: what follows a `#` in its text.
fn split_number(row: &[Node]) -> (Row, Option<Row>) {
    for (k, n) in row.iter().enumerate() {
        if let Node::Text(t) = n
            && !t.style.normal
            && let Some(at) = t.text.find('#')
        {
            let mut body: Row = row[..k].to_vec();
            if at > 0 {
                body.push(Node::Text(Run::styled(&t.text[..at], t.style)));
            }
            let mut num = Row::new();
            let rest = &t.text[at + 1..];
            if !rest.is_empty() {
                num.push(Node::Text(Run::styled(rest, Style::PLAIN)));
            }
            num.extend(row[k + 1..].iter().cloned());
            return (body, Some(num));
        }
    }
    (row.to_vec(), None)
}

// ── boxes ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct B {
    w: f32,
    asc: f32,
    desc: f32,
    /// Italic correction of a box that is one slanted glyph.
    italic: f32,
    /// Top accent attachment (x) of a one-glyph box.
    accent_x: Option<f32>,
    glyphs: Vec<Glyph>,
    rules: Vec<[f32; 4]>,
    /// Spacing class seen from the left and from the right.
    left: Class,
    right: Class,
    /// A big operator glyph (for script placement).
    large_op: bool,
}

impl Default for B {
    fn default() -> Self {
        Self {
            w: 0.0,
            asc: 0.0,
            desc: 0.0,
            italic: 0.0,
            accent_x: None,
            glyphs: Vec::new(),
            rules: Vec::new(),
            left: Class::Ord,
            right: Class::Ord,
            large_op: false,
        }
    }
}

impl B {
    fn append(&mut self, o: B, dx: f32, dy: f32) {
        for mut g in o.glyphs {
            g.x += dx;
            g.y += dy;
            self.glyphs.push(g);
        }
        for mut r in o.rules {
            r[0] += dx;
            r[1] += dy;
            self.rules.push(r);
        }
    }

    /// Appends `o` at `(dx, dy)` and grows this box to hold it.
    fn add(&mut self, o: B, dx: f32, dy: f32) {
        self.asc = self.asc.max(o.asc - dy);
        self.desc = self.desc.max(o.desc + dy);
        self.w = self.w.max(dx + o.w);
        self.append(o, dx, dy);
    }

    fn rule(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.rules.push([x, y, w, h]);
    }

    fn class(mut self, c: Class) -> B {
        self.left = c;
        self.right = c;
        self
    }

    fn translate_y(&mut self, dy: f32) {
        for g in &mut self.glyphs {
            g.y += dy;
        }
        for r in &mut self.rules {
            r[1] += dy;
        }
        self.asc -= dy;
        self.desc += dy;
    }
}

/// TeX's math styles: display, text, script, scriptscript; cramped
/// styles keep superscripts lower.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct St {
    level: u8,
    cramped: bool,
}

impl St {
    const DISPLAY: St = St {
        level: 0,
        cramped: false,
    };
    const TEXT: St = St {
        level: 1,
        cramped: false,
    };

    fn display(self) -> bool {
        self.level == 0
    }

    fn sup(self) -> St {
        St {
            level: if self.level < 2 { 2 } else { 3 },
            cramped: self.cramped,
        }
    }

    fn sub(self) -> St {
        St {
            cramped: true,
            ..self.sup()
        }
    }

    fn num(self) -> St {
        St {
            level: match self.level {
                0 => 1,
                1 => 2,
                _ => 3,
            },
            cramped: self.cramped,
        }
    }

    fn den(self) -> St {
        St {
            cramped: true,
            ..self.num()
        }
    }

    fn cramp(self) -> St {
        St {
            cramped: true,
            ..self
        }
    }

    fn script_like(self) -> bool {
        self.level >= 2
    }
}

struct Ctx<'a> {
    face: Face<'a>,
    fallback: Option<Face<'a>>,
    consts: Option<math::Constants<'a>>,
    info: Option<math::GlyphInfo<'a>>,
    variants: Option<math::Variants<'a>>,
    upem: f32,
    /// Font size in px.
    size: f32,
    text: String,
}

/// Sizes of TeX's spaces in mu (1/18 em).
const THIN: f32 = 3.0;
const MED: f32 = 4.0;
const THICK: f32 = 5.0;

impl<'a> Ctx<'a> {
    fn new(face: Face<'a>, fallback: Option<Face<'a>>, size: f32) -> Self {
        let m = face.tables().math;
        Self {
            consts: m.and_then(|m| m.constants),
            info: m.and_then(|m| m.glyph_info),
            variants: m.and_then(|m| m.variants),
            upem: face.units_per_em().max(1) as f32,
            face,
            fallback,
            size,
            text: String::new(),
        }
    }

    /// Font size in px at a style.
    fn font_size(&self, st: St) -> f32 {
        let pct = match st.level {
            0 | 1 => 100.0,
            2 => self
                .consts
                .map(|c| c.script_percent_scale_down() as f32)
                .filter(|v| *v > 0.0)
                .unwrap_or(70.0),
            _ => self
                .consts
                .map(|c| c.script_script_percent_scale_down() as f32)
                .filter(|v| *v > 0.0)
                .unwrap_or(50.0),
        };
        self.size * pct / 100.0
    }

    /// px per font unit at a style.
    fn k(&self, st: St) -> f32 {
        self.font_size(st) / self.upem
    }

    /// One em at a style.
    fn em(&self, st: St) -> f32 {
        self.font_size(st)
    }

    /// A MATH constant in px at a style, with a fallback in em.
    fn c(&self, st: St, f: fn(&math::Constants<'a>) -> math::MathValue<'a>, em: f32) -> f32 {
        match self.consts.as_ref() {
            Some(c) => f(c).value as f32 * self.k(st),
            None => em * self.em(st),
        }
    }

    fn axis(&self, st: St) -> f32 {
        self.c(st, |c| c.axis_height(), 0.25)
    }

    fn rule_t(&self, st: St) -> f32 {
        self.c(st, |c| c.fraction_rule_thickness(), 0.05)
    }

    fn mu(&self, st: St, n: f32) -> f32 {
        self.em(st) * n / 18.0
    }

    // ── glyphs ─────────────────────────────────────────────────────────

    /// The box of one glyph of the math font.
    fn glyph_id_box(&mut self, gid: GlyphId, st: St, ch: &str) -> B {
        let k = self.k(st);
        let start = self.text.len();
        self.text.push_str(ch);
        let adv = self.face.glyph_hor_advance(gid).unwrap_or(0) as f32 * k;
        let bb = self.face.glyph_bounding_box(gid);
        let (asc, desc) = bb
            .map(|r| (r.y_max as f32 * k, -(r.y_min as f32) * k))
            .unwrap_or((0.0, 0.0));
        let italic = self
            .info
            .and_then(|i| i.italic_corrections)
            .and_then(|v| v.get(gid))
            .map(|v| v.value as f32 * k)
            .unwrap_or(0.0);
        let accent_x = self
            .info
            .and_then(|i| i.top_accent_attachments)
            .and_then(|v| v.get(gid))
            .map(|v| v.value as f32 * k);
        B {
            w: adv,
            asc: asc.max(0.0),
            desc: desc.max(0.0),
            italic,
            accent_x,
            glyphs: vec![Glyph {
                font: 0,
                id: gid.0,
                x: 0.0,
                y: 0.0,
                size: self.font_size(st),
                cluster: start..self.text.len(),
            }],
            ..B::default()
        }
    }

    /// The box of a character: the math font's glyph, else the fallback
    /// font's.
    fn char_box(&mut self, c: char, st: St) -> B {
        if let Some(gid) = self.face.glyph_index(c) {
            return self.glyph_id_box(gid, st, &c.to_string());
        }
        if let Some(fb) = &self.fallback
            && let Some(gid) = fb.glyph_index(c)
        {
            let k = self.font_size(st) / fb.units_per_em().max(1) as f32;
            let adv = fb.glyph_hor_advance(gid).unwrap_or(0) as f32 * k;
            let (asc, desc) = fb
                .glyph_bounding_box(gid)
                .map(|r| (r.y_max as f32 * k, -(r.y_min as f32) * k))
                .unwrap_or((0.7 * self.em(st), 0.2 * self.em(st)));
            let start = self.text.len();
            self.text.push(c);
            return B {
                w: adv,
                asc: asc.max(0.0),
                desc: desc.max(0.0),
                glyphs: vec![Glyph {
                    font: 1,
                    id: gid.0,
                    x: 0.0,
                    y: 0.0,
                    size: self.font_size(st),
                    cluster: start..self.text.len(),
                }],
                ..B::default()
            };
        }
        self.glyph_id_box(GlyphId(0), st, &c.to_string())
    }

    /// A character as math shows it: letters italic unless the style
    /// says otherwise.
    fn math_char(&mut self, c: char, style: &Style, st: St) -> B {
        let c = if c == '-' { '−' } else { c };
        let bold = style.weight.is_some_and(|w| w.bold());
        let italic = match style.weight {
            Some(w) => w.italic(),
            None => {
                !style.normal
                    && style.variant == Variant::Roman
                    && (c.is_ascii_alphabetic()
                        || matches!(c, 'α'..='ω' | 'ϵ' | 'ϑ' | 'ϕ' | 'ϱ' | 'ϖ' | 'ϰ' | 'ı' | 'ȷ'))
            }
        };
        let styled = alnum::styled(c, style.variant, bold, italic);
        let c = if styled != c && self.face.glyph_index(styled).is_some() {
            styled
        } else {
            c
        };
        self.char_box(c, st)
    }

    /// A glyph's vertical (or horizontal) construction.
    fn construction(&self, gid: GlyphId, vertical: bool) -> Option<math::GlyphConstruction<'a>> {
        let v = self.variants?;
        if vertical {
            v.vertical_constructions.get(gid)
        } else {
            v.horizontal_constructions.get(gid)
        }
    }

    /// A character stretched to at least `target` px along an axis:
    /// the first size variant that's big enough, else a glyph assembly,
    /// else the biggest variant. `None` when it has no variants.
    fn stretch(&mut self, c: char, target: f32, vertical: bool, st: St) -> Option<B> {
        let gid = self.face.glyph_index(c)?;
        let cons = self.construction(gid, vertical)?;
        let k = self.k(st);
        let mut last = None;
        for v in cons.variants {
            last = Some(v.variant_glyph);
            if v.advance_measurement as f32 * k >= target {
                if v.variant_glyph == gid {
                    return None;
                }
                return Some(self.glyph_id_box(v.variant_glyph, st, &c.to_string()));
            }
        }
        if let Some(asm) = cons.assembly {
            return Some(self.assemble(asm, target, vertical, st, c));
        }
        match last {
            Some(g) if g != gid => Some(self.glyph_id_box(g, st, &c.to_string())),
            _ => None,
        }
    }

    /// A glyph assembly of total size `target` (MathML Core §3.3.1.3).
    /// Vertical assemblies come out with their bottom on the baseline.
    fn assemble(
        &mut self,
        asm: math::GlyphAssembly<'a>,
        target: f32,
        vertical: bool,
        st: St,
        c: char,
    ) -> B {
        let k = self.k(st);
        let parts: Vec<math::GlyphPart> = asm.parts.into_iter().collect();
        let min_overlap = self.variants.map(|v| v.min_connector_overlap).unwrap_or(0) as f32 * k;
        // Repeat the extenders until the parts reach the target.
        let fixed: f32 = parts
            .iter()
            .filter(|p| !p.part_flags.extender())
            .map(|p| p.full_advance as f32 * k)
            .sum();
        let ext: f32 = parts
            .iter()
            .filter(|p| p.part_flags.extender())
            .map(|p| p.full_advance as f32 * k)
            .sum();
        let n_fixed = parts.iter().filter(|p| !p.part_flags.extender()).count();
        let n_ext = parts.len() - n_fixed;
        let mut reps = 0usize;
        if n_ext > 0 {
            while reps < 1000 {
                let n = n_fixed + n_ext * reps;
                let size = fixed + ext * reps as f32 - min_overlap * n.saturating_sub(1) as f32;
                if size >= target {
                    break;
                }
                reps += 1;
            }
        }
        let seq: Vec<math::GlyphPart> = parts
            .iter()
            .flat_map(|p| {
                let n = if p.part_flags.extender() { reps } else { 1 };
                std::iter::repeat_n(*p, n)
            })
            .collect();
        let total: f32 = seq.iter().map(|p| p.full_advance as f32 * k).sum();
        let joints = seq.len().saturating_sub(1).max(1) as f32;
        // The overlap that hits the target, within what the connectors allow.
        let max_overlap = seq
            .windows(2)
            .map(|w| w[0].end_connector_length.min(w[1].start_connector_length) as f32 * k)
            .fold(f32::INFINITY, f32::min);
        let max_overlap = if max_overlap.is_finite() {
            max_overlap.max(min_overlap)
        } else {
            min_overlap
        };
        let overlap = ((total - target) / joints).clamp(min_overlap, max_overlap);
        let mut b = B::default();
        let mut pos = 0.0f32;
        let size = self.font_size(st);
        let start = self.text.len();
        self.text.push(c);
        let cluster = start..self.text.len();
        for p in &seq {
            let gid = p.glyph_id;
            let bb = self.face.glyph_bounding_box(gid);
            let adv = self.face.glyph_hor_advance(gid).unwrap_or(0) as f32 * k;
            if vertical {
                let y_min = bb.map(|r| r.y_min as f32 * k).unwrap_or(0.0);
                // Ink bottom at `pos` above the baseline.
                let y = -pos + y_min;
                b.glyphs.push(Glyph {
                    font: 0,
                    id: gid.0,
                    x: 0.0,
                    y,
                    size,
                    cluster: cluster.clone(),
                });
                b.w = b.w.max(adv);
            } else {
                let x_min = bb.map(|r| r.x_min as f32 * k).unwrap_or(0.0);
                b.glyphs.push(Glyph {
                    font: 0,
                    id: gid.0,
                    x: pos - x_min,
                    y: 0.0,
                    size,
                    cluster: cluster.clone(),
                });
                if let Some(r) = bb {
                    b.asc = b.asc.max(r.y_max as f32 * k);
                    b.desc = b.desc.max(-(r.y_min as f32) * k);
                }
            }
            pos += p.full_advance as f32 * k - overlap;
        }
        let extent = pos + overlap;
        if vertical {
            b.asc = extent;
            b.desc = 0.0;
            b.italic = asm.italics_correction.value as f32 * k;
        } else {
            b.w = extent;
        }
        b
    }

    /// A delimiter covering `asc` above and `desc` below the baseline,
    /// centred on the axis (TeX's \left…\right sizing).
    fn delimiter(&mut self, s: &str, asc: f32, desc: f32, st: St, grow: bool) -> B {
        let Some(c) = s.chars().next() else {
            // `.`: TeX's null delimiter space.
            return B {
                w: 0.12 * self.em(st),
                ..B::default()
            };
        };
        if !grow {
            return self.char_box(c, st);
        }
        let axis = self.axis(st);
        let delta = (asc - axis).max(desc + axis);
        let target = (2.0 * delta * 0.901).max(2.0 * delta - 0.5 * self.em(st));
        let base = self.char_box(c, st);
        if base.asc + base.desc >= target {
            return base;
        }
        // The base glyph's text goes; the stretched one adds its own.
        self.text.truncate(self.text.len() - c.len_utf8());
        match self.stretch(c, target, true, st) {
            Some(mut b) => {
                let center = (b.desc - b.asc) / 2.0;
                b.translate_y(-axis - center);
                b
            }
            None => self.char_box(c, st),
        }
    }

    // ── rows ───────────────────────────────────────────────────────────

    /// A row: its items side by side with TeX's spacing between them.
    fn row(&mut self, r: &[Node], st: St) -> B {
        let mut atoms: Vec<B> = Vec::new();
        for n in r {
            self.node(n, st, &mut atoms);
        }
        self.hpack(atoms, st)
    }

    fn hpack(&mut self, mut atoms: Vec<B>, st: St) -> B {
        // Binary operators with nothing to combine become ordinary.
        for i in 0..atoms.len() {
            if atoms[i].left == Class::Bin {
                let prev = if i == 0 {
                    None
                } else {
                    Some(atoms[i - 1].right)
                };
                let next = atoms.get(i + 1).map(|a| a.left);
                let unary = matches!(
                    prev,
                    None | Some(Class::Bin | Class::Op | Class::Rel | Class::Open | Class::Punct)
                ) || matches!(
                    next,
                    None | Some(Class::Rel | Class::Close | Class::Punct)
                );
                if unary {
                    atoms[i].left = Class::Ord;
                    atoms[i].right = Class::Ord;
                }
            }
        }
        let mut b = B::default();
        let mut x = 0.0f32;
        let n = atoms.len();
        let mut first = true;
        let mut last_right = Class::Ord;
        let mut only = None;
        for a in atoms {
            if !first {
                x += self.space(last_right, a.left, st);
            }
            if first {
                b.left = a.left;
            }
            first = false;
            last_right = a.right;
            if n == 1 {
                only = Some((a.italic, a.accent_x, a.large_op));
            }
            let w = a.w;
            b.add(a, x, 0.0);
            x += w;
        }
        b.w = x;
        b.right = last_right;
        if let Some((italic, accent_x, large_op)) = only {
            b.italic = italic;
            b.accent_x = accent_x;
            b.large_op = large_op;
        }
        b
    }

    /// Space between two atoms (TeX's table, The TeXbook ch. 18).
    fn space(&self, l: Class, r: Class, st: St) -> f32 {
        use Class::*;
        // (mu, also in script styles)
        let (mu, script) = match (l, r) {
            (Ord, Op) | (Op, Ord) | (Op, Op) | (Close, Op) | (Inner, Op) => (THIN, true),
            (Ord, Bin)
            | (Bin, Ord)
            | (Bin, Op)
            | (Bin, Open)
            | (Bin, Inner)
            | (Close, Bin)
            | (Inner, Bin)
            | (Op, Bin) => (MED, false),
            (Ord, Rel)
            | (Op, Rel)
            | (Rel, Ord)
            | (Rel, Op)
            | (Rel, Open)
            | (Rel, Inner)
            | (Close, Rel)
            | (Inner, Rel)
            | (Bin, Rel)
            | (Rel, Bin) => (THICK, false),
            (Ord, Inner)
            | (Op, Inner)
            | (Close, Inner)
            | (Inner, Ord)
            | (Inner, Open)
            | (Inner, Punct)
            | (Inner, Inner) => (THIN, false),
            (Punct, _) => (THIN, false),
            _ => (0.0, true),
        };
        if mu == 0.0 || (st.script_like() && !script) {
            0.0
        } else {
            self.mu(st, mu)
        }
    }

    /// Lays out one item, pushing its atoms.
    fn node(&mut self, n: &Node, st: St, atoms: &mut Vec<B>) {
        match n {
            Node::Text(r) => self.text_atoms(r, st, atoms),
            _ => {
                let b = self.item(n, st);
                atoms.push(b);
            }
        }
    }

    fn text_atoms(&mut self, r: &Run, st: St, atoms: &mut Vec<B>) {
        if r.style.normal {
            // Ordinary text: one atom, spaces kept.
            let mut row = B::default();
            let mut x = 0.0;
            for c in r.text.chars() {
                if c == ' ' || c == '\u{00A0}' {
                    x += 0.25 * self.em(st);
                    continue;
                }
                let g = self.math_char(c, &r.style, st);
                let w = g.w;
                row.add(g, x, 0.0);
                x += w;
            }
            row.w = x;
            atoms.push(row);
            return;
        }
        let chars: Vec<char> = r.text.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            if c == '&' || c == ' ' || c == '\t' || c == '\n' {
                continue;
            }
            if let Some(w) = symbols::space_width(c) {
                atoms.push(B {
                    w: w * self.em(st),
                    ..B::default()
                });
                continue;
            }
            let mut b = self.math_char(c, &r.style, st);
            let mut cl = symbols::class(c);
            // A comma or point between digits is part of the number.
            if matches!(c, ',' | '.')
                && i > 0
                && chars[i - 1].is_ascii_digit()
                && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())
            {
                cl = Class::Ord;
            }
            b.left = cl;
            b.right = cl;
            atoms.push(b);
        }
    }

    /// Lays out a structure (anything but text).
    fn item(&mut self, n: &Node, st: St) -> B {
        match n {
            Node::Text(r) => {
                let mut atoms = Vec::new();
                self.text_atoms(r, st, &mut atoms);
                self.hpack(atoms, st)
            }
            Node::Frac { kind, num, den } => self.frac(*kind, num, den, st),
            Node::Scripts { base, sub, sup } => {
                let b = self.row(base, st);
                self.scripts(b, sub.as_deref(), sup.as_deref(), st)
            }
            Node::PreScripts { sub, sup, base } => self.prescripts(sub, sup, base, st),
            Node::Radical { degree, body } => self.radical(degree.as_deref(), body, st),
            Node::Nary {
                op,
                under_over,
                sub,
                sup,
                body,
            } => self.nary(*op, *under_over, sub.as_deref(), sup.as_deref(), body, st),
            Node::Delim {
                open,
                close,
                sep,
                items,
                grow,
            } => self.delim(open, close, sep, items, *grow, st),
            Node::Matrix { rows } => self.matrix(rows, st),
            Node::EqArray { rows } => self.eq_array(rows, st, false).0,
            Node::Accent { chr, body } => self.accent(*chr, body, st),
            Node::Bar { top, body } => self.bar(*top, body, st),
            Node::Boxed { body } => self.boxed(body, st),
            Node::Group { body } => self.row(body, st),
            Node::GroupChr { chr, top, body } => self.group_chr(*chr, *top, body, st),
            Node::Limit { upper, base, limit } => {
                let b = self.row(base, st);
                self.limit(b, *upper, limit, st)
            }
            Node::Func { name, body } => self.func(name, body, st),
            Node::Phantom {
                body,
                show,
                zero_width,
                zero_asc,
                zero_desc,
            } => {
                let mut b = self.row(body, st);
                if !show {
                    b.glyphs.clear();
                    b.rules.clear();
                }
                if *zero_width {
                    b.w = 0.0;
                }
                if *zero_asc {
                    b.asc = 0.0;
                }
                if *zero_desc {
                    b.desc = 0.0;
                }
                b
            }
        }
    }

    // ── structures ─────────────────────────────────────────────────────

    fn frac(&mut self, kind: FracKind, num: &[Node], den: &[Node], st: St) -> B {
        match kind {
            FracKind::Linear => {
                let mut atoms = vec![self.row(num, st)];
                let mut slash = self.char_box('/', st);
                slash.left = Class::Ord;
                slash.right = Class::Ord;
                atoms.push(slash);
                atoms.push(self.row(den, st));
                return self.hpack(atoms, st);
            }
            FracKind::Skewed => {
                let n = self.row(num, st.num());
                let d = self.row(den, st.den());
                let slash = self.char_box('⁄', st);
                let gap = self.c(st, |c| c.skewed_fraction_horizontal_gap(), 0.1);
                let mut b = B::default();
                let (sa, sd) = (slash.asc, slash.desc);
                let sw = slash.w;
                let nw = n.w;
                let n_y = -(sa - n.asc).max(0.0) * 0.5 - self.axis(st) * 0.5;
                b.add(n, 0.0, n_y);
                let sx = nw - gap * 0.25;
                b.add(slash, sx.max(0.0), 0.0);
                let d_y = (sd - d.desc).max(0.0) * 0.5;
                b.add(d, sx.max(0.0) + sw - gap * 0.25, d_y);
                return b.class(Class::Inner);
            }
            _ => {}
        }
        let n = self.row(num, st.num());
        let d = self.row(den, st.den());
        let display = st.display();
        let axis = self.axis(st);
        let pad = 0.1 * self.em(st);
        let w = n.w.max(d.w) + 2.0 * pad;
        let mut b = B { w, ..B::default() };
        let (nw, dw) = (n.w, d.w);
        if kind == FracKind::NoBar {
            let mut u = if display {
                self.c(st, |c| c.stack_top_display_style_shift_up(), 0.68)
            } else {
                self.c(st, |c| c.stack_top_shift_up(), 0.44)
            };
            let mut v = if display {
                self.c(st, |c| c.stack_bottom_display_style_shift_down(), 0.69)
            } else {
                self.c(st, |c| c.stack_bottom_shift_down(), 0.35)
            };
            let gap_min = if display {
                self.c(st, |c| c.stack_display_style_gap_min(), 0.35)
            } else {
                self.c(st, |c| c.stack_gap_min(), 0.15)
            };
            let gap = (u - n.desc) - (d.asc - v);
            if gap < gap_min {
                u += (gap_min - gap) / 2.0;
                v += (gap_min - gap) / 2.0;
            }
            b.add(n, (w - nw) / 2.0, -u);
            b.add(d, (w - dw) / 2.0, v);
            b.w = w;
            return b.class(Class::Inner);
        }
        let t = self.rule_t(st);
        let (shift_up, shift_down, gap_num, gap_den) = if display {
            (
                self.c(st, |c| c.fraction_numerator_display_style_shift_up(), 0.68),
                self.c(
                    st,
                    |c| c.fraction_denominator_display_style_shift_down(),
                    0.69,
                ),
                self.c(st, |c| c.fraction_num_display_style_gap_min(), 0.15),
                self.c(st, |c| c.fraction_denom_display_style_gap_min(), 0.15),
            )
        } else {
            (
                self.c(st, |c| c.fraction_numerator_shift_up(), 0.39),
                self.c(st, |c| c.fraction_denominator_shift_down(), 0.34),
                self.c(st, |c| c.fraction_numerator_gap_min(), 0.05),
                self.c(st, |c| c.fraction_denominator_gap_min(), 0.05),
            )
        };
        let up = shift_up.max(gap_num + t / 2.0 + axis + n.desc);
        let down = shift_down.max(gap_den + t / 2.0 - axis + d.asc);
        b.add(n, (w - nw) / 2.0, -up);
        b.add(d, (w - dw) / 2.0, down);
        b.rule(0.0, -axis - t / 2.0, w, t);
        b.asc = b.asc.max(axis + t / 2.0);
        b.w = w;
        b.class(Class::Inner)
    }

    /// Scripts after a base (MathML Core §3.4.1).
    fn scripts(&mut self, base: B, sub: Option<&[Node]>, sup: Option<&[Node]>, st: St) -> B {
        let sub_b = sub.map(|s| self.row(s, st.sub()));
        let sup_b = sup.map(|s| self.row(s, st.sup()));
        self.attach(base, sub_b, sup_b, st)
    }

    fn attach(&mut self, base: B, sub: Option<B>, sup: Option<B>, st: St) -> B {
        let ic = base.italic;
        let (bw, basc, bdesc) = (base.w, base.asc, base.desc);
        let (left, right) = (base.left, base.right);
        let single = base.glyphs.len() <= 1 && base.rules.is_empty();
        let mut b = B {
            w: bw,
            asc: basc,
            desc: bdesc,
            ..B::default()
        };
        b.append(base, 0.0, 0.0);
        let after = self.c(st, |c| c.space_after_script(), 0.05);
        // Single characters take no baseline drop (TeX).
        let drop = !single;
        let mut sup_shift = 0.0;
        let mut sub_shift = 0.0;
        if let Some(sp) = &sup {
            let up = if st.cramped {
                self.c(st, |c| c.superscript_shift_up_cramped(), 0.29)
            } else {
                self.c(st, |c| c.superscript_shift_up(), 0.36)
            };
            let drop_max = self.c(st, |c| c.superscript_baseline_drop_max(), 0.25);
            let bottom_min = self.c(st, |c| c.superscript_bottom_min(), 0.11);
            sup_shift = up
                .max(if drop { basc - drop_max } else { 0.0 })
                .max(bottom_min + sp.desc);
        }
        if let Some(sb) = &sub {
            let down = self.c(st, |c| c.subscript_shift_down(), 0.21);
            let drop_min = self.c(st, |c| c.subscript_baseline_drop_min(), 0.05);
            let top_max = self.c(st, |c| c.subscript_top_max(), 0.34);
            sub_shift = down
                .max(if drop { bdesc + drop_min } else { 0.0 })
                .max(sb.asc - top_max);
        }
        if let (Some(sp), Some(sb)) = (&sup, &sub) {
            let gap_min = self.c(st, |c| c.sub_superscript_gap_min(), 0.2);
            let gap = (sup_shift - sp.desc) - (sb.asc - sub_shift);
            if gap < gap_min {
                sub_shift += gap_min - gap;
                let max_bottom = self.c(st, |c| c.superscript_bottom_max_with_subscript(), 0.34);
                let delta = max_bottom - (sup_shift - sp.desc);
                if delta > 0.0 {
                    sup_shift += delta;
                    sub_shift -= delta;
                }
            }
        }
        let mut w = bw;
        if let Some(sp) = sup {
            let x = bw + ic;
            w = w.max(x + sp.w);
            b.add(sp, x, -sup_shift);
        }
        if let Some(sb) = sub {
            let x = bw;
            w = w.max(x + sb.w);
            b.add(sb, x, sub_shift);
        }
        b.w = w + after;
        b.left = left;
        b.right = if right == Class::Op {
            Class::Ord
        } else {
            right
        };
        b
    }

    fn prescripts(&mut self, sub: &[Node], sup: &[Node], base: &[Node], st: St) -> B {
        let base_b = self.row(base, st);
        let sb = (!sub.is_empty()).then(|| self.row(sub, st.sub()));
        let sp = (!sup.is_empty()).then(|| self.row(sup, st.sup()));
        // Lay the scripts out as if after an empty base of the base's
        // height, then put the base after them.
        let probe = B {
            asc: base_b.asc,
            desc: base_b.desc,
            ..B::default()
        };
        let scripts = self.attach(probe, sb, sp, st);
        let sw = scripts.w;
        let mut b = B::default();
        b.add(scripts, 0.0, 0.0);
        b.add(base_b, sw, 0.0);
        b
    }

    fn radical(&mut self, degree: Option<&[Node]>, body: &[Node], st: St) -> B {
        let body_b = self.row(body, st.cramp());
        let t = self.c(st, |c| c.radical_rule_thickness(), 0.05);
        let gap = if st.display() {
            self.c(st, |c| c.radical_display_style_vertical_gap(), 0.15)
        } else {
            self.c(st, |c| c.radical_vertical_gap(), 0.06)
        };
        let extra = self.c(st, |c| c.radical_extra_ascender(), 0.05);
        let target = body_b.asc + body_b.desc + gap + t;
        let mut surd = match self.stretch('√', target, true, st) {
            Some(b) => b,
            None => self.char_box('√', st),
        };
        let surd_h = surd.asc + surd.desc;
        let adj_gap = gap + ((surd_h - target) / 2.0).max(0.0);
        let rule_top = body_b.asc + adj_gap + t;
        // The surd's top at the rule's top.
        let dy = surd.asc - rule_top;
        surd.translate_y(dy);
        let mut b = B::default();
        let mut x = 0.0;
        if let Some(deg) = degree {
            let d = self.row(
                deg,
                St {
                    level: 3,
                    cramped: true,
                },
            );
            let before = self.c(st, |c| c.radical_kern_before_degree(), 0.28);
            let after = self.c(st, |c| c.radical_kern_after_degree(), -0.55);
            let raise_pct = self
                .consts
                .map(|c| c.radical_degree_bottom_raise_percent() as f32)
                .unwrap_or(60.0);
            let surd_bottom = surd.desc;
            let raise = raise_pct / 100.0 * (surd.asc + surd.desc);
            let dw = d.w;
            let d_y = surd_bottom - raise - d.desc;
            b.add(d, before, d_y);
            x = (before + dw + after).max(0.0);
        }
        let sw = surd.w;
        b.add(surd, x, 0.0);
        let bw = body_b.w;
        b.add(body_b, x + sw, 0.0);
        b.rule(x + sw, -rule_top, bw, t);
        b.asc = b.asc.max(rule_top + extra);
        b.w = x + sw + bw;
        b
    }

    fn nary(
        &mut self,
        op: char,
        under_over: bool,
        sub: Option<&[Node]>,
        sup: Option<&[Node]>,
        body: &[Node],
        st: St,
    ) -> B {
        let mut op_b = self.char_box(op, st);
        if st.display() {
            let min_h = self
                .consts
                .map(|c| c.display_operator_min_height() as f32 * self.k(st))
                .unwrap_or(1.3 * self.em(st));
            if op_b.asc + op_b.desc < min_h {
                self.text.truncate(self.text.len() - op.len_utf8());
                op_b = match self.stretch(op, min_h, true, st) {
                    Some(b) => b,
                    None => self.char_box(op, st),
                };
            }
        }
        // Centred on the axis.
        let axis = self.axis(st);
        let center = (op_b.desc - op_b.asc) / 2.0;
        op_b.translate_y(-axis - center);
        op_b.large_op = true;
        op_b.left = Class::Op;
        op_b.right = Class::Op;
        let limits = if under_over && st.display() {
            let upper = sup.map(|s| self.row(s, st.sup()));
            let lower = sub.map(|s| self.row(s, st.sub()));
            self.under_over(op_b, upper, lower, st, true)
        } else {
            self.scripts(op_b, sub, sup, st)
        };
        let mut atoms = vec![limits.class(Class::Op)];
        if !body.is_empty() {
            let mut inner = Vec::new();
            for n in body {
                self.node(n, st, &mut inner);
            }
            atoms.extend(inner);
        }
        let mut b = self.hpack(atoms, st);
        b.left = Class::Op;
        b
    }

    /// Limits above and below a base, centred (italic correction shifts
    /// them apart for slanted operators).
    fn under_over(&mut self, base: B, upper: Option<B>, lower: Option<B>, st: St, op: bool) -> B {
        let ic = if op { base.italic } else { 0.0 };
        let (bw, basc, bdesc) = (base.w, base.asc, base.desc);
        let (left, right) = (base.left, base.right);
        let w = [
            bw,
            upper.as_ref().map_or(0.0, |u| u.w + ic / 2.0),
            lower.as_ref().map_or(0.0, |l| l.w + ic / 2.0),
        ]
        .into_iter()
        .fold(0.0f32, f32::max);
        let mut b = B::default();
        b.add(base, (w - bw) / 2.0, 0.0);
        if let Some(u) = upper {
            let gap_min = self.c(st, |c| c.upper_limit_gap_min(), 0.1);
            let rise_min = self.c(st, |c| c.upper_limit_baseline_rise_min(), 0.3);
            let gap = gap_min.max(rise_min - u.desc);
            let y = -(basc + gap + u.desc);
            let uw = u.w;
            b.add(u, (w - uw) / 2.0 + ic / 2.0, y);
        }
        if let Some(l) = lower {
            let gap_min = self.c(st, |c| c.lower_limit_gap_min(), 0.17);
            let drop_min = self.c(st, |c| c.lower_limit_baseline_drop_min(), 0.6);
            let gap = gap_min.max(drop_min - l.asc);
            let y = bdesc + gap + l.asc;
            let lw = l.w;
            b.add(l, (w - lw) / 2.0 - ic / 2.0, y);
        }
        b.w = w;
        b.left = left;
        b.right = right;
        b
    }

    fn delim(
        &mut self,
        open: &str,
        close: &str,
        sep: &str,
        items: &[Row],
        grow: bool,
        st: St,
    ) -> B {
        let inner: Vec<B> = items.iter().map(|i| self.row(i, st)).collect();
        let asc = inner.iter().map(|b| b.asc).fold(0.0f32, f32::max);
        let desc = inner.iter().map(|b| b.desc).fold(0.0f32, f32::max);
        let mut b = B::default();
        let mut x = 0.0;
        let o = self.delimiter(open, asc, desc, st, grow);
        let ow = o.w;
        b.add(o, x, 0.0);
        x += ow;
        let n = inner.len();
        for (k, i) in inner.into_iter().enumerate() {
            let iw = i.w;
            b.add(i, x, 0.0);
            x += iw;
            if k + 1 < n {
                let s = self.delimiter(sep, asc, desc, st, grow);
                let sw = s.w;
                let pad = self.mu(st, THIN);
                b.add(s, x + pad, 0.0);
                x += sw + 2.0 * pad;
            }
        }
        let c = self.delimiter(close, asc, desc, st, grow);
        let cw = c.w;
        b.add(c, x, 0.0);
        x += cw;
        b.w = x;
        let cl = if open.is_empty() && close.is_empty() {
            Class::Ord
        } else {
            Class::Inner
        };
        b.class(cl)
    }

    /// Cells in a grid: rows with struts, columns 1 em apart, centred on
    /// the axis.
    fn grid(
        &mut self,
        cells: Vec<Vec<B>>,
        aligns: &dyn Fn(usize) -> f32,
        col_gap: f32,
        row_gap: f32,
        st: St,
    ) -> B {
        let em = self.em(st);
        let cols = cells.iter().map(|r| r.len()).max().unwrap_or(0);
        let mut widths = vec![0.0f32; cols];
        let mut heights = Vec::new();
        for r in &cells {
            let mut a = 0.7 * em;
            let mut d = 0.3 * em;
            for (j, c) in r.iter().enumerate() {
                widths[j] = widths[j].max(c.w);
                a = a.max(c.asc);
                d = d.max(c.desc);
            }
            heights.push((a, d));
        }
        let total_h: f32 = heights.iter().map(|(a, d)| a + d).sum::<f32>()
            + row_gap * heights.len().saturating_sub(1) as f32;
        let axis = self.axis(st);
        let top = -(axis + total_h / 2.0);
        let mut b = B::default();
        let mut y = top;
        for (r, (a, d)) in cells.into_iter().zip(heights) {
            let base = y + a;
            let mut x = 0.0;
            for (j, c) in r.into_iter().enumerate() {
                let cw = c.w;
                let dx = (widths[j] - cw) * aligns(j);
                b.add(c, x + dx, base);
                x += widths[j] + col_gap;
            }
            y = base + d + row_gap;
        }
        b.asc = b.asc.max(-top);
        b.desc = b.desc.max(top + total_h);
        b.w = widths.iter().sum::<f32>() + col_gap * cols.saturating_sub(1) as f32;
        b
    }

    fn matrix(&mut self, rows: &[Vec<Row>], st: St) -> B {
        let cst = if st.display() { St::TEXT } else { st };
        let cells: Vec<Vec<B>> = rows
            .iter()
            .map(|r| r.iter().map(|c| self.row(c, cst)).collect())
            .collect();
        let em = self.em(st);
        self.grid(cells, &|_| 0.5, em, 0.2 * em, st)
    }

    /// An equation array: rows split at `&` into columns aligned
    /// right / left in turn. With `numbers`, each row's `#` number is
    /// returned (with the row's baseline) instead of drawn.
    #[allow(clippy::type_complexity)]
    fn eq_array(&mut self, rows: &[Row], st: St, numbers: bool) -> (B, Vec<Option<(f32, B)>>) {
        let mut cells: Vec<Vec<B>> = Vec::new();
        let mut nums: Vec<Option<B>> = Vec::new();
        let mut aligned = false;
        for r in rows {
            let (body, num) = if numbers {
                split_number(r)
            } else {
                (r.clone(), None)
            };
            let parts = split_amp(&body);
            aligned |= parts.len() > 1;
            cells.push(parts.iter().map(|p| self.row(p, st)).collect());
            nums.push(num.map(|n| self.row(&n, St::TEXT)));
        }
        let em = self.em(st);
        let align = move |j: usize| -> f32 {
            if !aligned {
                0.5
            } else if j % 2 == 0 {
                1.0
            } else {
                0.0
            }
        };
        // Baselines of the rows, for the numbers.
        let mut bases = Vec::new();
        {
            let mut heights = Vec::new();
            for r in &cells {
                let a = r.iter().map(|c| c.asc).fold(0.7 * em, f32::max);
                let d = r.iter().map(|c| c.desc).fold(0.3 * em, f32::max);
                heights.push((a, d));
            }
            let gap = 0.25 * em;
            let total: f32 = heights.iter().map(|(a, d)| a + d).sum::<f32>()
                + gap * heights.len().saturating_sub(1) as f32;
            let mut y = -(self.axis(st) + total / 2.0);
            for (a, d) in heights {
                bases.push(y + a);
                y += a + d + gap;
            }
        }
        let b = self.grid(cells, &align, 0.0, 0.25 * em, st);
        let nums = nums
            .into_iter()
            .zip(bases)
            .map(|(n, y)| n.map(|n| (y, n)))
            .collect();
        (b, nums)
    }

    fn accent(&mut self, chr: char, body: &[Node], st: St) -> B {
        let base = self.row(body, st.cramp());
        let base_w = base.w;
        let mut acc = self.char_box(chr, st);
        let acc_ink = self.ink_x(&acc);
        if base_w > acc_ink.1 - acc_ink.0 && body.len() + plain_text(body).chars().count() > 2 {
            self.text.truncate(self.text.len() - chr.len_utf8());
            acc = match self.stretch(chr, base_w, false, st) {
                Some(b) => b,
                None => self.char_box(chr, st),
            };
        }
        let (ix0, ix1) = self.ink_x(&acc);
        let acc_attach = match (acc.glyphs.len(), acc.accent_x) {
            (1, Some(a)) => a,
            _ => (ix0 + ix1) / 2.0,
        };
        let base_attach = base.accent_x.unwrap_or(base_w / 2.0);
        let accent_base = self.c(st, |c| c.accent_base_height(), 0.45);
        let shift = (base.asc - accent_base).max(0.0);
        let mut b = B::default();
        let (left, right) = (base.left, base.right);
        let italic = base.italic;
        b.add(base, 0.0, 0.0);
        let ax = base_attach - acc_attach;
        b.add(acc, ax, -shift);
        b.w = base_w;
        b.left = left;
        b.right = right;
        b.italic = italic;
        b
    }

    /// Horizontal ink extent of a box's glyphs (x min, x max).
    fn ink_x(&self, b: &B) -> (f32, f32) {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for g in &b.glyphs {
            if g.font != 0 {
                continue;
            }
            let k = g.size / self.upem;
            if let Some(r) = self.face.glyph_bounding_box(GlyphId(g.id)) {
                lo = lo.min(g.x + r.x_min as f32 * k);
                hi = hi.max(g.x + r.x_max as f32 * k);
            }
        }
        if lo.is_finite() { (lo, hi) } else { (0.0, b.w) }
    }

    fn bar(&mut self, top: bool, body: &[Node], st: St) -> B {
        let base = self.row(body, if top { st.cramp() } else { st });
        let (bw, basc, bdesc) = (base.w, base.asc, base.desc);
        let mut b = B::default();
        b.add(base, 0.0, 0.0);
        if top {
            let gap = self.c(st, |c| c.overbar_vertical_gap(), 0.15);
            let t = self.c(st, |c| c.overbar_rule_thickness(), 0.05);
            let extra = self.c(st, |c| c.overbar_extra_ascender(), 0.05);
            b.rule(0.0, -(basc + gap + t), bw, t);
            b.asc = basc + gap + t + extra;
        } else {
            let gap = self.c(st, |c| c.underbar_vertical_gap(), 0.15);
            let t = self.c(st, |c| c.underbar_rule_thickness(), 0.05);
            let extra = self.c(st, |c| c.underbar_extra_descender(), 0.05);
            b.rule(0.0, bdesc + gap, bw, t);
            b.desc = bdesc + gap + t + extra;
        }
        b.w = bw;
        b
    }

    fn boxed(&mut self, body: &[Node], st: St) -> B {
        let base = self.row(body, st);
        let pad = 0.15 * self.em(st);
        let t = self.rule_t(st).max(0.5);
        let (bw, basc, bdesc) = (base.w, base.asc, base.desc);
        let w = bw + 2.0 * (pad + t);
        let top = -(basc + pad + t);
        let h = basc + bdesc + 2.0 * (pad + t);
        let mut b = B::default();
        b.add(base, pad + t, 0.0);
        b.rule(0.0, top, w, t);
        b.rule(0.0, top + h - t, w, t);
        b.rule(0.0, top, t, h);
        b.rule(w - t, top, t, h);
        b.asc = -top;
        b.desc = top + h;
        b.w = w;
        b
    }

    fn group_chr(&mut self, chr: char, top: bool, body: &[Node], st: St) -> B {
        let base = self.row(body, st);
        let bw = base.w;
        let mut g = match self.stretch(chr, bw, false, st) {
            Some(b) => b,
            None => self.char_box(chr, st),
        };
        let gw = g.w;
        let (gasc, gdesc) = (g.asc, g.desc);
        let (basc, bdesc) = (base.asc, base.desc);
        let w = bw.max(gw);
        let mut b = B::default();
        b.add(base, (w - bw) / 2.0, 0.0);
        if top {
            let gap = self.c(st, |c| c.stretch_stack_gap_below_min(), 0.1);
            b.add(g, (w - gw) / 2.0, -(basc + gap + gdesc));
        } else {
            let gap = self.c(st, |c| c.stretch_stack_gap_above_min(), 0.1);
            b.add(g, (w - gw) / 2.0, bdesc + gap + gasc);
        }
        b.w = w;
        b
    }

    fn limit(&mut self, base: B, upper: bool, limit: &[Node], st: St) -> B {
        let l = self.row(limit, if upper { st.sup() } else { st.sub() });
        if !st.display() && base.left == Class::Op {
            // `lim` in a line of text: the limit as a subscript.
            return if upper {
                self.attach(base, None, Some(l), st)
            } else {
                self.attach(base, Some(l), None, st)
            };
        }
        let (left, right) = (base.left, base.right);
        let mut b = if upper {
            self.under_over(base, Some(l), None, st, false)
        } else {
            self.under_over(base, None, Some(l), st, false)
        };
        b.left = left;
        b.right = right;
        b
    }

    fn func(&mut self, name: &[Node], body: &[Node], st: St) -> B {
        // The name upright, as an operator.
        let name: Row = name
            .iter()
            .map(|n| match n {
                Node::Text(r) if r.style.is_default() => {
                    Node::Text(Run::styled(r.text.clone(), Style::PLAIN))
                }
                Node::Limit { upper, base, limit } => Node::Limit {
                    upper: *upper,
                    base: base
                        .iter()
                        .map(|b| match b {
                            Node::Text(r) if r.style.is_default() => {
                                Node::Text(Run::styled(r.text.clone(), Style::PLAIN))
                            }
                            b => b.clone(),
                        })
                        .collect(),
                    limit: limit.clone(),
                },
                n => n.clone(),
            })
            .collect();
        let name_b = match name.as_slice() {
            [Node::Limit { upper, base, limit }] => {
                let mut bb = self.word(base, st);
                bb.left = Class::Op;
                bb.right = Class::Op;
                self.limit(bb, *upper, limit, st)
            }
            [Node::Scripts { base, sub, sup }] => {
                let bb = self.word(base, st);
                self.scripts(bb, sub.as_deref(), sup.as_deref(), st)
            }
            _ => self.word(&name, st),
        };
        let mut atoms = vec![name_b.class(Class::Op)];
        for n in body {
            self.node(n, st, &mut atoms);
        }
        let mut b = self.hpack(atoms, st);
        b.left = Class::Op;
        b
    }

    /// A function name: its letters side by side with no spacing.
    fn word(&mut self, r: &[Node], st: St) -> B {
        let mut b = B::default();
        let mut x = 0.0;
        for n in r {
            let item = match n {
                Node::Text(t) => {
                    let mut row = B::default();
                    let mut rx = 0.0;
                    for c in t.text.chars() {
                        if c == ' ' {
                            continue;
                        }
                        let g = self.math_char(c, &t.style, st);
                        let w = g.w;
                        row.add(g, rx, 0.0);
                        rx += w;
                    }
                    row.w = rx;
                    row
                }
                n => self.item(n, st),
            };
            let w = item.w;
            b.add(item, x, 0.0);
            x += w;
        }
        b.w = x;
        b
    }
}

/// A row cut at its `&` alignment points.
fn split_amp(row: &[Node]) -> Vec<Row> {
    let mut out = vec![Row::new()];
    for n in row {
        match n {
            Node::Text(t) if !t.style.normal && t.text.contains('&') => {
                let mut first = true;
                for piece in t.text.split('&') {
                    if !first {
                        out.push(Row::new());
                    }
                    first = false;
                    if !piece.is_empty()
                        && let Some(last) = out.last_mut()
                    {
                        last.push(Node::Text(Run::styled(piece, t.style)));
                    }
                }
            }
            n => {
                if let Some(last) = out.last_mut() {
                    last.push(n.clone());
                }
            }
        }
    }
    out
}
