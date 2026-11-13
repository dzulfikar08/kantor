//! The parsed RTF tree written as a WordprocessingML package, which the
//! `.docx` reader then opens like any other file.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::{Cursor, Write as _};

use super::parse::*;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const R_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// XML text or attribute value; drops characters XML 1.0 can't hold.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\t' | '\n' | '\r' => out.push(ch),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

#[derive(Default)]
struct Rels(Vec<(String, &'static str, String, bool)>);

impl Rels {
    fn add(&mut self, ty: &'static str, target: String, external: bool) -> String {
        let id = format!("rId{}", self.0.len() + 1);
        self.0.push((id.clone(), ty, target, external));
        id
    }

    fn xml(&self) -> String {
        let mut s = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
        );
        for (id, ty, target, external) in &self.0 {
            let _ = write!(
                s,
                "<Relationship Id=\"{id}\" Type=\"{REL}/{ty}\" Target=\"{}\"{}/>",
                esc(target),
                if *external {
                    " TargetMode=\"External\""
                } else {
                    ""
                }
            );
        }
        s.push_str("</Relationships>");
        s
    }
}

/// A numbered or bulleted list level, ready to write.
struct LevelOut {
    start: i32,
    fmt: &'static str,
    suff: Option<&'static str>,
    text: String,
    jc: &'static str,
    li: i32,
    fi: i32,
    font: Option<String>,
}

struct Writer<'a> {
    d: &'a Parsed,
    para_ids: HashMap<i32, String>,
    char_ids: HashMap<i32, String>,
    /// `\ls` → `w:numId`.
    nums: BTreeMap<i32, u32>,
    /// Old-style lists: definition → `w:numId`.
    pn_nums: Vec<(Pn, u32)>,
    media: Vec<(String, Vec<u8>)>,
    doc_pr: u32,
    parts: Vec<(String, String)>,
    hf_count: u32,
}

fn nfc_format(nfc: i32) -> &'static str {
    match nfc {
        0 => "decimal",
        1 => "upperRoman",
        2 => "lowerRoman",
        3 => "upperLetter",
        4 => "lowerLetter",
        5 => "ordinal",
        6 => "cardinalText",
        7 => "ordinalText",
        22 => "decimalZero",
        23 => "bullet",
        255 => "none",
        _ => "decimal",
    }
}

/// A bullet character from a symbol font as the Unicode character it
/// shows (so it needs no special font), if it is one.
fn plain_bullet(ch: char, font: Option<&str>) -> Option<char> {
    let symbolic = font.is_some_and(|f| {
        let f = f.to_ascii_lowercase();
        f.contains("symbol") || f.contains("wingdings")
    });
    let code = ch as u32;
    let byte = if (0xF000..=0xF0FF).contains(&code) {
        code - 0xF000
    } else if symbolic && code < 0x100 {
        code
    } else if symbolic && ch == '·' {
        0xB7
    } else {
        return None;
    };
    Some(match byte {
        0xA7 => '\u{25AA}',
        0xD8 => '\u{27A2}',
        0xFC => '\u{2713}',
        0x6F => '\u{25E6}',
        0x71 => '\u{25AA}',
        0x76 => '\u{2756}',
        _ => '\u{2022}',
    })
}

/// The 16 colours `w:highlight` can name.
const HIGHLIGHTS: [(&str, [u8; 3]); 16] = [
    ("black", [0, 0, 0]),
    ("blue", [0, 0, 255]),
    ("cyan", [0, 255, 255]),
    ("green", [0, 255, 0]),
    ("magenta", [255, 0, 255]),
    ("red", [255, 0, 0]),
    ("yellow", [255, 255, 0]),
    ("white", [255, 255, 255]),
    ("darkBlue", [0, 0, 128]),
    ("darkCyan", [0, 128, 128]),
    ("darkGreen", [0, 128, 0]),
    ("darkMagenta", [128, 0, 128]),
    ("darkRed", [128, 0, 0]),
    ("darkYellow", [128, 128, 0]),
    ("darkGray", [128, 128, 128]),
    ("lightGray", [192, 192, 192]),
];

fn highlight_name(rgb: [u8; 3]) -> &'static str {
    let dist = |c: [u8; 3]| {
        (0..3)
            .map(|i| (i32::from(c[i]) - i32::from(rgb[i])).pow(2))
            .sum::<i32>()
    };
    HIGHLIGHTS
        .iter()
        .min_by_key(|(_, c)| dist(*c))
        .map_or("yellow", |(n, _)| n)
}

fn hex(rgb: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// A style id from a style name: `heading 1` → `Heading1`.
fn style_id(name: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for ch in name.chars() {
        if ch.is_alphanumeric() {
            if upper {
                out.extend(ch.to_uppercase());
            } else {
                out.push(ch);
            }
            upper = false;
        } else {
            upper = true;
        }
    }
    if out.is_empty() {
        out.push_str("Style");
    }
    out
}

/// `heading N` (any case) → N.
fn heading_level(name: &str) -> Option<u8> {
    let rest = name.trim().to_ascii_lowercase();
    let n = rest.strip_prefix("heading")?.trim().parse::<u8>().ok()?;
    (1..=9).contains(&n).then_some(n)
}

fn line_spacing(p: &PFmt) -> (i32, &'static str) {
    match p.sl {
        0 => (240, "auto"),
        n if p.slmult => (n.abs().max(1), "auto"),
        n if n > 0 => (n, "atLeast"),
        n => (-n, "exact"),
    }
}

fn border_xml(tag: &str, b: &Option<Brdr>, colors: &[Option<[u8; 3]>]) -> String {
    match b {
        None => String::new(),
        Some(b) => {
            let sz = (b.width * 2 / 5).clamp(2, 96);
            let color = colors
                .get(b.color as usize)
                .copied()
                .flatten()
                .map_or("auto".to_owned(), hex);
            format!(
                "<w:{tag} w:val=\"{}\" w:sz=\"{sz}\" w:space=\"0\" w:color=\"{color}\"/>",
                b.kind
            )
        }
    }
}

impl<'a> Writer<'a> {
    fn new(d: &'a Parsed) -> Self {
        let mut para_ids = HashMap::new();
        let mut char_ids = HashMap::new();
        let mut used: Vec<String> = Vec::new();
        for s in &d.styles {
            let id = match s.kind {
                StyleKind::Para if s.idx == 0 => "Normal".to_owned(),
                StyleKind::Para => match heading_level(&s.name) {
                    Some(n) => format!("Heading{n}"),
                    None => style_id(&s.name),
                },
                StyleKind::Char if s.name.eq_ignore_ascii_case("Default Paragraph Font") => {
                    continue;
                }
                StyleKind::Char => style_id(&s.name),
                StyleKind::Other => continue,
            };
            let mut unique = id.clone();
            let mut n = 2;
            while used.contains(&unique) || (s.idx != 0 && unique == "Normal") {
                unique = format!("{id}{n}");
                n += 1;
            }
            used.push(unique.clone());
            match s.kind {
                StyleKind::Para => para_ids.insert(s.idx, unique),
                _ => char_ids.insert(s.idx, unique),
            };
        }
        let mut nums = BTreeMap::new();
        let mut next = 1;
        for (&ls, list) in &d.overrides {
            if d.lists.contains_key(list) {
                nums.insert(ls, next);
                next += 1;
            }
        }
        Self {
            d,
            para_ids,
            char_ids,
            nums,
            pn_nums: Vec::new(),
            media: Vec::new(),
            doc_pr: 1,
            parts: Vec::new(),
            hf_count: 0,
        }
    }

    fn style(&self, kind: StyleKind, idx: i32) -> Option<&'a Style> {
        self.d
            .styles
            .iter()
            .find(|s| s.kind == kind && s.idx == idx)
    }

    fn rgb(&self, idx: i32) -> Option<[u8; 3]> {
        if idx <= 0 {
            return None;
        }
        self.d.colors.get(idx as usize).copied().flatten()
    }

    fn font_name(&self, idx: Option<i32>) -> Option<&'a str> {
        idx.and_then(|i| self.d.fonts.get(&i))
            .map(|f| f.name.as_str())
    }

    fn fonts_xml(&self, idx: Option<i32>) -> String {
        let Some(f) = idx.and_then(|i| self.d.fonts.get(&i)) else {
            return String::new();
        };
        let name = esc(&f.name);
        let east = if matches!(f.charset, 128 | 129 | 130 | 134 | 136) {
            format!(" w:eastAsia=\"{name}\"")
        } else {
            String::new()
        };
        format!("<w:rFonts w:ascii=\"{name}\" w:hAnsi=\"{name}\"{east} w:cs=\"{name}\"/>")
    }

    /// `w:rPr` children for `c` where it differs from `base`.
    fn rpr_inner(&self, c: &CFmt, base: &CFmt) -> String {
        let mut s = String::new();
        if let Some(id) = c.style.and_then(|i| self.char_ids.get(&i)) {
            let _ = write!(s, "<w:rStyle w:val=\"{}\"/>", esc(id));
        }
        if c.font != base.font {
            s.push_str(&self.fonts_xml(c.font));
        }
        let toggle = |s: &mut String, tag: &str, on: bool, was: bool| {
            if on != was {
                if on {
                    let _ = write!(s, "<w:{tag}/>");
                } else {
                    let _ = write!(s, "<w:{tag} w:val=\"0\"/>");
                }
            }
        };
        toggle(&mut s, "b", c.bold, base.bold);
        toggle(&mut s, "i", c.italic, base.italic);
        toggle(&mut s, "caps", c.caps, base.caps);
        toggle(&mut s, "smallCaps", c.scaps, base.scaps);
        toggle(&mut s, "strike", c.strike, base.strike);
        toggle(&mut s, "dstrike", c.dstrike, base.dstrike);
        if c.color != base.color && self.rgb(c.color) != self.rgb(base.color) {
            let v = self.rgb(c.color).map_or("auto".to_owned(), hex);
            let _ = write!(s, "<w:color w:val=\"{v}\"/>");
        }
        if c.size != base.size {
            let _ = write!(
                s,
                "<w:sz w:val=\"{0}\"/><w:szCs w:val=\"{0}\"/>",
                c.size.clamp(1, 3276)
            );
        }
        if c.highlight != base.highlight
            && let Some(rgb) = self.rgb(c.highlight)
        {
            let _ = write!(s, "<w:highlight w:val=\"{}\"/>", highlight_name(rgb));
        }
        if c.ul != base.ul {
            let _ = write!(s, "<w:u w:val=\"{}\"/>", c.ul.unwrap_or("none"));
        }
        if c.bg != base.bg
            && let Some(rgb) = self.rgb(c.bg)
        {
            let _ = write!(
                s,
                "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>",
                hex(rgb)
            );
        }
        if c.valign != base.valign {
            let v = match c.valign {
                1 => "superscript",
                -1 => "subscript",
                _ => "baseline",
            };
            let _ = write!(s, "<w:vertAlign w:val=\"{v}\"/>");
        }
        s
    }

    fn rpr(&self, c: &CFmt, base: &CFmt) -> String {
        let inner = self.rpr_inner(c, base);
        if inner.is_empty() {
            inner
        } else {
            format!("<w:rPr>{inner}</w:rPr>")
        }
    }

    /// The paragraph style's formatting (the defaults without one).
    fn para_base(&self, p: &PFmt) -> (Option<&'a Style>, CFmt, PFmt) {
        match self
            .style(StyleKind::Para, p.style)
            .filter(|_| self.para_ids.contains_key(&p.style))
        {
            Some(s) => (Some(s), s.c.clone(), s.p.clone()),
            None => (None, self.d.plain.clone(), self.d.pard.clone()),
        }
    }

    /// What a run with character style `c.style` gets before its own
    /// formatting.
    fn run_base(&self, c: &CFmt, para: &CFmt) -> CFmt {
        let mut base = para.clone();
        base.style = c.style;
        let Some(cs) = c.style.and_then(|i| self.style(StyleKind::Char, i)) else {
            return base;
        };
        let plain = &self.d.plain;
        let cs = &cs.c;
        macro_rules! over {
            ($($f:ident),*) => {$( if cs.$f != plain.$f { base.$f = cs.$f.clone(); } )*};
        }
        over!(bold, italic, ul, strike, dstrike, caps, scaps, size, font, color, highlight, bg, valign);
        base
    }

    fn num_id(&mut self, p: &PFmt) -> Option<(u32, i32)> {
        if let Some(ls) = p.ls {
            return self.nums.get(&ls).map(|&n| (n, p.ilvl));
        }
        let pn = p.pn.as_ref()?;
        let level = pn.level;
        let mut key = pn.clone();
        key.level = 0;
        if let Some((_, id)) = self.pn_nums.iter().find(|(k, _)| *k == key) {
            return Some((*id, level));
        }
        let id = self.nums.len() as u32 + self.pn_nums.len() as u32 + 1;
        self.pn_nums.push((key, id));
        Some((id, level))
    }

    /// `w:pPr` children for `p` where it differs from `base`.
    fn ppr(&mut self, p: &PFmt, base: &PFmt, style: Option<&Style>, sect: Option<&str>) -> String {
        let mut s = String::new();
        if let Some(st) = style
            && let Some(id) = self.para_ids.get(&st.idx)
            && st.idx != 0
        {
            let _ = write!(s, "<w:pStyle w:val=\"{}\"/>", esc(id));
        }
        let flag = |s: &mut String, tag: &str, on: bool, was: bool| {
            if on != was {
                if on {
                    let _ = write!(s, "<w:{tag}/>");
                } else {
                    let _ = write!(s, "<w:{tag} w:val=\"0\"/>");
                }
            }
        };
        flag(&mut s, "keepNext", p.keepn, base.keepn);
        flag(&mut s, "keepLines", p.keep, base.keep);
        flag(&mut s, "pageBreakBefore", p.pagebb, base.pagebb);
        if let Some((num, ilvl)) = self.num_id(p) {
            let _ = write!(
                s,
                "<w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{num}\"/></w:numPr>",
                ilvl.clamp(0, 8)
            );
        }
        if p.tabs != base.tabs && !p.tabs.is_empty() {
            s.push_str("<w:tabs>");
            for t in &p.tabs {
                let _ = write!(s, "<w:tab w:val=\"{}\"", t.kind);
                if let Some(l) = t.leader {
                    let _ = write!(s, " w:leader=\"{l}\"");
                }
                let _ = write!(s, " w:pos=\"{}\"/>", t.pos);
            }
            s.push_str("</w:tabs>");
        }
        let line = line_spacing(p);
        if p.sb != base.sb || p.sa != base.sa || line != line_spacing(base) {
            let _ = write!(
                s,
                "<w:spacing w:before=\"{}\" w:after=\"{}\" w:line=\"{}\" w:lineRule=\"{}\"/>",
                p.sb, p.sa, line.0, line.1
            );
        }
        if (p.li, p.ri, p.fi) != (base.li, base.ri, base.fi) {
            let first = if p.fi < 0 {
                format!("w:hanging=\"{}\"", -p.fi)
            } else {
                format!("w:firstLine=\"{}\"", p.fi)
            };
            let _ = write!(
                s,
                "<w:ind w:left=\"{}\" w:right=\"{}\" {first}/>",
                p.li, p.ri
            );
        }
        flag(&mut s, "contextualSpacing", p.contextual, base.contextual);
        if p.align != base.align {
            let _ = write!(s, "<w:jc w:val=\"{}\"/>", p.align.unwrap_or("left"));
        }
        if p.outline != base.outline
            && let Some(l) = p.outline
        {
            let _ = write!(s, "<w:outlineLvl w:val=\"{l}\"/>");
        }
        if let Some(sect) = sect {
            s.push_str(sect);
        }
        s
    }

    fn para(&mut self, para: &Para, rels: &mut Rels, out: &mut String) {
        let (style, base_c, base_p) = self.para_base(&para.p);
        let sect = para.sect.as_deref().map(|s| self.sect_pr(s, rels));
        let ppr = self.ppr(&para.p, &base_p, style, sect.as_deref());
        out.push_str("<w:p>");
        if !ppr.is_empty() {
            let _ = write!(out, "<w:pPr>{ppr}</w:pPr>");
        }
        self.inlines(&para.inl, &base_c, rels, out);
        out.push_str("</w:p>");
    }

    fn inlines(&mut self, inl: &[Inline], para_c: &CFmt, rels: &mut Rels, out: &mut String) {
        for i in inl {
            match i {
                Inline::Text(t, c) => {
                    let rpr = self.rpr(c, &self.run_base(c, para_c));
                    let _ = write!(
                        out,
                        "<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                        esc(t)
                    );
                }
                Inline::Tab(c) => {
                    let rpr = self.rpr(c, &self.run_base(c, para_c));
                    let _ = write!(out, "<w:r>{rpr}<w:tab/></w:r>");
                }
                Inline::Break(c) => {
                    let rpr = self.rpr(c, &self.run_base(c, para_c));
                    let _ = write!(out, "<w:r>{rpr}<w:br/></w:r>");
                }
                Inline::PageBreak => out.push_str("<w:r><w:br w:type=\"page\"/></w:r>"),
                Inline::Field { instr, shown, c } => {
                    let rpr = self.rpr(c, &self.run_base(c, para_c));
                    let _ = write!(
                        out,
                        "<w:fldSimple w:instr=\" {} \\* MERGEFORMAT \"><w:r>{rpr}<w:t>{}</w:t></w:r></w:fldSimple>",
                        esc(instr),
                        esc(shown)
                    );
                }
                Inline::Link { url, runs } => {
                    if let Some(anchor) = url.strip_prefix('#') {
                        let _ = write!(out, "<w:hyperlink w:anchor=\"{}\">", esc(anchor));
                    } else {
                        let id = rels.add("hyperlink", url.clone(), true);
                        let _ = write!(out, "<w:hyperlink r:id=\"{id}\" w:history=\"1\">");
                    }
                    let mut inner = String::new();
                    // Pictures and fields stay outside the link.
                    let (plain, other): (Vec<_>, Vec<_>) = runs.iter().cloned().partition(|r| {
                        matches!(r, Inline::Text(..) | Inline::Tab(_) | Inline::Break(_))
                    });
                    self.inlines(&plain, para_c, rels, &mut inner);
                    out.push_str(&inner);
                    out.push_str("</w:hyperlink>");
                    self.inlines(&other, para_c, rels, out);
                }
                Inline::Image(pic) => {
                    let n = self.media.len() + 1;
                    let ext = if pic.png { "png" } else { "jpeg" };
                    let name = format!("image{n}.{ext}");
                    self.media.push((name.clone(), pic.data.clone()));
                    let id = rels.add("image", format!("media/{name}"), false);
                    let doc_pr = self.doc_pr;
                    self.doc_pr += 1;
                    let _ = write!(
                        out,
                        "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\" \
                         xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\">\
                         <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:effectExtent l=\"0\" t=\"0\" r=\"0\" b=\"0\"/>\
                         <wp:docPr id=\"{doc_pr}\" name=\"Picture {doc_pr}\"/><wp:cNvGraphicFramePr>\
                         <a:graphicFrameLocks xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
                         noChangeAspect=\"1\"/></wp:cNvGraphicFramePr><a:graphic \
                         xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\"><a:graphicData \
                         uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:pic \
                         xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"><pic:nvPicPr>\
                         <pic:cNvPr id=\"0\" name=\"{name}\"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill>\
                         <a:blip r:embed=\"{id}\"/><a:stretch><a:fillRect/></a:stretch>\
                         </pic:blipFill><pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/>\
                         </a:xfrm><a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
                         </a:graphicData></a:graphic></wp:inline></w:drawing></w:r>",
                        cx = pic.cx,
                        cy = pic.cy,
                    );
                }
            }
        }
    }

    fn blocks(&mut self, blocks: &[Block], rels: &mut Rels, out: &mut String) {
        for b in blocks {
            match b {
                Block::Para(p) => self.para(p, rels, out),
                Block::Table(rows) => self.table(rows, rels, out),
            }
        }
    }

    fn table(&mut self, rows: &[Row], rels: &mut Rels, out: &mut String) {
        // Each row as (left, right, def, content) cells, merges applied.
        struct Cell<'r> {
            left: i32,
            right: i32,
            def: CellDef,
            paras: Vec<&'r Para>,
        }
        let mut laid: Vec<(&RowDef, Vec<Cell>)> = Vec::new();
        for row in rows {
            let mut defs = row.def.cells.clone();
            let n = row.cells.len().max(defs.len()).max(1);
            while defs.len() < n {
                let right = defs.last().map_or(row.def.left, |d| d.right) + 1440;
                defs.push(CellDef {
                    right,
                    ..CellDef::default()
                });
            }
            let mut cells: Vec<Cell> = Vec::new();
            let mut left = row.def.left;
            for (i, def) in defs.into_iter().enumerate() {
                let right = def.right.max(left + 20);
                let paras: Vec<&Para> = row.cells.get(i).map(|c| c.iter().collect()).unwrap_or_default();
                if def.hmerge == 2
                    && let Some(prev) = cells.last_mut()
                {
                    prev.right = right;
                    if paras.iter().any(|p| !p.inl.is_empty()) {
                        prev.paras.extend(paras);
                    }
                } else {
                    cells.push(Cell {
                        left,
                        right,
                        def,
                        paras,
                    });
                }
                left = right;
            }
            laid.push((&row.def, cells));
        }
        // The grid: every cell edge, close ones snapped together.
        let mut edges: Vec<i32> = laid
            .iter()
            .flat_map(|(_, cells)| cells.iter().flat_map(|c| [c.left, c.right]))
            .collect();
        edges.sort_unstable();
        let mut grid: Vec<i32> = Vec::new();
        for e in edges {
            if grid.last().is_none_or(|&g| e - g > 10) {
                grid.push(e);
            }
        }
        let col = |x: i32| -> usize {
            grid.iter()
                .enumerate()
                .min_by_key(|(_, g)| (*g - x).abs())
                .map_or(0, |(i, _)| i)
        };
        let first = &rows[0].def;
        out.push_str("<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/>");
        if let Some(jc) = first.jc {
            let _ = write!(out, "<w:jc w:val=\"{jc}\"/>");
        }
        let table_left = grid.first().copied().unwrap_or(0);
        let _ = write!(
            out,
            "<w:tblInd w:w=\"{}\" w:type=\"dxa\"/>",
            table_left + first.pad[0].unwrap_or(first.gaph)
        );
        let tags = ["top", "left", "bottom", "right", "insideH", "insideV"];
        let borders: String = (0..6)
            .map(|i| border_xml(tags[i], &first.borders[i], &self.d.colors))
            .collect();
        if !borders.is_empty() {
            let _ = write!(out, "<w:tblBorders>{borders}</w:tblBorders>");
        }
        let _ = write!(
            out,
            "<w:tblLayout w:type=\"fixed\"/><w:tblCellMar><w:left w:w=\"{}\" w:type=\"dxa\"/>\
             <w:right w:w=\"{}\" w:type=\"dxa\"/></w:tblCellMar></w:tblPr><w:tblGrid>",
            first.pad[0].unwrap_or(first.gaph),
            first.pad[1].unwrap_or(first.gaph)
        );
        for w in grid.windows(2) {
            let _ = write!(out, "<w:gridCol w:w=\"{}\"/>", w[1] - w[0]);
        }
        out.push_str("</w:tblGrid>");
        for (def, cells) in &laid {
            out.push_str("<w:tr>");
            let before = cells.first().map_or(0, |c| col(c.left));
            let mut trpr = String::new();
            if before > 0 {
                let _ = write!(
                    trpr,
                    "<w:gridBefore w:val=\"{before}\"/><w:wBefore w:w=\"{}\" w:type=\"dxa\"/>",
                    grid[before] - grid[0]
                );
            }
            if def.height != 0 {
                let _ = write!(
                    trpr,
                    "<w:trHeight w:val=\"{}\" w:hRule=\"{}\"/>",
                    def.height.abs(),
                    if def.height < 0 { "exact" } else { "atLeast" }
                );
            }
            if def.header {
                trpr.push_str("<w:tblHeader/>");
            }
            if !trpr.is_empty() {
                let _ = write!(out, "<w:trPr>{trpr}</w:trPr>");
            }
            for c in cells {
                let span = col(c.right).saturating_sub(col(c.left)).max(1);
                let _ = write!(
                    out,
                    "<w:tc><w:tcPr><w:tcW w:w=\"{}\" w:type=\"dxa\"/>",
                    c.right - c.left
                );
                if span > 1 {
                    let _ = write!(out, "<w:gridSpan w:val=\"{span}\"/>");
                }
                match c.def.vmerge {
                    1 => out.push_str("<w:vMerge w:val=\"restart\"/>"),
                    2 => out.push_str("<w:vMerge/>"),
                    _ => {}
                }
                let borders: String = (0..4)
                    .map(|i| border_xml(tags[i], &c.def.borders[i], &self.d.colors))
                    .collect();
                if !borders.is_empty() {
                    let _ = write!(out, "<w:tcBorders>{borders}</w:tcBorders>");
                }
                if let Some(rgb) = self.rgb(c.def.shade) {
                    let _ = write!(
                        out,
                        "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>",
                        hex(rgb)
                    );
                }
                if let Some(v) = c.def.valign {
                    let _ = write!(out, "<w:vAlign w:val=\"{v}\"/>");
                }
                out.push_str("</w:tcPr>");
                if c.paras.is_empty() {
                    out.push_str("<w:p/>");
                }
                for p in &c.paras {
                    let mut p = (*p).clone();
                    p.sect = None;
                    self.para(&p, rels, out);
                }
                out.push_str("</w:tc>");
            }
            out.push_str("</w:tr>");
        }
        out.push_str("</w:tbl>");
    }

    /// A header or footer part; its relationship id in `rels`.
    fn hf_part(&mut self, footer: bool, blocks: &[Block], rels: &mut Rels) -> String {
        self.hf_count += 1;
        let (tag, kind) = if footer {
            ("ftr", "footer")
        } else {
            ("hdr", "header")
        };
        let name = format!("{kind}{}.xml", self.hf_count);
        let mut part_rels = Rels::default();
        let mut body = String::new();
        self.blocks(blocks, &mut part_rels, &mut body);
        if !body.ends_with("</w:p>") {
            body.push_str("<w:p/>");
        }
        let xml = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <w:{tag} xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\">{body}</w:{tag}>"
        );
        self.parts.push((format!("word/{name}"), xml));
        if !part_rels.0.is_empty() {
            self.parts
                .push((format!("word/_rels/{name}.rels"), part_rels.xml()));
        }
        rels.add(kind, name, false)
    }

    fn sect_pr(&mut self, s: &SectFmt, rels: &mut Rels) -> String {
        let mut out = String::from("<w:sectPr>");
        let mut hf: Vec<&(bool, HfType, Vec<Block>)> = s.hf.iter().collect();
        hf.sort_by_key(|(footer, kind, _)| (*footer, *kind));
        for (footer, kind, blocks) in hf {
            let id = self.hf_part(*footer, blocks, rels);
            let t = match kind {
                HfType::Default => "default",
                HfType::Even => "even",
                HfType::First => "first",
            };
            let el = if *footer {
                "footerReference"
            } else {
                "headerReference"
            };
            let _ = write!(out, "<w:{el} w:type=\"{t}\" r:id=\"{id}\"/>");
        }
        if s.brk != "nextPage" {
            let _ = write!(out, "<w:type w:val=\"{}\"/>", s.brk);
        }
        let _ = write!(
            out,
            "<w:pgSz w:w=\"{}\" w:h=\"{}\"{}/>",
            s.pgw,
            s.pgh,
            if s.landscape {
                " w:orient=\"landscape\""
            } else {
                ""
            }
        );
        let [l, r, t, b] = s.margins;
        let _ = write!(
            out,
            "<w:pgMar w:top=\"{t}\" w:right=\"{r}\" w:bottom=\"{b}\" w:left=\"{l}\" \
             w:header=\"{}\" w:footer=\"{}\" w:gutter=\"{}\"/>",
            s.header_y, s.footer_y, s.gutter
        );
        if let Some(n) = s.pgn_start {
            let _ = write!(out, "<w:pgNumType w:start=\"{}\"/>", n.max(0));
        }
        if s.titlepg {
            out.push_str("<w:titlePg/>");
        }
        out.push_str("</w:sectPr>");
        out
    }

    fn styles_xml(&mut self) -> String {
        let d = self.d;
        let mut s = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <w:styles xmlns:w=\"{W_NS}\"><w:docDefaults><w:rPrDefault><w:rPr>{}\
             <w:sz w:val=\"{1}\"/><w:szCs w:val=\"{1}\"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr>",
            self.fonts_xml(d.plain.font),
            d.plain.size.clamp(1, 3276)
        );
        let (line, rule) = line_spacing(&d.pard);
        let _ = write!(
            s,
            "<w:spacing w:before=\"{}\" w:after=\"{}\" w:line=\"{line}\" w:lineRule=\"{rule}\"/>",
            d.pard.sb, d.pard.sa
        );
        s.push_str("</w:pPr></w:pPrDefault></w:docDefaults>");
        if !self.para_ids.values().any(|id| id == "Normal") {
            s.push_str(
                "<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\">\
                 <w:name w:val=\"Normal\"/><w:qFormat/></w:style>",
            );
        }
        let mut no_style = d.pard.clone();
        no_style.style = 0;
        for st in &d.styles {
            let (kind, id) = match st.kind {
                StyleKind::Para => ("paragraph", self.para_ids.get(&st.idx)),
                StyleKind::Char => ("character", self.char_ids.get(&st.idx)),
                StyleKind::Other => continue,
            };
            let Some(id) = id.cloned() else { continue };
            let default = if id == "Normal" {
                " w:default=\"1\""
            } else {
                ""
            };
            let _ = write!(
                s,
                "<w:style w:type=\"{kind}\"{default} w:styleId=\"{}\"><w:name w:val=\"{}\"/>",
                esc(&id),
                esc(&st.name)
            );
            if st.kind == StyleKind::Para {
                if let Some(next) = st.next.and_then(|n| self.para_ids.get(&n)) {
                    let _ = write!(s, "<w:next w:val=\"{}\"/>", esc(next));
                }
                s.push_str("<w:qFormat/>");
                let mut p = st.p.clone();
                if p.outline.is_none() {
                    p.outline = heading_level(&st.name).map(|n| n - 1);
                }
                let mut base = no_style.clone();
                base.style = st.idx;
                // A style's own pPr: no style reference, no list.
                p.ls = None;
                p.pn = None;
                let ppr = self.ppr(&p, &base, None, None);
                if !ppr.is_empty() {
                    let _ = write!(s, "<w:pPr>{ppr}</w:pPr>");
                }
            }
            let rpr = self.rpr(&st.c, &d.plain);
            s.push_str(&rpr);
            s.push_str("</w:style>");
        }
        s.push_str("</w:styles>");
        s
    }

    fn level(&self, l: &Level, ilvl: usize) -> LevelOut {
        let fmt = nfc_format(l.nfc);
        let font = self.font_name(l.font);
        let mut chars = l.text.chars();
        let len = chars.next().map_or(0, |c| c as usize);
        let mut text = String::new();
        let mut font_out = font.map(str::to_owned);
        for ch in chars.take(len) {
            if (ch as u32) < 9 {
                let _ = write!(text, "%{}", ch as u32 + 1);
            } else if fmt == "bullet"
                && let Some(b) = plain_bullet(ch, font)
            {
                text.push(b);
                font_out = None;
            } else {
                text.push(ch);
            }
        }
        if text.is_empty() && fmt != "none" {
            text = if fmt == "bullet" {
                font_out = None;
                "\u{2022}".into()
            } else {
                format!("%{}.", ilvl + 1)
            };
        }
        if fmt != "bullet" {
            font_out = None;
        }
        LevelOut {
            start: l.start,
            fmt,
            suff: match l.follow {
                1 => Some("space"),
                2 => Some("nothing"),
                _ => None,
            },
            text,
            jc: match l.jc {
                1 => "center",
                2 => "right",
                _ => "left",
            },
            li: l.li,
            fi: l.fi,
            font: font_out,
        }
    }

    fn pn_levels(&self, pn: &Pn) -> Vec<LevelOut> {
        (0..9)
            .map(|i| {
                let li = 720 * (i as i32 + 1);
                if i as i32 != pn.level {
                    return LevelOut {
                        start: 1,
                        fmt: "decimal",
                        suff: None,
                        text: format!("%{}.", i + 1),
                        jc: "left",
                        li,
                        fi: -360,
                        font: None,
                    };
                }
                let font = self.font_name(pn.font);
                let (text, font) = if pn.nfc == "bullet" {
                    let ch = pn.before.chars().next().unwrap_or('\u{2022}');
                    match plain_bullet(ch, font) {
                        Some(b) => (b.to_string(), None),
                        None => (ch.to_string(), font.map(str::to_owned)),
                    }
                } else {
                    (format!("{}%{}{}", pn.before, i + 1, pn.after), None)
                };
                LevelOut {
                    start: pn.start.max(0),
                    fmt: pn.nfc,
                    suff: None,
                    text,
                    jc: "left",
                    li,
                    fi: -360,
                    font,
                }
            })
            .collect()
    }

    fn numbering_xml(&self) -> Option<String> {
        if self.nums.is_empty() && self.pn_nums.is_empty() {
            return None;
        }
        let mut s = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:numbering xmlns:w=\"{W_NS}\">"
        );
        let mut abstracts: BTreeMap<i32, usize> = BTreeMap::new();
        let mut defs: Vec<Vec<LevelOut>> = Vec::new();
        for (_, list) in self.nums.keys().map(|ls| (ls, self.d.overrides[ls])) {
            if abstracts.contains_key(&list) {
                continue;
            }
            let levels = &self.d.lists[&list];
            abstracts.insert(list, defs.len());
            defs.push(
                levels
                    .iter()
                    .enumerate()
                    .map(|(i, l)| self.level(l, i))
                    .collect(),
            );
        }
        let pn_start = defs.len();
        for (pn, _) in &self.pn_nums {
            defs.push(self.pn_levels(pn));
        }
        for (a, levels) in defs.iter().enumerate() {
            let _ = write!(
                s,
                "<w:abstractNum w:abstractNumId=\"{a}\"><w:multiLevelType w:val=\"hybridMultilevel\"/>"
            );
            for (i, l) in levels.iter().enumerate() {
                let _ = write!(
                    s,
                    "<w:lvl w:ilvl=\"{i}\"><w:start w:val=\"{}\"/><w:numFmt w:val=\"{}\"/>",
                    l.start.max(0),
                    l.fmt
                );
                if let Some(suff) = l.suff {
                    let _ = write!(s, "<w:suff w:val=\"{suff}\"/>");
                }
                let first = if l.fi < 0 {
                    format!("w:hanging=\"{}\"", -l.fi)
                } else {
                    format!("w:firstLine=\"{}\"", l.fi)
                };
                let _ = write!(
                    s,
                    "<w:lvlText w:val=\"{}\"/><w:lvlJc w:val=\"{}\"/><w:pPr><w:ind w:left=\"{}\" {first}/></w:pPr>",
                    esc(&l.text),
                    l.jc,
                    l.li
                );
                if let Some(f) = &l.font {
                    let f = esc(f);
                    let _ = write!(
                        s,
                        "<w:rPr><w:rFonts w:ascii=\"{f}\" w:hAnsi=\"{f}\" w:hint=\"default\"/></w:rPr>"
                    );
                }
                s.push_str("</w:lvl>");
            }
            s.push_str("</w:abstractNum>");
        }
        for (ls, num) in &self.nums {
            let a = abstracts[&self.d.overrides[ls]];
            let _ = write!(
                s,
                "<w:num w:numId=\"{num}\"><w:abstractNumId w:val=\"{a}\"/></w:num>"
            );
        }
        for (k, (_, num)) in self.pn_nums.iter().enumerate() {
            let _ = write!(
                s,
                "<w:num w:numId=\"{num}\"><w:abstractNumId w:val=\"{}\"/></w:num>",
                pn_start + k
            );
        }
        s.push_str("</w:numbering>");
        Some(s)
    }
}

/// The `.docx` bytes for a parsed RTF document.
pub fn package(d: &Parsed) -> anyhow::Result<Vec<u8>> {
    let mut w = Writer::new(d);
    let mut rels = Rels::default();
    rels.add("styles", "styles.xml".into(), false);
    rels.add("settings", "settings.xml".into(), false);
    let mut body = String::new();
    w.blocks(&d.blocks, &mut rels, &mut body);
    // Word ends every body with a paragraph.
    if matches!(d.blocks.last(), Some(Block::Table(_))) {
        body.push_str("<w:p/>");
    }
    let sect = w.sect_pr(&d.sect, &mut rels);
    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document xmlns:w=\"{W_NS}\" xmlns:r=\"{R_NS}\"><w:body>{body}{sect}</w:body></w:document>"
    );
    let styles = w.styles_xml();
    let numbering = w.numbering_xml();
    if numbering.is_some() {
        rels.add("numbering", "numbering.xml".into(), false);
    }
    let settings = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:settings xmlns:w=\"{W_NS}\">\
         <w:defaultTabStop w:val=\"{}\"/>{}<w:compat><w:compatSetting w:name=\"compatibilityMode\" \
         w:uri=\"http://schemas.microsoft.com/office/word\" w:val=\"15\"/></w:compat></w:settings>",
        d.deftab,
        if d.facing {
            "<w:evenAndOddHeaders/>"
        } else {
            ""
        }
    );

    let main = "application/vnd.openxmlformats-officedocument.wordprocessingml";
    let mut types = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Default Extension=\"png\" ContentType=\"image/png\"/>\
         <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>",
    );
    let _ = write!(
        types,
        "<Override PartName=\"/word/document.xml\" ContentType=\"{main}.document.main+xml\"/>\
         <Override PartName=\"/word/styles.xml\" ContentType=\"{main}.styles+xml\"/>\
         <Override PartName=\"/word/settings.xml\" ContentType=\"{main}.settings+xml\"/>"
    );
    if numbering.is_some() {
        let _ = write!(
            types,
            "<Override PartName=\"/word/numbering.xml\" ContentType=\"{main}.numbering+xml\"/>"
        );
    }
    for (name, _) in &w.parts {
        let kind = if name.starts_with("word/header") {
            "header"
        } else if name.starts_with("word/footer") {
            "footer"
        } else {
            continue;
        };
        let _ = write!(
            types,
            "<Override PartName=\"/{name}\" ContentType=\"{main}.{kind}+xml\"/>"
        );
    }
    types.push_str("</Types>");

    let mut parts: Vec<(String, Vec<u8>)> = vec![
        ("[Content_Types].xml".into(), types.into_bytes()),
        (
            "_rels/.rels".into(),
            format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
                 <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
                 <Relationship Id=\"rId1\" Type=\"{REL}/officeDocument\" Target=\"word/document.xml\"/>\
                 </Relationships>"
            )
            .into_bytes(),
        ),
        ("word/document.xml".into(), document.into_bytes()),
        ("word/_rels/document.xml.rels".into(), rels.xml().into_bytes()),
        ("word/styles.xml".into(), styles.into_bytes()),
        ("word/settings.xml".into(), settings.into_bytes()),
    ];
    if let Some(n) = numbering {
        parts.push(("word/numbering.xml".into(), n.into_bytes()));
    }
    for (name, xml) in std::mem::take(&mut w.parts) {
        parts.push((name, xml.into_bytes()));
    }
    for (name, data) in std::mem::take(&mut w.media) {
        parts.push((format!("word/media/{name}"), data));
    }

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in parts {
        zip.start_file(name, opts)?;
        zip.write_all(&data)?;
    }
    Ok(zip.finish()?.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[std::prelude::v1::test]
    fn ids_bullets_and_highlights() {
        assert_eq!(style_id("heading 1"), "Heading1");
        assert_eq!(style_id("my body-text"), "MyBodyText");
        assert_eq!(heading_level("Heading 3"), Some(3));
        assert_eq!(heading_level("heading x"), None);
        assert_eq!(plain_bullet('\u{F0B7}', None), Some('\u{2022}'));
        assert_eq!(plain_bullet('\u{B7}', Some("Symbol")), Some('\u{2022}'));
        assert_eq!(plain_bullet('o', Some("Courier New")), None);
        assert_eq!(highlight_name([255, 255, 0]), "yellow");
        assert_eq!(highlight_name([0, 250, 10]), "green");
        assert_eq!(esc("a<\u{1}&\"b"), "a&lt;&amp;&quot;b");
    }
}
