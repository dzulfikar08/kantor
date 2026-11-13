//! Text: paragraph and character properties ([MS-PPT] 2.9 TextPFException,
//! TextCFException), master text styles, rulers, and DrawingML paragraphs.

use super::rec::{Cur, Rec, rt};
use super::{Color, Ctx, MU_EMU, esc};

/// Paragraph properties; `None` = not set here (inherited).
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Pf {
    pub has_bullet: Option<bool>,
    pub bullet_has_font: Option<bool>,
    pub bullet_has_color: Option<bool>,
    pub bullet_has_size: Option<bool>,
    pub bullet_char: Option<u16>,
    pub bullet_font: Option<u16>,
    /// Percent of the text size when positive, points when negative.
    pub bullet_size: Option<i16>,
    pub bullet_color: Option<Color>,
    pub align: Option<u16>,
    /// Percent when positive, master units (1/8 pt) when negative.
    pub line_spacing: Option<i16>,
    pub space_before: Option<i16>,
    pub space_after: Option<i16>,
    /// Text start, master units.
    pub left_margin: Option<i16>,
    /// Bullet (first line) start, master units.
    pub indent: Option<i16>,
}

/// Character properties; `None` = inherited.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Cf {
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub font: Option<u16>,
    pub ea_font: Option<u16>,
    pub symbol_font: Option<u16>,
    /// Points.
    pub size: Option<u16>,
    pub color: Option<Color>,
    /// Superscript (+) / subscript (−) offset in percent.
    pub position: Option<i16>,
}

macro_rules! over {
    ($self:ident, $o:ident, $($f:ident),*) => { $( if $o.$f.is_some() { $self.$f = $o.$f.clone(); } )* };
}

impl Pf {
    pub fn over(&mut self, o: &Pf) {
        over!(
            self,
            o,
            has_bullet,
            bullet_has_font,
            bullet_has_color,
            bullet_has_size,
            bullet_char,
            bullet_font,
            bullet_size,
            bullet_color,
            align,
            line_spacing,
            space_before,
            space_after,
            left_margin,
            indent
        );
    }
}

impl Cf {
    pub fn over(&mut self, o: &Cf) {
        over!(
            self,
            o,
            bold,
            italic,
            underline,
            font,
            ea_font,
            symbol_font,
            size,
            color,
            position
        );
    }
}

pub(super) fn color_index(v: u32) -> Option<Color> {
    let [r, g, b, i] = v.to_le_bytes();
    match i {
        0xFE => Some(Color::Rgb(r, g, b)),
        0..=7 => Some(Color::Scheme(i)),
        _ => None,
    }
}

/// TextPFException.
pub(super) fn read_pf(c: &mut Cur) -> Option<Pf> {
    let m = c.u32()?;
    let mut pf = Pf::default();
    if m & 0xF != 0 {
        let f = c.u16()?;
        let bit = |mask: u32, b: u16| (m & mask != 0).then_some(f & b != 0);
        pf.has_bullet = bit(0x1, 0x1);
        pf.bullet_has_font = bit(0x2, 0x2);
        pf.bullet_has_color = bit(0x4, 0x4);
        pf.bullet_has_size = bit(0x8, 0x8);
    }
    if m & 0x80 != 0 {
        pf.bullet_char = Some(c.u16()?);
    }
    if m & 0x10 != 0 {
        pf.bullet_font = Some(c.u16()?);
    }
    if m & 0x40 != 0 {
        pf.bullet_size = Some(c.i16()?);
    }
    if m & 0x20 != 0 {
        pf.bullet_color = color_index(c.u32()?);
    }
    if m & 0x800 != 0 {
        pf.align = Some(c.u16()?);
    }
    if m & 0x1000 != 0 {
        pf.line_spacing = Some(c.i16()?);
    }
    if m & 0x2000 != 0 {
        pf.space_before = Some(c.i16()?);
    }
    if m & 0x4000 != 0 {
        pf.space_after = Some(c.i16()?);
    }
    if m & 0x100 != 0 {
        pf.left_margin = Some(c.i16()?);
    }
    if m & 0x400 != 0 {
        pf.indent = Some(c.i16()?);
    }
    if m & 0x8000 != 0 {
        c.skip(2)?; // defaultTabSize
    }
    if m & 0x10_0000 != 0 {
        let n = c.u16()? as usize;
        c.skip(n.checked_mul(4)?)?;
    }
    if m & 0x1_0000 != 0 {
        c.skip(2)?; // fontAlign
    }
    if m & 0xE_0000 != 0 {
        c.skip(2)?; // wrapFlags
    }
    if m & 0x20_0000 != 0 {
        c.skip(2)?; // textDirection
    }
    Some(pf)
}

/// TextCFException.
pub(super) fn read_cf(c: &mut Cur) -> Option<Cf> {
    let m = c.u32()?;
    let mut cf = Cf::default();
    if m & 0xFFFF != 0 {
        let s = c.u16()?;
        let bit = |mask: u32, b: u16| (m & mask != 0).then_some(s & b != 0);
        cf.bold = bit(0x1, 0x1);
        cf.italic = bit(0x2, 0x2);
        cf.underline = bit(0x4, 0x4);
    }
    if m & 0x1_0000 != 0 {
        cf.font = Some(c.u16()?);
    }
    if m & 0x20_0000 != 0 {
        cf.ea_font = Some(c.u16()?);
    }
    if m & 0x40_0000 != 0 {
        c.skip(2)?; // ansiFontRef
    }
    if m & 0x80_0000 != 0 {
        cf.symbol_font = Some(c.u16()?);
    }
    if m & 0x2_0000 != 0 {
        cf.size = Some(c.u16()?);
    }
    if m & 0x4_0000 != 0 {
        cf.color = color_index(c.u32()?);
    }
    if m & 0x8_0000 != 0 {
        cf.position = Some(c.i16()?);
    }
    Some(cf)
}

/// One level of a master text style.
pub(super) type Level = (Pf, Cf);

/// A TextMasterStyleAtom: up to five levels.
pub(super) fn read_master_style(r: &Rec) -> Vec<Option<Level>> {
    let mut levels = vec![None; 5];
    let mut c = r.cur();
    let Some(n) = c.u16() else { return levels };
    for i in 0..n.min(5) as usize {
        let lvl = if r.inst >= 5 {
            match c.u16() {
                Some(l) => (l as usize).min(4),
                None => break,
            }
        } else {
            i
        };
        let Some(pf) = read_pf(&mut c) else { break };
        let Some(cf) = read_cf(&mut c) else { break };
        levels[lvl] = Some((pf, cf));
    }
    levels
}

/// Text types (TextHeaderAtom.textType), which pick the master style.
pub(super) mod tx {
    pub const TITLE: u32 = 0;
    pub const BODY: u32 = 1;
    pub const NOTES: u32 = 2;
    pub const OTHER: u32 = 4;
    pub const CENTER_BODY: u32 = 5;
    pub const CENTER_TITLE: u32 = 6;
}

/// Master text styles by text type (0..=8), each with five levels.
#[derive(Clone, Debug, Default)]
pub(super) struct Styles {
    pub types: Vec<Vec<Option<Level>>>,
}

impl Styles {
    pub fn read(container: &Rec) -> Self {
        let mut types = vec![Vec::new(); 9];
        for r in container.kids(rt::TEXT_MASTER_STYLE) {
            if let Some(t) = types.get_mut(r.inst as usize) {
                *t = read_master_style(&r);
            }
        }
        Self { types }
    }

    fn get(&self, ty: u32, lvl: usize) -> Option<&Level> {
        self.types.get(ty as usize)?.get(lvl)?.as_ref()
    }

    /// The fully resolved style of `ty` at `lvl`: built-in defaults, then
    /// `doc` (the document's default style), then the parent type (body for
    /// the centred/half/quarter bodies, title for the centred title), then
    /// the type itself; each level inherits from the levels below it.
    pub fn resolve(&self, doc: &Styles, ty: u32, lvl: usize) -> Level {
        let mut pf = Pf {
            has_bullet: Some(false),
            align: Some(0),
            line_spacing: Some(100),
            space_before: Some(0),
            space_after: Some(0),
            left_margin: Some(0),
            indent: Some(0),
            ..Pf::default()
        };
        let mut cf = Cf {
            bold: Some(false),
            italic: Some(false),
            underline: Some(false),
            font: Some(0),
            size: Some(18),
            color: Some(Color::Scheme(
                if ty == tx::TITLE || ty == tx::CENTER_TITLE {
                    3
                } else {
                    1
                },
            )),
            ..Cf::default()
        };
        let lvl = lvl.min(4);
        let mut apply = |s: &Styles, t: u32| {
            for l in 0..=lvl {
                if let Some((p, c)) = s.get(t, l) {
                    pf.over(p);
                    cf.over(c);
                }
            }
        };
        apply(doc, tx::OTHER);
        let parent = match ty {
            5 | 7 | 8 => Some(tx::BODY),
            6 => Some(tx::TITLE),
            _ => None,
        };
        if let Some(p) = parent {
            apply(self, p);
        }
        apply(self, ty);
        (pf, cf)
    }
}

/// TextRulerAtom: per-level margins (master units).
#[derive(Clone, Debug, Default)]
pub(super) struct Ruler {
    pub margin: [Option<i16>; 5],
    pub indent: [Option<i16>; 5],
}

pub(super) fn read_ruler(r: &Rec) -> Ruler {
    let mut ru = Ruler::default();
    let mut c = r.cur();
    let Some(m) = c.u32() else { return ru };
    let ok = (|| {
        if m & 0x1 != 0 {
            c.skip(2)?;
        }
        if m & 0x2 != 0 {
            c.skip(2)?;
        }
        if m & 0x4 != 0 {
            let n = c.u16()? as usize;
            c.skip(n.checked_mul(4)?)?;
        }
        for l in 0..5 {
            if m & (0x8 << l) != 0 {
                ru.margin[l] = Some(c.i16()?);
            }
            if m & (0x100 << l) != 0 {
                ru.indent[l] = Some(c.i16()?);
            }
        }
        Some(())
    })();
    let _ = ok;
    ru
}

/// A field in the text (a `*` placeholder character).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Field {
    SlideNumber,
    Date,
    /// Header/footer text: the character is dropped.
    Drop,
}

/// A text block: characters and their formatting.
#[derive(Clone, Debug, Default)]
pub(super) struct Text {
    pub ty: u32,
    pub chars: Vec<u16>,
    /// `(count, level, pf)` paragraph runs.
    pub pf_runs: Vec<(u32, u16, Pf)>,
    pub cf_runs: Vec<(u32, Cf)>,
    /// `(count, level)` from a MasterTextPropAtom.
    pub master_levels: Vec<(u32, u16)>,
    pub ruler: Option<Ruler>,
    pub fields: Vec<(u32, Field)>,
}

impl Text {
    /// Read the text atoms in `recs` (a ClientTextbox's children or a block
    /// of SlideListWithText); `ty` is used when there is no TextHeaderAtom.
    pub fn read<'a>(recs: impl Iterator<Item = Rec<'a>>, ty: u32) -> Option<Text> {
        let mut t = Text {
            ty,
            ..Text::default()
        };
        let mut style = None;
        let mut has_text = false;
        for r in recs {
            match r.ty {
                rt::TEXT_HEADER => {
                    if let Some(v) = r.cur().u32() {
                        t.ty = v;
                    }
                }
                rt::TEXT_CHARS => {
                    has_text = true;
                    t.chars = r
                        .data
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .map(|c| u16::from_le_bytes(*c))
                        .collect();
                }
                rt::TEXT_BYTES => {
                    has_text = true;
                    t.chars = r.data.iter().map(|&b| b as u16).collect();
                }
                rt::STYLE_TEXT_PROP => style = Some(r),
                rt::MASTER_TEXT_PROP => {
                    let mut c = r.cur();
                    while let (Some(n), Some(l)) = (c.u32(), c.u16()) {
                        t.master_levels.push((n, l));
                    }
                }
                rt::TEXT_RULER => t.ruler = Some(read_ruler(&r)),
                rt::SLIDE_NUMBER_MC => push_field(&mut t, &r, Field::SlideNumber),
                rt::DATE_TIME_MC | rt::GENERIC_DATE_MC | rt::RTF_DATE_TIME_MC => {
                    push_field(&mut t, &r, Field::Date)
                }
                rt::HEADER_MC | rt::FOOTER_MC => push_field(&mut t, &r, Field::Drop),
                _ => {}
            }
        }
        if !has_text {
            return None;
        }
        if let Some(s) = style {
            t.read_style(&s);
        }
        Some(t)
    }

    fn read_style(&mut self, r: &Rec) {
        let total = self.chars.len() as u64 + 1;
        let mut c = r.cur();
        let mut seen = 0u64;
        while seen < total {
            let (Some(n), Some(l)) = (c.u32(), c.u16()) else {
                return;
            };
            let Some(pf) = read_pf(&mut c) else { return };
            self.pf_runs.push((n, l, pf));
            if n == 0 {
                break;
            }
            seen += n as u64;
        }
        seen = 0;
        while seen < total {
            let Some(n) = c.u32() else { return };
            let Some(cf) = read_cf(&mut c) else { return };
            self.cf_runs.push((n, cf));
            if n == 0 {
                break;
            }
            seen += n as u64;
        }
    }

    fn pf_at(&self, pos: u32) -> (u16, Option<&Pf>) {
        let mut start = 0u32;
        for (n, l, pf) in &self.pf_runs {
            if pos < start.saturating_add(*n) {
                return (*l, Some(pf));
            }
            start = start.saturating_add(*n);
        }
        (0, None)
    }

    fn master_level_at(&self, pos: u32) -> Option<u16> {
        let mut start = 0u32;
        for (n, l) in &self.master_levels {
            if pos < start.saturating_add(*n) {
                return Some(*l);
            }
            start = start.saturating_add(*n);
        }
        None
    }

    /// The character runs: `(start, end, cf)`, covering the text.
    fn cf_spans(&self) -> Vec<(u32, u32, Option<&Cf>)> {
        let len = self.chars.len() as u32;
        let mut v = Vec::new();
        let mut start = 0u32;
        for (n, cf) in &self.cf_runs {
            if start >= len {
                break;
            }
            let end = start.saturating_add(*n).min(len);
            if end > start {
                v.push((start, end, Some(cf)));
            }
            start = end;
        }
        if start < len {
            v.push((start, len, None));
        }
        v
    }
}

fn push_field(t: &mut Text, r: &Rec, f: Field) {
    if let Some(p) = r.cur().i32()
        && p >= 0
    {
        t.fields.push((p as u32, f));
    }
}

fn emu(mu: i64) -> i64 {
    (mu as f64 * MU_EMU).round() as i64
}

/// `a:pPr`/`a:lvlNpPr` children and attributes for `pf` (only what is set).
/// `full` is the resolved style, for bullet details and the margins.
pub(super) fn ppr_parts(
    cx: &Ctx,
    pf: &Pf,
    full: &Pf,
    margins: Option<(i16, i16)>,
) -> (String, String) {
    let mut attrs = String::new();
    if let Some((margin, indent)) = margins {
        attrs.push_str(&format!(
            " marL=\"{}\" indent=\"{}\"",
            emu(margin as i64).max(0),
            emu(indent as i64 - margin as i64)
        ));
    }
    if let Some(a) = pf.align {
        let v = match a {
            1 => "ctr",
            2 => "r",
            3 | 6 => "just",
            4 | 5 => "dist",
            _ => "l",
        };
        attrs.push_str(&format!(" algn=\"{v}\""));
    }
    let mut kids = String::new();
    let spacing = |name: &str, v: i16, kids: &mut String| {
        if v >= 0 {
            kids.push_str(&format!(
                "<a:{name}><a:spcPct val=\"{}\"/></a:{name}>",
                v as i64 * 1000
            ));
        } else {
            // Master units are 1/8 pt; spcPts is in 1/100 pt.
            kids.push_str(&format!(
                "<a:{name}><a:spcPts val=\"{}\"/></a:{name}>",
                (-(v as i64) * 100 / 8).min(158400)
            ));
        }
    };
    if let Some(v) = pf.line_spacing {
        spacing("lnSpc", v, &mut kids);
    }
    if let Some(v) = pf.space_before {
        spacing("spcBef", v, &mut kids);
    }
    if let Some(v) = pf.space_after {
        spacing("spcAft", v, &mut kids);
    }
    let bullet_set = pf.has_bullet.is_some()
        || pf.bullet_char.is_some()
        || pf.bullet_font.is_some()
        || pf.bullet_color.is_some()
        || pf.bullet_size.is_some();
    if bullet_set {
        if full.has_bullet == Some(true) {
            if full.bullet_has_color == Some(true)
                && let Some(c) = full.bullet_color.and_then(|c| cx.rgb(c))
            {
                kids.push_str(&format!("<a:buClr><a:srgbClr val=\"{c}\"/></a:buClr>"));
            }
            if full.bullet_has_size == Some(true)
                && let Some(s) = full.bullet_size
            {
                if s > 0 {
                    kids.push_str(&format!(
                        "<a:buSzPct val=\"{}\"/>",
                        (s as i64).clamp(25, 400) * 1000
                    ));
                } else if s < 0 {
                    kids.push_str(&format!(
                        "<a:buSzPts val=\"{}\"/>",
                        (-(s as i64) * 100).min(400000)
                    ));
                }
            }
            let font = (full.bullet_has_font == Some(true))
                .then_some(full.bullet_font)
                .flatten()
                .and_then(|f| cx.font(f));
            if let Some(f) = &font {
                kids.push_str(&format!("<a:buFont typeface=\"{}\"/>", esc(f)));
            }
            let ch = bullet_char(full.bullet_char.unwrap_or(0x2022));
            kids.push_str(&format!("<a:buChar char=\"{}\"/>", esc(&ch)));
        } else {
            kids.push_str("<a:buNone/>");
        }
    }
    (attrs, kids)
}

fn bullet_char(c: u16) -> String {
    // Symbol-font characters are often stored in the private use area.
    let c = if (0xF000..=0xF0FF).contains(&c) {
        c - 0xF000
    } else {
        c
    };
    let c = match c {
        0 | 0x0D | 0x0B => 0x2022,
        c => c,
    };
    String::from_utf16_lossy(&[c])
}

/// `a:rPr`/`a:defRPr` attributes and children for `cf`.
pub(super) fn rpr_parts(cx: &Ctx, cf: &Cf) -> (String, String) {
    let mut attrs = String::new();
    if let Some(s) = cf.size {
        attrs.push_str(&format!(" sz=\"{}\"", (s as u32).clamp(1, 4000) * 100));
    }
    if let Some(b) = cf.bold {
        attrs.push_str(if b { " b=\"1\"" } else { " b=\"0\"" });
    }
    if let Some(i) = cf.italic {
        attrs.push_str(if i { " i=\"1\"" } else { " i=\"0\"" });
    }
    if let Some(u) = cf.underline {
        attrs.push_str(if u { " u=\"sng\"" } else { " u=\"none\"" });
    }
    if let Some(p) = cf.position
        && p != 0
    {
        attrs.push_str(&format!(
            " baseline=\"{}\"",
            (p as i64).clamp(-100, 100) * 1000
        ));
    }
    let mut kids = String::new();
    if let Some(c) = cf.color.and_then(|c| cx.rgb(c)) {
        kids.push_str(&format!(
            "<a:solidFill><a:srgbClr val=\"{c}\"/></a:solidFill>"
        ));
    }
    if let Some(f) = cf.font.and_then(|f| cx.font(f)) {
        kids.push_str(&format!("<a:latin typeface=\"{}\"/>", esc(&f)));
    }
    if let Some(f) = cf.ea_font.and_then(|f| cx.font(f)) {
        kids.push_str(&format!("<a:ea typeface=\"{}\"/>", esc(&f)));
    }
    if let Some(f) = cf.symbol_font.and_then(|f| cx.font(f)) {
        kids.push_str(&format!("<a:sym typeface=\"{}\"/>", esc(&f)));
    }
    (attrs, kids)
}

/// `a:lvl1pPr`..`a:lvl9pPr` of a fully resolved style (levels 6-9 repeat 5).
pub(super) fn list_style(cx: &Ctx, levels: &[Level]) -> String {
    let mut s = String::new();
    for n in 0..9 {
        let Some((pf, cf)) = levels.get(n.min(levels.len().saturating_sub(1))) else {
            break;
        };
        let margins = Some((pf.left_margin.unwrap_or(0), pf.indent.unwrap_or(0)));
        let (pa, pk) = ppr_parts(cx, pf, pf, margins);
        let (ra, rk) = rpr_parts(cx, cf);
        s.push_str(&format!(
            "<a:lvl{l}pPr{pa}>{pk}<a:defRPr{ra}>{rk}</a:defRPr></a:lvl{l}pPr>",
            l = n + 1
        ));
    }
    s
}

/// The paragraphs (`a:p` elements) of `t`. `style` gives the resolved
/// master style of a level (for margins and bullet details).
pub(super) fn paragraphs(cx: &Ctx, t: &Text, style: &dyn Fn(usize) -> Level) -> String {
    let mut out = String::new();
    let spans = t.cf_spans();
    let len = t.chars.len() as u32;
    let mut start = 0u32;
    loop {
        let end = t.chars[start as usize..]
            .iter()
            .position(|&c| c == 0x0D)
            .map(|p| start + p as u32)
            .unwrap_or(len);
        let (mut lvl, pf) = t.pf_at(start);
        if let Some(l) = t.master_level_at(start) {
            lvl = l;
        }
        let lvl = (lvl as usize).min(4);
        let (base_pf, _) = style(lvl);
        let mut full = base_pf.clone();
        let own = pf.cloned().unwrap_or_default();
        full.over(&own);
        // Margins: the paragraph's own, else the text's ruler.
        let ruler = t.ruler.as_ref();
        let margin = own
            .left_margin
            .or_else(|| ruler.and_then(|r| r.margin[lvl]));
        let indent = own.indent.or_else(|| ruler.and_then(|r| r.indent[lvl]));
        let margins = (margin.is_some() || indent.is_some()).then(|| {
            (
                margin.or(full.left_margin).unwrap_or(0),
                indent.or(full.indent).unwrap_or(0),
            )
        });
        let (mut pa, pk) = ppr_parts(cx, &own, &full, margins);
        if lvl > 0 {
            pa = format!(" lvl=\"{lvl}\"{pa}");
        }
        out.push_str("<a:p>");
        if !pa.is_empty() || !pk.is_empty() {
            out.push_str(&format!("<a:pPr{pa}>{pk}</a:pPr>"));
        }
        let mut last_rpr = String::new();
        for &(s, e, cf) in &spans {
            let (s, e) = (s.max(start), e.min(end));
            if s >= e {
                continue;
            }
            let (ra, rk) = cf.map(|c| rpr_parts(cx, c)).unwrap_or_default();
            last_rpr = format!("{ra}>{rk}");
            write_run(&mut out, t, s, e, &ra, &rk);
        }
        if last_rpr.is_empty() {
            // An empty paragraph keeps the size of its mark.
            let cf = spans
                .iter()
                .find(|(s, e, _)| *s <= end && end < (*e).max(*s + 1))
                .and_then(|(_, _, c)| *c)
                .or_else(|| t.cf_runs.last().map(|(_, c)| c));
            let (ra, rk) = cf.map(|c| rpr_parts(cx, c)).unwrap_or_default();
            last_rpr = format!("{ra}>{rk}");
        }
        out.push_str(&format!(
            "<a:endParaRPr lang=\"en-US\"{last_rpr}</a:endParaRPr></a:p>"
        ));
        if end >= len {
            break;
        }
        start = end + 1;
    }
    out
}

fn write_run(out: &mut String, t: &Text, s: u32, e: u32, ra: &str, rk: &str) {
    let mut buf: Vec<u16> = Vec::new();
    let flush = |buf: &mut Vec<u16>, out: &mut String| {
        if buf.is_empty() {
            return;
        }
        let text: String = String::from_utf16_lossy(buf)
            .chars()
            .filter(|c| *c == '\t' || !c.is_control())
            .collect();
        buf.clear();
        out.push_str(&format!(
            "<a:r><a:rPr lang=\"en-US\"{ra}>{rk}</a:rPr><a:t>{}</a:t></a:r>",
            esc(&text)
        ));
    };
    for i in s..e {
        let c = t.chars[i as usize];
        let field = t.fields.iter().find(|(p, _)| *p == i).map(|(_, f)| *f);
        match (c, field) {
            (_, Some(Field::Drop)) => {}
            (_, Some(f)) => {
                flush(&mut buf, out);
                let (ty, shown) = match f {
                    Field::SlideNumber => ("slidenum", "‹#›"),
                    _ => ("datetime1", ""),
                };
                out.push_str(&format!(
                    "<a:fld id=\"{{B6F15528-21DE-4FAA-801E-634DDDAF4B2B}}\" type=\"{ty}\"><a:rPr lang=\"en-US\"{ra}>{rk}</a:rPr><a:t>{shown}</a:t></a:fld>"
                ));
            }
            (0x0B, None) => {
                flush(&mut buf, out);
                out.push_str(&format!(
                    "<a:br><a:rPr lang=\"en-US\"{ra}>{rk}</a:rPr></a:br>"
                ));
            }
            (c, None) => buf.push(c),
        }
    }
    flush(&mut buf, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncated_exceptions_are_rejected() {
        // Masks say bold/italic and a font follow, but the bytes stop.
        let b = [0x03, 0x00, 0x01, 0x00, 0x01];
        assert!(read_cf(&mut Cur::new(&b)).is_none());
        let b = [0x00, 0x08, 0x00, 0x00, 0x01, 0x00];
        let pf = read_pf(&mut Cur::new(&b)).unwrap();
        assert_eq!(pf.align, Some(1));
    }
}
