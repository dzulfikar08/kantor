//! Writing an imported `.doc` ([`Doc`]) as a WordprocessingML package.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::{Cursor, Write as _};

use anyhow::Result;

use super::sprm::{Brd, Chp, Color, Pap, Tab, Tap};
use super::{Block, Doc, Item, NoteKind, Para, Row};
use crate::{LINE_BREAK, PAGE_BREAK};

const NS: &str = "xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
    xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
    xmlns:wp=\"http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing\" \
    xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
    xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\"";
const REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const CT: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml";

/// Text for XML: escaped, without characters XML can't hold.
fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\t' | '\n' | '\r' => o.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => o.push(c),
        }
    }
    o
}

/// Reads one toggle property of a [`Chp`].
type Toggle = fn(&Chp) -> Option<u8>;

struct Rel {
    id: String,
    kind: &'static str,
    target: String,
    external: bool,
}

struct W<'a> {
    doc: &'a Doc,
    style_ids: Vec<Option<String>>,
    rels: Vec<Rel>,
    /// Media index → relationship id.
    media_rels: Vec<Option<String>>,
    pictures: u32,
    /// Parts besides document.xml: name, content type, XML.
    parts: Vec<(String, String, String)>,
}

/// Word's style id for a name: words capitalised, joined, letters and
/// digits only ("heading 1" → "Heading1").
fn style_id(name: &str) -> String {
    let mut id = String::new();
    for w in name.split_whitespace() {
        let mut cs = w.chars();
        if let Some(f) = cs.next() {
            id.extend(f.to_uppercase());
            id.extend(cs);
        }
    }
    id.retain(|c| c.is_alphanumeric());
    id
}

pub fn package(doc: &Doc) -> Result<Vec<u8>> {
    let mut seen = HashSet::new();
    let style_ids = doc
        .styles
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let s = s.as_ref()?;
            let mut id = match (i, s.sti) {
                (0, _) | (_, 0) => "Normal".to_owned(),
                (_, n @ 1..=9) => format!("Heading{n}"),
                _ => style_id(&s.name),
            };
            if id.is_empty() {
                id = format!("Style{i}");
            }
            let base = id.clone();
            let mut k = 1;
            while !seen.insert(id.clone()) {
                k += 1;
                id = format!("{base}{k}");
            }
            Some(id)
        })
        .collect();
    let mut w = W {
        doc,
        style_ids,
        rels: Vec::new(),
        media_rels: vec![None; doc.media.len()],
        pictures: 0,
        parts: Vec::new(),
    };
    w.rel("styles", "styles.xml", false);
    w.rel("settings", "settings.xml", false);
    let styles = w.styles_xml();
    w.parts
        .push(("word/styles.xml".into(), format!("{CT}.styles+xml"), styles));
    let settings = format!(
        "<w:settings {NS}><w:defaultTabStop w:val=\"{}\"/>{}</w:settings>",
        doc.default_tab,
        if doc.facing_pages {
            "<w:evenAndOddHeaders/>"
        } else {
            ""
        }
    );
    w.parts.push((
        "word/settings.xml".into(),
        format!("{CT}.settings+xml"),
        settings,
    ));
    if let Some(n) = w.numbering_xml() {
        w.rel("numbering", "numbering.xml", false);
        w.parts.push((
            "word/numbering.xml".into(),
            format!("{CT}.numbering+xml"),
            n,
        ));
    }
    for (kind, notes) in [
        (NoteKind::Foot, &doc.footnotes),
        (NoteKind::End, &doc.endnotes),
    ] {
        if notes.is_empty() {
            continue;
        }
        let (tag, file) = match kind {
            NoteKind::Foot => ("footnote", "footnotes"),
            NoteKind::End => ("endnote", "endnotes"),
        };
        let mut x = format!(
            "<w:{file} {NS}><w:{tag} w:type=\"separator\" w:id=\"-1\"><w:p><w:r><w:separator/></w:r></w:p></w:{tag}>\
             <w:{tag} w:type=\"continuationSeparator\" w:id=\"0\"><w:p><w:r><w:continuationSeparator/></w:r></w:p></w:{tag}>"
        );
        for (i, blocks) in notes.iter().enumerate() {
            let _ = write!(x, "<w:{tag} w:id=\"{}\">", i + 1);
            x.push_str(&w.blocks(blocks));
            let _ = write!(x, "</w:{tag}>");
        }
        let _ = write!(x, "</w:{file}>");
        w.rel(file, &format!("{file}.xml"), false);
        w.parts
            .push((format!("word/{file}.xml"), format!("{CT}.{file}+xml"), x));
    }
    // Headers and footers, then each section's properties.
    let mut sect_prs = Vec::new();
    let mut hf_count = 0;
    for (s, sec) in doc.sections.iter().enumerate() {
        let mut refs = String::new();
        for (t, story) in sec.hf.iter().enumerate() {
            let Some(blocks) = story else { continue };
            let footer = t == 2 || t == 3 || t == 5;
            let kind = match t {
                0 | 2 if doc.facing_pages => "even",
                1 | 3 => "default",
                4 | 5 if sec.sep.title_page => "first",
                _ => continue,
            };
            hf_count += 1;
            let (tag, part) = if footer {
                ("ftr", format!("footer{hf_count}.xml"))
            } else {
                ("hdr", format!("header{hf_count}.xml"))
            };
            let x = format!("<w:{tag} {NS}>{}</w:{tag}>", w.blocks(blocks));
            let id = w.rel(if footer { "footer" } else { "header" }, &part, false);
            w.parts.push((
                format!("word/{part}"),
                format!("{CT}.{}+xml", if footer { "footer" } else { "header" }),
                x,
            ));
            let _ = write!(
                refs,
                "<w:{} w:type=\"{kind}\" r:id=\"{id}\"/>",
                if footer {
                    "footerReference"
                } else {
                    "headerReference"
                }
            );
        }
        let _ = s;
        sect_prs.push(sect_pr(&sec.sep, &refs));
    }
    let mut body = String::new();
    body.push_str(&w.blocks_with_sections(&doc.body, &sect_prs));
    let last = sect_prs.last().cloned().unwrap_or_default();
    let document = format!("<w:document {NS}><w:body>{body}{last}</w:body></w:document>");
    zip(&w, &document)
}

fn sect_pr(sep: &super::sprm::Sep, refs: &str) -> String {
    let kind = match sep.bkc {
        0 => "continuous",
        1 => "nextColumn",
        3 => "evenPage",
        4 => "oddPage",
        _ => "nextPage",
    };
    let mut x = format!("<w:sectPr>{refs}<w:type w:val=\"{kind}\"/>");
    let _ = write!(
        x,
        "<w:pgSz w:w=\"{}\" w:h=\"{}\"{}/>",
        sep.width,
        sep.height,
        if sep.landscape {
            " w:orient=\"landscape\""
        } else {
            ""
        }
    );
    let _ = write!(
        x,
        "<w:pgMar w:top=\"{}\" w:right=\"{}\" w:bottom=\"{}\" w:left=\"{}\" w:header=\"{}\" w:footer=\"{}\" w:gutter=\"{}\"/>",
        sep.top, sep.right, sep.bottom, sep.left, sep.header, sep.footer, sep.gutter
    );
    if let Some(n) = sep.pgn_start.filter(|_| sep.pgn_restart) {
        let _ = write!(x, "<w:pgNumType w:start=\"{n}\"/>");
    }
    if sep.columns > 1 {
        let _ = write!(
            x,
            "<w:cols w:num=\"{}\" w:space=\"{}\"/>",
            sep.columns.min(45),
            sep.column_space
        );
    }
    if sep.title_page {
        x.push_str("<w:titlePg/>");
    }
    x.push_str("</w:sectPr>");
    x
}

impl W<'_> {
    fn rel(&mut self, kind: &'static str, target: &str, external: bool) -> String {
        let id = format!("rId{}", self.rels.len() + 1);
        self.rels.push(Rel {
            id: id.clone(),
            kind,
            target: target.to_owned(),
            external,
        });
        id
    }

    fn font(&self, i: Option<u16>) -> Option<&str> {
        self.doc
            .fonts
            .get(i? as usize)
            .map(String::as_str)
            .filter(|f| !f.is_empty())
    }

    fn fonts_xml(&self, f: [Option<u16>; 4]) -> String {
        let mut a = String::new();
        let ascii = self.font(f[0]);
        let pairs = [
            ("ascii", ascii),
            ("hAnsi", self.font(f[2]).or(ascii)),
            ("eastAsia", self.font(f[1])),
            ("cs", self.font(f[3])),
        ];
        for (k, v) in pairs {
            if let Some(v) = v {
                let _ = write!(a, " w:{k}=\"{}\"", esc(v));
            }
        }
        if a.is_empty() {
            String::new()
        } else {
            format!("<w:rFonts{a}/>")
        }
    }

    /// A toggle's value in the paragraph style chain (for 0x80/0x81).
    fn style_toggle(&self, istd: u16, get: Toggle) -> bool {
        let mut cur = Some(istd);
        let mut n = 0;
        while let Some(i) = cur {
            n += 1;
            if n > 20 {
                break;
            }
            let Some(Some(s)) = self.doc.styles.get(i as usize) else {
                break;
            };
            let mut c = Chp::default();
            c.apply(&s.chp);
            if let Some(v) = get(&c) {
                return v == 1;
            }
            cur = s.base;
        }
        false
    }

    fn rpr(&self, c: &Chp, para_istd: u16) -> String {
        let mut x = String::new();
        if let Some(id) = c.istd.filter(|&i| i != 10).and_then(|i| self.sid(i)) {
            let _ = write!(x, "<w:rStyle w:val=\"{}\"/>", esc(id));
        }
        x.push_str(&self.fonts_xml(c.fonts));
        let toggles: [(&str, Toggle); 7] = [
            ("b", |c| c.bold),
            ("i", |c| c.italic),
            ("caps", |c| c.caps),
            ("smallCaps", |c| c.small_caps),
            ("strike", |c| c.strike),
            ("dstrike", |c| c.dstrike),
            ("vanish", |c| c.vanish),
        ];
        for (tag, get) in toggles {
            let on = match get(c) {
                Some(0) => false,
                Some(1) => true,
                Some(0x81) => !self.style_toggle(para_istd, get),
                _ => continue,
            };
            if tag == "b" || tag == "i" {
                let _ = write!(
                    x,
                    "<w:{tag}{}/><w:{tag}Cs{}/>",
                    if on { "" } else { " w:val=\"0\"" },
                    if on { "" } else { " w:val=\"0\"" }
                );
            } else {
                let _ = write!(x, "<w:{tag}{}/>", if on { "" } else { " w:val=\"0\"" });
            }
        }
        if let Some(col) = c.color {
            let _ = write!(x, "<w:color w:val=\"{}\"/>", hex(col));
        }
        if let Some(sz) = c.size.filter(|s| (2..=3276).contains(s)) {
            let _ = write!(x, "<w:sz w:val=\"{sz}\"/><w:szCs w:val=\"{sz}\"/>");
        }
        if let Some(h) = c.highlight {
            const NAMES: [&str; 16] = [
                "black",
                "blue",
                "cyan",
                "green",
                "magenta",
                "red",
                "yellow",
                "white",
                "darkBlue",
                "darkCyan",
                "darkGreen",
                "darkMagenta",
                "darkRed",
                "darkYellow",
                "darkGray",
                "lightGray",
            ];
            let name = if h == 0 {
                "none"
            } else {
                NAMES.get(h as usize - 1).copied().unwrap_or("yellow")
            };
            let _ = write!(x, "<w:highlight w:val=\"{name}\"/>");
        }
        if let Some(u) = c.underline {
            let v = match u {
                0 => "none",
                2 => "words",
                3 => "double",
                4 => "dotted",
                6 => "thick",
                7 => "dash",
                9 => "dotDash",
                10 => "dotDotDash",
                11 => "wave",
                _ => "single",
            };
            let _ = write!(x, "<w:u w:val=\"{v}\"/>");
        }
        if let Some(iss) = c.iss {
            let v = match iss {
                1 => "superscript",
                2 => "subscript",
                _ => "baseline",
            };
            let _ = write!(x, "<w:vertAlign w:val=\"{v}\"/>");
        }
        if let Some(tag) = c.lang.and_then(lang) {
            let _ = write!(x, "<w:lang w:val=\"{tag}\"/>");
        }
        x
    }

    fn sid(&self, istd: u16) -> Option<&str> {
        self.style_ids.get(istd as usize)?.as_deref()
    }

    fn num_valid(&self, ilfo: u16) -> bool {
        (1..=self.doc.lfos.len()).contains(&(ilfo as usize))
            && self
                .doc
                .lists
                .iter()
                .any(|l| l.lsid == self.doc.lfos[ilfo as usize - 1].lsid)
    }

    fn ppr_inner(&self, p: &Pap, with_style: bool) -> String {
        let mut x = String::new();
        if with_style
            && p.istd != 0
            && let Some(id) = self.sid(p.istd)
        {
            let _ = write!(x, "<w:pStyle w:val=\"{}\"/>", esc(id));
        }
        let flag = |x: &mut String, tag: &str, v: Option<bool>| {
            if let Some(v) = v {
                let _ = write!(x, "<w:{tag}{}/>", if v { "" } else { " w:val=\"0\"" });
            }
        };
        flag(&mut x, "keepNext", p.keep_next);
        flag(&mut x, "keepLines", p.keep);
        flag(&mut x, "pageBreakBefore", p.page_break_before);
        flag(&mut x, "widowControl", p.widow);
        if let Some(ilfo) = p.ilfo
            && self.num_valid(ilfo)
        {
            let _ = write!(
                x,
                "<w:numPr><w:ilvl w:val=\"{}\"/><w:numId w:val=\"{ilfo}\"/></w:numPr>",
                p.ilvl.unwrap_or(0).min(8)
            );
        }
        x.push_str(&tabs_xml(&p.tabs));
        let mut sp = String::new();
        if let Some(b) = p.before {
            let _ = write!(sp, " w:before=\"{b}\"");
        }
        if let Some(a) = p.after {
            let _ = write!(sp, " w:after=\"{a}\"");
        }
        if let Some((line, mult)) = p.line {
            if mult != 0 {
                let _ = write!(sp, " w:line=\"{}\" w:lineRule=\"auto\"", line.max(1));
            } else if line < 0 {
                let _ = write!(sp, " w:line=\"{}\" w:lineRule=\"exact\"", -(line as i32));
            } else if line > 0 {
                let _ = write!(sp, " w:line=\"{line}\" w:lineRule=\"atLeast\"");
            }
        }
        if !sp.is_empty() {
            let _ = write!(x, "<w:spacing{sp}/>");
        }
        let mut ind = String::new();
        if let Some(l) = p.left {
            let _ = write!(ind, " w:left=\"{l}\"");
        }
        if let Some(r) = p.right {
            let _ = write!(ind, " w:right=\"{r}\"");
        }
        match p.first {
            Some(f) if f < 0 => {
                let _ = write!(ind, " w:hanging=\"{}\"", -f);
            }
            Some(f) => {
                let _ = write!(ind, " w:firstLine=\"{f}\"");
            }
            None => {}
        }
        if !ind.is_empty() {
            let _ = write!(x, "<w:ind{ind}/>");
        }
        flag(&mut x, "contextualSpacing", p.contextual);
        if let Some(jc) = p.jc {
            let v = match jc {
                1 => "center",
                2 => "right",
                3 | 4 => "both",
                _ => "left",
            };
            let _ = write!(x, "<w:jc w:val=\"{v}\"/>");
        }
        if let Some(o) = p.outline.filter(|&o| o < 9) {
            let _ = write!(x, "<w:outlineLvl w:val=\"{o}\"/>");
        }
        x
    }

    fn blocks(&mut self, blocks: &[Block]) -> String {
        self.blocks_with_sections(blocks, &[])
    }

    fn blocks_with_sections(&mut self, blocks: &[Block], sects: &[String]) -> String {
        let mut x = String::new();
        for b in blocks {
            match b {
                Block::Para(p) => x.push_str(&self.para(p, sects)),
                Block::Table(rows) => x.push_str(&self.table(rows)),
            }
        }
        // A story must end with a paragraph (also after a table).
        if !matches!(blocks.last(), Some(Block::Para(_))) {
            x.push_str("<w:p/>");
        }
        x
    }

    fn para(&mut self, p: &Para, sects: &[String]) -> String {
        let mut x = String::from("<w:p><w:pPr>");
        x.push_str(&self.ppr_inner(&p.pap, true));
        let mark = self.rpr(&p.mark, p.pap.istd);
        if !mark.is_empty() {
            let _ = write!(x, "<w:rPr>{mark}</w:rPr>");
        }
        if let Some(s) = p.section_end.and_then(|s| sects.get(s)) {
            x.push_str(s);
        }
        x.push_str("</w:pPr>");
        // Links and fields only wrap runs when they close in this paragraph.
        let closes = |from: usize, end: fn(&Item) -> bool| p.items[from..].iter().any(end);
        let mut open: Option<u8> = None;
        for (i, item) in p.items.iter().enumerate() {
            match item {
                Item::Text(c, t) => x.push_str(&self.text_runs(c, t, p.pap.istd)),
                Item::Picture(c, m, wd, ht) => {
                    let r = self.picture(c, *m, *wd, *ht, p.pap.istd);
                    x.push_str(&r);
                }
                Item::NoteRef(c, kind, n) => {
                    let tag = match kind {
                        NoteKind::Foot => "footnoteReference",
                        NoteKind::End => "endnoteReference",
                    };
                    let _ = write!(
                        x,
                        "<w:r><w:rPr>{}</w:rPr><w:{tag} w:id=\"{n}\"/></w:r>",
                        self.rpr(c, p.pap.istd)
                    );
                }
                Item::NoteMark(c, kind) => {
                    let tag = match kind {
                        NoteKind::Foot => "footnoteRef",
                        NoteKind::End => "endnoteRef",
                    };
                    let _ = write!(
                        x,
                        "<w:r><w:rPr>{}</w:rPr><w:{tag}/></w:r>",
                        self.rpr(c, p.pap.istd)
                    );
                }
                Item::LinkStart(url) => {
                    if open.is_none() && closes(i, |it| matches!(it, Item::LinkEnd)) {
                        let id = self.rel("hyperlink", url, true);
                        let _ = write!(x, "<w:hyperlink r:id=\"{id}\" w:history=\"1\">");
                        open = Some(1);
                    }
                }
                Item::FieldStart(instr) => {
                    if open.is_none() && closes(i, |it| matches!(it, Item::FieldEnd)) {
                        let _ = write!(x, "<w:fldSimple w:instr=\" {} \">", esc(instr));
                        open = Some(2);
                    }
                }
                Item::LinkEnd if open == Some(1) => {
                    x.push_str("</w:hyperlink>");
                    open = None;
                }
                Item::FieldEnd if open == Some(2) => {
                    x.push_str("</w:fldSimple>");
                    open = None;
                }
                Item::LinkEnd | Item::FieldEnd => {}
            }
        }
        x.push_str("</w:p>");
        x
    }

    fn text_runs(&self, c: &Chp, t: &str, istd: u16) -> String {
        let rpr = self.rpr(c, istd);
        let mut x = format!("<w:r>{}", wrap("w:rPr", &rpr));
        let mut buf = String::new();
        let flush = |x: &mut String, buf: &mut String| {
            if !buf.is_empty() {
                let _ = write!(x, "<w:t xml:space=\"preserve\">{}</w:t>", esc(buf));
                buf.clear();
            }
        };
        for ch in t.chars() {
            let el = match ch {
                '\t' => "<w:tab/>",
                LINE_BREAK => "<w:br/>",
                PAGE_BREAK => "<w:br w:type=\"page\"/>",
                '\u{2011}' => "<w:noBreakHyphen/>",
                '\u{AD}' => "<w:softHyphen/>",
                _ => {
                    buf.push(ch);
                    continue;
                }
            };
            flush(&mut x, &mut buf);
            x.push_str(el);
        }
        flush(&mut x, &mut buf);
        x.push_str("</w:r>");
        x
    }

    fn picture(&mut self, c: &Chp, m: usize, wd: u32, ht: u32, istd: u16) -> String {
        let Some((ext, _)) = self.doc.media.get(m) else {
            return String::new();
        };
        let id = match &self.media_rels[m] {
            Some(id) => id.clone(),
            None => {
                let id = self.rel("image", &format!("media/image{}.{ext}", m + 1), false);
                self.media_rels[m] = Some(id.clone());
                id
            }
        };
        self.pictures += 1;
        let n = self.pictures;
        let (cx, cy) = (wd as u64 * 635, ht as u64 * 635);
        format!(
            "<w:r>{}<w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
             <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{n}\" name=\"Picture {n}\"/>\
             <a:graphic><a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
             <pic:pic><pic:nvPicPr><pic:cNvPr id=\"{n}\" name=\"image{n}.{ext}\"/><pic:cNvPicPr/></pic:nvPicPr>\
             <pic:blipFill><a:blip r:embed=\"{id}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
             <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
             <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>\
             </a:graphicData></a:graphic></wp:inline></w:drawing></w:r>",
            wrap("w:rPr", &self.rpr(c, istd))
        )
    }

    fn table(&mut self, rows: &[Row]) -> String {
        // One grid from every row's cell boundaries.
        let mut xs: Vec<i32> = rows
            .iter()
            .flat_map(|r| r.tap.centers.iter().map(|&v| v as i32))
            .collect();
        xs.sort_unstable();
        xs.dedup();
        if xs.len() < 2 {
            // No row definitions: one column per cell, even widths.
            let n = rows.iter().map(|r| r.cells.len()).max().unwrap_or(1).max(1);
            xs = (0..=n as i32).map(|k| k * (9000 / n as i32)).collect();
        }
        let first = rows.first().map(|r| &r.tap);
        let mut x = String::from("<w:tbl><w:tblPr><w:tblW w:w=\"0\" w:type=\"auto\"/>");
        if let Some(jc) = first.and_then(|t| t.jc) {
            let v = match jc {
                1 => "center",
                2 => "right",
                _ => "left",
            };
            let _ = write!(x, "<w:jc w:val=\"{v}\"/>");
        }
        let _ = write!(x, "<w:tblInd w:w=\"{}\" w:type=\"dxa\"/>", xs[0]);
        if let Some(t) = first {
            let b = borders_xml(
                &t.borders,
                &["top", "left", "bottom", "right", "insideH", "insideV"],
            );
            x.push_str(&wrap("w:tblBorders", &b));
        }
        x.push_str("<w:tblLayout w:type=\"fixed\"/>");
        let gap = first.and_then(|t| t.gap_half).unwrap_or(108).max(0);
        let pad = first.map(|t| t.padding).unwrap_or_default();
        let _ = write!(
            x,
            "<w:tblCellMar><w:top w:w=\"{}\" w:type=\"dxa\"/><w:left w:w=\"{}\" w:type=\"dxa\"/>\
             <w:bottom w:w=\"{}\" w:type=\"dxa\"/><w:right w:w=\"{}\" w:type=\"dxa\"/></w:tblCellMar>",
            pad[0].unwrap_or(0),
            pad[1].unwrap_or(gap as u16),
            pad[2].unwrap_or(0),
            pad[3].unwrap_or(gap as u16)
        );
        x.push_str("</w:tblPr><w:tblGrid>");
        for w in xs.windows(2) {
            let _ = write!(x, "<w:gridCol w:w=\"{}\"/>", w[1] - w[0]);
        }
        x.push_str("</w:tblGrid>");
        let col = |v: i32| xs.partition_point(|&p| p < v);
        for row in rows {
            x.push_str(&self.row(row, &xs, &col));
        }
        x.push_str("</w:tbl>");
        x
    }

    fn row(&mut self, row: &Row, xs: &[i32], col: &dyn Fn(i32) -> usize) -> String {
        let t: &Tap = &row.tap;
        let mut x = String::from("<w:tr>");
        let mut tr = String::new();
        if row.tap.cant_split {
            tr.push_str("<w:cantSplit/>");
        }
        match t.height {
            Some(h) if h > 0 => {
                let _ = write!(tr, "<w:trHeight w:val=\"{h}\" w:hRule=\"atLeast\"/>");
            }
            Some(h) if h < 0 => {
                let _ = write!(
                    tr,
                    "<w:trHeight w:val=\"{}\" w:hRule=\"exact\"/>",
                    -(h as i32)
                );
            }
            _ => {}
        }
        if t.header {
            tr.push_str("<w:tblHeader/>");
        }
        x.push_str(&wrap("w:trPr", &tr));
        // Cells with their boundaries; horizontally merged ones join the
        // first, zero-width ones the previous.
        struct C<'a> {
            a: i32,
            b: i32,
            k: usize,
            paras: Vec<&'a Para>,
        }
        let mut cells: Vec<C> = Vec::new();
        let has_def = t.centers.len() >= 2;
        for (k, paras) in row.cells.iter().enumerate() {
            let (a, b) = if has_def {
                let a = t.centers.get(k).copied().map(i32::from);
                let b = t.centers.get(k + 1).copied().map(i32::from);
                match (a, b, cells.last()) {
                    (Some(a), Some(b), _) => (a, b),
                    (_, _, Some(prev)) => (prev.b, prev.b),
                    _ => (xs[0], xs[xs.len() - 1]),
                }
            } else {
                (xs[k.min(xs.len() - 2)], xs[(k + 1).min(xs.len() - 1)])
            };
            let merged = t.cells.get(k).is_some_and(|c| c.merged && !c.first_merged);
            if let Some(prev) = cells.last_mut()
                && (merged || b <= a)
            {
                prev.b = prev.b.max(b);
                prev.paras
                    .extend(paras.iter().filter(|p| !p.items.is_empty()));
                continue;
            }
            cells.push(C {
                a,
                b,
                k,
                paras: paras.iter().collect(),
            });
        }
        // A row starting right of the table's left edge: a blank cell.
        if let Some(c) = cells.first()
            && col(c.a) > 0
        {
            let span = col(c.a);
            let w = c.a - xs[0];
            let _ = write!(
                x,
                "<w:tc><w:tcPr><w:tcW w:w=\"{w}\" w:type=\"dxa\"/>{}<w:tcBorders><w:top w:val=\"nil\"/>\
                 <w:left w:val=\"nil\"/><w:bottom w:val=\"nil\"/><w:right w:val=\"nil\"/></w:tcBorders></w:tcPr><w:p/></w:tc>",
                if span > 1 {
                    format!("<w:gridSpan w:val=\"{span}\"/>")
                } else {
                    String::new()
                }
            );
        }
        for c in &cells {
            let span = col(c.b).saturating_sub(col(c.a)).max(1);
            let tc = t.cells.get(c.k).cloned().unwrap_or_default();
            let mut pr = format!("<w:tcW w:w=\"{}\" w:type=\"dxa\"/>", (c.b - c.a).max(0));
            if span > 1 {
                let _ = write!(pr, "<w:gridSpan w:val=\"{span}\"/>");
            }
            if tc.vert_merge {
                pr.push_str(if tc.vert_restart {
                    "<w:vMerge w:val=\"restart\"/>"
                } else {
                    "<w:vMerge/>"
                });
            }
            pr.push_str(&wrap(
                "w:tcBorders",
                &borders_xml(&tc.borders.map(|b| b), &["top", "left", "bottom", "right"]),
            ));
            if let Some(f) = tc.fill {
                let _ = write!(
                    pr,
                    "<w:shd w:val=\"clear\" w:color=\"auto\" w:fill=\"{}\"/>",
                    hex(Some(f))
                );
            }
            if tc.valign != 0 {
                let v = if tc.valign == 1 { "center" } else { "bottom" };
                let _ = write!(pr, "<w:vAlign w:val=\"{v}\"/>");
            }
            let _ = write!(x, "<w:tc><w:tcPr>{pr}</w:tcPr>");
            if c.paras.is_empty() {
                x.push_str("<w:p/>");
            }
            for p in &c.paras {
                x.push_str(&self.para(p, &[]));
            }
            x.push_str("</w:tc>");
        }
        if cells.is_empty() {
            let _ = write!(
                x,
                "<w:tc><w:tcPr><w:tcW w:w=\"0\" w:type=\"auto\"/></w:tcPr><w:p/></w:tc>"
            );
        }
        x.push_str("</w:tr>");
        x
    }

    fn styles_xml(&self) -> String {
        let d = self.doc;
        let f = d.default_fonts;
        let mut x = format!(
            "<w:styles {NS}><w:docDefaults><w:rPrDefault><w:rPr>{}<w:sz w:val=\"20\"/><w:szCs w:val=\"20\"/>\
             </w:rPr></w:rPrDefault><w:pPrDefault><w:pPr/></w:pPrDefault></w:docDefaults>",
            self.fonts_xml([f[0], f[1], f[2], None])
        );
        let mut have_normal = false;
        for (i, s) in d.styles.iter().enumerate() {
            let (Some(s), Some(id)) = (s, self.style_ids.get(i).and_then(|s| s.as_deref())) else {
                continue;
            };
            let kind = match s.kind {
                2 => "character",
                3 => "table",
                4 => "numbering",
                _ => "paragraph",
            };
            let default = (i == 0 && s.kind == 1) || s.sti == 65 || s.sti == 105 || s.sti == 107;
            have_normal |= id == "Normal";
            let name = if s.name.is_empty() { id } else { &s.name };
            let _ = write!(
                x,
                "<w:style w:type=\"{kind}\"{} w:styleId=\"{}\"><w:name w:val=\"{}\"/>",
                if default { " w:default=\"1\"" } else { "" },
                esc(id),
                esc(name)
            );
            if let Some(b) = s.base.and_then(|b| self.sid(b)).filter(|b| *b != id) {
                let _ = write!(x, "<w:basedOn w:val=\"{}\"/>", esc(b));
            }
            if s.kind == 1
                && let Some(n) = s.next.and_then(|n| self.sid(n))
            {
                let _ = write!(x, "<w:next w:val=\"{}\"/>", esc(n));
            }
            if s.kind != 2 {
                let mut p = Pap::default();
                p.apply(&s.pap);
                x.push_str(&wrap("w:pPr", &self.ppr_inner(&p, false)));
            }
            let mut c = Chp::default();
            c.apply(&s.chp);
            c.istd = None;
            x.push_str(&wrap("w:rPr", &self.rpr(&c, 0)));
            x.push_str("</w:style>");
        }
        if !have_normal {
            x.push_str(
                "<w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/></w:style>",
            );
        }
        x.push_str("</w:styles>");
        x
    }

    fn numbering_xml(&self) -> Option<String> {
        let d = self.doc;
        if d.lists.is_empty() || d.lfos.is_empty() {
            return None;
        }
        let mut x = format!("<w:numbering {NS}>");
        for (a, l) in d.lists.iter().enumerate() {
            let kind = if l.levels.len() == 1 {
                "singleLevel"
            } else if l.hybrid {
                "hybridMultilevel"
            } else {
                "multilevel"
            };
            let _ = write!(
                x,
                "<w:abstractNum w:abstractNumId=\"{a}\"><w:multiLevelType w:val=\"{kind}\"/>"
            );
            for (i, lv) in l.levels.iter().enumerate() {
                let fmt = match lv.nfc {
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
                };
                let _ = write!(
                    x,
                    "<w:lvl w:ilvl=\"{i}\"><w:start w:val=\"{}\"/><w:numFmt w:val=\"{fmt}\"/>",
                    lv.start
                );
                if lv.legal {
                    x.push_str("<w:isLgl/>");
                }
                match lv.follow {
                    1 => x.push_str("<w:suff w:val=\"space\"/>"),
                    2 => x.push_str("<w:suff w:val=\"nothing\"/>"),
                    _ => {}
                }
                let jc = match lv.jc {
                    1 => "center",
                    2 => "right",
                    _ => "left",
                };
                let _ = write!(
                    x,
                    "<w:lvlText w:val=\"{}\"/><w:lvlJc w:val=\"{jc}\"/>",
                    esc(&lv.text)
                );
                let mut p = Pap::default();
                p.apply(&lv.pap);
                x.push_str(&wrap("w:pPr", &self.ppr_inner(&p, false)));
                let mut c = Chp::default();
                c.apply(&lv.chp);
                c.istd = None;
                x.push_str(&wrap("w:rPr", &self.rpr(&c, 0)));
                x.push_str("</w:lvl>");
            }
            x.push_str("</w:abstractNum>");
        }
        for (i, lfo) in d.lfos.iter().enumerate() {
            let Some(a) = d.lists.iter().position(|l| l.lsid == lfo.lsid) else {
                continue;
            };
            let _ = write!(
                x,
                "<w:num w:numId=\"{}\"><w:abstractNumId w:val=\"{a}\"/>",
                i + 1
            );
            for &(lvl, start) in &lfo.starts {
                let _ = write!(
                    x,
                    "<w:lvlOverride w:ilvl=\"{}\"><w:startOverride w:val=\"{start}\"/></w:lvlOverride>",
                    lvl.min(8)
                );
            }
            x.push_str("</w:num>");
        }
        x.push_str("</w:numbering>");
        Some(x)
    }
}

fn wrap(tag: &str, inner: &str) -> String {
    if inner.is_empty() {
        String::new()
    } else {
        format!("<{tag}>{inner}</{tag}>")
    }
}

fn hex(c: Color) -> String {
    match c {
        Some([r, g, b]) => format!("{r:02X}{g:02X}{b:02X}"),
        None => "auto".into(),
    }
}

fn borders_xml(b: &[Option<Brd>], names: &[&str]) -> String {
    let mut x = String::new();
    for (side, name) in b.iter().zip(names) {
        let Some(b) = side else { continue };
        let v = match b.kind {
            0 | 0xFF => "nil",
            3 => "double",
            6 => "dotted",
            7 => "dashed",
            8 => "dotDash",
            9 => "dotDotDash",
            10 => "triple",
            22 => "dashSmallGap",
            _ => "single",
        };
        if v == "nil" {
            let _ = write!(x, "<w:{name} w:val=\"nil\"/>");
        } else {
            let _ = write!(
                x,
                "<w:{name} w:val=\"{v}\" w:sz=\"{}\" w:space=\"0\" w:color=\"{}\"/>",
                b.size.max(2),
                hex(b.color)
            );
        }
    }
    x
}

fn tabs_xml(tabs: &[Tab]) -> String {
    let mut x = String::new();
    for t in tabs {
        let v = if t.clear {
            "clear"
        } else {
            match t.jc {
                1 => "center",
                2 => "right",
                3 => "decimal",
                4 => "bar",
                _ => "left",
            }
        };
        let leader = match t.leader {
            1 => " w:leader=\"dot\"",
            2 => " w:leader=\"hyphen\"",
            3 => " w:leader=\"underscore\"",
            4 => " w:leader=\"heavy\"",
            5 => " w:leader=\"middleDot\"",
            _ => "",
        };
        let _ = write!(x, "<w:tab w:val=\"{v}\"{leader} w:pos=\"{}\"/>", t.pos);
    }
    wrap("w:tabs", &x)
}

/// A few language ids, for spell checking.
fn lang(lid: u16) -> Option<&'static str> {
    Some(match lid {
        0x0409 => "en-US",
        0x0809 => "en-GB",
        0x0C09 => "en-AU",
        0x0421 => "id-ID",
        0x043E => "ms-MY",
        _ => return None,
    })
}

fn zip(w: &W, document: &str) -> Result<Vec<u8>> {
    let mut out = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let decl = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\r\n";
    let mut ct = String::from(
        "<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
         <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
         <Default Extension=\"xml\" ContentType=\"application/xml\"/>\
         <Default Extension=\"png\" ContentType=\"image/png\"/>\
         <Default Extension=\"jpeg\" ContentType=\"image/jpeg\"/>",
    );
    let _ = write!(
        ct,
        "<Override PartName=\"/word/document.xml\" ContentType=\"{CT}.document.main+xml\"/>"
    );
    for (name, kind, _) in &w.parts {
        let _ = write!(
            ct,
            "<Override PartName=\"/{name}\" ContentType=\"{kind}\"/>"
        );
    }
    ct.push_str("</Types>");
    let mut files: Vec<(String, Vec<u8>)> = vec![
        ("[Content_Types].xml".into(), format!("{decl}{ct}").into_bytes()),
        (
            "_rels/.rels".into(),
            format!(
                "{decl}<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
                 <Relationship Id=\"rId1\" Type=\"{REL}/officeDocument\" Target=\"word/document.xml\"/></Relationships>"
            )
            .into_bytes(),
        ),
        ("word/document.xml".into(), format!("{decl}{document}").into_bytes()),
    ];
    let mut rels = String::from(
        "<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">",
    );
    for r in &w.rels {
        let _ = write!(
            rels,
            "<Relationship Id=\"{}\" Type=\"{REL}/{}\" Target=\"{}\"{}/>",
            r.id,
            r.kind,
            esc(&r.target),
            if r.external {
                " TargetMode=\"External\""
            } else {
                ""
            }
        );
    }
    rels.push_str("</Relationships>");
    files.push((
        "word/_rels/document.xml.rels".into(),
        format!("{decl}{rels}").into_bytes(),
    ));
    for (name, _, xml) in &w.parts {
        files.push((name.clone(), format!("{decl}{xml}").into_bytes()));
    }
    for (m, (ext, bytes)) in w.doc.media.iter().enumerate() {
        if w.media_rels[m].is_some() {
            files.push((format!("word/media/image{}.{ext}", m + 1), bytes.clone()));
        }
    }
    for (name, bytes) in files {
        out.start_file(name, opts)?;
        out.write_all(&bytes)?;
    }
    Ok(out.finish()?.into_inner())
}
